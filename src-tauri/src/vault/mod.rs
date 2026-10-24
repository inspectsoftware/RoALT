//! Envelope encryption for everything secret.
//!
//! One random 256-bit data key encrypts every secret (AES-256-GCM, random 96-bit nonce per
//! value, AAD = row identity). The data key itself is stored wrapped:
//! - `Dpapi`: `DPAPI(data key)` for the current Windows user; opens without a prompt.
//! - `Master`: `DPAPI(seal(Argon2id(password, salt), data key))`; locked until `unlock`.
//! Switching mode only re-wraps the data key.

pub mod backup;
mod dpapi;
mod hello;
pub mod master;

use std::fmt;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::model::VaultMode;

pub use hello::windows_hello;

/// A secret string (cookie, password). Zeroed on drop, never printed.
#[derive(Clone)]
pub struct Secret(Zeroizing<String>);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// AAD for a stored cookie, so a blob cannot be moved to another account's row.
pub fn cookie_aad(user_id: i64) -> Vec<u8> {
    format!("roalt.cookie:{user_id}").into_bytes()
}

/// Argon2id parameters stored next to a master-password vault.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kdf {
    pub salt: Vec<u8>,
    pub m_kib: u32,
    pub t: u32,
    pub p: u32,
}

/// The `vault` table row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultRecord {
    pub mode: VaultMode,
    pub wrapped_key: Vec<u8>,
    /// `Some` in master mode only.
    pub kdf: Option<Kdf>,
}

struct Inner {
    mode: VaultMode,
    key: Option<Zeroizing<[u8; 32]>>,
    last_activity: Instant,
}

pub struct Vault {
    inner: Mutex<Inner>,
}

/// Shortest master password accepted.
pub const MIN_MASTER_PASSWORD: usize = 8;

impl Vault {
    /// A new vault: random data key, DPAPI-wrapped, returned unlocked.
    pub fn create() -> Result<(Vault, VaultRecord)> {
        todo!("vault: create")
    }

    /// DPAPI mode unwraps at once (unlocked). Master mode stays locked until `unlock`.
    pub fn open(rec: &VaultRecord) -> Result<Vault> {
        let _ = rec;
        todo!("vault: open")
    }

    pub fn mode(&self) -> VaultMode {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).mode
    }

    pub fn is_locked(&self) -> bool {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).key.is_none()
    }

    /// Master mode: derive, unwrap and keep the data key. `WrongPassword` on failure.
    pub fn unlock(&self, rec: &VaultRecord, password: &str) -> Result<()> {
        let _ = (rec, password);
        todo!("vault: unlock")
    }

    /// Drops (zeroes) the data key. No-op in DPAPI mode, which cannot be locked.
    pub fn lock(&self) {
        todo!("vault: lock")
    }

    /// Re-authentication in master mode (reveal / copy cookie). `WrongPassword` on mismatch.
    pub fn check_password(&self, rec: &VaultRecord, password: &str) -> Result<()> {
        let _ = (rec, password);
        todo!("vault: check_password")
    }

    /// Re-wraps the (unlocked) data key for `mode` and returns the new record.
    /// Leaving master mode needs `current_password`; entering or changing it needs
    /// `new_password` (at least `MIN_MASTER_PASSWORD` chars).
    pub fn rewrap(
        &self,
        rec: &VaultRecord,
        mode: VaultMode,
        current_password: Option<&str>,
        new_password: Option<&str>,
    ) -> Result<VaultRecord> {
        let _ = (rec, mode, current_password, new_password);
        todo!("vault: rewrap")
    }

    /// `Locked` when locked.
    pub fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let _ = (aad, plaintext);
        todo!("vault: encrypt")
    }

    /// `Locked` when locked, `Crypto` when the blob or AAD does not match.
    pub fn decrypt(&self, aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let _ = (aad, blob);
        todo!("vault: decrypt")
    }

    pub fn decrypt_secret(&self, aad: &[u8], blob: &[u8]) -> Result<Secret> {
        let bytes = self.decrypt(aad, blob)?;
        let s = std::str::from_utf8(&bytes).map_err(|_| AppError::Crypto)?;
        Ok(Secret::new(s))
    }

    /// Marks user activity (auto-lock).
    pub fn touch(&self) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).last_activity = Instant::now();
    }

    pub fn idle(&self) -> Duration {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).last_activity.elapsed()
    }
}
