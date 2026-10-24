//! Roblox client processes: listing, liveness, closing. Handles are short-lived and
//! limited-rights (Roblox's anti-cheat has flagged long-held full handles).

use std::time::Duration;

use crate::error::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Process start, seconds since the Unix epoch (pid + start time identify a process).
    pub start_time: u64,
}

/// Running `RobloxPlayerBeta.exe` processes.
pub fn roblox_processes() -> Vec<ProcInfo> {
    todo!("process: roblox_processes")
}

/// Start time of a running process, `None` if it is gone.
pub fn start_time(pid: u32) -> Option<u64> {
    let _ = pid;
    todo!("process: start_time")
}

/// Asks the client to close: WM_CLOSE to its top-level windows, again after ~400 ms (Roblox may
/// show a leave prompt), then TerminateProcess if it is still alive after `grace`. `force`
/// terminates at once. Already gone → Ok.
pub fn close(pid: u32, force: bool, grace: Duration) -> Result<()> {
    let _ = (pid, force, grace);
    todo!("process: close")
}
