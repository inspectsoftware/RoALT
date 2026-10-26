//! Following the clients RoALT starts: one entry per launch, polled once a second.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use tauri::{AppHandle, Manager};

use super::logwatch::{self, LogFacts};
use super::process::{self, ProcInfo};
use crate::model::{Instance, InstanceState, LaunchInfo, LaunchTarget};

/// A client that vanishes this soon after launch without reaching a game may come back under
/// a new PID (Roblox's updater relaunches it, research M8/M9), or, behind a bootstrapper, may
/// not have started yet.
const REBIND_MS: i64 = 120_000;
const MAX_ENDED: usize = 30;
/// How far a client's start may be from its log file's creation, in seconds.
const LOG_SKEW_SECS: u64 = 3;
/// The note of a client whose log signed in as a different user than its account.
pub const WRONG_ACCOUNT: &str = "Signed in as another account";
pub const CHALLENGED: &str = "Roblox wants a human check";
/// The note of a client that ended without RoALT closing it and with nothing else to blame.
const UNEXPECTED: &str = "Closed unexpectedly";
/// In a game this long, a client's auto-rejoins stop counting against `MAX_REJOINS`.
const REJOIN_RESET_MS: i64 = 5 * 60_000;

pub struct Tracker {
    inner: Mutex<Vec<Entry>>,
}

/// One launch: what the UI sees plus what following it needs.
#[derive(Clone)]
struct Entry {
    inst: Instance,
    btid: i64,
    /// Start time of `inst.pid` when first seen (0: already gone), so a reused PID is not it.
    start_time: Option<u64>,
    /// `inst.pid` is the client, not a bootstrapper standing in for it.
    adopted: bool,
    alive: bool,
    log: Option<PathBuf>,
    offset: u64,
    facts: LogFacts,
    /// The account's Roblox user id, looked up on the first poll.
    expected_user: Option<i64>,
    user_checked: bool,
    closing: bool,
    /// Unix ms the client first reached a server.
    in_game_at: Option<i64>,
    /// Unix ms the client's current stretch in a game began (None while not in one).
    in_game_since: Option<i64>,
    /// Unix ms the bootstrapper of a not yet adopted entry was first seen gone.
    gone_at: Option<i64>,
    /// Handed out by `take_rejoins` (or ended): not again until it is back in a game.
    rejoin_seen: bool,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker { inner: Mutex::new(Vec::new()) }
    }

    fn entries(&self) -> MutexGuard<'_, Vec<Entry>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Starts following a launch (state Starting).
    pub fn register(&self, info: &LaunchInfo, target: LaunchTarget) -> Instance {
        let inst = Instance {
            id: uuid::Uuid::new_v4().to_string(),
            account_id: info.account_id,
            pid: info.pid,
            state: InstanceState::Starting,
            target,
            place_id: info.place_id,
            job_id: None,
            started_at: info.started_at,
            ended_at: None,
            via_bootstrapper: info.via_bootstrapper,
            note: None,
            rejoins: 0,
        };
        self.entries().push(Entry {
            inst: inst.clone(),
            btid: info.browser_tracker_id,
            start_time: info.pid_start,
            adopted: !info.via_bootstrapper,
            alive: true,
            log: None,
            offset: 0,
            facts: LogFacts::default(),
            expected_user: None,
            user_checked: false,
            closing: false,
            in_game_at: None,
            in_game_since: None,
            gone_at: None,
            rejoin_seen: false,
        });
        inst
    }

    /// Running ones first, then ended ones (newest first, at most 30 kept).
    pub fn list(&self) -> Vec<Instance> {
        let mut v: Vec<Instance> = self.entries().iter().map(|e| e.inst.clone()).collect();
        // Running ones in launch order.
        v.sort_by_key(|i| match i.state {
            InstanceState::Exited => (1, -i.ended_at.unwrap_or(0)),
            _ => (0, i.started_at),
        });
        v
    }

    pub fn get(&self, id: &str) -> Option<Instance> {
        self.entries().iter().find(|e| e.inst.id == id).map(|e| e.inst.clone())
    }

    /// The user asked to close it: its exit is not unexpected (no auto-rejoin, note "Closed").
    /// Returns the process to close, under the same lock: its pid and the start time it was
    /// first seen with (None before that). None once it ended or its process is gone, so a
    /// reused pid is never closed; the next poll ends the entry.
    pub fn mark_closing(&self, id: &str) -> Option<(u32, Option<u64>)> {
        let mut entries = self.entries();
        let e = entries.iter_mut().find(|e| e.inst.id == id && e.inst.state != InstanceState::Exited)?;
        e.closing = true;
        e.alive.then_some((e.inst.pid, e.start_time))
    }

    /// A close that didn't land: its exit counts as unexpected again.
    pub fn clear_closing(&self, id: &str) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.closing = false;
        }
    }

    /// True while a client RoALT started hasn't ended.
    pub fn any_running(&self) -> bool {
        self.entries().iter().any(|e| e.inst.state != InstanceState::Exited)
    }

    /// How many auto-rejoins led to the client `id` (shown in the UI, capped by `take_rejoins`).
    pub fn set_rejoins(&self, id: &str, rejoins: u32) {
        if let Some(e) = self.entries().iter_mut().find(|e| e.inst.id == id) {
            e.inst.rejoins = rejoins;
        }
    }

    /// Still a rejoin case (`wants_rejoin`), checked again after the launch delay: false once
    /// the user closed it, it reconnected or it was cleared.
    pub fn still_wants_rejoin(&self, id: &str) -> bool {
        self.entries().iter().any(|e| e.inst.id == id && wants_rejoin(&e.inst, &e.facts))
    }

    /// Clients that got disconnected since the last call and may be rejoined (`wants_rejoin`),
    /// each returned once per disconnect (again after it got back into a game). Drained whether
    /// or not auto-rejoin is on, so turning it on later doesn't relaunch old disconnects.
    pub fn take_rejoins(&self) -> Vec<Rejoin> {
        let mut out = Vec::new();
        for e in self.entries().iter_mut() {
            match e.inst.state {
                InstanceState::Disconnected | InstanceState::Exited if !e.rejoin_seen => {
                    e.rejoin_seen = true;
                    if wants_rejoin(&e.inst, &e.facts) {
                        let i = &e.inst;
                        out.push(Rejoin { from: i.id.clone(), account_id: i.account_id, target: i.target.clone(), rejoins: i.rejoins + 1 });
                    }
                }
                InstanceState::Disconnected | InstanceState::Exited => {}
                // Reconnected (or teleported): a later disconnect counts again.
                _ => e.rejoin_seen = false,
            }
        }
        out
    }

    /// Forgets ended entries.
    pub fn clear_ended(&self) {
        self.entries().retain(|e| e.inst.state != InstanceState::Exited);
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
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let handle = app.clone();
                let (changed, rejoins) = tauri::async_runtime::spawn_blocking(move || {
                    let s = handle.state::<crate::App>();
                    let user_of = |account_id| s.db.account(account_id).ok().map(|a| a.user_id);
                    let changed = s.tracker.poll(logwatch::logs_dir().as_deref(), &Live, &user_of, crate::commands::now_ms());
                    // The shared login files are held only while RoALT's clients run (or launch).
                    s.multi.release_if(|| !s.tracker.any_running());
                    (changed, s.tracker.take_rejoins())
                })
                .await
                .unwrap_or_default();
                if changed {
                    crate::commands::instances_changed(&app);
                }
                super::rejoin::handle(&app, rejoins);
            }
        });
    }

    /// One round over the running entries; true when what the UI sees changed. The files and
    /// processes are looked at on copies, outside the lock.
    fn poll(&self, dir: Option<&Path>, procs: &dyn Procs, user_of: &dyn Fn(i64) -> Option<i64>, now: i64) -> bool {
        let mut work: Vec<Entry> = self.entries().iter().filter(|e| e.inst.state != InstanceState::Exited).cloned().collect();
        if work.is_empty() {
            return false;
        }
        let mut taken: Vec<u32> = work.iter().filter(|e| e.adopted).map(|e| e.inst.pid).collect();
        let mut roblox = None;
        for e in &mut work {
            if !e.user_checked {
                e.expected_user = user_of(e.inst.account_id);
                e.user_checked = true;
            }
            observe(e, dir, procs, &mut roblox, &mut taken, now);
        }
        let mut entries = self.entries();
        let mut changed = false;
        for w in work {
            // Only this loop ends entries, so a running one is still there.
            let Some(e) = entries.iter_mut().find(|e| e.inst.id == w.inst.id) else { continue };
            // A close that reached only the bootstrapper doesn't cover the client it started.
            let closing = e.closing && (e.adopted || !w.adopted);
            let before = std::mem::replace(e, w).inst;
            e.closing = closing;
            // set_rejoins may have run since the copy was taken.
            e.inst.rejoins = before.rejoins;
            settle(e, now);
            changed |= e.inst != before;
        }
        prune(&mut entries);
        changed
    }
}

/// At most this many auto-rejoins in a row for one client.
pub const MAX_REJOINS: u32 = 3;

/// A client to launch again: same account, same target.
#[derive(Clone, Debug, PartialEq)]
pub struct Rejoin {
    /// The instance that ended.
    pub from: String,
    pub account_id: i64,
    pub target: LaunchTarget,
    /// For the new client: the ended one's `rejoins` + 1.
    pub rejoins: u32,
}

/// A client is worth rejoining when Roblox disconnected it (kicked, server shut down, connection
/// lost; not the player's own Leave, research: reason 285) and it either still shows the
/// disconnect dialog (no problem note) or then closed by itself ("Closed unexpectedly"), with
/// fewer than `MAX_REJOINS` rejoins. A plain exit isn't one: closing Roblox's window leaves no
/// trace in its log, so it can't be told apart from a crash.
pub fn wants_rejoin(inst: &Instance, facts: &LogFacts) -> bool {
    let ok = match inst.state {
        InstanceState::Disconnected => inst.note.is_none(),
        InstanceState::Exited => inst.note.as_deref() == Some(UNEXPECTED),
        _ => false,
    };
    ok && facts.disconnected && !facts.left && !facts.replaced && inst.rejoins < MAX_REJOINS
}

impl Default for Tracker {
    fn default() -> Self {
        Self::new()
    }
}

/// The process queries a poll makes (a fixed list in tests).
trait Procs {
    fn start_time(&self, pid: u32) -> Option<u64>;
    fn roblox(&self) -> Vec<ProcInfo>;
}

struct Live;

impl Procs for Live {
    fn start_time(&self, pid: u32) -> Option<u64> {
        process::start_time(pid)
    }

    fn roblox(&self) -> Vec<ProcInfo> {
        process::roblox_processes()
    }
}

/// The blocking part of a poll for one entry: liveness, rebinding, log binding and reading.
/// `roblox` lists the Roblox processes at most once per poll; `taken` holds the PIDs entries follow.
fn observe(e: &mut Entry, dir: Option<&Path>, procs: &dyn Procs, roblox: &mut Option<Vec<ProcInfo>>, taken: &mut Vec<u32>, now: i64) {
    let seen = procs.start_time(e.inst.pid);
    let first = *e.start_time.get_or_insert(seen.unwrap_or(0));
    e.alive = seen == Some(first);
    if !e.adopted && !e.alive {
        e.gone_at.get_or_insert(now);
    }
    let Some(dir) = dir else { return };
    let launched = UNIX_EPOCH + Duration::from_millis(e.inst.started_at.max(0) as u64);
    // A client starts after the launch (whole seconds: one of slack) and after the process it
    // replaces or that started it.
    let after = first.max(u64::try_from(e.inst.started_at / 1000 - 1).unwrap_or(0));

    // Roblox's updater relaunched it: a log created after the dead client's own (which is
    // created within seconds of its start) names the same tracker id. Judged by creation time,
    // not by the bound path: the dead client may have died before its log was bound, or the
    // relaunch's log may have been bound to the dead PID.
    if !e.alive
        && e.adopted
        && may_appear(e, now)
        && let Some(path) = logwatch::find_log(dir, e.btid, launched)
        && let Some(created) = created_secs(&path).filter(|&c| c > after + LOG_SKEW_SECS)
        && let Some(p) = pick_client(roblox.get_or_insert_with(|| procs.roblox()), Some(created), after, taken)
    {
        adopt(e, p, taken);
        if e.log.as_ref() != Some(&path) {
            e.log = Some(path);
            e.offset = 0;
            e.facts = LogFacts::default();
        }
    }
    if e.log.is_none() {
        e.log = logwatch::find_log(dir, e.btid, launched);
    }
    // Behind a bootstrapper: the client is the one that wrote the log.
    if !e.adopted
        && let Some(path) = &e.log
        && let Some(p) = pick_client(roblox.get_or_insert_with(|| procs.roblox()), created_secs(path), after, taken)
    {
        adopt(e, p, taken);
    }
    if let Some(path) = &e.log {
        match logwatch::read_new(path, e.offset, &mut e.facts) {
            Ok(offset) => e.offset = offset,
            Err(err) => log::debug!("instance {}: client log not readable: {err}", e.inst.id),
        }
    }
}

fn adopt(e: &mut Entry, p: ProcInfo, taken: &mut Vec<u32>) {
    e.inst.pid = p.pid;
    e.start_time = Some(p.start_time);
    e.adopted = true;
    e.alive = true;
    taken.push(p.pid);
}

/// The client that wrote a log: started within a few seconds of the log's creation, else the
/// newest one started since `after`. PIDs already followed are skipped.
fn pick_client(procs: &[ProcInfo], log_created: Option<u64>, after: u64, taken: &[u32]) -> Option<ProcInfo> {
    let fresh = || procs.iter().filter(|p| p.start_time >= after && !taken.contains(&p.pid));
    log_created
        .and_then(|c| fresh().filter(|p| p.start_time.abs_diff(c) <= LOG_SKEW_SECS).min_by_key(|p| p.start_time.abs_diff(c)))
        .or_else(|| fresh().max_by_key(|p| p.start_time))
        .cloned()
}

fn created_secs(path: &Path) -> Option<u64> {
    let created = std::fs::metadata(path).ok()?.created().ok()?;
    Some(created.duration_since(UNIX_EPOCH).ok()?.as_secs())
}

/// Too early to call a vanished client gone: it never reached a game, was not replaced, was
/// launched under 120 s ago, and wasn't closed (a close that reached only a bootstrapper gets
/// 5 s for the client it may already have started to show up).
fn may_appear(e: &Entry, now: i64) -> bool {
    let closed = e.closing && (e.adopted || e.gone_at.is_some_and(|g| now - g >= BOOTSTRAPPER_GRACE_MS));
    e.in_game_at.is_none() && !closed && !e.facts.replaced && now - e.inst.started_at < REBIND_MS
}

const BOOTSTRAPPER_GRACE_MS: i64 = 5_000;

/// Recomputes what the UI sees (under the lock, with the current closing flag).
fn settle(e: &mut Entry, now: i64) {
    if e.facts.in_game && e.in_game_at.is_none() {
        e.in_game_at = Some(now);
    }
    let ended = !e.alive && !may_appear(e, now);
    let (state, note) = state_of(&e.facts, e.log.is_some(), ended, e.closing, e.expected_user);
    e.inst.state = state;
    e.inst.note = note;
    if e.inst.state == InstanceState::InGame {
        // Rejoins count "in a row": a client that stayed in a game a while starts over.
        if now - *e.in_game_since.get_or_insert(now) >= REJOIN_RESET_MS {
            e.inst.rejoins = 0;
        }
    } else {
        e.in_game_since = None;
    }
    if e.facts.place_id.is_some() {
        e.inst.place_id = e.facts.place_id;
    }
    if e.facts.job_id.is_some() {
        e.inst.job_id.clone_from(&e.facts.job_id);
    }
    if ended {
        e.inst.ended_at = Some(now);
    }
}

/// State and note from what the log said, whether the log is bound, whether the client is
/// gone for good, whether the user closed it, and the account's user id (pure).
fn state_of(facts: &LogFacts, bound: bool, ended: bool, closing: bool, expected_user: Option<i64>) -> (InstanceState, Option<String>) {
    let problem = if facts.replaced {
        Some("Replaced by another Roblox client")
    } else if matches!((facts.user_id, expected_user), (Some(got), Some(want)) if got != want) {
        Some(WRONG_ACCOUNT)
    } else if facts.challenged {
        Some(CHALLENGED)
    } else {
        None
    };
    if ended {
        let note = if closing { "Closed" } else { problem.unwrap_or(UNEXPECTED) };
        return (InstanceState::Exited, Some(note.into()));
    }
    let state = if facts.disconnected {
        InstanceState::Disconnected
    } else if facts.in_game {
        InstanceState::InGame
    } else if bound {
        InstanceState::Joining
    } else {
        InstanceState::Starting
    };
    (state, problem.map(Into::into))
}

/// Keeps the newest `MAX_ENDED` ended entries.
fn prune(entries: &mut Vec<Entry>) {
    loop {
        let ended = entries.iter().enumerate().filter(|(_, e)| e.inst.state == InstanceState::Exited);
        if ended.clone().count() <= MAX_ENDED {
            return;
        }
        let Some((i, _)) = ended.min_by_key(|(_, e)| e.inst.ended_at) else { return };
        entries.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::InstanceState::*;

    const BTID: i64 = 5_550_001;
    const USER: i64 = 261;

    /// Fake processes: what `roblox()` lists and `start_time` finds (bootstrappers included).
    struct Fake(Mutex<Vec<ProcInfo>>);

    impl Fake {
        fn new(procs: &[(u32, u64)]) -> Fake {
            let f = Fake(Mutex::new(Vec::new()));
            f.set(procs);
            f
        }

        fn set(&self, procs: &[(u32, u64)]) {
            *self.0.lock().unwrap() = procs.iter().map(|&(pid, start_time)| ProcInfo { pid, start_time }).collect();
        }
    }

    impl Procs for Fake {
        fn start_time(&self, pid: u32) -> Option<u64> {
            self.0.lock().unwrap().iter().find(|p| p.pid == pid).map(|p| p.start_time)
        }

        fn roblox(&self) -> Vec<ProcInfo> {
            self.0.lock().unwrap().clone()
        }
    }

    fn now_ms() -> i64 {
        crate::commands::now_ms()
    }

    fn info(pid: u32, started_at: i64, via_bootstrapper: bool) -> LaunchInfo {
        LaunchInfo { account_id: 1, pid, browser_tracker_id: BTID, place_id: Some(1818), started_at, via_bootstrapper, warning: None, pid_start: None }
    }

    fn target() -> LaunchTarget {
        LaunchTarget::Place { place_id: 1818, launch_data: None }
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("roalt-tracker-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn append(path: &Path, msgs: &[String]) {
        use std::io::Write;
        let mut f = std::fs::File::options().create(true).append(true).open(path).unwrap();
        for m in msgs {
            writeln!(f, "2026-10-19T09:08:03.437Z,0.437181,0a50,6,Info {m}").unwrap();
        }
    }

    fn btid_line(btid: i64) -> String {
        format!("[FLog::LogWin32BTId] LogWin32BTId, App, web launch, websiteBTId is {btid}")
    }

    fn join_lines(user: i64) -> Vec<String> {
        vec![
            "[FLog::Output] ! Joining game '0d6c3f4e-5b2a-4c1d-9e8f-7a6b5c4d3e2f' place 1818 at 10.9.2.90".into(),
            format!("[FLog::GameJoinLoadTime] Report game_join_loadtime: placeid:1818, join_time:0.68, userid:{user},"),
        ]
    }

    fn replicator() -> String {
        "[FLog::Network] Replicator created: 000000007A26CF00".into()
    }

    fn user_of(_: i64) -> Option<i64> {
        Some(USER)
    }

    fn state(t: &Tracker, id: &str) -> (InstanceState, Option<String>, u32) {
        let i = t.get(id).unwrap();
        (i.state, i.note, i.pid)
    }

    fn note(s: &str) -> Option<String> {
        Some(s.into())
    }

    #[test]
    fn states_from_facts() {
        let f = |set: fn(&mut LogFacts)| {
            let mut f = LogFacts::default();
            set(&mut f);
            f
        };
        let none = LogFacts::default();
        let in_game = f(|f| f.in_game = true);
        assert_eq!(state_of(&none, false, false, false, Some(USER)), (Starting, None));
        assert_eq!(state_of(&none, true, false, false, Some(USER)), (Joining, None));
        assert_eq!(state_of(&in_game, true, false, false, Some(USER)), (InGame, None));
        let dropped = f(|f| {
            f.in_game = true;
            f.disconnected = true;
        });
        assert_eq!(state_of(&dropped, true, false, false, Some(USER)), (Disconnected, None));

        let other = f(|f| f.user_id = Some(USER + 1));
        assert_eq!(state_of(&other, true, false, false, Some(USER)), (Joining, note("Signed in as another account")));
        assert_eq!(state_of(&other, true, false, false, None), (Joining, None));
        assert_eq!(state_of(&f(|f| f.user_id = Some(USER)), true, false, false, Some(USER)), (Joining, None));
        let challenged = f(|f| f.challenged = true);
        assert_eq!(state_of(&challenged, true, false, false, Some(USER)), (Joining, note("Roblox wants a human check")));

        let replaced = f(|f| f.replaced = true);
        assert_eq!(state_of(&replaced, true, true, false, Some(USER)), (Exited, note("Replaced by another Roblox client")));
        assert_eq!(state_of(&replaced, true, true, true, Some(USER)), (Exited, note("Closed")));
        assert_eq!(state_of(&in_game, true, true, false, Some(USER)), (Exited, note("Closed unexpectedly")));
        assert_eq!(state_of(&in_game, true, true, true, Some(USER)), (Exited, note("Closed")));
        assert_eq!(state_of(&other, true, true, false, Some(USER)), (Exited, note("Signed in as another account")));
        assert_eq!(state_of(&challenged, false, true, false, Some(USER)), (Exited, note("Roblox wants a human check")));
    }

    #[test]
    fn lists_running_first_and_keeps_30_ended() {
        let t = Tracker::new();
        let ids: Vec<String> = (0..40).map(|n| t.register(&info(100 + n, 1_000 + n as i64, false), target()).id).collect();
        {
            let mut v = t.entries();
            // 35 end (the first one last), 5 keep running.
            for (n, e) in v.iter_mut().enumerate().take(35) {
                e.inst.state = Exited;
                e.inst.ended_at = Some(if n == 0 { 99_999 } else { 5_000 + n as i64 });
            }
            prune(&mut v);
        }
        let list = t.list();
        assert_eq!(list.len(), 35);
        let running: Vec<&str> = list[..5].iter().map(|i| i.id.as_str()).collect();
        assert_eq!(running, ids[35..].iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(list[5].id, ids[0]);
        assert_eq!(list[6].id, ids[34]);
        assert_eq!(list.last().unwrap().id, ids[6]);

        t.mark_closing(&ids[0]);
        assert!(!t.entries().iter().any(|e| e.closing));
        t.clear_ended();
        assert_eq!(t.list().len(), 5);
    }

    #[test]
    fn follows_a_client_from_start_to_exit() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);

        assert!(!poll(start + 1_000));
        assert_eq!(state(&t, &id), (Starting, None, 100));

        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_CrashHandler_last.log"), &[btid_line(BTID)]);
        append(&log, &[btid_line(BTID)]);
        assert!(poll(start + 2_000));
        assert_eq!(state(&t, &id), (Joining, None, 100));
        assert!(!poll(start + 3_000));

        append(&log, &join_lines(USER));
        append(&log, &[replicator()]);
        assert!(poll(start + 4_000));
        let i = t.get(&id).unwrap();
        assert_eq!((i.state, i.job_id.as_deref(), i.place_id), (InGame, Some("0d6c3f4e-5b2a-4c1d-9e8f-7a6b5c4d3e2f"), Some(1818)));

        append(&log, &["[FLog::Network] Disconnection Notification. Reason: 267".into()]);
        assert!(poll(start + 5_000));
        assert_eq!(state(&t, &id).0, Disconnected);

        // Reached the game, so a vanished process is gone at once.
        procs.set(&[]);
        assert!(poll(start + 6_000));
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
        assert_eq!(t.get(&id).unwrap().ended_at, Some(start + 6_000));
        assert!(!poll(start + 7_000));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notes_the_wrong_account_and_a_requested_close() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        append(&log, &join_lines(USER + 1));
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id), (Joining, note("Signed in as another account"), 100));

        t.mark_closing(&id);
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_reused_pid_is_not_the_client() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, 10)]);
        t.poll(None, &procs, &user_of, start + 1_000);
        procs.set(&[(100, 20)]);
        t.poll(None, &procs, &user_of, start + 121_000);
        assert_eq!(state(&t, &id).0, Exited);
    }

    /// A log written by a client that started at `secs`: created then, modified a bit ago.
    fn old_log(path: &Path, secs: u64) {
        use std::os::windows::fs::FileTimesExt;
        let times = std::fs::FileTimes::new()
            .set_created(UNIX_EPOCH + Duration::from_secs(secs))
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(2));
        std::fs::File::options().write(true).open(path).unwrap().set_times(times).unwrap();
    }

    #[test]
    fn rebinds_after_an_updater_relaunch() {
        let dir = temp_dir();
        let t = Tracker::new();
        // Launched 10 s ago; the updater ran in between.
        let start = now_ms() - 10_000;
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let other = t.register(&LaunchInfo { browser_tracker_id: BTID + 1, ..info(300, start, false) }, target()).id;
        let procs = Fake::new(&[(100, secs), (300, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);

        let old = dir.join("0.743.0.7431134_20261024T120000Z_Player_AAAAA_last.log");
        append(&old, &[btid_line(BTID), "[FLog::UpdateController] Update check thread: updateRequired TRUE".into()]);
        old_log(&old, secs);
        poll(start + 1_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));

        // The client quits for the update: not gone yet, nothing new to bind.
        procs.set(&[(300, secs)]);
        poll(start + 2_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));

        // The relaunched client: new PID, new log naming the same tracker id. The other
        // entry's client is never picked.
        let new = dir.join("0.744.0.7441080_20261024T120010Z_Player_BBBBB_last.log");
        append(&new, &[btid_line(BTID)]);
        append(&new, &join_lines(USER));
        procs.set(&[(300, secs), (200, secs + 10)]);
        poll(start + 12_000);
        assert_eq!(state(&t, &id), (Joining, None, 200));
        append(&new, &[replicator()]);
        poll(start + 13_000);
        assert_eq!(state(&t, &id), (InGame, None, 200));
        assert_eq!(state(&t, &other), (Starting, None, 300));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dead_clients_own_log_is_not_a_relaunch() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms() - 10_000;
        let secs = (start / 1000) as u64;
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, secs)]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);

        // It quits before its log is bound; a client the user started at about the same time
        // keeps running. Still waiting for a relaunch, not adopting that one.
        let own = dir.join("0.743.0.7431134_20261024T120000Z_Player_AAAAA_last.log");
        append(&own, &[btid_line(BTID)]);
        old_log(&own, secs);
        procs.set(&[(400, secs + 1)]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Joining, None, 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_client_gone_before_the_game_ends_after_120_s() {
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[]);
        t.poll(None, &procs, &user_of, start + 1_000);
        assert_eq!(state(&t, &id).0, Starting);
        t.poll(None, &procs, &user_of, start + 119_000);
        assert_eq!(state(&t, &id).0, Starting);
        t.poll(None, &procs, &user_of, start + 120_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 100));
    }

    #[test]
    fn adopts_the_client_behind_a_bootstrapper() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(900, start, true), target()).id;
        // An older Roblox (the user's own) is never the one.
        let procs = Fake::new(&[(900, secs), (50, secs - 600)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));

        // The bootstrapper exits after starting the client; its log appears.
        procs.set(&[(50, secs - 600), (901, secs + 1)]);
        poll(start + 3_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));
        append(&dir.join("0.744.0.7441080_20261024T120002Z_Player_CCCCC_last.log"), &[btid_line(BTID)]);
        poll(start + 4_000);
        assert_eq!(state(&t, &id), (Joining, None, 901));
        procs.set(&[(50, secs - 600)]);
        poll(start + 5_000);
        assert_eq!(state(&t, &id).0, Joining);
        assert!(t.get(&id).unwrap().via_bootstrapper);
        poll(start + 121_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed unexpectedly"), 901));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn closing_a_bootstrapper_leaves_its_client_running() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let id = t.register(&info(900, start, true), target()).id;
        let procs = Fake::new(&[(900, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(t.mark_closing(&id), Some((900, Some(secs))));

        // The close reached only the bootstrapper; its client shows up within the grace.
        procs.set(&[(901, secs + 1)]);
        poll(start + 3_000);
        assert_eq!(state(&t, &id), (Starting, None, 900));
        append(&dir.join("0.744.0.7441080_20261024T120002Z_Player_DDDDD_last.log"), &[btid_line(BTID)]);
        poll(start + 4_000);
        assert_eq!(state(&t, &id), (Joining, None, 901));
        // Closing applies to the client again, which then ends as closed.
        assert_eq!(t.mark_closing(&id), Some((901, Some(secs + 1))));
        procs.set(&[]);
        poll(start + 5_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 901));

        // No client within the grace (and no log naming it): closed.
        let empty = temp_dir();
        let poll = |now: i64| t.poll(Some(&empty), &procs, &user_of, now);
        let id = t.register(&info(910, start, true), target()).id;
        procs.set(&[(910, secs)]);
        poll(start + 1_000);
        t.mark_closing(&id);
        procs.set(&[]);
        poll(start + 2_000);
        assert_eq!(state(&t, &id).0, Starting);
        poll(start + 8_000);
        assert_eq!(state(&t, &id), (Exited, note("Closed"), 910));
        assert_eq!(t.mark_closing(&id), None);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn rejoins_only_disconnects_roblox_caused_up_to_three_times() {
        let base = Tracker::new().register(&info(100, 0, false), target());
        let inst = |state, note: Option<&str>, rejoins| Instance { state, note: note.map(Into::into), rejoins, ..base.clone() };
        let facts = |disconnected, left| {
            let mut f = LogFacts::default();
            (f.disconnected, f.left) = (disconnected, left);
            f
        };
        let kicked = facts(true, false);
        assert!(wants_rejoin(&inst(Disconnected, None, 0), &kicked));
        assert!(wants_rejoin(&inst(Disconnected, None, 2), &kicked));
        assert!(wants_rejoin(&inst(Exited, Some(UNEXPECTED), 0), &kicked), "closed by itself after the kick");
        assert!(!wants_rejoin(&inst(Disconnected, None, MAX_REJOINS), &kicked));
        assert!(!wants_rejoin(&inst(Disconnected, None, 0), &facts(true, true)), "the player left");
        assert!(!wants_rejoin(&inst(Exited, Some(UNEXPECTED), 0), &facts(false, false)), "a plain exit");
        for n in ["Closed", WRONG_ACCOUNT, CHALLENGED] {
            assert!(!wants_rejoin(&inst(Exited, Some(n), 0), &kicked), "{n}");
        }
        assert!(!wants_rejoin(&inst(Disconnected, Some(CHALLENGED), 0), &kicked));
        for state in [Starting, Joining, InGame] {
            assert!(!wants_rejoin(&inst(state, None, 0), &kicked), "{state:?}");
        }
    }

    #[test]
    fn hands_out_each_disconnect_once() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let launch = |pid: u32, account_id: i64| LaunchInfo { account_id, browser_tracker_id: BTID + i64::from(pid), ..info(pid, start, false) };
        let follow = LaunchTarget::FollowUser { user_id: 42 };
        let kicked = t.register(&launch(100, 7), follow.clone()).id;
        let left = t.register(&launch(200, 1), target()).id;
        let closed = t.register(&launch(300, 1), target()).id;
        t.set_rejoins(&kicked, 1);
        let log = |pid: u32| dir.join(format!("0.744.0.7441080_20261024T1200{pid}Z_Player_R{pid}_last.log"));
        for pid in [100, 200, 300] {
            append(&log(pid), &[btid_line(BTID + i64::from(pid))]);
            append(&log(pid), &join_lines(USER));
            append(&log(pid), &[replicator()]);
        }
        let procs = Fake::new(&[(100, secs), (200, secs), (300, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        assert_eq!(state(&t, &kicked).0, InGame);
        assert!(t.take_rejoins().is_empty());

        // Kicked (idle, 267) and left by the player (285): only the kick is handed out, once.
        append(&log(100), &["[FLog::Network] Sending disconnect with reason: 267".into()]);
        append(&log(200), &["[FLog::Network] Disconnection Notification. Reason: 285".into()]);
        // A plain exit (the window closed) is no disconnect.
        procs.set(&[(100, secs), (200, secs)]);
        poll(start + 2_000);
        assert_eq!(state(&t, &kicked).0, Disconnected);
        assert_eq!(state(&t, &closed), (Exited, note("Closed unexpectedly"), 300));
        assert_eq!(t.take_rejoins(), vec![Rejoin { from: kicked.clone(), account_id: 7, target: follow.clone(), rejoins: 2 }]);
        assert!(t.take_rejoins().is_empty());
        poll(start + 3_000);
        assert!(t.take_rejoins().is_empty());

        // Reconnected, then kicked again: handed out again.
        append(&log(100), &join_lines(USER));
        append(&log(100), &[replicator()]);
        poll(start + 4_000);
        assert!(t.take_rejoins().is_empty());
        append(&log(100), &["[FLog::Network] Lost connection with reason : You have been kicked from the game".into()]);
        poll(start + 5_000);
        assert_eq!(t.take_rejoins().len(), 1);
        assert_eq!(state(&t, &left).0, Disconnected);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejoins_count_in_a_row_and_a_closed_client_is_not_rejoined() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let secs = (start / 1000) as u64;
        let launch = |pid: u32| LaunchInfo { browser_tracker_id: BTID + i64::from(pid), ..info(pid, start, false) };
        let long = t.register(&launch(100), target()).id;
        let quick = t.register(&launch(200), target()).id;
        let log = |pid: u32| dir.join(format!("0.744.0.7441080_20261024T1200{pid}Z_Player_R{pid}_last.log"));
        for (pid, id) in [(100, &long), (200, &quick)] {
            t.set_rejoins(id, MAX_REJOINS);
            append(&log(pid), &[btid_line(BTID + i64::from(pid))]);
            append(&log(pid), &join_lines(USER));
            append(&log(pid), &[replicator()]);
        }
        let procs = Fake::new(&[(100, secs), (200, secs)]);
        let poll = |now: i64| t.poll(Some(&dir), &procs, &user_of, now);
        poll(start + 1_000);
        poll(start + 60_000);
        let kick = |pid| append(&log(pid), &["[FLog::Network] Sending disconnect with reason: 267".into()]);

        // Kicked after a minute in game: still at the cap.
        kick(200);
        poll(start + 61_000);
        assert_eq!(state(&t, &quick).0, Disconnected);
        assert!(t.take_rejoins().is_empty());

        // Kicked after five minutes in game: the count started over.
        poll(start + 1_000 + REJOIN_RESET_MS);
        assert_eq!(t.get(&long).unwrap().rejoins, 0);
        kick(100);
        poll(start + 2_000 + REJOIN_RESET_MS);
        assert_eq!(t.take_rejoins(), vec![Rejoin { from: long.clone(), account_id: 1, target: target(), rejoins: 1 }]);
        assert!(t.still_wants_rejoin(&long));

        // Closed by the user during the launch delay: no longer wanted; cleared: neither.
        t.mark_closing(&long);
        procs.set(&[(200, secs)]);
        poll(start + 3_000 + REJOIN_RESET_MS);
        assert_eq!(state(&t, &long).1, note("Closed"));
        assert!(!t.still_wants_rejoin(&long));
        t.clear_ended();
        assert!(!t.still_wants_rejoin(&long));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_replaced_client_ends_at_once() {
        let dir = temp_dir();
        let t = Tracker::new();
        let start = now_ms();
        let id = t.register(&info(100, start, false), target()).id;
        let procs = Fake::new(&[(100, (start / 1000) as u64)]);
        let log = dir.join("0.744.0.7441080_20261024T120000Z_Player_ABCDE_last.log");
        append(&log, &[btid_line(BTID)]);
        t.poll(Some(&dir), &procs, &user_of, start + 1_000);
        append(
            &log,
            &[
                "[FLog::WndProcessCheck] WindowsAppReloadTelemetry EventWaitResultUpdate".into(),
                "[FLog::Systray] Main thread: Received close main window request".into(),
            ],
        );
        procs.set(&[]);
        t.poll(Some(&dir), &procs, &user_of, start + 2_000);
        assert_eq!(state(&t, &id), (Exited, note("Replaced by another Roblox client"), 100));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
