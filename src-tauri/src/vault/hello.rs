//! Windows Hello confirmation (DPAPI-mode re-authentication).

use crate::error::Result;

/// Shows the Windows Hello prompt over `window`. `Ok(())` only when the user is verified.
/// Not set up / cancelled / failed → `AppError::Invalid` with a message saying what to do
/// (e.g. "Windows Hello isn't set up. Set a master password to reveal cookies.").
pub async fn windows_hello(window: &tauri::WebviewWindow, message: &str) -> Result<()> {
    let _ = (window, message);
    todo!("hello: windows_hello")
}
