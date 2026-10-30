//! A client's window as the dashboard sees it: a small picture of it (`thumbnail`, through
//! Windows.Graphics.Capture, `capture`, which copies what the compositor already has, so
//! nothing is read from the client's memory) and showing, minimizing or bringing it to the front
//! (`show`), making it small (`micro`, back with the window placement it had) and laying all
//! of them out in a grid (`tile`). Window calls only (`SetWindowPos` and friends, as dragging a
//! window's edge does); a minimized client draws nothing and has no picture. `free_resize` gets
//! past the minimum size Roblox holds its window to; `resize` makes that work for a drag.

use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_STYLE, GetClientRect, GetWindowLongPtrW, GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed, SWP_ASYNCWINDOWPOS, SWP_FRAMECHANGED,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE,
    SW_SHOWNOACTIVATE, SetWindowLongPtrW, SetWindowPlacement, SetWindowPos, ShowWindow, ShowWindowAsync, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
    WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_OVERLAPPEDWINDOW, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};

use super::{afk, process};
use crate::error::{AppError, Result};

/// One picture: `width` × `height` pixels, RGBA, rows top-down (as canvas `ImageData` wants).
#[derive(Clone)]
pub struct Thumb {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What `show` does with a client's window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Show {
    /// Restored if minimized, then brought to the front.
    Focus,
    Minimize,
    Restore,
}

/// The client's window at most `max_w` pixels wide (its aspect kept), and whether it is parked
/// off-screen; None while it is minimized, has no window yet or can't be captured.
pub fn thumbnail(pid: u32, max_w: u32) -> Option<(Thumb, bool)> {
    let (hwnd, minimized) = process::main_window(pid)?;
    if minimized {
        return None;
    }
    Some((super::capture::picture(hwnd, max_w)?, is_parked(hwnd)))
}

/// Where a parked window goes: past any monitor, and not where Windows puts minimized ones.
const PARK_AT: i32 = -30_000;

/// Where each parked window was (pid, x, y, maximized). In memory only: after RoALT restarts, a
/// parked client is still recognized by its position (`is_parked`) and comes back at `UNPARK_AT`.
static PARKED: Mutex<Vec<(u32, i32, i32, bool)>> = Mutex::new(Vec::new());
const UNPARK_AT: (i32, i32) = (100, 100);

/// A left edge this far out is a parked window, not one on a monitor (pure).
fn parked_left(left: i32) -> bool {
    left <= PARK_AT / 2
}

fn is_parked(hwnd: HWND) -> bool {
    let mut r = RECT::default();
    // SAFETY: reads one window's state, its rectangle into a local.
    unsafe { !IsIconic(hwnd).as_bool() && GetWindowRect(hwnd, &mut r).is_ok() && parked_left(r.left) }
}

/// Moves the window off-screen instead of minimizing it: it keeps drawing, so its preview stays
/// live (and it keeps using the GPU). Queued, like a minimize; a hung client is minimized instead,
/// since getting it out of maximized or minimized would wait for it.
fn park(pid: u32, hwnd: HWND) {
    if is_parked(hwnd) {
        return;
    }
    if process::hung(pid) {
        // SAFETY: a window-manager call on a top-level window; posted.
        let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE) };
        return;
    }
    // SAFETY: reads one window's state.
    let zoomed = unsafe { IsZoomed(hwnd) }.as_bool();
    normal(hwnd, false);
    let mut r = RECT::default();
    // SAFETY: reads one window's rectangle into a local.
    let (x, y) = match unsafe { GetWindowRect(hwnd, &mut r) } {
        Ok(()) => (r.left, r.top),
        Err(e) => {
            log::debug!("park: client {pid}'s position unreadable: {e}");
            UNPARK_AT
        }
    };
    {
        let mut parked = PARKED.lock().unwrap_or_else(|e| e.into_inner());
        parked.retain(|p| p.0 != pid);
        parked.push((pid, x, y, zoomed));
    }
    // SAFETY: a window-manager call on a top-level window; queued, so a busy client holds up nothing.
    if let Err(e) = unsafe { SetWindowPos(hwnd, None, PARK_AT, PARK_AT, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) } {
        log::debug!("park: client {pid} not moved: {e}");
    }
}

/// Brings a parked window back where it was; false if it wasn't parked.
fn unpark(pid: u32, hwnd: HWND) -> bool {
    if !is_parked(hwnd) {
        return false;
    }
    let was = {
        let mut parked = PARKED.lock().unwrap_or_else(|e| e.into_inner());
        parked.iter().position(|p| p.0 == pid).map(|i| parked.remove(i))
    };
    let (x, y, zoomed) = was.map_or((UNPARK_AT.0, UNPARK_AT.1, false), |p| (p.1, p.2, p.3));
    // SAFETY: window-manager calls on a top-level window; queued.
    unsafe {
        if let Err(e) = SetWindowPos(hwnd, None, x, y, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) {
            log::debug!("unpark: client {pid} not moved: {e}");
        }
        if zoomed {
            let _ = ShowWindowAsync(hwnd, SW_SHOWMAXIMIZED);
        }
    }
    true
}

/// Brings the client back if it is parked (RoALT is closing: nothing could find it otherwise).
pub fn unpark_client(pid: u32) {
    if let Some((hwnd, _)) = process::main_window(pid) {
        unpark(pid, hwnd);
    }
}

/// Shows, minimizes or focuses the client's window. Minimize and Restore are queued, so a
/// busy or hung client holds up nothing; Focus waits, and refuses a client not responding.
/// `park`: Minimize moves it off-screen instead (`park`), so its preview stays live.
pub fn show(pid: u32, what: Show, park_it: bool) -> Result<()> {
    let (hwnd, minimized) = process::main_window(pid).ok_or_else(|| AppError::Invalid(tr!("That client has no window yet.")))?;
    let queue = |cmd| {
        // SAFETY: a window-manager call on a top-level window; posted, its return the old visibility.
        let _ = unsafe { ShowWindowAsync(hwnd, cmd) };
    };
    match what {
        Show::Minimize if park_it => park(pid, hwnd),
        Show::Minimize => queue(SW_MINIMIZE),
        Show::Restore => {
            if !unpark(pid, hwnd) {
                queue(SW_RESTORE);
            }
        }
        Show::Focus => {
            let hwnd = window_to_change(pid)?;
            unpark(pid, hwnd);
            if minimized {
                // SAFETY: as above; waits, so the window is back before it is brought to the front.
                let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
            }
            if !afk::to_front(hwnd) {
                return Err(AppError::Invalid(tr!("Windows didn't let RoALT bring that client to the front.")));
            }
        }
    }
    Ok(())
}

/// Where each window RoALT resized was before, by (instance id, pid), so "Normal size" puts it
/// back exactly; the first resize's wins. In memory only: after RoALT restarts, a small window
/// stays small.
static SAVED: Mutex<Vec<(String, u32, WINDOWPLACEMENT)>> = Mutex::new(Vec::new());

fn saved() -> MutexGuard<'static, Vec<(String, u32, WINDOWPLACEMENT)>> {
    SAVED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimizes the client without taking the focus (Minimize on launch), or parks it (`park`);
/// nothing if it has no window or is minimized already.
pub fn minimize_quietly(pid: u32, park_it: bool) {
    if let Some((hwnd, false)) = process::main_window(pid) {
        if park_it {
            return park(pid, hwnd);
        }
        // SAFETY: a window-manager call on a top-level window; posted (the tracker's poll must not
        // wait on a client still loading), its return the old visibility.
        let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE) };
    }
}

/// The client's main window, for changing it: an error while it has none or doesn't respond
/// (sizing a hung window would wait until it does).
fn window_to_change(pid: u32) -> Result<HWND> {
    let (hwnd, _) = process::main_window(pid).ok_or_else(|| AppError::Invalid(tr!("That client has no window yet.")))?;
    if process::hung(pid) {
        return Err(AppError::Invalid(tr!("That client isn't responding.")));
    }
    Ok(hwnd)
}

/// A window's style for one resize. Windows only holds a window to the minimum size the program
/// asks for (`WM_GETMINMAXINFO`) when it has a sizing frame or isn't a popup (pure).
fn borderless(style: u32) -> u32 {
    (style & !(WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX).0) | WS_POPUP.0
}

/// A window's style while it is dragged by its title bar: it keeps the title bar, and Windows
/// doesn't throw it back up to the minimum size (pure).
pub fn drag_style(style: u32) -> u32 {
    (style & !WS_THICKFRAME.0) | WS_POPUP.0
}

pub fn style_of(hwnd: HWND) -> isize {
    // SAFETY: reads one window's style.
    unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) }
}

/// Sets a window's style; the frame follows with the next SWP_FRAMECHANGED.
pub fn set_style(hwnd: HWND, style: isize) {
    // SAFETY: changes one top-level window's style; it answers with a message, as to a theme change.
    unsafe { SetWindowLongPtrW(hwnd, GWL_STYLE, style) };
}

/// Sets a window to `rect` (x, y, width, height; `keep_place`: only the size), below the minimum
/// Roblox asks for if need be: the style changes only for this one call, so the window keeps its
/// title bar and edges. Waits for the client, so it must have answered before the style goes back.
pub fn free_resize(hwnd: HWND, (x, y, w, h): (i32, i32, i32, i32), keep_place: bool) {
    super::resize::mark_small(hwnd);
    let style = style_of(hwnd);
    set_style(hwnd, borderless(style as u32) as isize);
    let place = if keep_place { SWP_NOMOVE } else { Default::default() };
    // SAFETY: window-manager calls on a top-level window.
    if let Err(e) = unsafe { SetWindowPos(hwnd, None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE | place) } {
        log::debug!("resize failed: {e}");
    }
    set_style(hwnd, style);
    // SAFETY: as above; only makes the frame follow the style again.
    let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
}

/// `on`: remembers where the window is (once per client), then sets its client area to `size`;
/// off: puts back what was remembered. Returns the client area it ended up with: Roblox may still
/// resize itself, and then it stays bigger than asked; the caller says so.
pub fn micro(id: &str, pid: u32, on: bool, size: (u32, u32)) -> Result<(u32, u32)> {
    let hwnd = window_to_change(pid)?;
    if on {
        remember(id, pid, hwnd);
        normal(hwnd, false);
        let (fw, fh) = frame(hwnd).unwrap_or((0, 0));
        free_resize(hwnd, (0, 0, size.0 as i32 + fw, size.1 as i32 + fh), true);
    } else {
        let before = {
            let mut s = saved();
            let at = s.iter().position(|(i, p, ..)| i == id && *p == pid);
            at.map(|at| s.remove(at).2)
        };
        if let Some(mut wp) = before {
            wp.showCmd = restore_cmd(wp.showCmd, (wp.flags & WPF_RESTORETOMAXIMIZED).0 != 0);
            // SAFETY: `wp` was filled by GetWindowPlacement for this window, its length set.
            unsafe { SetWindowPlacement(hwnd, &wp) }.map_err(|e| AppError::Internal(tr!("Could not resize that window: {why}", why = e.message())))?;
        } else if (style_of(hwnd) as u32) & WS_POPUP.0 != 0 {
            set_style(hwnd, (((style_of(hwnd) as u32) & !WS_POPUP.0) | WS_OVERLAPPEDWINDOW.0) as isize);
            // SAFETY: as above.
            let _ = unsafe { SetWindowPos(hwnd, None, 0, 0, 1280, 720, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED) };
        }
    }
    client_size(hwnd).ok_or_else(|| AppError::Invalid(tr!("That client's window has closed.")))
}

/// Lays the clients (instance id, pid) out in a grid on the work area (taskbar left free) of
/// the monitor `near` is on: with `micro`, client areas of that size left to right and top to
/// bottom (more than fit start over at the top, overlapping), each remembered as `micro` does;
/// else cells sized so all fit, each as big as a 4:3 picture can be, and that is its normal size
/// from then on (what `micro` remembered is dropped). Minimized clients stay
/// minimized (Restore all first to include them), as do ones without a window or not
/// responding. Returns the ids laid out.
pub fn tile(clients: &[(String, u32)], near: HWND, micro: Option<(u32, u32)>) -> Vec<String> {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: reads the monitor nearest to a window into a local with cbSize set; with
    // MONITOR_DEFAULTTONEAREST there always is one.
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST), &mut mi) }.as_bool() {
        log::warn!("tile: monitor info unreadable, nothing laid out: {}", std::io::Error::last_os_error());
        return Vec::new();
    }
    let w = mi.rcWork;
    let wins: Vec<(&str, u32, HWND)> = clients
        .iter()
        .filter_map(|(id, pid)| match process::main_window(*pid) {
            Some((hwnd, false)) if !process::hung(*pid) => Some((id.as_str(), *pid, hwnd)),
            _ => None,
        })
        .collect();
    // One frame size for all: Roblox windows share a style.
    let cell = micro.and_then(|(w, h)| wins.first().and_then(|c| frame(c.2)).map(|(fw, fh)| (w as i32 + fw, h as i32 + fh)));
    let cells = grid(wins.len(), (w.left, w.top, w.right - w.left, w.bottom - w.top), cell);
    for (&(id, pid, hwnd), &(x, y, cw, ch)) in wins.iter().zip(&cells) {
        if micro.is_some() {
            remember(id, pid, hwnd);
        } else {
            saved().retain(|(i, p, _)| !(i == id && *p == pid));
        }
        normal(hwnd, false);
        free_resize(hwnd, (x, y, cw, ch), false);
    }
    wins.into_iter().map(|w| w.0.to_owned()).collect()
}

/// Cells (x, y, w, h) for `n` windows in `area` (x, y, w, h): `cell`-sized ones left to right,
/// top to bottom, starting over at the top once the area is full; else the column count whose
/// cells hold the biggest 4:3 picture, fewer columns on a tie (pure).
pub fn grid(n: usize, area: (i32, i32, i32, i32), cell: Option<(i32, i32)>) -> Vec<(i32, i32, i32, i32)> {
    let (ax, ay, aw, ah) = area;
    if n == 0 || aw <= 0 || ah <= 0 {
        return Vec::new();
    }
    let (cols, cw, ch, per) = match cell {
        Some((cw, ch)) => {
            let (cw, ch) = (cw.max(1), ch.max(1));
            let cols = (aw / cw).max(1) as usize;
            (cols, cw, ch, cols * (ah / ch).max(1) as usize)
        }
        None => {
            let picture = |c: usize| (aw / c as i32).min(ah / n.div_ceil(c) as i32 * 4 / 3);
            let cols = (1..=n).fold(1, |best, c| if picture(c) > picture(best) { c } else { best });
            (cols, aw / cols as i32, ah / n.div_ceil(cols) as i32, n)
        }
    };
    (0..n)
        .map(|i| {
            let slot = i % per;
            (ax + (slot % cols) as i32 * cw, ay + (slot / cols) as i32 * ch, cw, ch)
        })
        .collect()
}

/// What a remembered placement is shown with again (pure): maximized stays maximized, a
/// minimized one comes back as it would have on its own, else normal; without taking the focus
/// unless maximized.
pub fn restore_cmd(show_cmd: u32, to_maximized: bool) -> u32 {
    let max = SW_SHOWMAXIMIZED.0 as u32;
    if show_cmd == max || (show_cmd == SW_SHOWMINIMIZED.0 as u32 && to_maximized) { max } else { SW_SHOWNOACTIVATE.0 as u32 }
}

/// Keeps the window's placement unless one is kept for this client already.
fn remember(id: &str, pid: u32, hwnd: HWND) {
    let mut s = saved();
    if s.iter().any(|(i, p, _)| i == id && *p == pid) {
        return;
    }
    let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    // SAFETY: reads one window's placement into a local with length set.
    match unsafe { GetWindowPlacement(hwnd, &mut wp) } {
        Ok(()) => s.push((id.to_owned(), pid, wp)),
        Err(e) => log::debug!("instance {id}: window placement not remembered: {e}"),
    }
}

/// Out of maximized or minimized, without taking the focus, so it can be sized; `queued`
/// posts it (ahead of a queued resize) instead of waiting for the client.
fn normal(hwnd: HWND, queued: bool) {
    // SAFETY: reads and sets one top-level window's state.
    unsafe {
        if IsIconic(hwnd).as_bool() || IsZoomed(hwnd).as_bool() {
            let _ = if queued { ShowWindowAsync(hwnd, SW_SHOWNOACTIVATE) } else { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        }
    }
}

/// How much bigger the window is than its client area (its borders and title bar).
fn frame(hwnd: HWND) -> Option<(i32, i32)> {
    let (mut w, mut c) = (RECT::default(), RECT::default());
    // SAFETY: reads one window's rectangles into locals.
    unsafe {
        GetWindowRect(hwnd, &mut w).ok()?;
        GetClientRect(hwnd, &mut c).ok()?;
    }
    Some((w.right - w.left - c.right, w.bottom - w.top - c.bottom))
}

fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut r = RECT::default();
    // SAFETY: reads one window's client size into a local.
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    Some(((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32))
}

/// BGRX to opaque RGBA (pure).
pub fn to_rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A window that insists on 800 x 600, as Roblox's does.
    unsafe extern "system" fn insists(hwnd: HWND, msg: u32, w: windows::Win32::Foundation::WPARAM, l: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
        use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, MINMAXINFO, WM_GETMINMAXINFO};
        if msg == WM_GETMINMAXINFO {
            // SAFETY: for this message `l` points at the MINMAXINFO to fill in.
            unsafe { (*(l.0 as *mut MINMAXINFO)).ptMinTrackSize = windows::Win32::Foundation::POINT { x: 800, y: 600 } };
            return windows::Win32::Foundation::LRESULT(0);
        }
        // SAFETY: the default for everything else.
        unsafe { DefWindowProcW(hwnd, msg, w, l) }
    }

    /// Windows' own move loop, driven by keys, on a window below its minimum. Needs a desktop of
    /// its own: `hidden-launch`, then this test by name with --ignored --exact.
    fn moved_size(toggle: bool, size_loop: bool, start: (i32, i32)) -> (i32, i32) {
        use windows::Win32::Foundation::{LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, PostMessageW, RegisterClassW, SC_MOVE, SC_SIZE, SendMessageW, WINDOW_EX_STYLE, WM_KEYDOWN, WM_SYSCOMMAND, WNDCLASSW, WS_VISIBLE};
        use windows::core::w;
        // SAFETY: a private window of this thread, destroyed at the end.
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(insists), lpszClassName: w!("roalt-test-moveloop"), ..Default::default() };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("roalt-test-moveloop"), w!("t"), WS_OVERLAPPEDWINDOW | WS_VISIBLE, 100, 100, 1000, 800, None, None, None, None).unwrap();
            free_resize(hwnd, (100, 100, start.0, start.1), false);
            let h = hwnd.0 as isize;
            let keys = std::thread::spawn(move || {
                let hwnd = HWND(h as *mut _);
                std::thread::sleep(std::time::Duration::from_millis(400));
                if toggle {
                    set_style(hwnd, drag_style(style_of(hwnd) as u32) as isize);
                }
                // In the size loop the first Right picks the right edge, then Left takes it in.
                let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(0x27), LPARAM(0));
                for _ in 0..5 {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(if size_loop { 0x25 } else { 0x27 }), LPARAM(0));
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
                let _ = PostMessageW(Some(hwnd), WM_KEYDOWN, WPARAM(0x0D), LPARAM(0)); // Return
            });
            SendMessageW(hwnd, WM_SYSCOMMAND, Some(WPARAM(if size_loop { SC_SIZE } else { SC_MOVE } as usize)), Some(LPARAM(0)));
            keys.join().unwrap();
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).unwrap();
            let size = (r.right - r.left, r.bottom - r.top);
            let moved = r.left != 100;
            DestroyWindow(hwnd).unwrap();
            println!("toggle {toggle} size_loop {size_loop}: size {size:?}, moved {moved}");
            size
        }
    }

    #[test]
    #[ignore = "opens a window: run on a desktop of its own"]
    fn move_loop_keeps_the_size_with_the_drag_style() {
        println!("without: {:?}", moved_size(false, false, (200, 150)));
        assert_eq!(moved_size(true, false, (200, 150)), (200, 150));
    }

    #[test]
    #[ignore = "opens a window: run on a desktop of its own"]
    fn size_loop_goes_below_the_minimum_with_the_drag_style() {
        println!("small, plain: {:?}", moved_size(false, true, (500, 400)));
        println!("small, drag style: {:?}", moved_size(true, true, (500, 400)));
        println!("big, plain: {:?}", moved_size(false, true, (900, 700)));
        println!("big, drag style: {:?}", moved_size(true, true, (900, 700)));
    }

    #[test]
    fn goes_below_the_windows_minimum_and_keeps_its_frame() {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, RegisterClassW, WINDOW_EX_STYLE, WNDCLASSW};
        use windows::core::w;
        // SAFETY: a private hidden window of this thread, destroyed at the end.
        unsafe {
            let class = WNDCLASSW { lpfnWndProc: Some(insists), lpszClassName: w!("roalt-test-minsize"), ..Default::default() };
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), w!("roalt-test-minsize"), w!("t"), WS_OVERLAPPEDWINDOW, 0, 0, 1000, 800, None, None, None, None).unwrap();
            let before = style_of(hwnd);

            SetWindowPos(hwnd, None, 0, 0, 200, 150, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE).unwrap();
            let mut r = RECT::default();
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!(r.right - r.left, 800, "with a frame, Windows holds it to the minimum");

            free_resize(hwnd, (0, 0, 200, 150), true);
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!((r.right - r.left, r.bottom - r.top), (200, 150));
            assert_eq!(style_of(hwnd), before, "title bar and edges are back");
            assert!(frame(hwnd).unwrap().0 > 0, "and drawn");

            // Moving it doesn't throw it back to the minimum.
            SetWindowPos(hwnd, None, 50, 50, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE).unwrap();
            GetWindowRect(hwnd, &mut r).unwrap();
            assert_eq!((r.right - r.left, r.bottom - r.top), (200, 150));
            DestroyWindow(hwnd).unwrap();
        }
    }

    #[test]
    fn tells_a_parked_window_from_one_on_a_monitor() {
        assert!(parked_left(PARK_AT));
        assert!(!parked_left(-1920), "a monitor to the left");
        assert!(!parked_left(-7680), "several of them");
        assert!(!parked_left(0));
    }

    #[test]
    fn pictures_turn_rgba() {
        assert_eq!(to_rgba(&[1, 2, 3, 0, 4, 5, 6, 9]), [3, 2, 1, 255, 6, 5, 4, 255]);
    }

    #[test]
    fn grids_fit_the_work_area() {
        let area = (0, 40, 1920, 1000);
        assert_eq!(grid(0, area, None), []);
        assert_eq!(grid(1, area, None), [(0, 40, 1920, 1000)]);
        // Two side by side beat two stacked on a wide screen.
        assert_eq!(grid(2, area, None), [(0, 40, 960, 1000), (960, 40, 960, 1000)]);
        assert_eq!(grid(4, area, None)[3], (960, 540, 960, 500), "2 x 2");
        assert!(grid(12, area, None).iter().all(|&(x, y, w, h)| x >= 0 && x + w <= 1920 && y >= 40 && y + h <= 1040));
        // Fixed cells: 5 across, 3 down; the 16th starts over at the top left.
        let micro = grid(16, area, Some((336, 279)));
        assert_eq!(micro[4], (1344, 40, 336, 279));
        assert_eq!(micro[5], (0, 319, 336, 279));
        assert_eq!(micro[15], (0, 40, 336, 279));
        assert_eq!(grid(2, (0, 0, 100, 100), Some((300, 300))), [(0, 0, 300, 300), (0, 0, 300, 300)]);
    }

    #[test]
    fn placements_come_back_without_the_focus() {
        let (min, max, normal) = (SW_SHOWMINIMIZED.0 as u32, SW_SHOWMAXIMIZED.0 as u32, 1);
        let quiet = SW_SHOWNOACTIVATE.0 as u32;
        assert_eq!(restore_cmd(max, false), max);
        assert_eq!(restore_cmd(min, true), max);
        assert_eq!(restore_cmd(min, false), quiet);
        assert_eq!(restore_cmd(normal, false), quiet);
    }
}
