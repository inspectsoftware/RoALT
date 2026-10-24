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
