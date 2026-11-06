//! The control API proxies: the Sessions page talks to the AltDock in a session through these
//! (`control::client`), with that session's token from the vault. Nothing here is elevated.
//!
//! Account ids: a session has its own database, so its account ids aren't the main app's. The
//! commands that take ids from the session's own answers (`remote_state`: `accountId`,
//! `instanceId`; `remote_macros`: `id`) send them back as they are; `remote_launch` and
//! `session_push_accounts` take the main app's ids and match them by Roblox user.

use std::time::Duration;

use tauri::State;
use zeroize::Zeroizing;

use super::commands::{session, stored_secret};
use super::types::{SessionConfig, SessionControl};
use crate::App;
use crate::control::client::ControlClient;
use crate::control::types::{ControlClient as ClientState, ImportAnswer, LaunchAnswer};
use crate::error::{AppError, Result};
use crate::model::{AfkClientConfig, Macro, MacroInput};
use crate::vault::backup::{self, BackupAccount, BackupGroup, BackupPayload};

/// How long `sessions_list` waits for a session's AltDock: the page asks every few seconds.
const HEALTH_WAIT: Duration = Duration::from_millis(1500);

/// A client for the session's control API; `Invalid` until its setup made a token.
pub(super) fn client(s: &App, c: &SessionConfig) -> Result<ControlClient> {
    let token = stored_secret(s, &c.name, "token")?.ok_or_else(|| AppError::Invalid(tr!("Run the setup for this session first.")))?;
    ControlClient::new(c.port, Zeroizing::new(token.expose().to_owned()))
}

fn client_for(s: &App, name: &str) -> Result<ControlClient> {
    client(s, &session(s, name)?)
}

/// Whether the AltDock in that session answers (`sessions_list`), within `HEALTH_WAIT`.
pub(super) async fn control_of(client: Result<ControlClient>) -> SessionControl {
    match client {
        Ok(client) => match client.health(HEALTH_WAIT).await {
            Ok(health) => SessionControl::Ok { health },
            Err(e) => SessionControl::Down { reason: e.message() },
        },
        Err(e) => SessionControl::Down { reason: e.message() },
    }
}

/// The session's own ids of these main-app accounts, matched by Roblox user (pure). Err: the
/// names of those the session doesn't have.
pub fn session_ids(wanted: &[(i64, String)], session: &[(i64, i64)]) -> std::result::Result<Vec<i64>, Vec<String>> {
    let (mut ids, mut missing) = (Vec::new(), Vec::new());
    for (user_id, name) in wanted {
        match session.iter().find(|(_, u)| u == user_id) {
            Some((id, _)) => ids.push(*id),
            None => missing.push(name.clone()),
        }
    }
    if missing.is_empty() { Ok(ids) } else { Err(missing) }
}

/// Signs the session out through its AltDock, which closes its clients first.
#[tauri::command]
pub async fn session_logoff(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.logoff().await?;
    log::info!("session {name} logging off");
    Ok(())
}

/// Copies these accounts (main-app ids, with their cookies) into the session's vault: an
/// encrypted backup with a one-time random password, sent over loopback with the session's token.
#[tauri::command]
pub async fn session_push_accounts(s: State<'_, App>, name: String, account_ids: Vec<i64>) -> Result<ImportAnswer> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let client = client_for(&s, &name)?;
    let groups = s.db.groups()?;
    let name_of = |gid: i64| groups.iter().find(|g| g.id == gid).map(|g| g.name.clone());
    let mut accounts = Vec::new();
    for a in s.db.accounts()?.into_iter().filter(|a| account_ids.contains(&a.id)) {
        let cookie = crate::launcher::account_cookie(&s, a.id)?;
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
            // The main account stays the main app's.
            main: false,
        });
    }
    if accounts.is_empty() {
        return Err(AppError::Invalid(tr!("Pick the accounts to send to the session.")));
    }
    // Only the groups these accounts are in.
    let groups = groups
        .iter()
        .filter(|g| accounts.iter().any(|a| a.groups.contains(&g.name)))
        .map(|g| BackupGroup { name: g.name.clone(), color: g.color.clone(), sort_order: g.sort_order })
        .collect();
    let payload = BackupPayload {
        version: 1,
        exported_at: crate::commands::now_ms(),
        accounts,
        groups,
        favourites: Vec::new(),
        macros: Vec::new(),
    };
    let password = super::new_token();
    let pw = password.clone();
    // Argon2id takes a moment: off the async runtime.
    let bytes = tauri::async_runtime::spawn_blocking(move || backup::to_bytes(&pw, &payload).map(Zeroizing::new))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    let answer = client.import_accounts(&bytes, password).await?;
    log::info!("pushed {} account(s) to session {name}, {} failed", answer.imported, answer.failed);
    Ok(answer)
}

/// Copies these macros (main-app ids) into the session, where pushed macros count as trusted, so
/// only ones trusted here go; one of the same name there is replaced. Without hotkeys: nobody
/// presses keys in a session. How many were copied.
#[tauri::command]
pub async fn session_push_macros(s: State<'_, App>, name: String, macro_ids: Vec<i64>) -> Result<u32> {
    let client = client_for(&s, &name)?;
    let chosen: Vec<Macro> = crate::macros::list(&s)?.into_iter().filter(|m| m.trusted && macro_ids.contains(&m.id)).collect();
    let mut pushed = 0;
    for m in chosen {
        client.macro_save(&MacroInput { id: None, name: m.name, body: m.body, hotkey: String::new() }).await?;
        pushed += 1;
    }
    log::info!("pushed {pushed} macro(s) to session {name}");
    Ok(pushed)
}

/// The clients running in the session (ids are the session's own).
#[tauri::command]
pub async fn remote_state(s: State<'_, App>, name: String) -> Result<Vec<ClientState>> {
    client_for(&s, &name)?.state().await
}

/// The session's own macros (their ids are the session's).
#[tauri::command]
pub async fn remote_macros(s: State<'_, App>, name: String) -> Result<Vec<Macro>> {
    client_for(&s, &name)?.macros().await
}

/// A client's thumbnail, in the bytes `instance_thumbnail` answers with (`known`: the picture
/// number the caller has; then only the header comes).
#[tauri::command]
pub async fn remote_thumbnail(s: State<'_, App>, name: String, id: String, max_w: u32, known: u32) -> Result<tauri::ipc::Response> {
    Ok(tauri::ipc::Response::new(client_for(&s, &name)?.thumbnail(&id, max_w.clamp(16, 1280), known).await?))
}

/// Launches these accounts (main-app ids; pushed to the session before) to `target`, parsed in
/// the session like the Launch box does.
#[tauri::command]
pub async fn remote_launch(s: State<'_, App>, name: String, account_ids: Vec<i64>, target: String) -> Result<LaunchAnswer> {
    let client = client_for(&s, &name)?;
    let wanted: Vec<(i64, String)> =
        s.db.accounts()?.into_iter().filter(|a| account_ids.contains(&a.id)).map(|a| (a.user_id, a.username)).collect();
    if wanted.is_empty() {
        return Err(AppError::Invalid(tr!("Pick at least one account.")));
    }
    let there: Vec<(i64, i64)> = client.accounts().await?.into_iter().map(|a| (a.id, a.user_id)).collect();
    let ids = session_ids(&wanted, &there)
        .map_err(|missing| AppError::Invalid(tr!("Push these accounts to the session first: {names}", names = missing.join(", "))))?;
    client.launch(ids, target).await
}

/// Closes a client (the session's instance id), gracefully unless `force`.
#[tauri::command]
pub async fn remote_close(s: State<'_, App>, name: String, id: String, force: bool) -> Result<()> {
    client_for(&s, &name)?.close(id, force).await
}

#[tauri::command]
pub async fn remote_kill_all(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.kill_all().await
}

/// Anti-AFK of an account in the session (`account_id`: the session's, from `remote_state`).
#[tauri::command]
pub async fn remote_afk(s: State<'_, App>, name: String, account_id: i64, config: AfkClientConfig) -> Result<()> {
    client_for(&s, &name)?.afk(account_id, config).await
}

/// The macro of an account in the session (both ids the session's; None: none).
#[tauri::command]
pub async fn remote_macro_assign(s: State<'_, App>, name: String, account_id: i64, macro_id: Option<i64>, autorun: bool) -> Result<()> {
    client_for(&s, &name)?.macro_assign(account_id, macro_id, autorun).await
}

#[tauri::command]
pub async fn remote_macro_run(s: State<'_, App>, name: String, macro_id: i64, instance_ids: Vec<String>) -> Result<()> {
    client_for(&s, &name)?.macro_run(macro_id, instance_ids).await
}

#[tauri::command]
pub async fn remote_macro_stop(s: State<'_, App>, name: String, instance_id: String) -> Result<()> {
    client_for(&s, &name)?.macro_stop(instance_id).await
}

/// Starts the verified Roblox install for that session's Windows user; `sessions_list` shows
/// when it is there (`robloxInstalled`).
#[tauri::command]
pub async fn remote_roblox_install(s: State<'_, App>, name: String) -> Result<()> {
    client_for(&s, &name)?.roblox_install().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_accounts_by_roblox_user() {
        let wanted = [(100, "alpha".to_string()), (200, "beta".to_string())];
        assert_eq!(session_ids(&wanted, &[(7, 200), (5, 100), (9, 300)]), Ok(vec![5, 7]));
        assert_eq!(session_ids(&wanted, &[(5, 100)]), Err(vec!["beta".to_string()]));
        assert_eq!(session_ids(&wanted, &[]), Err(vec!["alpha".to_string(), "beta".to_string()]));
        assert_eq!(session_ids(&[], &[(5, 100)]), Ok(vec![]));
    }
}
