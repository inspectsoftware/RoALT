//! Opt-in, end-to-end encrypted Remote Dashboard service (`https://remote.roalt.dev/`).
//!
//! Maintains a single outbound TLS WebSocket to the stateless blind relay only while enabled
//! and either paired with ≥1 device or actively running a 120-second pairing session.
//! All state updates, RPCs, and on-demand JPEG previews are E2EE with P-256 + AES-256-GCM.

pub mod crypto;
pub mod dispatch;
pub mod preview;
pub mod protocol;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use futures_util::{FutureExt, Sink, SinkExt, StreamExt};
use p256::SecretKey;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::{Notify, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMsg;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use uuid::Uuid;
use zeroize::Zeroizing;

use self::crypto::{
    DIR_PC_TO_VIEWER, DIR_VIEWER_TO_PC, FRAME_DATA, FRAME_HANDSHAKE, PAIR_MODE_MANUAL, PAIR_MODE_QR, PcIdentity, SessionCipher, build_handshake,
    derive_manual_pair_keys, derive_qr_pair_key, new_ephemeral_key, open_pair_blob, pair_commit, pair_transcript, parse_pubkey, pubkey_bytes,
    random_manual_code, random_pair_id, random_token, seal_pair_blob, sha256_hex, sign_pc_binding, verify_device_binding, verify_handshake,
};
use self::protocol::{APP_KIND_JSON, RemoteCommand, RemoteEvent, RemotePreviewTarget, RemoteRpcRequest, encode_json, encode_preview_frame};
use crate::App;
use crate::commands::now_ms;
use crate::error::{AppError, Result};
use crate::model::{
    BulkProgress, LEGACY_RELAY_URL, RemoteDevice, RemotePairPending, RemotePairSession, RemoteRole, RemoteSettings, RemoteState, RemoteStatus,
};
use crate::storage::RemoteDeviceRow;
use crate::vault::master::random_bytes;

const IDENTITY_KEY: &str = "remote_identity_v1";
const PAUSED_KEY: &str = "remote_paused";
const PROTO: u32 = 2;
const BIN_VERSION: u8 = 0x01;
const BIN_HEADER_LEN: usize = 17;
const SEAL_OVERHEAD: usize = 25;
const MAX_WIRE_FRAME: usize = 240 * 1024;
const PAIR_DURATION_MS: i64 = 120_000;
const PAIR_CONFIRM_MS: i64 = 60_000;
const PENDING_DEVICE_MS: i64 = 10 * 60_000;
const MAX_PAIR_RENEWALS: u8 = 5;
const WEB_BASE_URL: &str = "https://remote.roalt.dev/";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const OUTBOUND_QUEUE: usize = 64;
const PREVIEW_BACKLOG: usize = 8;
const RELAY_SILENCE: Duration = Duration::from_secs(60);
const VIEWER_SILENCE: Duration = Duration::from_secs(15);
const METRICS_EVERY: Duration = Duration::from_secs(3);
const PENDING_TICK_MAX: Duration = Duration::from_secs(6);
const SESSIONS_PER_DEVICE: usize = 3;
const RPC_BURST: f32 = 30.0;
const RPC_PER_SEC: f32 = 10.0;
const SNAPSHOT_BURST: f32 = 2.0;
const SNAPSHOT_PER_SEC: f32 = 0.5;
const INPUT_BURST: f32 = 120.0;
const INPUT_PER_SEC: f32 = 240.0;
const RELEASE_BURST: f32 = 60.0;
const RELEASE_PER_SEC: f32 = 120.0;
const BROADCAST_EVERY: Duration = Duration::from_millis(500);
/// Largest WebSocket message the relay sends a PC: a viewer's frame (`MAX_VIEWER_FRAME`, 64 KiB)
/// or a control message carrying a viewer's text (`MAX_TEXT_FRAME`, 32 KiB), with room for the
/// relay's own fields.
const MAX_RELAY_MESSAGE: usize = 64 * 1024 + 1024;
const RPCS_IN_FLIGHT: usize = 4;

const MAX_DEVICES: usize = 64;
const TOO_MANY_DEVICES: &str = "64 devices are paired with this PC. Revoke one in Settings > Remote before pairing another.";

/// Active pairing window kept in memory for at most 120 s, locked to the first browser that joins.
struct ActivePairing {
    pair_id: String,
    manual_code: String,
    qr_secret: Zeroizing<[u8; 32]>,
    qr_url: String,
    expires_at: i64,
    joiner: Option<Uuid>,
    stage: PairStage,
    renewals: u8,
}

enum PairStage {
    Open,
    Committed { commit: [u8; 32], pc_ephem: SecretKey },
    Pending(PendingPair),
    /// The user approved and the ack went out; the device row is written once the browser confirms.
    Approved(Box<ApprovedPair>),
    Aborted,
}

struct PendingPair {
    device_id: Uuid,
    label: String,
    device_pubkey: Vec<u8>,
    pair_key: Zeroizing<[u8; 32]>,
    sas: Option<String>,
    transcript: [u8; 32],
}

struct ApprovedPair {
    row: RemoteDeviceRow,
    pair_key: Zeroizing<[u8; 32]>,
    transcript: [u8; 32],
    sas: Option<String>,
}

impl ActivePairing {
    fn new(expires_at: i64) -> Self {
        let pair_id = random_pair_id();
        let qr_secret = Zeroizing::new(random_bytes::<32>());
        let qr_url = qr_url(&pair_id, &qr_secret);
        Self { pair_id, manual_code: random_manual_code(), qr_secret, qr_url, expires_at, joiner: None, stage: PairStage::Open, renewals: 0 }
    }

    /// Aborts the pairing if someone joined it; true when it did.
    fn strand(&mut self) -> bool {
        let joined = self.joiner.is_some() && !matches!(self.stage, PairStage::Aborted);
        if joined {
            self.stage = PairStage::Aborted;
        }
        joined
    }
}

/// The hosted dashboard only talks to the official relay, so the link never names a custom one.
fn qr_url(pair_id: &str, qr_secret: &[u8; 32]) -> String {
    format!("{WEB_BASE_URL}#pair={pair_id}.{}", URL_SAFE_NO_PAD.encode(qr_secret))
}

/// One viewer currently attached to this PC on the relay.
struct ViewerSession {
    device_id: Uuid,
    device_label: String,
    device_pubkey: Vec<u8>,
    role: RemoteRole,
    my_ephem_sk: SecretKey,
    my_ephem_pub: [u8; 65],
    cipher: Option<SessionCipher>,
    confirmed: bool,
    last_rx: Instant,
    snapshot_due: bool,
    previews: Vec<RemotePreviewTarget>,
    preview_flows: HashMap<String, preview::Flow>,
    releases_refused: bool,
}

struct Bucket {
    tokens: f32,
    burst: f32,
    per_sec: f32,
    at: Instant,
}

impl Bucket {
    fn new(now: Instant, burst: f32, per_sec: f32) -> Self {
        Self { tokens: burst, burst, per_sec, at: now }
    }

    fn take(&mut self, now: Instant) -> bool {
        self.tokens = (self.tokens + now.saturating_duration_since(self.at).as_secs_f32() * self.per_sec).min(self.burst);
        self.at = now;
        let ok = self.tokens >= 1.0;
        if ok {
            self.tokens -= 1.0;
        }
        ok
    }
}

/// A device's budgets, shared by all of its sessions and kept while the connection lasts, so
/// reconnecting doesn't buy a fresh burst.
struct DeviceBudget {
    rpcs: Bucket,
    snapshots: Bucket,
    inputs: Bucket,
    /// Lifts have their own budget, so a flood of moves never holds a key down.
    releases: Bucket,
    in_flight: Arc<AtomicUsize>,
}

impl DeviceBudget {
    fn new(now: Instant) -> Self {
        Self {
            rpcs: Bucket::new(now, RPC_BURST, RPC_PER_SEC),
            snapshots: Bucket::new(now, SNAPSHOT_BURST, SNAPSHOT_PER_SEC),
            inputs: Bucket::new(now, INPUT_BURST, INPUT_PER_SEC),
            releases: Bucket::new(now, RELEASE_BURST, RELEASE_PER_SEC),
            in_flight: Arc::default(),
        }
    }
}

/// A lift against its device's budget: `None` when it goes through, else whether it is the first
/// refused one in a row for its session (that one lets go of the session's input and is answered).
fn refuse_lift(releases: &mut Bucket, refused_before: &mut bool, now: Instant) -> Option<bool> {
    if releases.take(now) {
        *refused_before = false;
        return None;
    }
    Some(!std::mem::replace(refused_before, true))
}

/// One spawned command counted against its device's `RPCS_IN_FLIGHT`.
struct InFlight(Arc<AtomicUsize>);

impl InFlight {
    fn start(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count.clone())
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// How a relay connection ended badly.
enum ConnEnd {
    /// The relay wants a newer RoALT: retrying won't help.
    Upgrade,
    Failed(String),
}

impl From<String> for ConnEnd {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

impl From<&str> for ConnEnd {
    fn from(why: &str) -> Self {
        Self::Failed(why.to_owned())
    }
}

fn upgrade_required() -> String {
    tr!("This RoALT version is too old for Remote. Update RoALT to use Remote again.")
}

enum ControlSignal {
    Reload,
    Reconnect,
    OpenPair { pair_id: String, manual_code: String },
    ClosePair { pair_id: String },
    ExtendPair { pair_id: String },
    SendPairMsg { pair_id: String, viewer_id: Uuid, data: String },
    SyncTokens,
    Pause,
    Broadcast,
}

#[derive(Default)]
struct Broadcasts {
    accounts: bool,
    instances: bool,
    lock_state: bool,
    bulk: Vec<BulkProgress>,
}

impl Broadcasts {
    fn is_empty(&self) -> bool {
        !self.accounts && !self.instances && !self.lock_state && self.bulk.is_empty()
    }
}

struct Inner {
    state: RemoteState,
    error: Option<String>,
    pc_id: Option<Uuid>,
    pairing: Option<ActivePairing>,
    /// Currently connected viewer conn_id -> device_id (for desktop status UI).
    connected_viewers: HashMap<Uuid, Uuid>,
    watching: u32,
    paused: bool,
    broadcasts: Broadcasts,
    ctrl_tx: Option<mpsc::UnboundedSender<ControlSignal>>,
}

pub struct Remote {
    inner: Mutex<Inner>,
    wake: Notify,
}

impl Remote {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                state: RemoteState::Off,
                error: None,
                pc_id: None,
                pairing: None,
                connected_viewers: HashMap::new(),
                watching: 0,
                paused: false,
                broadcasts: Broadcasts::default(),
                ctrl_tx: None,
            }),
            wake: Notify::new(),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn send_ctrl(&self, sig: ControlSignal) {
        if let Some(tx) = &self.lock().ctrl_tx {
            let _ = tx.send(sig);
        }
        self.wake.notify_one();
    }

    fn open_pairing(&self, pairing: ActivePairing) -> Result<Option<ActivePairing>> {
        let mut g = self.lock();
        if g.paused {
            return Err(paused_refusal());
        }
        Ok(g.pairing.replace(pairing))
    }

    /// Pausing also closes an open pairing, so no device is paired while paused.
    fn pause(&self, paused: bool) {
        let open = {
            let mut g = self.lock();
            g.paused = paused;
            g.pairing.take_if(|_| paused)
        };
        if let Some(p) = open {
            close_pairing(self, p, "closed");
        }
        self.send_ctrl(if paused { ControlSignal::Pause } else { ControlSignal::Reconnect });
    }

    pub fn notify_settings_changed(&self) {
        self.send_ctrl(ControlSignal::Reload);
    }

    fn queue_broadcast(&self, mark: impl FnOnce(&mut Broadcasts)) {
        let wake = {
            let mut g = self.lock();
            if g.connected_viewers.is_empty() {
                return;
            }
            let idle = g.broadcasts.is_empty();
            mark(&mut g.broadcasts);
            idle
        };
        if wake {
            self.send_ctrl(ControlSignal::Broadcast);
        }
    }

    pub fn notify_accounts_changed(&self) {
        self.queue_broadcast(|b| b.accounts = true);
    }

    pub fn notify_instances_changed(&self) {
        self.queue_broadcast(|b| b.instances = true);
    }

    pub fn notify_bulk_progress(&self, p: BulkProgress) {
        self.queue_broadcast(|b| {
            b.bulk.retain(|old| old.batch_id != p.batch_id);
            b.bulk.push(p);
        });
    }

    pub fn notify_lock_state_changed(&self) {
        self.queue_broadcast(|b| b.lock_state = true);
    }

    pub fn notify_network_changed(&self) {
        self.send_ctrl(ControlSignal::Reload);
    }
}

/// Default Windows computer name used when `settings.remote.pc_name` is empty.
pub fn effective_pc_name(remote: &RemoteSettings) -> String {
    let trimmed = remote.pc_name.trim();
    if !trimmed.is_empty() {
        return trimmed.chars().take(48).collect();
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .map(|s| s.trim().chars().take(48).collect::<String>())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "My PC".into())
}

/// The official relay (the hosted dashboard only talks to it).
fn is_official_relay(relay_url: &str) -> bool {
    relay_url == RemoteSettings::default().relay_url
}

/// Dev and QA builds may use another relay; a release build always uses the official one.
pub const CUSTOM_RELAY: bool = cfg!(debug_assertions) || option_env!("ROALT_DEV").is_some();

/// The relay this build connects to: the official one in a release build, otherwise a validated
/// custom URL (`wss://`, or `ws://localhost|127.0.0.1` for local dev).
pub fn validate_relay_url(raw: &str) -> Result<String> {
    relay_url_for(raw, CUSTOM_RELAY)
}

fn relay_url_for(raw: &str, custom: bool) -> Result<String> {
    let s = raw.trim();
    if !custom || s.is_empty() || s == LEGACY_RELAY_URL {
        return Ok(RemoteSettings::default().relay_url);
    }
    let u = url::Url::parse(s).map_err(|_| AppError::Invalid("Invalid relay URL.".into()))?;
    let host = u.host_str().unwrap_or("");
    let ok = match u.scheme() {
        "wss" => !host.is_empty(),
        "ws" => host == "localhost" || host == "127.0.0.1" || host == "::1",
        _ => false,
    };
    if !ok {
        return Err(AppError::Invalid("Relay URL must use wss:// (or ws://localhost for local testing).".into()));
    }
    Ok(s.to_owned())
}

/// Loads or creates the PC's static cryptographic identity inside the SQLCipher DB. One caller at
/// a time, so two first uses can't each store a different identity. A stored identity that no
/// longer reads is an error, never silently replaced (that would unpair every device).
fn load_or_create_identity(s: &App) -> Result<PcIdentity> {
    static CREATING: Mutex<()> = Mutex::new(());
    let _one = CREATING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(raw) = s.db.setting_value(IDENTITY_KEY)? {
        let mut id = serde_json::from_str::<PcIdentity>(&raw).map_err(|_| AppError::Crypto)?;
        // Identities from before the relay bound pcIds to the secret: keep the keys, fix the id
        // (browsers paired with the old id pair again).
        let want = crypto::pc_id_for_secret_hash(&id.pc_secret_hash_hex());
        if id.pc_id != want {
            id.pc_id = want;
            let json = serde_json::to_string(&id).map_err(|_| AppError::Crypto)?;
            s.db.set_setting_value(IDENTITY_KEY, Some(&json))?;
        }
        return Ok(id);
    }
    let id = PcIdentity::generate();
    let json = serde_json::to_string(&id).map_err(|_| AppError::Crypto)?;
    s.db.set_setting_value(IDENTITY_KEY, Some(&json))?;
    Ok(id)
}

/// The identity this PC shows the relay it is configured for (`PcIdentity::for_relay`).
fn relay_identity(s: &App, relay_url: &str) -> Result<PcIdentity> {
    load_or_create_identity(s)?.for_relay(relay_url, is_official_relay(relay_url))
}

fn emit_changed(app: &AppHandle) {
    let _ = app.emit("remote://changed", ());
}

/// Current snapshot for the desktop Settings UI & top-bar badge.
pub fn status(app: &AppHandle) -> Result<RemoteStatus> {
    let s = app.state::<App>();
    let settings = s.db.settings()?;
    let rows = s.db.remote_devices()?;
    let pending = s.db.remote_pending_ids()?;
    let g = s.remote.lock();

    let connected_ids: Vec<Uuid> = g.connected_viewers.values().copied().collect();
    let devices = rows
        .into_iter()
        .map(|r| {
            let connected = Uuid::parse_str(&r.id).is_ok_and(|u| connected_ids.contains(&u));
            RemoteDevice {
                pending: pending.contains(&r.id),
                id: r.id,
                label: r.label,
                role: r.role,
                created_at: r.created_at,
                last_seen_at: r.last_seen_at,
                connected,
            }
        })
        .collect();

    let pairing = g.pairing.as_ref().filter(|p| p.expires_at > now_ms()).map(|p| RemotePairSession {
        pair_id: p.pair_id.clone(),
        qr_url: p.qr_url.clone(),
        manual_code: p.manual_code.clone(),
        expires_at: p.expires_at,
        pending: match (&p.stage, p.joiner) {
            (PairStage::Pending(m), Some(viewer_id)) => Some(RemotePairPending {
                viewer_id: viewer_id.to_string(),
                device_id: m.device_id.to_string(),
                label: m.label.clone(),
                sas: m.sas.clone(),
                approved: false,
            }),
            (PairStage::Approved(a), Some(viewer_id)) => Some(RemotePairPending {
                viewer_id: viewer_id.to_string(),
                device_id: a.row.id.clone(),
                label: a.row.label.clone(),
                sas: a.sas.clone(),
                approved: true,
            }),
            _ => None,
        },
        aborted: matches!(p.stage, PairStage::Aborted),
    });

    Ok(RemoteStatus {
        state: g.state,
        pc_id: g.pc_id.map(|u| u.to_string()),
        pc_name: effective_pc_name(&settings.remote),
        relay_url: validate_relay_url(&settings.remote.relay_url).unwrap_or(settings.remote.relay_url),
        error: g.error.clone(),
        devices,
        active_viewers: g.connected_viewers.len() as u32,
        watching: g.watching,
        paused: g.paused,
        custom_relay: CUSTOM_RELAY,
        pairing,
    })
}

/// Opens a new 120-second pairing session (enabling Remote Dashboard if off).
pub fn pair_start(app: &AppHandle) -> Result<RemotePairSession> {
    let s = app.state::<App>();
    if is_paused(app) {
        return Err(paused_refusal());
    }
    let settings = s.db.settings()?;
    if s.db.remote_devices()?.len() >= MAX_DEVICES {
        return Err(AppError::Invalid(TOO_MANY_DEVICES.into()));
    }
    validate_relay_url(&settings.remote.relay_url)?;
    let _ = load_or_create_identity(&s)?;
    if !settings.remote.enabled {
        let mut next = settings.remote.clone();
        next.enabled = true;
        let _ = crate::commands::set_settings(app, &s, crate::model::SettingsPatch { remote: Some(next), ..Default::default() })?;
    }

    let pairing = ActivePairing::new(now_ms() + PAIR_DURATION_MS);
    let session = RemotePairSession {
        pair_id: pairing.pair_id.clone(),
        qr_url: pairing.qr_url.clone(),
        manual_code: pairing.manual_code.clone(),
        expires_at: pairing.expires_at,
        pending: None,
        aborted: false,
    };
    let (pair_id, manual_code) = (pairing.pair_id.clone(), pairing.manual_code.clone());
    if let Some(old) = s.remote.open_pairing(pairing)? {
        if let Some(joiner) = old.joiner {
            send_pair_mode(&s.remote, &old.pair_id, joiner, "denied");
        }
        s.remote.send_ctrl(ControlSignal::ClosePair { pair_id: old.pair_id });
    }
    s.remote.send_ctrl(ControlSignal::OpenPair { pair_id, manual_code });
    emit_changed(app);
    Ok(session)
}

fn paused_refusal() -> AppError {
    AppError::Invalid(tr!("Remote is paused. Resume it before pairing a device."))
}

/// Cancels any active pairing session; a browser that joined it hears so.
pub fn pair_cancel(app: &AppHandle) {
    let s = app.state::<App>();
    let old = s.remote.lock().pairing.take();
    if let Some(p) = old {
        close_pairing(&s.remote, p, "closed");
        emit_changed(app);
    }
}

fn close_pairing(remote: &Remote, p: ActivePairing, mode: &str) {
    if let Some(joiner) = p.joiner {
        send_pair_mode(remote, &p.pair_id, joiner, mode);
    }
    remote.send_ctrl(ControlSignal::ClosePair { pair_id: p.pair_id });
}

fn pairing_open(p: &ActivePairing) -> bool {
    p.expires_at > now_ms() && !matches!(p.stage, PairStage::Aborted)
}

/// Approves or denies the pending pairing request. `sas` must be the code the user was shown
/// (none for a QR join), so a request that changed under the user's click is refused. Approving
/// sends the ack; the device is only enrolled once the browser confirms it (`enroll`).
pub fn pair_approve(app: &AppHandle, viewer_id: &str, approve: bool, role: RemoteRole, sas: Option<&str>) -> Result<()> {
    let s = app.state::<App>();
    let target_vid = Uuid::parse_str(viewer_id).map_err(|_| AppError::NotFound)?;
    if approve && s.db.remote_devices()?.len() >= MAX_DEVICES {
        return Err(AppError::Invalid(TOO_MANY_DEVICES.into()));
    }
    let settings = s.db.settings()?;
    let identity = relay_identity(&s, &validate_relay_url(&settings.remote.relay_url)?)?;
    let pc_name = effective_pc_name(&settings.remote);
    let (pair_id, ack) = {
        let mut g = s.remote.lock();
        let Some(p) = g.pairing.as_mut().filter(|p| p.joiner == Some(target_vid) && p.expires_at > now_ms()) else {
            return Err(AppError::NotFound);
        };
        let PairStage::Pending(m) = &p.stage else {
            return Err(AppError::NotFound);
        };
        if m.sas.as_deref() != sas {
            return Err(AppError::Invalid("The pairing request changed. Check the code again.".into()));
        }
        let pair_id = p.pair_id.clone();
        if !approve {
            g.pairing = None;
            drop(g);
            send_pair_mode(&s.remote, &pair_id, target_vid, "denied");
            s.remote.send_ctrl(ControlSignal::ClosePair { pair_id });
            emit_changed(app);
            return Ok(());
        }
        let (ack, approved) = approval(&identity, &pc_name, &pair_id, m, role)?;
        p.stage = PairStage::Approved(Box::new(approved));
        p.expires_at = confirm_deadline(p.expires_at, now_ms());
        (pair_id, ack)
    };
    s.remote.send_ctrl(ControlSignal::ExtendPair { pair_id: pair_id.clone() });
    s.remote.send_ctrl(ControlSignal::SendPairMsg { pair_id, viewer_id: target_vid, data: ack });
    emit_changed(app);
    Ok(())
}

fn confirm_deadline(expires_at: i64, now: i64) -> i64 {
    expires_at.max(now + PAIR_CONFIRM_MS)
}

fn send_pair_json(remote: &Remote, pair_id: &str, viewer_id: Uuid, msg: &serde_json::Value) {
    let data = URL_SAFE_NO_PAD.encode(msg.to_string());
    remote.send_ctrl(ControlSignal::SendPairMsg { pair_id: pair_id.to_owned(), viewer_id, data });
}

fn send_pair_mode(remote: &Remote, pair_id: &str, viewer_id: Uuid, mode: &str) {
    send_pair_json(remote, pair_id, viewer_id, &serde_json::json!({ "mode": mode }));
}

pub fn device_revoke(app: &AppHandle, device_id: &str) -> Result<()> {
    let s = app.state::<App>();
    s.db.remote_device_remove(device_id)?;
    s.remote.send_ctrl(ControlSignal::SyncTokens);
    emit_changed(app);
    Ok(())
}

pub fn device_update(app: &AppHandle, device_id: &str, label: Option<String>, role: Option<RemoteRole>) -> Result<()> {
    let s = app.state::<App>();
    if !s.db.remote_device_update(device_id, label.as_deref(), role)? {
        return Err(AppError::NotFound);
    }
    // Reload session roles / broadcast updated snapshot.
    s.remote.send_ctrl(ControlSignal::SyncTokens);
    emit_changed(app);
    Ok(())
}

/// Pausing drops every viewer and lets none back in, also after a restart, until resumed.
pub fn set_paused(app: &AppHandle, paused: bool) -> Result<()> {
    let s = app.state::<App>();
    s.db.set_setting_value(PAUSED_KEY, paused.then_some("1"))?;
    s.remote.pause(paused);
    emit_changed(app);
    crate::tray::refresh(app);
    Ok(())
}

fn is_paused(app: &AppHandle) -> bool {
    app.state::<App>().remote.lock().paused
}

pub fn watching(app: &AppHandle) -> bool {
    app.state::<App>().remote.lock().watching > 0
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", rename_all_fields = "camelCase")]
enum PairJoinMsg {
    Qr { nonce_b64: String, blob: String },
    ManualCommit { commit_b64: String },
    ManualReveal { ephem_pub_b64: String, nonce_b64: String, blob: String },
    /// The browser accepted the ack (after the user confirmed on it): enroll the device. `blob` is
    /// a `SealedDone` sealed under the pair key; without a valid one the pairing is aborted. A
    /// browser that gives up closes its socket instead (`pair_left`).
    Done { blob: String },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceBinding {
    device_id: Uuid,
    device_pub_b64: String,
    label: String,
    sig_b64: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PairAckPayload {
    pc_id: Uuid,
    pc_name: String,
    pc_pub_b64: String,
    room_token: String,
    role: RemoteRole,
    sig_b64: String,
}

/// Characters that draw nothing, reorder text or stack on what is next to it: every format (Cf)
/// character of Unicode 15.1 (bidi controls included), line/paragraph separators (Zl, Zp), variation
/// selectors, fillers, tag characters and the like.
fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061C}'
            | '\u{06DD}'
            | '\u{070F}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08E2}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{2028}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{2800}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

/// Every mark (Mn, Mc, Me): at most one may follow each other character.
fn is_combining(c: char) -> bool {
    static MARK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"^\p{M}$").expect("a valid pattern"));
    MARK.is_match(c.encode_utf8(&mut [0; 4]))
}

/// A device label as the approval card shows it: no control or invisible characters, no stacks
/// of combining marks, whitespace collapsed, at most 64 characters.
fn sanitize_label(raw: &str) -> String {
    let mut clean = String::new();
    let mut marks = 0;
    for c in raw.chars().filter(|c| !c.is_control() && !is_invisible_format(*c)) {
        if is_combining(c) {
            marks += 1;
            if marks > 1 || clean.is_empty() {
                continue;
            }
        } else {
            marks = 0;
        }
        if c.is_whitespace() {
            if !clean.is_empty() && !clean.ends_with(' ') {
                clean.push(' ');
            }
            continue;
        }
        clean.push(c);
    }
    let trimmed = clean.trim();
    if trimmed.is_empty() {
        "Remote browser".into()
    } else {
        trimmed.chars().take(64).collect()
    }
}

/// The sealed ack for an approved request (base64 `pair_msg` data) and what enrolling needs later.
fn approval(identity: &PcIdentity, pc_name: &str, pair_id: &str, pending: &PendingPair, role: RemoteRole) -> Result<(String, ApprovedPair)> {
    let room_token = random_token();
    let now = now_ms();
    let ack = PairAckPayload {
        pc_id: identity.pc_id,
        pc_name: pc_name.to_owned(),
        pc_pub_b64: URL_SAFE_NO_PAD.encode(identity.public_key_bytes()?),
        room_token: room_token.clone(),
        role,
        sig_b64: URL_SAFE_NO_PAD.encode(sign_pc_binding(&identity.signing_key()?, &pending.transcript, identity.pc_id, pending.device_id)),
    };
    let ack_bytes = serde_json::to_vec(&ack).map_err(|_| AppError::Crypto)?;
    let sealed = seal_pair_blob(&pending.pair_key, DIR_PC_TO_VIEWER, pair_id, &ack_bytes)?;
    let wrapped = serde_json::json!({ "mode": "ack", "blob": sealed }).to_string();
    let row = RemoteDeviceRow {
        id: pending.device_id.to_string(),
        label: pending.label.clone(),
        pubkey: pending.device_pubkey.clone(),
        room_token,
        role,
        created_at: now,
        last_seen_at: None,
    };
    let approved = ApprovedPair { row, pair_key: pending.pair_key.clone(), transcript: pending.transcript, sas: pending.sas.clone() };
    Ok((URL_SAFE_NO_PAD.encode(wrapped.as_bytes()), approved))
}

#[derive(Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
enum SealedDone {
    Done {
        #[serde(default)]
        replaces: Vec<Replaces>,
    },
}

fn done_proof(approved: &ApprovedPair, pair_id: &str, blob: &str) -> Option<Vec<Replaces>> {
    let plain = open_pair_blob(&approved.pair_key, DIR_VIEWER_TO_PC, pair_id, blob).ok()?;
    let SealedDone::Done { replaces } = serde_json::from_slice(&plain).ok()?;
    Some(replaces)
}

/// `{mode:"enrolled", blob}` for `pair_msg`: the blob seals `{"mode":"enrolled"}` under the pair
/// key, so the relay can't make the browser save a pairing the PC never wrote.
fn enrolled_msg(pair_key: &[u8; 32], pair_id: &str) -> Result<serde_json::Value> {
    let blob = seal_pair_blob(pair_key, DIR_PC_TO_VIEWER, pair_id, br#"{"mode":"enrolled"}"#)?;
    Ok(serde_json::json!({ "mode": "enrolled", "blob": blob }))
}

/// The browser re-pairing a PC it was paired with before: the old device's id, and that device
/// key's signature over this pairing's transcript (`verify_device_binding`), so its row goes.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Replaces {
    device_id: Uuid,
    sig_b64: String,
}

fn proven_replacements(replaces: &[Replaces], new_id: &str, transcript: &[u8; 32], stored_pubkey: impl Fn(&str) -> Option<Vec<u8>>) -> Vec<String> {
    let mut proven: Vec<String> = Vec::new();
    for r in replaces {
        let id = r.device_id.to_string();
        let holds_key = || {
            stored_pubkey(&id).is_some_and(|pubkey| URL_SAFE_NO_PAD.decode(&r.sig_b64).is_ok_and(|sig| verify_device_binding(&pubkey, transcript, r.device_id, &sig).is_ok()))
        };
        if id != new_id && !proven.contains(&id) && holds_key() {
            proven.push(id);
        }
    }
    proven
}

/// The browser accepted the ack: writes the device row, pending until the device's first session
/// (which also drops the rows it replaces, those whose keys it proved it holds), lets its token in,
/// tells the browser, and shows the toast.
fn enroll(app: &AppHandle, pair_id: &str, viewer_id: Uuid, approved: ApprovedPair, replaces: Vec<Replaces>) {
    let s = app.state::<App>();
    let row = approved.row;
    let replaced = proven_replacements(&replaces, &row.id, &approved.transcript, |id| s.db.remote_device(id).ok().flatten().map(|old| old.pubkey));
    let saved = enrolled_msg(&approved.pair_key, pair_id).and_then(|msg| {
        s.db.remote_device_insert(&row, PENDING_DEVICE_MS, &replaced)?;
        Ok(msg)
    });
    let msg = match saved {
        Ok(msg) => msg,
        Err(e) => {
            log::warn!("remote: could not save the paired device: {e}");
            send_pair_mode(&s.remote, pair_id, viewer_id, "denied");
            s.remote.send_ctrl(ControlSignal::ClosePair { pair_id: pair_id.to_owned() });
            emit_changed(app);
            return;
        }
    };
    s.remote.send_ctrl(ControlSignal::SyncTokens);
    send_pair_json(&s.remote, pair_id, viewer_id, &msg);
    s.remote.send_ctrl(ControlSignal::ClosePair { pair_id: pair_id.to_owned() });
    log::info!("remote: paired new device {:?} ({}), waiting for its first connection", row.label, row.id);
    emit_changed(app);
}

/// Spawns the background relay supervisor task for the app's lifetime.
pub fn spawn(app: AppHandle) {
    let (ctrl_tx, ctrl_rx) = mpsc::unbounded_channel();
    let paused = app.state::<App>().db.setting_value(PAUSED_KEY).ok().flatten().is_some();
    {
        let s = app.state::<App>();
        let mut g = s.remote.lock();
        g.ctrl_tx = Some(ctrl_tx);
        g.paused = paused;
    }
    tauri::async_runtime::spawn(supervisor(app, ctrl_rx));
}

/// Rewrites settings that still name the relay's old address: they load as the default
/// (`RemoteSettings`), and saving them drops the old address for good. Off the main thread, as
/// `Db::settings_change` wants.
fn forget_legacy_relay(s: &App) {
    let _change = s.db.settings_change();
    let stored = s.db.setting_value("settings").ok().flatten().filter(|json| json.contains(crate::model::LEGACY_RELAY_URL));
    if let Some(settings) = stored.and_then(|json| serde_json::from_str::<crate::model::Settings>(&json).ok())
        && let Err(e) = s.db.save_settings(&settings)
    {
        log::warn!("remote: could not update the stored relay address: {e}");
    }
}

fn set_state(app: &AppHandle, state: RemoteState, error: Option<String>) {
    let s = app.state::<App>();
    let changed = {
        let mut g = s.remote.lock();
        let diff = g.state != state || g.error != error;
        g.state = state;
        g.error = error;
        diff
    };
    if changed {
        emit_changed(app);
    }
}

fn forget_viewers(app: &AppHandle) {
    let s = app.state::<App>();
    let had = {
        let mut g = s.remote.lock();
        let had = !g.connected_viewers.is_empty() || g.watching > 0;
        g.connected_viewers.clear();
        g.watching = 0;
        had
    };
    if had {
        emit_changed(app);
        crate::tray::refresh(app);
    }
}

/// The relay forgets a pairing room, and who joined it, with the connection: a pairing someone
/// already joined can't finish, so it stops instead of waiting for an Approve that goes nowhere.
/// One nobody joined is opened again on the next connection.
fn strand_joined_pairing(app: &AppHandle) {
    let s = app.state::<App>();
    let stranded = s.remote.lock().pairing.as_mut().is_some_and(ActivePairing::strand);
    if stranded {
        log::info!("remote: the relay connection dropped during a pairing; pairing stopped");
        emit_changed(app);
    }
}

fn jittered(cap: Duration) -> Duration {
    let cap_ms = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX).max(1);
    Duration::from_millis(500 + u64::from_le_bytes(random_bytes::<8>()) % cap_ms)
}

async fn supervisor(app: AppHandle, mut ctrl_rx: mpsc::UnboundedReceiver<ControlSignal>) {
    forget_legacy_relay(&app.state::<App>());
    let mut backoff = Duration::from_secs(2);

    loop {
        // Drain any stale control signals while disconnected.
        while ctrl_rx.try_recv().is_ok() {}

        let (settings, devices_empty, pairing_active, public_net) = {
            let s = app.state::<App>();
            let st = s.db.settings().unwrap_or_default();
            let empty = s.db.remote_devices().map(|d| d.is_empty()).unwrap_or(true) || s.remote.lock().paused;
            let pairing = s.remote.lock().pairing.as_ref().is_some_and(pairing_open);
            let pub_net = s.network.is_public();
            (st.remote, empty, pairing, pub_net)
        };

        if !settings.enabled {
            set_state(&app, RemoteState::Off, None);
            backoff = Duration::from_secs(2);
            wait_for_wake(&app, &mut ctrl_rx).await;
            continue;
        }

        if settings.block_on_public_network && public_net {
            set_state(&app, RemoteState::PausedPublic, None);
            backoff = Duration::from_secs(2);
            wait_for_wake(&app, &mut ctrl_rx).await;
            continue;
        }

        if devices_empty && !pairing_active {
            set_state(&app, RemoteState::Idle, None);
            backoff = Duration::from_secs(2);
            wait_for_wake(&app, &mut ctrl_rx).await;
            continue;
        }

        let relay_url = match validate_relay_url(&settings.relay_url) {
            Ok(u) => u,
            Err(e) => {
                set_state(&app, RemoteState::Error, Some(e.message()));
                wait_for_wake(&app, &mut ctrl_rx).await;
                continue;
            }
        };

        let identity = match relay_identity(&app.state::<App>(), &relay_url) {
            Ok(id) => id,
            Err(e) => {
                set_state(&app, RemoteState::Error, Some(e.message()));
                tokio::time::sleep(jittered(backoff)).await;
                continue;
            }
        };
        app.state::<App>().remote.lock().pc_id = Some(identity.pc_id);
        set_state(&app, RemoteState::Connecting, None);

        let started = Instant::now();
        let ended = run_connection(&app, &relay_url, &identity, &mut ctrl_rx).await;
        // Every phone is gone with the connection: nothing they held may stay pressed.
        crate::instances::remote_input::release_all();
        forget_viewers(&app);
        strand_joined_pairing(&app);
        match ended {
            Ok(()) => {
                // Clean reload / disable.
                backoff = Duration::from_secs(2);
            }
            Err(ConnEnd::Upgrade) => {
                log::warn!("remote: the relay wants a newer RoALT");
                set_state(&app, RemoteState::Error, Some(upgrade_required()));
                wait_for_wake(&app, &mut ctrl_rx).await;
            }
            Err(ConnEnd::Failed(msg)) => {
                log::info!("remote: relay disconnected: {msg}");
                set_state(&app, RemoteState::Error, Some(msg));
                if started.elapsed() > Duration::from_secs(30) {
                    backoff = Duration::from_secs(10);
                }
                while ctrl_rx.try_recv().is_ok() {}
                let _ = app.state::<App>().remote.wake.notified().now_or_never();
                tokio::select! {
                    _ = tokio::time::sleep(jittered(backoff)) => {}
                    _ = wait_for_wake(&app, &mut ctrl_rx) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        }
    }
}

async fn wait_for_wake(app: &AppHandle, ctrl_rx: &mut mpsc::UnboundedReceiver<ControlSignal>) {
    let s = app.state::<App>();
    tokio::select! {
        _ = s.remote.wake.notified() => {}
        _ = ctrl_rx.recv() => {}
    }
}

fn current_token_hashes(app: &AppHandle) -> Vec<String> {
    if is_paused(app) {
        return Vec::new();
    }
    app.state::<App>()
        .db
        .remote_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| sha256_hex(&d.room_token))
        .collect()
}

fn wrap_binary(viewer_id: Uuid, payload: &[u8]) -> WsMsg {
    let mut out = Vec::with_capacity(BIN_HEADER_LEN + payload.len());
    out.push(BIN_VERSION);
    out.extend_from_slice(viewer_id.as_bytes());
    out.extend_from_slice(payload);
    WsMsg::Binary(out.into())
}

fn fits_frame(plaintext_len: usize) -> bool {
    BIN_HEADER_LEN + SEAL_OVERHEAD + plaintext_len <= MAX_WIRE_FRAME
}

fn kick(viewer_id: Uuid, reason: Option<&str>) -> WsMsg {
    WsMsg::Text(serde_json::json!({"t": "kick_viewer", "viewerId": viewer_id, "code": reason}).to_string().into())
}

fn join_refusal(paused: bool, crowded: bool) -> Option<&'static str> {
    if paused {
        Some("paused")
    } else if crowded {
        Some("device_sessions")
    } else {
        None
    }
}

fn pc_name(app: &AppHandle) -> String {
    effective_pc_name(&app.state::<App>().db.settings().unwrap_or_default().remote)
}

fn previews_allowed(app: &AppHandle, role: RemoteRole) -> bool {
    role == RemoteRole::Control || app.state::<App>().db.settings().is_ok_and(|s| s.remote.allow_view_previews)
}

/// The device's role now, or none once it is revoked or Remote is paused.
fn current_role(app: &AppHandle, device_id: Uuid) -> Option<RemoteRole> {
    if is_paused(app) {
        return None;
    }
    app.state::<App>().db.remote_device(&device_id.to_string()).ok().flatten().map(|d| d.role)
}

fn rpc_error(req_id: u32, code: &str, error: String) -> RemoteEvent {
    RemoteEvent::RpcResult { req_id, ok: false, data: None, error: Some(error), code: Some(code.to_owned()) }
}

/// Frames to the relay go through a bounded queue drained by a task of their own, so a relay that
/// stops reading never blocks the session loop (input, revoke, pause), and a send stuck for
/// `SEND_TIMEOUT` ends the connection.
struct Outbound(mpsc::Sender<WsMsg>);

impl Outbound {
    fn spawn<S>(mut sink: S) -> (Self, tokio::task::JoinHandle<()>)
    where
        S: Sink<WsMsg> + Unpin + Send + 'static,
        S::Error: Send,
    {
        let (tx, mut rx) = mpsc::channel::<WsMsg>(OUTBOUND_QUEUE);
        let writer = tokio::spawn(async move {
            while let Some(msg) = rx.recv().await {
                let closing = matches!(msg, WsMsg::Close(_));
                let sent = tokio::time::timeout(SEND_TIMEOUT, sink.send(msg)).await;
                if closing || !matches!(sent, Ok(Ok(()))) {
                    break;
                }
            }
        });
        (Self(tx), writer)
    }

    async fn send(&self, msg: WsMsg) {
        let _ = self.0.send(msg).await;
    }

    /// Queues a preview only while fewer than `PREVIEW_BACKLOG` frames wait: previews are dropped
    /// rather than wait (a newer one comes), and never crowd out an input ack, a revoke or a pong.
    fn offer(&self, msg: WsMsg) -> bool {
        self.0.max_capacity() - self.0.capacity() < PREVIEW_BACKLOG && self.0.try_send(msg).is_ok()
    }

    async fn text(&self, value: serde_json::Value) {
        self.send(WsMsg::Text(value.to_string().into())).await;
    }

    async fn close(&self) {
        self.send(WsMsg::Close(None)).await;
    }
}

/// An event too big for one frame becomes a short error the dashboard shows instead of waiting.
fn oversized(ev: &RemoteEvent, len: usize) -> Option<RemoteEvent> {
    log::warn!("remote: a {len}-byte event is over the relay's frame cap");
    match ev {
        RemoteEvent::RpcResult { req_id, .. } => {
            Some(rpc_error(*req_id, "too_large", "The answer is too big to send to the dashboard.".into()))
        }
        RemoteEvent::Snapshot { .. } => Some(RemoteEvent::Notice {
            error: true,
            message: "This PC has more data than Remote can send in one go. Close some clients and reload the dashboard.".into(),
        }),
        _ => Some(RemoteEvent::Notice {
            error: true,
            message: "An update from this PC was too big to send, so what you see may be out of date. Reload the dashboard.".into(),
        }),
    }
}

fn sealed_frame(vid: Uuid, cipher: &mut SessionCipher, ev: &RemoteEvent) -> Option<WsMsg> {
    let pt = encode_json(ev)?;
    if !fits_frame(pt.len()) {
        let small = encode_json(&oversized(ev, pt.len())?)?;
        return cipher.seal(&small).ok().map(|ct| wrap_binary(vid, &ct));
    }
    cipher.seal(&pt).ok().map(|ct| wrap_binary(vid, &ct))
}

async fn send_event_to(out: &Outbound, vid: Uuid, cipher: &mut SessionCipher, ev: &RemoteEvent) {
    if let Some(frame) = sealed_frame(vid, cipher, ev) {
        out.send(frame).await;
    }
}

async fn broadcast_event(out: &Outbound, sessions: &mut HashMap<Uuid, ViewerSession>, ev: &RemoteEvent) {
    for (vid, sess) in sessions.iter_mut() {
        if sess.confirmed
            && let Some(cipher) = sess.cipher.as_mut()
        {
            send_event_to(out, *vid, cipher, ev).await;
        }
    }
}

fn publish_watching(app: &AppHandle, sessions: &HashMap<Uuid, ViewerSession>) {
    let n = u32::try_from(sessions.values().filter(|s| !s.previews.is_empty()).count()).unwrap_or(u32::MAX);
    let s = app.state::<App>();
    let changed = std::mem::replace(&mut s.remote.lock().watching, n) != n;
    if changed {
        emit_changed(app);
        crate::tray::refresh(app);
    }
}

fn device_sessions(sessions: &HashMap<Uuid, ViewerSession>, did: Uuid, vid: Uuid) -> usize {
    sessions.iter().filter(|(v, s)| s.device_id == did && **v != vid).count()
}

/// One relay connection's viewers and how to reach them.
struct Link<'a> {
    app: &'a AppHandle,
    identity: &'a PcIdentity,
    out: Outbound,
    sessions: HashMap<Uuid, ViewerSession>,
    budgets: HashMap<Uuid, DeviceBudget>,
    rpc_out_tx: mpsc::UnboundedSender<(Uuid, RemoteEvent)>,
    broadcast_at: Option<Instant>,
    last_broadcast: Instant,
    pending: PendingClock,
}

struct PendingClock {
    last: Instant,
}

impl PendingClock {
    fn new(now: Instant) -> Self {
        Self { last: now }
    }

    fn spend(&mut self, db: &crate::storage::Db, now: Instant, counting: bool) -> Vec<String> {
        let spent = now.saturating_duration_since(std::mem::replace(&mut self.last, now)).min(PENDING_TICK_MAX);
        if !counting {
            return Vec::new();
        }
        db.remote_devices_spend_pending(i64::try_from(spent.as_millis()).unwrap_or(i64::MAX)).unwrap_or_default()
    }
}

impl Link<'_> {
    /// Forgets a session and lets go of the input it holds (only its own).
    fn forget(&mut self, vid: Uuid) {
        if self.sessions.remove(&vid).is_some() {
            crate::instances::remote_input::release_owner(vid);
        }
        if self.app.state::<App>().remote.lock().connected_viewers.remove(&vid).is_some() {
            emit_changed(self.app);
        }
    }

    async fn drop_viewer(&mut self, vid: Uuid) {
        self.forget(vid);
        self.out.send(kick(vid, None)).await;
    }

    async fn viewer_joined(&mut self, vid: Uuid, did: Uuid, token_hash: &str) {
        let app = self.app;
        let dev = app.state::<App>().db.remote_device(&did.to_string()).ok().flatten().filter(|d| sha256_hex(&d.room_token) == token_hash);
        let refusal = join_refusal(is_paused(app), device_sessions(&self.sessions, did, vid) >= SESSIONS_PER_DEVICE);
        let (Some(dev), None) = (dev, refusal) else {
            self.out.send(kick(vid, refusal)).await;
            return;
        };
        let Ok(signing) = self.identity.signing_key() else { return };
        let ephem_sk = new_ephemeral_key();
        let ephem_pub = pubkey_bytes(&ephem_sk);
        let hs = build_handshake(&signing, &ephem_sk, DIR_PC_TO_VIEWER, self.identity.pc_id, did);
        self.out.send(wrap_binary(vid, &hs)).await;
        self.forget(vid);
        self.sessions.insert(vid, ViewerSession {
            device_id: did,
            device_label: dev.label,
            device_pubkey: dev.pubkey,
            role: dev.role,
            my_ephem_sk: ephem_sk,
            my_ephem_pub: ephem_pub,
            cipher: None,
            confirmed: false,
            last_rx: Instant::now(),
            snapshot_due: false,
            previews: Vec::new(),
            preview_flows: HashMap::new(),
            releases_refused: false,
        });
    }

    async fn on_frame(&mut self, vid: Uuid, payload: &[u8]) {
        let app = self.app;
        let Some(sess) = self.sessions.get_mut(&vid) else { return };

        if payload[0] == FRAME_HANDSHAKE && sess.cipher.is_none() {
            let Ok(viewer_ephem_pub) = verify_handshake(&sess.device_pubkey, payload, DIR_VIEWER_TO_PC, self.identity.pc_id, sess.device_id) else {
                log::warn!("remote: invalid handshake from viewer {vid}; kicking");
                self.drop_viewer(vid).await;
                return;
            };
            if let Ok(cipher) = SessionCipher::derive(&sess.my_ephem_sk, &viewer_ephem_pub, &sess.my_ephem_pub, &viewer_ephem_pub, true) {
                sess.cipher = Some(cipher);
            }
            return;
        }
        if payload[0] != FRAME_DATA {
            return;
        }
        let Some(cipher) = sess.cipher.as_mut() else { return };
        let Ok(pt) = cipher.open(payload) else {
            log::warn!("remote: decrypt/replay error from viewer {vid}; kicking");
            self.drop_viewer(vid).await;
            return;
        };
        let now = Instant::now();
        sess.last_rx = now;
        if !sess.confirmed {
            sess.confirmed = true;
            sess.snapshot_due = true;
            let first = app.state::<App>().db.remote_device_touch_seen(&sess.device_id.to_string(), now_ms());
            app.state::<App>().remote.lock().connected_viewers.insert(vid, sess.device_id);
            if let Ok(Some(first)) = first {
                log::info!("remote: device {:?} connected for the first time", sess.device_label);
                let _ = app.emit("remote://pairing", serde_json::json!({ "outcome": "paired", "label": sess.device_label }));
                if !first.replaced.is_empty() {
                    app.state::<App>().remote.send_ctrl(ControlSignal::SyncTokens);
                }
            }
            emit_changed(app);
        }
        if pt.first() != Some(&APP_KIND_JSON) {
            return;
        }
        let budget = self.budgets.entry(sess.device_id).or_insert_with(|| DeviceBudget::new(now));
        let req = match serde_json::from_slice::<RemoteRpcRequest>(&pt[1..]) {
            Ok(r) => r,
            Err(_) if !budget.rpcs.take(now) => return,
            Err(e) => {
                // Answer what we can't parse, so the dashboard shows an error instead of a 15 s timeout.
                let val = serde_json::from_slice::<serde_json::Value>(&pt[1..]).ok();
                let op = val
                    .as_ref()
                    .and_then(|v| v.get("op")?.as_str())
                    .filter(|op| op.len() <= 32 && op.chars().all(|c| c.is_ascii_alphanumeric()))
                    .unwrap_or("?");
                log::warn!("remote: unreadable request from {}: {:?} error, op {op}, {} bytes", sess.device_label, e.classify(), pt.len() - 1);
                let req_id = val.as_ref().and_then(|v| v.get("reqId")?.as_u64()).and_then(|n| u32::try_from(n).ok());
                if let Some(req_id) = req_id {
                    let ev = rpc_error(req_id, "bad_request", "RoALT did not understand this request. Reload the dashboard or update RoALT.".into());
                    send_event_to(&self.out, vid, cipher, &ev).await;
                }
                return;
            }
        };
        let release = dispatch::is_release(&req.cmd);
        let refused = match &req.cmd {
            RemoteCommand::PreviewAck { .. } | RemoteCommand::RequestSnapshot => false,
            _ if release => match refuse_lift(&mut budget.releases, &mut sess.releases_refused, now) {
                None => false,
                Some(false) => return,
                Some(true) => {
                    if sess.role == RemoteRole::Control {
                        crate::instances::remote_input::release_owner(vid);
                    }
                    true
                }
            },
            RemoteCommand::InputTouch { .. } | RemoteCommand::InputKey { .. } | RemoteCommand::InputCamera { .. } => !budget.inputs.take(now),
            RemoteCommand::SubscribePreviews { .. } => !budget.rpcs.take(now),
            _ => budget.in_flight.load(Ordering::SeqCst) >= RPCS_IN_FLIGHT || !budget.rpcs.take(now),
        };
        if refused {
            let ev = rpc_error(req.req_id, "rate_limited", "Too many requests. Wait a moment and try again.".into());
            send_event_to(&self.out, vid, cipher, &ev).await;
            return;
        }
        match req.cmd {
            RemoteCommand::RequestSnapshot => sess.snapshot_due = true,
            RemoteCommand::PreviewAck { instance_id, seq } => {
                if instance_id.len() <= preview::MAX_INSTANCE_ID
                    && let Some(flow) = sess.preview_flows.get_mut(&instance_id)
                {
                    flow.acked(seq, now);
                }
            }
            RemoteCommand::SubscribePreviews { mut targets } => {
                let refused = !targets.is_empty() && !previews_allowed(app, sess.role);
                if refused {
                    targets.clear();
                }
                preview::clamp_targets(&mut targets);
                sess.previews = targets;
                sess.preview_flows.retain(|id, _| sess.previews.iter().any(|t| &t.instance_id == id));
                let ev = if refused {
                    rpc_error(req.req_id, "previews_off", "Live previews are off for view-only devices on this PC.".into())
                } else {
                    RemoteEvent::RpcResult {
                        req_id: req.req_id,
                        ok: true,
                        data: Some(serde_json::json!({"subscribed": sess.previews.len()})),
                        error: None,
                        code: None,
                    }
                };
                send_event_to(&self.out, vid, cipher, &ev).await;
            }
            cmd @ (RemoteCommand::InputTouch { .. } | RemoteCommand::InputKey { .. } | RemoteCommand::InputCamera { .. }) => {
                let ev = dispatch::input(app, vid, sess.device_id, sess.role, req.req_id, cmd);
                send_event_to(&self.out, vid, cipher, &ev).await;
            }
            other => {
                let app_c = app.clone();
                let label = sess.device_label.clone();
                let device_id = sess.device_id;
                let out_tx = self.rpc_out_tx.clone();
                let req_id = req.req_id;
                let slot = InFlight::start(&budget.in_flight);
                tauri::async_runtime::spawn(async move {
                    let _slot = slot;
                    let ev = match current_role(&app_c, device_id) {
                        Some(role) => dispatch::execute(&app_c, &label, role, req_id, other).await,
                        None => rpc_error(req_id, "forbidden", "This device is no longer allowed on this PC.".into()),
                    };
                    let _ = out_tx.send((vid, ev));
                });
            }
        }
    }

    /// Sends the snapshots sessions are waiting for, as far as each device's snapshot budget goes.
    async fn flush_snapshots(&mut self) {
        let now = Instant::now();
        let mut name = None;
        for (vid, sess) in &mut self.sessions {
            if !sess.snapshot_due {
                continue;
            }
            let Some(cipher) = sess.cipher.as_mut() else { continue };
            if !self.budgets.entry(sess.device_id).or_insert_with(|| DeviceBudget::new(now)).snapshots.take(now) {
                continue;
            }
            sess.snapshot_due = false;
            let name = name.get_or_insert_with(|| pc_name(self.app));
            for ev in dispatch::build_snapshot(self.app, name, sess.role) {
                send_event_to(&self.out, *vid, cipher, &ev).await;
            }
        }
    }

    async fn broadcast(&mut self, ev: &RemoteEvent) {
        broadcast_event(&self.out, &mut self.sessions, ev).await;
    }

    async fn flush_broadcasts(&mut self) {
        let app = self.app;
        self.broadcast_at = None;
        self.last_broadcast = Instant::now();
        let due = std::mem::take(&mut app.state::<App>().remote.lock().broadcasts);
        let mut events = Vec::new();
        if due.accounts {
            events.extend(dispatch::accounts_events(app));
        }
        if due.instances {
            events.extend(dispatch::instances_events(app));
        }
        for progress in &due.bulk {
            events.extend(dispatch::bulk_events(progress));
        }
        if due.lock_state {
            events.extend(dispatch::metrics_events(app));
        }
        for ev in &events {
            self.broadcast(ev).await;
        }
    }

    async fn on_relay_text(&mut self, val: &serde_json::Value) -> std::result::Result<(), ConnEnd> {
        let app = self.app;
        let str_of = |key: &str| val.get(key).and_then(|v| v.as_str());
        let uuid_of = |key: &str| str_of(key).and_then(|s| Uuid::parse_str(s).ok());
        match str_of("t").unwrap_or("") {
            "pc_ready" => set_state(app, RemoteState::Connected, None),
            "viewer_joined" => {
                if let (Some(vid), Some(did)) = (uuid_of("viewerId"), uuid_of("deviceId")) {
                    self.viewer_joined(vid, did, str_of("tokenHash").unwrap_or("")).await;
                }
            }
            "viewer_left" => {
                if let Some(vid) = uuid_of("viewerId") {
                    self.forget(vid);
                }
            }
            "pair_joined" => {
                if let (Some(pair_id), Some(vid), Some(data)) = (str_of("pairId"), uuid_of("viewerId"), str_of("data")) {
                    handle_pair_join(app, pair_id, vid, data);
                }
            }
            "pair_left" => {
                if let (Some(pair_id), Some(vid)) = (str_of("pairId"), uuid_of("viewerId")) {
                    handle_pair_left(app, pair_id, vid);
                }
            }
            "err" => match str_of("code").unwrap_or("unknown") {
                "forbidden" => return Err("Relay rejected PC credentials".into()),
                "upgrade_required" => return Err(ConnEnd::Upgrade),
                "pair_taken" => {
                    if let Some((pair_id, manual_code)) = str_of("pairId").and_then(|pid| renew_taken_pairing(app, pid)) {
                        self.out.text(serde_json::json!({ "t": "pair_open", "pairId": pair_id, "manualCode": manual_code })).await;
                    }
                }
                other => log::warn!("remote: relay error {:?}", other.chars().take(32).collect::<String>()),
            },
            _ => {}
        }
        Ok(())
    }

    /// Handles a control signal; true when the connection should end cleanly.
    async fn on_signal(&mut self, sig: ControlSignal, relay_url: &str) -> bool {
        let app = self.app;
        let pairing_open = || app.state::<App>().remote.lock().pairing.as_ref().is_some_and(pairing_open);
        let no_devices = || app.state::<App>().db.remote_devices().map(|d| d.is_empty()).unwrap_or(true);
        let nothing_paired = || is_paused(app) || no_devices();
        match sig {
            ControlSignal::Reload => {
                let s = app.state::<App>();
                let st = s.db.settings().unwrap_or_default().remote;
                let relay_moved = validate_relay_url(&st.relay_url).ok().as_deref() != Some(relay_url);
                if !st.enabled || (st.block_on_public_network && s.network.is_public()) || (nothing_paired() && !pairing_open()) || relay_moved {
                    self.out.close().await;
                    return true;
                }
                if !st.allow_view_previews {
                    for sess in self.sessions.values_mut().filter(|s| s.role == RemoteRole::Readonly) {
                        sess.previews.clear();
                        sess.preview_flows.clear();
                    }
                }
            }
            ControlSignal::Reconnect => {
                self.out.close().await;
                return true;
            }
            ControlSignal::OpenPair { pair_id, manual_code } => {
                self.out.text(serde_json::json!({ "t": "pair_open", "pairId": pair_id, "manualCode": manual_code })).await;
            }
            ControlSignal::ClosePair { pair_id } => {
                self.out.text(serde_json::json!({ "t": "pair_close", "pairId": pair_id })).await;
                // Cancelled or denied with nothing paired: go idle instead of holding the socket.
                if !pairing_open() && no_devices() {
                    self.out.close().await;
                    return true;
                }
            }
            ControlSignal::ExtendPair { pair_id } => {
                self.out.text(serde_json::json!({ "t": "pair_extend", "pairId": pair_id })).await;
            }
            ControlSignal::SendPairMsg { pair_id, viewer_id, data } => {
                self.out.text(serde_json::json!({ "t": "pair_msg", "pairId": pair_id, "viewerId": viewer_id, "data": data })).await;
            }
            ControlSignal::SyncTokens => {
                self.out.text(serde_json::json!({ "t": "pc_tokens", "allowedTokens": current_token_hashes(app) })).await;
                let devs = app.state::<App>().db.remote_devices().unwrap_or_default();
                // A revoked device must lose its live session now, not on its next reconnect.
                let revoked: Vec<Uuid> = self
                    .sessions
                    .iter()
                    .filter(|(_, sess)| !devs.iter().any(|d| d.id == sess.device_id.to_string()))
                    .map(|(vid, _)| *vid)
                    .collect();
                for vid in revoked {
                    self.drop_viewer(vid).await;
                }
                if devs.is_empty() && !pairing_open() {
                    self.out.close().await;
                    return true;
                }
                let pc_name = pc_name(app);
                for (vid, sess) in &mut self.sessions {
                    let Some(d) = devs.iter().find(|d| d.id == sess.device_id.to_string()) else { continue };
                    if sess.role != d.role {
                        crate::instances::remote_input::release_owner(*vid);
                    }
                    sess.role = d.role;
                    sess.device_label = d.label.clone();
                    if !previews_allowed(app, sess.role) {
                        sess.previews.clear();
                        sess.preview_flows.clear();
                    }
                    if sess.confirmed
                        && let Some(cipher) = sess.cipher.as_mut()
                    {
                        for ev in dispatch::build_snapshot(app, &pc_name, sess.role) {
                            send_event_to(&self.out, *vid, cipher, &ev).await;
                        }
                    }
                }
            }
            ControlSignal::Pause => {
                crate::instances::remote_input::release_all();
                let vids: Vec<Uuid> = self.sessions.keys().copied().collect();
                for vid in vids {
                    self.forget(vid);
                    self.out.send(kick(vid, Some("paused"))).await;
                }
                if !pairing_open() {
                    self.out.close().await;
                    return true;
                }
                self.out.text(serde_json::json!({ "t": "pc_tokens", "allowedTokens": current_token_hashes(app) })).await;
            }
            ControlSignal::Broadcast => {
                if self.broadcast_at.is_none() {
                    self.broadcast_at = Some(Instant::now().max(self.last_broadcast + BROADCAST_EVERY));
                }
            }
        }
        false
    }
}

async fn run_connection(
    app: &AppHandle,
    relay_url: &str,
    identity: &PcIdentity,
    ctrl_rx: &mut mpsc::UnboundedReceiver<ControlSignal>,
) -> std::result::Result<(), ConnEnd> {
    let config = WebSocketConfig::default().max_message_size(Some(MAX_RELAY_MESSAGE)).max_frame_size(Some(MAX_RELAY_MESSAGE));
    let connect = tokio::time::timeout(CONNECT_TIMEOUT, tokio_tungstenite::connect_async_with_config(relay_url, Some(config), false));
    tokio::pin!(connect);
    let ws = loop {
        tokio::select! {
            done = &mut connect => {
                let (ws, _) = done.map_err(|_| "The relay did not answer in time".to_string())?.map_err(|e| format!("Could not reach relay: {e}"))?;
                break ws;
            }
            sig = ctrl_rx.recv() => {
                if matches!(sig, None | Some(ControlSignal::Reload | ControlSignal::Reconnect | ControlSignal::Pause)) {
                    return Ok(());
                }
            }
        }
    };
    let (ws_tx, mut ws_rx) = ws.split();
    let (out, mut writer) = Outbound::spawn(ws_tx);

    out.text(serde_json::json!({
        "t": "pc_hello",
        "proto": PROTO,
        "app": env!("CARGO_PKG_VERSION"),
        "pcId": identity.pc_id,
        "pcSecretHash": identity.pc_secret_hash_hex(),
        "allowedTokens": current_token_hashes(app),
    }))
    .await;

    // If a pairing session was already opened locally, register it on the relay now.
    let open_pair = {
        let s = app.state::<App>();
        let g = s.remote.lock();
        g.pairing
            .as_ref()
            .filter(|p| p.expires_at > now_ms() && matches!(p.stage, PairStage::Open))
            .map(|p| (p.pair_id.clone(), p.manual_code.clone()))
    };
    if let Some((pair_id, manual_code)) = open_pair {
        out.text(serde_json::json!({ "t": "pair_open", "pairId": pair_id, "manualCode": manual_code })).await;
    }

    let (rpc_out_tx, mut rpc_out_rx) = mpsc::unbounded_channel::<(Uuid, RemoteEvent)>();
    let mut link = Link {
        app,
        identity,
        out,
        sessions: HashMap::new(),
        budgets: HashMap::new(),
        rpc_out_tx,
        broadcast_at: None,
        last_broadcast: Instant::now() - BROADCAST_EVERY,
        pending: PendingClock::new(Instant::now()),
    };
    let (preview_tx, mut preview_rx) = mpsc::channel::<Vec<EncodedPreview>>(1);
    let mut preview_busy = false;
    let mut last_inbound = Instant::now();
    let mut ping_tick = tokio::time::interval(Duration::from_secs(25));
    let mut metrics_tick = tokio::time::interval(METRICS_EVERY);
    ping_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    metrics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        publish_watching(app, &link.sessions);
        let preview_due = if preview_busy { None } else { next_preview_at(&mut link.sessions) };
        tokio::select! {
            biased;
            Some(sig) = ctrl_rx.recv() => {
                if link.on_signal(sig, relay_url).await {
                    return Ok(());
                }
            }
            _ = &mut writer => return Err("The relay stopped taking data".into()),
            ws_item = ws_rx.next() => {
                let Some(frame) = ws_item else {
                    return Err("Relay closed connection".into());
                };
                let frame = frame.map_err(|e| e.to_string())?;
                last_inbound = Instant::now();
                match frame {
                    WsMsg::Text(text) => {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                            link.on_relay_text(&val).await?;
                        }
                    }
                    WsMsg::Binary(bytes) => {
                        if bytes.len() < BIN_HEADER_LEN + 1 || bytes[0] != BIN_VERSION {
                            continue;
                        }
                        let Ok(vid) = Uuid::from_slice(&bytes[1..17]) else { continue };
                        link.on_frame(vid, &bytes[BIN_HEADER_LEN..]).await;
                        link.flush_snapshots().await;
                    }
                    WsMsg::Ping(p) => link.out.send(WsMsg::Pong(p)).await,
                    WsMsg::Close(_) => return Err("Relay closed socket".into()),
                    _ => {}
                }
            }
            Some((vid, ev)) = rpc_out_rx.recv() => {
                if let Some(sess) = link.sessions.get_mut(&vid)
                    && let Some(cipher) = sess.cipher.as_mut()
                {
                    send_event_to(&link.out, vid, cipher, &ev).await;
                }
            }
            Some(frames) = preview_rx.recv() => {
                preview_busy = false;
                send_previews(&link.out, &mut link.sessions, frames);
            }
            () = tokio::time::sleep_until(link.broadcast_at.unwrap_or_else(Instant::now).into()), if link.broadcast_at.is_some() => {
                link.flush_broadcasts().await;
            }
            _ = ping_tick.tick() => {
                link.out.send(WsMsg::Text(r#"{"t":"ping"}"#.into())).await;
                // Sweep expired pairing session.
                let expired = {
                    let s = app.state::<App>();
                    let mut g = s.remote.lock();
                    if g.pairing.as_ref().is_some_and(|p| p.expires_at <= now_ms()) {
                        g.pairing.take()
                    } else {
                        None
                    }
                };
                if let Some(p) = expired {
                    if let Some(joiner) = p.joiner {
                        send_pair_mode(&app.state::<App>().remote, &p.pair_id, joiner, "expired");
                    }
                    link.out.text(serde_json::json!({"t": "pair_close", "pairId": p.pair_id})).await;
                    emit_changed(app);
                    // Pairing timed out with nothing paired: drop the socket and go idle.
                    if is_paused(app) || app.state::<App>().db.remote_devices().map(|d| d.is_empty()).unwrap_or(true) {
                        link.out.close().await;
                        return Ok(());
                    }
                }
            }
            _ = metrics_tick.tick() => {
                expire_pending_devices(app, &mut link.pending);
                let now = Instant::now();
                let quiet: Vec<Uuid> = link.sessions.iter().filter(|(_, s)| now.duration_since(s.last_rx) > VIEWER_SILENCE).map(|(vid, _)| *vid).collect();
                for vid in quiet {
                    log::info!("remote: viewer {vid} went quiet; dropping its session");
                    link.drop_viewer(vid).await;
                }
                link.flush_snapshots().await;
                if link.sessions.values().any(|s| s.confirmed) {
                    for ev in dispatch::metrics_events(app) {
                        link.broadcast(&ev).await;
                    }
                }
            }
            () = tokio::time::sleep_until((last_inbound + RELAY_SILENCE).into()) => {
                return Err("The relay stopped answering".into());
            }
            () = tokio::time::sleep_until(preview_due.unwrap_or_else(Instant::now).into()), if preview_due.is_some() => {
                if crate::session::locked() {
                    let paused = Instant::now();
                    link.sessions.values_mut().flat_map(|s| s.preview_flows.values_mut()).for_each(|f| f.tried(paused, 1));
                    continue;
                }
                let jobs = due_previews(app, &mut link.sessions);
                if !jobs.is_empty() {
                    preview_busy = true;
                    let tx = preview_tx.clone();
                    tauri::async_runtime::spawn(async move {
                        let frames = tokio::task::spawn_blocking(move || encode_previews(jobs)).await.unwrap_or_default();
                        let _ = tx.send(frames).await;
                    });
                }
            }
        }
    }
}

fn next_preview_at(sessions: &mut HashMap<Uuid, ViewerSession>) -> Option<Instant> {
    let now = Instant::now();
    let mut soonest: Option<Instant> = None;
    for sess in sessions.values_mut().filter(|s| s.confirmed) {
        for target in &sess.previews {
            let at = match sess.preview_flows.get_mut(&target.instance_id) {
                Some(flow) => flow.next_at(now),
                None => sess.preview_flows.entry(target.instance_id.clone()).or_default().next_at(now),
            };
            soonest = Some(soonest.map_or(at, |s| s.min(at)));
        }
    }
    soonest
}

type PreviewJob = (Uuid, String, Option<u32>, u32, u8, u32);
type EncodedPreview = (Uuid, String, u32, bool, Vec<u8>);

fn due_previews(app: &AppHandle, sessions: &mut HashMap<Uuid, ViewerSession>) -> Vec<PreviewJob> {
    let now = Instant::now();
    let total: u32 = sessions.values().filter(|s| s.confirmed).flat_map(|s| &s.previews).map(|t| u32::from(t.fps)).sum();
    let mut work = Vec::new();
    for (vid, sess) in sessions.iter_mut() {
        if !sess.confirmed {
            continue;
        }
        for target in &sess.previews {
            let flow = sess.preview_flows.entry(target.instance_id.clone()).or_default();
            if flow.next_at(now) > now {
                continue;
            }
            let fps = preview::shared_fps(target.fps, total);
            flow.tried(now, fps);
            // Same lookup as the desktop's own previews: works for clients whose start time can't be read.
            let pid = app.state::<App>().tracker.window_pid(&target.instance_id);
            work.push((*vid, target.instance_id.clone(), pid, target.max_w, fps, flow.last_seq));
        }
    }
    work
}

fn encode_previews(jobs: Vec<PreviewJob>) -> Vec<EncodedPreview> {
    let mut batch = preview::Batch::default();
    jobs.into_iter()
        .filter_map(|(vid, id, pid, max_w, fps, last_seq)| {
            let (seq, age_ms, status, jpeg) = batch.grab(pid, max_w, fps, last_seq);
            // No new picture: a live one is just unchanged, anything else still tells the viewer why
            // (minimized, no window, or capture not possible on this PC).
            let jpeg = match jpeg {
                Some(j) if j.len() > preview::MAX_JPEG => return None,
                Some(j) => j,
                None if status != crate::instances::window::Status::Live as u8 => Vec::new(),
                None => return None,
            };
            encode_preview_frame(&id, seq, age_ms, status, &jpeg).map(|frame| (vid, id, seq, !jpeg.is_empty(), frame))
        })
        .collect()
}

fn send_previews(out: &Outbound, sessions: &mut HashMap<Uuid, ViewerSession>, frames: Vec<EncodedPreview>) {
    let sent_at = Instant::now();
    for (vid, id, seq, picture, pt) in frames {
        if let Some(sess) = sessions.get_mut(&vid)
            && sess.previews.iter().any(|t| t.instance_id == id)
            && let Some(cipher) = sess.cipher.as_mut()
            && fits_frame(pt.len())
            && let Ok(ct) = cipher.seal(&pt)
            && out.offer(wrap_binary(vid, &ct))
        {
            sess.preview_flows.entry(id).or_default().sent(seq, picture, sent_at);
        }
    }
}

fn renew_taken_pairing(app: &AppHandle, taken_pair_id: &str) -> Option<(String, String)> {
    let s = app.state::<App>();
    let renewed = {
        let mut g = s.remote.lock();
        let p = g.pairing.as_mut().filter(|p| p.pair_id == taken_pair_id)?;
        if p.joiner.is_none() && p.renewals < MAX_PAIR_RENEWALS {
            let renewals = p.renewals + 1;
            *p = ActivePairing { renewals, ..ActivePairing::new(p.expires_at) };
            Some((p.pair_id.clone(), p.manual_code.clone()))
        } else {
            p.stage = PairStage::Aborted;
            None
        }
    };
    match &renewed {
        Some(_) => log::info!("remote: pairing code already taken on the relay; opened a fresh one"),
        None => log::warn!("remote: pairing code taken on the relay again; pairing stopped"),
    }
    emit_changed(app);
    renewed
}

fn expire_pending_devices(app: &AppHandle, clock: &mut PendingClock) {
    let expired = clock.spend(&app.state::<App>().db, Instant::now(), !is_paused(app));
    for label in &expired {
        log::info!("remote: device {label:?} never connected after pairing; removed");
        let _ = app.emit("remote://pairing", serde_json::json!({ "outcome": "expired", "label": label }));
    }
    if !expired.is_empty() {
        app.state::<App>().remote.send_ctrl(ControlSignal::SyncTokens);
        emit_changed(app);
    }
}

fn handle_pair_join(app: &AppHandle, pair_id: &str, viewer_id: Uuid, data_b64: &str) {
    let s = app.state::<App>();
    let msg = URL_SAFE_NO_PAD.decode(data_b64).ok().and_then(|raw| serde_json::from_slice::<PairJoinMsg>(&raw).ok());
    let late = {
        let mut g = s.remote.lock();
        let late = g.pairing.as_ref().is_some_and(|p| p.pair_id == pair_id && p.expires_at <= now_ms());
        late.then(|| g.pairing.take()).flatten()
    };
    if let Some(p) = late {
        if p.joiner != Some(viewer_id) {
            send_pair_mode(&s.remote, pair_id, viewer_id, "expired");
        }
        close_pairing(&s.remote, p, "expired");
        emit_changed(app);
        return;
    }
    let (first, aborted, challenge) = {
        let mut g = s.remote.lock();
        let Some(p) = g.pairing.as_mut().filter(|p| p.pair_id == pair_id && p.expires_at > now_ms()) else { return };
        if matches!(p.stage, PairStage::Aborted) {
            return;
        }
        let first = *p.joiner.get_or_insert(viewer_id);
        let done = match (&msg, &p.stage) {
            (Some(PairJoinMsg::Done { blob }), PairStage::Approved(a)) if first == viewer_id => done_proof(a, pair_id, blob),
            _ => None,
        };
        if let Some(replaces) = done {
            let taken = g.pairing.take();
            drop(g);
            if let Some(ActivePairing { stage: PairStage::Approved(approved), .. }) = taken {
                enroll(app, pair_id, viewer_id, *approved, replaces);
            }
            return;
        }
        let stage = std::mem::replace(&mut p.stage, PairStage::Aborted);
        if first == viewer_id
            && let Some(next) = msg.and_then(|m| next_pair_stage(stage, pair_id, &p.qr_secret, m))
        {
            p.stage = next;
        }
        let challenge = match &p.stage {
            PairStage::Committed { pc_ephem, .. } => Some(pubkey_bytes(pc_ephem)),
            _ => None,
        };
        (first, matches!(p.stage, PairStage::Aborted), challenge)
    };

    if aborted {
        log::warn!("remote: pairing stopped: a second device or an invalid request used the pairing code");
        send_pair_mode(&s.remote, pair_id, first, "aborted");
        if viewer_id != first {
            send_pair_mode(&s.remote, pair_id, viewer_id, "aborted");
        }
        s.remote.send_ctrl(ControlSignal::ClosePair { pair_id: pair_id.to_owned() });
    } else if let Some(pc_ephem_pub) = challenge {
        let msg = serde_json::json!({ "mode": "manual_challenge", "pairId": pair_id, "pcEphemPubB64": URL_SAFE_NO_PAD.encode(pc_ephem_pub) });
        s.remote.send_ctrl(ControlSignal::SendPairMsg {
            pair_id: pair_id.to_owned(),
            viewer_id,
            data: URL_SAFE_NO_PAD.encode(msg.to_string()),
        });
    }
    emit_changed(app);
}

/// The joiner's relay socket closed before pairing finished: nothing gets enrolled, and the user
/// hears why the request went away.
fn handle_pair_left(app: &AppHandle, pair_id: &str, viewer_id: Uuid) {
    let s = app.state::<App>();
    let ended = {
        let mut g = s.remote.lock();
        let ours = g.pairing.as_ref().is_some_and(|p| p.pair_id == pair_id && p.joiner == Some(viewer_id) && !matches!(p.stage, PairStage::Aborted));
        if ours {
            g.pairing = None;
        }
        ours
    };
    if ended {
        log::info!("remote: the pairing device left before pairing finished");
        s.remote.send_ctrl(ControlSignal::ClosePair { pair_id: pair_id.to_owned() });
        let _ = app.emit("remote://pairing", serde_json::json!({ "outcome": "left" }));
        emit_changed(app);
    }
}

fn next_pair_stage(stage: PairStage, pair_id: &str, qr_secret: &[u8; 32], msg: PairJoinMsg) -> Option<PairStage> {
    match (stage, msg) {
        (PairStage::Open, PairJoinMsg::Qr { nonce_b64, blob }) => {
            let transcript = pair_transcript(PAIR_MODE_QR, pair_id, &[], &[], &decode_array(&nonce_b64)?);
            let key = derive_qr_pair_key(qr_secret, pair_id).ok()?;
            open_device_binding(key, None, pair_id, &blob, transcript)
        }
        (PairStage::Open, PairJoinMsg::ManualCommit { commit_b64 }) => {
            Some(PairStage::Committed { commit: decode_array(&commit_b64)?, pc_ephem: new_ephemeral_key() })
        }
        (PairStage::Committed { commit, pc_ephem }, PairJoinMsg::ManualReveal { ephem_pub_b64, nonce_b64, blob }) => {
            let br_ephem = decode_array::<65>(&ephem_pub_b64)?;
            let nonce = decode_array::<32>(&nonce_b64)?;
            if pair_commit(&br_ephem, &nonce) != commit {
                return None;
            }
            let transcript = pair_transcript(PAIR_MODE_MANUAL, pair_id, &pubkey_bytes(&pc_ephem), &br_ephem, &nonce);
            let (key, sas) = derive_manual_pair_keys(&pc_ephem, &br_ephem, &transcript).ok()?;
            open_device_binding(key, Some(sas), pair_id, &blob, transcript)
        }
        _ => None,
    }
}

fn decode_array<const N: usize>(b64: &str) -> Option<[u8; N]> {
    URL_SAFE_NO_PAD.decode(b64).ok()?.try_into().ok()
}

fn open_device_binding(pair_key: Zeroizing<[u8; 32]>, sas: Option<String>, pair_id: &str, blob: &str, transcript: [u8; 32]) -> Option<PairStage> {
    let plain = open_pair_blob(&pair_key, DIR_VIEWER_TO_PC, pair_id, blob).ok()?;
    let binding: DeviceBinding = serde_json::from_slice(&plain).ok()?;
    let device_pubkey = URL_SAFE_NO_PAD.decode(&binding.device_pub_b64).ok()?;
    parse_pubkey(&device_pubkey).ok()?;
    let sig = URL_SAFE_NO_PAD.decode(&binding.sig_b64).ok()?;
    verify_device_binding(&device_pubkey, &transcript, binding.device_id, &sig).ok()?;
    Some(PairStage::Pending(PendingPair {
        device_id: binding.device_id,
        label: sanitize_label(&binding.label),
        device_pubkey,
        pair_key,
        sas,
        transcript,
    }))
}

#[cfg(test)]
mod pairing_tests {
    use p256::ecdsa::SigningKey;

    use super::crypto::sign_device_binding;
    use super::*;

    const PAIR_ID: &str = "0123456789abcdef";

    fn binding_blob(key: &[u8; 32], dir: u8, transcript: &[u8; 32], device: &SigningKey, device_id: Uuid) -> String {
        let device_pub = device.verifying_key().to_encoded_point(false).as_bytes().to_vec();
        let sig = sign_device_binding(device, transcript, device_id, &device_pub);
        let json = serde_json::json!({
            "deviceId": device_id,
            "devicePubB64": URL_SAFE_NO_PAD.encode(&device_pub),
            "label": "Phone\u{202E}\u{200B}lanigiro",
            "sigB64": URL_SAFE_NO_PAD.encode(sig),
        });
        seal_pair_blob(key, dir, PAIR_ID, json.to_string().as_bytes()).unwrap()
    }

    fn parse(json: serde_json::Value) -> PairJoinMsg {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn a_late_approval_leaves_the_device_time_to_confirm() {
        let opened = 1_000_000;
        let expires = opened + PAIR_DURATION_MS;
        let approved = opened + 100_000;
        let deadline = confirm_deadline(expires, approved);
        assert_eq!(deadline, approved + PAIR_CONFIRM_MS);
        assert!(deadline > expires + 30_000, "a done 30 s after the first window still enrolls");
        assert_eq!(confirm_deadline(expires, opened + 10_000), expires, "an early approval keeps its window");
    }

    #[test]
    fn a_re_pairing_drops_every_old_device_whose_key_it_proves() {
        let transcript = [9u8; 32];
        let devices = [0x31u8, 0x32, 0x33].map(|b| (Uuid::from_bytes([b; 16]), SigningKey::from_slice(&[b; 32]).unwrap()));
        let pubkey = |key: &SigningKey| key.verifying_key().to_encoded_point(false).as_bytes().to_vec();
        let proof = |id: Uuid, key: &SigningKey| serde_json::json!({ "deviceId": id, "sigB64": URL_SAFE_NO_PAD.encode(sign_device_binding(key, &transcript, id, &pubkey(key))) });
        let [(a, ka), (b, kb), (c, kc)] = &devices;
        let wrong_key = serde_json::json!({ "deviceId": c, "sigB64": proof(*a, ka)["sigB64"] });
        let done = serde_json::json!({ "mode": "done", "replaces": [proof(*a, ka), proof(*b, kb), proof(*a, ka), wrong_key, proof(*c, kc)] });
        let SealedDone::Done { replaces } = serde_json::from_value(done).unwrap();
        let stored = |id: &str| devices.iter().find(|(d, _)| d.to_string() == id).map(|(_, k)| pubkey(k));
        assert_eq!(proven_replacements(&replaces, &c.to_string(), &transcript, stored), [a.to_string(), b.to_string()]);
    }

    #[test]
    fn manual_pairing_needs_matching_commit_and_signed_identity() {
        let device = SigningKey::from_slice(&[0x66u8; 32]).unwrap();
        let device_id = Uuid::new_v4();
        let br_ephem = new_ephemeral_key();
        let br_pub = pubkey_bytes(&br_ephem);
        let nonce = random_bytes::<32>();
        let commit = parse(serde_json::json!({ "mode": "manual_commit", "commitB64": URL_SAFE_NO_PAD.encode(pair_commit(&br_pub, &nonce)) }));

        let Some(PairStage::Committed { commit, pc_ephem }) = next_pair_stage(PairStage::Open, PAIR_ID, &[0; 32], commit) else {
            panic!("a commitment must get the PC's ephemeral key")
        };
        let pc_pub = pubkey_bytes(&pc_ephem);
        let transcript = pair_transcript(PAIR_MODE_MANUAL, PAIR_ID, &pc_pub, &br_pub, &nonce);
        let (key, sas) = derive_manual_pair_keys(&br_ephem, &pc_pub, &transcript).unwrap();
        let reveal = |nonce: [u8; 32], dir: u8| {
            parse(serde_json::json!({
                "mode": "manual_reveal",
                "ephemPubB64": URL_SAFE_NO_PAD.encode(br_pub),
                "nonceB64": URL_SAFE_NO_PAD.encode(nonce),
                "blob": binding_blob(&key, dir, &transcript, &device, device_id),
            }))
        };
        let committed = || PairStage::Committed { commit, pc_ephem: pc_ephem.clone() };

        assert!(next_pair_stage(committed(), PAIR_ID, &[0; 32], reveal(random_bytes::<32>(), DIR_VIEWER_TO_PC)).is_none());
        assert!(next_pair_stage(committed(), PAIR_ID, &[0; 32], reveal(nonce, DIR_PC_TO_VIEWER)).is_none());
        let Some(PairStage::Pending(p)) = next_pair_stage(committed(), PAIR_ID, &[0; 32], reveal(nonce, DIR_VIEWER_TO_PC)) else {
            panic!("a valid reveal must wait for approval")
        };
        assert_eq!(p.sas.as_deref(), Some(sas.as_str()));
        assert_eq!(p.device_id, device_id);
        assert_eq!(p.label, "Phonelanigiro");
        assert_eq!(p.device_pubkey, device.verifying_key().to_encoded_point(false).as_bytes());
    }

    #[test]
    fn only_a_sealed_done_for_this_pairing_enrolls() {
        let key = random_bytes::<32>();
        let row = RemoteDeviceRow {
            id: Uuid::new_v4().to_string(),
            label: "Phone".into(),
            pubkey: Vec::new(),
            room_token: random_token(),
            role: RemoteRole::Readonly,
            created_at: 0,
            last_seen_at: None,
        };
        let approved = ApprovedPair { row, pair_key: Zeroizing::new(key), transcript: [0; 32], sas: None };
        let seal = |dir: u8, json: serde_json::Value| seal_pair_blob(&key, dir, PAIR_ID, json.to_string().as_bytes()).unwrap();

        assert!(done_proof(&approved, PAIR_ID, &seal(DIR_VIEWER_TO_PC, serde_json::json!({ "mode": "done" }))).is_some_and(|r| r.is_empty()));
        let device = SigningKey::from_slice(&[0x55u8; 32]).unwrap();
        let binding = binding_blob(&key, DIR_VIEWER_TO_PC, &[0; 32], &device, Uuid::new_v4());
        assert!(done_proof(&approved, PAIR_ID, &binding).is_none(), "the device binding must not stand in for done");
        assert!(done_proof(&approved, PAIR_ID, &seal(DIR_VIEWER_TO_PC, serde_json::json!({}))).is_none());
        assert!(done_proof(&approved, PAIR_ID, &seal(DIR_PC_TO_VIEWER, serde_json::json!({ "mode": "done" }))).is_none());
        assert!(done_proof(&approved, PAIR_ID, "").is_none());
    }

    #[test]
    fn a_lost_relay_connection_stops_only_a_joined_pairing() {
        let mut p = ActivePairing::new(i64::MAX);
        assert!(!p.strand(), "nobody joined: it opens again on the next connection");
        assert!(matches!(p.stage, PairStage::Open));
        p.joiner = Some(Uuid::new_v4());
        assert!(p.strand());
        assert!(matches!(p.stage, PairStage::Aborted));
        assert!(!p.strand());
    }

    #[test]
    fn the_qr_link_carries_only_the_pairing_in_its_fragment() {
        let secret = [7u8; 32];
        assert_eq!(qr_url(PAIR_ID, &secret), format!("{WEB_BASE_URL}#pair={PAIR_ID}.{}", URL_SAFE_NO_PAD.encode(secret)));
    }

    const INTEROP_ENROLLED_BLOB: &str = "7A_-ncL568EkElXtV89LuIgN9NvikzBf5BaJNHNBemUXuhT_ka4CTkJx0VYJocs";

    #[test]
    fn enrolled_matches_the_dashboard_interop_vector() {
        let pc_ephem = SecretKey::from_slice(&[0x22u8; 32]).unwrap();
        let viewer_pub = pubkey_bytes(&SecretKey::from_slice(&[0x33u8; 32]).unwrap());
        let th = pair_transcript(PAIR_MODE_MANUAL, PAIR_ID, &pubkey_bytes(&pc_ephem), &viewer_pub, &[0x55u8; 32]);
        let (key, _) = derive_manual_pair_keys(&pc_ephem, &viewer_pub, &th).unwrap();
        let fresh = enrolled_msg(&key, PAIR_ID).unwrap();
        let open = |blob: &str| open_pair_blob(&key, DIR_PC_TO_VIEWER, PAIR_ID, blob).unwrap();
        assert_eq!(open(fresh["blob"].as_str().unwrap()), open(INTEROP_ENROLLED_BLOB));
    }

    #[test]
    fn enrolled_is_sealed_for_the_browser_and_cancel_is_not_a_message() {
        let key = random_bytes::<32>();
        let msg = enrolled_msg(&key, PAIR_ID).unwrap();
        assert_eq!(msg["mode"], "enrolled");
        let blob = msg["blob"].as_str().unwrap();
        assert_eq!(open_pair_blob(&key, DIR_PC_TO_VIEWER, PAIR_ID, blob).unwrap(), br#"{"mode":"enrolled"}"#);
        assert!(open_pair_blob(&key, DIR_VIEWER_TO_PC, PAIR_ID, blob).is_err());
        assert!(serde_json::from_value::<PairJoinMsg>(serde_json::json!({ "mode": "cancel" })).is_err());
    }

    #[test]
    fn qr_pairing_waits_for_approval_and_rejects_out_of_order_messages() {
        let device = SigningKey::from_slice(&[0x77u8; 32]).unwrap();
        let device_id = Uuid::new_v4();
        let secret = random_bytes::<32>();
        let nonce = random_bytes::<32>();
        let key = derive_qr_pair_key(&secret, PAIR_ID).unwrap();
        let transcript = pair_transcript(PAIR_MODE_QR, PAIR_ID, &[], &[], &nonce);
        let join = || {
            parse(serde_json::json!({
                "mode": "qr",
                "nonceB64": URL_SAFE_NO_PAD.encode(nonce),
                "blob": binding_blob(&key, DIR_VIEWER_TO_PC, &transcript, &device, device_id),
            }))
        };

        let Some(PairStage::Pending(p)) = next_pair_stage(PairStage::Open, PAIR_ID, &secret, join()) else {
            panic!("a QR join must wait for approval")
        };
        assert!(p.sas.is_none());
        assert!(next_pair_stage(PairStage::Open, PAIR_ID, &random_bytes::<32>(), join()).is_none());
        assert!(next_pair_stage(PairStage::Pending(p), PAIR_ID, &secret, join()).is_none());
        let reveal = parse(serde_json::json!({ "mode": "manual_reveal", "ephemPubB64": "", "nonceB64": "", "blob": "" }));
        assert!(next_pair_stage(PairStage::Open, PAIR_ID, &secret, reveal).is_none());
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    #[test]
    fn labels_lose_invisible_stacked_and_blank_characters() {
        assert_eq!(sanitize_label("Ph\u{202E}one\u{E0041}\u{2028}  X\u{FE0F}"), "Phone X");
        assert_eq!(sanitize_label("a\u{0301}\u{0301}\u{0301}b"), "a\u{0301}b");
        assert_eq!(sanitize_label("\u{3164}\u{115F}\u{2800}\u{0301}"), "Remote browser");
        assert_eq!(sanitize_label("\u{2066}A\u{2069}\u{061C}\u{0890}\u{0891}\u{2029}\u{E0001}\u{0085}B"), "AB");
        assert_eq!(sanitize_label(&"x".repeat(100)).chars().count(), 64);
        for mark in ['\u{0E31}', '\u{0F71}', '\u{1D165}', '\u{0903}', '\u{20DD}'] {
            assert_eq!(sanitize_label(&format!("a{}b", mark.to_string().repeat(40))), format!("a{mark}b"));
        }
    }

    #[test]
    fn the_relay_message_cap_covers_what_the_relay_may_send() {
        let relay = include_str!("../../../server/remote-relay/src/lib.rs");
        let size = |name: &str| -> usize {
            let line = relay.lines().find(|l| l.starts_with(&format!("pub const {name}: usize = "))).expect(name);
            line.trim_end_matches(';').rsplit(" = ").next().unwrap().split('*').map(|n| n.trim().parse::<usize>().unwrap()).product()
        };
        assert!(size("MAX_VIEWER_FRAME") <= MAX_RELAY_MESSAGE);
        assert!(size("MAX_TEXT_FRAME") + 1024 <= MAX_RELAY_MESSAGE);
    }

    #[test]
    fn a_burst_of_list_changes_queues_one_broadcast() {
        let remote = Remote::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        remote.lock().ctrl_tx = Some(tx);
        remote.notify_accounts_changed();
        assert!(rx.try_recv().is_err(), "nobody is watching");
        remote.lock().connected_viewers.insert(Uuid::new_v4(), Uuid::new_v4());
        for _ in 0..100 {
            remote.notify_accounts_changed();
            remote.notify_instances_changed();
        }
        assert!(matches!(rx.try_recv(), Ok(ControlSignal::Broadcast)));
        assert!(rx.try_recv().is_err());
        let due = std::mem::take(&mut remote.lock().broadcasts);
        assert!(due.accounts && due.instances && !due.lock_state);
        remote.notify_lock_state_changed();
        assert!(matches!(rx.try_recv(), Ok(ControlSignal::Broadcast)));
    }

    #[test]
    fn a_pending_device_spends_only_time_the_pc_was_awake_connected_and_unpaused() {
        let db = crate::storage::Db::open_in_memory().unwrap();
        let row = crate::storage::RemoteDeviceRow {
            id: Uuid::new_v4().to_string(),
            label: "Phone".into(),
            pubkey: vec![4; 65],
            room_token: "token".into(),
            role: RemoteRole::Readonly,
            created_at: 1,
            last_seen_at: None,
        };
        db.remote_device_insert(&row, PENDING_DEVICE_MS, &[]).unwrap();
        let t0 = Instant::now();
        let mut first = PendingClock::new(t0);
        assert!(first.spend(&db, t0 + Duration::from_secs(3600), true).is_empty(), "an hour asleep counts as one late tick");
        assert!(first.spend(&db, t0 + Duration::from_secs(3603), false).is_empty(), "paused");
        let mut second = PendingClock::new(t0);
        let ticks = u32::try_from(PENDING_DEVICE_MS / 3_000).unwrap();
        for n in 1..ticks - 2 {
            assert!(second.spend(&db, t0 + METRICS_EVERY * n, true).is_empty(), "tick {n}");
        }
        assert_eq!(second.spend(&db, t0 + METRICS_EVERY * (ticks - 2), true), ["Phone"], "a new connection brings no time back");
    }

    #[test]
    fn a_pairing_never_opens_while_remote_is_paused() {
        let remote = Remote::new();
        remote.lock().paused = true;
        assert!(remote.open_pairing(ActivePairing::new(i64::MAX)).is_err());
        assert!(remote.lock().pairing.is_none());
        remote.lock().paused = false;
        assert!(matches!(remote.open_pairing(ActivePairing::new(i64::MAX)), Ok(None)));
    }

    #[test]
    fn a_session_turned_away_for_pause_or_crowding_says_so() {
        assert_eq!(join_refusal(true, true), Some("paused"));
        assert_eq!(join_refusal(false, true), Some("device_sessions"));
        assert_eq!(join_refusal(false, false), None);
        let kicked: serde_json::Value = match kick(Uuid::nil(), Some("paused")) {
            WsMsg::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => unreachable!(),
        };
        assert_eq!(kicked["code"], "paused");
    }

    #[test]
    fn pausing_closes_an_open_pairing() {
        let remote = Remote::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        remote.lock().ctrl_tx = Some(tx);
        let joiner = Uuid::new_v4();
        let mut p = ActivePairing::new(i64::MAX);
        p.joiner = Some(joiner);
        let pair = p.pair_id.clone();
        remote.lock().pairing = Some(p);
        remote.pause(true);
        assert!(remote.lock().pairing.is_none() && remote.lock().paused);
        assert!(matches!(rx.try_recv(), Ok(ControlSignal::SendPairMsg { viewer_id, .. }) if viewer_id == joiner));
        assert!(matches!(rx.try_recv(), Ok(ControlSignal::ClosePair { pair_id }) if pair_id == pair));
        assert!(matches!(rx.try_recv(), Ok(ControlSignal::Pause)));
        remote.lock().pairing = Some(ActivePairing::new(i64::MAX));
        remote.pause(false);
        assert!(remote.lock().pairing.is_some(), "resuming leaves a pairing alone");
    }

    #[test]
    fn each_session_of_a_device_gets_its_own_first_refused_lift() {
        let t0 = Instant::now();
        let mut releases = Bucket::new(t0, 1.0, 0.0);
        let (mut a, mut b) = (false, false);
        assert_eq!(refuse_lift(&mut releases, &mut a, t0), None);
        assert_eq!(refuse_lift(&mut releases, &mut a, t0), Some(true));
        assert_eq!(refuse_lift(&mut releases, &mut a, t0), Some(false));
        assert_eq!(refuse_lift(&mut releases, &mut b, t0), Some(true), "another session's input is let go of too");
        assert_eq!(refuse_lift(&mut releases, &mut b, t0), Some(false));
    }

    #[test]
    fn an_aborted_or_expired_pairing_is_not_open() {
        let mut p = ActivePairing::new(i64::MAX);
        assert!(pairing_open(&p));
        p.stage = PairStage::Aborted;
        assert!(!pairing_open(&p));
        assert!(!pairing_open(&ActivePairing::new(now_ms() - 1)));
    }

    #[test]
    fn the_old_default_relay_means_the_default() {
        let default = RemoteSettings::default().relay_url;
        assert_eq!(validate_relay_url(LEGACY_RELAY_URL).unwrap(), default);
        assert_eq!(validate_relay_url(" ").unwrap(), default);
        assert!(is_official_relay(&default) && !is_official_relay("wss://relay.example/ws"));
        assert!(validate_relay_url("ws://relay.example/ws").is_err());
    }

    #[test]
    fn a_release_build_always_uses_the_official_relay() {
        let default = RemoteSettings::default().relay_url;
        assert_eq!(relay_url_for("wss://relay.example/ws", false).unwrap(), default);
        assert_eq!(relay_url_for("ws://relay.example/ws", false).unwrap(), default);
        assert_eq!(relay_url_for("wss://relay.example/ws", true).unwrap(), "wss://relay.example/ws");
    }

    #[test]
    fn oversized_answers_become_short_errors() {
        let huge = RemoteEvent::RpcResult { req_id: 9, ok: true, data: None, error: None, code: None };
        assert!(matches!(oversized(&huge, 1 << 20), Some(RemoteEvent::RpcResult { req_id: 9, ok: false, .. })));
        assert!(matches!(oversized(&RemoteEvent::InstancesUpdated { batch: 0, instances: vec![] }, 1 << 20), Some(RemoteEvent::Notice { error: true, .. })));
    }

    #[test]
    fn previews_never_take_more_than_their_backlog() {
        let (tx, _rx) = mpsc::channel(OUTBOUND_QUEUE);
        let out = Outbound(tx);
        let offered = (0..OUTBOUND_QUEUE).filter(|_| out.offer(WsMsg::Text("p".into()))).count();
        assert_eq!(offered, PREVIEW_BACKLOG);
        assert!(out.0.try_send(WsMsg::Text("ack".into())).is_ok(), "control frames still get in");
    }

    #[test]
    fn rpc_bucket_allows_a_burst_then_its_rate() {
        let t0 = Instant::now();
        let mut b = Bucket::new(t0, RPC_BURST, RPC_PER_SEC);
        assert_eq!((0..40).filter(|_| b.take(t0)).count(), RPC_BURST as usize);
        assert!(!b.take(t0));
        assert!(b.take(t0 + Duration::from_millis(100)));
        assert!(!b.take(t0 + Duration::from_millis(100)));
        assert_eq!((0..40).filter(|_| b.take(t0 + Duration::from_secs(60))).count(), RPC_BURST as usize);
    }

    #[test]
    fn lifts_have_a_budget_of_their_own() {
        let t0 = Instant::now();
        let mut budget = DeviceBudget::new(t0);
        while budget.inputs.take(t0) {}
        assert_eq!((0..1000).filter(|_| budget.releases.take(t0)).count(), RELEASE_BURST as usize, "moves never use up lifts, and lifts are capped");
        assert!(budget.releases.take(t0 + Duration::from_millis(10)));
    }

    #[test]
    fn frames_over_the_relay_cap_are_refused() {
        assert!(fits_frame(MAX_WIRE_FRAME - BIN_HEADER_LEN - SEAL_OVERHEAD));
        assert!(!fits_frame(MAX_WIRE_FRAME - BIN_HEADER_LEN - SEAL_OVERHEAD + 1));
        assert!(fits_frame(preview::MAX_JPEG + 300));
    }

    #[test]
    fn reconnect_waits_are_spread_up_to_their_cap() {
        let cap = Duration::from_secs(8);
        let waits: Vec<Duration> = (0..200).map(|_| jittered(cap)).collect();
        assert!(waits.iter().all(|w| *w >= Duration::from_millis(500) && *w < cap + Duration::from_millis(500)));
        assert!(waits.iter().any(|w| *w < cap / 2) && waits.iter().any(|w| *w > cap / 2));
    }
}
