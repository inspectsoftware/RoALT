// TODO(merge): needs Stream B. Written against the assumed `control::client::ControlClient` API
// (`ControlClient::new(port, Zeroizing<String>)`, one async method per endpoint, each returning
// `crate::error::Result<_>`), Stream B's `control::types` and Stream A's macro/anti-AFK types. It is
// left out of the build (`// mod remote;` in sessions/mod.rs) until those are merged; then enable
// the module, register the commands listed in main.rs's Stream C block and use `control_of` in
// `commands::sessions_list`.
//! The control API proxies: the Sessions page talks to the AltDock in a session through these,
//! with that session's token from the vault. Nothing here is elevated.

use tauri::State;
use zeroize::Zeroizing;

use super::commands::{session, stored_secret};
use super::types::{SessionConfig, SessionControl};
use crate::App;
use crate::control::client::ControlClient;
// TODO(merge): Stream B's names for the state rows and accounts; Stream A's for these two.
use crate::control::types::ControlClient as ClientState;
use crate::error::{AppError, Result};
use crate::instances::afk::AfkClientConfig;
use crate::vault::backup::{self, BackupAccount, BackupGroup, BackupPayload};
use crate::vault::cookie_aad;

/// A client for the session's control API; `Invalid` until its setup made a token.
fn client(s: &App, c: &SessionConfig) -> Result<ControlClient> {
    let token = stored_secret(s, &c.name, "token")?.ok_or_else(|| AppError::Invalid(tr!("Run the setup for this session first.")))?;
    Ok(ControlClient::new(c.port, Zeroizing::new(token.expose().to_owned())))
}

fn client_for(s: &App, name: &str) -> Result<ControlClient> {
    client(s, &session(s, name)?)
}

/// Whether the AltDock in that session answers (`sessions_list`).
pub async fn control_of(s: &App, c: &SessionConfig) -> SessionControl {
    let client = match client(s, c) {
        Ok(client) => client,
        Err(e) => return SessionControl::Down { reason: e.message() },
    };
    match client.health().await {
        Ok(health) => SessionControl::Ok { health },
        Err(e) => SessionControl::Down { reason: e.message() },
    }
}

/// Standard base64 with padding (pure), for the pushed backup.
pub fn base64(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Signs the session out through its AltDock, which closes its clients first.
#[tauri::command]
pub async fn session_logoff(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.logoff().await?;
    log::info!("session {name} logging off");
    Ok(())
}

/// Copies these accounts (with their cookies) into the session's vault: an encrypted backup with
/// a one-time random password, sent over loopback with the session's token. The number imported.
#[tauri::command]
pub async fn session_push_accounts(s: State<'_, App>, name: String, account_ids: Vec<i64>) -> Result<u32> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let client = client_for(&s, &name)?;
    let groups = s.db.groups()?;
    let name_of = |gid: i64| groups.iter().find(|g| g.id == gid).map(|g| g.name.clone());
    let mut accounts = Vec::new();
    for a in s.db.accounts()?.into_iter().filter(|a| account_ids.contains(&a.id)) {
        let (user_id, blob) = s.db.account_cookie(a.id)?;
        let cookie = s.vault.decrypt_secret(&cookie_aad(user_id), &blob)?;
        accounts.push(BackupAccount {
            user_id: a.user_id,
            username: a.username,
            display_name: a.display_name,
            alias: a.alias,
            notes: a.notes,
            color: a.color,
            favourite: a.favourite,
            sort_order: a.sort_order,
            groups: a.group_ids.iter().filter_map(|g| name_of(*g)).collect(),
            cookie: cookie.expose().to_owned().into(),
            allow_disconnect: Some(a.allow_disconnect),
            launch_args: a.launch_args,
            discord_muted: a.discord_muted,
            fps_cap: a.fps_cap,
            full_quality: a.full_quality,
            main: false,
        });
    }
    if accounts.is_empty() {
        return Err(AppError::Invalid(tr!("Pick the accounts to send to the session.")));
    }
    let payload = BackupPayload {
        version: 1,
        exported_at: crate::commands::now_ms(),
        accounts,
        groups: groups.iter().map(|g| BackupGroup { name: g.name.clone(), color: g.color.clone(), sort_order: g.sort_order }).collect(),
        favourites: Vec::new(),
    };
    let password = super::new_token();
    // TODO(merge): use Stream B's `vault::backup` to-bytes writer when it has one.
    let tmp = s.data_dir.join(format!("push-{}.tmp", &super::new_token()[..16]));
    let written = backup::write(&tmp, &password, &payload).and_then(|()| Ok(std::fs::read(&tmp)?));
    let _ = std::fs::remove_file(&tmp);
    let bytes = Zeroizing::new(written?);
    let imported = client.import_accounts(base64(&bytes), password.to_string()).await?;
    log::info!("pushed {imported} account(s) to session {name}");
    Ok(imported)
}

/// Copies these macros into the session (pushed macros count as trusted there).
#[tauri::command]
pub async fn session_push_macros(s: State<'_, App>, name: String, macro_ids: Vec<i64>) -> Result<u32> {
    let client = client_for(&s, &name)?;
    // TODO(merge): Stream A's `macros::list` and `MacroInput` fields.
    let chosen: Vec<_> = crate::macros::list(&s)?.into_iter().filter(|m| macro_ids.contains(&m.id)).collect();
    let mut pushed = 0;
    for m in chosen {
        client.save_macro(crate::model::MacroInput { id: None, name: m.name, body: m.body, hotkey: String::new(), ..Default::default() }).await?;
        pushed += 1;
    }
    Ok(pushed)
}

#[tauri::command]
pub async fn remote_state(s: State<'_, App>, name: String) -> Result<Vec<ClientState>> {
    client_for(&s, &name)?.state().await
}

/// A client's thumbnail, in the bytes format `instance_thumbnail` answers with.
#[tauri::command]
pub async fn remote_thumbnail(s: State<'_, App>, name: String, id: String, max_w: u32) -> Result<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(client_for(&s, &name)?.thumbnail(&id, max_w.clamp(16, 1280)).await?))
}

/// Launches these (session-side) accounts to `target` (parsed like the Launch box); how many started.
#[tauri::command]
pub async fn remote_launch(s: State<'_, App>, name: String, account_ids: Vec<i64>, target: String) -> Result<u32> {
    client_for(&s, &name)?.launch(account_ids, target).await
}

#[tauri::command]
pub async fn remote_close(s: State<'_, App>, name: String, id: String, force: bool) -> Result<()> {
    client_for(&s, &name)?.close(&id, force).await
}

#[tauri::command]
pub async fn remote_kill_all(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.kill_all().await
}

#[tauri::command]
pub async fn remote_afk(s: State<'_, App>, name: String, account_id: i64, config: AfkClientConfig) -> Result<()> {
    client_for(&s, &name)?.set_afk(account_id, config).await
}

#[tauri::command]
pub async fn remote_macro_assign(s: State<'_, App>, name: String, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<()> {
    client_for(&s, &name)?.assign_macro(account_id, macro_id, autorun).await
}

#[tauri::command]
pub async fn remote_macro_run(s: State<'_, App>, name: String, macro_id: i64, instance_ids: Vec<String>) -> Result<()> {
    client_for(&s, &name)?.run_macro(macro_id, instance_ids).await
}

#[tauri::command]
pub async fn remote_macro_stop(s: State<'_, App>, name: String, instance_id: String) -> Result<()> {
    client_for(&s, &name)?.stop_macro(&instance_id).await
}

/// Starts the verified Roblox install for that session's Windows user.
#[tauri::command]
pub async fn remote_roblox_install(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.install_roblox().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_like_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xFF, 0xFE, 0x00]), "//4A");
    }
}
