//! Windows DPAPI (current user) with an app-specific entropy.

use zeroize::Zeroizing;

use crate::error::Result;

pub const ENTROPY: &[u8] = b"RoALT/vault/v1";

pub fn protect(data: &[u8]) -> Result<Vec<u8>> {
    let _ = data;
    todo!("dpapi: protect")
}

pub fn unprotect(blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    let _ = blob;
    todo!("dpapi: unprotect")
}
