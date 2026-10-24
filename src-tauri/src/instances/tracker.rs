//! Following the clients RoALT starts: one entry per launch, polled once a second.

use std::sync::Mutex;

use tauri::AppHandle;

use crate::model::{Instance, LaunchInfo, LaunchTarget};

pub struct Tracker {
    inner: Mutex<Vec<Instance>>,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker { inner: Mutex::new(Vec::new()) }
    }

    /// Starts following a launch (state Starting).
    pub fn register(&self, info: &LaunchInfo, target: LaunchTarget) -> Instance {
        let _ = (info, target, &self.inner);
        todo!("tracker: register")
    }

    /// Running ones first, then ended ones (newest first, at most 30 kept).
    pub fn list(&self) -> Vec<Instance> {
        todo!("tracker: list")
    }

    pub fn get(&self, id: &str) -> Option<Instance> {
        let _ = id;
        todo!("tracker: get")
    }

    /// The user asked to close it: its exit is not unexpected (no auto-rejoin, note "Closed").
    pub fn mark_closing(&self, id: &str) {
        let _ = id;
        todo!("tracker: mark_closing")
    }

    /// Forgets ended entries.
    pub fn clear_ended(&self) {
        todo!("tracker: clear_ended")
    }

    /// Runs for the app's lifetime (started in setup). Once a second: process liveness by
    /// (pid, start time); binds each entry to its log (`logwatch::find_log` by browser tracker
    /// id) and reads new lines; maps facts to states (Joining / InGame / Disconnected; note
    /// "Signed in as another account" when the log's userid differs from the account's,
    /// "Roblox wants a human check", "Replaced by another Roblox client"); when a process ends
    /// before reaching the game within 120 s, looks for a new client whose log names the same
    /// browser tracker id (Roblox's updater relaunches with a new PID) and rebinds to it;
    /// otherwise marks it Exited (note "Closed" if mark_closing was called, else "Closed
    /// unexpectedly"). Emits `instances://changed` whenever an entry changes. The bootstrapper
    /// case (`via_bootstrapper`): the started PID is the bootstrapper; bind to the client by log.
    pub fn spawn(app: AppHandle) {
        let _ = app;
        todo!("tracker: spawn")
    }
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}
