//! Anti-AFK. Roblox disconnects a player after 20 minutes without input (reason 278). While
//! `Settings.anti_afk` is on, each client in a game whose account hasn't opted out
//! (`AfkClientConfig.enabled`, mirrored in `Instance.anti_afk`) is, every `interval_sec` of its
//! account, brought to the front for a moment, gets its account's action (`AfkAction`: F13 by
//! default; no game binds F13, so the character doesn't move) as real key presses through
//! `SendInput` (by scan code, as a keyboard sends them), and the window that was in front gets the
//! focus back. One client at a time, about 100 ms each (a jump-and-turn holds a key 0.6 s). Only
//! while nobody has touched the keyboard or mouse for 5 s (`GetLastInputInfo`); otherwise it tries
//! again 30 s later. A minimized client is restored for its press (`window::unhide`) and
//! minimized again. Clients running a macro are left alone: its keys keep them active.
//!
//! Measured 2026-11-02 on minimized clients over 21 minutes: keys posted to the window
//! (WM_KEYDOWN, with or without WM_ACTIVATE) and focus without a key don't count as activity
//! and the client is kicked; a real Space or F13 does. Games with their own idle handling
//! teleport such clients back to spawn before the kick, which stops egg hatching.
//!
//! Nothing is injected into or read from the client; auto-rejoin on the 278 kick stays the
//! fallback.

use std::thread::sleep;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetLastInputInfo, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, LASTINPUTINFO, MOUSEEVENTF_MOVE, MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, SetForegroundWindow};

use super::{process, window};
use crate::App;
use crate::commands::{instances_changed, now_ms};
use crate::error::{AppError, Result};
use crate::model::{AfkAction, AfkClientConfig, InstanceState, Settings};

/// No keyboard or mouse input for this long counts as the user being away.
const IDLE_MS: u32 = 5_000;
const TICK: Duration = Duration::from_secs(10);
/// After the user was active, or a client couldn't be brought to the front.
const RETRY: Duration = Duration::from_secs(30);
const BETWEEN: Duration = Duration::from_millis(100);
/// A restored client's time to draw again before it takes the focus.
const RESTORED: Duration = Duration::from_millis(300);
/// A client's own interval, in seconds: at least a minute, under Roblox's 20 idle minutes.
pub const INTERVAL_SEC: std::ops::RangeInclusive<u32> = 60..=1140;
/// How long a key is held, and the pause after it is let go.
const HOLD_MS: u64 = 40;
/// How long `JumpTurn` holds the Left arrow.
const TURN_MS: u64 = 600;

/// A key by its set-1 scan code; `extended`: sent with the E0 prefix (the arrows).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Key {
    pub scan: u16,
    pub extended: bool,
}

const fn key(scan: u16) -> Key {
    Key { scan, extended: false }
}

const F13: Key = key(0x64);
const SPACE: Key = key(0x39);
const LEFT: Key = Key { scan: 0x4B, extended: true };

/// The keys an `AfkAction::Key` may name, with their scan codes. Nothing that opens a menu or the
/// chat in Roblox (Esc, Alt, Win, `/`, Enter) and no modifiers.
pub const KEYS: &[(&str, Key)] = &[
    ("A", key(0x1E)), ("B", key(0x30)), ("C", key(0x2E)), ("D", key(0x20)), ("E", key(0x12)), ("F", key(0x21)),
    ("G", key(0x22)), ("H", key(0x23)), ("I", key(0x17)), ("J", key(0x24)), ("K", key(0x25)), ("L", key(0x26)),
    ("M", key(0x32)), ("N", key(0x31)), ("O", key(0x18)), ("P", key(0x19)), ("Q", key(0x10)), ("R", key(0x13)),
    ("S", key(0x1F)), ("T", key(0x14)), ("U", key(0x16)), ("V", key(0x2F)), ("W", key(0x11)), ("X", key(0x2D)),
    ("Y", key(0x15)), ("Z", key(0x2C)),
    ("0", key(0x0B)), ("1", key(0x02)), ("2", key(0x03)), ("3", key(0x04)), ("4", key(0x05)), ("5", key(0x06)),
    ("6", key(0x07)), ("7", key(0x08)), ("8", key(0x09)), ("9", key(0x0A)),
    ("F1", key(0x3B)), ("F2", key(0x3C)), ("F3", key(0x3D)), ("F4", key(0x3E)), ("F5", key(0x3F)), ("F6", key(0x40)),
    ("F7", key(0x41)), ("F8", key(0x42)), ("F9", key(0x43)), ("F10", key(0x44)), ("F11", key(0x57)), ("F12", key(0x58)),
    ("F13", key(0x64)), ("F14", key(0x65)), ("F15", key(0x66)), ("F16", key(0x67)), ("F17", key(0x68)), ("F18", key(0x69)),
    ("F19", key(0x6A)), ("F20", key(0x6B)), ("F21", key(0x6C)), ("F22", key(0x6D)), ("F23", key(0x6E)), ("F24", key(0x76)),
    ("Space", SPACE), ("Tab", key(0x0F)),
    ("Up", Key { scan: 0x48, extended: true }), ("Down", Key { scan: 0x50, extended: true }), ("Left", LEFT), ("Right", Key { scan: 0x4D, extended: true }),
];

/// `name` in `KEYS`, ignoring case: its own spelling and its key.
pub fn key_named(name: &str) -> Option<(&'static str, Key)> {
    KEYS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name.trim())).copied()
}

/// One thing `press` does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Down(Key),
    Up(Key),
    /// Milliseconds; input other than ours meanwhile means the user is back.
    Wait(u64),
}

fn tap(k: Key) -> [Step; 4] {
    [Step::Down(k), Step::Wait(HOLD_MS), Step::Up(k), Step::Wait(HOLD_MS)]
}

/// What `action` presses (pure); None: a key name not in `KEYS`.
pub fn steps(action: &AfkAction) -> Option<Vec<Step>> {
    Some(match action {
        AfkAction::F13 => tap(F13).to_vec(),
        AfkAction::Space | AfkAction::Jump => tap(SPACE).to_vec(),
        AfkAction::JumpTurn => [&tap(SPACE)[..], &[Step::Down(LEFT), Step::Wait(TURN_MS), Step::Up(LEFT), Step::Wait(HOLD_MS)]].concat(),
        AfkAction::Key(name) => tap(key_named(name)?.1).to_vec(),
    })
}

/// `c` checked: its interval in `INTERVAL_SEC`, its key in `KEYS` (spelt as there).
pub fn check(mut c: AfkClientConfig) -> Result<AfkClientConfig> {
    if !INTERVAL_SEC.contains(&c.interval_sec) {
        return Err(AppError::Invalid(tr!("Anti-AFK must press a key every 1 to 19 minutes.")));
    }
    if let AfkAction::Key(name) = &c.action {
        let Some((own, _)) = key_named(name) else {
            return Err(AppError::Invalid(tr!("Anti-AFK can't press \"{key}\". Pick a letter, a digit, F1 to F24, Space, Tab or an arrow key.", key = name.chars().take(16).collect::<String>())));
        };
        c.action = AfkAction::Key(own.to_owned());
    }
    Ok(c)
}

/// What an account without its own setting gets: on, the global interval, F13.
pub fn default_config(v: &Settings) -> AfkClientConfig {
    let interval_sec = (v.anti_afk.interval_min.saturating_mul(60)).clamp(*INTERVAL_SEC.start(), *INTERVAL_SEC.end());
    AfkClientConfig { enabled: true, interval_sec, action: AfkAction::F13 }
}

fn config_with(s: &App, v: &Settings, account_id: i64) -> AfkClientConfig {
    match s.db.afk_config(account_id) {
        Ok(Some((enabled, interval_sec, action))) => AfkClientConfig { enabled, interval_sec, action },
        Ok(None) => default_config(v),
        Err(e) => {
            log::warn!("anti-AFK: could not read account {account_id}'s setting, using the default: {e}");
            default_config(v)
        }
    }
}

/// The account's anti-AFK: its own, else the default from the global setting.
pub fn client_config(s: &App, account_id: i64) -> AfkClientConfig {
    let v = s.db.settings().unwrap_or_else(|e| {
        log::warn!("anti-AFK: could not read settings: {e}");
        Settings::default()
    });
    config_with(s, &v, account_id)
}

/// Stores the account's anti-AFK (checked) and applies it to its running clients now.
pub fn set_client(app: &AppHandle, s: &App, account_id: i64, config: AfkClientConfig) -> Result<()> {
    let c = check(config)?;
    s.db.set_afk_config(account_id, c.enabled, c.interval_sec, &c.action)?;
    log::info!("anti-AFK for account {account_id}: on: {}, every {} s, {:?}", c.enabled, c.interval_sec, c.action);
    for i in s.tracker.list().into_iter().filter(|i| i.account_id == account_id && i.state != InstanceState::Exited) {
        s.tracker.set_anti_afk(&i.id, c.enabled);
    }
    instances_changed(app);
    Ok(())
}

/// A running client's on/off (`instance_set_anti_afk`): its account's, so it lasts beyond this client.
pub fn set_instance(app: &AppHandle, s: &App, instance_id: &str, enabled: bool) -> Result<()> {
    let account_id = s.tracker.get(instance_id).filter(|i| i.state != InstanceState::Exited).ok_or(AppError::NotFound)?.account_id;
    set_client(app, s, account_id, AfkClientConfig { enabled, ..client_config(s, account_id) })
}

/// A client anti-AFK may press keys in (`Tracker::afk_clients`).
#[derive(Clone, Debug, PartialEq)]
pub struct AfkClient {
    pub id: String,
    pub pid: u32,
    pub account_id: i64,
    /// Unix ms of its last press, else of reaching the game.
    pub since: i64,
}

/// A client and how often it wants a press, for `plan`.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    pub id: String,
    pub since: i64,
    pub interval_sec: u32,
}

/// What one tick does.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Nobody is due.
    Wait,
    /// Someone is due but the user is at the PC: again in 30 s.
    Retry,
    /// Press a key in these clients (ids).
    Press(Vec<String>),
}

/// Which clients are due (each `interval_sec` since its `since`), gated on the user having been
/// idle for 5 s (pure).
pub fn plan(clients: &[Planned], now: i64, idle_ms: u32) -> Plan {
    let due: Vec<String> = clients.iter().filter(|c| now - c.since >= i64::from(c.interval_sec) * 1000).map(|c| c.id.clone()).collect();
    if due.is_empty() {
        Plan::Wait
    } else if idle_ms < IDLE_MS {
        Plan::Retry
    } else {
        Plan::Press(due)
    }
}

/// Runs for the app's lifetime on its own thread (focus changes and `SendInput` block briefly).
pub fn spawn(app: AppHandle) {
    let started = std::thread::Builder::new().name("anti-afk".into()).spawn(move || {
        loop {
            sleep(tick(&app));
        }
    });
    if let Err(e) = started {
        log::warn!("anti-AFK not running: {e}");
    }
}

/// Each running client's `anti_afk` follows its account's setting; true when one changed.
fn mirror(s: &App, v: &Settings) -> bool {
    let mut changed = false;
    for i in s.tracker.list().into_iter().filter(|i| i.state != InstanceState::Exited) {
        let on = config_with(s, v, i.account_id).enabled;
        if i.anti_afk != on {
            changed |= s.tracker.set_anti_afk(&i.id, on);
        }
    }
    changed
}

/// One round; how long to wait before the next.
fn tick(app: &AppHandle) -> Duration {
    let s = app.state::<App>();
    let v = match s.db.settings() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("anti-AFK: could not read settings: {e}");
            return TICK;
        }
    };
    if mirror(&s, &v) {
        instances_changed(app);
    }
    if !v.anti_afk.enabled {
        return TICK;
    }
    let busy = s.macros.busy();
    // A hung client would block this one thread, and the others would get kicked for idling.
    let clients: Vec<(AfkClient, AfkClientConfig)> = s
        .tracker
        .afk_clients()
        .into_iter()
        .filter(|c| !busy.contains(&c.id) && !process::hung(c.pid))
        .map(|c| {
            let cfg = config_with(&s, &v, c.account_id);
            (c, cfg)
        })
        .filter(|(_, cfg)| cfg.enabled)
        .collect();
    let planned: Vec<Planned> = clients.iter().map(|(c, cfg)| Planned { id: c.id.clone(), since: c.since, interval_sec: cfg.interval_sec }).collect();
    let ids = match plan(&planned, now_ms(), idle_ms()) {
        Plan::Wait => return TICK,
        Plan::Retry => return RETRY,
        Plan::Press(ids) => ids,
    };
    let mut failed = false;
    // Our own SendInput moves GetLastInputInfo too, so "the user came back" is any input newer
    // than our last injection.
    let mut ours = last_input();
    for (c, cfg) in clients.iter().filter(|(c, _)| ids.contains(&c.id)) {
        if last_input() != ours {
            failed = true;
            break;
        }
        let Some(keys) = steps(&cfg.action) else {
            log::warn!("anti-AFK: account {} has an action it can't press: {:?}", c.account_id, cfg.action);
            continue;
        };
        let Some((hwnd, minimized)) = process::main_window(c.pid) else {
            log::debug!("anti-AFK: instance {} has no window", c.id);
            continue;
        };
        if minimized {
            window::unhide(c.pid, hwnd);
            sleep(RESTORED);
        }
        let r = press(hwnd, &keys);
        if minimized {
            window::hide(hwnd);
        }
        ours = last_input();
        match r {
            Pressed::Yes => {
                log::info!("anti-AFK: pressed {:?} in instance {} (minimized: {minimized})", cfg.action, c.id);
                s.tracker.mark_afk(&c.id, now_ms());
            }
            Pressed::No => {
                failed = true;
                log::info!("anti-AFK: instance {} couldn't be brought to the front", c.id);
            }
            Pressed::Interrupted => {
                failed = true;
                break;
            }
        }
        sleep(BETWEEN);
    }
    instances_changed(app);
    if failed { RETRY } else { TICK }
}

/// Tick count of the last keyboard or mouse input anywhere in the session (ours included);
/// None if it can't be read.
fn last_input() -> Option<u32> {
    let mut li = LASTINPUTINFO { cbSize: size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
    // SAFETY: `li` is a local with cbSize set, as GetLastInputInfo requires.
    unsafe { GetLastInputInfo(&mut li) }.as_bool().then_some(li.dwTime)
}

/// How long since the last keyboard or mouse input anywhere in the session.
fn idle_ms() -> u32 {
    let Some(t) = last_input() else {
        log::warn!("anti-AFK: GetLastInputInfo failed: {}", std::io::Error::last_os_error());
        return 0;
    };
    // SAFETY: no arguments; the tick count wraps the same way dwTime does.
    unsafe { GetTickCount() }.wrapping_sub(t)
}

enum Pressed {
    Yes,
    /// Couldn't get or keep the focus: no key was sent.
    No,
    /// The user (or the restore) needs the round to stop.
    Interrupted,
}

/// `keys` in `hwnd`, then the focus back where it was. Every key pressed is let go again, also
/// when the round stops halfway.
fn press(hwnd: HWND, keys: &[Step]) -> Pressed {
    // SAFETY: reads which window is in front.
    let prev = unsafe { GetForegroundWindow() };
    if !to_front(hwnd) {
        return Pressed::No;
    }
    // SAFETY: reads which window is in front.
    let in_front = || unsafe { GetForegroundWindow() } == hwnd;
    let mut sent = false;
    // Input other than ours while keys go in: the user is back.
    let mut user = false;
    let mut held: Vec<Key> = Vec::new();
    let mut ours = last_input();
    for step in keys {
        match *step {
            Step::Down(k) => {
                // Never into another window: only while the client is still the one in front.
                if !in_front() {
                    user = sent;
                    break;
                }
                send_key(k, false);
                held.push(k);
                sent = true;
                ours = last_input();
            }
            Step::Up(k) => {
                send_key(k, true);
                held.retain(|&h| h != k);
                ours = last_input();
            }
            Step::Wait(ms) => {
                sleep(Duration::from_millis(ms));
                if last_input() != ours {
                    user = true;
                    break;
                }
            }
        }
    }
    for k in held {
        send_key(k, true);
    }
    if !prev.is_invalid() && prev != hwnd && !to_front(prev) {
        log::warn!("anti-AFK: couldn't give the focus back to the previous window; stopping this round");
        return Pressed::Interrupted;
    }
    match (sent, user) {
        (_, true) => Pressed::Interrupted,
        (true, false) => Pressed::Yes,
        (false, false) => Pressed::No,
    }
}

/// Brings `hwnd` to the front: plainly; else attached to the input of the window in front
/// (whose thread may hand the focus over); else after a zero-movement mouse input, which makes
/// this process the last input source and so lifts Windows' foreground lock without any window
/// reacting (an ALT tap would open menus).
pub(super) fn to_front(hwnd: HWND) -> bool {
    // SAFETY: reads which window is in front.
    let front = || unsafe { GetForegroundWindow() } == hwnd;
    // SAFETY: a window-manager call on a top-level window handle; one that is gone just fails.
    if unsafe { SetForegroundWindow(hwnd) }.as_bool() && front() {
        return true;
    }
    // SAFETY: attaches this thread's input to the foreground window's thread only for these
    // calls and detaches it again; both thread ids are live (ours, the window's owner).
    unsafe {
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let me = GetCurrentThreadId();
        if fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool() {
            let _ = BringWindowToTop(hwnd);
            let _ = SetForegroundWindow(hwnd);
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
    if front() {
        return true;
    }
    let nudge = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx: 0, dy: 0, dwFlags: MOUSEEVENTF_MOVE, ..Default::default() } },
    };
    // SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires.
    if unsafe { SendInput(&[nudge], size_of::<INPUT>() as i32) } == 0 {
        log::debug!("anti-AFK: mouse nudge not sent: {}", std::io::Error::last_os_error());
    }
    // SAFETY: as the first SetForegroundWindow.
    let _ = unsafe { SetForegroundWindow(hwnd) };
    front()
}

/// One key going down or up, by scan code.
fn send_key(k: Key, up: bool) {
    let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
    if k.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: k.scan, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    // SAFETY: one fully initialised keyboard INPUT and its size, as SendInput requires.
    if unsafe { SendInput(&[input], size_of::<INPUT>() as i32) } == 0 {
        log::warn!("anti-AFK: SendInput sent no key: {}", std::io::Error::last_os_error());
    }
}

// ---------- commands ----------

/// The account's anti-AFK: its own, else the default from Settings.
#[tauri::command]
pub fn anti_afk_client_get(s: State<'_, App>, account_id: i64) -> AfkClientConfig {
    client_config(&s, account_id)
}

/// Sets the account's anti-AFK (kept across launches); its running clients follow at once.
#[tauri::command]
pub fn anti_afk_client_set(app: AppHandle, s: State<'_, App>, account_id: i64, config: AfkClientConfig) -> Result<AfkClientConfig> {
    set_client(&app, &s, account_id, config)?;
    Ok(client_config(&s, account_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn client(id: &str, since: i64, interval_sec: u32) -> Planned {
        Planned { id: id.into(), since, interval_sec }
    }

    #[test]
    fn presses_only_due_clients_while_the_user_is_away() {
        let min = 60_000;
        let every = |since| client("", since, 15 * 60);
        let clients = [Planned { id: "a".into(), ..every(0) }, Planned { id: "b".into(), ..every(10 * min) }, Planned { id: "c".into(), ..every(15 * min + 1) }];
        let ids = |v: &[&str]| Plan::Press(v.iter().map(|s| s.to_string()).collect());
        // Nobody due yet.
        assert_eq!(plan(&clients, 15 * min - 1, 60_000), Plan::Wait);
        assert_eq!(plan(&[], 99 * min, 60_000), Plan::Wait);
        // Due exactly at the interval.
        assert_eq!(plan(&clients, 15 * min, 60_000), ids(&["a"]));
        assert_eq!(plan(&clients, 25 * min, 5_000), ids(&["a", "b"]));
        // The user touched the keyboard or mouse within 5 s: later.
        assert_eq!(plan(&clients, 25 * min, 4_999), Plan::Retry);
        assert_eq!(plan(&clients, 25 * min, 0), Plan::Retry);
        // Nobody due wins over a busy user: nothing to retry.
        assert_eq!(plan(&clients, 5 * min, 0), Plan::Wait);
    }

    #[test]
    fn each_client_is_due_at_its_own_interval() {
        let min = 60_000;
        let clients = [client("fast", 0, 60), client("slow", 0, 1140), client("mid", 2 * min, 300)];
        let ids = |v: &[&str]| Plan::Press(v.iter().map(|s| s.to_string()).collect());
        assert_eq!(plan(&clients, min - 1, 10_000), Plan::Wait);
        assert_eq!(plan(&clients, min, 10_000), ids(&["fast"]));
        assert_eq!(plan(&clients, 7 * min, 10_000), ids(&["fast", "mid"]));
        assert_eq!(plan(&clients, 19 * min - 1, 10_000), ids(&["fast", "mid"]));
        assert_eq!(plan(&clients, 19 * min, 10_000), ids(&["fast", "slow", "mid"]));
    }

    #[test]
    fn actions_become_key_presses() {
        let space = [Step::Down(SPACE), Step::Wait(HOLD_MS), Step::Up(SPACE), Step::Wait(HOLD_MS)];
        assert_eq!(steps(&AfkAction::F13).unwrap(), [Step::Down(F13), Step::Wait(HOLD_MS), Step::Up(F13), Step::Wait(HOLD_MS)]);
        assert_eq!(steps(&AfkAction::Space).unwrap(), space);
        assert_eq!(steps(&AfkAction::Jump).unwrap(), space);
        let turn = steps(&AfkAction::JumpTurn).unwrap();
        assert_eq!(turn[..4], space);
        assert_eq!(turn[4..], [Step::Down(LEFT), Step::Wait(600), Step::Up(LEFT), Step::Wait(HOLD_MS)]);
        const { assert!(LEFT.extended && !SPACE.extended) };
        let w = key(0x11);
        assert_eq!(steps(&AfkAction::Key("w".into())).unwrap(), [Step::Down(w), Step::Wait(HOLD_MS), Step::Up(w), Step::Wait(HOLD_MS)]);
        assert_eq!(steps(&AfkAction::Key("Escape".into())), None);
        // Every key let go as often as it went down.
        for a in [AfkAction::F13, AfkAction::JumpTurn, AfkAction::Key("Up".into())] {
            let s = steps(&a).unwrap();
            let downs = s.iter().filter(|x| matches!(x, Step::Down(_))).count();
            assert_eq!(downs, s.iter().filter(|x| matches!(x, Step::Up(_))).count(), "{a:?}");
        }
    }

    #[test]
    fn only_listed_keys_and_intervals_pass() {
        assert_eq!(key_named(" f5 "), Some(("F5", key(0x3F))));
        assert_eq!(key_named("SPACE").map(|k| k.0), Some("Space"));
        assert_eq!(key_named("right"), Some(("Right", Key { scan: 0x4D, extended: true })));
        for bad in ["Esc", "Escape", "Alt", "LWin", "Enter", "/", "", "AA", "F25", "Ctrl"] {
            assert!(key_named(bad).is_none(), "{bad}");
        }
        // Names are unique, and each scan code is used once.
        for (i, (n, k)) in KEYS.iter().enumerate() {
            assert!(KEYS[i + 1..].iter().all(|(m, l)| !m.eq_ignore_ascii_case(n) && l != k), "{n}");
        }
        let cfg = |interval_sec, action| AfkClientConfig { enabled: true, interval_sec, action };
        assert_eq!(check(cfg(60, AfkAction::Key("w".into()))).unwrap().action, AfkAction::Key("W".into()));
        assert!(check(cfg(1140, AfkAction::JumpTurn)).is_ok());
        for bad in [cfg(59, AfkAction::F13), cfg(1141, AfkAction::F13), cfg(0, AfkAction::F13), cfg(300, AfkAction::Key("Esc".into()))] {
            assert!(matches!(check(bad), Err(AppError::Invalid(_))));
        }
    }

    #[test]
    fn the_default_follows_the_global_interval() {
        let mut v = Settings::default();
        v.anti_afk.interval_min = 15;
        assert_eq!(default_config(&v), AfkClientConfig { enabled: true, interval_sec: 900, action: AfkAction::F13 });
        v.anti_afk.interval_min = 0;
        assert_eq!(default_config(&v).interval_sec, 60);
        v.anti_afk.interval_min = 40;
        assert_eq!(default_config(&v).interval_sec, 1140);
    }
}
