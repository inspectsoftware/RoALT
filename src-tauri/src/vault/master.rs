//! Argon2id key derivation and AES-256-GCM sealing shared by the vault and backups.

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use zeroize::Zeroizing;

use super::Kdf;
use crate::error::{AppError, Result};

/// RFC 9106 second recommendation: 64 MiB, 3 passes, 4 lanes.
pub const DEFAULT_M_KIB: u32 = 64 * 1024;
pub const DEFAULT_T: u32 = 3;
pub const DEFAULT_P: u32 = 4;

const NONCE_LEN: usize = 12;

/// `n` bytes from the OS RNG.
pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    // ProcessPrng (Windows 10+) is documented never to fail, so there is nothing to recover.
    getrandom::fill(&mut b).expect("OS random number generator failed");
    b
}

/// A fresh `Kdf` with the default cost and a random 16-byte salt.
pub fn new_kdf() -> Kdf {
    let (m_kib, t, p) = COST;
    Kdf { salt: random_bytes::<16>().to_vec(), m_kib, t, p }
}

#[cfg(not(test))]
const COST: (u32, u32, u32) = (DEFAULT_M_KIB, DEFAULT_T, DEFAULT_P);
// ponytail: unoptimised Argon2 takes ~3 s at the default cost, so tests derive with a cheap one.
#[cfg(test)]
const COST: (u32, u32, u32) = (1024, 1, 1);

/// Argon2id(password, kdf) → 32-byte key.
pub fn derive_key(password: &str, kdf: &Kdf) -> Result<Zeroizing<[u8; 32]>> {
    let params = Params::new(kdf.m_kib, kdf.t, kdf.p, Some(32)).map_err(|e| {
        log::warn!("bad argon2 parameters (m {} KiB, t {}, p {}): {e}", kdf.m_kib, kdf.t, kdf.p);
        AppError::Crypto
    })?;
    let mut key = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &kdf.salt, key.as_mut())
        .map_err(|e| {
            log::warn!("argon2 hashing failed: {e}");
            AppError::Crypto
        })?;
    Ok(key)
}

/// `nonce (12) || ciphertext+tag`.
pub fn seal(key: &[u8; 32], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce = random_bytes::<NONCE_LEN>();
    let ct = Aes256Gcm::new(key.into())
        .encrypt(&Nonce::from(nonce), Payload { msg: plaintext, aad })
        .map_err(|_| AppError::Crypto)?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// Inverse of `seal`; `AppError::Crypto` when the key, AAD or blob is wrong.
pub fn open(key: &[u8; 32], aad: &[u8], blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let (nonce, ct) = blob.split_first_chunk::<NONCE_LEN>().ok_or(AppError::Crypto)?;
    Aes256Gcm::new(key.into())
        .decrypt(&Nonce::from(*nonce), Payload { msg: ct, aad })
        .map(Zeroizing::new)
        .map_err(|_| AppError::Crypto)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trip() {
        let key = random_bytes::<32>();
        let blob = seal(&key, b"aad", b"secret").unwrap();
        assert_eq!(open(&key, b"aad", &blob).unwrap().as_slice(), b"secret");
        assert_ne!(seal(&key, b"aad", b"secret").unwrap(), blob, "nonce must be fresh");
    }

    #[test]
    fn open_rejects_wrong_key_aad_or_blob() {
        let key = random_bytes::<32>();
        let blob = seal(&key, b"aad", b"secret").unwrap();
        assert!(matches!(open(&random_bytes::<32>(), b"aad", &blob), Err(AppError::Crypto)));
        assert!(matches!(open(&key, b"other", &blob), Err(AppError::Crypto)));
        let mut tampered = blob.clone();
        *tampered.last_mut().unwrap() ^= 1;
        assert!(matches!(open(&key, b"aad", &tampered), Err(AppError::Crypto)));
        assert!(matches!(open(&key, b"aad", &blob[..5]), Err(AppError::Crypto)));
    }

    #[test]
    fn derive_key_is_deterministic_per_password() {
        let kdf = new_kdf();
        let a = derive_key("hunter22", &kdf).unwrap();
        assert_eq!(*a, *derive_key("hunter22", &kdf).unwrap());
        assert_ne!(*a, *derive_key("hunter23", &kdf).unwrap());
        assert!(Params::new(DEFAULT_M_KIB, DEFAULT_T, DEFAULT_P, Some(32)).is_ok());
    }
}
