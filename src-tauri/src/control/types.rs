//! What the control API (`control::server`) answers and takes; the main app's
//! `control::client` reads and sends the same types.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::model::{AccountStatus, AfkClientConfig, InstanceState, MacroRun};

/// What an alt session is for (its control config's `mode`).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum ControlMode {
    /// Several clients without macros; anti-AFK rotates the focus among them.
    Shared,
    /// One client, for a continuous macro.
    Dedicated,
}

/// `GET /v1/health`: the session's AltDock is up, and what it can do.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ControlHealth {
    /// AltDock's version in the session (the shared program copy).
    pub version: String,
    pub windows_user: String,
    /// The Windows session it runs in.
    pub session_id: u32,
    pub mode: ControlMode,
    pub vault_locked: bool,
    pub roblox_installed: bool,
    /// Roblox clients it follows that haven't exited.
    pub clients: u32,
}

/// `GET /v1/state`: one client the session follows (exited ones are left out).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ControlClient {
    pub instance_id: String,
    /// The session's own account id (its database's, not the main app's).
    pub account_id: i64,
    /// The Roblox user: what the main app matches its own accounts by.
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub state: InstanceState,
    /// The game's name once the session knows it; None before it has joined one.
    pub place_name: Option<String>,
    /// Unix ms.
    pub started_at: i64,
    /// Percent of the whole machine (None on the first sample).
    pub cpu_pct: Option<f64>,
    /// Private working set, MB (None until sampled).
    pub memory_mb: Option<f64>,
    pub afk: AfkClientConfig,
    pub macro_run: Option<MacroRun>,
    /// The macro assigned to this client's account (an id of the session's own macros).
    pub macro_assigned: Option<i64>,
    /// The assigned macro starts on its own when the client reaches a game (false without one).
    pub macro_autorun: bool,
}

/// `GET /v1/accounts`: one account stored in the session.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ControlAccount {
    /// The session's own account id.
    pub id: i64,
    /// The Roblox user: what the main app matches its own accounts by.
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub status: AccountStatus,
}

/// `POST /v1/accounts/import`: a `.roalt` backup (base64) and its password.
// No Debug: it would print the password.
#[derive(Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportRequest {
    pub backup: String,
    #[ts(type = "string")]
    pub password: Zeroizing<String>,
}

/// What `POST /v1/accounts/import` did: accounts added or updated, and ones it couldn't take
/// (a cookie that doesn't sign in as its user).
#[derive(Serialize, Deserialize, TS, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportAnswer {
    pub imported: u32,
    pub failed: u32,
}

/// `POST /v1/launch`: accounts launched as one batch to `target`, text as the Launch box takes
/// it (a game or server link, a place id, `@user` to follow).
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchRequest {
    pub account_ids: Vec<i64>,
    pub target: String,
}

/// The batch `POST /v1/launch` started and how many accounts it launches.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LaunchAnswer {
    pub started: u32,
    pub batch_id: String,
}

/// `POST /v1/close`: one client, gracefully unless `force`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CloseRequest {
    pub id: String,
    #[serde(default)]
    pub force: bool,
}

/// `POST /v1/afk`: one account's anti-AFK.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AfkRequest {
    pub account_id: i64,
    pub config: AfkClientConfig,
}

/// `POST /v1/macro/assign`: the account's macro (None: none) and whether it starts in game.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MacroAssignRequest {
    pub account_id: i64,
    pub macro_id: Option<i64>,
    pub autorun: bool,
}

/// `POST /v1/macro/run`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MacroRunRequest {
    pub macro_id: i64,
    pub instance_ids: Vec<String>,
}

/// `POST /v1/macro/stop`.
#[derive(Serialize, Deserialize, TS, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MacroStopRequest {
    pub instance_id: String,
}

/// Every refusal and failure: the session's `AppError` (`kind` as `ErrorPayload`'s), its
/// message in the session's language. Never holds a secret, like every `AppError`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ControlError {
    pub kind: String,
    pub message: String,
}

/// `GET /v1/hello`'s answer: `control::proof` of the challenge (not for the UI).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hello {
    pub proof: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AfkAction;

    #[test]
    fn bodies_are_camel_case_json() {
        let r: LaunchRequest = serde_json::from_str(r#"{"accountIds":[1,2],"target":"https://www.roblox.com/games/1"}"#).unwrap();
        assert_eq!(r, LaunchRequest { account_ids: vec![1, 2], target: "https://www.roblox.com/games/1".into() });
        assert!(serde_json::from_str::<LaunchRequest>(r#"{"accountIds":"1","target":"x"}"#).is_err());
        assert!(serde_json::from_str::<LaunchRequest>(r#"{"target":"x"}"#).is_err());

        // `force` may be left out.
        let c: CloseRequest = serde_json::from_str(r#"{"id":"abc"}"#).unwrap();
        assert_eq!(c, CloseRequest { id: "abc".into(), force: false });
        assert!(serde_json::from_str::<CloseRequest>(r#"{"id":"abc","force":true}"#).unwrap().force);

        let i: ImportRequest = serde_json::from_str(r#"{"backup":"Uk9BTFRCSzE=","password":"pw"}"#).unwrap();
        assert_eq!((i.backup.as_str(), i.password.as_str()), ("Uk9BTFRCSzE=", "pw"));

        let a: MacroAssignRequest = serde_json::from_str(r#"{"accountId":3,"macroId":null,"autorun":true}"#).unwrap();
        assert_eq!(a, MacroAssignRequest { account_id: 3, macro_id: None, autorun: true });
        let m: MacroRunRequest = serde_json::from_str(r#"{"macroId":4,"instanceIds":["a","b"]}"#).unwrap();
        assert_eq!(m.instance_ids, ["a", "b"]);
        let s: MacroStopRequest = serde_json::from_str(r#"{"instanceId":"a"}"#).unwrap();
        assert_eq!(s.instance_id, "a");
    }

    #[test]
    fn answers_round_trip() {
        let h = ControlHealth {
            version: "1.0.2".into(),
            windows_user: "altdock-1".into(),
            session_id: 3,
            mode: ControlMode::Dedicated,
            vault_locked: false,
            roblox_installed: true,
            clients: 2,
        };
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains(r#""windowsUser":"altdock-1""#) && json.contains(r#""mode":"dedicated""#), "{json}");
        assert_eq!(serde_json::from_str::<ControlHealth>(&json).unwrap(), h);

        let c = ControlClient {
            instance_id: "i".into(),
            account_id: 1,
            user_id: 77,
            username: "u".into(),
            display_name: "U".into(),
            state: InstanceState::InGame,
            place_name: Some("Adopt Me!".into()),
            started_at: 5,
            cpu_pct: None,
            memory_mb: Some(512.5),
            afk: AfkClientConfig { enabled: true, interval_sec: 600, action: AfkAction::Key("e".into()) },
            macro_run: Some(MacroRun { instance_id: "i".into(), account_id: 1, macro_id: 2, name: "Farm".into(), started_at: 6, restarts: 0 }),
            macro_assigned: Some(2),
            macro_autorun: true,
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""state":"inGame""#) && json.contains(r#""macroRun":{"instanceId":"i""#), "{json}");
        assert!(json.contains(r#""afk":{"enabled":true,"intervalSec":600,"action":{"key":"e"}}"#) && json.contains(r#""macroAutorun":true"#), "{json}");
        assert_eq!(serde_json::from_str::<ControlClient>(&json).unwrap(), c);

        let e: ControlError = serde_json::from_str(r#"{"kind":"locked","message":"The vault is locked."}"#).unwrap();
        assert_eq!(e.kind, "locked");
    }
}
