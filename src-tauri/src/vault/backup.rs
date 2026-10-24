//! Password-protected `.roalt` backup files (portable: no DPAPI).
//!
//! Layout: `b"ROALTBK1" | m_kib u32le | t u32le | p u32le | salt[16] | seal(key, header, json)`
//! where `header` is every byte before the sealed part and `key = Argon2id(password, salt)`.

use std::path::Path;

use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::error::Result;

pub const MAGIC: &[u8; 8] = b"ROALTBK1";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupPayload {
    pub version: u32,
    pub exported_at: i64,
    pub accounts: Vec<BackupAccount>,
    pub groups: Vec<BackupGroup>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupAccount {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub alias: String,
    pub notes: String,
    pub color: Option<String>,
    pub favourite: bool,
    pub sort_order: i64,
    /// Group names.
    pub groups: Vec<String>,
    pub cookie: Zeroizing<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupGroup {
    pub name: String,
    pub color: Option<String>,
    pub sort_order: i64,
}

/// Writes atomically (temp file in the same folder, then rename).
pub fn write(path: &Path, password: &str, payload: &BackupPayload) -> Result<()> {
    let _ = (path, password, payload);
    todo!("backup: write")
}

/// `WrongPassword` when the password (or file) does not authenticate,
/// `Invalid("This isn't a RoALT backup.")` on a bad magic / truncated file.
pub fn read(path: &Path, password: &str) -> Result<BackupPayload> {
    let _ = (path, password);
    todo!("backup: read")
}
