//! Argon2id key derivation and AES-256-GCM sealing shared by the vault and backups.

use zeroize::Zeroizing;

use super::Kdf;
use crate::error::Result;

/// RFC 9106 second recommendation: 64 MiB, 3 passes, 4 lanes.
pub const DEFAULT_M_KIB: u32 = 64 * 1024;
pub const DEFAULT_T: u32 = 3;
pub const DEFAULT_P: u32 = 4;

/// `n` bytes from the OS RNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    todo!("master: random_bytes")
}

/// A fresh `Kdf` with the default cost and a random 16-byte salt.
pub fn new_kdf() -> Kdf {
    todo!("master: new_kdf")
}

/// Argon2id(password, kdf) → 32-byte key.
pub fn derive_key(password: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>> {
    let _ = (password, kdf);
    todo!("master: derive_key")
}

/// `nonce (12) || ciphertext+tag`.
pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let _ = (key, aad, plaintext);
    todo!("master: seal")
}

/// Inverse of `seal`; `AppError::Crypto` when the key, AAD or blob is wrong.
pub fn open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let _ = (key, aad, blob);
    todo!("master: open")
}
