//! The IPC surface. Each command validates input, calls the modules and emits change events.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_clipboard_manager::ClipboardExt;

use crate::error::{AppError, Result};
use crate::model::*;
use crate::storage::{AccountMeta, NewAccount};
use crate::vault::backup::{self, BackupAccount, BackupGroup, BackupPayload};
use crate::vault::{Secret, cookie_aad};
use crate::roblox_api::types::{AuthenticatedUser, UserPresence};
use crate::{App, webview};

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn accounts_changed(app: &AppHandle) {
    let _ = app.emit("accounts://changed", ());
}

// ---------- vault ----------

#[tauri::command]
pub fn vault_status(s: State<'_, App>) -> Result<VaultStatus> {
    Ok(VaultStatus {
        mode: s.vault.mode(),
        locked: s.vault.is_locked(),
        accounts: s.db.accounts()?.len() as u32,
        auto_lock_minutes: s.db.settings()?.auto_lock_minutes,
    })
}

#[tauri::command]
pub async fn vault_unlock(s: State<'_, App>, password: String) -> Result<()> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    s.vault.unlock(&rec, &password)
}

#[tauri::command]
pub fn vault_lock(app: AppHandle, s: State<'_, App>) {
    if s.vault.mode() == VaultMode::Master {
        s.vault.lock();
        let _ = app.emit("vault://locked", ());
    }
}

#[tauri::command]
pub async fn vault_set_mode(
    app: AppHandle,
    s: State<'_, App>,
    mode: VaultMode,
    current_password: Option<String>,
    new_password: Option<String>,
) -> Result<()> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    // Otherwise setting a master password would be a way round the Hello check on reveal.
    if rec.mode == VaultMode::Dpapi && mode == VaultMode::Master && crate::vault::hello_available().await {
        reauth(&app, &s, None, "Confirm it's you to set a master password.").await?;
    }
    let new_rec = s.vault.rewrap(&rec, mode, current_password.as_deref(), new_password.as_deref())?;
    s.db.save_vault_record(&new_rec)?;
    s.vault.set_mode(mode);
    Ok(())
}

/// Called by the UI (throttled) on user input; drives auto-lock.
#[tauri::command]
pub fn app_activity(s: State<'_, App>) {
    s.vault.touch();
}

// ---------- accounts ----------

#[tauri::command]
pub fn accounts_list(s: State<'_, App>) -> Result<Vec<AccountSummary>> {
    s.db.accounts()
}

/// Accepts a raw value, `.ROBLOSECURITY=value`, quoted or with a trailing `;`.
fn normalize_cookie(raw: &str) -> Result<Secret> {
    let v = raw.trim().trim_matches(|c| c == '"' || c == '\'').trim();
    let v = v.strip_prefix(".ROBLOSECURITY=").unwrap_or(v).trim_end_matches(';').trim();
    if v.is_empty() || v.len() > 8192 || !v.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(AppError::Invalid("That doesn't look like a .ROBLOSECURITY cookie.".into()));
    }
    Ok(Secret::new(v))
}

/// Validates a cookie that is not stored yet and saves it as a new or refreshed account.
async fn add_account(app: &AppHandle, s: &App, cookie: Secret) -> Result<AccountSummary> {
    let (user, rotated): (AuthenticatedUser, _) = s.api.authenticated_user(0, &cookie).await?;
    let cookie = rotated.unwrap_or(cookie);
    let blob = s.vault.encrypt(&cookie_aad(user.id), cookie.expose().as_bytes())?;
    let id = s.db.upsert_account(&NewAccount {
        user_id: user.id,
        username: &user.name,
        display_name: &user.display_name,
        cookie: &blob,
        status: AccountStatus::Valid,
        checked_at: Some(now_ms()),
    })?;
    accounts_changed(app);
    s.db.account(id)
}

#[tauri::command]
pub async fn account_add_cookie(app: AppHandle, s: State<'_, App>, cookie: String) -> Result<AccountSummary> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    add_account(&app, &s, normalize_cookie(&cookie)?).await
}

#[tauri::command]
pub async fn account_add_login(app: AppHandle, s: State<'_, App>) -> Result<AccountSummary> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let cookie = webview::capture_login(&app, &s.data_dir).await?;
    add_account(&app, &s, cookie).await
}

#[tauri::command]
pub fn account_update(app: AppHandle, s: State<'_, App>, id: i64, patch: AccountPatch) -> Result<AccountSummary> {
    s.db.update_account(id, &patch)?;
    accounts_changed(&app);
    s.db.account(id)
}

#[tauri::command]
pub fn accounts_reorder(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    s.db.reorder_accounts(&ids)?;
    accounts_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn accounts_remove(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    s.db.remove_accounts(&ids)?;
    accounts_changed(&app);
    Ok(())
}

/// Checks each cookie with Roblox (one at a time; the API client throttles).
/// `None` checks every account. Returns the full, updated list.
#[tauri::command]
pub async fn accounts_validate(app: AppHandle, s: State<'_, App>, ids: Option<Vec<i64>>) -> Result<Vec<AccountSummary>> {
    let ids = match ids {
        Some(ids) => ids,
        None => s.db.accounts()?.iter().map(|a| a.id).collect(),
    };
    let (mut answered, mut last_err) = (0, None);
    for id in ids {
        // Removed while a long run was going: skip it.
        let (user_id, blob) = match s.db.account_cookie(id) {
            Err(AppError::NotFound) => continue,
            r => r?,
        };
        let cookie = s.vault.decrypt_secret(&cookie_aad(user_id), &blob)?;
        let now = now_ms();
        let stored = match s.api.authenticated_user(id, &cookie).await {
            Ok((u, _)) if u.id == user_id => {
                s.db.set_status(id, AccountStatus::Valid, now, Some(&u.name), Some(&u.display_name))
            }
            // The cookie signs in as someone else (e.g. an account switch in a browser).
            Ok(_) => s.db.set_status(id, AccountStatus::Invalid, now, None, None),
            Err(AppError::CookieInvalid | AppError::BoundSessionRequired) => {
                s.db.set_status(id, AccountStatus::Invalid, now, None, None)
            }
            Err(AppError::Challenge) => s.db.set_status(id, AccountStatus::Challenged, now, None, None),
            Err(e) => {
                log::warn!("could not validate account {id}: {e}");
                last_err = Some(e);
                continue;
            }
        };
        match stored {
            Err(AppError::NotFound) => continue,
            r => r?,
        }
        answered += 1;
    }
    accounts_changed(&app);
    match last_err {
        Some(e) if answered == 0 => Err(e),
        _ => s.db.accounts(),
    }
}

/// Master mode asks for the password again; DPAPI mode asks Windows Hello.
async fn reauth(app: &AppHandle, s: &App, password: Option<String>, prompt: &str) -> Result<()> {
    match s.vault.mode() {
        VaultMode::Master => {
            let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
            s.vault.check_password(&rec, password.as_deref().ok_or(AppError::WrongPassword)?)
        }
        VaultMode::Dpapi => {
            let window = app.get_webview_window("main").ok_or(AppError::Internal("no main window".into()))?;
            crate::vault::windows_hello(&window, prompt).await
        }
    }
}

const REVEAL_PROMPT: &str = "Confirm it's you to reveal a Roblox cookie.";

fn stored_cookie(s: &App, id: i64) -> Result<Secret> {
    let (user_id, blob) = s.db.account_cookie(id)?;
    s.vault.decrypt_secret(&cookie_aad(user_id), &blob)
}

#[tauri::command]
pub async fn account_reveal_cookie(app: AppHandle, s: State<'_, App>, id: i64, password: Option<String>) -> Result<String> {
    reauth(&app, &s, password, REVEAL_PROMPT).await?;
    Ok(stored_cookie(&s, id)?.expose().to_owned())
}

/// Copies from Rust so the cookie never passes through the UI. `write_secret` keeps it out of
/// Windows clipboard history and cloud sync; the clipboard is cleared after the configured
/// delay, or at exit (`clear_copied_cookie`) if that comes first.
#[tauri::command]
pub async fn account_copy_cookie(app: AppHandle, s: State<'_, App>, id: i64, password: Option<String>) -> Result<()> {
    reauth(&app, &s, password, REVEAL_PROMPT).await?;
    let secs = s.db.settings()?.clipboard_clear_seconds.max(5);
    let cookie = stored_cookie(&s, id)?;
    app.clipboard().write_secret(cookie.expose().to_owned()).map_err(|e| AppError::Internal(e.to_string()))?;
    *s.copied.lock().unwrap_or_else(|e| e.into_inner()) = Some(cookie);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs as u64)).await;
        clear_copied_cookie(&app);
    });
    Ok(())
}

/// Clears the clipboard if it still holds the last copied cookie.
pub fn clear_copied_cookie(app: &AppHandle) {
    let s = app.state::<App>();
    let Some(cookie) = s.copied.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    if app.clipboard().read_text().is_ok_and(|t| t == cookie.expose()) {
        let _ = app.clipboard().clear();
    }
}

// ---------- account info ----------

const HEADSHOT_TTL_MS: i64 = 6 * 60 * 60 * 1000;

fn presence_of(p: UserPresence) -> Presence {
    Presence {
        kind: PresenceKind::from_code(p.user_presence_type),
        last_location: p.last_location,
        place_id: p.place_id,
        root_place_id: p.root_place_id,
        universe_id: p.universe_id,
        game_id: p.game_id,
    }
}

/// The info panel. Served from the cache unless `refresh` or older than `cache_minutes`.
/// Each part is fetched on its own; parts Roblox doesn't answer are listed in `unavailable`.
#[tauri::command]
pub async fn account_info(s: State<'_, App>, id: i64, refresh: bool) -> Result<AccountInfo> {
    let a = s.db.account(id)?;
    let key = format!("info:{}", a.user_id);
    let ttl = s.db.settings()?.cache_minutes as i64 * 60_000;
    if !refresh
        && let Some((json, _)) = s.db.cache_get(&key, ttl, now_ms())?
        && let Ok(info) = serde_json::from_str::<AccountInfo>(&json)
    {
        return Ok(info);
    }
    let cookie = stored_cookie(&s, id)?;
    let uid = a.user_id;
    let one = [uid];
    let (details, heads, robux, premium, plus, counts, presence, email, two_step) = tokio::join!(
        s.api.user_details(id, Some(&cookie), uid),
        s.api.headshots(&one),
        s.api.robux(id, &cookie),
        s.api.premium(id, &cookie, uid),
        s.api.plus(&one),
        s.api.social_counts(uid),
        s.api.presence(id, &cookie, &one),
        s.api.email_verified(id, &cookie),
        s.api.two_step_methods(id, &cookie, uid),
    );
    let mut unavailable = Vec::new();
    let mut part = |name: &str, ok: bool| {
        if !ok {
            unavailable.push(name.to_owned());
        }
    };
    part("profile", details.is_ok());
    part("avatar", heads.as_ref().is_ok_and(|h| h.contains_key(&uid)));
    part("robux", robux.is_ok());
    part("friends", counts.is_ok());
    part("presence", presence.as_ref().is_ok_and(|p| !p.is_empty()));
    part("email", email.is_ok());
    part("2-step", two_step.is_ok());
    let subscription = match (plus.as_ref().ok().and_then(|p| p.get(&uid).copied()), premium.as_ref().ok().copied()) {
        (Some(true), _) => Some(Subscription::Plus),
        (_, Some(true)) => Some(Subscription::Premium),
        (Some(false), _) => Some(Subscription::None),
        (None, _) => None,
    };
    part("subscription", subscription.is_some());

    // Everything failed the same way (offline, cookie dead): say why instead of an empty panel.
    if unavailable.len() == 8
        && let Err(e) = robux
    {
        return Err(e);
    }
    let details = details.ok();
    let counts = counts.ok();
    let info = AccountInfo {
        account_id: id,
        user_id: uid,
        username: details.as_ref().map_or(a.username.clone(), |d| d.name.clone()),
        display_name: details.as_ref().map_or(a.display_name.clone(), |d| d.display_name.clone()),
        description: details.as_ref().map(|d| d.description.clone()).unwrap_or_default(),
        created: details.as_ref().and_then(|d| d.created.clone()),
        is_banned: details.as_ref().is_some_and(|d| d.is_banned),
        verified_badge: details.as_ref().is_some_and(|d| d.has_verified_badge),
        headshot_url: heads.ok().and_then(|mut h| h.remove(&uid)),
        robux: robux.ok(),
        subscription,
        friends: counts.map(|c| c.friends),
        followers: counts.map(|c| c.followers),
        following: counts.map(|c| c.following),
        email_verified: email.ok(),
        two_step_methods: two_step.ok(),
        presence: presence.ok().and_then(|p| p.into_iter().find(|p| p.user_id == uid)).map(presence_of),
        fetched_at: now_ms(),
        unavailable,
    };
    let json = serde_json::to_string(&info).map_err(|e| AppError::Internal(e.to_string()))?;
    s.db.cache_put(&key, &json, info.fetched_at)?;
    if let Some(url) = &info.headshot_url {
        s.db.cache_put(&format!("headshot:{uid}"), url, info.fetched_at)?;
    }
    Ok(info)
}

/// Avatar URLs for every account (cached 6 h); one batched call for the missing ones.
#[tauri::command]
pub async fn accounts_headshots(s: State<'_, App>, refresh: bool) -> Result<Vec<Headshot>> {
    let now = now_ms();
    let accounts = s.db.accounts()?;
    let mut out = Vec::new();
    let mut missing = Vec::new();
    for a in &accounts {
        match s.db.cache_get(&format!("headshot:{}", a.user_id), HEADSHOT_TTL_MS, now)? {
            Some((url, _)) if !refresh => out.push(Headshot { account_id: a.id, url }),
            _ => missing.push(a.user_id),
        }
    }
    if !missing.is_empty() {
        let urls = s.api.headshots(&missing).await?;
        for a in accounts.iter().filter(|a| missing.contains(&a.user_id)) {
            if let Some(url) = urls.get(&a.user_id) {
                s.db.cache_put(&format!("headshot:{}", a.user_id), url, now)?;
                out.push(Headshot { account_id: a.id, url: url.clone() });
            }
        }
    }
    Ok(out)
}

/// Presence of every account in one batched call, seen by the first valid account.
/// Game details are only filled in where Roblox lets that viewer see them; the info panel
/// asks each account about itself. No valid account → empty.
// ponytail: one viewer for the whole list; query per account if hidden game details matter.
#[tauri::command]
pub async fn accounts_presence(s: State<'_, App>) -> Result<Vec<AccountPresence>> {
    let accounts = s.db.accounts()?;
    let Some(viewer) = accounts.iter().find(|a| a.status == AccountStatus::Valid) else {
        return Ok(Vec::new());
    };
    let cookie = stored_cookie(&s, viewer.id)?;
    let ids: Vec<i64> = accounts.iter().map(|a| a.user_id).collect();
    let rows = s.api.presence(viewer.id, &cookie, &ids).await?;
    Ok(rows
        .into_iter()
        .filter_map(|p| {
            let account_id = accounts.iter().find(|a| a.user_id == p.user_id)?.id;
            Some(AccountPresence { account_id, presence: presence_of(p) })
        })
        .collect())
}

/// Opens roblox.com signed in as the account, in its own throw-away browser profile.
/// When the window closes, a rotated cookie is kept if it still signs in as the same user;
/// a sign-out or account switch there is logged and the stored cookie left as it was.
#[tauri::command]
pub async fn account_open_session(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    let a = s.db.account(id)?;
    let cookie = stored_cookie(&s, id)?;
    let name = if a.alias.is_empty() { a.display_name.clone() } else { a.alias.clone() };
    let title = format!("{name} (@{}) — Roblox — RoALT", a.username);
    let (user_id, handle) = (a.user_id, app.clone());
    let original = cookie.clone();
    webview::open_session(&app, &s.data_dir, &title, cookie, move |last| {
        let Some(last) = last.filter(|c| c.expose() != original.expose()) else { return };
        tauri::async_runtime::spawn(async move {
            let s = handle.state::<App>();
            match s.api.authenticated_user(0, &last).await {
                Ok((u, _)) if u.id == user_id => s.store_rotated(id, last),
                Ok(_) => log::warn!("session window for account {id} ended signed in as another user; cookie not changed"),
                Err(e) => log::warn!("session window for account {id} ended with an unusable cookie ({e}); cookie not changed"),
            }
            accounts_changed(&handle);
        });
    })
    .await
}

// ---------- groups ----------

#[tauri::command]
pub fn groups_list(s: State<'_, App>) -> Result<Vec<Group>> {
    s.db.groups()
}

#[tauri::command]
pub fn group_save(app: AppHandle, s: State<'_, App>, input: GroupInput) -> Result<Group> {
    let g = s.db.save_group(&input)?;
    accounts_changed(&app);
    Ok(g)
}

#[tauri::command]
pub fn group_remove(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    s.db.remove_group(id)?;
    accounts_changed(&app);
    Ok(())
}

// ---------- backup ----------

const MIN_BACKUP_PASSWORD: usize = 8;

#[tauri::command]
pub async fn backup_export(
    app: AppHandle,
    s: State<'_, App>,
    path: String,
    password: String,
    current_password: Option<String>,
    account_ids: Option<Vec<i64>>,
) -> Result<u32> {
    if password.chars().count() < MIN_BACKUP_PASSWORD {
        return Err(AppError::Invalid(format!("Use at least {MIN_BACKUP_PASSWORD} characters for the backup password.")));
    }
    // Exporting hands out every cookie, so it needs the same check as reveal.
    reauth(&app, &s, current_password, "Confirm it's you to export your Roblox accounts.").await?;
    let groups = s.db.groups()?;
    let name_of = |gid: i64| groups.iter().find(|g| g.id == gid).map(|g| g.name.clone());
    let mut accounts = Vec::new();
    for a in s.db.accounts()? {
        if account_ids.as_ref().is_some_and(|ids| !ids.contains(&a.id)) {
            continue;
        }
        let cookie = stored_cookie(&s, a.id)?;
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
        });
    }
    let count = accounts.len() as u32;
    let payload = BackupPayload {
        version: 1,
        exported_at: now_ms(),
        accounts,
        groups: groups.iter().map(|g| BackupGroup { name: g.name.clone(), color: g.color.clone(), sort_order: g.sort_order }).collect(),
    };
    backup::write(Path::new(&path), &password, &payload)?;
    Ok(count)
}

#[tauri::command]
pub async fn backup_import(app: AppHandle, s: State<'_, App>, path: String, password: String, on_conflict: ConflictPolicy) -> Result<ImportReport> {
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let payload = backup::read(Path::new(&path), &password)?;
    for g in &payload.groups {
        s.db.group_by_name_or_create(&g.name, g.color.as_deref())?;
    }
    let mut report = ImportReport::default();
    for a in &payload.accounts {
        let existing = s.db.account_id_for_user(a.user_id)?;
        if existing.is_some() && on_conflict == ConflictPolicy::Skip {
            report.skipped += 1;
            continue;
        }
        let blob = s.vault.encrypt(&cookie_aad(a.user_id), a.cookie.as_bytes())?;
        let id = s.db.upsert_account(&NewAccount {
            user_id: a.user_id,
            username: &a.username,
            display_name: &a.display_name,
            cookie: &blob,
            status: AccountStatus::Unknown,
            checked_at: None,
        })?;
        s.db.set_meta(id, &AccountMeta { alias: a.alias.clone(), notes: a.notes.clone(), color: a.color.clone(), favourite: a.favourite })?;
        let gids = a.groups.iter().map(|n| s.db.group_by_name_or_create(n, None)).collect::<Result<Vec<_>>>()?;
        s.db.set_account_groups(id, &gids)?;
        if existing.is_some() { report.updated += 1 } else { report.added += 1 }
    }
    accounts_changed(&app);
    Ok(report)
}

// ---------- settings ----------

#[tauri::command]
pub fn settings_get(s: State<'_, App>) -> Result<Settings> {
    s.db.settings()
}

#[tauri::command]
pub fn settings_set(s: State<'_, App>, patch: SettingsPatch) -> Result<Settings> {
    let mut v = s.db.settings()?;
    if let Some(t) = patch.theme { v.theme = t }
    if let Some(d) = patch.density { v.density = d }
    if let Some(m) = patch.auto_lock_minutes {
        if m > 24 * 60 {
            return Err(AppError::Invalid("Auto-lock can be at most 24 hours.".into()));
        }
        v.auto_lock_minutes = m;
    }
    if let Some(c) = patch.clipboard_clear_seconds {
        if !(5..=300).contains(&c) {
            return Err(AppError::Invalid("Clipboard clearing must be between 5 and 300 seconds.".into()));
        }
        v.clipboard_clear_seconds = c;
    }
    if let Some(c) = patch.cache_minutes {
        if !(1..=24 * 60).contains(&c) {
            return Err(AppError::Invalid("Cache time must be between 1 minute and 24 hours.".into()));
        }
        v.cache_minutes = c;
    }
    s.db.save_settings(&v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::normalize_cookie;

    #[test]
    fn cookie_input_forms() {
        for raw in ["abc123", " abc123 ", "\"abc123\"", ".ROBLOSECURITY=abc123;", "'.ROBLOSECURITY=abc123'"] {
            assert_eq!(normalize_cookie(raw).unwrap().expose(), "abc123", "{raw}");
        }
        assert!(normalize_cookie("").is_err());
        assert!(normalize_cookie("has space").is_err());
        assert!(normalize_cookie(".ROBLOSECURITY=").is_err());
    }
}
