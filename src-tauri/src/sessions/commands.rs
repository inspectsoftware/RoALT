//! The Sessions page's commands (the control API proxies are in `remote`). Each one emits
//! `sessions://changed` when it changed something the list shows.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, State};

use super::types::{HealthCheck, PlanUser, SessionConfig, SessionControl, SessionView, SetupAction, SetupPlan, SetupResult, WtsState};
use super::{MAX_SESSIONS, checked, creds, health, rdp, reg, remote, secret_key, setup, slot, slot_of, users, window, wts};
use crate::App;
use crate::error::{AppError, Result};
use crate::vault::Secret;

/// Settings-table key of the signing certificate's SHA-1 thumbprint (the last setup's).
const THUMBPRINT: &str = "altdock.sessions.thumbprint";
/// How long `session_start` waits for mstsc's window to hide it.
const WINDOW_WAIT: Duration = Duration::from_secs(60);

fn changed(app: &AppHandle) {
    let _ = app.emit("sessions://changed", ());
}

/// Runs Windows calls that may wait off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Internal(e.to_string()))?
}

fn sessions(s: &App) -> Result<Vec<SessionConfig>> {
    Ok(s.db.settings()?.sessions)
}

pub(super) fn session(s: &App, name: &str) -> Result<SessionConfig> {
    sessions(s)?.into_iter().find(|c| c.name == name).ok_or(AppError::NotFound)
}

/// A session's stored secret (`password` or `token`); None when none was made yet.
pub(super) fn stored_secret(s: &App, name: &str, kind: &str) -> Result<Option<Secret>> {
    let key = secret_key(name, kind);
    let Some(stored) = s.db.setting_value(&key)? else { return Ok(None) };
    let blob = crate::discord::unhex(&stored).ok_or(AppError::Crypto)?;
    s.vault.decrypt_secret(key.as_bytes(), &blob).map(Some)
}

/// The stored secret, or a new one from `make`, stored in the vault first.
fn secret(s: &App, name: &str, kind: &str, make: fn() -> zeroize::Zeroizing<String>) -> Result<Secret> {
    if let Some(found) = stored_secret(s, name, kind)? {
        return Ok(found);
    }
    let fresh = make();
    let key = secret_key(name, kind);
    let blob = s.vault.encrypt(key.as_bytes(), fresh.as_bytes())?;
    s.db.set_setting_value(&key, Some(&crate::discord::hex(&blob)))?;
    log::info!("made the {kind} of session {name}");
    Ok(Secret::new(fresh.as_str()))
}

/// Writes the session's `.rdp` file unless the one there (maybe signed since) says the same.
fn ensure_rdp(data_dir: &Path, c: &SessionConfig) -> Result<PathBuf> {
    let path = rdp::path(data_dir, &c.name);
    let built = rdp::build(c, &creds::user_name(&c.windows_user));
    if std::fs::read(&path).is_ok_and(|b| rdp::same_settings(&rdp::decode(&b), &built)) {
        return Ok(path);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, rdp::encode(&built))?;
    Ok(path)
}

#[tauri::command]
pub async fn sessions_health(s: State<'_, App>) -> Result<Vec<HealthCheck>> {
    let list = sessions(&s)?;
    let data_dir = s.data_dir.clone();
    let thumbprint = s.db.setting_value(THUMBPRINT)?;
    blocking(move || Ok(health::checks(&list, &data_dir, thumbprint.as_deref()))).await
}

/// Sets this PC up for the sessions named (all when none is named): one admin prompt.
#[tauri::command]
pub async fn sessions_setup(app: AppHandle, s: State<'_, App>, names: Vec<String>) -> Result<SetupResult> {
    let all = sessions(&s)?;
    if let Some(unknown) = names.iter().find(|n| !all.iter().any(|c| &c.name == *n)) {
        return Err(AppError::Invalid(tr!("There is no session {name}.", name = unknown)));
    }
    let chosen: Vec<&SessionConfig> = all.iter().filter(|c| names.is_empty() || names.contains(&c.name)).collect();
    let mut local = SetupResult::default();
    // The main user's own setting: not something the elevated side may write for them.
    local.push(
        "suppressMinimized",
        reg::set_dword(reg::HKCU, reg::RDP_CLIENT, "RemoteDesktop_SuppressWhenMinimized", 2).map(|()| String::new()),
    );
    let mut plan_users = Vec::new();
    for c in &chosen {
        let password = secret(&s, &c.name, "password", super::new_password)?;
        let token = secret(&s, &c.name, "token", super::new_token)?;
        local.push(format!("credentials:{}", c.name), creds::write(&c.address, &c.windows_user, password.expose()).map(|()| String::new()).map_err(|e| e.message()));
        plan_users.push(PlanUser {
            name: c.windows_user.clone(),
            password: zeroize::Zeroizing::new(password.expose().to_owned()),
            port: c.port,
            token: zeroize::Zeroizing::new(token.expose().to_owned()),
            mode: c.mode,
        });
    }
    let mut rdp_files = Vec::new();
    for c in &all {
        rdp_files.push(ensure_rdp(&s.data_dir, c)?);
    }
    let plan = SetupPlan {
        action: SetupAction::Setup,
        main_user_sid: users::current_user_sid().ok_or_else(|| AppError::Internal("could not read the current user's SID".into()))?,
        managed_users: all.iter().map(|c| c.windows_user.clone()).collect(),
        users: plan_users,
        remove_users: Vec::new(),
        rdp_files,
        thumbprint: s.db.setting_value(THUMBPRINT)?,
        system: true,
        delete_users: false,
    };
    let data_dir = s.data_dir.clone();
    let mut result = blocking(move || setup::run(&data_dir, &plan)).await?;
    if let Some(t) = &result.thumbprint {
        s.db.set_setting_value(THUMBPRINT, Some(t))?;
    }
    local.steps.append(&mut result.steps);
    local.thumbprint = result.thumbprint;
    log::info!("session setup: {} of {} steps ok{}", local.steps.iter().filter(|s| s.ok).count(), local.steps.len(), if local.ok() { "" } else { " (see below)" });
    for step in local.steps.iter().filter(|s| !s.ok) {
        log::warn!("session setup step {} failed: {}", step.id, step.detail);
    }
    changed(&app);
    Ok(local)
}

/// The sessions with their sign-in state, window and whether their AltDock answers. Every
/// session's AltDock is asked at once, each for at most `remote::HEALTH_WAIT`, while Windows is
/// asked for the rest.
#[tauri::command]
pub async fn sessions_list(s: State<'_, App>) -> Result<Vec<SessionView>> {
    let list = sessions(&s)?;
    let asked: Vec<_> = list.iter().map(|c| tauri::async_runtime::spawn(remote::control_of(remote::client(&s, c)))).collect();
    let local = blocking(move || {
        let seen = wts::sessions();
        Ok(list
            .into_iter()
            .map(|config| {
                let win = window::find(&config.address);
                let state = wts::state_of(&seen, &config.windows_user);
                (config, state, win.is_some_and(window::is_visible), win.is_some())
            })
            .collect::<Vec<_>>())
    })
    .await?;
    let mut views = Vec::with_capacity(local.len());
    for ((config, wts, window_visible, window_open), control) in local.into_iter().zip(asked) {
        let control = control.await.unwrap_or_else(|e| SessionControl::Down { reason: e.to_string() });
        views.push(SessionView { config, wts, window_visible, window_open, control });
    }
    Ok(views)
}

/// Adds a session or changes one (same name); its slot (user, address, port) never changes.
#[tauri::command]
pub fn session_save(app: AppHandle, s: State<'_, App>, config: SessionConfig) -> Result<SessionConfig> {
    let mut v = s.db.settings()?;
    let c = checked(config, &v.sessions)?;
    match v.sessions.iter_mut().find(|o| o.name == c.name) {
        Some(old) => *old = c.clone(),
        None => v.sessions.push(c.clone()),
    }
    s.db.save_settings(&v)?;
    log::info!("session {} saved ({})", c.name, c.windows_user);
    changed(&app);
    Ok(c)
}

/// Removes a session: its window, sign-in, `.rdp` file and secrets; with `delete_user` also its
/// Windows user and profile (one admin prompt, first: declining it removes nothing). The answer
/// is that setup's result.
#[tauri::command]
pub async fn session_remove(app: AppHandle, s: State<'_, App>, name: String, delete_user: bool) -> Result<Option<SetupResult>> {
    let c = session(&s, &name)?;
    let mut result = None;
    if delete_user {
        let user = c.windows_user.clone();
        if blocking(move || Ok(wts::state_of(&wts::sessions(), &user))).await? != WtsState::None {
            return Err(AppError::Invalid(tr!("Log the session off first: its Windows user is still signed in.")));
        }
        let plan = SetupPlan {
            main_user_sid: users::current_user_sid().ok_or_else(|| AppError::Internal("could not read the current user's SID".into()))?,
            managed_users: sessions(&s)?.into_iter().filter(|o| o.name != c.name).map(|o| o.windows_user).collect(),
            remove_users: vec![c.windows_user.clone()],
            system: false,
            ..Default::default()
        };
        let data_dir = s.data_dir.clone();
        result = Some(blocking(move || setup::run(&data_dir, &plan)).await?);
    }
    if let Some(w) = window::find(&c.address) {
        window::close(w)?;
    }
    creds::delete(&c.address)?;
    let _ = std::fs::remove_file(rdp::path(&s.data_dir, &c.name));
    for kind in ["password", "token"] {
        s.db.set_setting_value(&secret_key(&c.name, kind), None)?;
    }
    let mut v = s.db.settings()?;
    v.sessions.retain(|o| o.name != c.name);
    s.db.save_settings(&v)?;
    log::info!("session {name} removed{}", if delete_user { " with its Windows user" } else { "" });
    changed(&app);
    Ok(result)
}

/// "Remove from this PC": one admin prompt undoes every machine-wide change the setup made
/// (firewall rule, autostart, signing certificates, policies and settings back to what they were,
/// `%ProgramData%\AltDock`), with `delete_users` the alt users and their profiles too. Then this
/// user's side: the sessions' Credential Manager entries, `.rdp` files and secrets, and the session
/// list. Refused while an alt user is signed in; declining the prompt removes nothing.
#[tauri::command]
pub async fn sessions_uninstall(app: AppHandle, s: State<'_, App>, delete_users: bool) -> Result<SetupResult> {
    let all = sessions(&s)?;
    let signed_in: Vec<String> = blocking(|| {
        Ok(wts::sessions().into_iter().filter(|w| w.state != WtsState::None && slot_of(&w.user).is_some()).map(|w| w.user).collect())
    })
    .await?;
    if !signed_in.is_empty() {
        return Err(AppError::Invalid(tr!("Log every session off first: {users} still signed in.", users = signed_in.join(", "))));
    }
    for c in &all {
        if let Some(w) = window::find(&c.address) {
            let _ = window::close(w);
        }
    }
    let plan = SetupPlan {
        action: SetupAction::Uninstall,
        main_user_sid: users::current_user_sid().ok_or_else(|| AppError::Internal("could not read the current user's SID".into()))?,
        managed_users: all.iter().map(|c| c.windows_user.clone()).collect(),
        delete_users,
        ..Default::default()
    };
    let data_dir = s.data_dir.clone();
    let mut result = blocking(move || setup::run(&data_dir, &plan)).await?;

    // This user's side, which the elevated run can't reach.
    let mut gone = Vec::new();
    let mut failed = Vec::new();
    for n in 1..=MAX_SESSIONS {
        match creds::delete_ours(&slot(n).1) {
            Ok(true) => gone.push(slot(n).1),
            Ok(false) => {}
            Err(e) => failed.push(e.to_string()),
        }
    }
    result.push("credentials", if failed.is_empty() { Ok(gone.join(", ")) } else { Err(failed.join("; ")) });
    result.push("sessionFiles", remove_rdp_files(&s.data_dir));
    for c in &all {
        for kind in ["password", "token"] {
            s.db.set_setting_value(&secret_key(&c.name, kind), None)?;
        }
    }
    s.db.set_setting_value(THUMBPRINT, None)?;
    let mut v = s.db.settings()?;
    v.sessions.clear();
    s.db.save_settings(&v)?;
    log::info!("removed from this PC{}: {} of {} steps ok", if delete_users { " with the alt users" } else { "" }, result.steps.iter().filter(|s| s.ok).count(), result.steps.len());
    for step in result.steps.iter().filter(|s| !s.ok) {
        log::warn!("removal step {} failed: {}", step.id, step.detail);
    }
    changed(&app);
    Ok(result)
}

/// Deletes the `.rdp` files in the data folder's `sessions` folder (only AltDock writes there),
/// and the folder once it is empty.
fn remove_rdp_files(data_dir: &Path) -> std::result::Result<String, String> {
    let dir = data_dir.join("sessions");
    let mut failed = Vec::new();
    for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("rdp"))
            && e.file_type().is_ok_and(|t| t.is_file())
            && let Err(err) = std::fs::remove_file(&p)
        {
            failed.push(format!("{}: {err}", p.display()));
        }
    }
    let _ = std::fs::remove_dir(&dir);
    if failed.is_empty() { Ok(String::new()) } else { Err(failed.join("; ")) }
}

/// Opens the session in a hidden mstsc window. Refused while RDP Wrapper doesn't support this
/// Windows build: a mismatched wrapper can take over the user's own session.
#[tauri::command]
pub async fn session_start(app: AppHandle, s: State<'_, App>, name: String) -> Result<()> {
    let c = session(&s, &name)?;
    blocking(|| health::wrapper_ready().map_err(AppError::Invalid)).await?;
    if window::find(&c.address).is_some() {
        return Ok(());
    }
    let password = stored_secret(&s, &c.name, "password")?.ok_or_else(|| AppError::Invalid(tr!("Run the setup for this session first.")))?;
    creds::write(&c.address, &c.windows_user, password.expose())?;
    drop(password);
    let file = ensure_rdp(&s.data_dir, &c)?;
    let root = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| r"C:\Windows".into());
    std::process::Command::new(root.join(r"System32\mstsc.exe"))
        .arg(&file)
        .spawn()
        .map_err(|e| AppError::Internal(tr!("Could not start Remote Desktop: {why}", why = e)))?;
    log::info!("session {name} starting");
    let address = c.address.clone();
    std::thread::spawn(move || {
        let deadline = Instant::now() + WINDOW_WAIT;
        while Instant::now() < deadline {
            if let Some(w) = window::find(&address) {
                window::set_visible(w, false);
                break;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        // mstsc may show its window again once connected.
        let settle = Instant::now() + Duration::from_secs(5);
        while Instant::now() < settle {
            if let Some(w) = window::find(&address).filter(|w| window::is_visible(*w)) {
                window::set_visible(w, false);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        changed(&app);
    });
    Ok(())
}

/// Shows or hides the session's window; whether it is shown now.
#[tauri::command]
pub fn session_window(app: AppHandle, s: State<'_, App>, name: String, show: bool) -> Result<bool> {
    let c = session(&s, &name)?;
    let w = window::find(&c.address).ok_or_else(|| AppError::Invalid(tr!("The session isn't open.")))?;
    window::set_visible(w, show);
    changed(&app);
    Ok(window::is_visible(w))
}

/// Disconnects the session (its clients keep running): ends its mstsc.
#[tauri::command]
pub fn session_disconnect(app: AppHandle, s: State<'_, App>, name: String) -> Result<()> {
    let c = session(&s, &name)?;
    if let Some(w) = window::find(&c.address) {
        window::close(w)?;
        log::info!("session {name} disconnected");
    }
    changed(&app);
    Ok(())
}
