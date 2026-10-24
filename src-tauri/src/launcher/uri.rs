//! The `roblox-player:` URI and PlaceLauncher URL, byte-for-byte as roblox.com builds them
//! (docs/research/fixtures/authlaunch-web-launch-uri-spec.json).

use uuid::Uuid;

use super::Join;
use crate::vault::Secret;

pub const PLACE_LAUNCHER: &str = "https://www.roblox.com/Game/PlaceLauncher.ashx";

pub struct PlaceLaunch<'a> {
    pub join: &'a Join,
    pub browser_tracker_id: i64,
    pub join_attempt_id: Uuid,
}

/// `https://www.roblox.com/Game/PlaceLauncher.ashx?request=<T>&browserTrackerId=<id>&...`
/// with the website's parameter order; keys and values encodeURIComponent-encoded, `%20` → `+`.
/// Game: placeId, [launchData], isPlayTogetherGame=false, joinAttemptId, joinAttemptOrigin=PlayButton.
/// Job: placeId, gameId, isPlayTogetherGame=false, joinAttemptId, joinAttemptOrigin=JoinInstance.
/// Private: placeId, [accessCode], [linkCode], joinAttemptId, joinAttemptOrigin=PlayButton.
/// Follow: userId, joinAttemptId, joinAttemptOrigin=JoinUser.
pub fn place_launcher_url(p: &PlaceLaunch) -> String {
    let _ = p;
    todo!("uri: place_launcher_url")
}

/// `roblox-player:1+launchmode:play+gameinfo:<enc ticket>+launchtime:<ms>+placelauncherurl:<enc url>+
/// browsertrackerid:<id>+robloxLocale:en_us+gameLocale:en_us+LaunchExp:InApp` (no channel:
/// production). The result holds the ticket, so it is a `Secret`.
pub fn player_uri(ticket: &Secret, launch_time_ms: i64, p: &PlaceLaunch) -> Secret {
    let _ = (ticket, launch_time_ms, p);
    todo!("uri: player_uri")
}

/// A random 16-digit browser tracker id, unique per launch (the client log names it, which is
/// how a client is tied back to its account).
pub fn new_browser_tracker_id() -> i64 {
    todo!("uri: new_browser_tracker_id")
}
