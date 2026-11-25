use std::collections::HashMap;
use std::path::PathBuf;

const TOKENS: [&str; 4] = ["LOCALAPPDATA", "APPDATA", "PROGRAMDATA", "USERPROFILE"];

#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: HashMap<String, String>,
}

impl Env {
    pub fn from_system() -> Env {
        let mut vars = HashMap::new();
        for t in TOKENS.iter().chain(["ProgramFiles", "ProgramFiles(x86)", "SystemRoot", "SystemDrive", "USERNAME"].iter()) {
            if let Ok(v) = std::env::var(t) {
                vars.insert(t.to_uppercase(), v);
            }
        }
        Env { vars }
    }

    pub fn with(mut self, key: &str, value: &str) -> Env {
        self.vars.insert(key.to_uppercase(), value.to_string());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(&key.to_uppercase()).map(|s| s.as_str())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Remap {
    pub drives: HashMap<char, char>,
    pub root: Option<PathBuf>,
}

fn norm(p: &str) -> String {
    p.replace('/', "\\")
}

fn starts_with_ci(path: &str, prefix: &str) -> bool {
    path.len() >= prefix.len()
        && path.is_char_boundary(prefix.len())
        && path[..prefix.len()].eq_ignore_ascii_case(prefix)
        && (path.len() == prefix.len() || path.as_bytes()[prefix.len()] == b'\\')
}

pub fn strip_verbatim(p: &str) -> &str {
    p.strip_prefix("\\\\?\\").unwrap_or(p)
}

pub fn tokenise(env: &Env, abs: &str) -> (String, Option<char>) {
    let path = norm(strip_verbatim(abs));
    let drive = drive_of(&path);
    let mut best: Option<(&str, usize)> = None;
    for t in TOKENS {
        if let Some(base) = env.get(t) {
            let base = norm(strip_verbatim(base));
            let base = base.trim_end_matches('\\').to_string();
            if !base.is_empty() && starts_with_ci(&path, &base) && best.map_or(true, |(_, l)| base.len() > l) {
                best = Some((t, base.len()));
            }
        }
    }
    match best {
        Some((t, len)) => (format!("%{}%{}", t, &path[len..]), drive),
        None => (path, drive),
    }
}

pub fn drive_of(path: &str) -> Option<char> {
    let b = path.as_bytes();
    if b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic() {
        Some((b[0] as char).to_ascii_uppercase())
    } else {
        None
    }
}

pub fn resolve(env: &Env, tokenised: &str, drive: Option<char>, remap: &Remap) -> PathBuf {
    let mut path = tokenised.to_string();
    let mut was_token = None;
    for t in TOKENS {
        let tok = format!("%{}%", t);
        if path.len() >= tok.len() && path[..tok.len()].eq_ignore_ascii_case(&tok) {
            was_token = Some(t);
            if remap.root.is_none() {
                if let Some(base) = env.get(t) {
                    path = format!("{}{}", base.trim_end_matches('\\'), &path[tok.len()..]);
                }
            }
            break;
        }
    }
    if let Some(root) = &remap.root {
        let rest = match was_token {
            Some(t) => format!("{}{}", t, &path[t.len() + 2..]),
            None => {
                let d = drive.map(|c| c.to_string()).unwrap_or_else(|| "root".to_string());
                let tail = if drive.is_some() { path.get(2..).unwrap_or("") } else { path.as_str() };
                format!("{}{}", d, tail)
            }
        };
        let mut out = root.clone();
        for part in rest.split('\\').filter(|s| !s.is_empty() && *s != "." && *s != "..") {
            out.push(part.replace(':', "_"));
        }
        return out;
    }
    if let Some(d) = drive_of(&path) {
        if let Some(n) = remap.drives.get(&d) {
            path = format!("{}{}", n.to_ascii_uppercase(), &path[1..]);
        }
    }
    PathBuf::from(path)
}

pub fn to_long(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy().to_string();
    if cfg!(windows) && s.len() >= 240 && !s.starts_with("\\\\?\\") && drive_of(&s).is_some() {
        PathBuf::from(format!("\\\\?\\{}", s))
    } else {
        p
    }
}

pub fn is_protected(env: &Env, path: &str) -> bool {
    let p = norm(strip_verbatim(path));
    for key in ["ProgramFiles", "ProgramFiles(x86)", "SystemRoot"] {
        if let Some(base) = env.get(key) {
            if starts_with_ci(&p, &norm(base)) {
                return true;
            }
        }
    }
    if let (Some(profile), true) = (env.get("USERPROFILE"), p.to_lowercase().contains("\\users\\")) {
        let own = norm(profile).to_lowercase();
        let low = p.to_lowercase();
        if !low.starts_with(&own) {
            let after = low.split("\\users\\").nth(1).unwrap_or("");
            let name = after.split('\\').next().unwrap_or("");
            if !matches!(name, "" | "public" | "default" | "all users") {
                return true;
            }
        }
    }
    false
}
