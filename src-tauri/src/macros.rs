//! The user's AutoHotkey v2 macros, run against RoALT's clients (restored from RoALT 1.0.0 for
//! AltDock). AutoHotkey isn't bundled: RoALT runs the one the user installed (`find_ahk`). Each
//! run is one AutoHotkey process per client, fed the script on stdin with the client's window id
//! and pid as its only arguments (never a cookie or anything else of the account), inside a
//! kill-on-close job: Stop, Kill all, the client exiting, RoALT exiting or dying all end it. Only
//! trusted macros run (`may_run`): typed in RoALT, pushed from the main app, or imported and then
//! reviewed. One macro per client at a time.
//!
//! Each account may have one macro (`account_macros`); with `autorun` it starts on the account's
//! client once that reaches a game (`watch`). A run that ends by itself while its client is still
//! in a game starts again after 5 s, at most `RESTARTS` times in 5 minutes.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectExtendedLimitInformation, SetInformationJobObject,
};
use windows::core::{Owned, PCWSTR};

use crate::App;
use crate::commands::{notice, now_ms};
use crate::error::{AppError, Result};
use crate::model::{AhkStatus, InstanceState, Macro, MacroAssignment, MacroInput, MacroRun};

/// How much of a run's stderr is kept for its error toast and the log.
const TAIL: usize = 4 << 10;
/// AutoHotkey's working folder, in the data folder.
const DIR: &str = "macros";
/// A run that ended by itself starts again after this long…
const RESTART_AFTER_MS: i64 = 5_000;
/// …at most this many times…
pub const RESTARTS: usize = 3;
/// …within this long.
const RESTART_WINDOW_MS: i64 = 5 * 60_000;
/// How often `watch` looks for clients that reached a game and restarts that are due.
const WATCH: Duration = Duration::from_secs(1);

/// A job that kills the process in it, and whatever that starts, when it is dropped or RoALT dies.
struct KillJob(#[allow(dead_code)] Owned<HANDLE>);
// SAFETY: a job handle may be used from any thread; it is only dropped (closed) under the runner's lock.
unsafe impl Send for KillJob {}

fn kill_on_close(child: &Child) -> Result<KillJob> {
    use std::os::windows::io::AsRawHandle;
    // SAFETY: default security, no name; Owned closes the handle on drop.
    let job = unsafe { CreateJobObjectW(None, PCWSTR::null()) }.map(|h| unsafe { Owned::new(h) }).map_err(|e| AppError::Internal(e.message()))?;
    let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: `info` is a local of the size passed.
    unsafe { SetInformationJobObject(*job, JobObjectExtendedLimitInformation, (&raw const info).cast(), size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32) }
        .map_err(|e| AppError::Internal(e.message()))?;
    // SAFETY: the child's handle is live while `child` is borrowed; the job only adds it.
    unsafe { AssignProcessToJobObject(*job, HANDLE(child.as_raw_handle())) }.map_err(|e| AppError::Internal(e.message()))?;
    Ok(KillJob(job))
}

struct Run {
    info: MacroRun,
    client_pid: u32,
    ahk_pid: u32,
    /// Dropping it ends the AutoHotkey process. None only in tests.
    _job: Option<KillJob>,
}

/// A run that ended by itself, to start again once `due` (unix ms).
#[derive(Clone, Debug, PartialEq)]
struct Restart {
    instance_id: String,
    client_pid: u32,
    macro_id: i64,
    due: i64,
}

/// The running macros, by instance id, and the macro hotkeys registered with Windows.
pub struct Runner {
    runs: Mutex<HashMap<String, Run>>,
    hotkeys: Mutex<Vec<(Shortcut, i64)>>,
    /// Restarts waiting for their 5 s, and when each client's run was restarted (unix ms).
    restarts: Mutex<(Vec<Restart>, HashMap<String, Vec<i64>>)>,
    /// (instance id, client pid) of the clients `watch` has seen in a game: autorun once each.
    seen: Mutex<HashSet<(String, u32)>>,
}

impl Runner {
    pub fn new() -> Runner {
        Runner { runs: Mutex::new(HashMap::new()), hotkeys: Mutex::new(Vec::new()), restarts: Mutex::new((Vec::new(), HashMap::new())), seen: Mutex::new(HashSet::new()) }
    }

    fn runs(&self) -> MutexGuard<'_, HashMap<String, Run>> {
        self.runs.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn hotkeys(&self) -> MutexGuard<'_, Vec<(Shortcut, i64)>> {
        self.hotkeys.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn restarts(&self) -> MutexGuard<'_, (Vec<Restart>, HashMap<String, Vec<i64>>)> {
        self.restarts.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn seen(&self) -> MutexGuard<'_, HashSet<(String, u32)>> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Oldest first.
    pub fn list(&self) -> Vec<MacroRun> {
        let mut out: Vec<MacroRun> = self.runs().values().map(|r| r.info.clone()).collect();
        out.sort_by_key(|r| r.started_at);
        out
    }

    /// Instance ids of the clients running a macro (anti-AFK leaves them alone).
    pub fn busy(&self) -> Vec<String> {
        self.runs().keys().cloned().collect()
    }

    /// Ends the run on this client, if any (its job closes), and any restart it waits for.
    pub fn stop(&self, instance_id: &str) {
        self.restarts().0.retain(|r| r.instance_id != instance_id);
        if self.runs().remove(instance_id).is_some() {
            log::info!("macro stopped on instance {instance_id}");
        }
    }

    /// Stops macro `id` everywhere; false when it wasn't running.
    pub fn stop_macro(&self, id: i64) -> bool {
        self.restarts().0.retain(|r| r.macro_id != id);
        let mut runs = self.runs();
        let before = runs.len();
        runs.retain(|_, r| r.info.macro_id != id);
        before != runs.len()
    }

    /// Stop all, Kill all, the panic hotkey, RoALT exiting.
    pub fn stop_all(&self) {
        self.restarts().0.clear();
        let mut runs = self.runs();
        if !runs.is_empty() {
            log::info!("stopping {} macro run(s)", runs.len());
        }
        runs.clear();
    }

    /// Runs whose client is gone (or came back under another pid) stop; what was kept for gone
    /// clients goes. `clients`: (instance id, client pid).
    fn sync(&self, clients: &[(String, u32)]) {
        let live = |id: &str, pid: u32| clients.iter().any(|c| c.0 == id && c.1 == pid);
        self.runs().retain(|id, r| live(id, r.client_pid));
        let mut restarts = self.restarts();
        restarts.0.retain(|r| live(&r.instance_id, r.client_pid));
        restarts.1.retain(|id, _| clients.iter().any(|c| c.0 == *id));
        self.seen().retain(|(id, pid)| live(id, *pid));
    }

    /// The run of AutoHotkey `ahk_pid` on `instance_id` ended: forgotten. Its info when it was
    /// still listed, so it ended by itself rather than being stopped.
    fn ended(&self, instance_id: &str, ahk_pid: u32) -> Option<(MacroRun, u32)> {
        let mut runs = self.runs();
        if !runs.get(instance_id).is_some_and(|r| r.ahk_pid == ahk_pid) {
            return None;
        }
        runs.remove(instance_id).map(|r| (r.info.clone(), r.client_pid))
    }

    /// A run on `instance_id` ended by itself at `now`: true when it may start again (and then it
    /// is counted and waits `RESTART_AFTER_MS`).
    fn plan_restart(&self, instance_id: &str, client_pid: u32, macro_id: i64, now: i64) -> bool {
        let mut restarts = self.restarts();
        let history = restarts.1.entry(instance_id.to_owned()).or_default();
        history.retain(|&t| now - t < RESTART_WINDOW_MS);
        if !may_restart(history, now) {
            return false;
        }
        history.push(now);
        restarts.0.retain(|r| r.instance_id != instance_id);
        restarts.0.push(Restart { instance_id: instance_id.to_owned(), client_pid, macro_id, due: now + RESTART_AFTER_MS });
        true
    }

    /// Restarts due at `now`, taken off the list.
    fn due(&self, now: i64) -> Vec<Restart> {
        let mut restarts = self.restarts();
        let (due, wait) = std::mem::take(&mut restarts.0).into_iter().partition(|r| r.due <= now);
        restarts.0 = wait;
        due
    }

    /// How often this client's run was restarted in the last 5 minutes.
    fn restarted(&self, instance_id: &str, now: i64) -> u32 {
        self.restarts().1.get(instance_id).map_or(0, |h| h.iter().filter(|&&t| now - t < RESTART_WINDOW_MS).count() as u32)
    }

    /// Starts `m` with AutoHotkey `ahk` on the client `client_pid` of instance `instance_id`.
    fn start(&self, app: &AppHandle, ahk: &Path, dir: &Path, m: &Macro, instance_id: &str, account_id: i64, client_pid: u32) -> Result<()> {
        let Some((hwnd, _)) = crate::instances::process::main_window(client_pid) else { return Err(not_in_game()) };
        let mut runs = self.runs();
        if runs.contains_key(instance_id) {
            return Err(AppError::Invalid(tr!("That client is already running a macro. Stop it first.")));
        }
        std::fs::create_dir_all(dir)?;
        // `*`: the script comes on stdin, which AutoHotkey reads to the end before it runs any of it,
        // so the job below is in place first.
        // shortcut: `*` and the directives appended below are unconfirmed on a real AutoHotkey v2; run one before release.
        let mut child = Command::new(ahk)
            .args(["/ErrorStdOut=UTF-8", "/CP65001", "*", &format!("{:#x}", hwnd.0 as usize), &client_pid.to_string()])
            .current_dir(dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| AppError::Invalid(tr!("AutoHotkey didn't start: {why}", why = e)))?;
        let job = kill_on_close(&child).inspect_err(|_| {
            let _ = child.kill();
        })?;
        let ahk_pid = child.id();
        let restarts = self.restarted(instance_id, now_ms());
        let info = MacroRun { instance_id: instance_id.to_owned(), account_id, macro_id: m.id, name: m.name.clone(), started_at: now_ms(), restarts };
        runs.insert(instance_id.to_owned(), Run { info, client_pid, ahk_pid, _job: Some(job) });
        drop(runs);
        log::info!("macro {} started on instance {instance_id} (AutoHotkey pid {ahk_pid})", m.id);
        // At the end, so the script's own line numbers stay right in its errors.
        let script = format!("{}\n#SingleInstance Off\n#NoTrayIcon\n", m.body);
        let (handle, id) = (app.clone(), instance_id.to_owned());
        std::thread::spawn(move || supervise(&handle, child, &script, &id, ahk_pid));
        emit_runs(app);
        Ok(())
    }
}

impl Default for Runner {
    fn default() -> Self {
        Self::new()
    }
}

/// Fewer than `RESTARTS` restarts in the 5 minutes before `now` (pure).
fn may_restart(history: &[i64], now: i64) -> bool {
    history.iter().filter(|&&t| now - t < RESTART_WINDOW_MS).count() < RESTARTS
}

/// Feeds a run its script, keeps the last `TAIL` of its stderr and waits for it. A run that ended
/// by itself while its client is still in a game starts again (`plan_restart`); one that can't, or
/// whose client left the game, is a toast when it failed or gave up.
fn supervise(app: &AppHandle, mut child: Child, script: &str, instance_id: &str, ahk_pid: u32) {
    if let Some(mut stdin) = child.stdin.take()
        && let Err(e) = stdin.write_all(script.as_bytes())
    {
        log::debug!("macro on instance {instance_id}: script not written: {e}");
    }
    // On its own thread: a process the script started can inherit stderr and keep it open after
    // AutoHotkey exits, until the job (dropped by `ended`) ends it.
    let (tx, rx) = std::sync::mpsc::channel();
    if let Some(mut err) = child.stderr.take() {
        std::thread::spawn(move || {
            let mut tail = Vec::new();
            let mut buf = [0u8; 1024];
            while let Ok(n @ 1..) = err.read(&mut buf) {
                tail.extend_from_slice(&buf[..n]);
                tail.drain(..tail.len().saturating_sub(TAIL));
            }
            let _ = tx.send(tail);
        });
    }
    let status = child.wait();
    let s = app.state::<App>();
    let ended = s.macros.ended(instance_id, ahk_pid);
    emit_runs(app);
    let failed = !status.as_ref().is_ok_and(|st| st.success());
    log::info!("macro on instance {instance_id} ended ({status:?}){}", if ended.is_some() { "" } else { ", stopped" });
    let Some((info, client_pid)) = ended else { return };
    let tail = if failed { rx.recv_timeout(Duration::from_millis(500)).unwrap_or_default() } else { Vec::new() };
    let said = String::from_utf8_lossy(&tail);
    if !said.trim().is_empty() {
        log::warn!("macro {} on instance {instance_id} said: {said}", info.name);
    }
    let why = said.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(300).collect::<String>());
    let in_game = in_game(&s).iter().any(|c| c.0 == instance_id && c.1 == client_pid);
    if in_game && s.macros.plan_restart(instance_id, client_pid, info.macro_id, now_ms()) {
        return log::info!("macro {} on instance {instance_id} ended by itself; starting it again in 5 s", info.macro_id);
    }
    let message = match (in_game, why) {
        (true, Some(why)) => tr!("Macro {name} ended {n} times in 5 minutes and wasn't started again. Last error: {why}", name = info.name, n = RESTARTS + 1, why = why),
        (true, None) => tr!("Macro {name} ended {n} times in 5 minutes and wasn't started again.", name = info.name, n = RESTARTS + 1),
        (false, Some(why)) => tr!("Macro {name} stopped with an error: {why}", name = info.name, why = why),
        (false, None) if failed => tr!("Macro {name} stopped with an error.", name = info.name),
        (false, None) => return,
    };
    notice(app, true, message);
}

fn emit_runs(app: &AppHandle) {
    let _ = app.emit("macros://runs", app.state::<App>().macros.list());
}

fn not_in_game() -> AppError {
    AppError::Invalid(tr!("That client isn't in a game."))
}

/// Locked: no new runs (running ones go on). Untrusted: never, until reviewed.
pub fn may_run(m: &Macro, locked: bool) -> Result<()> {
    if locked {
        return Err(AppError::Locked);
    }
    if !m.trusted {
        return Err(AppError::Invalid(tr!("Review {name} on the Macros page before it runs.", name = m.name)));
    }
    Ok(())
}

/// AutoHotkey v2's AutoHotkey64.exe: `custom` (Settings), else where its installer put it (the
/// registry's InstallDir, then Program Files, then the per-user Programs folder).
pub fn find_ahk(custom: &str) -> Option<PathBuf> {
    use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    let reg = |root, key| crate::launcher::client::reg_string(root, key, Some("InstallDir")).map(PathBuf::from);
    let env = |var, sub| std::env::var_os(var).map(|d| PathBuf::from(d).join(sub));
    let roots = [
        reg(HKEY_CURRENT_USER, r"SOFTWARE\AutoHotkey"),
        reg(HKEY_LOCAL_MACHINE, r"SOFTWARE\AutoHotkey"),
        reg(HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\AutoHotkey"),
        env("ProgramFiles", "AutoHotkey"),
        env("LOCALAPPDATA", r"Programs\AutoHotkey"),
    ];
    find_in(custom, roots.into_iter().flatten())
}

/// The first file of `custom`, then `<root>\v2\AutoHotkey64.exe` and `<root>\AutoHotkey64.exe`
/// of each root.
fn find_in(custom: &str, roots: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let custom = Some(PathBuf::from(custom.trim())).filter(|p| !p.as_os_str().is_empty());
    custom.into_iter().chain(roots.into_iter().flat_map(|r| [r.join(r"v2\AutoHotkey64.exe"), r.join("AutoHotkey64.exe")])).find(|p| p.is_file())
}

fn ahk(s: &App) -> Result<PathBuf> {
    find_ahk(&s.db.settings()?.ahk_path).ok_or_else(|| AppError::Invalid(tr!("Install AutoHotkey v2 to use macros.")))
}

/// Settings' `ahk_path`: "" or an existing absolute .exe.
pub fn check_ahk_path(p: &str) -> Result<String> {
    let p = p.trim();
    let path = Path::new(p);
    if !(p.is_empty() || (path.is_absolute() && path.is_file() && path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")))) {
        return Err(AppError::Invalid(tr!("Pick AutoHotkey64.exe, the AutoHotkey v2 program.")));
    }
    Ok(p.to_owned())
}

/// A macro file's text: UTF-8 with or without a BOM, or UTF-16 with its BOM. Anything else, or
/// text with NULs (UTF-16 without a BOM), is refused.
pub fn decode(bytes: &[u8]) -> Result<String> {
    let utf16 = |b: &[u8], be: bool| {
        if !b.len().is_multiple_of(2) {
            return None;
        }
        let units: Vec<u16> = b.as_chunks::<2>().0.iter().map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) }).collect();
        String::from_utf16(&units).ok()
    };
    let text = match bytes {
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8(rest.to_vec()).ok(),
        [0xFF, 0xFE, rest @ ..] => utf16(rest, false),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, true),
        _ => String::from_utf8(bytes.to_vec()).ok(),
    };
    let text = text.filter(|t| !t.contains('\0')).ok_or_else(|| AppError::Invalid(tr!("RoALT can't read that file as text. Save it as UTF-8 and import it again.")))?;
    check_text(&text)?;
    Ok(text)
}

/// Refuses source with bidi controls or invisible characters (a leading BOM aside): the review
/// must show exactly what runs. A zero-width joiner between two emoji (👨‍💻) is fine.
pub fn check_text(body: &str) -> Result<()> {
    let hidden = |c: char| {
        matches!(c,
            '\u{00AD}' | '\u{034F}' | '\u{061C}' | '\u{115F}'..='\u{1160}' | '\u{17B4}'..='\u{17B5}' | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
            | '\u{3164}' | '\u{FEFF}' | '\u{FFA0}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0000}'..='\u{E007F}')
    };
    let emoji = |c: Option<char>| c.is_some_and(|c| matches!(c, '\u{2600}'..='\u{27BF}' | '\u{FE0F}' | '\u{1F000}'..='\u{10FFFF}'));
    let chars: Vec<char> = body.strip_prefix('\u{FEFF}').unwrap_or(body).chars().collect();
    let joins_emoji = |i: usize| chars[i] == '\u{200D}' && emoji(i.checked_sub(1).map(|j| chars[j])) && emoji(chars.get(i + 1).copied());
    if (0..chars.len()).any(|i| hidden(chars[i]) && !joins_emoji(i)) {
        return Err(AppError::Invalid(tr!("This macro has invisible or text-direction characters. Remove them first.")));
    }
    Ok(())
}

/// A macro file's name (without extension) and text; its size is checked before it is read.
pub fn read_file(path: &Path) -> Result<(String, String)> {
    // UTF-16 takes up to twice the bytes; the decoded text is held to the limit when saved.
    if std::fs::metadata(path)?.len() > 2 * crate::storage::MAX_MACRO as u64 {
        return Err(AppError::Invalid(tr!("A macro can be at most {max} KiB.", max = crate::storage::MAX_MACRO >> 10)));
    }
    let name = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok((name, decode(&std::fs::read(path)?)?))
}

/// `key` ("" for none) as a macro hotkey: a key combination that isn't the panic hotkey or
/// another macro's (`id`'s own is fine).
pub fn check_hotkey(key: &str, id: Option<i64>, macros: &[Macro], panic: &str) -> Result<()> {
    if key.is_empty() {
        return Ok(());
    }
    let k = crate::commands::parse_hotkey(key)?;
    let same = |other: &str| other.parse::<Shortcut>().is_ok_and(|o| o == k);
    if same(panic) {
        return Err(AppError::Invalid(tr!("{key} is already the panic hotkey.", key = key)));
    }
    match macros.iter().find(|m| Some(m.id) != id && same(&m.hotkey)) {
        Some(m) => Err(AppError::Invalid(tr!("{key} is already the hotkey of {name}.", key = key, name = m.name))),
        None => Ok(()),
    }
}

/// Windows gives RoALT `key` ("" for none), unless it is already one of RoALT's macro hotkeys.
pub fn can_register(app: &AppHandle, key: &str) -> Result<()> {
    let Ok(k) = key.parse::<Shortcut>() else { return Ok(()) };
    if app.state::<App>().macros.hotkeys().iter().any(|(h, _)| *h == k) {
        return Ok(());
    }
    let gs = app.global_shortcut();
    gs.register(k).map_err(|e| {
        log::warn!("could not register the macro hotkey {key}: {e}");
        AppError::Invalid(tr!("Windows wouldn't give RoALT {key}: another app probably uses it. Pick another.", key = key))
    })?;
    if let Err(e) = gs.unregister(k) {
        log::warn!("could not unregister the macro hotkey {key} after trying it: {e}");
    }
    Ok(())
}

/// Registers every trusted macro's hotkey anew (at start, after a save, a delete, a trust or a
/// restore). One Windows won't give is logged and left out.
pub fn sync_hotkeys(app: &AppHandle) {
    let s = app.state::<App>();
    let gs = app.global_shortcut();
    // Not held while Windows is asked: a key pressed meanwhile looks the list up (`hotkey`).
    let old = std::mem::take(&mut *s.macros.hotkeys());
    for (k, id) in old {
        if let Err(e) = gs.unregister(k) {
            log::warn!("could not unregister the hotkey of macro {id}: {e}");
        }
    }
    let macros = match s.db.macros() {
        Ok(m) => m,
        Err(e) => return log::warn!("no macro hotkeys: could not read the macros: {e}"),
    };
    let mut held = Vec::new();
    for m in macros.iter().filter(|m| m.trusted && !m.hotkey.is_empty()) {
        match m.hotkey.parse::<Shortcut>().map(|k| (k, gs.register(k))) {
            Ok((k, Ok(()))) => held.push((k, m.id)),
            Ok((_, Err(e))) => log::warn!("could not register the hotkey {} of macro {}: {e}", m.hotkey, m.id),
            Err(_) => log::warn!("macro {} has no usable hotkey: {:?}", m.id, m.hotkey),
        }
    }
    *s.macros.hotkeys() = held;
}

/// What a pressed global shortcut is.
#[derive(Debug, PartialEq)]
pub enum Pressed {
    Macro(i64),
    Panic,
    /// Neither: a macro's key pressed while the hotkeys re-sync.
    Ignore,
}

/// `pressed` among the macro hotkeys, then the panic hotkey `panic`.
pub fn classify(pressed: &Shortcut, macros: &[(Shortcut, i64)], panic: &str) -> Pressed {
    if let Some(&(_, id)) = macros.iter().find(|(k, _)| k == pressed) {
        return Pressed::Macro(id);
    }
    if panic.parse::<Shortcut>().is_ok_and(|p| p == *pressed) { Pressed::Panic } else { Pressed::Ignore }
}

/// A pressed global shortcut: a macro's is toggled off this thread, the panic hotkey kills all.
/// When the settings can't be read, an unmatched key is taken as the panic hotkey.
pub fn hotkey(app: &AppHandle, key: &Shortcut) {
    let s = app.state::<App>();
    let panic = s.db.settings().map(|st| st.panic_hotkey);
    let what = match &panic {
        Ok(p) => classify(key, &s.macros.hotkeys(), p),
        Err(_) => match classify(key, &s.macros.hotkeys(), "") {
            Pressed::Ignore => Pressed::Panic,
            m => m,
        },
    };
    match what {
        Pressed::Macro(id) => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || toggle(&app, id));
        }
        Pressed::Panic => crate::tray::kill_all(app),
        Pressed::Ignore => log::debug!("a hotkey that is no longer RoALT's was pressed; ignored"),
    }
}

/// Running anywhere: stopped everywhere. Else started on every client in a game, but not on Full
/// quality accounts or clients already running a macro.
fn toggle(app: &AppHandle, id: i64) {
    let s = app.state::<App>();
    if s.macros.stop_macro(id) {
        return emit_runs(app);
    }
    let m = match s.db.macro_by_id(id) {
        Ok(m) => m,
        Err(e) => return log::warn!("macro hotkey: could not read macro {id}: {e}"),
    };
    if let Err(e) = may_run(&m, s.vault.is_locked()) {
        return notice(app, true, e.message());
    }
    let full = s.db.full_quality_accounts().unwrap_or_default();
    let busy = s.macros.busy();
    let targets: Vec<_> = in_game(&s).into_iter().filter(|(id, _, account)| !full.contains(account) && !busy.contains(id)).collect();
    match start_on(app, &s, &m, &targets) {
        (0, Some(e)) => notice(app, true, e.message()),
        (0, None) => notice(app, true, tr!("No client in a game can run {name} now.", name = m.name)),
        _ => {}
    }
}

/// Clients in a game: (instance id, client pid, account id).
fn in_game(s: &App) -> Vec<(String, u32, i64)> {
    s.tracker
        .clients()
        .into_iter()
        .filter_map(|(id, pid, ..)| s.tracker.get(&id).filter(|i| i.state == InstanceState::InGame).map(|i| (id, pid, i.account_id)))
        .collect()
}

/// Starts `m` on each (instance id, client pid, account id): how many started, and the first refusal.
fn start_on(app: &AppHandle, s: &App, m: &Macro, targets: &[(String, u32, i64)]) -> (usize, Option<AppError>) {
    let ahk = match ahk(s) {
        Ok(p) => p,
        Err(e) => return (0, Some(e)),
    };
    let dir = s.data_dir.join(DIR);
    let mut started = 0;
    let mut first = None;
    for (id, pid, account) in targets {
        match s.macros.start(app, &ahk, &dir, m, id, *account, *pid) {
            Ok(()) => started += 1,
            Err(e) => {
                log::info!("macro {} not started on instance {id}: {e}", m.id);
                first.get_or_insert(e);
            }
        }
    }
    (started, first)
}

/// Follows the clients for the app's lifetime: runs of gone clients stop, due restarts start,
/// and a client that reached a game starts its account's autorun macro (once per client).
pub fn spawn(app: AppHandle) {
    let started = std::thread::Builder::new().name("macros".into()).spawn(move || {
        loop {
            std::thread::sleep(WATCH);
            watch(&app);
        }
    });
    if let Err(e) = started {
        log::warn!("macro autorun and restarts not running: {e}");
    }
}

fn watch(app: &AppHandle) {
    let s = app.state::<App>();
    let clients: Vec<(String, u32)> = s.tracker.clients().into_iter().map(|c| (c.0, c.1)).collect();
    let before = s.macros.runs().len();
    s.macros.sync(&clients);
    if s.macros.runs().len() != before {
        emit_runs(app);
    }
    let live = in_game(&s);
    for r in s.macros.due(now_ms()) {
        let Some(target) = live.iter().find(|c| c.0 == r.instance_id && c.1 == r.client_pid).cloned() else { continue };
        if s.macros.runs().contains_key(&r.instance_id) {
            continue;
        }
        let m = match s.db.macro_by_id(r.macro_id).and_then(|m| may_run(&m, s.vault.is_locked()).map(|()| m)) {
            Ok(m) => m,
            Err(e) => {
                log::info!("macro {} not restarted on instance {}: {e}", r.macro_id, r.instance_id);
                continue;
            }
        };
        if let (0, Some(e)) = start_on(app, &s, &m, &[target]) {
            notice(app, true, tr!("Macro {name} didn't start: {why}", name = m.name, why = e.message()));
        }
    }
    // Locked: they wait, and start once it is unlocked.
    if s.vault.is_locked() {
        return;
    }
    let fresh: Vec<_> = live.into_iter().filter(|(id, pid, _)| !s.macros.seen().contains(&(id.clone(), *pid))).collect();
    for (id, pid, account) in fresh {
        // Not while it loads: its window comes first.
        if crate::instances::process::main_window(pid).is_none() {
            continue;
        }
        s.macros.seen().insert((id.clone(), pid));
        auto_run(app, &s, &id, pid, account);
    }
}

/// A client of `account` reached a game: its assigned macro starts when the assignment says autorun.
fn auto_run(app: &AppHandle, s: &App, instance_id: &str, pid: u32, account: i64) {
    let assigned = match s.db.macro_assignment(account) {
        Ok(Some(a)) if a.autorun => a,
        Ok(_) => return,
        Err(e) => return log::warn!("autorun: could not read the macro of account {account}: {e}"),
    };
    if s.macros.runs().contains_key(instance_id) {
        return;
    }
    let m = match s.db.macro_by_id(assigned.macro_id).and_then(|m| may_run(&m, false).map(|()| m)) {
        Ok(m) => m,
        Err(e) => return notice(app, true, tr!("The macro of this account didn't start: {why}", why = e.message())),
    };
    if let (0, Some(e)) = start_on(app, s, &m, &[(instance_id.to_owned(), pid, account)]) {
        notice(app, true, tr!("Macro {name} didn't start: {why}", name = m.name, why = e.message()));
    }
}

// ---------- public functions (the control API calls these too) ----------

/// Every macro by name, with its source.
pub fn list(s: &App) -> Result<Vec<Macro>> {
    s.db.macros()
}

/// Saves `input` as trusted (pushed from the main app or typed here). Without an id, a macro of
/// the same name is replaced, so pushing again updates it. Its hotkey must not clash; the caller
/// registers hotkeys again (`sync_hotkeys`) where it has the app.
// Called by the control API (`POST /v1/macros`), which lands on its own branch.
#[allow(dead_code)]
pub fn save(s: &App, mut input: MacroInput) -> Result<Macro> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    input.hotkey = input.hotkey.trim().to_owned();
    check_text(&input.body)?;
    if input.id.is_none() {
        input.id = s.db.macro_named(&input.name)?;
    }
    check_hotkey(&input.hotkey, input.id, &s.db.macros()?, &s.db.settings()?.panic_hotkey)?;
    let id = s.db.save_macro(&input, true)?;
    s.db.macro_by_id(id)
}

/// Gives `account_id` macro `macro_id` (None: none); `autorun`: it starts when a client of the
/// account reaches a game.
pub fn assign(s: &App, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<()> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    s.db.assign_macro(account_id, macro_id, autorun)
}

/// Macro `macro_id` on these clients (each must be in a game). Err when any didn't start.
pub fn run(app: &AppHandle, s: &App, macro_id: i64, instance_ids: &[String]) -> Result<()> {
    let m = s.db.macro_by_id(macro_id)?;
    may_run(&m, s.vault.is_locked())?;
    let live = in_game(s);
    let targets = instance_ids.iter().map(|i| live.iter().find(|c| c.0 == *i).cloned().ok_or_else(not_in_game)).collect::<Result<Vec<_>>>()?;
    match start_on(app, s, &m, &targets) {
        (_, Some(e)) => Err(e),
        _ => Ok(()),
    }
}

/// Stops the macro on this client (and a restart it waits for); macros://runs follows once
/// AutoHotkey has ended.
pub fn stop_instance(app: &AppHandle, instance_id: &str) {
    app.state::<App>().macros.stop(instance_id);
}

/// What runs now, oldest first.
pub fn runs(s: &App) -> Vec<MacroRun> {
    s.macros.list()
}

// ---------- commands ----------

fn unlocked(s: &App) -> Result<()> {
    if s.vault.is_locked() { Err(AppError::Locked) } else { Ok(()) }
}

/// Where AutoHotkey v2 is (Settings' own pick first); None: not installed.
#[tauri::command]
pub fn ahk_status(s: State<'_, App>) -> Result<AhkStatus> {
    Ok(AhkStatus { path: find_ahk(&s.db.settings()?.ahk_path).map(|p| p.display().to_string()) })
}

/// Every macro by name, with its source.
#[tauri::command]
pub fn macros_list(s: State<'_, App>) -> Result<Vec<Macro>> {
    list(&s)
}

/// Create or edit from the editor: a new macro is trusted, an edited one keeps its trust (only
/// the review trusts an imported one). A new or changed hotkey must be free.
#[tauri::command]
pub fn macro_save(app: AppHandle, s: State<'_, App>, mut input: MacroInput) -> Result<Macro> {
    unlocked(&s)?;
    input.hotkey = input.hotkey.trim().to_owned();
    check_text(&input.body)?;
    let macros = s.db.macros()?;
    check_hotkey(&input.hotkey, input.id, &macros, &s.db.settings()?.panic_hotkey)?;
    // An unchanged key isn't tried again: one another app took before RoALT started would block every edit.
    let key = |k: &str| k.parse::<Shortcut>().ok();
    if !macros.iter().any(|m| Some(m.id) == input.id && key(&m.hotkey) == key(&input.hotkey)) {
        can_register(&app, &input.hotkey)?;
    }
    let id = s.db.save_macro(&input, input.id.is_none())?;
    sync_hotkeys(&app);
    s.db.macro_by_id(id)
}

/// Stops it where it runs, then deletes it (and the accounts' assignments of it).
#[tauri::command]
pub fn macro_remove(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    unlocked(&s)?;
    s.macros.stop_macro(id);
    s.db.remove_macro(id)?;
    sync_hotkeys(&app);
    emit_runs(&app);
    Ok(())
}

/// The user reviewed its whole source: it may run from now on.
#[tauri::command]
pub fn macro_trust(app: AppHandle, s: State<'_, App>, id: i64) -> Result<Macro> {
    unlocked(&s)?;
    s.db.trust_macro(id)?;
    sync_hotkeys(&app);
    s.db.macro_by_id(id)
}

/// `.ahk`/`.txt` files, each a new untrusted macro named after its file; nothing runs. Stops at
/// the first file that can't be read (the ones before it stay imported).
#[tauri::command]
pub async fn macros_import(s: State<'_, App>, paths: Vec<String>) -> Result<Vec<Macro>> {
    unlocked(&s)?;
    let mut out = Vec::new();
    for p in &paths {
        let (name, body) = read_file(Path::new(p))?;
        out.push(s.db.macro_by_id(s.db.import_macro(&name, &body)?)?);
    }
    Ok(out)
}

/// One macro as a `.ahk` file (UTF-8 with a BOM, as AutoHotkey reads it).
#[tauri::command]
pub async fn macro_export(s: State<'_, App>, id: i64, path: String) -> Result<()> {
    let m = s.db.macro_by_id(id)?;
    let mut bytes = b"\xEF\xBB\xBF".to_vec();
    bytes.extend_from_slice(m.body.as_bytes());
    crate::vault::backup::write_atomic(Path::new(&path), &bytes)
}

/// Starts macro `id` on these clients, one AutoHotkey each; macros://runs follows.
#[tauri::command]
pub async fn macro_run(app: AppHandle, s: State<'_, App>, id: i64, instance_ids: Vec<String>) -> Result<Vec<MacroRun>> {
    run(&app, &s, id, &instance_ids)?;
    Ok(runs(&s))
}

/// Stops the macro on this client; macros://runs follows once AutoHotkey has ended.
#[tauri::command]
pub fn macro_stop(app: AppHandle, instance_id: String) {
    stop_instance(&app, &instance_id);
}

/// Stops every running macro.
#[tauri::command]
pub fn macros_stop_all(s: State<'_, App>) {
    s.macros.stop_all();
}

/// What runs now, oldest first.
#[tauri::command]
pub fn macro_runs(s: State<'_, App>) -> Vec<MacroRun> {
    runs(&s)
}

/// Every account's macro.
#[tauri::command]
pub fn macro_assignments(s: State<'_, App>) -> Result<Vec<MacroAssignment>> {
    s.db.macro_assignments()
}

/// Gives an account a macro (None: none) and whether it starts on its own in a game.
#[tauri::command]
pub fn macro_assign(app: AppHandle, s: State<'_, App>, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<Vec<MacroAssignment>> {
    assign(&s, account_id, macro_id, autorun)?;
    let _ = app.emit("macros://assignments", ());
    s.db.macro_assignments()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macro_with(id: i64, hotkey: &str, trusted: bool) -> Macro {
        Macro { id, name: format!("m{id}"), body: String::new(), trusted, hotkey: hotkey.into(), created_at: 0, updated_at: 0 }
    }

    #[test]
    fn decodes_utf8_and_utf16_by_bom_only() {
        let utf16 = |be: bool, bom: bool| {
            let mut out: Vec<u8> = if bom { if be { vec![0xFE, 0xFF] } else { vec![0xFF, 0xFE] } } else { Vec::new() };
            for u in "Send 'ä'".encode_utf16() {
                out.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
            }
            out
        };
        assert_eq!(decode("Send 'ä'".as_bytes()).unwrap(), "Send 'ä'");
        assert_eq!(decode(b"\xEF\xBB\xBFSend 'x'").unwrap(), "Send 'x'");
        assert_eq!(decode(&utf16(false, true)).unwrap(), "Send 'ä'");
        assert_eq!(decode(&utf16(true, true)).unwrap(), "Send 'ä'");
        assert_eq!(decode(b"").unwrap(), "");
        let mut odd = utf16(false, true);
        odd.push(b'x');
        let bad = [utf16(false, false), odd, b"Send\0'x'".to_vec(), b"\xFF\xFE\x00\xD8".to_vec(), b"caf\xE9".to_vec(), b"\xEF\xBB\xBF\xC3".to_vec()];
        for b in bad {
            assert!(matches!(decode(&b), Err(AppError::Invalid(_))), "{b:?}");
        }
    }

    #[test]
    fn finds_autohotkey_v2_first() {
        let tmp = std::env::temp_dir().join(format!("roalt-ahk-{}", uuid::Uuid::new_v4()));
        let touch = |p: &Path| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        };
        let (a, b) = (tmp.join("a"), tmp.join("b"));
        let roots = || [tmp.join("none"), a.clone(), b.clone()];
        assert_eq!(find_in("", roots()), None);
        touch(&b.join(r"v2\AutoHotkey64.exe"));
        touch(&a.join("AutoHotkey64.exe"));
        assert_eq!(find_in("", roots()), Some(a.join("AutoHotkey64.exe")), "the first root that has one");
        touch(&a.join(r"v2\AutoHotkey64.exe"));
        assert_eq!(find_in("", roots()), Some(a.join(r"v2\AutoHotkey64.exe")), "v2 before the root's own");
        let custom = tmp.join(r"mine\AHK.exe");
        assert_eq!(find_in(&custom.display().to_string(), roots()), Some(a.join(r"v2\AutoHotkey64.exe")), "a picked file that is gone");
        touch(&custom);
        assert_eq!(find_in(&format!(" {} ", custom.display()), roots()), Some(custom.clone()));
        // A folder named like it isn't it.
        std::fs::create_dir_all(tmp.join(r"c\v2\AutoHotkey64.exe")).unwrap();
        assert_eq!(find_in("", [tmp.join("c")]), None);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn hotkeys_never_clash() {
        let macros = [macro_with(1, "Ctrl+Shift+F1", true), macro_with(2, "", true)];
        assert!(check_hotkey("", None, &macros, "Ctrl+Shift+F12").is_ok());
        assert!(check_hotkey("Ctrl+Shift+F2", None, &macros, "Ctrl+Shift+F12").is_ok());
        assert!(check_hotkey("shift+control+F1", Some(1), &macros, "").is_ok(), "its own");
        for (key, id) in [("control+shift+F1", None), ("Ctrl+Shift+F1", Some(2)), ("Ctrl+Shift+F12", Some(1)), ("Shift+Ctrl+F12", None), ("nonsense", None)] {
            assert!(matches!(check_hotkey(key, id, &macros, "Ctrl+Shift+F12"), Err(AppError::Invalid(_))), "{key}");
        }
    }

    #[test]
    fn refuses_hidden_and_bidi_characters() {
        assert!(check_text("Send 'ä'\n\tSleep 10").is_ok());
        assert!(check_text("\u{FEFF}Send 'x'").is_ok(), "a leading BOM");
        assert!(check_text("Send \"gg 👨\u{200D}💻 🏳\u{FE0F}\u{200D}🌈\"").is_ok(), "emoji joined by ZWJ");
        let hidden = ['\u{202A}', '\u{202E}', '\u{2066}', '\u{2069}', '\u{200B}', '\u{200D}', '\u{200F}', '\u{FEFF}', '\u{00AD}', '\u{061C}', '\u{2060}', '\u{3164}', '\u{E0041}'];
        for c in hidden {
            assert!(matches!(check_text(&format!("Send 'a{c}b'")), Err(AppError::Invalid(_))), "{:x}", c as u32);
        }
        assert!(matches!(check_text("\u{FEFF}\u{FEFF}x"), Err(AppError::Invalid(_))), "a second BOM");
        assert!(matches!(decode("x\u{202E}y".as_bytes()), Err(AppError::Invalid(_))), "on import too");
    }

    #[test]
    fn pressed_keys_are_a_macro_the_panic_key_or_nothing() {
        let k = |s: &str| s.parse::<Shortcut>().unwrap();
        let macros = [(k("Ctrl+Shift+F1"), 1), (k("Ctrl+Shift+F2"), 2)];
        assert_eq!(classify(&k("shift+control+F2"), &macros, "Ctrl+Shift+F12"), Pressed::Macro(2));
        assert_eq!(classify(&k("Ctrl+Shift+F12"), &macros, "Shift+Ctrl+F12"), Pressed::Panic);
        assert_eq!(classify(&k("Ctrl+Shift+F12"), &[], "Ctrl+Shift+F12"), Pressed::Panic);
        // A macro key mid re-sync, with the panic hotkey off or another key.
        assert_eq!(classify(&k("Ctrl+Shift+F1"), &[], ""), Pressed::Ignore);
        assert_eq!(classify(&k("Ctrl+Shift+F1"), &[], "Ctrl+Shift+F12"), Pressed::Ignore);
    }

    fn run(r: &Runner, instance_id: &str, client_pid: u32, ahk_pid: u32) {
        let info = MacroRun { instance_id: instance_id.into(), account_id: 1, macro_id: 1, name: "m".into(), started_at: 0, restarts: 0 };
        r.runs().insert(instance_id.into(), Run { info, client_pid, ahk_pid, _job: None });
    }

    #[test]
    fn runs_end_only_for_their_own_process() {
        let r = Runner::new();
        run(&r, "a", 10, 100);
        assert!(r.ended("a", 999).is_none(), "a newer run's process");
        assert!(r.ended("b", 100).is_none());
        assert_eq!(r.busy(), ["a"]);
        assert_eq!(r.ended("a", 100).map(|(i, pid)| (i.instance_id, pid)), Some(("a".into(), 10)));
        assert!(r.busy().is_empty());
        assert!(r.ended("a", 100).is_none(), "already stopped");
    }

    #[test]
    fn runs_stop_when_their_client_is_gone() {
        let r = Runner::new();
        run(&r, "a", 10, 100);
        run(&r, "b", 20, 200);
        run(&r, "c", 30, 300);
        assert!(r.plan_restart("c", 30, 1, 0));
        // "b" came back under another pid, "c" is gone.
        r.sync(&[("a".into(), 10), ("b".into(), 21)]);
        assert_eq!(r.busy(), ["a"]);
        assert!(r.due(i64::MAX).is_empty(), "c's restart went with it");
    }

    #[test]
    fn restarts_after_5_s_at_most_3_times_in_5_minutes() {
        let min = 60_000;
        assert!(may_restart(&[], 0));
        assert!(may_restart(&[0, 1], 2));
        assert!(!may_restart(&[0, 1, 2], 3));
        assert!(may_restart(&[0, 1, 2], 5 * min), "the first is 5 minutes old");

        let r = Runner::new();
        assert!(r.plan_restart("a", 10, 7, 0));
        assert!(r.due(4_999).is_empty());
        assert_eq!(r.due(5_000), [Restart { instance_id: "a".into(), client_pid: 10, macro_id: 7, due: 5_000 }]);
        assert!(r.due(5_000).is_empty(), "taken once");
        assert!(r.plan_restart("a", 10, 7, min));
        assert!(r.plan_restart("a", 10, 7, 2 * min));
        assert_eq!(r.restarted("a", 2 * min), 3);
        assert!(!r.plan_restart("a", 10, 7, 3 * min), "a fourth within 5 minutes");
        assert!(r.plan_restart("b", 20, 7, 3 * min), "per client");
        assert!(r.plan_restart("a", 10, 7, 5 * min + 1), "the first fell out of the window");
        // Stop and stopping the macro drop a waiting restart.
        r.stop("a");
        r.stop_macro(7);
        assert!(r.due(i64::MAX).is_empty());
    }

    #[test]
    fn only_trusted_macros_run_and_not_while_locked() {
        assert!(may_run(&macro_with(1, "", true), false).is_ok());
        assert!(matches!(may_run(&macro_with(1, "", false), false), Err(AppError::Invalid(_))));
        assert!(matches!(may_run(&macro_with(1, "", true), true), Err(AppError::Locked)));
    }
}
