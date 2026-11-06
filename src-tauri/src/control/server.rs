//! The control API a session-mode AltDock serves for the main app (`control::client`): plain
//! HTTP/1.1 on `127.0.0.1:<port>` only, JSON in and out (but the thumbnail's raw bytes), bodies up
//! to 1 MB. Built from RoALT 0.6.7's LAN dashboard (`remote.rs`), without its TLS, PIN and LAN.
//!
//! - Loopback is shared by every Windows session, so every request needs the session's token in
//!   `X-AltDock-Token`, compared in constant time; the token never leaves the config file and
//!   this process, and is never logged.
//! - `Host` must be exactly `127.0.0.1:<port>`: a website rebound to 127.0.0.1 (DNS rebinding)
//!   names its own host, and a browser page can't add the token header without CORS (never
//!   allowed).
//! - Answers are never cached, sniffed or framed.
//! - It does what the UI does, through the same code: a launch is a batch like the Launch view's.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use super::types::*;
use super::{ControlConfig, TOKEN_HEADER};
use crate::App;
use crate::error::{AppError, Result};
use crate::model::{BulkRequest, ConflictPolicy, InstanceState, LaunchTarget, ParsedInput, SameServer, Settings};

/// Largest request body: a backup of a few hundred accounts.
const BODY_LIMIT: usize = 1 << 20;
/// Every answer loads nothing and can't be framed.
const CSP: &str = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";
/// How long `roblox_installed` trusts its last look (it reads the registry and the disk).
const INSTALLED_FOR: Duration = Duration::from_secs(30);

/// What every handler gets.
#[derive(Clone)]
struct Ctl {
    app: AppHandle,
    port: u16,
    token: Arc<Zeroizing<String>>,
    mode: ControlMode,
    installed: Arc<Mutex<Option<(Instant, bool)>>>,
}

/// The running server (managed state); `stop` ends it.
#[derive(Default)]
pub struct Running(Mutex<Option<oneshot::Sender<()>>>);

/// Session mode's settings: multi-instance on (an alt session exists to run several clients),
/// and closing the window keeps AltDock in the tray, so the control API stays up. Saved when
/// they change; a failure to save is logged and the forced values still apply to this start.
pub fn session_settings(db: &crate::storage::Db, mut v: Settings) -> Settings {
    if !v.multi_instance || !v.close_to_tray {
        v.multi_instance = true;
        v.close_to_tray = true;
        if let Err(e) = db.save_settings(&v) {
            log::warn!("session mode: could not save the settings: {e}");
        }
    }
    v
}

/// Session mode's window: hidden in the tray from the start, no splash; the tray's Show opens it
/// at its default size, centered.
pub fn hidden_window(w: &tauri::WebviewWindow) -> tauri::Result<()> {
    w.set_min_size(Some(tauri::LogicalSize::new(crate::splash::MIN.0, crate::splash::MIN.1)))?;
    w.center()
}

/// Serves the control API on `127.0.0.1:<config.port>` until `stop` (or the app ends).
pub fn start(app: &AppHandle, config: &ControlConfig) -> Result<()> {
    let port = config.port;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))
        .map_err(|e| AppError::Internal(format!("control API: couldn't open 127.0.0.1:{port}: {e}")))?;
    listener.set_nonblocking(true)?;
    let (stop, stopped) = oneshot::channel::<()>();
    if !app.manage(Running(Mutex::new(Some(stop)))) {
        return Err(AppError::Internal("control API: already started".into()));
    }
    let ctl = Ctl {
        app: app.clone(),
        port,
        token: Arc::new(config.token.clone()),
        mode: config.mode,
        installed: Arc::default(),
    };
    let router = routes(ctl);
    tauri::async_runtime::spawn(async move {
        let served = async {
            let listener = tokio::net::TcpListener::from_std(listener)?;
            axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
        };
        if let Err(e) = served.await {
            log::error!("control API failed: {e}");
        }
    });
    log::info!("control API serving on 127.0.0.1:{port}");
    Ok(())
}

/// Stops the server, if it runs (the app is exiting).
pub fn stop(app: &AppHandle) {
    if let Some(r) = app.try_state::<Running>()
        && r.0.lock().unwrap_or_else(|e| e.into_inner()).take().is_some()
    {
        log::info!("control API stopped");
    }
}

fn routes(ctl: Ctl) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route("/v1/state", get(state))
        .route("/v1/accounts", get(accounts))
        .route("/v1/accounts/import", post(import))
        .route("/v1/launch", post(launch))
        .route("/v1/close", post(close))
        .route("/v1/kill-all", post(kill_all))
        .route("/v1/thumbnail/{id}", get(thumbnail))
        .route("/v1/afk", post(afk))
        .route("/v1/macros", get(macros_list).post(macros_save))
        .route("/v1/macro/assign", post(macro_assign))
        .route("/v1/macro/run", post(macro_run))
        .route("/v1/macro/stop", post(macro_stop))
        .route("/v1/roblox/install", post(roblox_install))
        .route("/v1/logoff", post(logoff))
        .fallback(|| async { refuse(StatusCode::NOT_FOUND, "notFound", "No such endpoint.") })
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(axum::middleware::from_fn_with_state(ctl.clone(), gate))
        .layer(axum::middleware::map_response(harden))
        .with_state(ctl)
}

/// No caching, sniffing, framing, referrers or outside resources, on every answer.
async fn harden(mut r: Response) -> Response {
    let h = r.headers_mut();
    for (k, v) in [
        (header::CACHE_CONTROL, "no-store"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        (header::REFERRER_POLICY, "no-referrer"),
        (header::X_FRAME_OPTIONS, "DENY"),
        (header::CONTENT_SECURITY_POLICY, CSP),
    ] {
        h.insert(k, HeaderValue::from_static(v));
    }
    r
}

/// Equal bytes, in time that depends on the length only (pure).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && std::hint::black_box(a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y))) == 0
}

/// The request names this server exactly: `127.0.0.1:<port>` (pure).
fn host_ok(host: Option<&str>, port: u16) -> bool {
    host.is_some_and(|h| h.strip_prefix("127.0.0.1:").is_some_and(|p| p == port.to_string()))
}

/// The request carries the token, once (pure).
fn token_ok(h: &HeaderMap, token: &str) -> bool {
    let mut all = h.get_all(TOKEN_HEADER).iter();
    match (all.next(), all.next()) {
        (Some(v), None) => same(v.as_bytes(), token.as_bytes()),
        _ => false,
    }
}

/// Refuses a request for another host (DNS rebinding) or without the token.
async fn gate(State(c): State<Ctl>, req: Request, next: Next) -> Response {
    if !host_ok(req.headers().get(header::HOST).and_then(|v| v.to_str().ok()), c.port) {
        log::warn!("control API: refused a request for another host");
        return refuse(StatusCode::MISDIRECTED_REQUEST, "invalid", "Address the control API as 127.0.0.1 and its port.");
    }
    if !token_ok(req.headers(), &c.token) {
        log::warn!("control API: refused a request without the right token ({} {})", req.method(), req.uri().path());
        return refuse(StatusCode::UNAUTHORIZED, "invalid", "Missing or wrong control token.");
    }
    next.run(req).await
}

fn refuse(code: StatusCode, kind: &str, message: &str) -> Response {
    (code, Json(ControlError { kind: kind.into(), message: message.into() })).into_response()
}

/// An `AppError` as the client reads it back (`ControlError`; its message is user-facing and
/// secret-free), logged once like the UI's errors.
fn failed(e: AppError) -> Response {
    e.log();
    let code = match e {
        AppError::NotFound => StatusCode::NOT_FOUND,
        AppError::Locked => StatusCode::LOCKED,
        AppError::Invalid(_) | AppError::Wait(_) | AppError::Busy(_) | AppError::WrongPassword => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    (code, Json(ControlError { kind: e.kind().into(), message: e.message() })).into_response()
}

fn answer<T: Serialize>(r: Result<T>) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => failed(e),
    }
}

/// `{}`: done.
fn done(r: Result<()>) -> Response {
    answer(r.map(|()| json!({})))
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| AppError::Internal(e.to_string()))
}

// ---------- handlers ----------

async fn health(State(c): State<Ctl>) -> Response {
    let roblox_installed = roblox_installed(&c).await;
    let s = c.app.state::<App>();
    let clients = s.tracker.list().iter().filter(|i| i.state != InstanceState::Exited).count();
    Json(ControlHealth {
        version: c.app.package_info().version.to_string(),
        windows_user: super::windows_user(),
        session_id: session_id(),
        mode: c.mode,
        vault_locked: s.vault.is_locked(),
        roblox_installed,
        clients: u32::try_from(clients).unwrap_or(u32::MAX),
    })
    .into_response()
}

/// Whether a Roblox client is installed for this user, looked at most every `INSTALLED_FOR`
/// (`launcher::client::find` logs each time it finds none).
async fn roblox_installed(c: &Ctl) -> bool {
    if let Some((at, yes)) = *c.installed.lock().unwrap_or_else(|e| e.into_inner())
        && at.elapsed() < INSTALLED_FOR
    {
        return yes;
    }
    let yes = blocking(|| crate::launcher::client::find().is_ok()).await.unwrap_or(false);
    *c.installed.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), yes));
    yes
}

/// The Windows session this process runs in (0 when Windows won't say).
fn session_id() -> u32 {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut id = 0;
    // SAFETY: our own process id and a valid out pointer.
    if let Err(e) = unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) } {
        log::debug!("control API: no session id: {e}");
    }
    id
}

async fn state(State(c): State<Ctl>) -> Response {
    let app = c.app.clone();
    let metrics = blocking(move || {
        let s = app.state::<App>();
        s.metrics.sample(&s.tracker.clients())
    })
    .await
    .unwrap_or_default();
    let s = c.app.state::<App>();
    let accounts = match s.db.accounts() {
        Ok(a) => a,
        Err(e) => return failed(e),
    };
    let runs = macro_runs(&s);
    let rows: Vec<ControlClient> = s
        .tracker
        .list()
        .into_iter()
        .filter(|i| i.state != InstanceState::Exited)
        .map(|i| {
            let m = metrics.iter().find(|m| m.instance_id == i.id);
            let a = accounts.iter().find(|a| a.id == i.account_id);
            ControlClient {
                account_id: i.account_id,
                username: a.map(|a| a.username.clone()).unwrap_or_default(),
                display_name: a.map(|a| a.display_name.clone()).unwrap_or_default(),
                state: i.state,
                place_name: i.place_id.map(|p| crate::commands::cached_game_name(&s, p).unwrap_or_else(|| format!("Place {p}"))),
                started_at: i.started_at,
                cpu_pct: m.and_then(|m| m.cpu_pct),
                memory_mb: m.map(|m| m.memory_mb),
                afk: afk_config(&s, i.account_id),
                macro_run: runs.iter().find(|r| r.instance_id == i.id).cloned(),
                macro_assigned: macro_assigned(&s, i.account_id),
                instance_id: i.id,
            }
        })
        .collect();
    Json(rows).into_response()
}

async fn accounts(State(c): State<Ctl>) -> Response {
    let s = c.app.state::<App>();
    answer(s.db.accounts().map(|list| {
        list.into_iter()
            .map(|a| ControlAccount { id: a.id, username: a.username, display_name: a.display_name, status: a.status })
            .collect::<Vec<_>>()
    }))
}

/// Imports a `.roalt` backup the main app sent, as Settings' import does with "replace": a
/// stored account is updated only when the backup's cookie signs in as that user.
async fn import(State(c): State<Ctl>, Json(body): Json<ImportRequest>) -> Response {
    let s = c.app.state::<App>();
    let r = async {
        let bytes = Zeroizing::new(
            base64::engine::general_purpose::STANDARD
                .decode(body.backup.trim())
                .map_err(|_| AppError::Invalid(tr!("This isn't an AltDock backup.")))?,
        );
        let payload = crate::vault::backup::read_bytes(&bytes, &body.password)?;
        let r = crate::commands::import_backup(&c.app, &s, payload, ConflictPolicy::Replace).await?;
        log::info!("control API: imported {} account(s), {} failed", r.added + r.updated, r.failed);
        Ok(ImportAnswer { imported: r.added + r.updated, failed: r.failed })
    };
    answer(r.await)
}

/// Pasted text → what to launch, as the Launch box resolves it: a username is looked up and
/// followed; a bare job id has no place to go with.
async fn target_of(c: &Ctl, text: &str) -> Result<LaunchTarget> {
    match crate::launcher::target::parse(text) {
        ParsedInput::Target { target } => Ok(target),
        ParsedInput::Username { name } => {
            let user = crate::commands::user_lookup(c.app.state::<App>(), name).await?;
            Ok(LaunchTarget::FollowUser { user_id: user.user_id })
        }
        // The Launch box asks for the place then; here the whole link has to come.
        ParsedInput::JobIdOnly { .. } => Err(AppError::Invalid(tr!("Paste a game link, an ID or a username."))),
        ParsedInput::Invalid { reason } | ParsedInput::Unsupported { reason } => Err(AppError::Invalid(reason)),
    }
}

/// Launches as a batch (`launcher::bulk::start`, as the Launch view does for ticked accounts)
/// with the launch delay.
async fn launch(State(c): State<Ctl>, Json(body): Json<LaunchRequest>) -> Response {
    let r = async {
        let target = target_of(&c, &body.target).await?;
        let s = c.app.state::<App>();
        let started = u32::try_from(body.account_ids.len()).unwrap_or(u32::MAX);
        let req = BulkRequest {
            account_ids: body.account_ids,
            target,
            delay_ms: s.db.settings()?.launch_delay_ms,
            same_server: SameServer::Off,
            favourite_id: None,
            fps_cap: None,
        };
        let batch_id = crate::launcher::bulk::start(c.app.clone(), req)?;
        log::info!("control API: launching {started} account(s)");
        Ok(LaunchAnswer { started, batch_id })
    };
    answer(r.await)
}

async fn close(State(c): State<Ctl>, Json(body): Json<CloseRequest>) -> Response {
    log::info!("control API: closing a client (force: {})", body.force);
    done(crate::commands::instance_close(c.app.state::<App>(), body.id, body.force).await)
}

async fn kill_all(State(c): State<Ctl>) -> Response {
    log::info!("control API: kill all Roblox");
    done(crate::commands::kill_all(&c.app.state::<App>()).await.map(drop))
}

#[derive(Deserialize)]
struct ThumbQuery {
    w: Option<u32>,
    /// The picture number the caller has already: then only the header comes back.
    known: Option<u32>,
}

/// One client's preview, the bytes `instance_thumbnail` gives the UI (24-byte header, then RGBA).
async fn thumbnail(State(c): State<Ctl>, Path(id): Path<String>, Query(q): Query<ThumbQuery>) -> Response {
    let s = c.app.state::<App>();
    if s.tracker.get(&id).is_none() {
        return failed(AppError::NotFound);
    }
    let pid = s.tracker.clients().into_iter().find(|c| c.0 == id).map(|c| c.1);
    let (w, known) = (q.w.unwrap_or(320).clamp(16, 1280), q.known.unwrap_or(0));
    match blocking(move || crate::instances::window::preview(pid, w).to_bytes(known)).await {
        Ok(bytes) => ([(header::CONTENT_TYPE, "application/octet-stream")], bytes).into_response(),
        Err(e) => failed(e),
    }
}

async fn afk(State(c): State<Ctl>, Json(body): Json<AfkRequest>) -> Response {
    done(set_afk(&c.app, &c.app.state::<App>(), body.account_id, body.config))
}

async fn macros_list(State(c): State<Ctl>) -> Response {
    answer(list_macros(&c.app.state::<App>()))
}

async fn macros_save(State(c): State<Ctl>, Json(body): Json<MacroInput>) -> Response {
    answer(save_macro(&c.app.state::<App>(), body))
}

async fn macro_assign(State(c): State<Ctl>, Json(b): Json<MacroAssignRequest>) -> Response {
    done(assign_macro(&c.app.state::<App>(), b.account_id, b.macro_id, b.autorun))
}

async fn macro_run(State(c): State<Ctl>, Json(b): Json<MacroRunRequest>) -> Response {
    done(run_macro(&c.app, &c.app.state::<App>(), b.macro_id, &b.instance_ids))
}

async fn macro_stop(State(c): State<Ctl>, Json(b): Json<MacroStopRequest>) -> Response {
    done(stop_macro(&c.app, &b.instance_id))
}

/// Starts the verified Roblox install (`launcher::update::install`) and answers at once: it
/// downloads for a while. Its outcome shows in `health` (`robloxInstalled`) and the log.
async fn roblox_install(State(c): State<Ctl>) -> Response {
    if c.app.state::<App>().update.installing() {
        return failed(AppError::Invalid(crate::commands::installing()));
    }
    *c.installed.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let ctl = c.clone();
    tauri::async_runtime::spawn(async move {
        if crate::launcher::update::install(&ctl.app).await.is_ok() {
            log::info!("control API: Roblox installed");
        }
        *ctl.installed.lock().unwrap_or_else(|e| e.into_inner()) = None;
    });
    done(Ok(()))
}

/// Signs this Windows session out, a moment after answering.
async fn logoff() -> Response {
    log::info!("control API: signing the session out");
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(Duration::from_millis(300)).await;
        use windows::Win32::System::Shutdown::{EWX_LOGOFF, ExitWindowsEx, SHTDN_REASON_FLAG_PLANNED, SHTDN_REASON_MAJOR_APPLICATION};
        // SAFETY: plain call; signs out the session this process runs in.
        if let Err(e) = unsafe { ExitWindowsEx(EWX_LOGOFF, SHTDN_REASON_MAJOR_APPLICATION | SHTDN_REASON_FLAG_PLANNED) } {
            log::warn!("control API: could not sign the session out: {e}");
        }
    });
    done(Ok(()))
}

// ---------- adapters to Stream A (macros, per-client anti-AFK) ----------

fn not_in_build() -> AppError {
    AppError::Invalid("Not available in this build.".into())
}

/// TODO(merge): `crate::instances::afk::client_config(s, account_id)`. Until then: the global
/// anti-AFK as this build has it.
fn afk_config(s: &App, _account_id: i64) -> AfkClientConfig {
    let v = s.db.settings().unwrap_or_default();
    AfkClientConfig { enabled: v.anti_afk.enabled, interval_sec: v.anti_afk.interval_min * 60, action: AfkAction::F13 }
}

/// TODO(merge): `crate::instances::afk::set_client(app, s, account_id, config)`.
fn set_afk(_app: &AppHandle, _s: &App, _account_id: i64, _config: AfkClientConfig) -> Result<()> {
    Err(not_in_build())
}

/// TODO(merge): `crate::macros::runs(s)`.
fn macro_runs(_s: &App) -> Vec<MacroRun> {
    Vec::new()
}

/// TODO(merge): the account's macro from Stream A's `account_macros` table.
fn macro_assigned(_s: &App, _account_id: i64) -> Option<i64> {
    None
}

/// TODO(merge): `crate::macros::list(s)`.
fn list_macros(_s: &App) -> Result<Vec<Macro>> {
    Err(not_in_build())
}

/// TODO(merge): `crate::macros::save(s, input)` (pushed from the main app: trusted).
fn save_macro(_s: &App, _input: MacroInput) -> Result<Macro> {
    Err(not_in_build())
}

/// TODO(merge): `crate::macros::assign(s, account_id, macro_id, autorun)`.
fn assign_macro(_s: &App, _account_id: i64, _macro_id: Option<i64>, _autorun: bool) -> Result<()> {
    Err(not_in_build())
}

/// TODO(merge): `crate::macros::run(app, s, macro_id, instance_ids)`.
fn run_macro(_app: &AppHandle, _s: &App, _macro_id: i64, _instance_ids: &[String]) -> Result<()> {
    Err(not_in_build())
}

/// TODO(merge): `crate::macros::stop_instance(app, instance_id)`.
fn stop_macro(_app: &AppHandle, _instance_id: &str) -> Result<()> {
    Err(not_in_build())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn compares_tokens() {
        assert!(same(TOKEN.as_bytes(), TOKEN.as_bytes()));
        assert!(!same(TOKEN.as_bytes(), TOKEN.replace('f', "e").as_bytes()));
        assert!(!same(&TOKEN.as_bytes()[1..], TOKEN.as_bytes()));
        assert!(!same(b"", b"1"));
        assert!(same(b"", b""));

        let headers = |values: &[&str]| {
            let mut h = HeaderMap::new();
            for v in values {
                h.append(TOKEN_HEADER, HeaderValue::from_str(v).unwrap());
            }
            h
        };
        assert!(token_ok(&headers(&[TOKEN]), TOKEN));
        assert!(!token_ok(&headers(&[]), TOKEN));
        assert!(!token_ok(&headers(&[&TOKEN.to_uppercase()]), TOKEN));
        assert!(!token_ok(&headers(&[&TOKEN[..63]]), TOKEN));
        assert!(!token_ok(&headers(&[&format!("{TOKEN} ")]), TOKEN));
        // A second value, right or wrong, is refused.
        assert!(!token_ok(&headers(&[TOKEN, TOKEN]), TOKEN));
        assert!(!token_ok(&headers(&["x", TOKEN]), TOKEN));
        // Header names ignore case.
        let mut h = HeaderMap::new();
        h.insert("X-AltDock-Token", HeaderValue::from_static(TOKEN));
        assert!(token_ok(&h, TOKEN));
    }

    #[test]
    fn only_this_host() {
        assert!(host_ok(Some("127.0.0.1:47611"), 47611));
        for bad in [
            None,
            Some("127.0.0.1"),
            Some("127.0.0.1:47612"),
            Some("127.0.0.1:047611"),
            Some("localhost:47611"),
            Some("127.0.0.2:47611"),
            Some("[::1]:47611"),
            Some("evil.example:47611"),
            Some("127.0.0.1:47611.evil.example"),
            Some("127.0.0.1:47611 "),
            Some(""),
        ] {
            assert!(!host_ok(bad, 47611), "{bad:?}");
        }
    }
}
