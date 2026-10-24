//! Reading Roblox client logs (`%LOCALAPPDATA%\Roblox\logs\*_Player_*_last.log`) to tie a
//! client to its launch and follow it. Line formats: research multi-instance M10/M11 and
//! fixtures/multiinstance-log-line-samples.json. Logs can contain the launch ticket
//! (`gameinfo:`); lines are parsed and dropped, never stored or logged.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// What a client's log has said so far.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LogFacts {
    /// From `web launch, websiteBTId is <id>` or `BTID is overriden to <id>`.
    pub browser_tracker_id: Option<i64>,
    /// From `! Joining game '<job>' place <place> at <ip>` (latest).
    pub job_id: Option<String>,
    pub place_id: Option<i64>,
    /// From `[FLog::GameJoinLoadTime] ... userid:<id>`.
    pub user_id: Option<i64>,
    /// `Replicator created` after the latest join.
    pub in_game: bool,
    /// Disconnect / lost connection / kick after the latest join.
    pub disconnected: bool,
    /// `challengedByGcs`.
    pub challenged: bool,
    /// Closed because another client started (EventWaitResultUpdate then a close request).
    pub replaced: bool,
}

/// `%LOCALAPPDATA%\Roblox\logs`.
pub fn logs_dir() -> Option<PathBuf> {
    todo!("logwatch: logs_dir")
}

/// The `_Player_` log (not `_CrashHandler_`) modified since `since` whose first lines name
/// this browser tracker id. Reads at most the first ~256 KB of each candidate.
pub fn find_log(dir: &Path, browser_tracker_id: i64, since: SystemTime) -> Option<PathBuf> {
    let _ = (dir, browser_tracker_id, since);
    todo!("logwatch: find_log")
}

/// Reads lines appended since `offset` (opened shared, so Roblox keeps writing), updates
/// `facts`, returns the new offset. A partial last line is left for the next read.
pub fn read_new(path: &Path, offset: u64, facts: &mut LogFacts) -> std::io::Result<u64> {
    let _ = (path, offset, facts);
    todo!("logwatch: read_new")
}

/// One log line into `facts` (pure).
pub fn parse_line(line: &str, facts: &mut LogFacts) {
    let _ = (line, facts);
    todo!("logwatch: parse_line")
}
