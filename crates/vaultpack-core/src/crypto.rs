use crate::error::{Result, VaultError};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{aead::{Aead, KeyInit}, XChaCha20Poly1305, XNonce};

pub const SALT_LEN: usize = 16;
pub const NONCE_LEN: usize = 24;
pub const TAG_LEN: usize = 16;

#[derive(Clone)]
pub struct Key([u8; 32]);

impl Key {
    pub fn derive(password: &str, salt: &[u8]) -> Result<Key> {
        let params = Params::new(32 * 1024, 3, 2, Some(32)).map_err(|e| VaultError::Crypto(e.to_string()))?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut out = [0u8; 32];
        argon.hash_password_into(password.as_bytes(), salt, &mut out).map_err(|e| VaultError::Crypto(e.to_string()))?;
        Ok(Key(out))
    }

    pub fn seal(&self, plain: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::getrandom(&mut nonce).map_err(|e| VaultError::Crypto(e.to_string()))?;
        let ct = cipher
            .encrypt(XNonce::from_slice(&nonce), chacha20poly1305::aead::Payload { msg: plain, aad })
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
        out.extend_from_slice(&nonce);
        out.extend_from_slice(&ct);
        Ok(out)
    }

    pub fn open(&self, sealed: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
        if sealed.len() < NONCE_LEN + TAG_LEN {
            return Err(VaultError::InvalidArchive);
        }
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let (nonce, ct) = sealed.split_at(NONCE_LEN);
        cipher
            .decrypt(XNonce::from_slice(nonce), chacha20poly1305::aead::Payload { msg: ct, aad })
            .map_err(|_| VaultError::WrongPassword)
    }
}

pub fn random_salt() -> Result<[u8; SALT_LEN]> {
    let mut salt = [0u8; SALT_LEN];
    getrandom::getrandom(&mut salt).map_err(|e| VaultError::Crypto(e.to_string()))?;
    Ok(salt)
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Key(..)")
    }
}
