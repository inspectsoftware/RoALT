//! Launching one account into a target, the way roblox.com's Play button does:
//! resolve the target with that account's cookie, get an authentication ticket, build the
//! `roblox-player:` URI and start the client. Tickets and URIs are secrets: never logged.

pub mod client;
pub mod target;
pub mod uri;

use crate::App;
use crate::error::Result;
use crate::model::{LaunchInfo, LaunchTarget};
use crate::roblox_api::Client;
use crate::vault::Secret;

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
///   longer valid.")`; `Expired` still joins, as the website does. ExperienceInvite with an
///   instance → `Job`; ExperienceDetails / Affiliate / Event / Notification / Screenshot → the
///   game (root place, launch data when given); Profile → `Follow`; other types → `Invalid`.
/// - `ShareLinkV2` → `POST apis.roblox.com/deeplinks/v2/resolve`; `Experience` → `Game`
///   (`targetId` place, `customData` as launch data); other link types → `Invalid`.
/// - `Universe` → its root place (`games.roblox.com/v1/games?universeIds=`).
/// - Launch data over 200 bytes is dropped (Roblox's limit) with a log line.
pub async fn resolve(api: &Client, account_id: i64, cookie: &Secret, target: &LaunchTarget) -> Result<Join> {
    let _ = (api, account_id, cookie, target);
    todo!("launcher: resolve")
}

/// One launch: resolve → `client-assertion` (failure ignored) → `authentication-ticket` →
/// URI with a fresh `browsertrackerid` → start the client (`client::find` + `client::start`).
/// `AppError::Locked` when the vault is locked; cookie errors as the API client maps them.
pub async fn launch(s: &App, account_id: i64, target: &LaunchTarget) -> Result<LaunchInfo> {
    let _ = (s, account_id, target);
    todo!("launcher: launch")
}
