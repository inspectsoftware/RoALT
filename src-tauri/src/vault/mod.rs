//! Envelope encryption for everything secret.
//!
//! One random 256-bit data key encrypts every secret (AES-256-GCM, random 96-bit nonce per
//! value, AAD = row identity). The data key itself is stored wrapped:
//! - `Dpapi`: `DPAPI(data key)` for the current Windows user; opens without a prompt.
//! - `Master`: `DPAPI(seal(Argon2id(password, salt), data key))`; locked until `unlock`.
//!
//! Switching mode only re-wraps the data key.

pub mod backup;
mod dpapi;
mod hello;
pub mod master;

use std::fmt;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zeroize::Zeroizing;

use crate::error::{AppError, Result};
use crate::model::VaultMode;

pub use dpapi::{protect_with, unprotect_with};
pub use hello::{hello_available, windows_hello};

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

/// AAD for the sealed data key in master mode.
const KEY_AAD: &[u8] = b"roalt.vault.key";

type Key = Zeroizing<[u8; 32]>;

impl Vault {
    /// A new vault: random data key, DPAPI-wrapped, returned unlocked.
    pub fn create() -> Result<(Vault, VaultRecord)> {
        let key = Zeroizing::new(master::random_bytes::<32>());
        let rec = wrap_dpapi(&key)?;
        Ok((Vault::with(VaultMode::Dpapi, Some(key)), rec))
    }

    /// DPAPI mode unwraps at once (unlocked). Master mode stays locked until `unlock`.
    pub fn open(rec: &VaultRecord) -> Result<Vault> {
        let key = match rec.mode {
            VaultMode::Dpapi => Some(unwrap_dpapi(rec)?),
            VaultMode::Master => None,
        };
        Ok(Vault::with(rec.mode, key))
    }

    fn with(mode: VaultMode, key: Option<Key>) -> Vault {
        Vault { inner: Mutex::new(Inner { mode, key, last_activity: Instant::now() }) }
    }

    fn guard(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| {
            log::warn!("vault lock was poisoned by an earlier panic; recovering");
            // Cleared so this is logged once, not on every call.
            self.inner.clear_poison();
            e.into_inner()
        })
    }

    /// A copy of the data key, or `Locked`.
    fn key(&self) -> Result<Key> {
        self.guard().key.clone().ok_or(AppError::Locked)
    }

    pub fn mode(&self) -> VaultMode {
        self.guard().mode
    }

    pub fn is_locked(&self) -> bool {
        self.guard().key.is_none()
    }

    /// Master mode: derive, unwrap and keep the data key. `WrongPassword` on failure.
    pub fn unlock(&self, rec: &VaultRecord, password: &str) -> Result<()> {
        let key = match rec.mode {
            VaultMode::Dpapi => unwrap_dpapi(rec)?,
            VaultMode::Master => unwrap_master(rec, password)?,
        };
        let mut g = self.guard();
        g.key = Some(key);
        g.last_activity = Instant::now();
        Ok(())
    }

    /// Drops (zeroes) the data key. No-op in DPAPI mode, which cannot be locked.
    pub fn lock(&self) {
        let mut g = self.guard();
        if g.mode == VaultMode::Master {
            g.key = None;
        }
    }

    /// Re-authentication in master mode (reveal / copy cookie). `WrongPassword` on mismatch.
    pub fn check_password(&self, rec: &VaultRecord, password: &str) -> Result<()> {
        unwrap_master(rec, password).map(drop)
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
        let key = self.key()?;
        let new_password = match mode {
            VaultMode::Master => Some(
                new_password.filter(|p| p.chars().count() >= MIN_MASTER_PASSWORD).ok_or_else(|| {
                    AppError::Invalid(tr!("Use at least {min} characters for the master password.", min = MIN_MASTER_PASSWORD))
                })?,
            ),
            VaultMode::Dpapi => None,
        };
        if rec.mode == VaultMode::Master {
            self.check_password(rec, current_password.ok_or(AppError::WrongPassword)?)?;
        }
        match new_password {
            Some(pw) => wrap_master(&key, pw),
            None => wrap_dpapi(&key),
        }
    }

    /// Switches the in-memory mode; call only after the record from `rewrap` is saved.
    pub fn set_mode(&self, mode: VaultMode) {
        self.guard().mode = mode;
    }

    /// `Locked` when locked.
    pub fn encrypt(&self, aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
        let key = self.key()?;
        master::seal(&key, aad, plaintext)
    }

    /// `Locked` when locked, `Crypto` when the blob or AAD does not match.
    pub fn decrypt(&self, aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let key = self.key()?;
        master::open(&key, aad, blob)
    }

    pub fn decrypt_secret(&self, aad: &[u8], blob: &[u8]) -> Result<Secret> {
        let bytes = self.decrypt(aad, blob)?;
        let s = std::str::from_utf8(&bytes).map_err(|e| {
            log::warn!("decrypted secret is not UTF-8 (valid up to byte {})", e.valid_up_to());
            AppError::Crypto
        })?;
        Ok(Secret::new(s))
    }

    /// Marks user activity (auto-lock).
    pub fn touch(&self) {
        self.guard().last_activity = Instant::now();
    }

    pub fn idle(&self) -> Duration {
        self.guard().last_activity.elapsed()
    }
}

fn wrap_dpapi(key: &[u8; 32]) -> Result<VaultRecord> {
    Ok(VaultRecord { mode: VaultMode::Dpapi, wrapped_key: dpapi::protect(key)?, kdf: None })
}

fn wrap_master(key: &[u8; 32], password: &str) -> Result<VaultRecord> {
    let kdf = master::new_kdf();
    let kek = master::derive_key(password, &kdf)?;
    let sealed = master::seal(&kek, KEY_AAD, key)?;
    Ok(VaultRecord { mode: VaultMode::Master, wrapped_key: dpapi::protect(&sealed)?, kdf: Some(kdf) })
}

fn unwrap_dpapi(rec: &VaultRecord) -> Result<Key> {
    to_key(&dpapi::unprotect(&rec.wrapped_key)?)
}

/// `WrongPassword` when `password` does not open the sealed data key.
fn unwrap_master(rec: &VaultRecord, password: &str) -> Result<Key> {
    let kdf = rec.kdf.as_ref().ok_or_else(|| {
        log::error!("vault record is in master mode but has no KDF parameters");
        AppError::Internal(tr!("No master password is set."))
    })?;
    let sealed = dpapi::unprotect(&rec.wrapped_key)?;
    let kek = master::derive_key(password, kdf)?;
    to_key(&master::open(&kek, KEY_AAD, &sealed).map_err(|_| AppError::WrongPassword)?)
}

fn to_key(bytes: &[u8]) -> Result<Key> {
    if bytes.len() != 32 {
        log::error!("vault data key has the wrong length ({} bytes)", bytes.len());
        return Err(AppError::Crypto);
    }
    let mut key = Zeroizing::new([0u8; 32]);
    key.copy_from_slice(bytes);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AAD: &[u8] = b"roalt.cookie:1";

    #[test]
    fn create_encrypt_decrypt_and_reopen() {
        let (v, rec) = Vault::create().unwrap();
        assert_eq!((v.mode(), v.is_locked(), rec.kdf.is_none()), (VaultMode::Dpapi, false, true));
        let blob = v.encrypt(AAD, b"cookie").unwrap();
        assert_eq!(v.decrypt(AAD, &blob).unwrap().as_slice(), b"cookie");
        assert!(matches!(v.decrypt(b"roalt.cookie:2", &blob), Err(AppError::Crypto)));
        v.lock();
        let reopened = Vault::open(&rec).unwrap();
        assert_eq!(reopened.decrypt_secret(AAD, &blob).unwrap().expose(), "cookie");
    }

    #[test]
    fn master_mode_round_trip() {
        let (v, rec) = Vault::create().unwrap();
        let blob = v.encrypt(AAD, b"cookie").unwrap();

        assert!(matches!(v.rewrap(&rec, VaultMode::Master, None, Some("short")), Err(AppError::Invalid(_))));
        let rec = v.rewrap(&rec, VaultMode::Master, None, Some("correct horse")).unwrap();
        v.set_mode(rec.mode);
        assert_eq!((v.mode(), rec.mode, rec.kdf.is_some()), (VaultMode::Master, VaultMode::Master, true));

        v.lock();
        assert!(v.is_locked());
        assert!(matches!(v.decrypt(AAD, &blob), Err(AppError::Locked)));
        assert!(matches!(v.rewrap(&rec, VaultMode::Dpapi, Some("correct horse"), None), Err(AppError::Locked)));
        assert!(matches!(v.unlock(&rec, "wrong password"), Err(AppError::WrongPassword)));

        let reopened = Vault::open(&rec).unwrap();
        assert!(reopened.is_locked());
        reopened.unlock(&rec, "correct horse").unwrap();
        assert_eq!(reopened.decrypt(AAD, &blob).unwrap().as_slice(), b"cookie");
        assert!(matches!(reopened.check_password(&rec, "nope nope"), Err(AppError::WrongPassword)));

        let changed = reopened.rewrap(&rec, VaultMode::Master, Some("correct horse"), Some("battery staple")).unwrap();
        assert!(matches!(reopened.unlock(&changed, "correct horse"), Err(AppError::WrongPassword)));
        let back = reopened.rewrap(&changed, VaultMode::Dpapi, Some("correct horse"), None);
        assert!(matches!(back, Err(AppError::WrongPassword)));

        let back = reopened.rewrap(&changed, VaultMode::Dpapi, Some("battery staple"), None).unwrap();
        reopened.set_mode(back.mode);
        assert_eq!((reopened.mode(), back.mode, back.kdf.is_none()), (VaultMode::Dpapi, VaultMode::Dpapi, true));
        assert_eq!(Vault::open(&back).unwrap().decrypt(AAD, &blob).unwrap().as_slice(), b"cookie");
    }
}
