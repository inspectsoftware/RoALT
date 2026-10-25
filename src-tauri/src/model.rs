//! Types shared with the UI. `npm run bindings` exports each one to `src/bindings/<Type>.ts`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum VaultMode {
    /// Data key wrapped with Windows DPAPI for the current user; opens without a prompt.
    Dpapi,
    /// Data key wrapped with a key derived from a master password (and DPAPI on top).
    Master,
}

#[derive(Serialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VaultStatus {
    pub mode: VaultMode,
    pub locked: bool,
    pub accounts: u32,
    pub auto_lock_minutes: u32,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum AccountStatus {
    Unknown,
    Valid,
    Invalid,
    /// Roblox asked for a human check (captcha / 2SV) on the last validation.
    Challenged,
}

impl AccountStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::Challenged => "challenged",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "valid" => Self::Valid,
            "invalid" => Self::Invalid,
            "challenged" => Self::Challenged,
            _ => Self::Unknown,
        }
    }
}

/// One stored account as the UI sees it. Never carries the cookie.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountSummary {
    pub id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub alias: String,
    pub notes: String,
    pub color: Option<String>,
    pub favourite: bool,
    pub sort_order: i64,
    pub group_ids: Vec<i64>,
    pub status: AccountStatus,
    /// Unix ms.
    pub status_checked_at: Option<i64>,
    /// Unix ms.
    pub created_at: i64,
}

/// Partial update from the UI; `None` leaves a field as it is.
/// `color: Some("")` clears the colour.
#[derive(Deserialize, TS, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountPatch {
    #[ts(optional)]
    pub alias: Option<String>,
    #[ts(optional)]
    pub notes: Option<String>,
    #[ts(optional)]
    pub color: Option<String>,
    #[ts(optional)]
    pub favourite: Option<bool>,
    #[ts(optional)]
    pub group_ids: Option<Vec<i64>>,
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub color: Option<String>,
    pub sort_order: i64,
}

/// Create (`id: None`) or rename / recolour a group.
#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GroupInput {
    #[ts(optional)]
    pub id: Option<i64>,
    pub name: String,
    #[ts(optional)]
    pub color: Option<String>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Theme {
    #[default]
    Dark,
    Light,
    System,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Density {
    #[default]
    Detailed,
    Compact,
}

/// App settings, stored as one JSON value. New fields need `#[serde(default)]` semantics,
/// which the struct-level default gives.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Settings {
    pub theme: Theme,
    pub density: Density,
    /// Master-password mode only.
    pub auto_lock_minutes: u32,
    pub clipboard_clear_seconds: u32,
    /// How long account info is reused before Roblox is asked again.
    pub cache_minutes: u32,
    /// Hold Roblox's single-instance lock so several clients can run.
    pub multi_instance: bool,
    /// While multi-instance is on, lock Roblox's shared login files so clients can't
    /// overwrite each other's session (teleport error 773, wrong account shown).
    pub protect_shared_login: bool,
    /// Bulk launch: wait between two launches (±1 s jitter is added), 2 000..=60 000.
    pub launch_delay_ms: u32,
    /// Relaunch a client Roblox disconnects (kicked, server shut down, connection lost; not the
    /// player's own Leave) into the same target, at most 3 tries in a row.
    pub auto_rejoin: bool,
    /// Closing the window hides it; the tray icon brings it back or quits.
    pub close_to_tray: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: Theme::Dark,
            density: Density::Detailed,
            auto_lock_minutes: 15,
            clipboard_clear_seconds: 30,
            cache_minutes: 10,
            multi_instance: false,
            protect_shared_login: true,
            launch_delay_ms: 6_000,
            auto_rejoin: false,
            close_to_tray: false,
        }
    }
}

#[derive(Deserialize, TS, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SettingsPatch {
    #[ts(optional)]
    pub theme: Option<Theme>,
    #[ts(optional)]
    pub density: Option<Density>,
    #[ts(optional)]
    pub auto_lock_minutes: Option<u32>,
    #[ts(optional)]
    pub clipboard_clear_seconds: Option<u32>,
    #[ts(optional)]
    pub cache_minutes: Option<u32>,
    #[ts(optional)]
    pub protect_shared_login: Option<bool>,
    #[ts(optional)]
    pub launch_delay_ms: Option<u32>,
    #[ts(optional)]
    pub auto_rejoin: Option<bool>,
    #[ts(optional)]
    pub close_to_tray: Option<bool>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum Subscription {
    None,
    /// Legacy Premium (no longer sold since 2026-04-30).
    Premium,
    /// Roblox Plus.
    Plus,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum PresenceKind {
    Offline,
    Online,
    InGame,
    Studio,
    Unknown,
}

impl PresenceKind {
    /// presence.roblox.com `userPresenceType`.
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => Self::Offline,
            1 => Self::Online,
            2 => Self::InGame,
            3 => Self::Studio,
            _ => Self::Unknown,
        }
    }
}

/// What Roblox shows about where a user is. Game fields are `None` when the viewer
/// is not allowed to see them.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Presence {
    pub kind: PresenceKind,
    /// Game name when in game, else e.g. "Website".
    pub last_location: Option<String>,
    pub place_id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub universe_id: Option<i64>,
    /// The server (job) id.
    pub game_id: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountPresence {
    pub account_id: i64,
    pub presence: Presence,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Headshot {
    pub account_id: i64,
    pub url: String,
}

/// The account info panel. Each part is `None` when Roblox did not answer for it;
/// `unavailable` names those parts for the UI.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AccountInfo {
    pub account_id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub description: String,
    /// ISO 8601 account creation time.
    pub created: Option<String>,
    pub is_banned: bool,
    pub verified_badge: bool,
    pub headshot_url: Option<String>,
    pub robux: Option<i64>,
    pub subscription: Option<Subscription>,
    pub friends: Option<i64>,
    pub followers: Option<i64>,
    pub following: Option<i64>,
    pub email_verified: Option<bool>,
    /// Enabled 2-step methods ("Email", "Authenticator", ...); empty = 2SV off.
    pub two_step_methods: Option<Vec<String>>,
    /// The account seen with its own cookie.
    pub presence: Option<Presence>,
    /// Unix ms.
    pub fetched_at: i64,
    pub unavailable: Vec<String>,
}

#[derive(Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[ts(export)]
pub enum ConflictPolicy {
    /// Keep the account already stored.
    Skip,
    /// Overwrite cookie and details with the backup's.
    Replace,
}

#[derive(Serialize, TS, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportReport {
    pub added: u32,
    pub updated: u32,
    pub skipped: u32,
}

/// Where to launch. Ids are numbers; codes stay strings (link codes can be 25-32 digits).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum LaunchTarget {
    /// A game (its start place); `launch_data` ≤ 200 bytes goes to the game.
    Place { place_id: i64, launch_data: Option<String> },
    /// One server (job) of a place.
    Server { place_id: i64, job_id: String },
    /// Private server via its link code (classic `?privateServerLinkCode=` links).
    PrivateLink { place_id: i64, link_code: String },
    /// Private server via its access code (a GUID).
    PrivateAccess { place_id: i64, access_code: String },
    /// `roblox.com/share?code=..&type=..`: resolved with the launching account's cookie.
    ShareLink { code: String, link_type: String },
    /// `roblox.com/share?code=..&v=v2`.
    ShareLinkV2 { code: String },
    /// A game named by universe; launched at its root place.
    Universe { universe_id: i64 },
    /// Join whatever server this user is in.
    FollowUser { user_id: i64 },
}

/// What `target_parse` made of pasted text.
#[derive(Serialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ParsedInput {
    Target { target: LaunchTarget },
    /// A bare job id: the UI must ask for the place.
    JobIdOnly { job_id: String },
    /// `@name` or a bare username: look it up, then follow.
    Username { name: String },
    Invalid { reason: String },
    Unsupported { reason: String },
}

#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Favourite {
    pub id: i64,
    pub name: String,
    pub target: LaunchTarget,
    /// Accounts a one-click (or tray) launch uses.
    pub account_ids: Vec<i64>,
    pub in_tray: bool,
    pub sort_order: i64,
    /// Unix ms.
    pub last_used_at: Option<i64>,
}

#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FavouriteInput {
    #[ts(optional)]
    pub id: Option<i64>,
    pub name: String,
    pub target: LaunchTarget,
    pub account_ids: Vec<i64>,
    pub in_tray: bool,
}

/// Someone an account can join: a friend online on Roblox, or another stored account.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JoinCandidate {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    /// Set when this is one of the stored accounts.
    pub account_id: Option<i64>,
    pub presence: Presence,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UserRef {
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
}

/// A client RoALT started.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchInfo {
    pub account_id: i64,
    /// Process id of what was started (the Roblox client, or a bootstrapper such as Bloxstrap).
    pub pid: u32,
    /// The `browsertrackerid` this launch used; the client's log names it.
    pub browser_tracker_id: i64,
    /// The place it was sent to, when known before the client starts.
    pub place_id: Option<i64>,
    /// Unix ms.
    pub started_at: i64,
    /// True when a bootstrapper (not RobloxPlayerBeta.exe) handled the launch.
    pub via_bootstrapper: bool,
    /// Something the user should know (expired share link, launch data left out).
    pub warning: Option<String>,
    /// Start time of `pid` (Unix s, 0: already gone), so a reused PID is never taken for it.
    #[serde(skip)]
    #[ts(skip)]
    pub pid_start: Option<u64>,
}

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum InstanceState {
    /// Process started, its log not found yet.
    Starting,
    /// Log found; joining a server.
    Joining,
    InGame,
    /// Lost the server (kick, shutdown, network); the client may still be open.
    Disconnected,
    Exited,
}

/// A Roblox client RoALT started and is following.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Instance {
    pub id: String,
    pub account_id: i64,
    pub pid: u32,
    pub state: InstanceState,
    pub target: LaunchTarget,
    pub place_id: Option<i64>,
    pub job_id: Option<String>,
    /// Unix ms.
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub via_bootstrapper: bool,
    /// Why it ended or what went wrong: "Closed", "Replaced by another Roblox client",
    /// "Signed in as another account", "Roblox wants a human check", ...
    pub note: Option<String>,
    /// How many auto-rejoins led to this client (0: launched by the user).
    pub rejoins: u32,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MultiInstanceStatus {
    /// The setting.
    pub enabled: bool,
    /// The lock is actually held.
    pub active: bool,
    pub protect_shared_login: bool,
    /// PIDs of Roblox processes that stop the lock from being taken.
    pub blocked_by: Vec<u32>,
    pub message: Option<String>,
}

// ---------- M5: bulk launch ----------

/// How a bulk launch puts everyone in one server (public-game targets only; targets that
/// already name one server ignore it).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SameServer {
    /// Each account is matched by Roblox on its own.
    #[default]
    Off,
    /// Choose one server with room for everyone from the server list, then send each account
    /// to it (`RequestGameJob`).
    PickFirst,
    /// Launch the first account normally, read its server from its client log (presence as a
    /// fallback), send the rest there.
    FollowFirst,
}

#[derive(Deserialize, TS, Clone, Debug)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkRequest {
    /// Launch order.
    pub account_ids: Vec<i64>,
    pub target: LaunchTarget,
    /// Wait between launches; clamped to 2 000..=60 000, ±1 s jitter is added.
    pub delay_ms: u32,
    pub same_server: SameServer,
    /// Marked as used once the first client starts.
    pub favourite_id: Option<i64>,
}

/// One account's progress. After `Starting` it mirrors its client (`Instance`).
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(export)]
pub enum BulkStage {
    Queued,
    /// Resolving the target / getting the ticket.
    Ticket,
    /// The client was started; its log isn't bound yet.
    Starting,
    Joining,
    InGame,
    Failed { message: String },
    Cancelled,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkItem {
    pub account_id: i64,
    /// The Roblox user the account was when the batch started (account ids are reused).
    pub user_id: i64,
    pub stage: BulkStage,
    /// The tracked client, once started.
    pub instance_id: Option<String>,
}

/// The whole batch; `bulk://progress` carries it on every change.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BulkProgress {
    pub batch_id: String,
    pub target: LaunchTarget,
    pub same_server: SameServer,
    pub delay_ms: u32,
    /// The server everyone is sent to, once known.
    pub job_id: Option<String>,
    pub items: Vec<BulkItem>,
    /// Why the batch stopped early ("Cancelled", a challenge, a wrong account); None while it
    /// runs and after a normal finish.
    pub stopped: Option<String>,
    /// No more launches or stage changes will come.
    pub done: bool,
    /// Something the user should know (no server had room, the first account's server
    /// couldn't be read, ...).
    pub note: Option<String>,
}

// ---------- M5/M6: servers ----------

#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ServerOrder {
    OccupancyDesc,
    OccupancyAsc,
    /// Needs a cookie.
    Recommended,
    /// Needs a cookie.
    BestLatency,
}

/// One public server. Roblox may leave anything but id / maxPlayers / playing out.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameServer {
    /// The job id (a GUID).
    pub id: String,
    pub playing: i32,
    pub max_players: i32,
    pub fps: Option<f64>,
    pub ping: Option<i64>,
    pub friend_count: Option<i32>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ServerPage {
    pub servers: Vec<GameServer>,
    /// Opaque; None on the last page.
    pub next_cursor: Option<String>,
}

// ---------- M6: server browser ----------

/// What the Servers view shows about a game.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GameInfo {
    /// The place that was asked about (may differ from the root place).
    pub place_id: i64,
    pub universe_id: i64,
    pub root_place_id: i64,
    pub name: String,
    pub creator: String,
    pub playing: i64,
    pub visits: i64,
    pub max_players: i32,
    /// 150×150 icon on *.rbxcdn.com, when Roblox has one.
    pub icon_url: Option<String>,
    /// The game sells private servers.
    pub private_servers: bool,
}

/// One private server of a place, as one account sees it.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivateServer {
    pub vip_server_id: i64,
    pub name: String,
    pub owner_id: Option<i64>,
    pub owner_name: Option<String>,
    pub playing: i32,
    pub max_players: i32,
    /// The running server's job id, when it runs.
    pub job_id: Option<String>,
    /// Only when this account can join it (launch with `LaunchTarget::PrivateAccess`).
    pub access_code: Option<String>,
}

#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PrivateServerPage {
    pub servers: Vec<PrivateServer>,
    /// Opaque; None on the last page.
    pub next_cursor: Option<String>,
    /// Roblox says this account may not join servers of this game (`gameJoinRestricted`).
    pub join_restricted: bool,
}

// ---------- M7 ----------

/// The Roblox client RoALT would start.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RobloxClient {
    /// None when Roblox isn't installed.
    pub path: Option<String>,
    /// A bootstrapper (e.g. Bloxstrap) handles launches.
    pub bootstrapper: bool,
}

/// Where RoALT keeps its data.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DataDir {
    pub path: String,
    /// `%LOCALAPPDATA%\RoALT` (not moved).
    pub default: bool,
    /// Set for test runs (`ROALT_DATA_DIR`): moving is not offered.
    pub overridden: bool,
}

/// A message for the user from the backend (`app://notice`), shown as a toast.
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Notice {
    pub error: bool,
    pub message: String,
}
