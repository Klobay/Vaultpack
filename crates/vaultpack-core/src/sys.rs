use crate::error::{Result, VaultError};
use std::process::Command;

pub trait System: Send + Sync {
    fn run(&self, program: &str, args: &[&str]) -> Result<String>;
    fn is_elevated(&self) -> bool;
    fn process_running(&self, image: &str) -> bool;
    fn kill_process(&self, image: &str) -> bool;
}

pub struct RealSystem;

fn command(program: &str) -> Command {
    let c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = c;
        c.creation_flags(0x0800_0000);
        return c;
    }
    #[cfg(not(windows))]
    c
}

pub fn decode_output(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).trim_start_matches('\u{feff}').to_string()
}

impl System for RealSystem {
    fn run(&self, program: &str, args: &[&str]) -> Result<String> {
        let out = command(program).args(args).output().map_err(|e| VaultError::System(format!("{}: {}", program, e)))?;
        let text = decode_output(&out.stdout);
        if out.status.success() {
            Ok(text)
        } else {
            let err = decode_output(&out.stderr);
            Err(VaultError::System(format!("{} exited with {}: {}{}", program, out.status, text.trim(), err.trim())))
        }
    }

    fn is_elevated(&self) -> bool {
        cfg!(windows) && command("net").arg("session").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn process_running(&self, image: &str) -> bool {
        let filter = format!("IMAGENAME eq {}", image);
        command("tasklist")
            .args(["/FI", &filter, "/NH"])
            .output()
            .map(|o| decode_output(&o.stdout).to_lowercase().contains(&image.to_lowercase()))
            .unwrap_or(false)
    }

    fn kill_process(&self, image: &str) -> bool {
        command("taskkill").args(["/IM", image, "/F"]).output().map(|o| o.status.success()).unwrap_or(false)
    }
}

pub fn relaunch_elevated(args: &[String]) -> Result<()> {
    let exe = std::env::current_exe()?;
    let quoted: Vec<String> = args.iter().map(|a| format!("'\"{}\"'", a.replace('\'', "''"))).collect();
    let arglist = if quoted.is_empty() { String::new() } else { format!(" -ArgumentList {}", quoted.join(",")) };
    let script = format!("Start-Process -FilePath '{}' -Verb RunAs{}", exe.display().to_string().replace('\'', "''"), arglist);
    command("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]).spawn().map_err(|e| VaultError::System(e.to_string()))?;
    Ok(())
}
