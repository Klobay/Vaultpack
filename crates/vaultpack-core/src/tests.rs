use super::*;
use crate::presets::{self, Program};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::{mpsc, Arc, Mutex};

struct Mock {
    elevated: bool,
    calls: Mutex<Vec<String>>,
}

impl System for Mock {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        self.calls.lock().unwrap().push(format!("{} {}", program, args.join(" ")));
        if program == "netsh" && args.contains(&"export") {
            let folder = args.iter().find_map(|a| a.strip_prefix("folder=")).unwrap();
            fs::write(Path::new(folder).join("Wi-Fi-Home.xml"), "<WLANProfile><name>HomeNet</name><key>secret</key></WLANProfile>").unwrap();
        }
        if program == "reg" {
            return Ok("HKEY_LOCAL_MACHINE\\X\\A\r\n    DisplayName    REG_SZ    Git\r\n    DisplayVersion    REG_SZ    2.0\r\n    Publisher    REG_SZ    Git Dev\r\n\r\nHKEY_LOCAL_MACHINE\\X\\B\r\n    DisplayName    REG_SZ    Obscure Tool\r\n".into());
        }
        if program == "winget" && args.first() == Some(&"list") {
            return Ok("Name  Id       Version\n--------------------\nGit   Git.Git  2.0\n".into());
        }
        Ok(String::new())
    }
    fn is_elevated(&self) -> bool {
        self.elevated
    }
    fn process_running(&self, _: &str) -> bool {
        false
    }
    fn kill_process(&self, _: &str) -> bool {
        true
    }
}

fn mock(elevated: bool) -> Arc<Mock> {
    Arc::new(Mock { elevated, calls: Mutex::new(Vec::new()) })
}

fn opts(sources: Vec<Source>, dest: &Path, pw: Option<&str>, sys: Arc<dyn System>) -> BackupOptions {
    BackupOptions { sources, destination: dest.to_path_buf(), level: CompressionLevel::Fast, password: pw.map(|s| s.to_string()), excludes: vec!["*.tmp".into(), "node_modules".into()], use_vss: false, verify: true, env: Env::default(), system: sys }
}

fn ropts(root: &Path, sys: Arc<dyn System>) -> RestoreOptions {
    RestoreOptions { selected: None, policy: ConflictPolicy::Overwrite, remap: Remap { drives: Default::default(), root: Some(root.to_path_buf()) }, env: Env::default(), system: sys, scratch: root.join("scratch") }
}

fn tree(root: &Path) {
    fs::create_dir_all(root.join("sub/empty")).unwrap();
    fs::create_dir_all(root.join("node_modules")).unwrap();
    fs::write(root.join("a.txt"), "hello world".repeat(1000)).unwrap();
    fs::write(root.join("sub/b.bin"), vec![7u8; 9 * 1024 * 1024 + 13]).unwrap();
    fs::write(root.join("sub/ü ñ 日本.txt"), "unicode").unwrap();
    fs::write(root.join("skip.tmp"), "x").unwrap();
    fs::write(root.join("node_modules/x.js"), "x").unwrap();
}

fn roundtrip(pw: Option<&str>) {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("src");
    tree(&src);
    let dest = d.path().join("out.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    let sum = run_backup(&opts(vec![Source::Path { path: src.clone(), group: None }], &dest, pw, mock(false)), &ctl, &tx).unwrap();
    assert!(sum.failed.is_empty());
    assert_eq!(is_encrypted(&dest).unwrap(), pw.is_some());
    let arc = open(&dest, pw).unwrap();
    assert!(arc.manifest.entries.iter().all(|e| !e.path.ends_with(".tmp") && !e.path.contains("node_modules")));
    let restore_root = d.path().join("restored");
    let s = run_restore(&arc, &ropts(&restore_root, mock(false)), &ctl, &tx).unwrap();
    assert!(s.failed.is_empty(), "{:?}", s.failed);
    let (e, _) = arc.manifest.entries.iter().find(|e| e.path.ends_with("b.bin")).map(|e| (e.clone(), 0)).unwrap();
    let t = target_path(&e, &Env::default(), &Remap { drives: Default::default(), root: Some(restore_root.clone()) });
    assert_eq!(fs::read(&t).unwrap().len(), 9 * 1024 * 1024 + 13);
    assert_eq!(fs::read(&t).unwrap(), fs::read(src.join("sub/b.bin")).unwrap());
    let empty = arc.manifest.entries.iter().find(|e| e.path.ends_with("empty")).unwrap();
    assert!(target_path(empty, &Env::default(), &Remap { drives: Default::default(), root: Some(restore_root.clone()) }).is_dir());
    let uni = arc.manifest.entries.iter().find(|e| e.path.ends_with("日本.txt")).unwrap();
    assert_eq!(fs::read_to_string(target_path(uni, &Env::default(), &Remap { drives: Default::default(), root: Some(restore_root) })).unwrap(), "unicode");
}

#[test]
fn roundtrip_unencrypted() {
    roundtrip(None);
}

#[test]
fn roundtrip_encrypted() {
    roundtrip(Some("correct horse"));
}

#[test]
fn wrong_password_fails_cleanly() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("s");
    tree(&src);
    let dest = d.path().join("o.vpk");
    let (tx, _rx) = mpsc::channel();
    run_backup(&opts(vec![Source::Path { path: src, group: None }], &dest, Some("pw1"), mock(false)), &Control::new(), &tx).unwrap();
    assert!(matches!(open(&dest, Some("nope")), Err(VaultError::WrongPassword)));
    assert!(matches!(open(&dest, None), Err(VaultError::PasswordRequired)));
    let raw = fs::read(&dest).unwrap();
    assert!(!raw.windows(5).any(|w| w == b"b.bin"));
}

#[test]
fn corrupt_archive_rejected() {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join("bad.vpk");
    fs::write(&p, b"not an archive at all, just some bytes that are long enough to pass the length check").unwrap();
    assert!(matches!(open(&p, None), Err(VaultError::InvalidArchive)));
    fs::write(&p, b"x").unwrap();
    assert!(open(&p, None).is_err());
    let src = d.path().join("s");
    tree(&src);
    let dest = d.path().join("o.vpk");
    let (tx, _rx) = mpsc::channel();
    run_backup(&opts(vec![Source::Path { path: src, group: None }], &dest, None, mock(false)), &Control::new(), &tx).unwrap();
    let mut raw = fs::read(&dest).unwrap();
    raw[60] ^= 0xFF;
    raw[200] ^= 0xFF;
    fs::write(&dest, &raw).unwrap();
    let bad = verify_archive(&dest, None, &Control::new(), &tx).unwrap();
    assert!(!bad.is_empty());
    let trunc = &raw[..raw.len() - 5];
    fs::write(&dest, trunc).unwrap();
    assert!(open(&dest, None).is_err());
}

#[test]
fn conflict_policies() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("f.txt"), "new").unwrap();
    let dest = d.path().join("o.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    run_backup(&opts(vec![Source::Path { path: src.clone(), group: None }], &dest, None, mock(false)), &ctl, &tx).unwrap();
    let arc = open(&dest, None).unwrap();
    let root = d.path().join("r");
    run_restore(&arc, &ropts(&root, mock(false)), &ctl, &tx).unwrap();
    let e = arc.manifest.entries.iter().find(|e| e.path.ends_with("f.txt")).unwrap();
    let t = target_path(e, &Env::default(), &Remap { drives: Default::default(), root: Some(root.clone()) });
    fs::write(&t, "local").unwrap();
    let mut o = ropts(&root, mock(false));
    o.policy = ConflictPolicy::Skip;
    let s = run_restore(&arc, &o, &ctl, &tx).unwrap();
    assert_eq!(fs::read_to_string(&t).unwrap(), "local");
    assert_eq!(s.skipped.len(), 1);
    o.policy = ConflictPolicy::KeepBoth;
    run_restore(&arc, &o, &ctl, &tx).unwrap();
    assert_eq!(fs::read_to_string(&t).unwrap(), "local");
    assert!(fs::read_dir(t.parent().unwrap()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains("(restored)")));
    o.policy = ConflictPolicy::Overwrite;
    run_restore(&arc, &o, &ctl, &tx).unwrap();
    assert_eq!(fs::read_to_string(&t).unwrap(), "new");
    o.policy = ConflictPolicy::OverwriteIfOlder;
    fs::write(&t, "newer-local").unwrap();
    run_restore(&arc, &o, &ctl, &tx).unwrap();
    assert_eq!(fs::read_to_string(&t).unwrap(), "newer-local");
}

#[cfg(unix)]
#[test]
fn symlinks_roundtrip() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("s");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.txt"), "t").unwrap();
    std::os::unix::fs::symlink("t.txt", src.join("l")).unwrap();
    let dest = d.path().join("o.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    run_backup(&opts(vec![Source::Path { path: src, group: None }], &dest, None, mock(false)), &ctl, &tx).unwrap();
    let arc = open(&dest, None).unwrap();
    assert!(arc.manifest.entries.iter().any(|e| e.kind == EntryKind::Symlink));
    let root = d.path().join("r");
    let s = run_restore(&arc, &ropts(&root, mock(false)), &ctl, &tx).unwrap();
    assert!(s.failed.is_empty(), "{:?}", s.failed);
}

#[test]
fn tokenising_and_remap() {
    let env = Env::default().with("USERPROFILE", "C:\\Users\\Old").with("APPDATA", "C:\\Users\\Old\\AppData\\Roaming").with("LOCALAPPDATA", "C:\\Users\\Old\\AppData\\Local");
    let (t, d) = paths::tokenise(&env, "C:\\Users\\Old\\AppData\\Roaming\\x\\y.txt");
    assert_eq!(t, "%APPDATA%\\x\\y.txt");
    assert_eq!(d, Some('C'));
    let (t2, _) = paths::tokenise(&env, "D:\\Games\\g.exe");
    assert_eq!(t2, "D:\\Games\\g.exe");
    let new_env = Env::default().with("USERPROFILE", "C:\\Users\\New").with("APPDATA", "C:\\Users\\New\\AppData\\Roaming");
    assert_eq!(paths::resolve(&new_env, &t, d, &Remap::default()), std::path::PathBuf::from("C:\\Users\\New\\AppData\\Roaming\\x\\y.txt"));
    let mut remap = Remap::default();
    remap.drives.insert('D', 'E');
    assert_eq!(paths::resolve(&new_env, &t2, Some('D'), &remap), std::path::PathBuf::from("E:\\Games\\g.exe"));
    let remap_root = Remap { drives: Default::default(), root: Some("/r".into()) };
    assert!(paths::resolve(&new_env, &t2, Some('D'), &remap_root).ends_with("Games/g.exe") || paths::resolve(&new_env, &t2, Some('D'), &remap_root).to_string_lossy().contains("Games"));
}

#[test]
fn protected_locations() {
    let env = Env::default().with("ProgramFiles", "C:\\Program Files").with("SystemRoot", "C:\\Windows").with("USERPROFILE", "C:\\Users\\Me");
    assert!(paths::is_protected(&env, "C:\\Program Files\\App\\a.dll"));
    assert!(paths::is_protected(&env, "C:\\Windows\\System32\\x"));
    assert!(paths::is_protected(&env, "C:\\Users\\Other\\Documents\\x"));
    assert!(!paths::is_protected(&env, "C:\\Users\\Me\\Documents\\x"));
}

#[test]
fn long_path_prefix() {
    let long = format!("C:\\{}", "a\\".repeat(150));
    let p = paths::to_long(std::path::PathBuf::from(&long));
    if cfg!(windows) {
        assert!(p.to_string_lossy().starts_with("\\\\?\\"));
    }
}

#[test]
fn wifi_preset_roundtrip_mocked() {
    let d = tempfile::tempdir().unwrap();
    let sys = mock(true);
    let list = presets::export_wifi(sys.as_ref(), &d.path().join("w")).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "HomeNet");
    let src: Vec<Source> = list.iter().map(|w| Source::Virtual { name: format!("{}.xml", w.name), data: w.xml.clone(), group: format!("wifi/{}", w.name) }).collect();
    let dest = d.path().join("w.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    run_backup(&opts(src, &dest, Some("pw"), sys.clone()), &ctl, &tx).unwrap();
    let arc = open(&dest, Some("pw")).unwrap();
    let s = run_restore(&arc, &ropts(d.path(), sys.clone()), &ctl, &tx).unwrap();
    assert_eq!(s.done, 1);
    assert!(sys.calls.lock().unwrap().iter().any(|c| c.starts_with("netsh wlan add profile")));
    let denied = presets::export_wifi(mock(false).as_ref(), &d.path().join("w2"));
    assert!(denied.is_err());
}

#[test]
fn programs_preset_mocked() {
    let sys = mock(true);
    let list = presets::list_programs(sys.as_ref());
    assert_eq!(list.len(), 2);
    let git = list.iter().find(|p| p.name == "Git").unwrap();
    assert_eq!(git.winget_id.as_deref(), Some("Git.Git"));
    assert!(list.iter().find(|p| p.name == "Obscure Tool").unwrap().winget_id.is_none());
    let d = tempfile::tempdir().unwrap();
    let src: Vec<Source> = list.iter().map(|p| Source::Virtual { name: format!("{}.json", p.name), data: serde_json::to_vec(p).unwrap(), group: format!("programs/{}", p.name) }).collect();
    let dest = d.path().join("p.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    run_backup(&opts(src, &dest, None, sys.clone()), &ctl, &tx).unwrap();
    let arc = open(&dest, None).unwrap();
    let mut o = ropts(d.path(), sys.clone());
    o.selected = Some((0..arc.manifest.entries.len()).collect::<HashSet<_>>());
    let s = run_restore(&arc, &o, &ctl, &tx).unwrap();
    assert_eq!(s.manual_programs.len(), 1);
    assert!(sys.calls.lock().unwrap().iter().any(|c| c.contains("winget install --id Git.Git")));
    assert!(presets::programs_csv(&s.manual_programs).contains("Obscure Tool"));
    let _: Option<Program> = None;
}

#[test]
fn browser_detection_mocked() {
    let d = tempfile::tempdir().unwrap();
    let ud = d.path().join("Google/Chrome/User Data");
    fs::create_dir_all(ud.join("Default")).unwrap();
    fs::write(ud.join("Default/Preferences"), "{}").unwrap();
    fs::write(ud.join("Default/Bookmarks"), "{}").unwrap();
    fs::write(ud.join("Local State"), "{}").unwrap();
    let env = Env::default().with("LOCALAPPDATA", &d.path().to_string_lossy());
    let found = presets::detect_browsers(&env, mock(false).as_ref());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].browser, "Chrome");
    assert!(found[0].paths().len() >= 3);
}

#[test]
fn cancel_leaves_no_partial() {
    let d = tempfile::tempdir().unwrap();
    let src = d.path().join("s");
    tree(&src);
    let dest = d.path().join("o.vpk");
    let (tx, _rx) = mpsc::channel();
    let ctl = Control::new();
    ctl.cancel();
    let r = run_backup(&opts(vec![Source::Path { path: src, group: None }], &dest, None, mock(false)), &ctl, &tx);
    assert!(matches!(r, Err(VaultError::Cancelled)));
    assert!(!dest.exists());
    assert!(!d.path().join("o.vpk.partial").exists());
}

#[test]
fn zen_and_firefox_detected() {
    let d = tempfile::tempdir().unwrap();
    fs::create_dir_all(d.path().join("zen/Profiles/abc.Default (release)")).unwrap();
    fs::write(d.path().join("zen/Profiles/abc.Default (release)/places.sqlite"), "x").unwrap();
    fs::create_dir_all(d.path().join("Mozilla/Firefox/Profiles/xyz.default")).unwrap();
    let env = Env::default().with("APPDATA", &d.path().to_string_lossy());
    let found = presets::detect_browsers(&env, mock(false).as_ref());
    assert!(found.iter().any(|b| b.browser == "Zen" && b.group().starts_with("browsers/Zen/")));
    assert!(found.iter().any(|b| b.browser == "Firefox"));
    assert!(found.iter().find(|b| b.browser == "Zen").unwrap().paths().len() == 1);
}
