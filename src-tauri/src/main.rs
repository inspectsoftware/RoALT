#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod error;
mod model;
mod roblox_api;
mod storage;
mod vault;
mod webview;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::window::Color;
use tauri::{App as TauriApp, AppHandle, Emitter, Manager, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_log::{Target, TargetKind};

use crate::model::{Theme, VaultMode};
use crate::vault::{Secret, cookie_aad};

/// Everything the commands share.
pub struct App {
    pub db: storage::Db,
    pub vault: vault::Vault,
    pub api: roblox_api::Client,
    pub data_dir: PathBuf,
    /// Last cookie put on the clipboard, until it is cleared.
    pub copied: Mutex<Option<Secret>>,
}

impl App {
    /// Roblox handed out a new `.ROBLOSECURITY` for a stored account: keep it.
    fn store_rotated(&self, account_id: i64, cookie: Secret) {
        let stored = self
            .db
            .account_cookie(account_id)
            .and_then(|(user_id, _)| self.vault.encrypt(&cookie_aad(user_id), cookie.expose().as_bytes()))
            .and_then(|blob| self.db.set_cookie(account_id, &blob));
        match stored {
            Ok(()) => log::info!("stored rotated cookie for account {account_id}"),
            Err(e) => log::warn!("could not store rotated cookie for account {account_id}: {e}"),
        }
    }
}

/// `ROALT_DATA_DIR` (test runs) or `%LOCALAPPDATA%\RoALT`.
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("ROALT_DATA_DIR") {
        return d.into();
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("RoALT")
}

fn main_window(app: &TauriApp, data_dir: &Path, theme: Theme) -> tauri::Result<()> {
    let bg = match theme {
        Theme::Light => Color(242, 243, 240, 255),
        _ => Color(10, 11, 13, 255),
    };
    #[allow(unused_mut)]
    let mut builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("RoALT")
        .inner_size(1280.0, 800.0)
        .min_inner_size(960.0, 600.0)
        .decorations(false)
        .background_color(bg)
        // The UI's own WebView2 profile; never used for roblox.com.
        .data_directory(data_dir.join("webview"));
    // Debug builds only: DevTools protocol for automated UI checks on a hidden desktop.
    #[cfg(debug_assertions)]
    if let Ok(port) = std::env::var("ROALT_CDP_PORT") {
        builder = builder.additional_browser_args(&format!(
            "--remote-debugging-port={port} --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"
        ));
    }
    builder.build()?;
    Ok(())
}

/// Master-password mode: lock after the configured idle time.
fn spawn_auto_lock(handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            let s = handle.state::<App>();
            if s.vault.mode() != VaultMode::Master || s.vault.is_locked() {
                continue;
            }
            let minutes = s.db.settings().map(|v| v.auto_lock_minutes).unwrap_or(15);
            if minutes > 0 && s.vault.idle() >= Duration::from_secs(minutes as u64 * 60) {
                s.vault.lock();
                let _ = handle.emit("vault://locked", ());
            }
        }
    });
}

/// A startup failure the user must see: there is no window to show it in.
fn fatal(message: &str) -> ! {
    use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    log::error!("{message}");
    // SAFETY: a plain modal message box with no owner window.
    unsafe { MessageBoxW(None, &windows::core::HSTRING::from(message), windows::core::w!("RoALT"), MB_ICONERROR | MB_OK) };
    std::process::exit(1)
}

fn init(app: &TauriApp, data_dir: &Path) -> std::result::Result<(), String> {
    webview::sweep_temp_profiles(data_dir);
    let db_path = data_dir.join("roalt.db");
    let db = storage::Db::open(&db_path).map_err(|e| format!("RoALT can't open its database {}: {e}", db_path.display()))?;
    let vault = match db.vault_record().map_err(|e| e.to_string())? {
        Some(rec) => vault::Vault::open(&rec).map_err(|e| {
            format!(
                "RoALT can't open its vault on this Windows account ({e}).\n\nIf this data folder came from another PC or \
                 Windows account, rename {} and import a .roalt backup.",
                db_path.display()
            )
        })?,
        None => {
            let (v, rec) = vault::Vault::create().map_err(|e| e.to_string())?;
            db.save_vault_record(&rec).map_err(|e| e.to_string())?;
            v
        }
    };
    let handle = app.handle().clone();
    let api = roblox_api::Client::new(Arc::new(move |id, cookie| handle.state::<App>().store_rotated(id, cookie)));
    let theme = db.settings().map_err(|e| e.to_string())?.theme;
    app.manage(App { db, vault, api, data_dir: data_dir.to_owned(), copied: Mutex::new(None) });
    main_window(app, data_dir, theme).map_err(|e| {
        format!(
            "RoALT can't open its window ({e}).\n\nIt needs the Microsoft Edge WebView2 Runtime: \
             https://go.microsoft.com/fwlink/p/?LinkId=2124703"
        )
    })?;
    spawn_auto_lock(app.handle().clone());
    Ok(())
}

fn main() {
    webview::clear_webview2_env();
    let data_dir = data_dir();
    if let Err(e) = std::fs::create_dir_all(&data_dir) {
        fatal(&format!("RoALT can't create its data folder {}: {e}", data_dir.display()));
    }
    let log_dir = data_dir.join("logs");

    tauri::Builder::default()
        // Must be first: a second launch focuses the running window instead.
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .target(Target::new(TargetKind::Folder { path: log_dir, file_name: Some("roalt".into()) }))
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_window_state::Builder::new().with_filter(|label| label == "main").build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(move |app| {
            if let Err(message) = init(app, &data_dir) {
                fatal(&message);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::vault_status,
            commands::vault_unlock,
            commands::vault_lock,
            commands::vault_set_mode,
            commands::app_activity,
            commands::accounts_list,
            commands::account_add_cookie,
            commands::account_add_login,
            commands::account_update,
            commands::accounts_reorder,
            commands::accounts_remove,
            commands::accounts_validate,
            commands::account_reveal_cookie,
            commands::account_copy_cookie,
            commands::groups_list,
            commands::group_save,
            commands::group_remove,
            commands::backup_export,
            commands::backup_import,
            commands::settings_get,
            commands::settings_set,
        ])
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| fatal(&format!("RoALT failed to start: {e}")))
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                commands::clear_copied_cookie(app);
            }
        });
}
