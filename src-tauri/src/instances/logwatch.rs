//! Reading Roblox client logs (`%LOCALAPPDATA%\Roblox\logs\*_Player_*_last.log`) to tie a
//! client to its launch and follow it. Line formats: research multi-instance M10/M11 and
//! fixtures/multiinstance-log-line-samples.json. Logs can contain the launch ticket
//! (`gameinfo:`); lines are parsed and dropped, never stored or logged.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    /// `EventWaitResultUpdate` seen: another client set the singleton event. A close request
    /// after it is a replacement; before it, the user's own tray Exit (verify M11).
    signalled: bool,
}

/// Mtime slack for `find_log` (clocks and file-time granularity).
const SINCE_SLACK: Duration = Duration::from_secs(5);
/// The tracker id is logged within the first second; this is far more than that.
const HEAD_BYTES: u64 = 256 * 1024;

const JOINING: &str = "! Joining game '";
const DISCONNECTS: [&str; 6] = [
    "Disconnection Notification",
    "Lost connection with reason",
    "Time to disconnect replication data",
    "Sending disconnect with reason",
    "Disconnected from server for reason",
    "Connection closed by server",
];

/// `%LOCALAPPDATA%\Roblox\logs`.
pub fn logs_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Roblox").join("logs"))
}

/// The `_Player_` log (not `_CrashHandler_`) modified since `since` whose first lines name
/// this browser tracker id. Reads at most the first ~256 KB of each candidate.
pub fn find_log(dir: &Path, browser_tracker_id: i64, since: SystemTime) -> Option<PathBuf> {
    let since = since.checked_sub(SINCE_SLACK).unwrap_or(UNIX_EPOCH);
    let mut candidates: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.contains("_Player_") && !name.contains("_CrashHandler_")
        })
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            (modified >= since).then(|| (modified, e.path()))
        })
        .collect();
    candidates.sort_by_key(|c| std::cmp::Reverse(c.0));
    candidates.into_iter().map(|(_, p)| p).find(|p| names_tracker(p, browser_tracker_id))
}

fn names_tracker(path: &Path, browser_tracker_id: i64) -> bool {
    let mut head = Vec::new();
    if open_shared(path).and_then(|f| f.take(HEAD_BYTES).read_to_end(&mut head)).is_err() {
        return false;
    }
    String::from_utf8_lossy(&head).lines().any(|l| tracker_id(l) == Some(browser_tracker_id))
}

/// Reads lines appended since `offset` (opened shared, so Roblox keeps writing), updates
/// `facts`, returns the new offset. A partial last line is left for the next read.
pub fn read_new(path: &Path, offset: u64, facts: &mut LogFacts) -> std::io::Result<u64> {
    let mut f = open_shared(path)?;
    // Shorter than what was read: a new file under the same name, start over.
    let offset = if f.metadata()?.len() < offset { 0 } else { offset };
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let Some(end) = buf.iter().rposition(|&b| b == b'\n') else { return Ok(offset) };
    for line in buf[..end].split(|&b| b == b'\n') {
        parse_line(&String::from_utf8_lossy(line), facts);
    }
    Ok(offset + end as u64 + 1)
}

/// One log line into `facts` (pure).
pub fn parse_line(line: &str, facts: &mut LogFacts) {
    if let Some(id) = tracker_id(line) {
        facts.browser_tracker_id = Some(id);
    } else if let Some(i) = line.find(JOINING) {
        if let Some((job, rest)) = line[i + JOINING.len()..].split_once('\'')
            && is_guid(job)
        {
            facts.job_id = Some(job.to_owned());
            facts.place_id = number_after(rest, " place ");
            facts.in_game = false;
            facts.disconnected = false;
        }
    } else if line.contains("[FLog::Network] Replicator created") || line.contains("[FLog::Network] serverId:") {
        facts.in_game = true;
    } else if DISCONNECTS.iter().any(|m| line.contains(m)) {
        facts.disconnected = true;
    } else if line.contains("challengedByGcs") {
        facts.challenged = true;
    } else if line.contains("[FLog::GameJoinLoadTime] Report game_join_loadtime:") {
        if let Some(id) = number_after(line, "userid:") {
            facts.user_id = Some(id);
        }
    } else if line.contains("EventWaitResultUpdate") {
        facts.signalled = true;
    } else if line.contains("Received close main window request") && facts.signalled {
        facts.replaced = true;
    }
}

fn tracker_id(line: &str) -> Option<i64> {
    number_after(line, "websiteBTId is ").or_else(|| number_after(line, "BTID is overriden to "))
}

/// The digits right after `marker`.
fn number_after(line: &str, marker: &str) -> Option<i64> {
    let rest = &line[line.find(marker)? + marker.len()..];
    let end = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn is_guid(s: &str) -> bool {
    s.len() == 36 && s.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-')
}

fn open_shared(path: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    // FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE: Roblox keeps writing (and may
    // delete) the file while it is open here.
    std::fs::OpenOptions::new().read(true).share_mode(0x1 | 0x2 | 0x4).open(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;

    const BTID: i64 = 7_351_204_118;
    const JOB: &str = "0d6c3f4e-5b2a-4c1d-9e8f-7a6b5c4d3e2f";
    const PLACE: i64 = 920_587_237;
    const USER: i64 = 261;

    /// The fixture's marker lines with their placeholders filled in.
    fn samples() -> HashMap<String, String> {
        let path = format!("{}/../docs/research/fixtures/multiinstance-log-line-samples.json", env!("CARGO_MANIFEST_DIR"));
        let json: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        json["markers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(k, v)| {
                let line = v["line"]
                    .as_str()
                    .unwrap()
                    .replace("<BTID>", &BTID.to_string())
                    .replace("<JOBID>", JOB)
                    .replace("<PLACEID>", &PLACE.to_string())
                    .replace("<USERID>", &USER.to_string())
                    .replace("<UNIVERSEID>", "1069")
                    .replace("<SID>", "a1b2c3d4-0000-4000-8000-000000000000")
                    .replace("<T>", "1760864884.514");
                (k.clone(), line)
            })
            .collect()
    }

    fn parsed(lines: &[&str]) -> LogFacts {
        let mut f = LogFacts::default();
        for l in lines {
            parse_line(l, &mut f);
        }
        f
    }

    fn line(msg: &str) -> String {
        format!("2026-10-19T09:08:03.437Z,0.437181,0a50,6,Info {msg}\n")
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("roalt-logwatch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn every_sample_line() {
        let samples = samples();
        assert!(samples.len() >= 20);
        for (key, l) in &samples {
            let mut want = LogFacts::default();
            match key.as_str() {
                "web_launch_btid" => want.browser_tracker_id = Some(BTID),
                "joining_game" => {
                    want.job_id = Some(JOB.into());
                    want.place_id = Some(PLACE);
                }
                "join_loadtime" => want.user_id = Some(USER),
                "replicator_created" => want.in_game = true,
                "time_to_disconnect" | "client_disconnect_reason" | "lost_connection" | "server_closed_reason"
                | "disconnection_notification" => want.disconnected = true,
                "singleton_event_signalled" => want.signalled = true,
                // Alone, a close request is the user's tray Exit.
                "close_main_window_request" | "update_required" | "tray_treatment" | "singleton_mutex_wait"
                | "singleton_mutex_loop" | "update_mode" | "udmux" | "client_disconnect" | "shutdown" | "leave_ugc"
                | "tray_user_exit" | "tray_spawned" => {}
                other => panic!("new fixture sample {other}: say here what it means"),
            }
            assert_eq!(parsed(&[l]), want, "{key}");
        }
    }

    #[test]
    fn other_documented_lines() {
        let f = parsed(&[&line(&format!("[FLog::AppRemoteCheckStarterModule] BTID is overriden to {BTID}."))]);
        assert_eq!(f.browser_tracker_id, Some(BTID));
        assert!(parsed(&[&line("[FLog::Network] Sending disconnect with reason: 267")]).disconnected);
        assert!(parsed(&[&line("[FLog::Network] serverId: 10.9.2.90|51492")]).in_game);
        assert!(parsed(&[&line("[FLog::Something] reason: challengedByGcs")]).challenged);
        // Not a GUID: ignored.
        assert_eq!(parsed(&[&line("[FLog::Output] ! Joining game 'nope' place 1 at 1.2.3.4")]), LogFacts::default());
    }

    #[test]
    fn a_new_join_resets_the_session() {
        let s = samples();
        let mut f = parsed(&[&s["joining_game"], &s["replicator_created"], &s["lost_connection"]]);
        assert!(f.in_game && f.disconnected);
        parse_line(&s["joining_game"], &mut f);
        assert!(!f.in_game && !f.disconnected);
        assert_eq!(f.job_id.as_deref(), Some(JOB));
    }

    #[test]
    fn replacement_vs_tray_exit() {
        let s = samples();
        let (event, close) = (s["singleton_event_signalled"].as_str(), s["close_main_window_request"].as_str());
        assert!(parsed(&[event, close]).replaced);
        let tray_exit = line("[FLog::Systray] Systray menu item Exit is clicked");
        assert!(!parsed(&[&tray_exit, close, event]).replaced);
        assert!(!parsed(&[close]).replaced);
        assert!(!parsed(&[event]).replaced);
    }

    #[test]
    fn finds_the_log_by_tracker_id() {
        let dir = temp_dir();
        let now = SystemTime::now();
        let write = |name: &str, btid: i64, age: Duration| {
            let p = dir.join(name);
            let body = format!(
                "{}{}",
                line("[FLog::UpdateController] Update mode is chosen as BACKGROUND. Telemetry sent"),
                line(&format!("[FLog::LogWin32BTId] LogWin32BTId, App, web launch, websiteBTId is {btid}")),
            );
            std::fs::write(&p, body).unwrap();
            File::options().write(true).open(&p).unwrap().set_modified(now - age).unwrap();
            p
        };
        let since = now - Duration::from_secs(60);
        write("0.743.0.7431134_20261023T220008Z_Player_0A5C5_CrashHandler_last.log", BTID, Duration::ZERO);
        write("0.743.0.7431134_20261023T210008Z_Player_11111_last.log", BTID, Duration::from_secs(3600));
        write("0.743.0.7431134_20261023T220009Z_Player_22222_last.log", BTID + 1, Duration::ZERO);
        write("RobloxPlayerInstaller_EA24F.log", BTID, Duration::ZERO);
        assert_eq!(find_log(&dir, BTID, since), None);

        let older = write("0.743.0.7431134_20261023T220010Z_Player_33333_last.log", BTID, Duration::from_secs(30));
        assert_eq!(find_log(&dir, BTID, since), Some(older));
        let newer = write("0.743.0.7431134_20261023T220011Z_Player_44444_last.log", BTID, Duration::from_secs(2));
        assert_eq!(find_log(&dir, BTID, since), Some(newer));
        // Modified a little before `since`: within the slack.
        assert!(find_log(&dir, BTID + 1, now + Duration::from_secs(3)).is_some());
        assert_eq!(find_log(&dir.join("missing"), BTID, since), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_appended_lines_only_when_complete() {
        let dir = temp_dir();
        let p = dir.join("0.743.0.7431134_20261023T220008Z_Player_8C675_last.log");
        let len = |p: &Path| std::fs::metadata(p).unwrap().len();
        std::fs::write(&p, line(&format!("[FLog::LogWin32BTId] LogWin32BTId, App, web launch, websiteBTId is {BTID}"))).unwrap();
        let mut f = LogFacts::default();
        let o1 = read_new(&p, 0, &mut f).unwrap();
        assert_eq!((o1, f.browser_tracker_id), (len(&p), Some(BTID)));

        let join = line(&format!("[FLog::Output] ! Joining game '{JOB}' place {PLACE} at 10.9.2.90"));
        let (head, tail) = join.split_at(70);
        let mut w = File::options().append(true).open(&p).unwrap();
        // A whole line with an invalid UTF-8 byte, then half of the next one.
        w.write_all(b"2026-10-19T09:08:04.000Z,1.0,0a50,6 \xff noise\n").unwrap();
        w.write_all(head.as_bytes()).unwrap();
        let o2 = read_new(&p, o1, &mut f).unwrap();
        assert_eq!(o2, len(&p) - head.len() as u64);
        assert_eq!(f.job_id, None);
        assert_eq!(read_new(&p, o2, &mut f).unwrap(), o2);

        w.write_all(tail.as_bytes()).unwrap();
        w.write_all(line("[FLog::Network] Replicator created: 000000007A26CF00").as_bytes()).unwrap();
        let o3 = read_new(&p, o2, &mut f).unwrap();
        assert_eq!(o3, len(&p));
        assert_eq!((f.job_id.as_deref(), f.place_id, f.in_game), (Some(JOB), Some(PLACE), true));
        drop(w);

        // Replaced by a shorter file: read from the start.
        std::fs::write(&p, line("[FLog::Network] Lost connection with reason : You have been kicked from the game")).unwrap();
        let o4 = read_new(&p, o3, &mut f).unwrap();
        assert_eq!(o4, len(&p));
        assert!(f.disconnected);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
