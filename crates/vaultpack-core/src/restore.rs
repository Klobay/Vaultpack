use crate::error::{Result, VaultError};
use crate::format::*;
use crate::fsmeta;
use crate::paths::{is_protected, resolve, to_long, Env, Remap};
use crate::presets::{self, Program};
use crate::sys::System;
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Overwrite,
    Skip,
    KeepBoth,
    OverwriteIfOlder,
}

impl ConflictPolicy {
    pub fn from_index(i: i32) -> Self {
        match i {
            1 => Self::Overwrite,
            2 => Self::Skip,
            3 => Self::KeepBoth,
            4 => Self::OverwriteIfOlder,
            _ => Self::KeepBoth,
        }
    }
}

#[derive(Clone)]
pub struct RestoreOptions {
    pub selected: Option<HashSet<usize>>,
    pub policy: ConflictPolicy,
    pub remap: Remap,
    pub env: Env,
    pub system: Arc<dyn System>,
    pub scratch: PathBuf,
}

pub fn target_path(entry: &Entry, env: &Env, remap: &Remap) -> PathBuf {
    resolve(env, &entry.tpath, entry.drive, remap)
}

fn keep_both(target: &Path) -> PathBuf {
    let stem = target.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let ext = target.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{} (restored){}", stem, ext) } else { format!("{} (restored {}){}", stem, n, ext) };
        let candidate = target.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn read_all(f: &mut File, archive: &Archive, e: &Entry) -> Result<Vec<u8>> {
    let mut data = Vec::with_capacity(e.size as usize);
    for c in &e.chunks {
        data.extend_from_slice(&read_chunk(f, &archive.key, c)?);
    }
    if blake3::hash(&data).to_hex().as_str() != e.hash {
        return Err(VaultError::Integrity(e.path.clone()));
    }
    Ok(data)
}

fn is_group(e: &Entry, prefix: &str) -> bool {
    e.group.as_deref().map_or(false, |g| g.starts_with(prefix))
}

pub fn run_restore(archive: &Archive, opts: &RestoreOptions, ctl: &Control, tx: &Sender) -> Result<Summary> {
    let mut summary = Summary::default();
    let mut f = File::open(&archive.path)?;
    let chosen: Vec<usize> = archive.manifest.entries.iter().enumerate().filter(|(i, _)| opts.selected.as_ref().map_or(true, |s| s.contains(i))).map(|(i, _)| i).collect();
    let total: u64 = chosen.iter().map(|&i| archive.manifest.entries[i].size).sum();
    let files = chosen.iter().filter(|&&i| archive.manifest.entries[i].kind == EntryKind::File).count() as u64;
    let _ = tx.send(Event::Scan { files, bytes: total });
    let elevated = opts.system.is_elevated();
    let mut done_bytes = 0u64;
    let mut last = Instant::now();
    let mut index = 0u64;
    let mut dirs: Vec<(PathBuf, usize)> = Vec::new();
    let mut programs: Vec<Program> = Vec::new();

    let mut order = chosen.clone();
    order.sort_by_key(|&i| match archive.manifest.entries[i].kind {
        EntryKind::Dir => 0,
        EntryKind::File => 1,
        _ => 2,
    });

    for i in order {
        ctl.checkpoint()?;
        let e = &archive.manifest.entries[i];
        if e.virtual_entry {
            index += 1;
            let _ = tx.send(Event::File { name: e.path.clone(), index, total: files });
            let data = match read_all(&mut f, archive, e) {
                Ok(d) => d,
                Err(err) => {
                    summary.failed.push((e.path.clone(), err.to_string()));
                    continue;
                }
            };
            if is_group(e, "wifi/") {
                let name = e.path.rsplit('/').next().unwrap_or("wifi").trim_end_matches(".xml").to_string();
                if !elevated {
                    summary.skipped.push((format!("Wi-Fi: {}", name), "requires administrator rights".into()));
                    continue;
                }
                match presets::import_wifi(opts.system.as_ref(), &name, &data, &opts.scratch) {
                    Ok(()) => summary.done += 1,
                    Err(err) => summary.failed.push((format!("Wi-Fi: {}", name), err.to_string())),
                }
            } else if is_group(e, "programs/") {
                if let Ok(p) = serde_json::from_slice::<Program>(&data) {
                    programs.push(p);
                }
            }
            continue;
        }
        let target = target_path(e, &opts.env, &opts.remap);
        let shown = target.display().to_string();
        if opts.remap.root.is_none() && is_protected(&opts.env, &shown) && !elevated {
            summary.skipped.push((shown, "protected location - run as administrator".into()));
            continue;
        }
        let long = to_long(target.clone());
        match e.kind {
            EntryKind::Dir => {
                match fs::create_dir_all(&long) {
                    Ok(()) => {
                        dirs.push((long, i));
                        summary.done += 1;
                    }
                    Err(err) => summary.failed.push((shown, err.to_string())),
                }
            }
            EntryKind::Symlink | EntryKind::Junction => {
                if let Some(parent) = long.parent() {
                    let _ = fs::create_dir_all(parent);
                }
                if long.symlink_metadata().is_ok() {
                    summary.skipped.push((shown, "already exists".into()));
                    continue;
                }
                match make_link(opts.system.as_ref(), e, &long) {
                    Ok(()) => summary.done += 1,
                    Err(err) => summary.failed.push((shown, err.to_string())),
                }
            }
            EntryKind::File => {
                index += 1;
                let _ = tx.send(Event::File { name: target.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), index, total: files });
                let mut dest = long.clone();
                if dest.exists() {
                    match opts.policy {
                        ConflictPolicy::Skip => {
                            summary.skipped.push((shown, "already exists".into()));
                            done_bytes += e.size;
                            continue;
                        }
                        ConflictPolicy::KeepBoth => dest = keep_both(&dest),
                        ConflictPolicy::OverwriteIfOlder => {
                            let existing = fs::metadata(&dest).map(|m| fsmeta::nanos(m.modified())).unwrap_or(0);
                            if existing >= e.modified {
                                summary.skipped.push((shown, "existing file is newer or same age".into()));
                                done_bytes += e.size;
                                continue;
                            }
                        }
                        ConflictPolicy::Overwrite => {}
                    }
                }
                if let Some(parent) = dest.parent() {
                    if let Err(err) = fs::create_dir_all(parent) {
                        summary.failed.push((shown, err.to_string()));
                        continue;
                    }
                }
                let mut tmp_name = dest.file_name().unwrap_or_default().to_os_string();
                tmp_name.push(".vpk-part");
                let tmp = dest.with_file_name(tmp_name);
                let res: Result<()> = (|| {
                    let mut w = BufWriter::new(File::create(&tmp)?);
                    let mut h = blake3::Hasher::new();
                    let mut fd = 0u64;
                    for c in &e.chunks {
                        ctl.checkpoint()?;
                        let raw = read_chunk(&mut f, &archive.key, c)?;
                        h.update(&raw);
                        w.write_all(&raw)?;
                        fd += raw.len() as u64;
                        done_bytes += raw.len() as u64;
                        if last.elapsed() >= Duration::from_millis(80) {
                            last = Instant::now();
                            let _ = tx.send(Event::Bytes { done: done_bytes, total, file_done: fd, file_total: e.size });
                        }
                    }
                    w.flush()?;
                    drop(w);
                    if h.finalize().to_hex().as_str() != e.hash {
                        return Err(VaultError::Integrity("hash mismatch".into()));
                    }
                    if dest.exists() {
                        fsmeta::clear_readonly(&dest);
                        fs::remove_file(&dest)?;
                    }
                    fs::rename(&tmp, &dest)?;
                    fsmeta::set_times(&dest, e.modified, e.accessed);
                    fsmeta::set_attrs(&dest, e.attrs);
                    Ok(())
                })();
                match res {
                    Ok(()) => {
                        summary.done += 1;
                        summary.bytes += e.size;
                    }
                    Err(VaultError::Cancelled) => {
                        let _ = fs::remove_file(&tmp);
                        return Err(VaultError::Cancelled);
                    }
                    Err(err) => {
                        let _ = fs::remove_file(&tmp);
                        let reason = match &err {
                            VaultError::Io(io) if io.kind() == std::io::ErrorKind::PermissionDenied => "access denied - try running as administrator".to_string(),
                            other => other.to_string(),
                        };
                        summary.failed.push((shown, reason));
                    }
                }
            }
        }
    }
    for (p, i) in dirs.into_iter().rev() {
        let e = &archive.manifest.entries[i];
        fsmeta::set_times(&p, e.modified, e.accessed);
        fsmeta::set_attrs(&p, e.attrs);
    }
    if !programs.is_empty() {
        let _ = tx.send(Event::Status("Installing programs".into()));
        for p in programs {
            ctl.checkpoint()?;
            let _ = tx.send(Event::File { name: p.name.clone(), index, total: files });
            if p.winget_id.is_some() {
                match presets::install_program(opts.system.as_ref(), &p) {
                    Ok(()) => summary.done += 1,
                    Err(err) => summary.failed.push((p.name.clone(), err.to_string())),
                }
            } else {
                summary.manual_programs.push(p);
            }
        }
    }
    let _ = tx.send(Event::Bytes { done: total, total, file_done: 0, file_total: 0 });
    Ok(summary)
}

fn make_link(sys: &dyn System, e: &Entry, at: &Path) -> Result<()> {
    let target = e.link_target.clone().ok_or_else(|| VaultError::InvalidArchive)?;
    #[cfg(windows)]
    {
        if e.kind == EntryKind::Junction {
            let t = crate::paths::strip_verbatim(&target).trim_start_matches("??\\").to_string();
            sys.run("cmd", &["/C", "mklink", "/J", &at.to_string_lossy(), &t])?;
            return Ok(());
        }
        let is_dir = Path::new(&target).is_dir();
        if is_dir {
            std::os::windows::fs::symlink_dir(&target, at)?;
        } else {
            std::os::windows::fs::symlink_file(&target, at)?;
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = sys;
        std::os::unix::fs::symlink(&target, at)?;
        Ok(())
    }
}
