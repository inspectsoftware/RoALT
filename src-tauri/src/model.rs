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
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: Theme::Dark, density: Density::Detailed, auto_lock_minutes: 15, clipboard_clear_seconds: 30 }
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
