//! Roblox login / session windows on throw-away WebView2 profiles.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tauri::webview::{Cookie, PlatformWebview};
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings4;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::core::Interface;

use crate::error::{AppError, Result};
use crate::roblox_api::endpoints;
use crate::vault::Secret;

const TMP_DIR: &str = "tmp-webview";
const POLL: Duration = Duration::from_millis(700);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// Variables WebView2 reads before (and instead of) the options Tauri passes it.
const WEBVIEW2_VARS: [&str; 6] = [
    "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
    "WEBVIEW2_USER_DATA_FOLDER",
    "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
    "WEBVIEW2_RELEASE_CHANNELS",
    "WEBVIEW2_CHANNEL_SEARCH_KIND",
    "WEBVIEW2_RELEASE_CHANNEL_PREFERENCE",
];

static LOGIN_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Opens `https://www.roblox.com/login` in a `login-<uuid>` window on a fresh profile under
/// `<data_dir>/tmp-webview/<uuid>` (incognito, no devtools, no autofill, no context menu,
/// no IPC capability). A worker thread polls `cookies()` every ~700 ms until a non-empty
/// `.ROBLOSECURITY` appears, then closes the window, waits for its browser process to exit
/// and deletes the profile. Closing the window first → `AppError::Cancelled`.
pub async fn capture_login(app: &AppHandle, data_dir: &Path) -> Result<Secret> {
    let slot = LoginSlot::take(app)?;
    let id = uuid::Uuid::new_v4();
    let label = format!("login-{id}");
    let profile = data_dir.join(TMP_DIR).join(id.to_string());
    let url = endpoints::LOGIN_PAGE.parse().map_err(|e: url::ParseError| AppError::Internal(e.to_string()))?;
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::External(url))
        .title("Sign in to Roblox — RoALT")
        .inner_size(480.0, 760.0)
        .center()
        .data_directory(profile.clone())
        .incognito(true)
        .devtools(cfg!(debug_assertions))
        .general_autofill_enabled(false)
        .build();
    let window = match built {
        Ok(w) => w,
        Err(e) => {
            cleanup(None, profile).await;
            return Err(e.into());
        }
    };
    let (pid, cookie) = match harden(&window).await {
        Ok(pid) => (Some(pid), wait_for_cookie(app.clone(), label).await),
        Err(e) => (None, Err(e)),
    };
    let _ = window.destroy();
    drop(slot);
    cleanup(pid, profile).await;
    cookie
}

/// Deletes leftover `tmp-webview/*` profiles from earlier runs.
pub fn sweep_temp_profiles(data_dir: &Path) {
    let entries = match std::fs::read_dir(data_dir.join(TMP_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            log::warn!("could not list old login profiles: {e}");
            return;
        }
    };
    for path in entries.flatten().map(|e| e.path()) {
        let removed = if path.is_dir() { std::fs::remove_dir_all(&path) } else { std::fs::remove_file(&path) };
        if let Err(e) = removed {
            log::warn!("could not delete old login profile {}: {e}", path.display());
        }
    }
}

/// Removes `WEBVIEW2_*` override variables so they cannot redirect or merge profiles.
/// Called first thing in `main`, before any thread starts.
pub fn clear_webview2_env() {
    for name in WEBVIEW2_VARS {
        // SAFETY: runs first thing in `main`, before any other thread exists, so nothing can
        // read the environment at the same time.
        unsafe { std::env::remove_var(name) };
    }
}

/// One login window at a time (a double click must not open two).
struct LoginSlot;

impl LoginSlot {
    fn take(app: &AppHandle) -> Result<Self> {
        if !LOGIN_ACTIVE.swap(true, Ordering::AcqRel) {
            return Ok(Self);
        }
        if let Some(w) = app.webview_windows().into_iter().find_map(|(l, w)| l.starts_with("login-").then_some(w)) {
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        Err(AppError::Invalid("Finish the Roblox sign-in window that's already open.".into()))
    }
}

impl Drop for LoginSlot {
    fn drop(&mut self) {
        LOGIN_ACTIVE.store(false, Ordering::Release);
    }
}

/// Turns off what Tauri does not expose and returns the profile's browser process id.
async fn harden(window: &WebviewWindow) -> Result<u32> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    window.with_webview(move |pv| {
        let r = harden_webview(&pv).map_err(|e| AppError::Internal(format!("WebView2 setup failed: {e}")));
        let _ = tx.send(r);
    })?;
    // The closure is dropped unrun if the window is already gone.
    rx.await.map_err(|_| AppError::Cancelled)?
}

fn harden_webview(pv: &PlatformWebview) -> windows::core::Result<u32> {
    // SAFETY: COM calls on the main thread (with_webview), on live interfaces from WebView2.
    unsafe {
        let wv = pv.controller().CoreWebView2()?;
        let settings = wv.Settings()?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        let settings = settings.cast::<ICoreWebView2Settings4>()?;
        settings.SetAreBrowserAcceleratorKeysEnabled(false)?;
        settings.SetIsPasswordAutosaveEnabled(false)?;
        let mut pid = 0;
        wv.BrowserProcessId(&mut pid)?;
        Ok(pid)
    }
}

async fn wait_for_cookie(app: AppHandle, label: String) -> Result<Secret> {
    tauri::async_runtime::spawn_blocking(move || poll_cookie(&app, &label)).await?
}

/// Blocking: `cookies()` deadlocks on the main thread and in event handlers.
fn poll_cookie(app: &AppHandle, label: &str) -> Result<Secret> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    let mut failures = 0;
    while Instant::now() < deadline {
        // Also how we notice the user closing the window.
        let Some(window) = app.get_webview_window(label) else {
            return Err(AppError::Cancelled);
        };
        // Not `cookies_for_url`: it panics if the window goes away mid-call.
        match window.cookies() {
            Ok(all) => {
                failures = 0;
                if let Some(cookie) = pick_roblosecurity(all) {
                    return Ok(cookie);
                }
            }
            Err(e) => {
                failures += 1;
                if failures == 5 {
                    return Err(e.into());
                }
            }
        }
        std::thread::sleep(POLL);
    }
    Err(AppError::Cancelled)
}

/// The first non-empty `.ROBLOSECURITY` from roblox.com (Tauri strips the leading dot).
fn pick_roblosecurity(cookies: Vec<Cookie<'static>>) -> Option<Secret> {
    cookies
        .into_iter()
        .find(|c| c.name() == ".ROBLOSECURITY" && !c.value().is_empty() && c.domain().is_some_and(is_roblox))
        .map(|c| Secret::new(c.value()))
}

fn is_roblox(domain: &str) -> bool {
    let d = domain.trim_start_matches('.').to_ascii_lowercase();
    d == "roblox.com" || d.ends_with(".roblox.com")
}

async fn cleanup(pid: Option<u32>, profile: PathBuf) {
    let _ = tauri::async_runtime::spawn_blocking(move || {
        if let Some(pid) = pid {
            wait_for_exit(pid);
        }
        remove_profile(&profile);
    })
    .await;
}

/// A profile folder stays locked until its browser process has exited (≤ 5 s here).
fn wait_for_exit(pid: u32) {
    // SAFETY: the handle is used only here and closed before returning.
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            WaitForSingleObject(h, 5_000);
            let _ = CloseHandle(h);
        }
    }
}

/// Child processes can hold files a moment longer, so retry; the start-up sweep catches the rest.
fn remove_profile(dir: &Path) {
    for attempt in 1..=5 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == ErrorKind::NotFound => return,
            Err(e) if attempt == 5 => log::warn!("could not delete login profile {}: {e}", dir.display()),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(name: &str, value: &str, domain: &str) -> Cookie<'static> {
        Cookie::build((name.to_owned(), value.to_owned())).domain(domain.to_owned()).build()
    }

    fn picked(cookies: Vec<Cookie<'static>>) -> Option<String> {
        pick_roblosecurity(cookies).map(|s| s.expose().to_owned())
    }

    #[test]
    fn picks_only_a_non_empty_roblox_cookie() {
        let jar = vec![
            cookie(".ROBLOSECURITY", "fake", "evil.com"),
            cookie(".ROBLOSECURITY", "", ".roblox.com"),
            cookie("GuestData", "guest", ".roblox.com"),
            cookie(".ROBLOSECURITY", "good", ".roblox.com"),
        ];
        assert_eq!(picked(jar), Some("good".into()));
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "w", "www.roblox.com")]), Some("w".into()));
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "x", "notroblox.com")]), None);
        assert_eq!(picked(vec![cookie(".ROBLOSECURITY", "x", "roblox.com.evil.com")]), None);
        assert_eq!(picked(vec![Cookie::new(".ROBLOSECURITY", "no-domain")]), None);
        assert_eq!(picked(vec![cookie(".roblosecurity", "x", "roblox.com")]), None);
        assert_eq!(picked(vec![]), None);
    }

    #[test]
    fn sweep_removes_leftover_profiles() {
        let root = std::env::temp_dir().join(format!("roalt-sweep-{}", uuid::Uuid::new_v4()));
        let tmp = root.join(TMP_DIR);
        std::fs::create_dir_all(tmp.join("a").join("EBWebView")).unwrap();
        std::fs::write(tmp.join("a").join("EBWebView").join("Cookies"), b"x").unwrap();
        std::fs::create_dir_all(tmp.join("b")).unwrap();
        std::fs::write(tmp.join("stray.tmp"), b"x").unwrap();
        sweep_temp_profiles(&root);
        assert_eq!(std::fs::read_dir(&tmp).unwrap().count(), 0);
        sweep_temp_profiles(&root.join("missing"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn clears_webview2_overrides() {
        for name in WEBVIEW2_VARS {
            // SAFETY: nothing else in the test binary reads these variables, and std serialises
            // its own environment access on Windows.
            unsafe { std::env::set_var(name, "x") };
        }
        clear_webview2_env();
        assert!(WEBVIEW2_VARS.iter().all(|n| std::env::var_os(n).is_none()));
    }
}
