//! Types of the background sessions shared with the UI (`npm run bindings` exports them), and the
//! plan the elevated setup gets (never sent to the UI: it holds passwords and tokens).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

/// Shared: several clients without macros in one alt user's session. Dedicated: one client, for a
/// continuous AutoHotkey macro.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SessionMode {
    #[default]
    Shared,
    Dedicated,
}

/// One background session, kept in `Settings.sessions`. Its alt user's password and control token
/// are in the vault (`altdock.session.<name>.password` / `.token`), never here.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionConfig {
    /// What the user calls it; the key of the session (its secrets, its `.rdp` file).
    pub name: String,
    /// `altdock-<n>`: given when the session is first saved, never changed.
    pub windows_user: String,
    /// `127.0.0.<n + 1>`: one loopback address (and Credential Manager entry) per session.
    pub address: String,
    /// The control API's port in that session, `47610 + n`.
    pub port: u16,
    pub mode: SessionMode,
    /// The accounts this session runs (at most one in a dedicated session).
    pub account_ids: Vec<i64>,
    /// The session's fixed desktop size.
    pub width: u32,
    pub height: u32,
}

/// Whether an alt user is signed in (`wts`).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum WtsState {
    /// Signed in with an RDP connection (the hidden mstsc window) attached.
    Active,
    /// Signed in, nothing connected: the clients keep running.
    Disconnected,
    /// Not signed in.
    None,
}

// TODO(merge): Stream B's `crate::control::types::ControlHealth` replaces this stand-in (same
// fields as the contract's `GET /v1/health`); delete it and import theirs.
/// What a session's control API says about itself (`GET /v1/health`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ControlHealth {
    pub version: String,
    pub windows_user: String,
    pub session_id: u32,
    pub mode: SessionMode,
    pub vault_locked: bool,
    pub roblox_installed: bool,
    pub clients: u32,
}

/// Whether the AltDock in that session answers.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum SessionControl {
    Ok { health: ControlHealth },
    Down { reason: String },
}

/// A session as the Sessions page shows it (`sessions_list`).
#[derive(Serialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SessionView {
    pub config: SessionConfig,
    pub wts: WtsState,
    /// The session's mstsc window is open and shown (not hidden).
    pub window_visible: bool,
    /// The session's mstsc window is open, shown or hidden.
    pub window_open: bool,
    pub control: SessionControl,
}

/// What fixes a failed health check.
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum FixKind {
    /// The "Set up" button (`sessions_setup`, one admin prompt).
    Setup,
    /// Install RDP Wrapper by its own instructions; AltDock never downloads or installs it.
    RdpWrapper,
    /// Get an `rdpwrap.ini` that knows this Windows build (RDP Wrapper's instructions).
    RdpWrapperIni,
    /// Restart Windows (or the Remote Desktop service).
    Restart,
}

/// One line of the health panel (`sessions_health`).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthCheck {
    /// Stable id (`rdpWrapper`, `rdpWrapperIni`, ...), for the UI's own text and links.
    pub id: String,
    pub ok: bool,
    pub title: String,
    pub detail: String,
    pub fix: Option<FixKind>,
}

/// One alt user the elevated setup creates or repairs, with its control config.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct PlanUser {
    pub name: String,
    pub password: Zeroizing<String>,
    pub port: u16,
    pub token: Zeroizing<String>,
    pub mode: SessionMode,
}

/// What `--altdock-setup <plan.json>` does. Written by the main app into its own data folder
/// (only the user and administrators can read it) and deleted by the elevated side once read.
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlan {
    /// The main user's SID (`S-1-5-21-...`), for the ACLs.
    pub main_user_sid: String,
    /// Every alt user AltDock manages after this run (`users.json`).
    pub managed_users: Vec<String>,
    /// Alt users to create or repair.
    pub users: Vec<PlanUser>,
    /// Alt users to delete (with their profile and control config).
    pub remove_users: Vec<String>,
    /// `.rdp` files to sign.
    pub rdp_files: Vec<PathBuf>,
    /// SHA-1 thumbprint of the signing certificate made before, reused while it is still there.
    pub thumbprint: Option<String>,
    /// Policies, program copy, autostart and certificate too (false: only remove users).
    pub system: bool,
}

/// One step of the elevated setup.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetupStep {
    pub id: String,
    pub ok: bool,
    pub detail: String,
}

/// What the elevated setup did (`<plan>.result.json`, and `sessions_setup`'s answer).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetupResult {
    pub steps: Vec<SetupStep>,
    /// SHA-1 thumbprint of the `.rdp` signing certificate, when there is one.
    pub thumbprint: Option<String>,
}

impl SetupResult {
    pub fn ok(&self) -> bool {
        self.steps.iter().all(|s| s.ok)
    }

    pub fn push(&mut self, id: impl Into<String>, outcome: std::result::Result<String, String>) {
        let (ok, detail) = match outcome {
            Ok(d) => (true, d),
            Err(d) => (false, d),
        };
        self.steps.push(SetupStep { id: id.into(), ok, detail });
    }
}
