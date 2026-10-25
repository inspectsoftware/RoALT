//! Launching one account into a target, the way roblox.com's Play button does:
//! resolve the target with that account's cookie, get an authentication ticket, build the
//! `roblox-player:` URI and start the client. Tickets and URIs are secrets: never logged.

pub mod bulk;
pub mod client;
pub mod target;
pub mod uri;

use serde_json::Value;
use uuid::Uuid;

use crate::App;
use crate::error::{AppError, Result};
use crate::model::{LaunchInfo, LaunchTarget};
use crate::roblox_api::Client;
use crate::roblox_api::types::{ShareLinkResolution, ShareLinkV2, ShareTarget};
use crate::vault::{Secret, cookie_aad};

/// Roblox's limit on launch data, in bytes.
const MAX_LAUNCH_DATA: usize = 200;

/// A target resolved for one account: what PlaceLauncher is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Join {
    /// `RequestGame`.
    Game { place_id: i64, launch_data: Option<String> },
    /// `RequestGameJob` (`gameId` = job id).
    Job { place_id: i64, job_id: String },
    /// `RequestPrivateGame`; share-link and `privateServerLinkCode` joins send only the link code.
    Private { place_id: i64, link_code: Option<String>, access_code: Option<String> },
    /// `RequestFollowUser`.
    Follow { user_id: i64 },
}

impl Join {
    /// The place the client goes to, when known before it starts.
    pub fn place_id(&self) -> Option<i64> {
        match self {
            Join::Game { place_id, .. } | Join::Job { place_id, .. } | Join::Private { place_id, .. } => Some(*place_id),
            Join::Follow { .. } => None,
        }
    }
}

/// Resolves `target` with the account's own cookie into a `Join`:
/// - `ShareLink` → `POST apis.roblox.com/sharelinks/v1/resolve-link`. Server links use
///   `privateServerInviteData` (`placeId` + `linkCode`; universe → root place only when
///   `placeId` is missing). `Invalid` status → `AppError::Invalid("This share link is no
///   longer valid.")`; `Expired` still joins, as the website does, with a warning.
///   ExperienceInvite / NotificationExperienceInvite with an instance → `Job`; ExperienceDetails /
///   Affiliate / Event / Screenshot → the game (root place, launch data when given); Profile →
///   `Follow`; other types → `Invalid`.
/// - `ShareLinkV2` → `POST apis.roblox.com/deeplinks/v2/resolve`; `Experience` → `Game`
///   (`targetId` place, `customData` as launch data); other link types → `Invalid`.
/// - `Universe` → its root place (`games.roblox.com/v1/games?universeIds=`).
/// - Launch data over 200 bytes is dropped (Roblox's limit) with a warning.
///
/// The second value is a warning for the user (expired link, dropped launch data).
pub async fn resolve(
    api: &Client,
    account_id: i64,
    cookie: &Secret,
    target: &LaunchTarget,
) -> Result<(Join, Option<&'static str>)> {
    let mut warning = None;
    let join = match target.clone() {
        LaunchTarget::Place { place_id, launch_data } => Join::Game { place_id, launch_data },
        LaunchTarget::Server { place_id, job_id } => Join::Job { place_id, job_id },
        LaunchTarget::PrivateLink { place_id, link_code } => {
            Join::Private { place_id, link_code: Some(link_code), access_code: None }
        }
        LaunchTarget::PrivateAccess { place_id, access_code } => {
            Join::Private { place_id, link_code: None, access_code: Some(access_code) }
        }
        LaunchTarget::ShareLink { code, link_type } => {
            let (join, expired) =
                share_join(api, api.resolve_share_link(account_id, cookie, &code, &link_type).await?).await?;
            warning = expired.then_some(EXPIRED);
            join
        }
        LaunchTarget::ShareLinkV2 { code } => {
            let v = api.resolve_share_link_v2(account_id, cookie, &code).await?;
            warning = is_status(&v.status, "Expired").then_some(EXPIRED);
            v2_join(v)?
        }
        LaunchTarget::Universe { universe_id } => {
            Join::Game { place_id: root_place(api, universe_id).await?, launch_data: None }
        }
        LaunchTarget::FollowUser { user_id } => Join::Follow { user_id },
    };
    let (join, dropped) = cap_launch_data(join);
    Ok((join, warning.or(dropped.then_some(DATA_DROPPED))))
}

const EXPIRED: &str = "This share link has expired, so Roblox may refuse the join.";
const DATA_DROPPED: &str = "The link's launch data is over Roblox's 200-byte limit and was left out.";

/// One launch: resolve → `client-assertion` (failure ignored) → `authentication-ticket` →
/// URI with a fresh `browsertrackerid` → start the client (`client::find` + `client::start`).
/// `AppError::Locked` when the vault is locked; cookie errors as the API client maps them.
pub async fn launch(s: &App, account_id: i64, target: &LaunchTarget) -> Result<LaunchInfo> {
    let cookie = account_cookie(s, account_id)?;
    // Before any network call: no ticket is fetched when Roblox isn't installed.
    let exe = client::find()?;
    let (join, warning) = resolve(&s.api, account_id, &cookie, target).await?;
    let assertion = s.api.client_assertion(account_id, &cookie).await.ok();
    let ticket = s.api.authentication_ticket(account_id, &cookie, assertion.as_ref()).await?;
    let (browser_tracker_id, started_at) = (uri::new_browser_tracker_id(), crate::commands::now_ms());
    let place = uri::PlaceLaunch { join: &join, browser_tracker_id, join_attempt_id: Uuid::new_v4() };
    let (pid, pid_start) = client::start(&exe, &uri::player_uri(&ticket, started_at, &place))?;
    log::info!("launched account {account_id} (pid {pid}, bootstrapper: {})", exe.bootstrapper);
    Ok(LaunchInfo {
        account_id,
        pid,
        browser_tracker_id,
        place_id: join.place_id(),
        started_at,
        via_bootstrapper: exe.bootstrapper,
        warning: warning.map(str::to_owned),
        pid_start: Some(pid_start),
    })
}

/// The account's decrypted cookie (`Locked` when the vault is locked).
pub(crate) fn account_cookie(s: &App, account_id: i64) -> Result<Secret> {
    let (user_id, blob) = s.db.account_cookie(account_id)?;
    s.vault.decrypt_secret(&cookie_aad(user_id), &blob)
}

enum ShareKind {
    Server,
    Invite,
    Game,
    Profile,
}

/// The filled `*Data` object of a share-link answer, mapped as `resolve` describes, and
/// whether the link has expired.
async fn share_join(api: &Client, r: ShareLinkResolution) -> Result<(Join, bool)> {
    let filled = [
        (ShareKind::Server, r.private_server_invite_data),
        (ShareKind::Invite, r.experience_invite_data),
        (ShareKind::Invite, r.notification_experience_invite_data),
        (ShareKind::Game, r.screenshot_invite_data),
        (ShareKind::Game, r.experience_details_invite_data),
        (ShareKind::Game, r.experience_affiliate_data),
        (ShareKind::Game, r.experience_event_data),
        (ShareKind::Profile, r.profile_link_resolution_response_data),
    ];
    let Some((kind, t)) = filled.into_iter().find_map(|(kind, t)| Some((kind, t?))) else {
        return Err(AppError::Invalid("This share link doesn't lead to a game or a player.".into()));
    };
    if is_status(&t.status, "Invalid") {
        return Err(no_longer_valid());
    }
    // As the website does, an invite joins its server only while it is Valid.
    let job = match kind {
        ShareKind::Invite if t.status.is_none() || is_status(&t.status, "Valid") => code(t.instance_id.clone()),
        _ => None,
    };
    let expired = is_status(&t.status, "Expired");
    let join = match (kind, job) {
        (ShareKind::Server, _) => Join::Private {
            place_id: share_place(api, &t).await?,
            link_code: Some(code(t.link_code).ok_or_else(no_longer_valid)?),
            access_code: None,
        },
        (ShareKind::Invite, Some(job_id)) => Join::Job { place_id: share_place(api, &t).await?, job_id },
        (ShareKind::Profile, _) => Join::Follow { user_id: t.user_id.ok_or_else(no_longer_valid)? },
        (ShareKind::Invite | ShareKind::Game, _) => {
            let data = t.launch_data.clone().filter(|d| !d.is_empty());
            let launch_data = data.or_else(|| text(t.join_data.as_ref()?.get("launchData")?));
            Join::Game { place_id: share_place(api, &t).await?, launch_data }
        }
    };
    Ok((join, expired))
}

/// `deeplinks/v2/resolve`: only `Experience` links launch a game.
fn v2_join(v: ShareLinkV2) -> Result<Join> {
    if is_status(&v.status, "Invalid") {
        return Err(no_longer_valid());
    }
    match (v.link_type.as_deref(), v.target_id) {
        (Some("Experience"), Some(place_id)) => {
            Ok(Join::Game { place_id, launch_data: v.custom_data.as_ref().and_then(text) })
        }
        _ => Err(AppError::Invalid("This share link doesn't lead to a game.".into())),
    }
}

/// The payload's place, else its universe's root place.
async fn share_place(api: &Client, t: &ShareTarget) -> Result<i64> {
    match (t.place_id, t.universe_id) {
        (Some(place_id), _) => Ok(place_id),
        (None, Some(universe_id)) => root_place(api, universe_id).await,
        (None, None) => Err(no_longer_valid()),
    }
}

async fn root_place(api: &Client, universe_id: i64) -> Result<i64> {
    let mut found = api.universe_root_places(&[universe_id]).await?;
    let place = found.remove(&universe_id).map(|(place_id, _)| place_id);
    place.ok_or_else(|| AppError::Invalid("This game no longer exists.".into()))
}

/// Drops launch data over Roblox's 200-byte limit (true when it did). Its content is never logged.
fn cap_launch_data(join: Join) -> (Join, bool) {
    match join {
        Join::Game { place_id, launch_data: Some(d) } if d.len() > MAX_LAUNCH_DATA => {
            log::warn!("launch data of {} bytes dropped (limit {MAX_LAUNCH_DATA})", d.len());
            (Join::Game { place_id, launch_data: None }, true)
        }
        join => (join, false),
    }
}

/// A link code or job id from Roblox, kept only if it is a plain token (letters, digits, `-`).
fn code(v: Option<String>) -> Option<String> {
    v.filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
}

/// Launch data as Roblox hands it over: a string as is, other JSON as its text.
fn text(v: &Value) -> Option<String> {
    match v {
        Value::Null => None,
        Value::String(s) => Some(s.clone()).filter(|s| !s.is_empty()),
        v => Some(v.to_string()),
    }
}

fn is_status(status: &Option<String>, want: &str) -> bool {
    status.as_deref().is_some_and(|s| s.eq_ignore_ascii_case(want))
}

fn no_longer_valid() -> AppError {
    AppError::Invalid("This share link is no longer valid.".into())
}
