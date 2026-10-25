//! Roblox client processes: listing, liveness, closing. Handles are short-lived and
//! limited-rights (Roblox's anti-cheat has flagged long-held full handles).
//!
//! Listing uses a Toolhelp snapshot (no process handles at all); sysinfo would open every
//! process with PROCESS_VM_READ and keep those handles, so it is not used here.

use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{FILETIME, HANDLE, HWND, LPARAM, WAIT_TIMEOUT, WPARAM};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::{
    GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
    PROCESS_TERMINATE, TerminateProcess, WaitForSingleObject,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
use windows::core::{BOOL, Owned};

use crate::error::{AppError, Result};

const CLIENT_EXE: &str = "RobloxPlayerBeta.exe";
/// 1601-01-01 (FILETIME's epoch) to 1970-01-01, in 100 ns ticks.
const UNIX_EPOCH_TICKS: u64 = 116_444_736_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Process start, seconds since the Unix epoch (pid + start time identify a process).
    pub start_time: u64,
}

/// Running `RobloxPlayerBeta.exe` processes.
pub fn roblox_processes() -> Vec<ProcInfo> {
    let mut out = Vec::new();
    // SAFETY: the snapshot handle is owned and closed on drop; `e` is a zeroed PROCESSENTRY32W
    // with dwSize set, as Process32FirstW/NextW require.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let snap = Owned::new(snap);
        let mut e = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(*snap, &mut e).is_ok();
        while more {
            if is_client(&e.szExeFile)
                && let Some(start_time) = start_time(e.th32ProcessID)
            {
                out.push(ProcInfo { pid: e.th32ProcessID, start_time });
            }
            more = Process32NextW(*snap, &mut e).is_ok();
        }
    }
    out
}

/// Start time of a running process, `None` if it is gone.
pub fn start_time(pid: u32) -> Option<u64> {
    started(*open(pid, PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE)?)
}

/// Asks the client to close: WM_CLOSE to its top-level windows, again after ~400 ms (Roblox may
/// show a leave prompt), then TerminateProcess if it is still alive after `grace`. `force`
/// terminates at once. `expected` is the start time the process was seen with; another
/// process now holding `pid` is left alone. Already gone → Ok.
pub fn close(pid: u32, expected: Option<u64>, force: bool, grace: Duration) -> Result<()> {
    // Pid + start time, so a pid already reused, or reused while we wait, is never touched.
    let Some(t0) = start_time(pid).filter(|t| expected.is_none_or(|e| e == *t)) else { return Ok(()) };
    let alive = || start_time(pid) == Some(t0);
    if !force {
        let deadline = Instant::now() + grace;
        post_close(pid);
        sleep(Duration::from_millis(400));
        if alive() {
            post_close(pid);
        }
        while alive() && Instant::now() < deadline {
            sleep(Duration::from_millis(100));
        }
        if !alive() {
            return Ok(());
        }
    }
    terminate(pid, t0)
}

fn terminate(pid: u32, t0: u64) -> Result<()> {
    let failed = |e: String| AppError::Internal(format!("Could not close Roblox (process {pid}): {e}"));
    let Some(h) = open(pid, PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE) else {
        return if start_time(pid) == Some(t0) { Err(failed("access denied".into())) } else { Ok(()) };
    };
    if started(*h) != Some(t0) {
        return Ok(());
    }
    // SAFETY: `h` is a live handle with PROCESS_TERMINATE | SYNCHRONIZE, closed on drop.
    unsafe {
        if let Err(e) = TerminateProcess(*h, 1)
            && started(*h).is_some()
        {
            return Err(failed(e.message()));
        }
        // Termination is asynchronous: wait (≤ 3 s) until its objects are gone, so taking the
        // multi-instance lock right after doesn't find Roblox's event still there.
        WaitForSingleObject(*h, 3_000);
    }
    Ok(())
}

/// Posts WM_CLOSE to every top-level window (visible or not) of `pid`.
fn post_close(pid: u32) {
    unsafe extern "system" fn each(hwnd: HWND, pid: LPARAM) -> BOOL {
        let mut owner = 0u32;
        // SAFETY: `hwnd` comes from EnumWindows; PostMessageW only queues a message.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&raw mut owner));
            if owner as isize == pid.0 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    // SAFETY: `each` has the WNDENUMPROC signature and only uses its arguments.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(pid as isize)) };
}

fn open(pid: u32, rights: PROCESS_ACCESS_RIGHTS) -> Option<Owned<HANDLE>> {
    // SAFETY: on success the handle is ours; Owned closes it on drop.
    unsafe { OpenProcess(rights, false, pid).ok().map(|h| Owned::new(h)) }
}

/// Start of the process behind `h` in Unix seconds, `None` once it has exited (signalled).
fn started(h: HANDLE) -> Option<u64> {
    let (mut created, mut exited, mut kernel, mut user) =
        (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: `h` is a live handle with PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE; the out
    // pointers are locals.
    unsafe {
        if WaitForSingleObject(h, 0) != WAIT_TIMEOUT {
            return None;
        }
        GetProcessTimes(h, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
    }
    let ticks = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    Some(ticks.saturating_sub(UNIX_EPOCH_TICKS) / 10_000_000)
}

fn is_client(exe: &[u16]) -> bool {
    let len = exe.iter().position(|&c| c == 0).unwrap_or(exe.len());
    String::from_utf16_lossy(&exe[..len]).eq_ignore_ascii_case(CLIENT_EXE)
}

#[cfg(test)]
mod tests {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// A windowless child from System32 (never Roblox).
    fn spawn(exe: &str, args: &[&str]) -> Child {
        let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
        Command::new(root.join("System32").join(exe))
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap()
    }

    fn sleeper() -> Child {
        spawn("PING.EXE", &["-n", "30", "127.0.0.1"])
    }

    #[test]
    fn start_time_of_live_and_dead_processes() {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let own = start_time(std::process::id()).unwrap();
        assert!(own <= now && own > now - 3600, "{own} vs {now}");
        assert_eq!(start_time(u32::MAX), None);
        // Exited, but our Child still holds its handle so the process object lingers: still gone.
        let mut done = spawn("cmd.exe", &["/c", "exit", "0"]);
        done.wait().unwrap();
        assert_eq!(start_time(done.id()), None);
    }

    #[test]
    fn force_close_terminates() {
        let mut child = sleeper();
        let pid = child.id();
        let t0 = start_time(pid);
        assert!(t0.is_some());
        close(pid, t0, true, Duration::from_secs(5)).unwrap();
        assert_eq!(child.try_wait().unwrap().and_then(|s| s.code()), Some(1));
        close(pid, t0, true, Duration::from_secs(5)).unwrap(); // already gone
    }

    #[test]
    fn a_process_started_at_another_time_is_left_alone() {
        let mut child = sleeper();
        let pid = child.id();
        let other = start_time(pid).map(|t| t - 10);
        close(pid, other, true, Duration::from_secs(5)).unwrap();
        assert!(child.try_wait().unwrap().is_none(), "a reused pid must not be closed");
        close(pid, None, true, Duration::from_secs(5)).unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn graceful_close_without_windows_terminates_after_grace() {
        let mut child = sleeper();
        let begun = Instant::now();
        close(child.id(), None, false, Duration::from_millis(200)).unwrap();
        assert!(begun.elapsed() >= Duration::from_millis(400));
        assert_eq!(child.try_wait().unwrap().and_then(|s| s.code()), Some(1));
    }

    #[test]
    fn roblox_processes_does_not_panic() {
        let _ = roblox_processes();
        let utf16 = |s: &str| s.encode_utf16().collect::<Vec<_>>();
        assert!(is_client(&utf16("robloxplayerbeta.EXE\0junk")));
        assert!(!is_client(&utf16("RobloxPlayerBeta.exe.bak")));
    }
}
