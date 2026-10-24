//! Response types. Lenient: unknown fields are ignored.

use serde::Deserialize;

#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthenticatedUser {
    pub id: i64,
    pub name: String,
    pub display_name: String,
}

/// `GET users.roblox.com/v1/users/{id}`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserDetails {
    pub id: i64,
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub description: String,
    /// ISO 8601.
    pub created: Option<String>,
    #[serde(default)]
    pub is_banned: bool,
    #[serde(default)]
    pub has_verified_badge: bool,
}

/// Friends / followers / following counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocialCounts {
    pub friends: i64,
    pub followers: i64,
    pub following: i64,
}

/// One row of `POST presence.roblox.com/v1/presence/users`.
#[derive(Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct UserPresence {
    pub user_id: i64,
    pub user_presence_type: i64,
    pub last_location: Option<String>,
    pub place_id: Option<i64>,
    pub root_place_id: Option<i64>,
    pub game_id: Option<String>,
    pub universe_id: Option<i64>,
}

/// `thumbnails.roblox.com/v1/users/avatar-headshot` (and `/v1/batch`).
#[derive(Deserialize, Debug)]
pub struct Thumbnails {
    pub data: Vec<Thumbnail>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Thumbnail {
    pub target_id: i64,
    /// Completed, Pending, Blocked, Error, InReview, TemporarilyUnavailable.
    pub state: String,
    /// `""` while Pending; Blocked may carry a placeholder.
    pub image_url: Option<String>,
}

/// `get-profiles`. Roblox silently drops fields it doesn't know, row by row.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profiles {
    #[serde(default)]
    pub profile_details: Vec<Profile>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub user_id: i64,
    pub has_roblox_subscription: Option<bool>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Presences {
    pub user_presences: Vec<UserPresence>,
}

/// `GET twostepverification.roblox.com/v1/users/{id}/configuration`.
#[derive(Deserialize, Debug)]
pub struct TwoStepConfig {
    #[serde(default)]
    pub methods: Vec<TwoStepMethod>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TwoStepMethod {
    /// An enum int (0 Email … 7 Passkey) or its name.
    pub media_type: serde_json::Value,
    #[serde(default)]
    pub enabled: bool,
}
