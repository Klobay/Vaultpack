use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("Not a valid Vaultpack archive")]
    InvalidArchive,
    #[error("This archive is encrypted and needs a password")]
    PasswordRequired,
    #[error("Wrong password or corrupted archive")]
    WrongPassword,
    #[error("Integrity check failed: {0}")]
    Integrity(String),
    #[error("Invalid path: {0}")]
    InvalidPath(String),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Compression error: {0}")]
    Compression(String),
    #[error("Encryption error: {0}")]
    Crypto(String),
    #[error("Pattern error: {0}")]
    Pattern(String),
    #[error("Operation cancelled")]
    Cancelled,
    #[error("System command failed: {0}")]
    System(String),
}

pub type Result<T> = std::result::Result<T, VaultError>;
