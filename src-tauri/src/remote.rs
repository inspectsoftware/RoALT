//! The LAN web dashboard (opt-in, `Settings.remote`): a small HTTPS server on every network
//! address, for a phone or another PC to see the clients and launch or close them.
//!
//! - HTTPS only (TLS 1.2+, rustls), with a self-signed certificate made by RoALT (ECDSA P-256,
//!   for this PC's LAN address and localhost; a new one each year, when the address changes or
//!   on "New certificate"). Its private key is kept in the vault (`CERT_AAD`); its SHA-256
//!   fingerprint shows in Settings, to check against the browser's warning on first visit.
//! - A random 8-digit PIN, kept in the vault (AES-GCM, `PIN_AAD`) and never logged. The login
//!   page trades it for a session cookie (HttpOnly, SameSite=Strict) holding a random token
//!   made at each start; regenerating the PIN makes a new one too, signing every device out.
//! - Every API request needs that cookie (compared in constant time); a POST also needs the
//!   `X-RoALT: 1` header, which a page from another origin can't send without CORS (never
//!   allowed).
//! - Wrong PINs: at most `MAX_PER_IP` per address and `MAX_ALL` overall per `WINDOW`, then
//!   every attempt is refused until the oldest ages out (the overall cap can lock the owner out
//!   for a while too: the price of stopping guesses spread over many addresses).
//! - Only requests addressed to an IP or localhost (`Host`), so a website rebound to this PC's
//!   address (DNS rebinding) can't reach it, not even its login.
//! - Runs only while the vault is unlocked: refused while locked, stopped on lock.
//! - Shows names, states and usage only: never a cookie, the webhook or another secret.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Manager};
use tokio::sync::oneshot;
use zeroize::Zeroizing;

use crate::App;
use crate::commands::now_ms;
use crate::discord::{hex, unhex};
use crate::error::{AppError, Result};
use crate::model::{AccountStatus, BulkRequest, InstanceState, LaunchTarget, ParsedInput, RemoteStatus, SameServer};
use crate::vault::Secret;
use crate::vault::master::random_bytes;

/// AAD for the stored PIN.
const PIN_AAD: &[u8] = b"roalt.remote.pin";
/// The settings-table row holding the encrypted PIN (hex).
const KEY: &str = "remote_pin";
/// The session cookie's name.
const COOKIE: &str = "roalt_session";
/// How long a phone stays signed in (the token still ends with the server).
const COOKIE_DAYS: u32 = 30;
const WINDOW: Duration = Duration::from_secs(5 * 60);
const MAX_PER_IP: usize = 5;
const MAX_ALL: usize = 20;
const PAGE: &str = include_str!("remote.html");
/// The page's scripts and styles are inline; it loads nothing else.
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; connect-src 'self'; \
                   img-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";
const LOCKED: &str = "Unlock the vault first: the web dashboard only runs while it is open.";
/// AAD for the certificate's private key.
const CERT_AAD: &[u8] = b"roalt.remote.cert";
/// The settings-table row holding the certificate (`StoredCert` JSON).
const CERT_ROW: &str = "remote_cert";
const YEAR_MS: i64 = 365 * 86_400_000;

/// The dashboard's certificate as stored: the certificate itself is public (hex DER), its key
/// vault-encrypted (hex).
#[derive(Serialize, Deserialize)]
struct StoredCert {
    cert: String,
    key: String,
    /// The LAN address it was made for.
    ip: Option<Ipv4Addr>,
    /// Unix ms after which a new one is made.
    renew_at: i64,
}

/// The server while it runs; dropping it (its `stop`) shuts the server down.
struct Running {
    port: u16,
    pin: Secret,
    token: Secret,
    _stop: oneshot::Sender<()>,
}

/// The web dashboard's state (`App.remote`).
pub struct Remote {
    running: Mutex<Option<Running>>,
    fails: Mutex<Fails>,
}

impl Remote {
    pub fn new() -> Self {
        Self { running: Mutex::new(None), fails: Mutex::new(Fails::default()) }
    }

    fn guard(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fails(&self) -> MutexGuard<'_, Fails> {
        self.fails.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The port it is serving on, None while stopped.
    pub fn port(&self) -> Option<u16> {
        self.guard().as_ref().map(|r| r.port)
    }

    /// Stops the server; true when it was running.
    pub fn stop(&self) -> bool {
        let was = self.guard().take().is_some();
        if was {
            log::info!("web dashboard stopped");
        }
        was
    }

    /// The PIN and the session token while running.
    fn secrets(&self) -> Option<(Secret, Secret)> {
        self.guard().as_ref().map(|r| (r.pin.clone(), r.token.clone()))
    }
}

impl Default for Remote {
    fn default() -> Self {
        Self::new()
    }
}

/// Failed PIN attempts of the last `WINDOW` (pure, given `now`). Never more than `MAX_ALL`
/// entries: once full, nothing is checked or recorded.
#[derive(Default)]
struct Fails(Vec<(IpAddr, Instant)>);

impl Fails {
    /// No more attempts from `ip` for now.
    fn blocked(&mut self, ip: IpAddr, now: Instant) -> bool {
        self.0.retain(|&(_, t)| now.duration_since(t) < WINDOW);
        self.0.len() >= MAX_ALL || self.0.iter().filter(|&&(i, _)| i == ip).count() >= MAX_PER_IP
    }

    fn record(&mut self, ip: IpAddr, now: Instant) {
        self.0.push((ip, now));
    }
}

/// Equal bytes, in time that depends on the length only (pure).
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && std::hint::black_box(a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y))) == 0
}

/// The session token in one `Cookie` header (pure).
fn session_of(cookies: &str) -> Option<&str> {
    cookies.split(';').find_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='))
}

/// A launch target as the app's own `LaunchTarget` or pasted text (a link), like the Launch view.
#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum TargetIn {
    Text(String),
    Target(LaunchTarget),
}

/// What POST /api/launch carries.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct LaunchBody {
    account_ids: Vec<i64>,
    target: TargetIn,
}

/// The target a launch body names (pure).
fn target_of(t: TargetIn) -> Result<LaunchTarget> {
    match t {
        TargetIn::Target(t) => Ok(t),
        TargetIn::Text(text) => match crate::launcher::target::parse(&text) {
            ParsedInput::Target { target } => Ok(target),
            ParsedInput::JobIdOnly { .. } => Err(AppError::Invalid("Paste the server's link, not only its job id.".into())),
            ParsedInput::Username { .. } => Err(AppError::Invalid("Paste a game link; following a user works in RoALT itself.".into())),
            ParsedInput::Invalid { reason } | ParsedInput::Unsupported { reason } => Err(AppError::Invalid(reason)),
        },
    }
}

/// A random 8-digit PIN (the modulo bias over 64 bits is negligible).
fn new_pin() -> Secret {
    Secret::new(format!("{:08}", u64::from_le_bytes(random_bytes::<8>()) % 100_000_000))
}

/// Makes and stores a new PIN. `Locked` while the vault is.
fn store_new_pin(s: &App) -> Result<Secret> {
    let pin = new_pin();
    let blob = s.vault.encrypt(PIN_AAD, pin.expose().as_bytes())?;
    s.db.set_setting_value(KEY, Some(&hex(&blob)))?;
    Ok(pin)
}

/// The stored PIN, made on first use (and again when it can't be opened, as after restoring
/// another PC's data). `Locked` while the vault is.
fn stored_pin(s: &App) -> Result<Secret> {
    let Some(stored) = s.db.setting_value(KEY)? else { return store_new_pin(s) };
    match unhex(&stored).ok_or(AppError::Crypto).and_then(|blob| s.vault.decrypt_secret(PIN_AAD, &blob)) {
        Err(AppError::Crypto) => store_new_pin(s),
        r => r,
    }
}

/// The PIN, for Settings' Reveal.
pub fn pin(s: &App) -> Result<String> {
    Ok(stored_pin(s)?.expose().to_owned())
}

/// A new PIN; a running server takes it at once and signs every device out.
pub fn regenerate(s: &App) -> Result<String> {
    let pin = store_new_pin(s)?;
    if let Some(r) = s.remote.guard().as_mut() {
        r.pin = pin.clone();
        r.token = new_token();
    }
    s.remote.fails().0.clear();
    log::info!("web dashboard PIN changed");
    Ok(pin.expose().to_owned())
}

fn new_token() -> Secret {
    Secret::new(hex(&random_bytes::<32>()))
}

/// A new self-signed certificate for `ip` (when known), 127.0.0.1 and localhost: its DER and
/// its private key's DER (PKCS#8).
fn make_cert(ip: Option<Ipv4Addr>) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>)> {
    use rcgen::{CertificateParams, DnType, KeyPair, SanType, date_time_ymd};
    let bad = |e: rcgen::Error| AppError::Internal(format!("couldn't make the dashboard's certificate: {e}"));
    let mut p = CertificateParams::new(vec!["localhost".to_owned()]).map_err(bad)?;
    p.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    if let Some(ip) = ip {
        p.subject_alt_names.push(SanType::IpAddress(IpAddr::V4(ip)));
    }
    p.distinguished_name.push(DnType::CommonName, "RoALT web dashboard");
    // Whole years around now; RoALT itself renews it after a year (`StoredCert::renew_at`).
    let year = i32::try_from(1970 + now_ms() / 31_556_952_000).unwrap_or(2026);
    p.not_before = date_time_ymd(year - 1, 1, 1);
    p.not_after = date_time_ymd(year + 2, 1, 1);
    let key = KeyPair::generate().map_err(bad)?;
    let cert = p.self_signed(&key).map_err(bad)?;
    Ok((cert.der().to_vec(), Zeroizing::new(key.serialize_der())))
}

fn stored_cert(s: &App) -> Result<Option<StoredCert>> {
    Ok(s.db.setting_value(CERT_ROW)?.and_then(|j| serde_json::from_str(&j).ok()))
}

/// The certificate and its key for serving now: the stored one while it is for this LAN
/// address and not due for renewal, else a new one (stored). `Locked` while the vault is.
fn cert(s: &App) -> Result<(Vec<u8>, Zeroizing<Vec<u8>>)> {
    let ip = lan_ip();
    if let Some(c) = stored_cert(s)?.filter(|c| c.ip == ip && now_ms() < c.renew_at) {
        let opened = unhex(&c.cert).zip(unhex(&c.key)).ok_or(AppError::Crypto).and_then(|(cert, key)| {
            Ok((cert, s.vault.decrypt(CERT_AAD, &key)?))
        });
        match opened {
            Err(AppError::Crypto) => {}
            r => return r,
        }
    }
    let (cert, key) = make_cert(ip)?;
    let stored = StoredCert { cert: hex(&cert), key: hex(&s.vault.encrypt(CERT_AAD, &key)?), ip, renew_at: now_ms() + YEAR_MS };
    let json = serde_json::to_string(&stored).map_err(|e| AppError::Internal(e.to_string()))?;
    s.db.set_setting_value(CERT_ROW, Some(&json))?;
    log::info!("web dashboard: new certificate {}", fingerprint(&cert));
    Ok((cert, key))
}

/// SHA-256 of a certificate, as browsers show it: upper-case hex pairs joined by colons (pure).
fn fingerprint(cert_der: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(cert_der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Forgets the certificate and makes a new one; a running server restarts with it (browsers
/// warn again, with the new fingerprint).
pub fn new_certificate(app: &AppHandle) -> Result<()> {
    let s = app.state::<App>();
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    s.db.set_setting_value(CERT_ROW, None)?;
    match s.remote.port() {
        Some(port) => {
            s.remote.stop();
            start(app, port)
        }
        None => cert(&s).map(drop),
    }
}

/// Whether it runs, and where other devices reach it.
pub fn status(s: &App) -> Result<RemoteStatus> {
    let port = match s.remote.port() {
        Some(p) => p,
        None => s.db.settings()?.remote.port,
    };
    let urls = lan_ip().map(|ip| format!("https://{ip}:{port}")).into_iter().collect();
    let fingerprint = stored_cert(s)?.and_then(|c| unhex(&c.cert)).map(|der| fingerprint(&der));
    Ok(RemoteStatus { running: s.remote.port().is_some(), urls, fingerprint })
}

/// This PC's address on the network it reaches others through (connecting a UDP socket sends
/// nothing). ponytail: the main adapter only; list every adapter (GetAdaptersAddresses) if
/// people run RoALT on several networks at once.
fn lan_ip() -> Option<Ipv4Addr> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    sock.connect((Ipv4Addr::new(8, 8, 8, 8), 53)).ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// Serves on 0.0.0.0:`port` (Windows Firewall asks the first time), in place of a server on
/// another port; nothing to do when it already serves there. Refused while the vault is locked.
pub fn start(app: &AppHandle, port: u16) -> Result<()> {
    let s = app.state::<App>();
    if s.vault.is_locked() {
        return Err(AppError::Invalid(LOCKED.into()));
    }
    if let Some(why) = crate::network::blocked(&s) {
        return Err(AppError::Invalid(why));
    }
    if s.remote.port() == Some(port) {
        return Ok(());
    }
    let pin = stored_pin(&s)?;
    let (cert_der, key_der) = cert(&s)?;
    let listener = TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).map_err(|e| match e.kind() {
        ErrorKind::AddrInUse => AppError::Invalid(format!("Port {port} is in use by another app. Pick another port.")),
        ErrorKind::PermissionDenied => AppError::Invalid(format!("Windows won't let RoALT use port {port}. Pick another port.")),
        _ => AppError::Internal(format!("Couldn't open port {port}: {e}")),
    })?;
    listener.set_nonblocking(true)?;
    let (stop, stopped) = oneshot::channel::<()>();
    *s.remote.guard() = Some(Running { port, pin, token: new_token(), _stop: stop });
    let router = routes(app.clone());
    // One crypto provider for the whole process.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tauri::async_runtime::spawn(async move {
        let served = async {
            let tls = axum_server::tls_rustls::RustlsConfig::from_der(vec![cert_der], key_der.to_vec()).await?;
            drop(key_der);
            let handle = axum_server::Handle::new();
            let stopper = handle.clone();
            // Ends when `Running` (its sender) is dropped: stopped or replaced.
            tokio::spawn(async move {
                let _ = stopped.await;
                stopper.graceful_shutdown(Some(Duration::from_secs(1)));
            });
            axum_server::from_tcp_rustls(listener, tls)
                .handle(handle)
                .serve(router.into_make_service_with_connect_info::<SocketAddr>())
                .await
        };
        if let Err(e) = served.await {
            log::warn!("web dashboard failed: {e}");
        }
    });
    log::info!("web dashboard serving on port {port}");
    Ok(())
}

/// Starts the server when the settings want it and the vault is open (at start, after an
/// unlock); Ok when there is nothing to do.
pub fn resume(app: &AppHandle) -> Result<()> {
    let s = app.state::<App>();
    let r = s.db.settings()?.remote;
    if r.enabled && !s.vault.is_locked() { start(app, r.port) } else { Ok(()) }
}

fn routes(app: AppHandle) -> Router {
    Router::new()
        .route("/", get(page))
        .route("/api/login", post(login))
        .route("/api/state", get(clients))
        .route("/api/accounts", get(accounts))
        .route("/api/launch", post(launch))
        .route("/api/close", post(close))
        .route("/api/kill-all", post(kill_all))
        .layer(axum::middleware::from_fn(ip_hosts_only))
        .layer(axum::middleware::map_response(harden))
        .with_state(app)
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
        (header::STRICT_TRANSPORT_SECURITY, "max-age=31536000"),
    ] {
        h.insert(k, HeaderValue::from_static(v));
    }
    r
}

/// A `Host` header naming an IP address or localhost, with or without a port (pure).
fn ip_host(host: &str) -> bool {
    let name = match host.strip_prefix('[') {
        Some(v6) => return v6.split_once(']').is_some_and(|(a, port)| a.parse::<std::net::Ipv6Addr>().is_ok() && (port.is_empty() || port.starts_with(':'))),
        None => host.rsplit_once(':').map_or(host, |(n, _)| n),
    };
    name.eq_ignore_ascii_case("localhost") || name.parse::<Ipv4Addr>().is_ok()
}

/// Refuses a request whose `Host` is a name (`ip_host`): DNS rebinding.
async fn ip_hosts_only(req: Request, next: Next) -> Response {
    if req.headers().get(header::HOST).and_then(|v| v.to_str().ok()).is_some_and(ip_host) {
        next.run(req).await
    } else {
        refuse(StatusCode::MISDIRECTED_REQUEST, "Open the dashboard by this PC's IP address.")
    }
}

fn refuse(code: StatusCode, message: &str) -> Response {
    (code, Json(json!({ "error": message }))).into_response()
}

fn csrf(h: &HeaderMap) -> bool {
    h.get("x-roalt").is_some_and(|v| v == "1")
}

/// None when the request carries the session cookie (and, for a POST, the CSRF header); else
/// the refusal to send.
fn gate(s: &App, h: &HeaderMap, post: bool) -> Option<Response> {
    if post && !csrf(h) {
        return Some(refuse(StatusCode::FORBIDDEN, "Missing the X-RoALT header."));
    }
    let token = s.remote.secrets().map(|(_, t)| t);
    let ok = token.is_some_and(|t| {
        h.get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .filter_map(session_of)
            .any(|v| same(v.as_bytes(), t.expose().as_bytes()))
    });
    (!ok).then(|| refuse(StatusCode::UNAUTHORIZED, "Sign in with the PIN."))
}

/// Gives an `AppError` (its message is user-facing and secret-free) to the page.
fn failed(e: AppError) -> Response {
    let code = if matches!(e, AppError::Invalid(_) | AppError::NotFound | AppError::Locked) { StatusCode::BAD_REQUEST } else { StatusCode::INTERNAL_SERVER_ERROR };
    refuse(code, &e.to_string())
}

async fn page() -> Html<&'static str> {
    Html(PAGE)
}

#[derive(Deserialize)]
struct Login {
    pin: String,
}

async fn login(State(app): State<AppHandle>, ConnectInfo(peer): ConnectInfo<SocketAddr>, h: HeaderMap, Json(body): Json<Login>) -> Response {
    if !csrf(&h) {
        return refuse(StatusCode::FORBIDDEN, "Missing the X-RoALT header.");
    }
    let s = app.state::<App>();
    let Some((pin, token)) = s.remote.secrets() else { return refuse(StatusCode::SERVICE_UNAVAILABLE, "The dashboard is stopping.") };
    let (ip, now) = (peer.ip(), Instant::now());
    // Held through the check and the record, so parallel guesses count one by one.
    let mut fails = s.remote.fails();
    if fails.blocked(ip, now) {
        log::warn!("web dashboard: sign-in from {ip} refused (too many wrong PINs)");
        return refuse(StatusCode::TOO_MANY_REQUESTS, "Too many wrong PINs. Try again in 5 minutes.");
    }
    if !same(body.pin.trim().as_bytes(), pin.expose().as_bytes()) {
        fails.record(ip, now);
        log::warn!("web dashboard: wrong PIN from {ip}");
        return refuse(StatusCode::UNAUTHORIZED, "Wrong PIN.");
    }
    drop(fails);
    log::info!("web dashboard: {ip} signed in");
    let cookie = format!("{COOKIE}={}; Secure; HttpOnly; SameSite=Strict; Path=/; Max-Age={}", token.expose(), COOKIE_DAYS * 86_400);
    ([(header::SET_COOKIE, cookie)], Json(json!({}))).into_response()
}

/// One running client as the page shows it.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClientRow {
    id: String,
    account: String,
    state: InstanceState,
    game: Option<String>,
    uptime_s: i64,
    cpu_pct: Option<f64>,
    ram_mb: Option<f64>,
    throttled: bool,
}

fn game_name(s: &App, place_id: i64) -> String {
    crate::commands::cached_game_name(s, place_id).unwrap_or_else(|| format!("Place {place_id}"))
}

async fn clients(State(app): State<AppHandle>, h: HeaderMap) -> Response {
    if let Some(r) = gate(&app.state::<App>(), &h, false) {
        return r;
    }
    let handle = app.clone();
    let metrics = tauri::async_runtime::spawn_blocking(move || {
        let s = handle.state::<App>();
        s.metrics.sample(&s.tracker.clients())
    })
    .await
    .unwrap_or_default();
    let s = app.state::<App>();
    let names: HashMap<i64, String> =
        s.db.accounts().unwrap_or_default().iter().map(|a| (a.id, crate::launcher::bulk::name_of(a))).collect();
    let now = now_ms();
    let rows: Vec<ClientRow> = s
        .tracker
        .list()
        .into_iter()
        .filter(|i| i.state != InstanceState::Exited)
        .map(|i| {
            let m = metrics.iter().find(|m| m.instance_id == i.id);
            ClientRow {
                account: names.get(&i.account_id).cloned().unwrap_or_else(|| format!("Account {}", i.account_id)),
                state: i.state,
                game: i.place_id.map(|p| game_name(&s, p)),
                uptime_s: (now - i.started_at).max(0) / 1000,
                cpu_pct: m.and_then(|m| m.cpu_pct),
                ram_mb: m.map(|m| m.working_set_mb),
                throttled: i.throttled,
                id: i.id,
            }
        })
        .collect();
    Json(rows).into_response()
}

/// One account as the page shows it.
#[derive(Serialize)]
struct AccountRow {
    id: i64,
    name: String,
    status: AccountStatus,
    running: bool,
}

async fn accounts(State(app): State<AppHandle>, h: HeaderMap) -> Response {
    let s = app.state::<App>();
    if let Some(r) = gate(&s, &h, false) {
        return r;
    }
    match s.db.accounts() {
        Ok(list) => Json(
            list.iter()
                .map(|a| AccountRow {
                    id: a.id,
                    name: crate::launcher::bulk::name_of(a),
                    status: a.status,
                    running: s.tracker.has_live_client(a.id),
                })
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(e) => failed(e),
    }
}

/// Launches as a batch (`launcher::bulk::start`, as the Launch view does for ticked accounts)
/// with the launch delay; its progress shows in RoALT too.
async fn launch(State(app): State<AppHandle>, ConnectInfo(peer): ConnectInfo<SocketAddr>, h: HeaderMap, Json(body): Json<LaunchBody>) -> Response {
    let s = app.state::<App>();
    if let Some(r) = gate(&s, &h, true) {
        return r;
    }
    let started = target_of(body.target).and_then(|target| {
        let n = body.account_ids.len();
        let req = BulkRequest {
            account_ids: body.account_ids,
            target,
            delay_ms: s.db.settings()?.launch_delay_ms,
            same_server: SameServer::Off,
            favourite_id: None,
            fps_cap: None,
        };
        let id = crate::launcher::bulk::start(app.clone(), req)?;
        log::info!("web dashboard: {} launched {n} account(s)", peer.ip());
        Ok(id)
    });
    match started {
        Ok(batch_id) => Json(json!({ "batchId": batch_id })).into_response(),
        Err(e) => failed(e),
    }
}

#[derive(Deserialize)]
struct CloseBody {
    id: String,
    #[serde(default)]
    force: bool,
}

async fn close(State(app): State<AppHandle>, ConnectInfo(peer): ConnectInfo<SocketAddr>, h: HeaderMap, Json(body): Json<CloseBody>) -> Response {
    if let Some(r) = gate(&app.state::<App>(), &h, true) {
        return r;
    }
    log::info!("web dashboard: {} closed a client (force: {})", peer.ip(), body.force);
    match crate::commands::instance_close(app.state::<App>(), body.id, body.force).await {
        Ok(()) => Json(json!({})).into_response(),
        Err(e) => failed(e),
    }
}

async fn kill_all(State(app): State<AppHandle>, ConnectInfo(peer): ConnectInfo<SocketAddr>, h: HeaderMap) -> Response {
    let s = app.state::<App>();
    if let Some(r) = gate(&s, &h, true) {
        return r;
    }
    log::info!("web dashboard: {} killed all Roblox clients", peer.ip());
    match crate::commands::kill_all(&s).await {
        Ok(_) => Json(json!({})).into_response(),
        Err(e) => failed(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_tokens() {
        assert!(same(b"12345678", b"12345678"));
        assert!(!same(b"12345678", b"12345679"));
        assert!(!same(b"1234567", b"12345678"));
        assert!(!same(b"", b"1"));
        assert!(same(b"", b""));
    }

    #[test]
    fn only_ip_hosts() {
        for ok in ["192.168.1.20:8787", "192.168.1.20", "localhost:8787", "LOCALHOST", "[::1]:8787", "[fe80::1]", "127.0.0.1:1"] {
            assert!(ip_host(ok), "{ok}");
        }
        for bad in ["evil.example:8787", "evil.example", "192.168.1.20.evil.example", "[::1]x", "[nope]:1", "", ":8787"] {
            assert!(!ip_host(bad), "{bad}");
        }
    }

    #[test]
    fn pins_are_8_digits() {
        for _ in 0..50 {
            let p = new_pin();
            assert!(p.expose().len() == 8 && p.expose().bytes().all(|b| b.is_ascii_digit()), "{p:?}");
        }
        assert_eq!(new_token().expose().len(), 64);
    }

    #[test]
    fn limits_wrong_pins() {
        let (a, b): (IpAddr, IpAddr) = ("192.168.1.2".parse().unwrap(), "192.168.1.3".parse().unwrap());
        let t0 = Instant::now();
        let mut f = Fails::default();
        for i in 0..MAX_PER_IP {
            assert!(!f.blocked(a, t0), "{i}");
            f.record(a, t0);
        }
        assert!(f.blocked(a, t0));
        assert!(!f.blocked(b, t0), "another address still may");
        assert!(f.blocked(a, t0 + WINDOW - Duration::from_secs(1)));
        assert!(!f.blocked(a, t0 + WINDOW), "the window has passed");
        assert!(f.0.is_empty());

        // Many addresses together: MAX_ALL in all.
        for i in 0..MAX_ALL {
            let ip = IpAddr::V4(Ipv4Addr::new(10, 0, 0, i as u8));
            assert!(!f.blocked(ip, t0));
            f.record(ip, t0);
        }
        assert!(f.blocked(IpAddr::V4(Ipv4Addr::new(10, 0, 1, 1)), t0));
    }

    #[test]
    fn finds_the_session_cookie() {
        assert_eq!(session_of("roalt_session=abc"), Some("abc"));
        assert_eq!(session_of("theme=dark; roalt_session=abc ; x=y"), Some("abc"));
        assert_eq!(session_of("roalt_sessionx=abc"), None);
        assert_eq!(session_of("xroalt_session=abc"), None);
        assert_eq!(session_of("theme=dark"), None);
        assert_eq!(session_of(""), None);
    }

    #[test]
    fn reads_launch_bodies() {
        let parse = |j: &str| serde_json::from_str::<LaunchBody>(j).map_err(|e| e.to_string()).and_then(|b| Ok((b.account_ids, target_of(b.target).map_err(|e| e.to_string())?)));
        assert_eq!(
            parse(r#"{"accountIds":[1,2],"target":"https://www.roblox.com/games/920587237/Adopt-Me"}"#).unwrap(),
            (vec![1, 2], LaunchTarget::Place { place_id: 920587237, launch_data: None })
        );
        assert_eq!(
            parse(r#"{"accountIds":[3],"target":{"kind":"place","placeId":5,"launchData":null}}"#).unwrap(),
            (vec![3], LaunchTarget::Place { place_id: 5, launch_data: None })
        );
        assert!(parse(r#"{"accountIds":[1],"target":"not a link at all !!"}"#).is_err());
        assert!(parse(r#"{"accountIds":[1],"target":"@builderman"}"#).is_err());
        assert!(parse(r#"{"accountIds":"1","target":"x"}"#).is_err());
        assert!(parse(r#"{"target":"https://www.roblox.com/games/1"}"#).is_err());
    }
}
