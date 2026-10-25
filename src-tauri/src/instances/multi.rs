//! Multi-instance: hold a mutex named exactly `ROBLOX_singletonEvent` (no prefix, not owned)
//! before any client starts, so a new client can't create its singleton event and doesn't
//! close the running one (research multi-instance M1-M5). Optionally, while RoALT's clients
//! run, hold Roblox's shared login files (`%LOCALAPPDATA%\Roblox\LocalStorage\RobloxCookies.dat`,
//! `appStorage.json`) open with share mode 0 so clients can't overwrite each other's session (M6).

use std::fs::{File, OpenOptions};
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::{ERROR_INVALID_HANDLE, GENERIC_READ, HANDLE};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::{HSTRING, Owned};

use super::process;
use crate::error::{AppError, Result};
use crate::model::MultiInstanceStatus;

pub const SINGLETON_NAME: &str = "ROBLOX_singletonEvent";

const RUNNING: &str = "Roblox is already running. Close it (including its tray icon) to run several clients.";

pub struct Multi {
    inner: Mutex<Option<Held>>,
}

/// The mutex handle and the locked files; dropping it releases everything.
struct Held {
    _mutex: Singleton,
    /// Empty unless RoALT's clients run with protection on.
    files: Vec<File>,
}

/// Our handle to the named mutex; closed on drop.
struct Singleton {
    _handle: Owned<HANDLE>,
}

// SAFETY: a kernel handle is valid on any thread of the process; it is never used after
// creation except by CloseHandle on drop, which may run on any thread.
unsafe impl Send for Singleton {}
// SAFETY: as above; `&Singleton` gives no access to the handle.
unsafe impl Sync for Singleton {}

impl Multi {
    pub fn new() -> Multi {
        Multi { inner: Mutex::new(None) }
    }

    /// Takes the lock (no-op when held). The name decides: if it already belongs to an event
    /// (`ERROR_INVALID_HANDLE`), Roblox is running unprotected and nothing is held; the error
    /// says Roblox must be closed first. Clients RoALT started under an earlier lock never
    /// created that event, so the lock can be taken again while they run (e.g. after a
    /// restart). An existing *mutex* of that name (another tool) is fine: it blocks clients the
    /// same way.
    pub fn enable(&self) -> Result<()> {
        self.take(SINGLETON_NAME)
    }

    /// Releases the mutex and the files.
    pub fn disable(&self) {
        *self.held() = None;
    }

    pub fn is_active(&self) -> bool {
        self.held().is_some()
    }

    /// Locks both shared login files while the lock is held, creating missing ones (a fresh
    /// install has none yet). No-op when both are already locked; a busy file is logged and
    /// tried again next time.
    pub fn lock_shared(&self) {
        self.lock_shared_at(&shared_files());
    }

    /// Releases the shared login files, keeping the lock.
    pub fn release_shared(&self) {
        if let Some(h) = self.held().as_mut() {
            h.files.clear();
        }
    }

    fn lock_shared_at(&self, paths: &[PathBuf]) {
        if let Some(h) = self.held().as_mut()
            && h.files.len() < paths.len()
        {
            // Our own handles would make the new opens fail: drop them first.
            h.files.clear();
            h.files = lock_files(paths);
        }
    }

    /// For the UI; `blocked_by` lists running RobloxPlayerBeta.exe PIDs when not active.
    pub fn status(&self, enabled: bool, protect_shared_login: bool) -> MultiInstanceStatus {
        let active = self.is_active();
        let blocked_by: Vec<u32> =
            if active { Vec::new() } else { process::roblox_processes().iter().map(|p| p.pid).collect() };
        let message = match (enabled && !active, blocked_by.is_empty()) {
            (false, _) => None,
            (true, false) => Some(RUNNING.to_owned()),
            (true, true) => Some("Multi-instance isn't active yet. Turn it off and on again to retry.".to_owned()),
        };
        MultiInstanceStatus { enabled, active, protect_shared_login, blocked_by, message }
    }

    /// Creates (or opens) the mutex `name`; no-op when already held.
    fn take(&self, name: &str) -> Result<()> {
        let mut held = self.held();
        if held.is_none() {
            *held = Some(Held { _mutex: create_mutex(name)?, files: Vec::new() });
        }
        Ok(())
    }

    fn held(&self) -> MutexGuard<'_, Option<Held>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Default for Multi {
    fn default() -> Self {
        Self::new()
    }
}

/// Success, with or without ERROR_ALREADY_EXISTS (another tool's mutex blocks clients the same
/// way), means held. ERROR_INVALID_HANDLE means the name belongs to an event, i.e. Roblox's.
fn create_mutex(name: &str) -> Result<Singleton> {
    // SAFETY: CreateMutexW only reads the name; on success the handle is ours to close.
    let h = unsafe { CreateMutexW(None, false, &HSTRING::from(name)) }.map_err(|e| {
        if e.code() == ERROR_INVALID_HANDLE.to_hresult() {
            AppError::Invalid(RUNNING.into())
        } else {
            AppError::Internal(format!("Could not take the multi-instance lock: {}", e.message()))
        }
    })?;
    // SAFETY: as above.
    Ok(Singleton { _handle: unsafe { Owned::new(h) } })
}

/// Opens each file read-only with share mode 0 so nothing else can open it until it is
/// dropped. A missing file (and its folder) is created empty, as evanovar does (OPEN_ALWAYS:
/// `write` only lets std create it, `access_mode` keeps the access read-only, nothing is
/// truncated). A busy one is logged and skipped (the mutex alone still works).
fn lock_files(paths: &[PathBuf]) -> Vec<File> {
    paths
        .iter()
        .filter_map(|p| {
            if let Some(dir) = p.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match OpenOptions::new().write(true).create(true).access_mode(GENERIC_READ.0).share_mode(0).open(p) {
                Ok(f) => Some(f),
                Err(e) => {
                    log::warn!("could not lock {}: {e}", p.display());
                    None
                }
            }
        })
        .collect()
}

fn shared_files() -> Vec<PathBuf> {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
    let dir = PathBuf::from(base).join("Roblox").join("LocalStorage");
    vec![dir.join("RobloxCookies.dat"), dir.join("appStorage.json")]
}

#[cfg(test)]
mod tests {
    use windows::Win32::Foundation::ERROR_SHARING_VIOLATION;
    use windows::Win32::System::Threading::CreateEventW;

    use super::*;

    /// Never the real name: tests must not touch Roblox's singleton.
    fn test_name() -> String {
        format!("RoALT_test_singleton_{}", uuid::Uuid::new_v4())
    }

    /// What a starting client does: create an event of that name (closed again right away).
    fn event(name: &str) -> windows::core::Result<Owned<HANDLE>> {
        // SAFETY: CreateEventW only reads the name; the handle is closed on drop.
        unsafe { CreateEventW(None, false, false, &HSTRING::from(name)).map(|h| Owned::new(h)) }
    }

    #[test]
    fn holds_and_releases_the_name() {
        let name = test_name();
        let (a, b) = (Multi::new(), Multi::new());
        assert!(!a.status(false, false).active && a.status(false, false).message.is_none());
        a.take(&name).unwrap();
        assert!(a.is_active());
        let s = a.status(true, true);
        assert!(s.active && s.blocked_by.is_empty() && s.message.is_none() && s.protect_shared_login);
        // A client can't create its event while the mutex holds the name.
        assert_eq!(event(&name).unwrap_err().code(), ERROR_INVALID_HANDLE.to_hresult());
        // Creating the same mutex again (ERROR_ALREADY_EXISTS) still counts as held.
        b.take(&name).unwrap();
        assert!(b.is_active());
        a.disable();
        b.disable();
        assert!(!a.is_active() && !b.is_active());
        event(&name).unwrap();
    }

    #[test]
    fn an_existing_event_means_roblox_runs() {
        let name = test_name();
        let _roblox = event(&name).unwrap();
        let m = Multi::new();
        let err = m.take(&name).unwrap_err();
        assert!(matches!(err, AppError::Invalid(ref s) if s == RUNNING), "{err}");
        assert!(!m.is_active());
    }

    #[test]
    fn locks_shared_files_while_held_and_creates_missing_ones() {
        let dir = std::env::temp_dir().join(format!("roalt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("RobloxCookies.dat");
        std::fs::write(&file, b"{}").unwrap();
        let missing = dir.join("LocalStorage").join("appStorage.json");
        let files = [file.clone(), missing.clone()];
        let locked = |p: &PathBuf| File::open(p).unwrap_err().raw_os_error() == Some(ERROR_SHARING_VIOLATION.0 as i32);

        let m = Multi::new();
        m.lock_shared_at(&files); // not held: nothing
        File::open(&file).unwrap();
        m.take(&test_name()).unwrap();
        m.lock_shared_at(&files);
        assert!(locked(&file) && locked(&missing));
        m.lock_shared_at(&files); // both held: kept, not reopened
        assert!(locked(&file) && locked(&missing));
        // A busy file is skipped, not an error.
        let other = Multi::new();
        other.take(&test_name()).unwrap();
        other.lock_shared_at(&files);
        assert!(other.is_active());

        // Releasing frees the files but keeps the lock; contents are untouched.
        m.release_shared();
        assert_eq!(std::fs::read(&file).unwrap(), b"{}");
        assert!(m.is_active());
        m.lock_shared_at(&files);
        assert!(locked(&file));
        m.disable();
        File::open(&file).unwrap();
        other.disable();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
