//! Finding and starting the Roblox client (Windows).

use std::path::PathBuf;

use crate::error::Result;
use crate::vault::Secret;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientExe {
    pub path: PathBuf,
    /// The protocol handler is a bootstrapper (Bloxstrap, Fishstrap, ...), not RobloxPlayerBeta.exe.
    pub bootstrapper: bool,
}

/// Roblox's own client: `HKCU\Software\ROBLOX Corporation\Environments\roblox-player` value
/// `clientExe`, if that file exists; else the `roblox-player` protocol handler
/// (`HKCU\Software\Classes\roblox-player\shell\open\command`, then HKCR), parsed out of its
/// command line; `bootstrapper` = its file name isn't RobloxPlayerBeta.exe.
/// Nothing found → `AppError::Invalid("Roblox isn't installed. Install it from roblox.com, then try again.")`.
pub fn find() -> Result<ClientExe> {
    todo!("client: find")
}

/// Starts the client with the URI as its only argument (raw: no quoting, it contains no
/// spaces), in its own folder, and closes the process handle at once (Roblox's anti-cheat has
/// flagged long-held handles). Returns the PID. The URI is never logged.
pub fn start(exe: &ClientExe, uri: &Secret) -> Result<u32> {
    let _ = (exe, uri);
    todo!("client: start")
}
