//! A client's window as the dashboard sees it: a small picture of it (`thumbnail`, through
//! `PrintWindow` with `PW_RENDERFULLCONTENT`, which copies what the compositor already has, so
//! nothing is read from the client's memory) and showing, minimizing or bringing it to the front
//! (`show`), making it small (`micro`, back with the window placement it had) and laying all
//! of them out in a grid (`tile`). Window calls only (`SetWindowPos` and friends, as dragging a
//! window's edge does); a minimized client draws nothing and has no picture.

use std::sync::{Mutex, MutexGuard};

use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDC, GetMonitorInfoW, HALFTONE, HGDIOBJ, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ReleaseDC,
    SRCCOPY, SelectObject, SetStretchBltMode, StretchBlt,
};
use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PW_CLIENTONLY, PrintWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetWindowPlacement, GetWindowRect, IsIconic, IsZoomed, PW_RENDERFULLCONTENT, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOZORDER, SW_MINIMIZE, SW_RESTORE, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, SW_SHOWMINNOACTIVE, SW_SHOWNOACTIVATE,
    SetWindowPlacement, SetWindowPos, ShowWindow, ShowWindowAsync, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED,
};

use super::{afk, process};
use crate::error::{AppError, Result};

/// One picture: `width` × `height` pixels, RGBA, rows top-down (as canvas `ImageData` wants).
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

/// The client's window at most `max_w` pixels wide (its aspect kept); None while it is
/// minimized, not responding (PrintWindow would wait for it), has no window yet or can't be drawn.
pub fn thumbnail(pid: u32, max_w: u32) -> Option<Thumb> {
    let (hwnd, minimized) = process::main_window(pid)?;
    if minimized || process::hung(pid) {
        return None;
    }
    let mut r = RECT::default();
    // SAFETY: reads one window's client size into a local.
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    let (cw, ch) = (r.right - r.left, r.bottom - r.top);
    if cw <= 0 || ch <= 0 {
        return None;
    }
    let (tw, th) = fit(cw as u32, ch as u32, max_w);
    // SAFETY: every GDI object made here is selected out and deleted before returning; the DIB's
    // bits are copied out while it is alive and of the size asked for.
    unsafe {
        let screen = GetDC(None);
        let full = CreateCompatibleDC(Some(screen));
        let small = CreateCompatibleDC(Some(screen));
        let big = CreateCompatibleBitmap(screen, cw, ch);
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: tw as i32,
                biHeight: -(th as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let dib = CreateDIBSection(Some(small), &info, DIB_RGB_COLORS, &mut bits, None, 0).ok();
        let mut out = None;
        if let Some(dib) = dib {
            let old_full = SelectObject(full, HGDIOBJ(big.0));
            let old_small = SelectObject(small, HGDIOBJ(dib.0));
            let drawn = PrintWindow(hwnd, full, PRINT_WINDOW_FLAGS(PW_CLIENTONLY.0 | PW_RENDERFULLCONTENT)).as_bool();
            SetStretchBltMode(small, HALFTONE);
            if drawn && StretchBlt(small, 0, 0, tw as i32, th as i32, Some(full), 0, 0, cw, ch, SRCCOPY).as_bool() {
                let bgra = std::slice::from_raw_parts(bits as *const u8, (tw * th * 4) as usize);
                out = Some(Thumb { width: tw, height: th, rgba: to_rgba(bgra) });
            }
            SelectObject(small, old_small);
            SelectObject(full, old_full);
            let _ = DeleteObject(HGDIOBJ(dib.0));
        }
        let _ = DeleteObject(HGDIOBJ(big.0));
        let _ = DeleteDC(small);
        let _ = DeleteDC(full);
        ReleaseDC(None, screen);
        out
    }
}

/// Shows, minimizes or focuses the client's window. Minimize and Restore are queued, so a
/// busy or hung client holds up nothing; Focus waits, and refuses a client not responding.
pub fn show(pid: u32, what: Show) -> Result<()> {
    let (hwnd, minimized) = process::main_window(pid).ok_or_else(|| AppError::Invalid("That client has no window yet.".into()))?;
    let queue = |cmd| {
        // SAFETY: a window-manager call on a top-level window; posted, its return the old visibility.
        let _ = unsafe { ShowWindowAsync(hwnd, cmd) };
    };
    match what {
        Show::Minimize => queue(SW_MINIMIZE),
        Show::Restore => queue(SW_RESTORE),
        Show::Focus => {
            let hwnd = window_to_change(pid)?;
            if minimized {
                // SAFETY: as above; waits, so the window is back before it is brought to the front.
                let _ = unsafe { ShowWindow(hwnd, SW_RESTORE) };
            }
            if !afk::to_front(hwnd) {
                return Err(AppError::Invalid("Windows didn't let RoALT bring that client to the front.".into()));
            }
        }
    }
    Ok(())
}

/// The client area "Micro window" asks for, in pixels.
pub const MICRO: (u32, u32) = (320, 240);

/// Where each window RoALT resized was before, by (instance id, pid), so "Normal size" puts it
/// back exactly; the first resize's wins. In memory only: after RoALT restarts, a small window
/// stays small.
static SAVED: Mutex<Vec<(String, u32, WINDOWPLACEMENT)>> = Mutex::new(Vec::new());

fn saved() -> MutexGuard<'static, Vec<(String, u32, WINDOWPLACEMENT)>> {
    SAVED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Minimizes the client without taking the focus (Minimize on launch); nothing if it has no
/// window or is minimized already.
pub fn minimize_quietly(pid: u32) {
    if let Some((hwnd, false)) = process::main_window(pid) {
        // SAFETY: a window-manager call on a top-level window; posted (the tracker's poll must not
        // wait on a client still loading), its return the old visibility.
        let _ = unsafe { ShowWindowAsync(hwnd, SW_SHOWMINNOACTIVE) };
    }
}

/// The client's main window, for changing it: an error while it has none or doesn't respond
/// (sizing a hung window would wait until it does).
fn window_to_change(pid: u32) -> Result<HWND> {
    let (hwnd, _) = process::main_window(pid).ok_or_else(|| AppError::Invalid("That client has no window yet.".into()))?;
    if process::hung(pid) {
        return Err(AppError::Invalid("That client isn't responding.".into()));
    }
    Ok(hwnd)
}

/// `on`: remembers where the window is (once per client), then sets its client area to
/// `MICRO`; off: puts back what was remembered (nothing to do if nothing was). Returns the
/// client area it ended up with: Roblox may hold its window to a minimum size of its own
/// (`WM_GETMINMAXINFO`), and then it stays bigger than asked; the caller says so.
pub fn micro(id: &str, pid: u32, on: bool) -> Result<(u32, u32)> {
    let hwnd = window_to_change(pid)?;
    if on {
        remember(id, pid, hwnd);
        normal(hwnd, false);
        if let Some((fw, fh)) = frame(hwnd) {
            // SAFETY: a window-manager call on a top-level window; it waits for the client to take it.
            let _ = unsafe {
                SetWindowPos(hwnd, None, 0, 0, MICRO.0 as i32 + fw, MICRO.1 as i32 + fh, SWP_NOZORDER | SWP_NOACTIVATE | SWP_NOMOVE)
            };
        }
    } else {
        let before = {
            let mut s = saved();
            let at = s.iter().position(|(i, p, _)| i == id && *p == pid);
            at.map(|at| s.remove(at).2)
        };
        if let Some(mut wp) = before {
            wp.showCmd = restore_cmd(wp.showCmd, (wp.flags & WPF_RESTORETOMAXIMIZED).0 != 0);
            // SAFETY: `wp` was filled by GetWindowPlacement for this window, its length set.
            unsafe { SetWindowPlacement(hwnd, &wp) }.map_err(|e| AppError::Internal(format!("Could not resize that window: {e}")))?;
        }
    }
    client_size(hwnd).ok_or_else(|| AppError::Invalid("That client's window has closed.".into()))
}

/// Lays the clients (instance id, pid) out in a grid on the work area (taskbar left free) of
/// the monitor `near` is on: with `micro`, `MICRO`-sized client areas left to right and top to
/// bottom (more than fit start over at the top, overlapping), each remembered as `micro` does;
/// else cells sized so all fit, each as big as a 4:3 picture can be, and that is its normal size
/// from then on (what `micro` remembered is dropped). Minimized clients stay
/// minimized (Restore all first to include them), as do ones without a window or not
/// responding. Returns the ids laid out.
pub fn tile(clients: &[(String, u32)], near: HWND, micro: bool) -> Vec<String> {
    let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
    // SAFETY: reads the monitor nearest to a window into a local with cbSize set; with
    // MONITOR_DEFAULTTONEAREST there always is one.
    if !unsafe { GetMonitorInfoW(MonitorFromWindow(near, MONITOR_DEFAULTTONEAREST), &mut mi) }.as_bool() {
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
    let cell = if micro { wins.first().and_then(|w| frame(w.2)).map(|(fw, fh)| (MICRO.0 as i32 + fw, MICRO.1 as i32 + fh)) } else { None };
    let cells = grid(wins.len(), (w.left, w.top, w.right - w.left, w.bottom - w.top), cell);
    for (&(id, pid, hwnd), &(x, y, cw, ch)) in wins.iter().zip(&cells) {
        if micro {
            remember(id, pid, hwnd);
        } else {
            saved().retain(|(i, p, _)| !(i == id && *p == pid));
        }
        normal(hwnd, true);
        // SAFETY: a window-manager call on a top-level window; queued (ASYNC) so one slow client
        // doesn't hold up the rest.
        let _ = unsafe { SetWindowPos(hwnd, None, x, y, cw, ch, SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS) };
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
    if unsafe { GetWindowPlacement(hwnd, &mut wp) }.is_ok() {
        s.push((id.to_owned(), pid, wp));
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
    Some(((w.right - w.left) - (c.right - c.left), (w.bottom - w.top) - (c.bottom - c.top)))
}

fn client_size(hwnd: HWND) -> Option<(u32, u32)> {
    let mut r = RECT::default();
    // SAFETY: reads one window's client size into a local.
    unsafe { GetClientRect(hwnd, &mut r) }.ok()?;
    Some(((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32))
}

/// A `w` × `h` picture scaled to at most `max_w` wide (pure; never upscaled, at least 1 × 1).
pub fn fit(w: u32, h: u32, max_w: u32) -> (u32, u32) {
    let tw = w.min(max_w).max(1);
    (tw, (u64::from(h) * u64::from(tw) / u64::from(w.max(1))).max(1) as u32)
}

/// GDI's BGRX to opaque RGBA (pure).
pub fn to_rgba(bgra: &[u8]) -> Vec<u8> {
    bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], 255]).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_fit_and_turn_rgba() {
        assert_eq!(fit(1920, 1080, 320), (320, 180));
        assert_eq!(fit(200, 100, 320), (200, 100), "never upscaled");
        assert_eq!(fit(5000, 1, 320), (320, 1));
        assert_eq!(fit(0, 0, 320), (1, 1));
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
