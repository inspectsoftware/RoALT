//! Finding and starting the Roblox client (Windows).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use windows::Win32::System::Registry::{HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::core::{HSTRING, PCWSTR};

use crate::error::{AppError, Result};
use crate::vault::Secret;

const PLAYER: &str = "RobloxPlayerBeta.exe";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientExe {
    pub path: PathBuf,
    /// The protocol handler is a bootstrapper (Bloxstrap, Fishstrap, ...), not RobloxPlayerBeta.exe.
    pub bootstrapper: bool,
}

/// Where Roblox records its install: per user (HKCU) or for all users (HKLM, 64- and 32-bit views).
const ENVIRONMENTS: [(HKEY, &str); 3] = [
    (HKEY_CURRENT_USER, r"Software\ROBLOX Corporation\Environments\roblox-player"),
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\ROBLOX Corporation\Environments\roblox-player"),
    (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\ROBLOX Corporation\Environments\roblox-player"),
];

/// The first that exists of:
/// 1. Roblox's own `clientExe` (`ENVIRONMENTS`, quotes trimmed);
/// 2. the `roblox-player` protocol handler (`HKCU\Software\Classes\roblox-player\shell\open\command`,
///    then HKCR), parsed out of its command line (how a bootstrapper is found);
/// 3. `Versions\<version>\RobloxPlayerBeta.exe` from the registry's `version`;
/// 4. the newest `RobloxPlayerBeta.exe` under a `Roblox\Versions` folder (`%LOCALAPPDATA%`,
///    Program Files (x86), Program Files): the registry can still name a version folder Roblox
///    deleted when it updated itself.
///
/// `bootstrapper` = its file name isn't RobloxPlayerBeta.exe. Nothing found → `AppError::Invalid`.
pub fn find() -> Result<ClientExe> {
    let own = ENVIRONMENTS.into_iter().filter_map(|(root, key)| reg_string(root, key, Some("clientExe")).map(unquote));
    let named = ENVIRONMENTS
        .into_iter()
        .filter_map(|(root, key)| reg_string(root, key, Some("version")))
        .flat_map(|v| versions_dirs().into_iter().map(move |d| d.join(&v).join(PLAYER)));
    let path = own
        .chain(handlers())
        .chain(named)
        .find(|p| p.is_file() && !in_store(p))
        .or_else(|| newest(versions_dirs().iter().flat_map(|d| players_in(d)).filter(|(p, _)| !in_store(p))));
    let path = path.ok_or_else(|| {
        log::warn!("no Roblox client: none in the registry (clientExe, roblox-player handler, version) or under {:?} (Store builds excluded)", versions_dirs());
        AppError::Invalid(tr!(
            "Roblox isn't installed. Install it from roblox.com, then try again. (The Microsoft Store version can't be used: install the one from roblox.com.)"
        ))
    })?;
    Ok(ClientExe { bootstrapper: !is_player(&path), path })
}

/// The `roblox-player` protocol handlers' programs (HKCU, then HKCR). Roblox's own launcher or
/// installer there (older installs) stands for the RobloxPlayerBeta.exe beside it, so it is never
/// taken for a bootstrapper.
fn handlers() -> impl Iterator<Item = PathBuf> {
    [
        (HKEY_CURRENT_USER, r"Software\Classes\roblox-player\shell\open\command"),
        (HKEY_CLASSES_ROOT, r"roblox-player\shell\open\command"),
    ]
    .into_iter()
    .filter_map(|(root, key)| handler_exe(&reg_string(root, key, None)?))
    .map(|h| {
        let player = h.with_file_name(PLAYER);
        if is_roblox_launcher(&h) && player.is_file() { player } else { h }
    })
}

/// RobloxPlayerLauncher.exe or RobloxPlayerInstaller.exe: Roblox's, not a bootstrapper (pure).
fn is_roblox_launcher(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| ["RobloxPlayerLauncher.exe", "RobloxPlayerInstaller.exe"].iter().any(|r| n.eq_ignore_ascii_case(r)))
}

/// The Microsoft Store build (under `WindowsApps`), which can't be started with a launch URI (pure).
fn in_store(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str().eq_ignore_ascii_case("WindowsApps"))
}

/// The `Versions` folders clients live in: Roblox's own (per user first), then Bloxstrap's and
/// Fishstrap's copies.
fn versions_dirs() -> Vec<PathBuf> {
    let roblox = ["LOCALAPPDATA", "ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(std::env::var_os).map(|d| PathBuf::from(d).join("Roblox"));
    let strap = std::env::var_os("LOCALAPPDATA").into_iter().flat_map(|d| ["Bloxstrap", "Fishstrap"].map(|b| PathBuf::from(&d).join(b)));
    roblox.chain(strap).map(|d| d.join("Versions")).collect()
}

/// Every `<dir>\<version>\RobloxPlayerBeta.exe` with its last-write time.
fn players_in(dir: &Path) -> Vec<(PathBuf, SystemTime)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    entries
        .filter_map(|e| {
            let exe = e.ok()?.path().join(PLAYER);
            let modified = std::fs::metadata(&exe).ok()?.modified().ok()?;
            Some((exe, modified))
        })
        .collect()
}

/// The most recently written (pure).
fn newest(found: impl IntoIterator<Item = (PathBuf, SystemTime)>) -> Option<PathBuf> {
    found.into_iter().max_by_key(|(_, t)| *t).map(|(p, _)| p)
}

/// A registry path without the quotes some installs keep around it (pure).
fn unquote(s: String) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"'))
}

/// Starts the client with the URI as its first argument, then `extra` (`launcher::launch_args`),
/// all raw: no quoting, none contains spaces or quotes. In its own folder; the process handle
/// is closed at once (Roblox's anti-cheat has flagged long-held handles). Returns the PID and
/// its start time, read while our handle still reserves the PID (0: already gone). The URI
/// and the arguments are never logged.
pub fn start(exe: &ClientExe, uri: &Secret, extra: &[String]) -> Result<(u32, u64)> {
    let dir = exe.path.parent().ok_or_else(|| AppError::Internal(tr!("Roblox's folder is unknown.")))?;
    let mut cmd = Command::new(&exe.path);
    cmd.raw_arg(uri.expose());
    for a in extra {
        cmd.raw_arg(a);
    }
    let child = cmd
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| AppError::Internal(tr!("Could not start Roblox: {why}", why = e)))?;
    let started = crate::instances::process::start_time(child.id()).unwrap_or_else(|| {
        log::warn!("Roblox client pid {} start time unreadable; tracking it without one", child.id());
        0
    });
    // Dropping the Child closes its process and thread handles; the process keeps running.
    Ok((child.id(), started))
}

/// The program of a `shell\open\command` line: `"C:\...\RobloxPlayerBeta.exe" %1`,
/// `"C:\...\Bloxstrap.exe" -player "%1"`, or unquoted up to `.exe`.
fn handler_exe(command: &str) -> Option<PathBuf> {
    let command = command.trim();
    let exe = match command.strip_prefix('"') {
        Some(rest) => rest.split_once('"')?.0,
        None => match command.to_ascii_lowercase().find(".exe") {
            Some(i) => &command[..i + 4],
            None => command.split_whitespace().next().unwrap_or(""),
        },
    };
    (!exe.is_empty()).then(|| PathBuf::from(exe))
}

/// A bootstrapper owns this install, so Roblox's installer must not run over it: `exe` is a
/// bootstrapper, sits under a `Bloxstrap`/`Fishstrap` folder (they set `clientExe` to their own
/// `Versions` copy), or the `roblox-player` protocol handler isn't RobloxPlayerBeta.exe.
pub fn managed_by_bootstrapper(exe: &ClientExe) -> bool {
    let handler = handlers().find(|p| p.is_file());
    exe.bootstrapper || in_bootstrapper_folder(&exe.path) || handler.is_some_and(|h| !is_player(&h))
}

fn in_bootstrapper_folder(path: &Path) -> bool {
    path.components().any(|c| ["bloxstrap", "fishstrap"].iter().any(|b| c.as_os_str().eq_ignore_ascii_case(b)))
}

fn is_player(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(PLAYER))
}

/// A `REG_SZ` (or expanded `REG_EXPAND_SZ`) value; `None` names the key's default value.
fn reg_string(root: HKEY, key: &str, value: Option<&str>) -> Option<String> {
    let (key, value) = (HSTRING::from(key), value.map(HSTRING::from));
    let name = value.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr()));
    let mut len = 0u32;
    // SAFETY: `key` and `name` are live NUL-terminated strings; this call only writes `len`.
    unsafe { RegGetValueW(root, &key, name, RRF_RT_REG_SZ, None, None, Some(&mut len)) }.ok().ok()?;
    let mut buf = vec![0u16; (len as usize).div_ceil(2)];
    // SAFETY: `buf` holds at least `len` bytes, the most the call writes.
    unsafe { RegGetValueW(root, &key, name, RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }
        .ok()
        .ok()?;
    buf.truncate(len as usize / 2);
    let s = String::from_utf16_lossy(&buf);
    Some(s.trim_end_matches('\0').to_owned()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    #[test]
    fn handler_command_lines() {
        let exe = |c: &str| handler_exe(c).map(|p| p.to_string_lossy().into_owned());
        let roblox = r"C:\Users\you\AppData\Local\Roblox\Versions\version-9ac8595712b447a7\RobloxPlayerBeta.exe";
        assert_eq!(exe(&format!("\"{roblox}\" %1")).as_deref(), Some(roblox));
        let bloxstrap = r"C:\Users\you\AppData\Local\Bloxstrap\Bloxstrap.exe";
        assert_eq!(exe(&format!("  \"{bloxstrap}\" -player \"%1\"")).as_deref(), Some(bloxstrap));
        let fishstrap = r"C:\Program Files\Fishstrap\Fishstrap.EXE";
        assert_eq!(exe(&format!("{fishstrap} -player %1")).as_deref(), Some(fishstrap));
        assert_eq!(exe(r"C:\x\launcher %1").as_deref(), Some(r"C:\x\launcher"));
        for bad in ["", "   ", "\"", "\"\" %1", "\"C:\\no-closing-quote.exe %1"] {
            assert_eq!(exe(bad), None, "{bad}");
        }

        assert!(is_player(Path::new(roblox)) && is_player(Path::new(r"D:\robloxplayerbeta.EXE")));
        assert!(!is_player(Path::new(bloxstrap)) && !is_player(Path::new(r"C:\RobloxPlayerInstaller.exe")));

        assert!(in_bootstrapper_folder(Path::new(r"C:\Users\you\AppData\Local\Bloxstrap\Versions\version-9ac8595712b447a7\RobloxPlayerBeta.exe")));
        assert!(in_bootstrapper_folder(Path::new(r"D:\FISHSTRAP\Versions\x\RobloxPlayerBeta.exe")));
        assert!(!in_bootstrapper_folder(Path::new(roblox)));
    }

    #[test]
    fn picks_the_newest_client_and_trims_quotes() {
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let found = vec![(PathBuf::from("old"), t(10)), (PathBuf::from("new"), t(30)), (PathBuf::from("mid"), t(20))];
        assert_eq!(newest(found), Some(PathBuf::from("new")));
        assert_eq!(newest(Vec::new()), None);
        let exe = r"C:\R\RobloxPlayerBeta.exe";
        assert_eq!(unquote(format!(" \"{exe}\" ")), PathBuf::from(exe));
        assert_eq!(unquote(exe.into()), PathBuf::from(exe));

        assert!(is_roblox_launcher(Path::new(r"C:\R\Versions\v\RobloxPlayerLauncher.exe")));
        assert!(is_roblox_launcher(Path::new(r"C:\R\Versions\v\robloxplayerinstaller.EXE")));
        assert!(!is_roblox_launcher(Path::new(r"C:\B\Bloxstrap.exe")) && !is_roblox_launcher(Path::new(exe)));
        assert!(in_store(Path::new(r"C:\Program Files\WindowsApps\ROBLOXCORPORATION.ROBLOX_2.6_x64\Windows10Universal.exe")));
        assert!(!in_store(Path::new(exe)));
    }

    #[test]
    fn finds_clients_in_a_versions_folder() {
        let dir = std::env::temp_dir().join(format!("roalt-test-{}", uuid::Uuid::new_v4()));
        for v in ["version-a", "version-b", "not-a-client"] {
            std::fs::create_dir_all(dir.join(v)).unwrap();
        }
        std::fs::write(dir.join("version-a").join(PLAYER), b"").unwrap();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(dir.join("version-b").join(PLAYER), b"").unwrap();
        let found = players_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(found.len(), 2, "the folder without a client is skipped");
        assert_eq!(newest(found), Some(dir.join("version-b").join(PLAYER)));
        assert!(players_in(&dir.join("missing")).is_empty());
    }

    #[test]
    fn missing_registry_values_are_none() {
        assert_eq!(reg_string(HKEY_CURRENT_USER, r"Software\RoALT-test-no-such-key", None), None);
        assert_eq!(reg_string(HKEY_CURRENT_USER, r"Software\Classes", Some("RoALT-test-no-such-value")), None);
    }

    /// `start` on a harmless system program (never the Roblox client): the raw argument
    /// reaches it untouched, quotes and all, it runs in its own folder, and a PID comes back.
    #[test]
    fn start_passes_the_raw_argument_in_the_exe_folder() {
        let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        let exe = ClientExe { path: system.join(r"System32\cmd.exe"), bootstrapper: true };
        let out = std::env::temp_dir().join(format!("roalt-test-{}.txt", uuid::Uuid::new_v4()));
        // Kept alive a second so its start time is read before it exits. The extra arguments
        // follow the first one, space-separated.
        let arg = Secret::new(format!("/c cd > \"{}\" & ping -n", out.display()));

        let (pid, started) = start(&exe, &arg, &["2".into(), "127.0.0.1".into(), ">nul".into()]).unwrap();
        assert!(pid > 0 && started > 0);
        let deadline = Instant::now() + Duration::from_secs(10);
        let written = loop {
            match std::fs::read_to_string(&out) {
                Ok(s) if s.ends_with('\n') => break s,
                _ if Instant::now() > deadline => panic!("cmd.exe never wrote {}", out.display()),
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        };
        let _ = std::fs::remove_file(&out);
        assert!(written.trim().eq_ignore_ascii_case(&system.join("System32").to_string_lossy()), "{written}");

        let missing = ClientExe { path: system.join(r"System32\roalt-no-such.exe"), bootstrapper: false };
        assert!(matches!(start(&missing, &arg, &[]), Err(AppError::Internal(_))));
    }
}
