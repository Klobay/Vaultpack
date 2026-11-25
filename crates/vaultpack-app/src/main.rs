#![cfg_attr(windows, windows_subsystem = "windows")]

slint::include_modules!();

mod settings;
mod tr_table;
mod tree;
mod util;

use settings::{Profile, Settings};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tree::Tree;
use util::*;
use vaultpack_core as vp;
use vaultpack_core::presets::{self, BrowserProfile};
use vaultpack_core::{Archive, BackupOptions, CompressionLevel, ConflictPolicy, Control, Env, Event, Remap, RestoreOptions, Source, Summary, System, VaultError};

#[derive(PartialEq, Clone, Copy)]
enum Job {
    None,
    Backup,
    Restore,
}

struct ItemState {
    path: PathBuf,
    info: Option<vp::ItemInfo>,
}

struct State {
    items: Vec<ItemState>,
    browsers: Vec<BrowserProfile>,
    browser_sel: Vec<bool>,
    archive: Option<Arc<Archive>>,
    archive_path: Option<PathBuf>,
    tree: Option<Tree>,
    ctl: Option<Arc<Control>>,
    rx: Option<Receiver<Event>>,
    job: Job,
    scan_tx: Sender<(PathBuf, vp::ItemInfo)>,
    scan_rx: Receiver<(PathBuf, vp::ItemInfo)>,
    started: Instant,
    last_t: Instant,
    last_done: u64,
    speed: f64,
    total_files: u64,
    settings: Settings,
    sys: Arc<dyn System>,
    env: Env,
    elevated: bool,
    last_log: String,
    manual: Vec<presets::Program>,
    password_used: Option<String>,
}

type Shared = Rc<RefCell<State>>;

fn level_of(i: i32) -> CompressionLevel {
    CompressionLevel::from_index(i)
}

fn refresh_items(app: &VaultpackWindow, st: &State) {
    let rows: Vec<ItemRow> = st
        .items
        .iter()
        .map(|it| {
            let name = it.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| it.path.display().to_string());
            let (detail, folder) = match &it.info {
                Some(i) => (format!("{} \u{2022} {} {}", fmt_size(i.size), i.files, if i.files == 1 { "file" } else { "files" }), i.is_dir),
                None => ("Scanning...".to_string(), it.path.is_dir()),
            };
            ItemRow { name: name.into(), detail: detail.into(), folder }
        })
        .collect();
    app.global::<App>().set_items(ModelRc::new(VecModel::from(rows)));
}

fn set_status(app: &VaultpackWindow, kind: i32, text: &str) {
    let a = app.global::<App>();
    a.set_status_kind(kind);
    a.set_status(text.into());
}

fn current_excludes(app: &VaultpackWindow) -> Vec<String> {
    split_rules(app.global::<App>().get_excludes().as_str())
}

fn update_scan_status(app: &VaultpackWindow, st: &State) {
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut pending = 0;
    for it in &st.items {
        match &it.info {
            Some(i) => {
                files += i.files;
                bytes += i.size;
            }
            None => pending += 1,
        }
    }
    let a = app.global::<App>();
    if st.items.is_empty() {
        a.set_scan_info("".into());
        set_status(app, 0, "Ready");
    } else if pending > 0 {
        set_status(app, 1, "Scanning selection...");
    } else {
        a.set_scan_info(format!("{} files, {}", files, fmt_size(bytes)).into());
        set_status(app, 0, "Ready");
    }
}

fn add_paths(app: &VaultpackWindow, st: &Shared, paths: Vec<PathBuf>) {
    let excludes = current_excludes(app);
    {
        let mut s = st.borrow_mut();
        for p in paths {
            if s.items.iter().any(|i| i.path == p) {
                continue;
            }
            s.items.push(ItemState { path: p.clone(), info: None });
            let tx = s.scan_tx.clone();
            let ex = excludes.clone();
            std::thread::spawn(move || {
                let set = vp::backup::exclude_set(&ex).unwrap_or_else(|_| vp::backup::exclude_set(&[]).unwrap());
                let info = vp::scan_item(&p, &set);
                let _ = tx.send((p, info));
            });
        }
    }
    let s = st.borrow();
    refresh_items(app, &s);
    update_scan_status(app, &s);
}

fn browser_rows(app: &VaultpackWindow, st: &State) {
    let rows: Vec<BrowserRow> = st
        .browsers
        .iter()
        .zip(st.browser_sel.iter())
        .map(|(b, &c)| BrowserRow { label: format!("{} \u{2014} {}", b.browser, b.name).into(), size: fmt_size(b.size).into(), checked: c, running: b.running })
        .collect();
    let a = app.global::<App>();
    a.set_browser_running(st.browsers.iter().any(|b| b.running));
    a.set_browser_rows(ModelRc::new(VecModel::from(rows)));
}

fn remap_of(app: &VaultpackWindow) -> Remap {
    let a = app.global::<App>();
    let root = a.get_restore_root().to_string();
    Remap { drives: parse_drive_map(a.get_drive_map().as_str()), root: if root.trim().is_empty() { None } else { Some(PathBuf::from(root.trim())) } }
}

fn refresh_tree(app: &VaultpackWindow, st: &Shared) {
    let mut s = st.borrow_mut();
    let archive = match &s.archive {
        Some(a) => a.clone(),
        None => return,
    };
    let Some(tree) = s.tree.as_mut() else { return };
    let rows = tree.rows();
    let slint_rows: Vec<TreeRow> = rows
        .iter()
        .map(|r| {
            let n = &tree.nodes[r.node];
            TreeRow { depth: r.depth, label: n.name.clone().into(), size: r.size.clone().into(), state: r.state, has_children: !n.children.is_empty(), expanded: n.expanded, icon: n.icon }
        })
        .collect();
    let (count, size) = tree.selected_stats(&archive);
    let a = app.global::<App>();
    a.set_tree_rows(ModelRc::new(VecModel::from(slint_rows)));
    a.set_restore_footer(format!("{}: {} {} \u{2022} {}", "Selected", count, "files", fmt_size(size)).into());
}

fn check_admin_needs(app: &VaultpackWindow, st: &State) {
    let a = app.global::<App>();
    let Some(archive) = &st.archive else { return };
    if st.elevated {
        a.set_need_admin(false);
        return;
    }
    let remap = remap_of(app);
    let wifi = archive.manifest.entries.iter().any(|e| e.group.as_deref().map_or(false, |g| g.starts_with("wifi/")));
    let protected = remap.root.is_none() && archive.manifest.entries.iter().any(|e| !e.virtual_entry && vp::paths::is_protected(&st.env, &vp::target_path(e, &st.env, &remap).to_string_lossy()));
    if wifi || protected {
        a.set_need_admin(true);
        a.set_status(if wifi { "Wi-Fi profiles and protected locations need administrator rights.".into() } else { "Some targets are in protected locations and need administrator rights.".into() });
    } else {
        a.set_need_admin(false);
    }
}

fn load_archive(app: &VaultpackWindow, st: &Shared, path: &Path, password: Option<&str>) {
    let a = app.global::<App>();
    a.set_page(1);
    match vp::is_encrypted(path) {
        Err(e) => {
            a.set_archive_open(false);
            a.set_needs_password(false);
            set_status(app, 2, &e.to_string());
            show_summary(app, "Cannot open archive", &e.to_string(), false);
            return;
        }
        Ok(enc) => {
            if enc && password.is_none() {
                st.borrow_mut().archive_path = Some(path.to_path_buf());
                a.set_needs_password(true);
                a.set_archive_open(false);
                a.set_archive_password("".into());
                a.set_status("".into());
                return;
            }
        }
    }
    match vp::open(path, password) {
        Ok(arc) => {
            {
                let mut s = st.borrow_mut();
                s.tree = Some(Tree::build(&arc));
                s.archive = Some(Arc::new(arc));
                s.archive_path = Some(path.to_path_buf());
                s.password_used = password.map(|p| p.to_string());
            }
            a.set_needs_password(false);
            a.set_archive_open(true);
            a.set_archive_name(path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().into());
            a.set_search("".into());
            a.set_resolved_path("".into());
            refresh_tree(app, st);
            check_admin_needs(app, &st.borrow());
        }
        Err(VaultError::WrongPassword) => {
            a.set_needs_password(true);
            a.set_status("Wrong password".into());
        }
        Err(e) => {
            a.set_needs_password(false);
            a.set_archive_open(false);
            show_summary(app, "Cannot open archive", &e.to_string(), false);
        }
    }
}

fn show_summary(app: &VaultpackWindow, title: &str, text: &str, manual: bool) {
    let a = app.global::<App>();
    a.set_busy(false);
    a.set_summary_title(title.into());
    a.set_summary_text(text.into());
    a.set_summary_has_manual(manual);
    a.set_summary_open(true);
}

fn start_job_ui(app: &VaultpackWindow, title: &str) {
    let a = app.global::<App>();
    a.set_job_title(title.into());
    a.set_job_status("".into());
    a.set_overall(0.0);
    a.set_file_progress(0.0);
    a.set_current_file("".into());
    a.set_speed("0 B/s".into());
    a.set_eta("--:--".into());
    a.set_counters("".into());
    a.set_paused(false);
    a.set_busy(true);
}

fn gather_sources(items: Vec<PathBuf>, browsers: Vec<BrowserProfile>, want_wifi: bool, want_programs: bool, sys: &dyn System, tx: &Sender<Event>) -> Result<Vec<Source>, VaultError> {
    let mut sources: Vec<Source> = items.into_iter().map(|p| Source::Path { path: p, group: None }).collect();
    for b in browsers {
        let group = b.group();
        for p in b.paths() {
            sources.push(Source::Path { path: p, group: Some(group.clone()) });
        }
    }
    if want_wifi {
        let _ = tx.send(Event::Status("Exporting Wi-Fi profiles".into()));
        let scratch = std::env::temp_dir().join(format!("vaultpack-wifi-{}", std::process::id()));
        for w in presets::export_wifi(sys, &scratch)? {
            let safe: String = w.name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == ' ' { c } else { '_' }).collect();
            sources.push(Source::Virtual { name: format!("{}.xml", safe), data: w.xml, group: format!("wifi/{}", safe) });
        }
    }
    if want_programs {
        let _ = tx.send(Event::Status("Reading installed programs".into()));
        for p in presets::list_programs(sys) {
            let safe: String = p.name.chars().map(|c| if c == '/' || c == '\\' || c == ':' { '_' } else { c }).collect();
            sources.push(Source::Virtual { name: format!("{}.json", safe), data: serde_json::to_vec(&p)?, group: format!("programs/{}", safe) });
        }
    }
    Ok(sources)
}

fn start_backup(app: &VaultpackWindow, st: &Shared) {
    let a = app.global::<App>();
    let (want_browsers, want_wifi, want_programs) = (a.get_preset_browsers(), a.get_preset_wifi(), a.get_preset_programs());
    let password = a.get_password().to_string();
    let (items, browsers, sys, env, elevated, out_dir) = {
        let s = st.borrow();
        let items: Vec<PathBuf> = s.items.iter().map(|i| i.path.clone()).collect();
        let browsers: Vec<BrowserProfile> = if want_browsers { s.browsers.iter().zip(s.browser_sel.iter()).filter(|(_, &c)| c).map(|(b, _)| b.clone()).collect() } else { Vec::new() };
        (items, browsers, s.sys.clone(), s.env.clone(), s.elevated, s.settings.out_dir.clone())
    };
    if items.is_empty() && browsers.is_empty() && !want_wifi && !want_programs {
        set_status(app, 1, "Add files, folders or a preset first");
        return;
    }
    if want_wifi && !elevated {
        a.set_need_admin(true);
        set_status(app, 1, "Exporting Wi-Fi keys needs administrator rights.");
        a.set_need_admin(true);
        return;
    }
    if browsers.iter().any(|b| b.running) {
        set_status(app, 1, "A selected browser is running - locked files may be skipped");
    }
    let mut dialog = rfd::FileDialog::new().add_filter("Vaultpack archive", &["vpk"]).set_file_name(format!("Vaultpack-{}.vpk", date_stamp(vp::format::now_secs())));
    if !out_dir.is_empty() {
        dialog = dialog.set_directory(&out_dir);
    }
    let Some(mut dest) = dialog.save_file() else { return };
    if dest.extension().is_none() {
        dest.set_extension("vpk");
    }
    let excludes = current_excludes(app);
    let level = level_of(a.get_level());
    let (tx, rx) = channel();
    let ctl = Control::new();
    {
        let mut s = st.borrow_mut();
        s.rx = Some(rx);
        s.ctl = Some(ctl.clone());
        s.job = Job::Backup;
        s.started = Instant::now();
        s.last_t = Instant::now();
        s.last_done = 0;
        s.speed = 0.0;
    }
    start_job_ui(app, "Backing up");
    let pw = if password.is_empty() { None } else { Some(password) };
    std::thread::spawn(move || {
        let result = (|| {
            let sources = gather_sources(items, browsers, want_wifi, want_programs, sys.as_ref(), &tx)?;
            let opts = BackupOptions { sources, destination: dest, level, password: pw, excludes, use_vss: elevated, verify: true, env, system: sys };
            vp::run_backup(&opts, &ctl, &tx)
        })();
        let _ = match result {
            Ok(s) => tx.send(Event::Done(s)),
            Err(e) => tx.send(Event::Failed(e.to_string())),
        };
    });
}

fn start_restore(app: &VaultpackWindow, st: &Shared) {
    let a = app.global::<App>();
    let (archive, selected, sys, env, elevated) = {
        let s = st.borrow();
        let Some(arc) = s.archive.clone() else { return };
        let Some(tree) = s.tree.as_ref() else { return };
        (arc, tree.selected_set(), s.sys.clone(), s.env.clone(), s.elevated)
    };
    if selected.is_empty() {
        a.set_resolved_path("Nothing selected".into());
        return;
    }
    let _ = elevated;
    let opts = RestoreOptions { selected: Some(selected), policy: ConflictPolicy::from_index(a.get_policy()), remap: remap_of(app), env, system: sys, scratch: std::env::temp_dir().join(format!("vaultpack-wifi-{}", std::process::id())) };
    let (tx, rx) = channel();
    let ctl = Control::new();
    {
        let mut s = st.borrow_mut();
        s.rx = Some(rx);
        s.ctl = Some(ctl.clone());
        s.job = Job::Restore;
        s.started = Instant::now();
        s.last_t = Instant::now();
        s.last_done = 0;
        s.speed = 0.0;
    }
    start_job_ui(app, "Restoring");
    std::thread::spawn(move || {
        let _ = match vp::run_restore(&archive, &opts, &ctl, &tx) {
            Ok(s) => tx.send(Event::Done(s)),
            Err(e) => tx.send(Event::Failed(e.to_string())),
        };
    });
}

fn pump(app: &VaultpackWindow, st: &Shared) {
    let mut scans = Vec::new();
    {
        let s = st.borrow();
        while let Ok(x) = s.scan_rx.try_recv() {
            scans.push(x);
        }
    }
    if !scans.is_empty() {
        let mut s = st.borrow_mut();
        for (p, info) in scans {
            if let Some(it) = s.items.iter_mut().find(|i| i.path == p) {
                it.info = Some(info);
            }
        }
        refresh_items(app, &s);
        update_scan_status(app, &s);
    }
    let mut events = Vec::new();
    {
        let s = st.borrow();
        if let Some(rx) = &s.rx {
            while let Ok(e) = rx.try_recv() {
                events.push(e);
            }
        }
    }
    let a = app.global::<App>();
    for ev in events {
        match ev {
            Event::Status(t) => a.set_job_status(t.into()),
            Event::Scan { files, bytes } => {
                st.borrow_mut().total_files = files;
                a.set_job_status(format!("{} files, {}", files, fmt_size(bytes)).into());
            }
            Event::Warning(w) => a.set_job_status(w.into()),
            Event::File { name, index, total } => {
                a.set_current_file(name.into());
                a.set_counters(format!("{} / {}", index, total).into());
                a.set_job_status("".into());
            }
            Event::Bytes { done, total, file_done, file_total } => {
                let mut s = st.borrow_mut();
                let now = Instant::now();
                let dt = now.duration_since(s.last_t).as_secs_f64();
                if dt >= 0.4 {
                    let inst = done.saturating_sub(s.last_done) as f64 / dt;
                    s.speed = if s.speed == 0.0 { inst } else { s.speed * 0.7 + inst * 0.3 };
                    s.last_t = now;
                    s.last_done = done;
                }
                let eta = if s.speed > 1.0 { total.saturating_sub(done) as f64 / s.speed } else { f64::INFINITY };
                a.set_overall(if total == 0 { 0.0 } else { (done as f64 / total as f64) as f32 });
                a.set_file_progress(if file_total == 0 { 0.0 } else { (file_done as f64 / file_total as f64) as f32 });
                a.set_speed(format!("{}/s", fmt_size(s.speed as u64)).into());
                a.set_eta(fmt_eta(eta).into());
            }
            Event::Done(sum) => finish(app, st, Ok(sum)),
            Event::Failed(msg) => finish(app, st, Err(msg)),
        }
    }
}

fn finish(app: &VaultpackWindow, st: &Shared, result: Result<Summary, String>) {
    let job = {
        let mut s = st.borrow_mut();
        let j = s.job;
        s.job = Job::None;
        s.rx = None;
        s.ctl = None;
        j
    };
    match result {
        Ok(sum) => {
            let title = match job {
                Job::Backup => "Backup complete",
                _ => "Restore complete",
            };
            let mut text = String::new();
            if let Some(o) = &sum.output {
                text.push_str(&format!("Archive: {}\n", o.display()));
            }
            text.push_str(&format!("Restored/archived: {}\n", sum.done));
            text.push_str(&format!("Skipped: {}\nFailed: {}\n\n", sum.skipped.len(), sum.failed.len()));
            for (p, r) in sum.skipped.iter().take(200) {
                text.push_str(&format!("SKIPPED  {}  ({})\n", p, r));
            }
            for (p, r) in sum.failed.iter().take(200) {
                text.push_str(&format!("FAILED   {}  ({})\n", p, r));
            }
            for w in &sum.warnings {
                text.push_str(&format!("WARNING  {}\n", w));
            }
            if !sum.manual_programs.is_empty() {
                text.push_str(&format!("\nManual reinstall needed ({}):\n{}\n", sum.manual_programs.len(), presets::programs_text(&sum.manual_programs)));
            }
            let manual = !sum.manual_programs.is_empty();
            {
                let mut s = st.borrow_mut();
                s.last_log = sum.log();
                s.manual = sum.manual_programs.clone();
            }
            show_summary(app, title, &text, manual);
        }
        Err(msg) => {
            let cancelled = msg.contains("cancelled");
            show_summary(app, if cancelled { "Cancelled" } else { "Failed" }, &msg, false);
            st.borrow_mut().last_log = msg;
        }
    }
}

fn profile_from_ui(app: &VaultpackWindow, st: &State) -> Profile {
    let a = app.global::<App>();
    Profile { items: st.items.iter().map(|i| i.path.to_string_lossy().to_string()).collect(), excludes: a.get_excludes().to_string(), browsers: a.get_preset_browsers(), wifi: a.get_preset_wifi(), programs: a.get_preset_programs(), level: a.get_level() }
}

fn apply_profile(app: &VaultpackWindow, st: &Shared, p: Profile) {
    {
        let mut s = st.borrow_mut();
        s.items.clear();
    }
    let a = app.global::<App>();
    a.set_excludes(p.excludes.into());
    a.set_preset_browsers(p.browsers);
    a.set_preset_wifi(p.wifi);
    a.set_preset_programs(p.programs);
    a.set_level(p.level);
    add_paths(app, st, p.items.into_iter().map(PathBuf::from).collect());
}

fn apply_settings(app: &VaultpackWindow, s: &Settings) {
    let a = app.global::<App>();
    let ar = s.language == 1;
    app.global::<Theme>().set_rtl(ar);
    tr_table::apply_tr(&app.global::<Tr>(), ar);
    a.set_out_dir(s.out_dir.clone().into());
    a.set_default_level(s.level);
    a.set_default_excludes(s.excludes.clone().into());
    a.set_theme_index(s.theme);
    a.set_language_index(s.language);
    a.set_default_policy(s.policy);
}

fn resize_dir(i: i32) -> slint::winit_030::winit::window::ResizeDirection {
    use slint::winit_030::winit::window::ResizeDirection as R;
    match i {
        0 => R::East,
        1 => R::West,
        2 => R::South,
        3 => R::SouthEast,
        _ => R::SouthWest,
    }
}

/// Re-applies the Mica/Acrylic backdrop so it matches the theme that is actually showing.
#[cfg(windows)]
fn apply_backdrop(app: &VaultpackWindow) {
    use slint::winit_030::WinitWindowAccessor;
    let mode = app.global::<App>().get_theme_index();
    let sys_dark = app.global::<Theme>().get_system_dark();
    let dark = mode == 1 || (mode == 0 && sys_dark);
    let tint = if dark { (28, 28, 30, 160) } else { (243, 244, 246, 160) };
    let ok = app
        .window()
        .with_winit_window(|win| window_vibrancy::apply_mica(win, Some(dark)).or_else(|_| window_vibrancy::apply_acrylic(win, Some(tint))).is_ok())
        .unwrap_or(false);
    app.global::<Theme>().set_glass(ok);
}

/// Windows blocks drag & drop from a normal Explorer into an elevated (administrator) window
/// unless the elevated process explicitly lets those messages through (UIPI).
#[cfg(windows)]
fn allow_drops_when_elevated(app: &VaultpackWindow) {
    use slint::winit_030::WinitWindowAccessor;
    use slint::winit_030::winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    extern "system" {
        fn ChangeWindowMessageFilterEx(hwnd: isize, message: u32, action: u32, change_filter_struct: *mut core::ffi::c_void) -> i32;
        fn ChangeWindowMessageFilter(message: u32, flag: u32) -> i32;
    }
    const MSGFLT_ADD: u32 = 1;
    const MSGFLT_ALLOW: u32 = 1;
    // WM_DROPFILES, WM_COPYDATA, WM_COPYGLOBALDATA
    const MSGS: [u32; 3] = [0x0233, 0x004A, 0x0049];
    let hwnd = app
        .window()
        .with_winit_window(|win| match win.window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
            _ => None,
        })
        .flatten();
    unsafe {
        for m in MSGS {
            ChangeWindowMessageFilter(m, MSGFLT_ADD);
            if let Some(h) = hwnd {
                ChangeWindowMessageFilterEx(h, m, MSGFLT_ALLOW, core::ptr::null_mut());
            }
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    use slint::winit_030::{winit, EventResult, WinitWindowAccessor};
    let app = VaultpackWindow::new()?;
    let sys: Arc<dyn System> = Arc::new(vp::RealSystem);
    let env = Env::from_system();
    let elevated = sys.is_elevated();
    let settings = settings::load();
    let (scan_tx, scan_rx) = channel();
    let st: Shared = Rc::new(RefCell::new(State {
        items: Vec::new(),
        browsers: Vec::new(),
        browser_sel: Vec::new(),
        archive: None,
        archive_path: None,
        tree: None,
        ctl: None,
        rx: None,
        job: Job::None,
        scan_tx,
        scan_rx,
        started: Instant::now(),
        last_t: Instant::now(),
        last_done: 0,
        speed: 0.0,
        total_files: 0,
        settings: settings.clone(),
        sys: sys.clone(),
        env: env.clone(),
        elevated,
        last_log: String::new(),
        manual: Vec::new(),
        password_used: None,
    }));
    apply_settings(&app, &settings);
    {
        let a = app.global::<App>();
        a.set_level(settings.level);
        a.set_excludes(settings.excludes.clone().into());
        a.set_policy(settings.policy);
    }

    let w = app.as_weak();
    app.global::<App>().on_minimize({
        let w = w.clone();
        move || {
            if let Some(a) = w.upgrade() {
                a.window().set_minimized(true);
            }
        }
    });
    app.global::<App>().on_toggle_maximize({
        let w = w.clone();
        move || {
            if let Some(a) = w.upgrade() {
                let m = !a.window().is_maximized();
                a.window().set_maximized(m);
                a.global::<App>().set_maximized(m);
            }
        }
    });
    app.global::<App>().on_close_window(|| {
        let _ = slint::quit_event_loop();
    });
    app.global::<App>().on_drag_window({
        let w = w.clone();
        move || {
            if let Some(a) = w.upgrade() {
                a.window().with_winit_window(|win| {
                    let _ = win.drag_window();
                });
            }
        }
    });
    app.global::<App>().on_resize_window({
        let w = w.clone();
        move |d| {
            if let Some(a) = w.upgrade() {
                a.window().with_winit_window(|win| {
                    let _ = win.drag_resize_window(resize_dir(d));
                });
            }
        }
    });

    app.global::<App>().on_add_files({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let (Some(a), Some(files)) = (w.upgrade(), rfd::FileDialog::new().pick_files()) {
                add_paths(&a, &st, files);
            }
        }
    });
    app.global::<App>().on_add_folder({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let (Some(a), Some(dirs)) = (w.upgrade(), rfd::FileDialog::new().pick_folders()) {
                add_paths(&a, &st, dirs);
            }
        }
    });
    app.global::<App>().on_remove_item({
        let (w, st) = (w.clone(), st.clone());
        move |i| {
            if let Some(a) = w.upgrade() {
                {
                    let mut s = st.borrow_mut();
                    if (i as usize) < s.items.len() {
                        s.items.remove(i as usize);
                    }
                }
                let s = st.borrow();
                refresh_items(&a, &s);
                update_scan_status(&a, &s);
            }
        }
    });
    app.global::<App>().on_start_backup({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                start_backup(&a, &st);
            }
        }
    });
    app.global::<App>().on_save_profile({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Vaultpack profile", &["vpkprofile"]).set_file_name("profile.vpkprofile").save_file() {
                    let prof = profile_from_ui(&a, &st.borrow());
                    if let Ok(b) = serde_json::to_vec_pretty(&prof) {
                        let _ = std::fs::write(p, b);
                    }
                }
            }
        }
    });
    app.global::<App>().on_load_profile({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Vaultpack profile", &["vpkprofile"]).pick_file() {
                    if let Some(prof) = std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Profile>(&b).ok()) {
                        apply_profile(&a, &st, prof);
                    }
                }
            }
        }
    });
    app.global::<App>().on_preset_changed({
        let (w, st) = (w.clone(), st.clone());
        move |kind, on| {
            let Some(a) = w.upgrade() else { return };
            match kind {
                0 if on => {
                    let found = {
                        let s = st.borrow();
                        presets::detect_browsers(&s.env, s.sys.as_ref())
                    };
                    let mut s = st.borrow_mut();
                    s.browser_sel = vec![true; found.len()];
                    s.browsers = found;
                    browser_rows(&a, &s);
                }
                1 => {
                    let s = st.borrow();
                    if on && !s.elevated {
                        a.global::<App>().set_need_admin(true);
                        set_status(&a, 1, "Exporting Wi-Fi keys needs administrator rights.");
                    } else if !on {
                        a.global::<App>().set_need_admin(false);
                        set_status(&a, 0, "Ready");
                    }
                }
                _ => {}
            }
        }
    });
    app.global::<App>().on_browser_toggled({
        let (w, st) = (w.clone(), st.clone());
        move |i, v| {
            if let Some(a) = w.upgrade() {
                let mut s = st.borrow_mut();
                if let Some(x) = s.browser_sel.get_mut(i as usize) {
                    *x = v;
                }
                browser_rows(&a, &s);
            }
        }
    });
    app.global::<App>().on_close_browsers({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                let mut s = st.borrow_mut();
                let sys = s.sys.clone();
                let mut images: Vec<String> = s.browsers.iter().filter(|b| b.running).map(|b| b.image.clone()).collect();
                images.sort();
                images.dedup();
                for i in images {
                    sys.kill_process(&i);
                }
                for b in s.browsers.iter_mut() {
                    b.running = sys.process_running(&b.image);
                }
                browser_rows(&a, &s);
            }
        }
    });
    app.global::<App>().on_relaunch_admin({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                let prof = profile_from_ui(&a, &st.borrow());
                let _ = std::fs::write(settings::data_dir().join("session.json"), serde_json::to_vec(&prof).unwrap_or_default());
                let mut args = vec!["--resume".to_string()];
                if let Some(p) = &st.borrow().archive_path {
                    args = vec![p.to_string_lossy().to_string()];
                }
                if vp::relaunch_elevated(&args).is_ok() {
                    let _ = slint::quit_event_loop();
                }
            }
        }
    });
    app.global::<App>().on_add_exclude_preset({
        let w = w.clone();
        move |i| {
            if let Some(a) = w.upgrade() {
                let presets = vp::exclude_presets();
                if let Some((_, rules)) = presets.get(i as usize) {
                    let mut cur = split_rules(a.global::<App>().get_excludes().as_str());
                    for r in rules {
                        if !cur.iter().any(|c| c == r) {
                            cur.push(r.to_string());
                        }
                    }
                    a.global::<App>().set_excludes(cur.join(", ").into());
                }
            }
        }
    });

    app.global::<App>().on_open_archive({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Vaultpack archive", &["vpk"]).pick_file() {
                    load_archive(&a, &st, &p, None);
                }
            }
        }
    });
    app.global::<App>().on_close_archive({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                {
                    let mut s = st.borrow_mut();
                    s.archive = None;
                    s.tree = None;
                    s.archive_path = None;
                    s.password_used = None;
                }
                let g = a.global::<App>();
                g.set_archive_open(false);
                g.set_needs_password(false);
                g.set_archive_password("".into());
                g.set_search("".into());
                g.set_tree_rows(ModelRc::new(VecModel::from(Vec::<TreeRow>::new())));
                g.set_restore_footer("".into());
                g.set_resolved_path("".into());
                if !g.get_preset_wifi() {
                    g.set_need_admin(false);
                }
                set_status(&a, 0, "Ready");
            }
        }
    });
    app.global::<App>().on_unlock({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                let path = st.borrow().archive_path.clone();
                let pw = a.global::<App>().get_archive_password().to_string();
                if let Some(p) = path {
                    load_archive(&a, &st, &p, Some(&pw));
                }
            }
        }
    });
    app.global::<App>().on_search_changed({
        let (w, st) = (w.clone(), st.clone());
        move |q| {
            if let Some(a) = w.upgrade() {
                if let Some(t) = st.borrow_mut().tree.as_mut() {
                    t.set_query(q.as_str());
                }
                refresh_tree(&a, &st);
            }
        }
    });
    app.global::<App>().on_tree_check({
        let (w, st) = (w.clone(), st.clone());
        move |i| {
            if let Some(a) = w.upgrade() {
                {
                    let mut s = st.borrow_mut();
                    if let Some(t) = s.tree.as_mut() {
                        if let Some(&n) = t.visible.get(i as usize) {
                            t.toggle_check(n);
                        }
                    }
                }
                refresh_tree(&a, &st);
            }
        }
    });
    app.global::<App>().on_tree_expand({
        let (w, st) = (w.clone(), st.clone());
        move |i| {
            if let Some(a) = w.upgrade() {
                {
                    let mut s = st.borrow_mut();
                    if let Some(t) = s.tree.as_mut() {
                        if let Some(&n) = t.visible.get(i as usize) {
                            t.toggle_expand(n);
                        }
                    }
                }
                refresh_tree(&a, &st);
            }
        }
    });
    app.global::<App>().on_tree_select({
        let (w, st) = (w.clone(), st.clone());
        move |i| {
            let Some(a) = w.upgrade() else { return };
            let remap = remap_of(&a);
            let s = st.borrow();
            let (Some(arc), Some(t)) = (&s.archive, &s.tree) else { return };
            let Some(&n) = t.visible.get(i as usize) else { return };
            let node = &t.nodes[n];
            let idx = node.entry.or_else(|| t.order.get(node.start).copied());
            if let Some(e) = idx.and_then(|k| arc.manifest.entries.get(k)) {
                let text = if e.virtual_entry { e.path.clone() } else { vp::target_path(e, &s.env, &remap).display().to_string() };
                a.global::<App>().set_resolved_path(text.into());
            }
        }
    });
    app.global::<App>().on_tree_bulk({
        let (w, st) = (w.clone(), st.clone());
        move |k| {
            if let Some(a) = w.upgrade() {
                if let Some(t) = st.borrow_mut().tree.as_mut() {
                    t.bulk(k);
                }
                refresh_tree(&a, &st);
            }
        }
    });
    app.global::<App>().on_start_restore({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                start_restore(&a, &st);
            }
        }
    });
    app.global::<App>().on_browse_root({
        let w = w.clone();
        move || {
            if let (Some(a), Some(p)) = (w.upgrade(), rfd::FileDialog::new().pick_folder()) {
                a.global::<App>().set_restore_root(p.to_string_lossy().to_string().into());
            }
        }
    });
    app.global::<App>().on_browse_output({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let (Some(a), Some(p)) = (w.upgrade(), rfd::FileDialog::new().pick_folder()) {
                a.global::<App>().set_out_dir(p.to_string_lossy().to_string().into());
                let mut s = st.borrow_mut();
                s.settings.out_dir = p.to_string_lossy().to_string();
                settings::save(&s.settings);
            }
        }
    });
    app.global::<App>().on_settings_changed({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                let g = a.global::<App>();
                let new = Settings { out_dir: g.get_out_dir().to_string(), level: g.get_default_level(), excludes: g.get_default_excludes().to_string(), theme: g.get_theme_index(), language: g.get_language_index(), policy: g.get_default_policy() };
                tr_table::apply_tr(&a.global::<Tr>(), new.language == 1);
                a.global::<Theme>().set_rtl(new.language == 1);
                settings::save(&new);
                st.borrow_mut().settings = new;
                #[cfg(windows)]
                apply_backdrop(&a);
            }
        }
    });
    app.global::<App>().on_pause_toggle({
        let (w, st) = (w.clone(), st.clone());
        move || {
            if let Some(a) = w.upgrade() {
                if let Some(c) = &st.borrow().ctl {
                    let p = !c.is_paused();
                    c.set_paused(p);
                    a.global::<App>().set_paused(p);
                }
            }
        }
    });
    app.global::<App>().on_cancel_job({
        let st = st.clone();
        move || {
            if let Some(c) = &st.borrow().ctl {
                c.set_paused(false);
                c.cancel();
            }
        }
    });
    app.global::<App>().on_close_summary({
        let w = w.clone();
        move || {
            if let Some(a) = w.upgrade() {
                a.global::<App>().set_summary_open(false);
            }
        }
    });
    app.global::<App>().on_export_log({
        let st = st.clone();
        move || {
            if let Some(p) = rfd::FileDialog::new().set_file_name("vaultpack-log.txt").save_file() {
                let s = st.borrow();
                let text = if s.last_log.is_empty() { "No operations have been run yet.\n".to_string() } else { s.last_log.clone() };
                let _ = std::fs::write(p, text);
            }
        }
    });
    app.global::<App>().on_export_programs({
        let st = st.clone();
        move || {
            if let Some(p) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).add_filter("Text", &["txt"]).set_file_name("programs-to-reinstall.csv").save_file() {
                let s = st.borrow();
                let csv = p.extension().map_or(false, |e| e.eq_ignore_ascii_case("csv"));
                let text = if csv { presets::programs_csv(&s.manual) } else { presets::programs_text(&s.manual) };
                let _ = std::fs::write(p, text);
            }
        }
    });

    let timer = slint::Timer::default();
    {
        let (w, st) = (w.clone(), st.clone());
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(60), move || {
            if let Some(a) = w.upgrade() {
                pump(&a, &st);
            }
        });
    }

    app.window().on_winit_window_event({
        let (w, st) = (w.clone(), st.clone());
        move |_, ev| {
            if let Some(a) = w.upgrade() {
                match ev {
                    winit::event::WindowEvent::HoveredFile(_) => a.global::<App>().set_dragging(true),
                    winit::event::WindowEvent::HoveredFileCancelled => a.global::<App>().set_dragging(false),
                    winit::event::WindowEvent::ThemeChanged(t) => {
                        a.global::<Theme>().set_system_dark(*t == winit::window::Theme::Dark);
                        #[cfg(windows)]
                        apply_backdrop(&a);
                    }
                    winit::event::WindowEvent::DroppedFile(p) => {
                        a.global::<App>().set_dragging(false);
                        let is_vpk = p.extension().map_or(false, |e| e.eq_ignore_ascii_case("vpk"));
                        if is_vpk || (a.global::<App>().get_page() == 1 && !p.is_dir()) {
                            load_archive(&a, &st, p, None);
                        } else {
                            a.global::<App>().set_page(0);
                            add_paths(&a, &st, vec![p.clone()]);
                        }
                    }
                    _ => {}
                }
            }
            EventResult::Propagate
        }
    });

    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(first) = args.first() {
        if first == "--resume" {
            if let Some(p) = std::fs::read(settings::data_dir().join("session.json")).ok().and_then(|b| serde_json::from_slice::<Profile>(&b).ok()) {
                apply_profile(&app, &st, p);
                let wifi = app.global::<App>().get_preset_wifi();
                if wifi {
                    app.global::<App>().set_preset_wifi(true);
                }
            }
        } else if Path::new(first).exists() {
            load_archive(&app, &st, Path::new(first), None);
        }
    }

    app.show()?;
    // Follow Windows' own light/dark setting when Theme = System.
    let sys_dark = app.window().with_winit_window(|win| win.theme().map_or(true, |t| t == winit::window::Theme::Dark)).unwrap_or(true);
    app.global::<Theme>().set_system_dark(sys_dark);
    #[cfg(windows)]
    {
        apply_backdrop(&app);
        if elevated {
            allow_drops_when_elevated(&app);
        }
    }
    app.run()
}
