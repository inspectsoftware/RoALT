//! Multi-instance: hold a mutex named exactly `ROBLOX_singletonEvent` (no prefix, not owned)
//! before any client starts, so a new client can't create its singleton event and doesn't
//! close the running one (research multi-instance M1-M5). Optionally hold Roblox's shared
//! login files (`%LOCALAPPDATA%\Roblox\LocalStorage\RobloxCookies.dat`, `appStorage.json`)
//! open with share mode 0 so clients can't overwrite each other's session (M6).

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::windows::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::{ERROR_INVALID_HANDLE, HANDLE};
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
    _files: Vec<File>,
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

    /// Takes the lock (no-op when held). If any RobloxPlayerBeta.exe runs (including Roblox's
    /// own tray process) or the name is already an event (`ERROR_INVALID_HANDLE`), nothing is
    /// held and the error says Roblox must be closed first. An existing *mutex* of that name
    /// (another tool) is fine: it blocks clients the same way. With `protect_shared_login`,
    /// also opens both shared files with share mode 0 (missing files are skipped).
    pub fn enable(&self, protect_shared_login: bool) -> Result<()> {
        if self.is_active() {
            return Ok(());
        }
        if !process::roblox_processes().is_empty() {
            return Err(AppError::Invalid(RUNNING.into()));
        }
        let files = if protect_shared_login { shared_files() } else { Vec::new() };
        self.take(SINGLETON_NAME, &files)
    }

    /// Releases the mutex and the files.
    pub fn disable(&self) {
        *self.held() = None;
    }

    pub fn is_active(&self) -> bool {
        self.held().is_some()
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

    /// Creates (or opens) the mutex `name` and locks `files`; no-op when already held.
    fn take(&self, name: &str, files: &[PathBuf]) -> Result<()> {
        let mut held = self.held();
        if held.is_none() {
            *held = Some(Held { _mutex: create_mutex(name)?, _files: lock_files(files) });
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

/// Opens each file with share mode 0 so nothing else can open it until it is dropped. Missing
/// files are skipped; a busy one is logged and skipped (the mutex alone still works).
fn lock_files(paths: &[PathBuf]) -> Vec<File> {
    paths
        .iter()
        .filter_map(|p| match OpenOptions::new().read(true).share_mode(0).open(p) {
            Ok(f) => Some(f),
            Err(e) if e.kind() == ErrorKind::NotFound => None,
            Err(e) => {
                log::warn!("could not lock {}: {e}", p.display());
                None
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
        a.take(&name, &[]).unwrap();
        assert!(a.is_active());
        let s = a.status(true, true);
        assert!(s.active && s.blocked_by.is_empty() && s.message.is_none() && s.protect_shared_login);
        // A client can't create its event while the mutex holds the name.
        assert_eq!(event(&name).unwrap_err().code(), ERROR_INVALID_HANDLE.to_hresult());
        // Creating the same mutex again (ERROR_ALREADY_EXISTS) still counts as held.
        b.take(&name, &[]).unwrap();
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
        let err = m.take(&name, &[]).unwrap_err();
        assert!(matches!(err, AppError::Invalid(ref s) if s == RUNNING), "{err}");
        assert!(!m.is_active());
    }

    #[test]
    fn locks_shared_files_until_disabled() {
        let dir = std::env::temp_dir().join(format!("roalt-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("RobloxCookies.dat");
        std::fs::write(&file, b"{}").unwrap();
        let files = [file.clone(), dir.join("appStorage.json")]; // the second is missing: skipped

        let m = Multi::new();
        m.take(&test_name(), &files).unwrap();
        let err = File::open(&file).unwrap_err();
        assert_eq!(err.raw_os_error(), Some(ERROR_SHARING_VIOLATION.0 as i32));
        // A busy file is skipped, not an error.
        let other = Multi::new();
        other.take(&test_name(), &files).unwrap();
        assert!(other.is_active());

        m.disable();
        File::open(&file).unwrap();
        other.disable();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
