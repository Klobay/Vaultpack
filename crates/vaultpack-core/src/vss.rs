use crate::error::{Result, VaultError};
use crate::sys::System;
use std::path::PathBuf;

pub struct Snapshot {
    pub id: String,
    pub mount: PathBuf,
    pub volume: String,
}

fn between<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let i = text.find(key)? + key.len();
    let rest = &text[i..];
    Some(rest.lines().next().unwrap_or("").trim())
}

pub fn create(sys: &dyn System, drive: char) -> Result<Snapshot> {
    if !sys.is_elevated() {
        return Err(VaultError::System("Volume Shadow Copy requires elevation".into()));
    }
    let volume = format!("{}:\\", drive.to_ascii_uppercase());
    let script = format!(
        "$r=(Get-WmiObject -List Win32_ShadowCopy).Create('{v}','ClientAccessible'); $s=Get-WmiObject Win32_ShadowCopy | Where-Object {{ $_.ID -eq $r.ShadowID }}; Write-Output ('ID=' + $s.ID); Write-Output ('DEV=' + $s.DeviceObject)",
        v = volume
    );
    let out = sys.run("powershell", &["-NoProfile", "-Command", &script])?;
    let id = between(&out, "ID=").ok_or_else(|| VaultError::System("shadow copy id missing".into()))?.to_string();
    let dev = between(&out, "DEV=").ok_or_else(|| VaultError::System("shadow copy device missing".into()))?.to_string();
    let mount = std::env::temp_dir().join(format!("vaultpack-vss-{}", id.trim_matches(|c| c == '{' || c == '}')));
    let link_target = format!("{}\\", dev);
    sys.run("cmd", &["/C", "mklink", "/D", &mount.to_string_lossy(), &link_target])?;
    Ok(Snapshot { id, mount, volume })
}

impl Snapshot {
    pub fn map(&self, original: &str) -> Option<PathBuf> {
        let rest = original.get(3..)?;
        if original.len() >= 3 && original[..3].eq_ignore_ascii_case(&self.volume) {
            Some(self.mount.join(rest))
        } else {
            None
        }
    }

    pub fn release(&self, sys: &dyn System) {
        let _ = std::fs::remove_dir(&self.mount);
        let script = format!("(Get-WmiObject Win32_ShadowCopy | Where-Object {{ $_.ID -eq '{}' }}).Delete()", self.id);
        let _ = sys.run("powershell", &["-NoProfile", "-Command", &script]);
    }
}
