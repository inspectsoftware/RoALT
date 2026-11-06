//! A session's mstsc window: found by class and address, hidden (`SW_HIDE`, never minimized: a
//! minimized RDP window stops drawing the session), shown, and closed by ending its mstsc.

use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible, SW_HIDE, SW_SHOW, SetForegroundWindow, ShowWindow,
};
use windows::core::{BOOL, Owned};

use crate::error::{AppError, Result};

const CLASS: &str = "TscShellContainerClass";

/// mstsc's title names `address` ("127.0.0.2 - Remote Desktop Connection"), and not a longer
/// address that starts the same ("127.0.0.20") (pure).
pub fn title_matches(title: &str, address: &str) -> bool {
    !address.is_empty()
        && title.match_indices(address).any(|(i, _)| {
            let before = title[..i].chars().next_back();
            let after = title[i + address.len()..].chars().next();
            let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_digit() || c == '.'));
            edge(before) && edge(after)
        })
}

/// The session window for `address`, as a raw handle (HWND isn't Send).
pub fn find(address: &str) -> Option<isize> {
    struct Find<'a> {
        address: &'a str,
        found: Option<isize>,
    }
    unsafe extern "system" fn each(hwnd: HWND, data: LPARAM) -> BOOL {
        // SAFETY: `data` is the `&mut Find` passed to EnumWindows below, alive for the whole call.
        let f = unsafe { &mut *(data.0 as *mut Find) };
        let (mut class, mut title) = ([0u16; 64], [0u16; 256]);
        // SAFETY: `hwnd` comes from EnumWindows; these calls only read into the local buffers.
        let (c, t) = unsafe { (GetClassNameW(hwnd, &mut class), GetWindowTextW(hwnd, &mut title)) };
        let class = String::from_utf16_lossy(&class[..c.max(0) as usize]);
        let title = String::from_utf16_lossy(&title[..t.max(0) as usize]);
        if class == CLASS && title_matches(&title, f.address) {
            f.found = Some(hwnd.0 as isize);
            return false.into();
        }
        true.into()
    }
    let mut f = Find { address, found: None };
    // SAFETY: `each` has the WNDENUMPROC signature; `f` outlives the call. Stopping early makes
    // EnumWindows report an error, which says nothing here.
    let _ = unsafe { EnumWindows(Some(each), LPARAM(&raw mut f as isize)) };
    f.found
}

fn hwnd(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

/// The window is shown (not hidden).
pub fn is_visible(raw: isize) -> bool {
    // SAFETY: only reads the window's state; a stale handle answers false.
    unsafe { IsWindowVisible(hwnd(raw)) }.as_bool()
}

/// Shows (and brings forward) or hides the window.
pub fn set_visible(raw: isize, show: bool) {
    // SAFETY: a stale handle makes these calls fail harmlessly.
    unsafe {
        let _ = ShowWindow(hwnd(raw), if show { SW_SHOW } else { SW_HIDE });
        if show {
            let _ = SetForegroundWindow(hwnd(raw));
        }
    }
}

/// Ends the mstsc behind the window: the session is then disconnected (its clients keep running).
/// Ending the process skips mstsc's "disconnect?" question.
pub fn close(raw: isize) -> Result<()> {
    let mut pid = 0u32;
    // SAFETY: only reads the window's owner.
    unsafe { GetWindowThreadProcessId(hwnd(raw), Some(&raw mut pid)) };
    if pid == 0 {
        return Ok(());
    }
    if !crate::instances::process::exe_name(pid).is_some_and(|n| n.eq_ignore_ascii_case("mstsc.exe")) {
        return Err(AppError::Internal(format!("window {raw:#x} isn't mstsc's")));
    }
    // SAFETY: a handle with PROCESS_TERMINATE only, closed by Owned.
    unsafe {
        let h = Owned::new(OpenProcess(PROCESS_TERMINATE, false, pid).map_err(|e| AppError::Internal(e.message()))?);
        TerminateProcess(*h, 0).map_err(|e| AppError::Internal(e.message()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_name_the_exact_address() {
        assert!(title_matches("127.0.0.2 - Remote Desktop Connection", "127.0.0.2"));
        assert!(title_matches("127.0.0.2 - Remotedesktopverbindung", "127.0.0.2"));
        assert!(title_matches("Farm (127.0.0.3)", "127.0.0.3"));
        assert!(!title_matches("127.0.0.20 - Remote Desktop Connection", "127.0.0.2"));
        assert!(!title_matches("127.0.0.21", "127.0.0.2"));
        assert!(!title_matches("1127.0.0.2", "127.0.0.2"));
        assert!(!title_matches("Remote Desktop Connection", "127.0.0.2"));
        assert!(!title_matches("anything", ""));
        assert!(title_matches("127.0.0.20 and 127.0.0.2", "127.0.0.2"));
    }
}
