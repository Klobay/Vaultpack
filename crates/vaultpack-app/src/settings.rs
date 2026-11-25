use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub out_dir: String,
    pub level: i32,
    pub excludes: String,
    pub theme: i32,
    pub language: i32,
    pub policy: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { out_dir: String::new(), level: 1, excludes: vaultpack_core::default_excludes().join(", "), theme: 0, language: 0, policy: 0 }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    pub items: Vec<String>,
    pub excludes: String,
    pub browsers: bool,
    pub wifi: bool,
    pub programs: bool,
    pub level: i32,
}

pub fn data_dir() -> PathBuf {
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| std::env::temp_dir());
    let dir = base.join("Vaultpack");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn load() -> Settings {
    std::fs::read(data_dir().join("settings.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(s: &Settings) {
    if let Ok(b) = serde_json::to_vec_pretty(s) {
        let _ = std::fs::write(data_dir().join("settings.json"), b);
    }
}
