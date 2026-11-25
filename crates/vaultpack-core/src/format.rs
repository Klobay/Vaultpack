use crate::crypto::{Key, SALT_LEN};
use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const MAGIC: &[u8; 8] = b"VPKARCH2";
pub const FOOTER_MAGIC: &[u8; 8] = b"VPKEND02";
pub const VERSION: u32 = 2;
pub const CHUNK_SIZE: usize = 4 * 1024 * 1024;
pub const HEADER_LEN: u64 = 8 + 4 + 1 + SALT_LEN as u64;
pub const FOOTER_LEN: u64 = 8 + 8 + 8;
pub const ATTR_READONLY: u32 = 1;
pub const ATTR_HIDDEN: u32 = 2;
pub const ATTR_SYSTEM: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CompressionLevel {
    Fast,
    Recommended,
    Maximum,
}

impl CompressionLevel {
    pub fn zstd(self) -> i32 {
        match self {
            Self::Fast => 1,
            Self::Recommended => 6,
            Self::Maximum => 19,
        }
    }

    pub fn from_index(i: i32) -> Self {
        match i {
            0 => Self::Fast,
            2 => Self::Maximum,
            _ => Self::Recommended,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Junction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkRef {
    pub seq: u64,
    pub offset: u64,
    pub stored: u32,
    pub raw: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: String,
    pub tpath: String,
    pub drive: Option<char>,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: i64,
    pub accessed: i64,
    pub created: i64,
    pub attrs: u32,
    pub hash: String,
    pub chunks: Vec<ChunkRef>,
    pub link_target: Option<String>,
    pub group: Option<String>,
    pub virtual_entry: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub created: i64,
    pub entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct Archive {
    pub path: std::path::PathBuf,
    pub key: Option<Key>,
    pub manifest: Manifest,
    pub encrypted: bool,
}

#[derive(Debug, Default)]
pub struct Control {
    cancel: AtomicBool,
    pause: AtomicBool,
}

impl Control {
    pub fn new() -> Arc<Control> {
        Arc::new(Control::default())
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    pub fn set_paused(&self, v: bool) {
        self.pause.store(v, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    pub fn checkpoint(&self) -> Result<()> {
        while self.is_paused() && !self.is_cancelled() {
            std::thread::sleep(std::time::Duration::from_millis(80));
        }
        if self.is_cancelled() {
            Err(VaultError::Cancelled)
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Status(String),
    Scan { files: u64, bytes: u64 },
    File { name: String, index: u64, total: u64 },
    Bytes { done: u64, total: u64, file_done: u64, file_total: u64 },
    Warning(String),
    Done(Summary),
    Failed(String),
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub done: u64,
    pub skipped: Vec<(String, String)>,
    pub failed: Vec<(String, String)>,
    pub warnings: Vec<String>,
    pub bytes: u64,
    pub manual_programs: Vec<crate::presets::Program>,
    pub output: Option<std::path::PathBuf>,
}

impl Summary {
    pub fn log(&self) -> String {
        let mut s = format!("Processed: {}\nBytes: {}\nSkipped: {}\nFailed: {}\n", self.done, self.bytes, self.skipped.len(), self.failed.len());
        for (p, r) in &self.skipped {
            s.push_str(&format!("SKIPPED  {}  ({})\n", p, r));
        }
        for (p, r) in &self.failed {
            s.push_str(&format!("FAILED   {}  ({})\n", p, r));
        }
        for w in &self.warnings {
            s.push_str(&format!("WARNING  {}\n", w));
        }
        s
    }
}

pub type Sender = std::sync::mpsc::Sender<Event>;

pub fn read_header(file: &mut File) -> Result<(bool, [u8; SALT_LEN])> {
    let len = file.metadata()?.len();
    if len < HEADER_LEN + FOOTER_LEN {
        return Err(VaultError::InvalidArchive);
    }
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 8];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(VaultError::InvalidArchive);
    }
    let mut v = [0u8; 4];
    file.read_exact(&mut v)?;
    if u32::from_le_bytes(v) != VERSION {
        return Err(VaultError::InvalidArchive);
    }
    let mut flag = [0u8; 1];
    file.read_exact(&mut flag)?;
    let mut salt = [0u8; SALT_LEN];
    file.read_exact(&mut salt)?;
    Ok((flag[0] & 1 == 1, salt))
}

pub fn is_encrypted(path: &Path) -> Result<bool> {
    let mut f = File::open(path)?;
    Ok(read_header(&mut f)?.0)
}

pub fn open(path: &Path, password: Option<&str>) -> Result<Archive> {
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let (encrypted, salt) = read_header(&mut f)?;
    f.seek(SeekFrom::End(-(FOOTER_LEN as i64)))?;
    let mut buf = [0u8; 24];
    f.read_exact(&mut buf)?;
    let mut tail = [0u8; 8];
    tail.copy_from_slice(&buf[16..24]);
    if &tail != FOOTER_MAGIC {
        return Err(VaultError::InvalidArchive);
    }
    let off = u64::from_le_bytes(buf[0..8].try_into().unwrap());
    let mlen = u64::from_le_bytes(buf[8..16].try_into().unwrap());
    if off < HEADER_LEN || off.checked_add(mlen).map_or(true, |e| e > len - FOOTER_LEN) || mlen > (1 << 31) {
        return Err(VaultError::InvalidArchive);
    }
    f.seek(SeekFrom::Start(off))?;
    let mut blob = vec![0u8; mlen as usize];
    f.read_exact(&mut blob)?;
    let key = if encrypted {
        let pw = password.filter(|p| !p.is_empty()).ok_or(VaultError::PasswordRequired)?;
        Some(Key::derive(pw, &salt)?)
    } else {
        None
    };
    let packed = match &key {
        Some(k) => k.open(&blob, b"manifest")?,
        None => blob,
    };
    let json = zstd::stream::decode_all(packed.as_slice()).map_err(|_| VaultError::InvalidArchive)?;
    let manifest: Manifest = serde_json::from_slice(&json).map_err(|_| VaultError::InvalidArchive)?;
    Ok(Archive { path: path.to_path_buf(), key, manifest, encrypted })
}

pub fn read_chunk(f: &mut File, key: &Option<Key>, c: &ChunkRef) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(c.offset))?;
    let mut hdr = [0u8; 8];
    f.read_exact(&mut hdr)?;
    let stored = u32::from_le_bytes(hdr[0..4].try_into().unwrap());
    let raw = u32::from_le_bytes(hdr[4..8].try_into().unwrap());
    if stored != c.stored || raw != c.raw || raw as usize > CHUNK_SIZE || stored as usize > CHUNK_SIZE * 2 {
        return Err(VaultError::InvalidArchive);
    }
    let mut data = vec![0u8; stored as usize];
    f.read_exact(&mut data)?;
    let packed = match key {
        Some(k) => k.open(&data, &c.seq.to_le_bytes())?,
        None => data,
    };
    let out = zstd::bulk::decompress(&packed, raw as usize).map_err(|e| VaultError::Compression(e.to_string()))?;
    if out.len() != raw as usize {
        return Err(VaultError::Integrity("chunk size mismatch".into()));
    }
    Ok(out)
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
