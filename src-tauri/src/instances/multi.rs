//! Multi-instance: hold a mutex named exactly `ROBLOX_singletonEvent` (no prefix, not owned)
//! before any client starts, so a new client can't create its singleton event and doesn't
//! close the running one (research multi-instance M1-M5). Optionally hold Roblox's shared
//! login files (`%LOCALAPPDATA%\Roblox\LocalStorage\RobloxCookies.dat`, `appStorage.json`)
//! open with share mode 0 so clients can't overwrite each other's session (M6).

use std::sync::Mutex;

use crate::error::Result;
use crate::model::MultiInstanceStatus;

pub const SINGLETON_NAME: &str = "ROBLOX_singletonEvent";

pub struct Multi {
    inner: Mutex<Option<Held>>,
}

/// The mutex handle and the locked files; dropping it releases everything.
struct Held {
    _todo: (),
}

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
        let _ = (protect_shared_login, &self.inner);
        todo!("multi: enable")
    }

    /// Releases the mutex and the files.
    pub fn disable(&self) {
        todo!("multi: disable")
    }

    pub fn is_active(&self) -> bool {
        todo!("multi: is_active")
    }

    /// For the UI; `blocked_by` lists running RobloxPlayerBeta.exe PIDs when not active.
    pub fn status(&self, enabled: bool, protect_shared_login: bool) -> MultiInstanceStatus {
        let _ = (enabled, protect_shared_login);
        todo!("multi: status")
    }
}

impl Default for Multi {
    fn default() -> Self {
        Self::new()
    }
}
