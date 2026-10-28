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
use crate::roblox_api::types::{AuthenticatedUser, GameDetails, UserPresence};
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
pub async fn vault_unlock(app: AppHandle, s: State<'_, App>, password: String) -> Result<()> {
    let rec = s.db.vault_record()?.ok_or(AppError::Internal("vault record missing".into()))?;
    s.vault.unlock(&rec, &password)?;
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn vault_lock(app: AppHandle, s: State<'_, App>) {
    if s.vault.mode() == VaultMode::Master {
        lock_vault(&app);
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
    crate::tray::refresh(&app);
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
    if let Some(a) = &patch.launch_args {
        crate::launcher::launch_args(a)?;
    }
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

/// Records a dead or challenged cookie on the account (true when `e` was one).
fn record_cookie_error(app: &AppHandle, s: &App, id: i64, e: &AppError) -> Result<bool> {
    let status = match e {
        AppError::CookieInvalid | AppError::BoundSessionRequired => AccountStatus::Invalid,
        AppError::Challenge => AccountStatus::Challenged,
        _ => return Ok(false),
    };
    s.db.set_status(id, status, now_ms(), None, None)?;
    accounts_changed(app);
    Ok(true)
}

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
pub async fn account_info(app: AppHandle, s: State<'_, App>, id: i64, refresh: bool) -> Result<AccountInfo> {
    let a = s.db.account(id)?;
    let key = format!("info:{}", a.user_id);
    let ttl = s.db.settings()?.cache_minutes as i64 * 60_000;
    if !refresh
        && let Some((json, _)) = s.db.cache_get(&key, ttl, now_ms())?
        && let Ok(mut info) = serde_json::from_str::<AccountInfo>(&json)
    {
        // Presence goes stale long before the rest: re-ask (self-view) once a minute at most.
        if now_ms() - info.fetched_at > 60_000
            && let Ok(rows) = s.api.presence(id, &stored_cookie(&s, id)?, &[a.user_id]).await
        {
            info.presence = rows.into_iter().find(|p| p.user_id == a.user_id).map(presence_of);
        }
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

    // Robux needs the cookie, so it is the probe: a dead or challenged cookie is reported (and
    // recorded) instead of showing a half-empty panel; so is being entirely offline. Neither
    // is cached. Public parts still answer anonymously when the cookie is dead.
    let robux = match robux {
        Ok(r) => Some(r),
        Err(e) => {
            if record_cookie_error(&app, &s, id, &e)? || unavailable.len() == 8 {
                return Err(e);
            }
            None
        }
    };
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
        robux,
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
        // Offline or rate limited: still show the avatars we have.
        match s.api.headshots(&missing).await {
            Ok(urls) => {
                for a in accounts.iter().filter(|a| missing.contains(&a.user_id)) {
                    if let Some(url) = urls.get(&a.user_id) {
                        s.db.cache_put(&format!("headshot:{}", a.user_id), url, now)?;
                        out.push(Headshot { account_id: a.id, url: url.clone() });
                    }
                }
            }
            Err(e) => log::warn!("could not fetch avatars: {e}"),
        }
    }
    Ok(out)
}

/// Presence of the given accounts (the rows on screen), each asked with its own cookie:
/// Roblox hides friends-only presence and game details from other viewers. Accounts whose
/// cookie isn't valid, or whose call fails, are left out.
#[tauri::command]
pub async fn accounts_presence(s: State<'_, App>, ids: Vec<i64>) -> Result<Vec<AccountPresence>> {
    let mut out = Vec::new();
    for a in s.db.accounts()?.into_iter().filter(|a| ids.contains(&a.id) && a.status == AccountStatus::Valid) {
        let Ok(cookie) = stored_cookie(&s, a.id) else { continue };
        match s.api.presence(a.id, &cookie, &[a.user_id]).await {
            Ok(rows) => out.extend(
                rows.into_iter()
                    .find(|p| p.user_id == a.user_id)
                    .map(|p| AccountPresence { account_id: a.id, presence: presence_of(p) }),
            ),
            Err(e) => log::info!("presence of account {} unavailable: {e}", a.id),
        }
    }
    Ok(out)
}

/// Opens roblox.com signed in as the account, in its own throw-away browser profile.
/// When the window closes, the cookie it ended with is checked: if it still signs in as the
/// same user it is kept (Roblox may have rotated it, or the user signed in again or solved a
/// challenge there) and the account is marked valid; a sign-out or account switch there is
/// logged and the stored cookie left as it was.
#[tauri::command]
pub async fn account_open_session(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    let a = s.db.account(id)?;
    let cookie = stored_cookie(&s, id)?;
    let name = if a.alias.is_empty() { a.display_name.clone() } else { a.alias.clone() };
    let title = format!("{name} (@{}) — Roblox — RoALT", a.username);
    let (user_id, handle) = (a.user_id, app.clone());
    webview::open_session(&app, &s.data_dir, &title, cookie, move |last| {
        let Some(last) = last else { return };
        // Runs on a blocking thread before the window is destroyed: finish before the app
        // can exit (closing the last window ends it).
        tauri::async_runtime::block_on(async move {
            let s = handle.state::<App>();
            // The row may have been removed (and its id reused) while the window was open.
            if !s.db.account(id).is_ok_and(|a| a.user_id == user_id) {
                return;
            }
            match s.api.authenticated_user(0, &last).await {
                Ok((u, rotated)) if u.id == user_id => {
                    s.store_rotated(id, rotated.unwrap_or(last));
                    let _ = s.db.set_status(id, AccountStatus::Valid, now_ms(), Some(&u.name), Some(&u.display_name));
                }
                Ok(_) => log::warn!("session window for account {id} ended signed in as another user; cookie not changed"),
                Err(e) => log::warn!("session window for account {id} ended with an unusable cookie ({e}); cookie not changed"),
            }
            accounts_changed(&handle);
        });
    })
    .await
}

// ---------- launching ----------

/// Local only: what pasted text points at.
#[tauri::command]
pub fn target_parse(input: String) -> ParsedInput {
    crate::launcher::target::parse(&input)
}

/// A Roblox username (current or previous) → the user.
#[tauri::command]
pub async fn user_lookup(s: State<'_, App>, name: String) -> Result<UserRef> {
    let name = name.trim().trim_start_matches('@').to_owned();
    if name.is_empty() {
        return Err(AppError::Invalid("Type a Roblox username.".into()));
    }
    let found = s.api.users_by_name(std::slice::from_ref(&name)).await?;
    let u = found.into_iter().next().ok_or(AppError::Invalid(format!("No Roblox user is called \"{name}\".")))?;
    Ok(UserRef { user_id: u.id, username: u.name, display_name: u.display_name })
}

/// Launches one account and starts following its client. `favourite_id` marks that
/// favourite as used.
#[tauri::command]
pub async fn launch(
    app: AppHandle,
    s: State<'_, App>,
    account_id: i64,
    target: LaunchTarget,
    favourite_id: Option<i64>,
) -> Result<LaunchInfo> {
    // Launches go strictly one at a time (DESIGN §6).
    let _single = s.bulk.begin_single_launch()?;
    Ok(launch_tracked(&app, &s, account_id, target, favourite_id, None).await?.0)
}

/// Why launches are refused while `roblox_install` runs.
pub(crate) const INSTALLING: &str = "Roblox is being installed. Launch again once it is done.";

/// One launch as every caller does it: the launch cooldown (`Bulk::cooldown`: waits until the
/// launch delay, `delay_ms` or else the setting's, less its 1 s jitter has passed since the
/// previous launch; a launch that fails before its client starts gives its slot back); stopped
/// by Kill all (`Bulk::kill`) while waiting, and a client started meanwhile is closed; the multi-instance
/// lock first (it has to be held before this client starts, or the next one replaces it), and
/// so the shared login files while protection is on; a dead or challenged cookie recorded on
/// the account; the client tracked; the favourite marked as used. An account whose client is
/// still running is refused (a second client would sign it out of the first).
pub(crate) async fn launch_tracked(
    app: &AppHandle,
    s: &App,
    account_id: i64,
    target: LaunchTarget,
    favourite_id: Option<i64>,
    delay_ms: Option<u32>,
) -> Result<(LaunchInfo, Instance)> {
    let kills = s.bulk.kills();
    let killed = || AppError::Invalid("Launch cancelled by Kill all.".into());
    // Claimed until the client is tracked, so a launch from another path (rejoin, tray,
    // batch) can't start the same account meanwhile.
    let Some(_claim) = s.tracker.claim(account_id) else {
        let name = s.db.account(account_id).map(|a| crate::launcher::bulk::name_of(&a)).ok().filter(|n| !n.is_empty());
        let name = name.as_deref().unwrap_or("This account");
        return Err(AppError::Invalid(format!("{name} is already running. Close its client first.")));
    };
    let _busy = s.bulk.begin_launch();
    // After `_busy`: an install starting now sees this launch, one already running is seen here.
    if s.update.installing() {
        return Err(AppError::Invalid(INSTALLING.into()));
    }
    let v = s.db.settings()?;
    // Less the jitter's second, so a batch's own jittered wait never adds to it.
    let gap = Duration::from_millis(u64::from(delay_ms.unwrap_or(v.launch_delay_ms).saturating_sub(1_000)));
    let (wait, slot) = s.bulk.cooldown(gap);
    tokio::time::sleep(wait).await;
    if s.bulk.kills() != kills {
        s.bulk.release(slot);
        return Err(killed());
    }
    retake_multi(s, &v);
    // Holds the shared files until the client is tracked (the poll releases them when idle).
    let launching = s.multi.launching(v.multi_instance && v.protect_shared_login);
    let info = match crate::launcher::launch(s, account_id, &target).await {
        Ok(info) => info,
        Err(e) => {
            s.bulk.release(slot);
            record_cookie_error(app, s, account_id, &e)?;
            return Err(e);
        }
    };
    let instance = s.tracker.register(&info, target);
    drop(launching);
    instances_changed(app);
    // Kill all pressed while the ticket was fetched: this client goes too.
    if s.bulk.kills() != kills {
        if let Some((pid, started)) = s.tracker.mark_closing(&instance.id) {
            let _ = tauri::async_runtime::spawn_blocking(move || {
                crate::instances::process::close(pid, started, true, Duration::ZERO)
            })
            .await;
        }
        return Err(killed());
    }
    if let Some(f) = favourite_id {
        let _ = s.db.touch_favourite(f, now_ms());
    }
    Ok((info, instance))
}

// ---------- M7: client, data folder, notices ----------

/// Shows a message to the user (a toast; the window may be hidden in the tray).
pub fn notice(app: &AppHandle, error: bool, message: impl Into<String>) {
    let _ = app.emit("app://notice", Notice { error, message: message.into() });
}

/// The Roblox client RoALT would start (`launcher::client::find`; not installed → path None),
/// its version and Roblox's latest (`launcher::update`).
#[tauri::command]
pub fn roblox_client(app: AppHandle) -> RobloxClient {
    crate::launcher::update::publish(&app)
}

/// Downloads Roblox's installer, checks it is signed by Roblox Corporation and runs it
/// (`launcher::update::install`); progress on `roblox://install`. Refused while a client runs.
#[tauri::command]
pub async fn roblox_install(app: AppHandle) -> Result<RobloxClient> {
    crate::launcher::update::install(&app).await
}

/// Where the data lives now (see `main::data_dir`).
#[tauri::command]
pub fn data_dir_info(s: State<'_, App>) -> DataDir {
    DataDir {
        path: s.data_dir.display().to_string(),
        default: crate::same_dir(&s.data_dir, &crate::default_dir()),
        overridden: crate::data_dir_overridden(),
    }
}

/// Opens the data folder in Explorer (opener plugin, Rust side: no capability needed).
#[tauri::command]
pub fn data_dir_open(app: AppHandle, s: State<'_, App>) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    app.opener().open_path(s.data_dir.display().to_string(), None::<&str>).map_err(|e| AppError::Internal(e.to_string()))
}

/// Copies the database into `path` (a new folder or one without a roalt.db; not the current
/// folder or inside it) with SQLite's `VACUUM INTO` (consistent while open), records it in
/// `%LOCALAPPDATA%\RoALT\location.json` as `{"dataDir": path, "movedFrom": current}` and
/// restarts RoALT, all with the database locked so no write lands in the old copy after the
/// copy. The next start deletes the old folder's roalt.db, roalt.db-wal and roalt.db-shm once
/// the new database opened, and drops `movedFrom`. Returns only on failure, after deleting the
/// database files it may have left in `path`. Refused for test runs (`ROALT_DATA_DIR`), when
/// `path` already has a roalt.db, and while a launch, batch or auto-rejoin runs (the restart
/// would cut it off). Async: the copy can take a moment.
#[tauri::command]
pub async fn data_dir_move(app: AppHandle, s: State<'_, App>, path: String) -> Result<()> {
    if crate::data_dir_overridden() {
        return Err(AppError::Invalid("A test run's data folder (ROALT_DATA_DIR) can't be moved.".into()));
    }
    if s.bulk.any_busy() {
        return Err(AppError::Invalid(
            "Wait for the running launch, batch or auto-rejoin to finish, then move the data folder.".into(),
        ));
    }
    let target = crate::move_target(&s.data_dir, &path)?;
    std::fs::create_dir_all(&target)?;
    let location = crate::Location { data_dir: target.clone(), moved_from: Some(s.data_dir.clone()) };
    let failed = s.db.copy_to(&target.join(crate::DB_FILE), || {
        crate::write_location(&location)?;
        log::info!("database copied to {}; restarting to use it", target.display());
        app.restart()
    });
    // A half-written copy would make a retry to the same folder refused.
    for f in crate::DB_FILES {
        let _ = std::fs::remove_file(target.join(f));
    }
    failed
}

// ---------- bulk ----------

/// Starts a batch in the background (see `launcher::bulk::start`); returns its id.
#[tauri::command]
pub fn bulk_launch(app: AppHandle, request: BulkRequest) -> Result<String> {
    crate::launcher::bulk::start(app, request)
}

/// Stops the running batch after its current step.
#[tauri::command]
pub fn bulk_cancel(s: State<'_, App>, batch_id: String) -> Result<()> {
    s.bulk.cancel(&batch_id)
}

/// The running or last batch (None before the first), for a view that opens later.
#[tauri::command]
pub fn bulk_current(s: State<'_, App>) -> Option<BulkProgress> {
    s.bulk.current()
}

// ---------- servers ----------

/// The game behind a place: name, creator, players, icon, whether it sells private servers.
/// Cached for `settings.cache_minutes` (key `game:<place_id>`, JSON of `GameInfo`). No cookie.
/// `NotFound` when Roblox knows no such place (no universe) or no such game.
#[tauri::command]
pub async fn game_info(s: State<'_, App>, place_id: i64) -> Result<GameInfo> {
    let key = format!("game:{place_id}");
    let ttl = s.db.settings()?.cache_minutes as i64 * 60_000;
    if let Some((json, _)) = s.db.cache_get(&key, ttl, now_ms())?
        && let Ok(info) = serde_json::from_str::<GameInfo>(&json)
    {
        return Ok(info);
    }
    // The place icon needs no universe, so it is asked alongside; it is optional.
    let (universe, icons) = tokio::join!(s.api.place_universe(place_id), s.api.place_icons(std::slice::from_ref(&place_id)));
    let universe = universe?.ok_or(AppError::NotFound)?;
    let game = s.api.games(&[universe]).await?.into_iter().find(|g| g.universe_id == universe).ok_or(AppError::NotFound)?;
    let info = game_info_of(place_id, game, icons.ok().and_then(|mut i| i.remove(&place_id)));
    // Without its icon (failed or still being made) it is asked for again next time.
    if info.icon_url.is_some() {
        let json = serde_json::to_string(&info).map_err(|e| AppError::Internal(e.to_string()))?;
        s.db.cache_put(&key, &json, now_ms())?;
    }
    Ok(info)
}

fn game_info_of(place_id: i64, g: GameDetails, icon_url: Option<String>) -> GameInfo {
    GameInfo {
        place_id,
        universe_id: g.universe_id,
        root_place_id: g.root_place_id,
        name: g.name,
        creator: g.creator,
        playing: g.playing,
        visits: g.visits,
        max_players: g.max_players,
        icon_url,
        private_servers: g.private_servers,
    }
}

/// The place a target leads to, for the server browser: its own place, a universe's root
/// place, or where a share link points (resolved with `account_id`'s cookie, which share links
/// need). `Invalid` for targets without a game (a user to follow).
#[tauri::command]
pub async fn target_place(app: AppHandle, s: State<'_, App>, target: LaunchTarget, account_id: Option<i64>) -> Result<i64> {
    let no_game = || AppError::Invalid("That link doesn't lead to a game.".into());
    match &target {
        LaunchTarget::Place { place_id, .. }
        | LaunchTarget::Server { place_id, .. }
        | LaunchTarget::PrivateLink { place_id, .. }
        | LaunchTarget::PrivateAccess { place_id, .. } => Ok(*place_id),
        LaunchTarget::Universe { universe_id } => crate::launcher::root_place(&s.api, *universe_id).await,
        LaunchTarget::ShareLink { .. } | LaunchTarget::ShareLinkV2 { .. } => {
            let id = account_id.ok_or_else(|| AppError::Invalid("Pick an account first: share links open as an account.".into()))?;
            let cookie = stored_cookie(&s, id)?;
            match crate::launcher::resolve(&s.api, id, &cookie, &target).await {
                Ok((join, _)) => join.place_id().ok_or_else(no_game),
                Err(e) => {
                    record_cookie_error(&app, &s, id, &e)?;
                    Err(e)
                }
            }
        }
        LaunchTarget::FollowUser { .. } => Err(no_game()),
    }
}

/// One page of the place's private servers as `account_id` sees them (its cookie; joinable ones
/// carry an access code). `Locked` when the vault is locked.
#[tauri::command]
pub async fn private_servers_list(
    app: AppHandle,
    s: State<'_, App>,
    place_id: i64,
    account_id: i64,
    cursor: Option<String>,
) -> Result<PrivateServerPage> {
    let cookie = stored_cookie(&s, account_id)?;
    let page = s.api.private_servers(account_id, &cookie, place_id, cursor.as_deref()).await;
    if let Err(e) = &page {
        record_cookie_error(&app, &s, account_id, e)?;
    }
    page
}

/// One page of a place's public servers (`cursor` from the previous page). With `account_id`,
/// asked with that account's cookie (needed for Recommended / BestLatency; a guest gets 3
/// requests a minute). `limit` is 10, 25, 50 or 100.
#[tauri::command]
pub async fn servers_list(
    s: State<'_, App>,
    place_id: i64,
    order: ServerOrder,
    exclude_full: bool,
    limit: u8,
    cursor: Option<String>,
    account_id: Option<i64>,
) -> Result<ServerPage> {
    let cookie = account_id.map(|id| stored_cookie(&s, id)).transpose()?;
    let id = account_id.unwrap_or(0);
    s.api.servers_public(id, cookie.as_ref(), place_id, order, exclude_full, limit, cursor.as_deref()).await
}

// ---------- instances ----------

pub fn instances_changed(app: &AppHandle) {
    let _ = app.emit("instances://changed", ());
    crate::tray::refresh(app);
}

#[tauri::command]
pub fn instances_list(s: State<'_, App>) -> Vec<Instance> {
    s.tracker.list()
}

/// Asks one client to close (graceful unless `force`); not treated as an unexpected exit.
#[tauri::command]
pub async fn instance_close(s: State<'_, App>, id: String, force: bool) -> Result<()> {
    s.tracker.get(&id).ok_or(AppError::NotFound)?;
    // Its process already gone: the next poll ends the entry.
    let Some(target) = s.tracker.mark_closing(&id) else { return Ok(()) };
    let r = close_all(vec![target], force, grace(&s)).await?.pop().unwrap_or(Ok(()));
    if r.is_err() {
        s.tracker.clear_closing(&id);
    }
    r
}

/// How long a closed client gets before it is terminated (`settings.close_grace_s`).
fn grace(s: &App) -> Duration {
    Duration::from_secs(u64::from(s.db.settings().map_or(5, |v| v.close_grace_s)))
}

/// Closes the processes at once (each `process::close` waits up to `grace`), so the whole
/// thing takes one grace period, not one per client. Results in input order.
async fn close_all(targets: Vec<(u32, Option<u64>)>, force: bool, grace: Duration) -> Result<Vec<Result<()>>> {
    tauri::async_runtime::spawn_blocking(move || {
        std::thread::scope(|sc| {
            let jobs: Vec<_> = targets
                .into_iter()
                .map(|(pid, t0)| sc.spawn(move || crate::instances::process::close(pid, t0, force, grace)))
                .collect();
            jobs.into_iter()
                .map(|j| j.join().unwrap_or_else(|_| Err(AppError::Internal("close panicked".into()))))
                .collect()
        })
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))
}

#[tauri::command]
pub async fn instances_close_all(s: State<'_, App>, force: bool) -> Result<()> {
    close_running(&s, force).await
}

/// Asks every running client to close (also the tray's "Close all clients").
pub(crate) async fn close_running(s: &App, force: bool) -> Result<()> {
    let running: Vec<Instance> = s.tracker.list().into_iter().filter(|i| i.state != InstanceState::Exited).collect();
    let (ids, targets): (Vec<String>, Vec<_>) =
        running.into_iter().filter_map(|i| Some((i.id.clone(), s.tracker.mark_closing(&i.id)?))).unzip();
    for (id, r) in ids.iter().zip(close_all(targets, force, grace(s)).await?) {
        if let Err(e) = r {
            log::warn!("could not close Roblox client of instance {id}: {e}");
            s.tracker.clear_closing(id);
        }
    }
    Ok(())
}

/// Logical processors (for auto-spread; affinity masks cover at most 64).
pub fn cores() -> u32 {
    std::thread::available_parallelism().map_or(1, |n| n.get() as u32)
}

/// Limits the user can set: a CPU cap of 1-100 %, a trim every 0-1440 minutes, at least one core.
fn check_limits(l: &Limits) -> Result<()> {
    if l.cpu_cap.is_some_and(|c| !(1..=100).contains(&c)) {
        return Err(AppError::Invalid("The CPU limit must be between 1 and 100 %.".into()));
    }
    if l.trim_every_min.is_some_and(|m| m > 24 * 60) {
        return Err(AppError::Invalid("Trimming can be at most once a day apart.".into()));
    }
    if l.affinity == Some(0) {
        return Err(AppError::Invalid("Pick at least one CPU core.".into()));
    }
    Ok(())
}

/// Sets one client's own limits (CPU cap, efficiency mode, trim interval, cores); they win
/// over the adaptive manager's and fill in the defaults (`limits::plan`). A client still behind
/// its bootstrapper gets them once it is found.
#[tauri::command]
pub async fn instance_set_limits(app: AppHandle, s: State<'_, App>, id: String, limits: Limits) -> Result<()> {
    check_limits(&limits)?;
    let (prev, client) = s.tracker.set_limits(&id, limits).ok_or(AppError::NotFound)?;
    instances_changed(&app);
    if client.is_none() {
        return Ok(());
    }
    let app2 = app.clone();
    let r = tauri::async_runtime::spawn_blocking(move || {
        let s = app2.state::<App>();
        let v = s.db.settings().unwrap_or_default();
        let fg = if v.adaptive.enabled { crate::instances::process::foreground_pid() } else { None };
        let plan = crate::instances::limits::plan(&s.tracker.clients(), &v, fg, cores());
        match plan.into_iter().find(|c| c.0 == id) {
            Some((_, pid, t0, l)) => s.limiter.apply(&id, pid, t0, l).map_err(|e| (id, e)),
            None => Ok(()),
        }
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))?;
    if let Err((id, e)) = r {
        s.tracker.set_limits(&id, prev);
        instances_changed(&app);
        return Err(e);
    }
    Ok(())
}

/// Opts one running client in or out of anti-AFK.
#[tauri::command]
pub fn instance_set_anti_afk(app: AppHandle, s: State<'_, App>, id: String, enabled: bool) -> Result<()> {
    if !s.tracker.set_anti_afk(&id, enabled) {
        return Err(AppError::NotFound);
    }
    instances_changed(&app);
    Ok(())
}

/// Pages one client's working set out now ("Trim now").
#[tauri::command]
pub async fn instance_trim(s: State<'_, App>, id: String) -> Result<()> {
    s.tracker.get(&id).ok_or(AppError::NotFound)?;
    let (_, pid, t0, _) = s
        .tracker
        .clients()
        .into_iter()
        .find(|c| c.0 == id)
        .ok_or_else(|| AppError::Invalid("That client hasn't started yet, or has closed.".into()))?;
    tauri::async_runtime::spawn_blocking(move || crate::instances::limits::trim(pid, t0))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))?
}

/// CPU, memory and IO of each running client (sampled at most every 2 s).
#[tauri::command]
pub async fn instances_metrics(app: AppHandle) -> Result<Vec<ClientMetrics>> {
    tauri::async_runtime::spawn_blocking(move || {
        let s = app.state::<App>();
        s.metrics.sample(&s.tracker.clients())
    })
    .await
    .map_err(|e| AppError::Internal(e.to_string()))
}

/// When the bandwidth policy was last applied (the boot it was applied in, as fetched_at).
const QOS_KEY: &str = "qos:applied";

pub(crate) fn bandwidth(s: &App) -> Result<BandwidthStatus> {
    let kbps = s.db.settings()?.bandwidth_kbps;
    let now = now_ms();
    let boot = crate::instances::qos::boot_ms(now);
    let applied = s.db.cache_get(QOS_KEY, i64::MAX / 2, now)?.is_some_and(|(_, at)| crate::instances::qos::applied_this_boot(at, boot));
    Ok(BandwidthStatus { kbps, active: kbps > 0 && applied })
}

/// The Roblox bandwidth limit and whether it is in force (no admin rights needed).
#[tauri::command]
pub fn bandwidth_status(s: State<'_, App>) -> Result<BandwidthStatus> {
    bandwidth(&s)
}

/// Sets the outbound limit for all Roblox clients (kbit/s; 0 removes it) as a Windows QoS
/// policy: Windows asks for admin rights first (`instances::qos::apply`). Saved only once set.
#[tauri::command]
pub async fn bandwidth_apply(s: State<'_, App>, kbps: u32) -> Result<BandwidthStatus> {
    crate::instances::qos::script(kbps)?;
    tauri::async_runtime::spawn_blocking(move || crate::instances::qos::apply(kbps))
        .await
        .map_err(|e| AppError::Internal(e.to_string()))??;
    let mut v = s.db.settings()?;
    v.bandwidth_kbps = kbps;
    s.db.save_settings(&v)?;
    s.db.cache_put(QOS_KEY, "", crate::instances::qos::boot_ms(now_ms()))?;
    bandwidth(&s)
}

#[tauri::command]
pub fn instances_clear_ended(app: AppHandle, s: State<'_, App>) {
    s.tracker.clear_ended();
    instances_changed(&app);
}

#[tauri::command]
pub fn multi_instance_status(s: State<'_, App>) -> Result<MultiInstanceStatus> {
    let v = s.db.settings()?;
    retake_multi(&s, &v);
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

/// Takes the lock when it is wanted but not held and Roblox has since closed (no-op otherwise).
pub(crate) fn retake_multi(s: &App, v: &Settings) {
    if v.multi_instance && !s.multi.is_active() && s.multi.enable().is_ok() {
        protect_running(s, v);
    }
}

/// Locks the shared login files at once when protection is on and clients already run
/// (RoALT's, or ones from before a restart).
fn protect_running(s: &App, v: &Settings) {
    if v.protect_shared_login && (s.multi.in_flight() || s.tracker.any_running() || s.multi.has_survivors()) {
        s.multi.lock_shared();
    }
}

/// Turns multi-instance on or off (saved). Turning it on while Roblox runs saves the setting
/// but reports why the lock isn't held yet; it is taken once Roblox has closed (status check,
/// next launch, next start) or after `roblox_close_all`.
#[tauri::command]
pub fn multi_instance_set(s: State<'_, App>, enabled: bool) -> Result<MultiInstanceStatus> {
    let mut v = s.db.settings()?;
    v.multi_instance = enabled;
    s.db.save_settings(&v)?;
    if enabled {
        match s.multi.enable() {
            Ok(()) => protect_running(&s, &v),
            Err(e) => log::info!("multi-instance not active yet: {e}"),
        }
    } else {
        s.multi.disable();
    }
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

/// Closes every Roblox client, tracked or not (including Roblox's tray process), then takes
/// the multi-instance lock if it is enabled.
#[tauri::command]
pub async fn roblox_close_all(s: State<'_, App>, force: bool) -> Result<MultiInstanceStatus> {
    close_roblox(&s, force, false).await
}

/// Panic button (sidebar, tray "Kill all Roblox"): the running batch stops after its current
/// step, every client counts as closed by the user (no rejoin or restart follows, also for ones
/// waiting their turn), and every Roblox process is terminated at once, tracked or not.
#[tauri::command]
pub async fn panic_kill(s: State<'_, App>) -> Result<MultiInstanceStatus> {
    kill_all(&s).await
}

pub(crate) async fn kill_all(s: &App) -> Result<MultiInstanceStatus> {
    log::info!("kill all Roblox");
    s.bulk.kill();
    s.tracker.mark_all_closing();
    close_roblox(s, true, true).await
}

/// `keep_closing` (Kill all): a client that couldn't be closed still counts as closed by the
/// user, so it is never rejoined or restarted when it ends later.
async fn close_roblox(s: &App, force: bool, keep_closing: bool) -> Result<MultiInstanceStatus> {
    for i in s.tracker.list() {
        s.tracker.mark_closing(&i.id);
    }
    let procs = crate::instances::process::roblox_processes();
    let targets = procs.iter().map(|p| (p.pid, Some(p.start_time))).collect();
    for (p, r) in procs.iter().zip(close_all(targets, force, grace(s)).await?) {
        if let Err(e) = r {
            log::warn!("could not close Roblox process {}: {e}", p.pid);
            // Still running: its exit stays unexpected (not after Kill all).
            for i in s.tracker.list().iter().filter(|i| !keep_closing && i.pid == p.pid && i.state != InstanceState::Exited) {
                s.tracker.clear_closing(&i.id);
            }
        }
    }
    let v = s.db.settings()?;
    retake_multi(s, &v);
    Ok(s.multi.status(v.multi_instance, v.protect_shared_login))
}

/// Clients that crashed, were disconnected or hung, newest first (the last 200).
#[tauri::command]
pub fn crashes_list(s: State<'_, App>) -> Result<Vec<Crash>> {
    s.db.crashes()
}

/// Opens a recorded crash's client log in the default app (opener plugin, Rust side). The path
/// comes from the database, never from the UI.
#[tauri::command]
pub fn crash_open_log(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let path = s.db.crash_log(id)?;
    if !Path::new(&path).is_file() {
        return Err(AppError::Invalid("That client log is gone; Roblox deletes old logs.".into()));
    }
    app.opener().open_path(path, None::<&str>).map_err(|e| AppError::Internal(e.to_string()))
}

const FAVOURITE_AAD: &[u8] = b"roalt.favourite";

pub(crate) fn favourite_of(s: &App, row: crate::storage::FavouriteRow) -> Result<Favourite> {
    let json = s.vault.decrypt(FAVOURITE_AAD, &row.target)?;
    let target = serde_json::from_slice(&json).map_err(|_| AppError::Crypto)?;
    Ok(Favourite {
        id: row.id,
        name: row.name,
        target,
        account_ids: row.account_ids,
        in_tray: row.in_tray,
        sort_order: row.sort_order,
        last_used_at: row.last_used_at,
    })
}

#[tauri::command]
pub fn favourites_list(s: State<'_, App>) -> Result<Vec<Favourite>> {
    s.db.favourites()?.into_iter().map(|r| favourite_of(&s, r)).collect()
}

#[tauri::command]
pub fn favourite_save(app: AppHandle, s: State<'_, App>, input: FavouriteInput) -> Result<Favourite> {
    let json = zeroize::Zeroizing::new(serde_json::to_vec(&input.target).map_err(|e| AppError::Internal(e.to_string()))?);
    let blob = s.vault.encrypt(FAVOURITE_AAD, &json)?;
    let id = s.db.save_favourite(input.id, &input.name, &blob, &input.account_ids, input.in_tray)?;
    crate::tray::refresh(&app);
    let row = s.db.favourites()?.into_iter().find(|f| f.id == id).ok_or(AppError::NotFound)?;
    favourite_of(&s, row)
}

#[tauri::command]
pub fn favourite_remove(app: AppHandle, s: State<'_, App>, id: i64) -> Result<()> {
    s.db.remove_favourite(id)?;
    crate::tray::refresh(&app);
    Ok(())
}

#[tauri::command]
pub fn favourites_reorder(app: AppHandle, s: State<'_, App>, ids: Vec<i64>) -> Result<()> {
    s.db.reorder_favourites(&ids)?;
    crate::tray::refresh(&app);
    Ok(())
}

fn presence_kind(s: Option<&str>) -> PresenceKind {
    match s {
        Some("InGame") => PresenceKind::InGame,
        Some("Online") => PresenceKind::Online,
        Some("InStudio") => PresenceKind::Studio,
        Some("Offline") => PresenceKind::Offline,
        _ => PresenceKind::Unknown,
    }
}

/// Who `account_id` can join: its friends online on Roblox, then the other stored accounts
/// with a valid cookie (each asked about itself). Asked when the picker opens, not polled.
#[tauri::command]
pub async fn join_candidates(app: AppHandle, s: State<'_, App>, account_id: i64) -> Result<Vec<JoinCandidate>> {
    let me = s.db.account(account_id)?;
    let cookie = stored_cookie(&s, account_id)?;
    // Friends are optional: a transient failure still offers the other stored accounts.
    let friends = match s.api.friends_online(account_id, &cookie, me.user_id).await {
        Ok(f) => f,
        Err(e) if record_cookie_error(&app, &s, account_id, &e)? => return Err(e),
        Err(e) => {
            log::info!("friends online for account {account_id} failed: {e}");
            Vec::new()
        }
    };
    let mut out: Vec<JoinCandidate> = friends
        .into_iter()
        .map(|f| {
            let p = f.presence.unwrap_or_default();
            JoinCandidate {
                user_id: f.id,
                username: f.name,
                display_name: f.display_name,
                account_id: None,
                presence: Presence {
                    kind: presence_kind(p.user_presence_type.as_deref()),
                    last_location: p.last_location,
                    place_id: p.place_id,
                    root_place_id: p.root_place_id,
                    universe_id: p.universe_id,
                    game_id: p.game_instance_id,
                },
            }
        })
        .collect();
    for a in s.db.accounts()?.into_iter().filter(|a| a.id != account_id && a.status == AccountStatus::Valid) {
        let Ok(c) = stored_cookie(&s, a.id) else { continue };
        let Ok(rows) = s.api.presence(a.id, &c, &[a.user_id]).await else { continue };
        let Some(p) = rows.into_iter().find(|p| p.user_id == a.user_id) else { continue };
        out.retain(|f| f.user_id != a.user_id);
        out.push(JoinCandidate {
            user_id: a.user_id,
            username: a.username,
            display_name: a.display_name,
            account_id: Some(a.id),
            presence: presence_of(p),
        });
    }
    Ok(out)
}

/// Locks the vault; open session windows close first (they hold a live roblox.com session).
pub fn lock_vault(app: &AppHandle) {
    for (label, w) in app.webview_windows() {
        if label.starts_with("session-") {
            let _ = w.close();
        }
    }
    let s = app.state::<App>();
    s.vault.lock();
    let _ = app.emit("vault://locked", ());
    crate::tray::refresh(app);
}

// ---------- discord ----------

/// Whether a webhook is set, and its id's last digits (never the URL). `Locked` while locked.
#[tauri::command]
pub fn discord_get(s: State<'_, App>) -> Result<DiscordStatus> {
    crate::discord::status(&s)
}

/// Stores a Discord webhook URL in the vault (only discord.com / discordapp.com webhooks);
/// None or "" removes it.
#[tauri::command]
pub fn discord_set(s: State<'_, App>, url: Option<String>) -> Result<DiscordStatus> {
    crate::discord::set(&s, url.as_deref())
}

/// Posts a test message to the webhook.
#[tauri::command]
pub async fn discord_test(s: State<'_, App>) -> Result<()> {
    crate::discord::test(&s).await
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
    if let Some(r) = patch.auto_rejoin { v.auto_rejoin = r }
    if let Some(r) = patch.auto_restart_on_crash { v.auto_restart_on_crash = r }
    if let Some(n) = patch.breaker_max {
        if !(1..=20).contains(&n) {
            return Err(AppError::Invalid("The restart limit must be between 1 and 20.".into()));
        }
        v.breaker_max = n;
    }
    if let Some(m) = patch.breaker_window_min {
        if !(1..=240).contains(&m) {
            return Err(AppError::Invalid("The restart window must be between 1 minute and 4 hours.".into()));
        }
        v.breaker_window_min = m;
    }
    if let Some(g) = patch.close_grace_s {
        if !(1..=60).contains(&g) {
            return Err(AppError::Invalid("The close grace must be between 1 and 60 seconds.".into()));
        }
        v.close_grace_s = g;
    }
    if let Some(a) = patch.launch_args {
        crate::launcher::launch_args(&a)?;
        v.launch_args = a.trim().to_owned();
    }
    if let Some(c) = patch.close_to_tray { v.close_to_tray = c }
    if let Some(m) = patch.mem_restart_mb {
        if m != 0 && !(512..=65_536).contains(&m) {
            return Err(AppError::Invalid("The memory limit must be 0 (off) or 512 to 65 536 MB.".into()));
        }
        v.mem_restart_mb = m;
    }
    if let Some(m) = patch.trim_every_min {
        if m > 24 * 60 {
            return Err(AppError::Invalid("Trimming can be at most once a day apart.".into()));
        }
        v.trim_every_min = m;
    }
    if let Some(a) = patch.auto_spread { v.auto_spread = a }
    if let Some(a) = patch.adaptive {
        check_limits(&a.background)?;
        v.adaptive = a;
    }
    if let Some(a) = patch.anti_afk {
        // Roblox disconnects after 20 idle minutes.
        if !(1..=19).contains(&a.interval_min) {
            return Err(AppError::Invalid("Anti-AFK must press a key every 1 to 19 minutes.".into()));
        }
        v.anti_afk = a;
    }
    if let Some(d) = patch.discord {
        if d.status_every_min != 0 && !(5..=24 * 60).contains(&d.status_every_min) {
            return Err(AppError::Invalid("The status summary must be off (0) or every 5 minutes to 24 hours.".into()));
        }
        v.discord = d;
    }
    if let Some(a) = patch.auto_update { v.auto_update = a }
    if let Some(d) = patch.launch_delay_ms {
        if !(2_000..=60_000).contains(&d) {
            return Err(AppError::Invalid("The launch delay must be between 2 and 60 seconds.".into()));
        }
        v.launch_delay_ms = d;
    }
    // Multi-instance itself goes through multi_instance_set (it has to take or drop the lock).
    if let Some(p) = patch.protect_shared_login
        && p != v.protect_shared_login
    {
        v.protect_shared_login = p;
        if !p {
            s.multi.release_shared();
        } else if v.multi_instance {
            protect_running(&s, &v);
        }
    }
    s.db.save_settings(&v)?;
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::{GameDetails, GameInfo, game_info_of, normalize_cookie};

    #[test]
    fn game_info_keeps_the_asked_place() {
        let g = GameDetails {
            universe_id: 994732206,
            root_place_id: 2753915549,
            name: "Blox Fruits".into(),
            creator: "Gamer Robot Inc".into(),
            playing: 193096,
            visits: 65079604670,
            max_players: 12,
            private_servers: true,
        };
        let info = game_info_of(7449423635, g, Some("https://tr.rbxcdn.com/i".into()));
        let want = GameInfo {
            place_id: 7449423635,
            universe_id: 994732206,
            root_place_id: 2753915549,
            name: "Blox Fruits".into(),
            creator: "Gamer Robot Inc".into(),
            playing: 193096,
            visits: 65079604670,
            max_players: 12,
            icon_url: Some("https://tr.rbxcdn.com/i".into()),
            private_servers: true,
        };
        assert_eq!(info, want);
        // What the cache stores reads back the same.
        assert_eq!(serde_json::from_str::<GameInfo>(&serde_json::to_string(&info).unwrap()).unwrap(), want);
    }

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
