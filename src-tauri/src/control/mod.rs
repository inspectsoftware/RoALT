//! Session mode and its control API. AltDock runs in an alt Windows user's session (a loopback
//! RDP session) in **session mode** when that user has a control config,
//! `%ProgramData%\AltDock\control\<user>.json` (`{ "port", "token", "mode" }`, written by the
//! main app's elevated setup): hidden in the tray, multi-instance on, a DPAPI vault, and the
//! control API (`server`) on `127.0.0.1:<port>`. The main app talks to it with `client`.
//!
//! `--session` is how Windows starts it at every sign-in (HKLM Run, so for every user): then it
//! quits at once unless the user is one AltDock manages (`users.json`).

pub mod client;
pub mod server;
pub mod types;

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;
use zeroize::Zeroizing;

pub use types::ControlMode;

/// The header every control request carries the session's token in.
pub const TOKEN_HEADER: &str = "x-altdock-token";
/// The command-line flag of the autostart.
pub const SESSION_ARG: &str = "--session";

/// A session's control config, checked (`parse_config`).
#[derive(Deserialize)]
pub struct ControlConfig {
    pub port: u16,
    /// 64 hex digits (32 random bytes).
    pub token: Zeroizing<String>,
    pub mode: ControlMode,
}

impl std::fmt::Debug for ControlConfig {
    /// Never the token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ControlConfig").field("port", &self.port).field("mode", &self.mode).finish_non_exhaustive()
    }
}

/// `{ "users": [...] }`: the alt users AltDock manages.
#[derive(Deserialize)]
struct Users {
    users: Vec<String>,
}

/// Set once by `main` in session mode.
static SESSION: OnceLock<ControlConfig> = OnceLock::new();

/// This start's control config while in session mode.
pub fn session() -> Option<&'static ControlConfig> {
    SESSION.get()
}

pub fn set_session(config: ControlConfig) {
    let _ = SESSION.set(config);
}

/// How this start goes (`start_mode`).
#[derive(Debug)]
pub enum Start {
    Normal,
    Session(ControlConfig),
    /// `--session` for a user AltDock doesn't manage: nothing to do here.
    Quit,
    /// Session mode, but its config can't be used: why (English, for the log).
    Broken(String),
}

/// `%ProgramData%\AltDock`.
pub fn program_dir() -> Option<PathBuf> {
    std::env::var_os("ProgramData").filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("AltDock"))
}

/// The signed-in Windows user, as `USERNAME` has it.
pub fn windows_user() -> String {
    std::env::var("USERNAME").unwrap_or_default()
}

/// How this process starts: `session_arg` (`--session` given), `dir` (`program_dir`), `user`
/// (`windows_user`); `read` reads a file, None when it can't (pure but for `read`).
pub fn start_mode(session_arg: bool, dir: Option<&Path>, user: &str, read: impl Fn(&Path) -> Option<String>) -> Start {
    let off = if session_arg { Start::Quit } else { Start::Normal };
    // A name that could leave the control folder is no user of ours.
    let Some(dir) = dir.filter(|_| !user.is_empty() && !user.contains(['\\', '/', ':'])) else { return off };
    if session_arg && !read(&dir.join("users.json")).is_some_and(|j| listed(&j, user)) {
        return Start::Quit;
    }
    match read(&dir.join("control").join(format!("{user}.json"))) {
        Some(json) => match parse_config(&json) {
            Ok(c) => Start::Session(c),
            Err(why) => Start::Broken(why),
        },
        None if session_arg => Start::Broken(format!("no control config for {user}")),
        None => Start::Normal,
    }
}

/// `user` is in `users.json` (Windows user names ignore case; unreadable JSON lists no one).
fn listed(users_json: &str, user: &str) -> bool {
    serde_json::from_str::<Users>(users_json).is_ok_and(|u| u.users.iter().any(|n| n.to_lowercase() == user.to_lowercase()))
}

/// A control config, checked: port 1024..=65535, token 64 hex digits, mode shared or dedicated.
/// Err: why (English; never quotes the token).
pub fn parse_config(json: &str) -> Result<ControlConfig, String> {
    let c: ControlConfig = serde_json::from_str(json).map_err(|e| format!("control config unreadable ({:?} at line {})", e.classify(), e.line()))?;
    if c.port < 1024 {
        return Err(format!("control config: port {} is below 1024", c.port));
    }
    if c.token.len() != 64 || !c.token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("control config: the token isn't 64 hex digits".into());
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789ABCDEF0123456789abcdef0123456789abcdef";

    fn config(port: u32, token: &str, mode: &str) -> String {
        format!(r#"{{"port":{port},"token":"{token}","mode":"{mode}"}}"#)
    }

    #[test]
    fn checks_configs() {
        let c = parse_config(&config(47611, TOKEN, "shared")).unwrap();
        assert_eq!((c.port, c.token.as_str(), c.mode), (47611, TOKEN, ControlMode::Shared));
        assert_eq!(parse_config(&config(65535, TOKEN, "dedicated")).unwrap().mode, ControlMode::Dedicated);
        assert_eq!(parse_config(&config(1024, TOKEN, "shared")).unwrap().port, 1024);
        assert!(!format!("{c:?}").contains(TOKEN));

        for bad in [
            config(1023, TOKEN, "shared"),
            config(0, TOKEN, "shared"),
            config(65536, TOKEN, "shared"),
            config(47611, &TOKEN[1..], "shared"),
            config(47611, &format!("{TOKEN}0"), "shared"),
            config(47611, &TOKEN.replace('a', "g"), "shared"),
            config(47611, "", "shared"),
            config(47611, TOKEN, "both"),
            r#"{"port":47611,"token":"x"}"#.into(),
            r#"{"port":"47611","mode":"shared"}"#.into(),
            "".into(),
            "[]".into(),
        ] {
            let err = parse_config(&bad).unwrap_err();
            assert!(!err.contains(TOKEN), "{err}");
        }
    }

    #[test]
    fn picks_the_start() {
        let dir = Path::new(r"C:\ProgramData\AltDock");
        let files = |users: Option<&'static str>, cfg: Option<String>| {
            move |p: &Path| {
                if p == dir.join("users.json") {
                    users.map(str::to_owned)
                } else if p == dir.join("control").join("altdock-1.json") {
                    cfg.clone()
                } else {
                    None
                }
            }
        };
        let users = Some(r#"{"users":["altdock-1","altdock-2"]}"#);
        let good = Some(config(47611, TOKEN, "shared"));
        let is = |s: Start, want: &str| assert_eq!(format!("{s:?}").split('(').next().unwrap(), want, "{s:?}");

        // The main user: no config, and not listed.
        is(start_mode(false, Some(dir), "dario", files(users, None)), "Normal");
        is(start_mode(true, Some(dir), "dario", files(users, None)), "Quit");
        // A managed alt user.
        is(start_mode(true, Some(dir), "altdock-1", files(users, good.clone())), "Session");
        is(start_mode(true, Some(dir), "altdock-1", files(users, None)), "Broken");
        assert!(listed(r#"{"users":["AltDock-1"]}"#, "altdock-1"));
        is(start_mode(false, Some(dir), "altdock-1", files(None, good.clone())), "Session");
        is(start_mode(true, Some(dir), "altdock-1", files(users, Some(config(80, TOKEN, "shared")))), "Broken");
        is(start_mode(false, Some(dir), "altdock-1", files(users, Some("{".into()))), "Broken");
        // `--session` needs the user listed, config or not.
        is(start_mode(true, Some(dir), "altdock-1", files(None, good.clone())), "Quit");
        is(start_mode(true, Some(dir), "altdock-1", files(Some(r#"{"users":["altdock-2"]}"#), good.clone())), "Quit");
        is(start_mode(true, Some(dir), "altdock-1", files(Some("nope"), good.clone())), "Quit");
        // No %ProgramData%, or a user name that isn't one.
        is(start_mode(true, None, "altdock-1", files(users, good.clone())), "Quit");
        is(start_mode(false, None, "altdock-1", files(users, good.clone())), "Normal");
        for odd in ["", r"..\altdock-1", "a/b", "c:"] {
            is(start_mode(true, Some(dir), odd, files(users, good.clone())), "Quit");
            is(start_mode(false, Some(dir), odd, files(users, good.clone())), "Normal");
        }
    }
}
