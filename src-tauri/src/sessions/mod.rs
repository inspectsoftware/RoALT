//! Background sessions (AltDock): Roblox clients run in other Windows users' sessions, opened
//! over loopback RDP (RDP Wrapper on this PC) in a hidden mstsc window, so anti-AFK and macros
//! there never take the user's focus. The AltDock in each session serves the control API
//! (Stream B); this side checks the PC (`health`), sets it up once elevated (`setup`), and opens,
//! shows, hides and disconnects the sessions.

pub mod health;
pub mod rdp;
pub mod setup;
pub mod types;

#[cfg(windows)]
pub mod commands;
#[cfg(windows)]
pub mod creds;
#[cfg(windows)]
mod reg;
#[cfg(windows)]
pub mod users;
#[cfg(windows)]
pub mod window;
#[cfg(windows)]
pub mod wts;
// mod remote; // TODO(merge): needs Stream B (`control::client::ControlClient`)

use std::path::PathBuf;

use crate::error::{AppError, Result};
use types::{SessionConfig, SessionMode};

/// Alt Windows users are `altdock-1`, `altdock-2`, ...
pub const USER_PREFIX: &str = "altdock-";
/// Session n's control API listens on `PORT_BASE + n`.
pub const PORT_BASE: u16 = 47610;
/// Loopback addresses `127.0.0.2..` and ports stay well inside their ranges.
pub const MAX_SESSIONS: u32 = 32;
/// The shared program copy's exe name.
pub const APP_EXE: &str = "altdock.exe";
/// HKLM Run value that starts AltDock in each alt user's session.
pub const RUN_VALUE: &str = "AltDockSession";
const MAX_NAME: usize = 32;

/// `%ProgramData%\AltDock`: the program copy, `users.json` and the control configs.
pub fn program_data() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| r"C:\ProgramData".into()).join("AltDock")
}

/// `%ProgramData%\AltDock\app\altdock.exe`.
pub fn app_exe() -> PathBuf {
    program_data().join("app").join(APP_EXE)
}

/// The autostart command line (HKLM Run `AltDockSession`).
pub fn run_command() -> String {
    format!("\"{}\" --session", app_exe().display())
}

/// The vault AAD (and settings-table key) of a session's secret: `password` or `token`.
pub fn secret_key(name: &str, kind: &str) -> String {
    format!("altdock.session.{name}.{kind}")
}

/// Session n's alt user, loopback address and control port (pure).
pub fn slot(n: u32) -> (String, String, u16) {
    (format!("{USER_PREFIX}{n}"), format!("127.0.0.{}", n + 1), PORT_BASE + n as u16)
}

/// The slot number of an alt user name (`altdock-3` → 3).
fn slot_of(user: &str) -> Option<u32> {
    user.strip_prefix(USER_PREFIX)?.parse().ok().filter(|n| (1..=MAX_SESSIONS).contains(n))
}

/// The lowest slot no session uses yet (pure). None when all are taken.
pub fn free_slot(sessions: &[SessionConfig]) -> Option<u32> {
    (1..=MAX_SESSIONS).find(|n| !sessions.iter().any(|s| slot_of(&s.windows_user) == Some(*n)))
}

/// Checks `c` and gives it its slot: a new session (no session of that name in `sessions`) gets
/// the lowest free one; an existing one keeps its own whatever `c` says (pure).
pub fn checked(mut c: SessionConfig, sessions: &[SessionConfig]) -> Result<SessionConfig> {
    let invalid = |m: String| Err(AppError::Invalid(m));
    c.name = c.name.trim().to_owned();
    if c.name.is_empty() || c.name.chars().count() > MAX_NAME {
        return invalid(tr!("A session name needs 1 to {max} characters.", max = MAX_NAME));
    }
    if !c.name.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == ' ') {
        return invalid(tr!("A session name may only have letters, digits, spaces, - and _."));
    }
    if !(640..=3840).contains(&c.width) || !(480..=2160).contains(&c.height) {
        return invalid(tr!("The session size must be 640×480 to 3840×2160."));
    }
    let mut seen = std::collections::HashSet::new();
    c.account_ids.retain(|id| seen.insert(*id));
    if c.mode == SessionMode::Dedicated && c.account_ids.len() > 1 {
        return invalid(tr!("A dedicated session runs one account."));
    }
    let n = match sessions.iter().find(|s| s.name.eq_ignore_ascii_case(&c.name)) {
        Some(old) => {
            c.name = old.name.clone();
            slot_of(&old.windows_user).ok_or_else(|| AppError::Internal(format!("session {} has no slot", old.name)))?
        }
        None => free_slot(sessions).ok_or_else(|| AppError::Invalid(tr!("All {max} sessions are in use.", max = MAX_SESSIONS)))?,
    };
    (c.windows_user, c.address, c.port) = slot(n);
    Ok(c)
}

/// A whole session list as settings take it (pure): sessions already in `current` keep their
/// slots, new ones get the lowest slots the list leaves free.
pub fn checked_list(list: Vec<SessionConfig>, current: &[SessionConfig]) -> Result<Vec<SessionConfig>> {
    let kept: Vec<SessionConfig> =
        current.iter().filter(|s| list.iter().any(|c| c.name.trim().eq_ignore_ascii_case(&s.name))).cloned().collect();
    let mut out: Vec<SessionConfig> = Vec::new();
    for c in list {
        if out.iter().any(|o| o.name.eq_ignore_ascii_case(c.name.trim())) {
            return Err(AppError::Invalid(tr!("Two sessions can't have the same name.")));
        }
        let known: Vec<SessionConfig> = kept.iter().chain(&out).cloned().collect();
        out.push(checked(c, &known)?);
    }
    Ok(out)
}

/// A 24-character alt user password from `bytes` (pure; 29 random bytes): letters, digits and
/// symbols, with each kind present so Windows' complexity rules accept it.
pub fn password_from(bytes: &[u8]) -> String {
    const SETS: [&[u8]; 4] = [b"ABCDEFGHJKLMNPQRSTUVWXYZ", b"abcdefghijkmnopqrstuvwxyz", b"23456789", b"!#%+-=?@_"];
    const ALL: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!#%+-=?@_";
    let mut out: Vec<u8> = bytes.iter().take(24).map(|b| ALL[*b as usize % ALL.len()]).collect();
    // The first four characters come one from each kind; their places depend on the bytes too.
    for (i, set) in SETS.iter().enumerate() {
        let b = bytes.get(24 + i).copied().unwrap_or(0) as usize;
        out[i] = set[b % set.len()];
    }
    let shift = bytes.get(28).copied().unwrap_or(0) as usize % 24;
    out.rotate_left(shift);
    String::from_utf8(out).unwrap_or_default()
}

/// `n` bytes from the OS random number generator.
pub fn random(n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    getrandom::fill(&mut b).expect("OS random number generator failed");
    b
}

/// A fresh alt user password.
pub fn new_password() -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(password_from(&zeroize::Zeroizing::new(random(29))))
}

/// A fresh control token: 64 hex characters.
pub fn new_token() -> zeroize::Zeroizing<String> {
    zeroize::Zeroizing::new(random(32).iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(name: &str) -> SessionConfig {
        SessionConfig {
            name: name.into(),
            windows_user: String::new(),
            address: String::new(),
            port: 0,
            mode: SessionMode::Shared,
            account_ids: vec![1, 2],
            width: 1280,
            height: 720,
        }
    }

    #[test]
    fn slots_are_allocated_lowest_first_and_kept() {
        assert_eq!(slot(1), ("altdock-1".into(), "127.0.0.2".into(), 47611));
        assert_eq!(slot(3), ("altdock-3".into(), "127.0.0.4".into(), 47613));
        let a = checked(cfg("Farm"), &[]).unwrap();
        assert_eq!((a.windows_user.as_str(), a.address.as_str(), a.port), ("altdock-1", "127.0.0.2", 47611));
        let b = checked(cfg("Macro"), std::slice::from_ref(&a)).unwrap();
        assert_eq!(b.windows_user, "altdock-2");
        // Saving again keeps the slot, whatever the UI sent.
        let mut again = cfg("farm");
        again.windows_user = "altdock-9".into();
        again.port = 1;
        let again = checked(again, &[a.clone(), b.clone()]).unwrap();
        assert_eq!((again.name.as_str(), again.windows_user.as_str(), again.port), ("Farm", "altdock-1", 47611));
        // A freed slot is used again.
        assert_eq!(free_slot(std::slice::from_ref(&b)), Some(1));
        let full: Vec<SessionConfig> = (1..=MAX_SESSIONS).map(|n| SessionConfig { windows_user: slot(n).0, ..cfg(&n.to_string()) }).collect();
        assert_eq!(free_slot(&full), None);
        assert!(checked(cfg("one more"), &full).is_err());
    }

    #[test]
    fn session_configs_are_checked() {
        assert!(checked(cfg(""), &[]).is_err());
        assert!(checked(cfg("   "), &[]).is_err());
        assert!(checked(cfg("a/b"), &[]).is_err());
        assert!(checked(cfg(&"x".repeat(33)), &[]).is_err());
        assert!(checked(SessionConfig { width: 100, ..cfg("a") }, &[]).is_err());
        assert!(checked(SessionConfig { height: 4000, ..cfg("a") }, &[]).is_err());
        assert!(checked(SessionConfig { mode: SessionMode::Dedicated, ..cfg("a") }, &[]).is_err());
        assert!(checked(SessionConfig { mode: SessionMode::Dedicated, account_ids: vec![4], ..cfg("a") }, &[]).is_ok());
        assert_eq!(checked(cfg("  Farm 1 "), &[]).unwrap().name, "Farm 1");
    }

    #[test]
    fn a_session_list_keeps_slots_and_refuses_twins() {
        let a = checked(cfg("A"), &[]).unwrap();
        let b = checked(cfg("B"), std::slice::from_ref(&a)).unwrap();
        let list = checked_list(vec![cfg("B"), cfg("C")], &[a.clone(), b.clone()]).unwrap();
        assert_eq!(list[0].windows_user, "altdock-2");
        assert_eq!(list[1].windows_user, "altdock-1", "A was dropped, its slot is free");
        assert!(checked_list(vec![cfg("A"), cfg("a")], &[]).is_err());
    }

    #[test]
    fn passwords_have_every_kind_of_character() {
        for seed in 0..=255u8 {
            let bytes: Vec<u8> = (0..29).map(|i| seed.wrapping_mul(31).wrapping_add(i * 7)).collect();
            let p = password_from(&bytes);
            assert_eq!(p.len(), 24);
            assert!(p.bytes().any(|c| c.is_ascii_uppercase()), "{p}");
            assert!(p.bytes().any(|c| c.is_ascii_lowercase()), "{p}");
            assert!(p.bytes().any(|c| c.is_ascii_digit()), "{p}");
            assert!(p.bytes().any(|c| !c.is_ascii_alphanumeric()), "{p}");
            assert!(!p.contains(['"', ' ', '\\', '\'']), "{p}");
        }
        assert_ne!(*new_password(), *new_password());
        let t = new_token();
        assert!(t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn settings_from_before_sessions_still_load() {
        let old = r#"{"theme":"dark","autoLockMinutes":30}"#;
        let s: crate::model::Settings = serde_json::from_str(old).unwrap();
        assert!(s.sessions.is_empty());
        assert_eq!(s.auto_lock_minutes, 30);
        let mut s = s;
        s.sessions.push(checked(cfg("Farm"), &[]).unwrap());
        let back: crate::model::Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.sessions, s.sessions);
    }
}
