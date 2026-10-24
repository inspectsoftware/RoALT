//! Roblox login / session windows on throw-away WebView2 profiles.

use std::path::Path;

use tauri::AppHandle;

use crate::error::Result;
use crate::vault::Secret;

/// Opens `https://www.roblox.com/login` in a `login-<uuid>` window on a fresh profile under
/// `<data_dir>/tmp-webview/<uuid>` (incognito, no devtools, no autofill, no context menu,
/// no IPC capability). A worker thread polls `cookies()` every ~700 ms until a non-empty
/// `.ROBLOSECURITY` appears, then closes the window, waits for its browser process to exit
/// and deletes the profile. Closing the window first → `AppError::Cancelled`.
pub async fn capture_login(app: &AppHandle, data_dir: &Path) -> Result<Secret> {
    let _ = (app, data_dir);
    todo!("webview: capture_login")
}

/// Deletes leftover `tmp-webview/*` profiles from earlier runs.
pub fn sweep_temp_profiles(data_dir: &Path) {
    let _ = data_dir;
    todo!("webview: sweep_temp_profiles")
}

/// Removes `WEBVIEW2_*` override variables so they cannot redirect or merge profiles.
/// Called first thing in `main`, before any thread starts.
pub fn clear_webview2_env() {
    todo!("webview: clear_webview2_env")
}
