//! Per-account browser state, encrypted with the vault while not in use.
//! Restored only into tmp-webview, which is swept after an unclean shutdown.
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use zeroize::Zeroizing;
use crate::error::{AppError, Result};
use crate::vault::Vault;

const MAGIC: &[u8] = b"ROALT_PROFILE1";
const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_FILES: usize = 8192;
static ACTIVE: LazyLock<Mutex<HashSet<i64>>> = LazyLock::new(|| Mutex::new(HashSet::new()));
static FILES: Mutex<()> = Mutex::new(());

pub struct Lease(i64);
impl Lease {
    pub fn take(user: i64) -> Option<Self> {
        ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(user).then(|| Self(user))
    }
}
impl Drop for Lease {
    fn drop(&mut self) { ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.0); }
}
fn bad() -> AppError { AppError::Storage("Browser session snapshot is invalid or too large.".into()) }
fn aad(user: i64) -> Vec<u8> { format!("roalt.browser-session.v1.{user}").into_bytes() }
fn archive(root: &Path, user: i64) -> PathBuf { root.join("browser-sessions").join(format!("{user}.bin")) }

// Include durable browser state (cookies, local storage, IndexedDB, device keys), not caches.
fn cache(name: &str) -> bool {
    matches!(name, "Cache" | "Code Cache" | "GPUCache" | "DawnCache" | "ShaderCache" | "GrShaderCache" | "Crashpad" | "BrowserMetrics" | "CacheStorage" | "component_crx_cache" | "optimization_guide_model_store")
}
fn regular(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    !meta.is_symlink() && meta.file_attributes() & 0x400 == 0
}
fn pack(root: &Path) -> Result<Zeroizing<Vec<u8>>> {
    let mut bytes = Zeroizing::new(MAGIC.to_vec());
    let mut count = 0;
    collect(root, root, &mut bytes, &mut count, 0)?;
    Ok(bytes)
}
fn collect(root: &Path, dir: &Path, bytes: &mut Vec<u8>, count: &mut usize, depth: usize) -> Result<()> {
    if depth > 32 { return Err(bad()); }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let meta = std::fs::symlink_metadata(&path)?;
        if !regular(&meta) || cache(&entry.file_name().to_string_lossy()) { continue; }
        if meta.is_dir() { collect(root, &path, bytes, count, depth + 1)?; }
        else if meta.is_file() {
            *count += 1;
            let name = path.strip_prefix(root).map_err(|_| bad())?.to_str().ok_or_else(bad)?.replace('\\', "/");
            if *count > MAX_FILES || name.len() > 4096 || meta.len() > MAX_BYTES as u64 { return Err(bad()); }
            if bytes.len() + name.len() + 12 > MAX_BYTES { return Err(bad()); }
            let remaining = MAX_BYTES - bytes.len() - name.len() - 12;
            if meta.len() > remaining as u64 { return Err(bad()); }
            let mut content = Zeroizing::new(Vec::new());
            std::fs::File::open(&path)?.take(remaining as u64 + 1).read_to_end(&mut content)?;
            if content.len() > remaining { return Err(bad()); }
            bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
            bytes.extend_from_slice(&(content.len() as u64).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(&content);
        }
    }
    Ok(())
}
fn relative(name: &str) -> Result<&Path> {
    // No Windows drive, ADS, parent traversal, or alternative separator tricks.
    if name.is_empty() || name.contains(['\\', ':', '\0']) { return Err(bad()); }
    let p = Path::new(name);
    if !p.components().all(|c| matches!(c, Component::Normal(_))) { return Err(bad()); }
    Ok(p)
}
fn unpack(bytes: &[u8], root: &Path) -> Result<()> {
    let mut rest = bytes.strip_prefix(MAGIC).ok_or_else(bad)?;
    let mut count = 0;
    while !rest.is_empty() {
        count += 1;
        if count > MAX_FILES || rest.len() < 12 { return Err(bad()); }
        let n = u32::from_le_bytes(rest[..4].try_into().map_err(|_| bad())?) as usize;
        let size = u64::from_le_bytes(rest[4..12].try_into().map_err(|_| bad())?);
        rest = &rest[12..];
        if n > 4096 || n > rest.len() || size > MAX_BYTES as u64 || size as usize > rest.len() - n { return Err(bad()); }
        let name = std::str::from_utf8(&rest[..n]).map_err(|_| bad())?;
        let path = root.join(relative(name)?);
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        // The caller supplies an empty, isolated directory; never overwrite another entry.
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&rest[n..n + size as usize])?;
        rest = &rest[n + size as usize..];
    }
    Ok(())
}
pub fn seal(vault: &Vault, user: i64, profile: &Path) -> Result<Vec<u8>> {
    vault.encrypt(&aad(user), &pack(profile)?)
}
pub fn restore(vault: &Vault, root: &Path, user: i64, profile: &Path) -> Result<bool> {
    let _guard = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let path = archive(root, user);
    let mut blob = Vec::new();
    match std::fs::File::open(path) {
        Ok(file) => { file.take(MAX_BYTES as u64 + 129).read_to_end(&mut blob)?; }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e.into()),
    }
    if blob.len() > MAX_BYTES + 128 { return Err(bad()); }
    let plain = vault.decrypt(&aad(user), &blob)?;
    std::fs::create_dir_all(profile)?;
    unpack(&plain, profile)?;
    Ok(true)
}
/// A removed account cannot regain a snapshot from a window finishing concurrently.
pub fn store(app: &tauri::AppHandle, user: i64, blob: &[u8]) -> Result<()> {
    use tauri::Manager;
    let _guard = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let state = app.state::<crate::App>();
    if !state.db.accounts()?.iter().any(|a| a.user_id == user) { return Ok(()); }
    let target = archive(&state.data_dir, user);
    std::fs::create_dir_all(target.parent().ok_or_else(bad)?)?;
    let temp = target.with_extension("new");
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(blob)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(temp, target)?;
    Ok(())
}
pub fn remove_accounts(state: &crate::App, ids: &[i64]) -> Result<()> {
    let _guard = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let users: Vec<_> = state.db.accounts()?.into_iter().filter(|a| ids.contains(&a.id)).map(|a| a.user_id).collect();
    // Remove snapshots before deleting rows; failure leaves the accounts available to retry.
    for user in users {
        for path in [archive(&state.data_dir, user), archive(&state.data_dir, user).with_extension("new")] {
            match std::fs::remove_file(path) {
                Ok(()) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(e.into()),
            }
        }
    }
    state.db.remove_accounts(ids)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_roundtrip_is_encrypted_and_bound_to_account() {
        let root = std::env::temp_dir().join(format!("roalt-profile-test-{}", uuid::Uuid::new_v4()));
        let source = root.join("source");
        std::fs::create_dir_all(source.join("Default/IndexedDB")).unwrap();
        std::fs::create_dir_all(source.join("Default/Cache")).unwrap();
        std::fs::write(source.join("Default/IndexedDB/key"), b"private-browser-key").unwrap();
        std::fs::write(source.join("Default/Cache/entry"), b"skip").unwrap();
        let (vault, _) = Vault::create().unwrap();
        let blob = seal(&vault, 42, &source).unwrap();
        assert!(!blob.windows(19).any(|w| w == b"private-browser-key"));
        assert!(vault.decrypt(&aad(43), &blob).is_err());
        let plain = vault.decrypt(&aad(42), &blob).unwrap();
        let dest = root.join("dest");
        std::fs::create_dir_all(root.join("browser-sessions")).unwrap();
        std::fs::write(archive(&root, 42), &blob).unwrap();
        assert!(restore(&vault, &root, 42, &dest).unwrap());
        std::fs::write(archive(&root, 43), &blob).unwrap();
        assert!(restore(&vault, &root, 43, &root.join("other")).is_err());
        assert!(!restore(&vault, &root, 44, &root.join("absent")).unwrap());
        assert!(!plain.is_empty());
        assert_eq!(std::fs::read(dest.join("Default/IndexedDB/key")).unwrap(), b"private-browser-key");
        assert!(!dest.join("Default/Cache").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn refuses_path_traversal_and_corrupt_snapshots() {
        for name in ["../escape", "/absolute", "C:/escape", "file:stream", "a\\..\\b", ""] { assert!(relative(name).is_err(), "{name}"); }
        assert!(relative("Default/IndexedDB/key").is_ok());
        let root = std::env::temp_dir().join("roalt-unused-invalid-profile");
        assert!(unpack(b"invalid", &root).is_err());
        let mut truncated = MAGIC.to_vec();
        truncated.extend_from_slice(&[0; 11]);
        assert!(unpack(&truncated, &root).is_err());
    }
    #[test]
    fn one_window_per_account_and_release_on_drop() {
        let lease = Lease::take(987654321).unwrap();
        assert!(Lease::take(987654321).is_none());
        assert!(Lease::take(987654322).is_some());
        drop(lease);
        assert!(Lease::take(987654321).is_some());
    }
}
