use crate::crypto::{random_salt, Key};
use crate::error::{Result, VaultError};
use crate::format::*;
use crate::fsmeta;
use crate::paths::{drive_of, tokenise, Env};
use crate::sys::System;
use crate::vss::{self, Snapshot};
use globset::{Glob, GlobSet, GlobSetBuilder};
use rayon::prelude::*;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
pub enum Source {
    Path { path: PathBuf, group: Option<String> },
    Virtual { name: String, data: Vec<u8>, group: String },
}

#[derive(Clone)]
pub struct BackupOptions {
    pub sources: Vec<Source>,
    pub destination: PathBuf,
    pub level: CompressionLevel,
    pub password: Option<String>,
    pub excludes: Vec<String>,
    pub use_vss: bool,
    pub verify: bool,
    pub env: Env,
    pub system: Arc<dyn System>,
}

#[derive(Debug, Clone, Default)]
pub struct ItemInfo {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    pub files: u64,
    pub unreadable: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ScanResult {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub warnings: Vec<String>,
}

pub fn exclude_set(patterns: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for raw in patterns.iter().map(|p| p.trim().replace('\\', "/")).filter(|p| !p.is_empty()) {
        let base = raw.trim_matches('/').to_string();
        for form in [base.clone(), format!("**/{}", base), format!("**/{}/**", base), format!("{}/**", base)] {
            b.add(Glob::new(&form).map_err(|e| VaultError::Pattern(e.to_string()))?);
        }
    }
    b.build().map_err(|e| VaultError::Pattern(e.to_string()))
}

fn norm_for_match(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

pub fn default_excludes() -> Vec<String> {
    ["*.tmp", "*.temp", "Thumbs.db", "desktop.ini", "node_modules", "$RECYCLE.BIN", "System Volume Information", "__pycache__", ".git/objects"].iter().map(|s| s.to_string()).collect()
}

pub fn exclude_presets() -> Vec<(&'static str, Vec<&'static str>)> {
    vec![
        ("Temporary files", vec!["*.tmp", "*.temp", "*.bak~", "~$*"]),
        ("Developer folders", vec!["node_modules", "target", "__pycache__", ".venv", "bin/Debug", "obj"]),
        ("Cache folders", vec!["Cache", "Code Cache", "GPUCache", "Cache2", "ShaderCache", "INetCache"]),
        ("System junk", vec!["Thumbs.db", "desktop.ini", "$RECYCLE.BIN", "System Volume Information", "pagefile.sys", "hiberfil.sys", "swapfile.sys"]),
    ]
}

pub fn scan_item(path: &Path, excludes: &GlobSet) -> ItemInfo {
    let meta = fs::symlink_metadata(path);
    let mut info = ItemInfo { path: path.to_path_buf(), ..Default::default() };
    match meta {
        Err(e) => info.unreadable.push(format!("{}: {}", path.display(), e)),
        Ok(m) if m.is_dir() => {
            info.is_dir = true;
            let walker = WalkDir::new(path).follow_links(false).into_iter().filter_entry(|e| e.depth() == 0 || !excludes.is_match(norm_for_match(e.path())));
            for e in walker {
                match e {
                    Ok(e) if e.file_type().is_file() => {
                        info.files += 1;
                        info.size += e.metadata().map(|m| m.len()).unwrap_or(0);
                    }
                    Ok(_) => {}
                    Err(err) => info.unreadable.push(err.to_string()),
                }
            }
        }
        Ok(m) => {
            info.files = 1;
            info.size = m.len();
        }
    }
    info
}

pub fn prescan(sources: &[Source], destination: &Path, excludes: &[String]) -> Result<ScanResult> {
    let set = exclude_set(excludes)?;
    let mut res = ScanResult::default();
    let mut src_drives = Vec::new();
    for s in sources {
        match s {
            Source::Path { path, .. } => {
                let i = scan_item(path, &set);
                res.files += i.files;
                res.bytes += i.size;
                if i.is_dir {
                    res.dirs += 1;
                }
                for u in i.unreadable.iter().take(20) {
                    res.warnings.push(format!("Unreadable: {}", u));
                }
                if let Some(d) = drive_of(&path.to_string_lossy()) {
                    if !src_drives.contains(&d) {
                        src_drives.push(d);
                    }
                }
            }
            Source::Virtual { data, .. } => {
                res.files += 1;
                res.bytes += data.len() as u64;
            }
        }
    }
    if let Some(free) = fsmeta::free_space(destination) {
        if free < res.bytes / 2 {
            res.warnings.push(format!("Destination may not have enough free space ({} MiB free, up to {} MiB needed)", free >> 20, res.bytes >> 20));
        }
    }
    if let Some(dd) = drive_of(&destination.to_string_lossy()) {
        if src_drives.contains(&dd) {
            res.warnings.push(format!("Destination is on the same drive ({}:) as the source - a drive failure would lose both", dd));
        }
    }
    Ok(res)
}

struct Out {
    w: BufWriter<File>,
    pos: u64,
    seq: u64,
}

impl Out {
    fn put(&mut self, b: &[u8]) -> Result<()> {
        self.w.write_all(b)?;
        self.pos += b.len() as u64;
        Ok(())
    }
}

struct Progress<'a> {
    tx: &'a Sender,
    total: u64,
    done: u64,
    last: Instant,
}

impl<'a> Progress<'a> {
    fn tick(&mut self, add: u64, file_done: u64, file_total: u64, force: bool) {
        self.done += add;
        if force || self.last.elapsed() >= Duration::from_millis(80) {
            self.last = Instant::now();
            let _ = self.tx.send(Event::Bytes { done: self.done, total: self.total, file_done, file_total });
        }
    }
}

const BATCH: usize = 8;

fn write_stream(reader: &mut dyn Read, out: &mut Out, key: &Option<Key>, level: CompressionLevel, ctl: &Control, prog: &mut Progress, file_total: u64) -> Result<(String, Vec<ChunkRef>, u64)> {
    let mut hasher = blake3::Hasher::new();
    let mut chunks = Vec::new();
    let mut total = 0u64;
    loop {
        ctl.checkpoint()?;
        let mut batch: Vec<Vec<u8>> = Vec::with_capacity(BATCH);
        let mut eof = false;
        while batch.len() < BATCH {
            let mut buf = vec![0u8; CHUNK_SIZE];
            let mut n = 0;
            while n < CHUNK_SIZE {
                let r = reader.read(&mut buf[n..])?;
                if r == 0 {
                    eof = true;
                    break;
                }
                n += r;
            }
            if n > 0 {
                buf.truncate(n);
                hasher.update(&buf);
                batch.push(buf);
            }
            if eof {
                break;
            }
        }
        if batch.is_empty() {
            break;
        }
        let base = out.seq;
        let sealed: Vec<Result<(Vec<u8>, u32)>> = batch
            .par_iter()
            .enumerate()
            .map(|(i, raw)| {
                let packed = zstd::bulk::compress(raw, level.zstd()).map_err(|e| VaultError::Compression(e.to_string()))?;
                let stored = match key {
                    Some(k) => k.seal(&packed, &(base + i as u64).to_le_bytes())?,
                    None => packed,
                };
                Ok((stored, raw.len() as u32))
            })
            .collect();
        for (i, r) in sealed.into_iter().enumerate() {
            let (stored, raw) = r?;
            let offset = out.pos;
            out.put(&(stored.len() as u32).to_le_bytes())?;
            out.put(&raw.to_le_bytes())?;
            out.put(&stored)?;
            chunks.push(ChunkRef { seq: base + i as u64, offset, stored: stored.len() as u32, raw });
            total += raw as u64;
            prog.tick(raw as u64, total, file_total, false);
        }
        out.seq = base + batch.len() as u64;
        if eof {
            break;
        }
    }
    Ok((hasher.finalize().to_hex().to_string(), chunks, total))
}

fn is_locked(e: &VaultError) -> bool {
    match e {
        VaultError::Io(io) => matches!(io.raw_os_error(), Some(32) | Some(33)) || io.kind() == std::io::ErrorKind::PermissionDenied,
        _ => false,
    }
}

pub fn run_backup(opts: &BackupOptions, ctl: &Control, tx: &Sender) -> Result<Summary> {
    let _ = tx.send(Event::Status("Scanning".into()));
    let set = exclude_set(&opts.excludes)?;
    let scan = prescan(&opts.sources, &opts.destination, &opts.excludes)?;
    let _ = tx.send(Event::Scan { files: scan.files, bytes: scan.bytes });
    let mut summary = Summary { warnings: scan.warnings.clone(), ..Default::default() };
    if let Some(parent) = opts.destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut tmp_name = opts.destination.file_name().ok_or_else(|| VaultError::InvalidPath(opts.destination.display().to_string()))?.to_os_string();
    tmp_name.push(".partial");
    let tmp = opts.destination.with_file_name(tmp_name);
    let result = write_archive(opts, &set, &scan, ctl, tx, &tmp, &mut summary);
    match result {
        Ok(()) => {
            if opts.destination.exists() {
                fs::remove_file(&opts.destination)?;
            }
            fs::rename(&tmp, &opts.destination)?;
            summary.output = Some(opts.destination.clone());
            if opts.verify {
                let _ = tx.send(Event::Status("Verifying".into()));
                let bad = verify_archive(&opts.destination, opts.password.as_deref(), ctl, tx)?;
                for b in bad {
                    summary.failed.push((b, "verification mismatch".into()));
                }
            }
            Ok(summary)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn write_archive(opts: &BackupOptions, set: &GlobSet, scan: &ScanResult, ctl: &Control, tx: &Sender, tmp: &Path, summary: &mut Summary) -> Result<()> {
    let encrypted = opts.password.as_deref().map_or(false, |p| !p.is_empty());
    let salt = random_salt()?;
    let key = if encrypted { Some(Key::derive(opts.password.as_deref().unwrap_or(""), &salt)?) } else { None };
    let mut out = Out { w: BufWriter::with_capacity(1 << 20, File::create(tmp)?), pos: 0, seq: 0 };
    out.put(MAGIC)?;
    out.put(&VERSION.to_le_bytes())?;
    out.put(&[if encrypted { 1 } else { 0 }])?;
    out.put(&salt)?;
    let mut manifest = Manifest { created: now_secs(), entries: Vec::new() };
    let mut prog = Progress { tx, total: scan.bytes, done: 0, last: Instant::now() };
    let mut index = 0u64;
    let mut snapshots: Vec<Snapshot> = Vec::new();
    for source in &opts.sources {
        ctl.checkpoint()?;
        match source {
            Source::Virtual { name, data, group } => {
                index += 1;
                let _ = tx.send(Event::File { name: name.clone(), index, total: scan.files });
                let (hash, chunks, size) = write_stream(&mut data.as_slice(), &mut out, &key, opts.level, ctl, &mut prog, data.len() as u64)?;
                let p = format!("vpk://{}/{}", group, name);
                manifest.entries.push(Entry { path: p.clone(), tpath: p, drive: None, kind: EntryKind::File, size, modified: now_secs() * 1_000_000_000, accessed: 0, created: 0, attrs: 0, hash, chunks, link_target: None, group: Some(group.clone()), virtual_entry: true });
                summary.done += 1;
            }
            Source::Path { path, group } => {
                let walker = WalkDir::new(path).follow_links(false).sort_by_file_name().into_iter().filter_entry(|e| e.depth() == 0 || !set.is_match(norm_for_match(e.path())));
                for item in walker {
                    ctl.checkpoint()?;
                    let item = match item {
                        Ok(i) => i,
                        Err(e) => {
                            summary.skipped.push((e.path().map(|p| p.display().to_string()).unwrap_or_default(), e.to_string()));
                            continue;
                        }
                    };
                    let p = item.path();
                    let meta = match fs::symlink_metadata(p) {
                        Ok(m) => m,
                        Err(e) => {
                            summary.skipped.push((p.display().to_string(), e.to_string()));
                            continue;
                        }
                    };
                    let abs = absolute_no_follow(p);
                    let (tpath, drive) = tokenise(&opts.env, &abs);
                    let mut entry = Entry {
                        path: abs.clone(),
                        tpath,
                        drive,
                        kind: EntryKind::File,
                        size: 0,
                        modified: fsmeta::nanos(meta.modified()),
                        accessed: fsmeta::nanos(meta.accessed()),
                        created: fsmeta::nanos(meta.created()),
                        attrs: fsmeta::attrs_of(&meta),
                        hash: String::new(),
                        chunks: Vec::new(),
                        link_target: None,
                        group: group.clone(),
                        virtual_entry: false,
                    };
                    if meta.file_type().is_symlink() {
                        let target = fs::read_link(p).map(|t| t.to_string_lossy().to_string()).ok();
                        entry.kind = if fsmeta::is_dir_link(&meta) && target.as_deref().map_or(false, |t| drive_of(crate::paths::strip_verbatim(t)).is_some()) { EntryKind::Junction } else { EntryKind::Symlink };
                        entry.link_target = target;
                        manifest.entries.push(entry);
                        summary.done += 1;
                        continue;
                    }
                    if meta.is_dir() {
                        entry.kind = EntryKind::Dir;
                        manifest.entries.push(entry);
                        continue;
                    }
                    index += 1;
                    let _ = tx.send(Event::File { name: p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(), index, total: scan.files });
                    let open = |path: &Path| File::open(crate::paths::to_long(path.to_path_buf())).map_err(VaultError::from);
                    let mut file = match open(p) {
                        Ok(f) => Some(f),
                        Err(e) if is_locked(&e) && opts.use_vss => {
                            let d = drive_of(&abs).unwrap_or('C');
                            if !snapshots.iter().any(|s| s.volume.starts_with(d.to_ascii_uppercase())) {
                                match vss::create(opts.system.as_ref(), d) {
                                    Ok(s) => snapshots.push(s),
                                    Err(err) => summary.warnings.push(format!("VSS unavailable: {}", err)),
                                }
                            }
                            snapshots.iter().find_map(|s| s.map(&abs)).and_then(|m| open(&m).ok())
                        }
                        Err(e) => {
                            summary.skipped.push((abs.clone(), if is_locked(&e) { "locked or access denied".into() } else { e.to_string() }));
                            None
                        }
                    };
                    let Some(f) = file.as_mut() else {
                        if opts.use_vss {
                            summary.skipped.push((abs.clone(), "locked even with shadow copy".into()));
                        }
                        continue;
                    };
                    let size = meta.len();
                    match write_stream(f, &mut out, &key, opts.level, ctl, &mut prog, size) {
                        Ok((hash, chunks, written)) => {
                            entry.size = written;
                            entry.hash = hash;
                            entry.chunks = chunks;
                            manifest.entries.push(entry);
                            summary.done += 1;
                            summary.bytes += written;
                        }
                        Err(VaultError::Cancelled) => return Err(VaultError::Cancelled),
                        Err(e) => {
                            summary.failed.push((abs, e.to_string()));
                        }
                    }
                }
            }
        }
    }
    for s in &snapshots {
        s.release(opts.system.as_ref());
    }
    prog.tick(0, 0, 0, true);
    let json = serde_json::to_vec(&manifest)?;
    let packed = zstd::bulk::compress(&json, 6).map_err(|e| VaultError::Compression(e.to_string()))?;
    let blob = match &key {
        Some(k) => k.seal(&packed, b"manifest")?,
        None => packed,
    };
    let off = out.pos;
    out.put(&blob)?;
    out.put(&off.to_le_bytes())?;
    out.put(&(blob.len() as u64).to_le_bytes())?;
    out.put(FOOTER_MAGIC)?;
    out.w.flush()?;
    out.w.get_ref().sync_all()?;
    Ok(())
}

fn absolute_no_follow(p: &Path) -> String {
    let abs = if p.is_absolute() { p.to_path_buf() } else { std::env::current_dir().map(|c| c.join(p)).unwrap_or_else(|_| p.to_path_buf()) };
    crate::paths::strip_verbatim(&abs.to_string_lossy()).to_string()
}

pub fn verify_archive(path: &Path, password: Option<&str>, ctl: &Control, tx: &Sender) -> Result<Vec<String>> {
    let archive = open(path, password)?;
    let mut f = File::open(path)?;
    let total: u64 = archive.manifest.entries.iter().map(|e| e.size).sum();
    let mut done = 0u64;
    let mut bad = Vec::new();
    for e in &archive.manifest.entries {
        ctl.checkpoint()?;
        if e.kind != EntryKind::File {
            continue;
        }
        let mut h = blake3::Hasher::new();
        let mut ok = true;
        for c in &e.chunks {
            match read_chunk(&mut f, &archive.key, c) {
                Ok(raw) => {
                    h.update(&raw);
                    done += raw.len() as u64;
                }
                Err(VaultError::Cancelled) => return Err(VaultError::Cancelled),
                Err(_) => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok || h.finalize().to_hex().as_str() != e.hash {
            bad.push(e.path.clone());
        }
        let _ = tx.send(Event::Bytes { done, total, file_done: e.size, file_total: e.size });
    }
    Ok(bad)
}
