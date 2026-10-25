//! The tray icon (`tray-icon` feature): left click shows the window; the menu has Show RoALT,
//! Favourites (those with `in_tray`), Close all clients, Lock vault (master-password mode),
//! Quit.

use tauri::AppHandle;

/// Builds the tray icon (the app's window icon, tooltip "RoALT") and its menu; menu events go
/// to `on_menu`. Called once from setup.
pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    let _ = app;
    todo!()
}

/// Rebuilds the menu: favourites in tray order by name (ids `fav:<id>`; disabled with the label
/// "Unlock RoALT to launch" while the vault is locked; "No favourites" when none), "Close all
/// clients" (disabled when none run), "Lock vault" (only in master-password mode, disabled
/// while locked). Called after favourites change, the vault locks or unlocks, and clients
/// start or end. Never fails loudly: errors are logged.
pub fn refresh(app: &AppHandle) {
    let _ = app;
    todo!()
}

/// Menu ids: "show" → show, unminimize and focus the main window; "close-all" →
/// `commands::instances_close_all(force: false)` logic; "lock" → `commands::lock_vault`;
/// "quit" → exit the app; "fav:<id>" → `launch_favourite`.
fn on_menu(app: &AppHandle, id: &str) {
    let _ = (app, id);
    todo!()
}

/// Launches a favourite from the tray with its stored accounts that have a valid cookie: one →
/// `commands::launch_tracked` (the favourite marked used); several → `launcher::bulk::start`
/// with the launch delay and `SameServer::PickFirst`; none → show the window with an
/// `app://notice` saying to pick accounts for it in RoALT. Errors become `app://notice`
/// (error), also when the window is hidden.
fn launch_favourite(app: &AppHandle, id: i64) {
    let _ = (app, id);
    todo!()
}
