//! Versioned wire protocol between RoALT Desktop and `https://remote.roalt.dev/`.
//!
//! Plaintext inside every AES-256-GCM session frame starts with a 1-byte kind:
//! - `0x01`: UTF-8 JSON (`RemoteCommand` viewer->PC, `RemoteEvent` PC->viewer).
//! - `0x02`: Binary preview frame (PC->viewer):
//!   `[0x02][id_len: u8][instance_id utf8][seq: u32 LE][age_ms: u32 LE][status: u8][jpeg_bytes...]`.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{
    AccountStatus, AccountSummary, BulkItem, BulkProgress, BulkRequest, BulkStage, ClientMetrics, Crash, CrashKind, Favourite, Group, Instance,
    InstanceState, LaunchTarget, Limits, RemoteRole, SystemUsage,
};

const MAX_REMOTE_TEXT: usize = 256;

fn clip(text: &str) -> String {
    text.chars().take(MAX_REMOTE_TEXT).collect()
}

pub const APP_KIND_JSON: u8 = 0x01;
pub const APP_KIND_PREVIEW: u8 = 0x02;

/// One preview subscription requested by the browser viewport.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemotePreviewTarget {
    pub instance_id: String,
    /// Clamped on PC to `160..=640`.
    pub max_w: u32,
    /// Clamped on PC to `1..=30` FPS, at most 90 across one viewer's targets.
    pub fps: u8,
}

/// Strict allowlist of remote commands a viewer may send to a paired PC.
///
/// Security invariant: cookies, vault passwords, recovery codes, cookie exports, and account
/// creation/deletion are intentionally absent and cannot be invoked remotely.
#[derive(Deserialize, TS, Clone, Debug)]
#[serde(tag = "op", rename_all = "camelCase")]
#[ts(export)]
pub enum RemoteCommand {
    Ping,
    RequestSnapshot,
    SubscribePreviews {
        targets: Vec<RemotePreviewTarget>,
    },
    /// The viewer has drawn this instance's preview frames up to `seq`; never answered.
    PreviewAck {
        #[serde(rename = "instanceId")]
        instance_id: String,
        seq: u32,
    },
    TargetParse {
        input: String,
    },
    Launch {
        #[serde(rename = "accountId")]
        #[ts(type = "number")]
        account_id: i64,
        target: LaunchTarget,
        #[serde(rename = "fpsCap", default)]
        fps_cap: Option<u32>,
    },
    BulkLaunch {
        request: BulkRequest,
    },
    /// Launches a saved favourite where it points: with `accountIds`, or its own accounts when empty.
    LaunchFavourite {
        #[serde(rename = "favouriteId")]
        #[ts(type = "number")]
        favourite_id: i64,
        #[serde(rename = "accountIds", default)]
        #[ts(type = "Array<number>")]
        account_ids: Vec<i64>,
        #[serde(rename = "fpsCap", default)]
        fps_cap: Option<u32>,
    },
    BulkCancel {
        #[serde(rename = "batchId")]
        batch_id: String,
    },
    InstanceClose {
        id: String,
        force: bool,
    },
    InstancesCloseAll,
    InstanceMove {
        id: String,
        target: LaunchTarget,
        #[serde(rename = "fpsCap", default)]
        fps_cap: Option<u32>,
    },
    /// Rejoins a running client where it is, or relaunches an ended one where it was.
    InstanceRejoin {
        id: String,
    },
    InstanceSetAntiAfk {
        id: String,
        enabled: bool,
    },
    InstanceSetLimits {
        id: String,
        limits: Limits,
    },
    InstanceTrim {
        id: String,
    },
    InstanceShow {
        id: String,
        /// `"focus" | "minimize" | "restore"`
        what: String,
    },
    InstancesShowAll {
        /// `"minimize" | "restore"`
        what: String,
    },
    InstanceMicro {
        id: String,
        on: bool,
    },
    InstancesTile {
        micro: bool,
    },
    InstanceStopRelaunch {
        id: String,
    },
    InstancesRetryRelaunch,
    PanicKill {
        confirmed: bool,
    },
    ShutdownSchedule {
        #[serde(rename = "inSecs")]
        in_secs: u32,
        #[serde(rename = "closeClients")]
        close_clients: bool,
    },
    ShutdownCancel,
    /// One finger on a client's live view, sent to the client as real touch: `touch` is the
    /// phone's id for the finger, `x`, `y` 0..=1 across the picture, `what` is `"down" | "move" | "up"`.
    InputTouch {
        id: String,
        touch: u32,
        x: f32,
        y: f32,
        what: String,
    },
    /// A finger dragging on a client's live view to turn the camera (right button held on the PC):
    /// `x`, `y` 0..=1 across the picture, `what` is `"down" | "move" | "up"`.
    InputCamera {
        id: String,
        x: f32,
        y: f32,
        what: String,
    },
    /// A movement key held by the phone's joystick or jump button: `code` is one of
    /// `"KeyW" | "KeyA" | "KeyS" | "KeyD" | "Space"` (nothing else is accepted).
    InputKey {
        id: String,
        code: String,
        down: bool,
        /// A refresh of a key the phone still holds: never presses it again.
        #[serde(default)]
        repeat: bool,
    },
    /// Phase 2 reserved WebRTC signaling envelope (SDP offer/answer + ICE candidates).
    WebRtcSignal {
        #[serde(rename = "instanceId")]
        #[allow(dead_code)]
        instance_id: String,
        #[ts(type = "unknown")]
        #[allow(dead_code)]
        payload: serde_json::Value,
    },
}

impl RemoteCommand {
    /// True for read-only commands allowed when the paired device has `RemoteRole::Readonly`.
    pub fn is_readonly(&self) -> bool {
        matches!(
            self,
            Self::Ping | Self::RequestSnapshot | Self::SubscribePreviews { .. } | Self::PreviewAck { .. } | Self::TargetParse { .. }
        )
    }
}

/// Viewer->PC RPC envelope.
#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteRpcRequest {
    pub req_id: u32,
    #[serde(flatten)]
    pub cmd: RemoteCommand,
}

/// What a viewer sees of an account: never its notes, launch arguments, flags or profile.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteAccount {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub alias: String,
    pub color: Option<String>,
    pub main: bool,
    pub status: AccountStatus,
    pub group_ids: Vec<i64>,
}

impl From<&AccountSummary> for RemoteAccount {
    fn from(a: &AccountSummary) -> Self {
        Self {
            id: a.id,
            username: a.username.clone(),
            display_name: a.display_name.clone(),
            alias: a.alias.clone(),
            color: a.color.clone(),
            main: a.main,
            status: a.status,
            group_ids: a.group_ids.clone(),
        }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteGroup {
    pub id: i64,
    pub name: String,
}

impl From<&Group> for RemoteGroup {
    fn from(g: &Group) -> Self {
        Self { id: g.id, name: clip(&g.name) }
    }
}

/// A launch target without its codes, job id or launch data: viewers relaunch by instance or
/// favourite id and the PC resolves the real target.
#[derive(Serialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteTarget {
    /// `LaunchTarget`'s `kind`.
    pub kind: String,
    pub place_id: Option<i64>,
}

impl From<&LaunchTarget> for RemoteTarget {
    fn from(t: &LaunchTarget) -> Self {
        let (kind, place_id) = match t {
            LaunchTarget::Place { place_id, .. } => ("place", Some(*place_id)),
            LaunchTarget::Server { place_id, .. } => ("server", Some(*place_id)),
            LaunchTarget::PrivateLink { place_id, .. } => ("privateLink", Some(*place_id)),
            LaunchTarget::PrivateAccess { place_id, .. } => ("privateAccess", Some(*place_id)),
            LaunchTarget::ShareLink { .. } => ("shareLink", None),
            LaunchTarget::ShareLinkV2 { .. } => ("shareLinkV2", None),
            LaunchTarget::Universe { .. } => ("universe", None),
            LaunchTarget::FollowUser { .. } => ("followUser", None),
        };
        Self { kind: kind.into(), place_id }
    }
}

/// A saved favourite by name; `LaunchFavourite` launches it by `id`.
#[derive(Serialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteFavourite {
    pub id: i64,
    pub name: String,
    pub place_id: Option<i64>,
}

impl From<&Favourite> for RemoteFavourite {
    fn from(f: &Favourite) -> Self {
        Self { id: f.id, name: clip(&f.name), place_id: RemoteTarget::from(&f.target).place_id }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteInstance {
    pub id: String,
    pub account_id: i64,
    pub state: InstanceState,
    pub target: RemoteTarget,
    pub place_id: Option<i64>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub note: Option<String>,
    pub limits: Limits,
    pub anti_afk: bool,
    pub fps_cap: Option<u32>,
    pub relaunching: bool,
}

impl From<&Instance> for RemoteInstance {
    fn from(i: &Instance) -> Self {
        Self {
            id: i.id.clone(),
            account_id: i.account_id,
            state: i.state,
            target: RemoteTarget::from(&i.target),
            place_id: i.place_id,
            started_at: i.started_at,
            ended_at: i.ended_at,
            note: i.note.as_deref().map(clip),
            limits: i.limits,
            anti_afk: i.anti_afk,
            fps_cap: i.fps_cap,
            relaunching: i.relaunching,
        }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteCrash {
    pub id: i64,
    pub account_id: i64,
    pub kind: CrashKind,
    pub at: i64,
    pub reason: Option<String>,
}

impl From<&Crash> for RemoteCrash {
    fn from(c: &Crash) -> Self {
        Self { id: c.id, account_id: c.account_id, kind: c.kind, at: c.at, reason: c.reason.as_deref().map(clip) }
    }
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteBulk {
    pub batch_id: String,
    pub items: Vec<BulkItem>,
    pub done: bool,
}

impl From<&BulkProgress> for RemoteBulk {
    fn from(p: &BulkProgress) -> Self {
        let item = |i: &BulkItem| BulkItem {
            stage: match &i.stage {
                BulkStage::Failed { message } => BulkStage::Failed { message: clip(message) },
                stage => stage.clone(),
            },
            ..i.clone()
        };
        Self { batch_id: p.batch_id.clone(), items: p.items.iter().map(item).collect(), done: p.done }
    }
}

/// Full PC state sent upon viewer connect or explicit `RequestSnapshot`. Every list holds its first
/// page: `AccountsPage` and `Page` events right after it carry the rest.
#[derive(Serialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoteSnapshot {
    pub pc_name: String,
    pub app_version: String,
    pub role: RemoteRole,
    pub vault_locked: bool,
    pub windows_locked: bool,
    pub accounts: Vec<RemoteAccount>,
    pub groups: Vec<RemoteGroup>,
    pub favourites: Vec<RemoteFavourite>,
    pub instances: Vec<RemoteInstance>,
    pub metrics: Vec<ClientMetrics>,
    pub system_usage: Option<SystemUsage>,
    pub bulk_progress: Option<RemoteBulk>,
    #[ts(type = "number | null")]
    pub shutdown_at: Option<i64>,
    pub crashes: Vec<RemoteCrash>,
}

/// PC->Viewer encrypted JSON event. An event carrying first pages has a `batch` number that each
/// of its `AccountsPage` and `Page` events repeats, so a page is only ever added to the lists of
/// the event it belongs to.
#[derive(Serialize, TS, Clone, Debug)]
#[serde(tag = "t", rename_all = "camelCase")]
#[ts(export)]
#[allow(clippy::large_enum_variant)] // built once per send and serialized right away
pub enum RemoteEvent {
    Snapshot {
        batch: u32,
        data: RemoteSnapshot,
    },
    /// Both lists are first pages, as in `Snapshot`.
    AccountsUpdated {
        batch: u32,
        accounts: Vec<RemoteAccount>,
        groups: Vec<RemoteGroup>,
    },
    /// The next accounts after the `Snapshot` or `AccountsUpdated` of `batch`, from index `offset` on.
    AccountsPage {
        batch: u32,
        offset: u32,
        accounts: Vec<RemoteAccount>,
    },
    /// The next items of the list `page` names, from index `offset` on, right after the event of
    /// `batch` that carried its first page (`Snapshot`, `AccountsUpdated`, `InstancesUpdated`,
    /// `MetricsUpdated` or `BulkProgress`).
    Page {
        batch: u32,
        offset: u32,
        page: RemotePage,
    },
    /// `instances` is the first page, as in `Snapshot`.
    InstancesUpdated {
        batch: u32,
        instances: Vec<RemoteInstance>,
    },
    /// `metrics` is the first page, as in `Snapshot`.
    MetricsUpdated {
        batch: u32,
        metrics: Vec<ClientMetrics>,
        #[serde(rename = "systemUsage")]
        system_usage: Option<SystemUsage>,
        #[serde(rename = "windowsLocked")]
        windows_locked: bool,
        #[serde(rename = "vaultLocked")]
        vault_locked: bool,
        #[serde(rename = "shutdownAt")]
        #[ts(type = "number | null")]
        shutdown_at: Option<i64>,
    },
    /// `progress.items` is the first page, as in `Snapshot`.
    BulkProgress {
        batch: u32,
        progress: RemoteBulk,
    },
    RpcResult {
        #[serde(rename = "reqId")]
        req_id: u32,
        ok: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[ts(type = "unknown | null")]
        data: Option<serde_json::Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        code: Option<String>,
    },
    #[allow(dead_code)]
    Notice {
        error: bool,
        message: String,
    },
    /// Phase 2 reserved WebRTC signaling envelope.
    #[allow(dead_code)]
    WebRtcSignal {
        #[serde(rename = "instanceId")]
        instance_id: String,
        #[ts(type = "unknown")]
        payload: serde_json::Value,
    },
}

/// The rest of one of the lists that can grow; accounts have `AccountsPage`.
#[derive(Serialize, TS, Clone, Debug)]
#[serde(tag = "list", content = "items", rename_all = "camelCase")]
#[ts(export)]
pub enum RemotePage {
    Groups(Vec<RemoteGroup>),
    Favourites(Vec<RemoteFavourite>),
    Instances(Vec<RemoteInstance>),
    Metrics(Vec<ClientMetrics>),
    Crashes(Vec<RemoteCrash>),
    /// `BulkProgress.items` of the batch just sent.
    BulkItems(Vec<BulkItem>),
}

/// JSON bytes of one list per event: a snapshot carries the first page of each of its seven lists
/// and stays under the relay's frame cap however long they get.
pub const PAGE_BYTES: usize = 24 * 1024;

/// Splits a list into pages of at most `PAGE_BYTES` of JSON each (an item bigger than that gets a
/// page of its own); one page even when empty.
pub fn pages<T: Serialize>(items: Vec<T>) -> Vec<Vec<T>> {
    let mut pages: Vec<Vec<T>> = vec![Vec::new()];
    let mut size = 0;
    for item in items {
        let n = serde_json::to_vec(&item).map_or(0, |j| j.len() + 1);
        if size + n > PAGE_BYTES && pages.last().is_some_and(|p| !p.is_empty()) {
            pages.push(Vec::new());
            size = 0;
        }
        size += n;
        if let Some(page) = pages.last_mut() {
            page.push(item);
        }
    }
    pages
}

/// The pages that follow one event carrying first pages, all of its `batch`.
pub struct Pages {
    pub batch: u32,
    rest: Vec<RemoteEvent>,
}

impl Default for Pages {
    fn default() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        Self { batch: NEXT.fetch_add(1, Ordering::Relaxed), rest: Vec::new() }
    }
}

impl Pages {
    /// The event `first` makes for this batch, then its pages.
    pub fn after(self, first: impl FnOnce(u32) -> RemoteEvent) -> Vec<RemoteEvent> {
        std::iter::once(first(self.batch)).chain(self.rest).collect()
    }
}

/// The first page of `items`; each further page goes to `more` as `rest(batch, offset, page)`.
pub fn first_page<T: Serialize>(items: Vec<T>, more: &mut Pages, rest: impl Fn(u32, u32, Vec<T>) -> RemoteEvent) -> Vec<T> {
    let mut pages = pages(items).into_iter();
    let first = pages.next().unwrap_or_default();
    let mut offset = first.len();
    for page in pages {
        let at = u32::try_from(offset).unwrap_or(u32::MAX);
        offset += page.len();
        more.rest.push(rest(more.batch, at, page));
    }
    first
}

/// `rest` for `first_page`: a `Page` of the list `wrap` makes.
pub fn page_of<T>(wrap: fn(Vec<T>) -> RemotePage) -> impl Fn(u32, u32, Vec<T>) -> RemoteEvent {
    move |batch, offset, items| RemoteEvent::Page { batch, offset, page: wrap(items) }
}

/// `rest` for `first_page`: an `AccountsPage`.
pub fn accounts_page(batch: u32, offset: u32, accounts: Vec<RemoteAccount>) -> RemoteEvent {
    RemoteEvent::AccountsPage { batch, offset, accounts }
}

/// Encodes a JSON message with the `APP_KIND_JSON` prefix.
pub fn encode_json<T: Serialize>(v: &T) -> Option<Vec<u8>> {
    let json = serde_json::to_vec(v).ok()?;
    let mut out = Vec::with_capacity(1 + json.len());
    out.push(APP_KIND_JSON);
    out.extend_from_slice(&json);
    Some(out)
}

/// Encodes a binary JPEG preview frame with `APP_KIND_PREVIEW` prefix.
pub fn encode_preview_frame(instance_id: &str, seq: u32, age_ms: u32, status: u8, jpeg: &[u8]) -> Option<Vec<u8>> {
    let id_bytes = instance_id.as_bytes();
    if id_bytes.len() > 255 {
        return None;
    }
    let mut out = Vec::with_capacity(1 + 1 + id_bytes.len() + 4 + 4 + 1 + jpeg.len());
    out.push(APP_KIND_PREVIEW);
    out.push(id_bytes.len() as u8);
    out.extend_from_slice(id_bytes);
    out.extend_from_slice(&seq.to_le_bytes());
    out.extend_from_slice(&age_ms.to_le_bytes());
    out.push(status);
    out.extend_from_slice(jpeg);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BulkStage, FavouriteOptions, SameServer};

    fn account(id: i64, alias: &str) -> AccountSummary {
        AccountSummary {
            id,
            user_id: 9,
            username: "alt_one".into(),
            display_name: "Alt".into(),
            alias: alias.into(),
            notes: "NOTE_SECRET".into(),
            color: None,
            favourite: false,
            sort_order: 0,
            group_ids: vec![1],
            status: AccountStatus::Valid,
            status_checked_at: None,
            created_at: 0,
            allow_disconnect: false,
            launch_args: Some("--ARGS_SECRET".into()),
            discord_muted: false,
            fps_cap: None,
            full_quality: false,
            main: false,
            custom_flags: [("FFlagSECRET".to_owned(), "1".to_owned())].into(),
            client_limits: None,
            profile: Some(Default::default()),
        }
    }

    fn instance() -> Instance {
        Instance {
            id: "i1".into(),
            account_id: 1,
            pid: 4,
            state: InstanceState::InGame,
            target: LaunchTarget::PrivateAccess { place_id: 1818, access_code: "ACCESS_SECRET".into() },
            place_id: Some(1818),
            job_id: Some("JOB_SECRET".into()),
            started_at: 0,
            ended_at: None,
            via_bootstrapper: false,
            note: None,
            rejoins: 0,
            limits: Limits::default(),
            anti_afk: true,
            afk_at: None,
            throttled: false,
            micro: false,
            micro_size: None,
            fps_cap: None,
            relaunching: false,
            relaunch_problem: None,
            sign_in_problem: false,
            applied_limits: Limits::default(),
            held: false,
        }
    }

    #[test]
    fn snapshot_leaves_out_secrets_and_codes() {
        let favourite = Favourite {
            id: 3,
            name: "Farm".into(),
            target: LaunchTarget::PrivateLink { place_id: 1818, link_code: "LINK_SECRET".into() },
            account_ids: vec![1],
            in_tray: false,
            sort_order: 0,
            options: FavouriteOptions::default(),
        };
        let bulk = BulkProgress {
            batch_id: "b".into(),
            target: LaunchTarget::Place { place_id: 1, launch_data: Some("DATA_SECRET".into()) },
            same_server: SameServer::Off,
            delay_ms: 6000,
            job_id: Some("JOB_SECRET".into()),
            items: vec![BulkItem { account_id: 1, user_id: 9, stage: BulkStage::Queued, instance_id: None }],
            stopped: None,
            done: false,
            note: None,
        };
        let group = Group { id: 1, name: "Farmers".into(), color: None, sort_order: 0, defaults: Default::default() };
        let snap = RemoteSnapshot {
            pc_name: "PC".into(),
            app_version: "1".into(),
            role: RemoteRole::Readonly,
            vault_locked: false,
            windows_locked: false,
            accounts: vec![(&account(1, "a")).into()],
            groups: vec![(&group).into()],
            favourites: vec![(&favourite).into()],
            instances: vec![(&instance()).into()],
            metrics: Vec::new(),
            system_usage: None,
            bulk_progress: Some((&bulk).into()),
            shutdown_at: None,
            crashes: Vec::new(),
        };
        let json = serde_json::to_string(&RemoteEvent::Snapshot { batch: 0, data: snap }).unwrap();
        for secret in ["SECRET", "notes", "launchArgs", "customFlags", "profile", "linkCode", "accessCode", "launchData", "jobId", "defaults"] {
            assert!(!json.contains(secret), "{secret} reached the viewer: {json}");
        }
        assert!(json.contains(r#""target":{"kind":"privateAccess","placeId":1818}"#));
        assert!(json.contains(r#""favourites":[{"id":3,"name":"Farm","placeId":1818}]"#));
    }

    #[test]
    fn pages_stay_under_the_budget_and_keep_order() {
        assert_eq!(pages(Vec::<RemoteGroup>::new()).len(), 1);
        let accounts: Vec<RemoteAccount> = (0..3000).map(|i| (&account(i, &"x".repeat(200))).into()).collect();
        let split = pages(accounts.clone());
        assert!(split.len() > 1);
        for page in &split {
            assert!(serde_json::to_vec(page).unwrap().len() <= PAGE_BYTES + 1);
        }
        assert_eq!(split.concat(), accounts);
    }

    #[test]
    fn the_rest_of_a_list_follows_with_its_offsets() {
        let groups: Vec<RemoteGroup> = (0..5000).map(|id| RemoteGroup { id, name: "g".repeat(40) }).collect();
        let mut more = Pages::default();
        let mut got = first_page(groups.clone(), &mut more, page_of(RemotePage::Groups));
        let batch = more.batch;
        assert_ne!(Pages::default().batch, batch, "every carrying event gets a batch of its own");
        let events = more.after(|batch| RemoteEvent::InstancesUpdated { batch, instances: Vec::new() });
        assert!(matches!(events[0], RemoteEvent::InstancesUpdated { batch: b, .. } if b == batch));
        assert!(events.len() > 1);
        for ev in events.into_iter().skip(1) {
            let RemoteEvent::Page { batch: b, offset, page: RemotePage::Groups(items) } = ev else { panic!("a groups page") };
            assert_eq!((b, offset as usize), (batch, got.len()));
            got.extend(items);
        }
        assert_eq!(got, groups);
        let json = serde_json::to_string(&RemoteEvent::Page { batch: 7, offset: 3, page: RemotePage::BulkItems(Vec::new()) }).unwrap();
        assert_eq!(json, r#"{"t":"page","batch":7,"offset":3,"page":{"list":"bulkItems","items":[]}}"#);
    }

    #[test]
    fn a_snapshot_of_huge_lists_still_fits_one_frame() {
        let mut more = Pages::default();
        let n = 20_000;
        let long = "\u{1}".repeat(300);
        let crash = Crash {
            id: 1,
            instance_id: "i".into(),
            account_id: 1,
            kind: CrashKind::Crash,
            at: 0,
            uptime_ms: 0,
            reason: Some(long.clone()),
            place_id: None,
            has_log: false,
            verdict: None,
        };
        let metric = ClientMetrics {
            instance_id: long.clone(),
            pid: 1,
            cpu_pct: Some(1.0 / 3.0),
            memory_mb: 1.0 / 3.0,
            private_mb: 1.0 / 3.0,
            read_bps: None,
            write_bps: None,
            gpu_pct: None,
            gpu_mem_mb: None,
        };
        let mut inst = instance();
        inst.note = Some(long.clone());
        let item = BulkItem { account_id: 1, user_id: 1, stage: BulkStage::Failed { message: long.clone() }, instance_id: Some(long.clone()) };
        let snap = RemoteSnapshot {
            pc_name: long.chars().take(48).collect(),
            app_version: "1".into(),
            role: RemoteRole::Control,
            vault_locked: false,
            windows_locked: false,
            accounts: first_page(vec![(&account(1, &long)).into(); n], &mut more, accounts_page),
            groups: first_page(vec![RemoteGroup { id: 1, name: long.clone() }; n], &mut more, page_of(RemotePage::Groups)),
            favourites: first_page(vec![RemoteFavourite { id: 1, name: long.clone(), place_id: None }; n], &mut more, page_of(RemotePage::Favourites)),
            instances: first_page(vec![(&inst).into(); n], &mut more, page_of(RemotePage::Instances)),
            metrics: first_page(vec![metric; n], &mut more, page_of(RemotePage::Metrics)),
            system_usage: None,
            bulk_progress: Some(RemoteBulk {
                batch_id: "b".into(),
                items: first_page(vec![item; n], &mut more, page_of(RemotePage::BulkItems)),
                done: false,
            }),
            shutdown_at: None,
            crashes: first_page(vec![(&crash).into(); n], &mut more, page_of(RemotePage::Crashes)),
        };
        assert!(more.after(|batch| RemoteEvent::Snapshot { batch, data: snap }).iter().all(|ev| super::super::fits_frame(encode_json(ev).unwrap().len())));
    }

    #[test]
    fn free_text_is_clipped_so_one_item_never_nears_the_frame_cap() {
        let huge = "\u{1F600}".repeat(100_000);
        let group = RemoteGroup::from(&Group { id: 1, name: huge, color: None, sort_order: 0, defaults: Default::default() });
        assert_eq!(group.name.chars().count(), MAX_REMOTE_TEXT);
        assert!(serde_json::to_vec(&group).unwrap().len() < PAGE_BYTES);
        assert_eq!(clip("short"), "short");
    }
}
