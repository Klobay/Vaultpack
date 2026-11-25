use crate::error::{Result, VaultError};
use crate::paths::Env;
use crate::sys::System;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrowserKind {
    Chromium,
    Firefox,
}

#[derive(Debug, Clone)]
pub struct BrowserProfile {
    pub browser: String,
    pub name: String,
    pub dir: PathBuf,
    pub size: u64,
    pub kind: BrowserKind,
    pub image: String,
    pub running: bool,
    pub shared: Vec<PathBuf>,
}

impl BrowserProfile {
    pub fn group(&self) -> String {
        format!("browsers/{}/{}", self.browser, self.name)
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        match self.kind {
            BrowserKind::Firefox => vec![self.dir.clone()],
            BrowserKind::Chromium => {
                let mut v = Vec::new();
                for f in ["Bookmarks", "Preferences", "Secure Preferences", "History", "Login Data", "Login Data For Account", "Web Data", "Favicons", "Extension State", "Network\\Cookies", "Cookies", "Extensions", "Local Extension Settings", "Sync Extension Settings", "Managed Extension Settings", "Extension Rules", "Extension Scripts"] {
                    let p = self.dir.join(f.replace('\\', std::path::MAIN_SEPARATOR_STR));
                    if p.exists() {
                        v.push(p);
                    }
                }
                v.extend(self.shared.iter().filter(|p| p.exists()).cloned());
                v
            }
        }
    }
}

pub fn dir_size(p: &Path) -> u64 {
    WalkDir::new(p).into_iter().filter_map(|e| e.ok()).filter(|e| e.file_type().is_file()).filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

fn chromium_profiles(browser: &str, image: &str, root: PathBuf, opera_style: bool, sys: &dyn System) -> Vec<BrowserProfile> {
    if !root.is_dir() {
        return Vec::new();
    }
    let running = sys.process_running(image);
    let shared = vec![root.join("Local State")];
    let mut out = Vec::new();
    if opera_style {
        out.push(BrowserProfile { browser: browser.into(), name: "Default".into(), dir: root.clone(), size: dir_size(&root), kind: BrowserKind::Chromium, image: image.into(), running, shared: Vec::new() });
        return out;
    }
    if let Ok(rd) = fs::read_dir(&root) {
        for e in rd.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().to_string();
            if e.path().is_dir() && (name == "Default" || name.starts_with("Profile ") || name == "Guest Profile") && e.path().join("Preferences").exists() {
                let paths_size = dir_size(&e.path());
                out.push(BrowserProfile { browser: browser.into(), name, dir: e.path(), size: paths_size, kind: BrowserKind::Chromium, image: image.into(), running, shared: shared.clone() });
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

pub fn detect_browsers(env: &Env, sys: &dyn System) -> Vec<BrowserProfile> {
    let local = env.get("LOCALAPPDATA").map(PathBuf::from);
    let roaming = env.get("APPDATA").map(PathBuf::from);
    let mut out = Vec::new();
    if let Some(l) = &local {
        out.extend(chromium_profiles("Chrome", "chrome.exe", l.join("Google").join("Chrome").join("User Data"), false, sys));
        out.extend(chromium_profiles("Edge", "msedge.exe", l.join("Microsoft").join("Edge").join("User Data"), false, sys));
        out.extend(chromium_profiles("Brave", "brave.exe", l.join("BraveSoftware").join("Brave-Browser").join("User Data"), false, sys));
        out.extend(chromium_profiles("Vivaldi", "vivaldi.exe", l.join("Vivaldi").join("User Data"), false, sys));
    }
    if let Some(r) = &roaming {
        out.extend(chromium_profiles("Opera", "opera.exe", r.join("Opera Software").join("Opera Stable"), true, sys));
        out.extend(chromium_profiles("Opera GX", "opera.exe", r.join("Opera Software").join("Opera GX Stable"), true, sys));
        let ff = r.join("Mozilla").join("Firefox").join("Profiles");
        if let Ok(rd) = fs::read_dir(&ff) {
            let running = sys.process_running("firefox.exe");
            for e in rd.filter_map(|e| e.ok()).filter(|e| e.path().is_dir()) {
                let name = e.file_name().to_string_lossy().to_string();
                out.push(BrowserProfile { browser: "Firefox".into(), name, dir: e.path(), size: dir_size(&e.path()), kind: BrowserKind::Firefox, image: "firefox.exe".into(), running, shared: Vec::new() });
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct WifiProfile {
    pub name: String,
    pub xml: Vec<u8>,
}

fn xml_tag(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{}>", tag);
    let close = format!("</{}>", tag);
    let s = xml.find(&open)? + open.len();
    let e = xml[s..].find(&close)? + s;
    Some(xml[s..e].to_string())
}

pub fn export_wifi(sys: &dyn System, scratch: &Path) -> Result<Vec<WifiProfile>> {
    if !sys.is_elevated() {
        return Err(VaultError::System("Exporting Wi-Fi keys requires administrator rights".into()));
    }
    fs::create_dir_all(scratch)?;
    let folder = format!("folder={}", scratch.display());
    sys.run("netsh", &["wlan", "export", "profile", "key=clear", &folder])?;
    let mut out = Vec::new();
    for e in fs::read_dir(scratch)?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.extension().map_or(false, |x| x.eq_ignore_ascii_case("xml")) {
            let data = fs::read(&p)?;
            let text = crate::sys::decode_output(&data);
            let name = xml_tag(&text, "name").unwrap_or_else(|| p.file_stem().unwrap_or_default().to_string_lossy().to_string());
            out.push(WifiProfile { name, xml: data });
        }
    }
    let _ = fs::remove_dir_all(scratch);
    Ok(out)
}

pub fn import_wifi(sys: &dyn System, name: &str, xml: &[u8], scratch: &Path) -> Result<()> {
    fs::create_dir_all(scratch)?;
    let safe: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
    let file = scratch.join(format!("{}.xml", safe));
    fs::write(&file, xml)?;
    let arg = format!("filename={}", file.display());
    let r = sys.run("netsh", &["wlan", "add", "profile", &arg]);
    let _ = fs::remove_file(&file);
    r.map(|_| ())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Program {
    pub name: String,
    pub version: String,
    pub publisher: String,
    pub winget_id: Option<String>,
}

const UNINSTALL_KEYS: [&str; 3] = [
    "HKLM\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKLM\\SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
    "HKCU\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall",
];

pub fn parse_reg(text: &str) -> Vec<Program> {
    let mut out = Vec::new();
    let mut cur: Option<(String, String, String, bool)> = None;
    let flush = |cur: &mut Option<(String, String, String, bool)>, out: &mut Vec<Program>| {
        if let Some((n, v, p, sys)) = cur.take() {
            if !n.is_empty() && !sys {
                out.push(Program { name: n, version: v, publisher: p, winget_id: None });
            }
        }
    };
    for line in text.lines() {
        let t = line.trim_end();
        if t.starts_with("HKEY") {
            flush(&mut cur, &mut out);
            cur = Some((String::new(), String::new(), String::new(), false));
        } else if let Some((n, v, p, sc)) = cur.as_mut() {
            let tr = t.trim_start();
            let parts: Vec<&str> = tr.splitn(3, "    ").collect();
            if parts.len() == 3 {
                let val = parts[2].trim().to_string();
                match parts[0] {
                    "DisplayName" => *n = val,
                    "DisplayVersion" => *v = val,
                    "Publisher" => *p = val,
                    "SystemComponent" => *sc = val.ends_with('1'),
                    _ => {}
                }
            }
        }
    }
    flush(&mut cur, &mut out);
    out
}

pub fn parse_winget_list(text: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = text.lines().collect();
    let header = lines.iter().position(|l| l.trim_start().starts_with("Name") && l.contains("Id"));
    let Some(h) = header else { return Vec::new() };
    let head = lines[h];
    let id_col = head.find("Id").unwrap_or(0);
    let ver_col = head.find("Version").unwrap_or(head.len());
    let mut out = Vec::new();
    for l in lines.iter().skip(h + 1) {
        if l.starts_with('-') || l.trim().is_empty() {
            continue;
        }
        let chars: Vec<char> = l.chars().collect();
        if chars.len() <= id_col {
            continue;
        }
        let name: String = chars[..id_col.min(chars.len())].iter().collect();
        let end = ver_col.min(chars.len());
        let id: String = if end > id_col { chars[id_col..end].iter().collect() } else { String::new() };
        let (name, id) = (name.trim().to_string(), id.trim().to_string());
        if !name.is_empty() && !id.is_empty() && !id.contains(' ') {
            out.push((name, id));
        }
    }
    out
}

fn norm_name(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

pub fn list_programs(sys: &dyn System) -> Vec<Program> {
    let mut all: Vec<Program> = Vec::new();
    for key in UNINSTALL_KEYS {
        if let Ok(text) = sys.run("reg", &["query", key, "/s"]) {
            all.extend(parse_reg(&text));
        }
    }
    all.sort_by(|a, b| norm_name(&a.name).cmp(&norm_name(&b.name)).then(a.version.cmp(&b.version)));
    all.dedup_by(|a, b| norm_name(&a.name) == norm_name(&b.name) && a.version == b.version);
    let winget = sys.run("winget", &["list", "--accept-source-agreements", "--disable-interactivity"]).map(|t| parse_winget_list(&t)).unwrap_or_default();
    let tmp = std::env::temp_dir().join(format!("vaultpack-winget-{}.json", std::process::id()));
    let mut exported_ids: Vec<String> = Vec::new();
    if sys.run("winget", &["export", "-o", &tmp.to_string_lossy(), "--accept-source-agreements", "--disable-interactivity"]).is_ok() {
        if let Ok(data) = fs::read(&tmp) {
            if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&data) {
                if let Some(srcs) = v.get("Sources").and_then(|s| s.as_array()) {
                    for s in srcs {
                        if let Some(pk) = s.get("Packages").and_then(|p| p.as_array()) {
                            for p in pk {
                                if let Some(id) = p.get("PackageIdentifier").and_then(|i| i.as_str()) {
                                    exported_ids.push(id.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }
        let _ = fs::remove_file(&tmp);
    }
    for p in all.iter_mut() {
        let n = norm_name(&p.name);
        let hit = winget.iter().find(|(wn, _)| norm_name(wn) == n).or_else(|| winget.iter().find(|(wn, _)| !n.is_empty() && (norm_name(wn).starts_with(&n) || n.starts_with(&norm_name(wn))) && norm_name(wn).len() > 3));
        if let Some((_, id)) = hit {
            if exported_ids.is_empty() || exported_ids.contains(id) {
                p.winget_id = Some(id.clone());
            }
        }
    }
    all
}

pub fn install_program(sys: &dyn System, p: &Program) -> Result<()> {
    match &p.winget_id {
        Some(id) => sys.run("winget", &["install", "--id", id, "-e", "--accept-source-agreements", "--accept-package-agreements", "--disable-interactivity"]).map(|_| ()),
        None => Err(VaultError::System("no winget id".into())),
    }
}

pub fn programs_csv(list: &[Program]) -> String {
    let mut s = String::from("Name,Version,Publisher\n");
    for p in list {
        s.push_str(&format!("\"{}\",\"{}\",\"{}\"\n", p.name.replace('"', "\"\""), p.version.replace('"', "\"\""), p.publisher.replace('"', "\"\"")));
    }
    s
}

pub fn programs_text(list: &[Program]) -> String {
    list.iter().map(|p| format!("{} {} - {}", p.name, p.version, p.publisher)).collect::<Vec<_>>().join("\n")
}
