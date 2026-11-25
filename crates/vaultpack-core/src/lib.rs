pub mod backup;
pub mod crypto;
pub mod error;
pub mod format;
pub mod fsmeta;
pub mod paths;
pub mod presets;
pub mod restore;
pub mod sys;
pub mod vss;

pub use backup::{default_excludes, exclude_presets, prescan, run_backup, scan_item, verify_archive, BackupOptions, ItemInfo, ScanResult, Source};
pub use error::{Result, VaultError};
pub use format::{is_encrypted, open, Archive, CompressionLevel, Control, Entry, EntryKind, Event, Manifest, Sender, Summary};
pub use paths::{Env, Remap};
pub use restore::{run_restore, target_path, ConflictPolicy, RestoreOptions};
pub use sys::{relaunch_elevated, RealSystem, System};

#[cfg(test)]
mod tests;
