//! Finding and starting the Roblox client (Windows).

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use windows::Win32::System::Registry::{HKEY, HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, RRF_RT_REG_SZ, RegGetValueW};
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

/// Roblox's own client: `HKCU\Software\ROBLOX Corporation\Environments\roblox-player` value
/// `clientExe`, if that file exists; else the `roblox-player` protocol handler
/// (`HKCU\Software\Classes\roblox-player\shell\open\command`, then HKCR), parsed out of its
/// command line; `bootstrapper` = its file name isn't RobloxPlayerBeta.exe.
/// Nothing found → `AppError::Invalid("Roblox isn't installed. Install it from roblox.com, then try again.")`.
pub fn find() -> Result<ClientExe> {
    let own = reg_string(HKEY_CURRENT_USER, r"Software\ROBLOX Corporation\Environments\roblox-player", Some("clientExe"));
    let handlers = [
        (HKEY_CURRENT_USER, r"Software\Classes\roblox-player\shell\open\command"),
        (HKEY_CLASSES_ROOT, r"roblox-player\shell\open\command"),
    ];
    let handler = handlers.into_iter().filter_map(|(root, key)| handler_exe(&reg_string(root, key, None)?));
    let path = own.map(PathBuf::from).into_iter().chain(handler).find(|p| p.is_file());
    let path = path.ok_or_else(|| AppError::Invalid("Roblox isn't installed. Install it from roblox.com, then try again.".into()))?;
    Ok(ClientExe { bootstrapper: !is_player(&path), path })
}

/// Starts the client with the URI as its first argument, then `extra` (`launcher::launch_args`),
/// all raw: no quoting, none contains spaces or quotes. In its own folder; the process handle
/// is closed at once (Roblox's anti-cheat has flagged long-held handles). Returns the PID and
/// its start time, read while our handle still reserves the PID (0: already gone). The URI
/// and the arguments are never logged.
pub fn start(exe: &ClientExe, uri: &Secret, extra: &[String]) -> Result<(u32, u64)> {
    let dir = exe.path.parent().ok_or_else(|| AppError::Internal("Roblox's folder is unknown.".into()))?;
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
        .map_err(|e| AppError::Internal(format!("Could not start Roblox: {e}")))?;
    let started = crate::instances::process::start_time(child.id()).unwrap_or(0);
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
    let handler = [
        (HKEY_CURRENT_USER, r"Software\Classes\roblox-player\shell\open\command"),
        (HKEY_CLASSES_ROOT, r"roblox-player\shell\open\command"),
    ]
    .into_iter()
    .filter_map(|(root, key)| handler_exe(&reg_string(root, key, None)?))
    .find(|p| p.is_file());
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
