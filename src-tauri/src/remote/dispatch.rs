//! Strict allowlist command dispatcher for RoALT Remote Dashboard.
//!
//! Security invariants:
//! - Only commands explicitly listed in `RemoteCommand` can ever execute.
//! - Cookies, vault passwords, recovery codes, cookie exports, and account add/remove are never exposed.
//! - `RemoteRole::Readonly` devices are rejected before any mutating command runs.
//! - Vault lock state is enforced by the underlying `commands::*` functions and checked upfront.

use serde_json::{Value, json};
use tauri::{AppHandle, Manager};

use super::protocol::{
    Pages, RemoteAccount, RemoteBulk, RemoteCommand, RemoteEvent, RemoteGroup, RemoteInstance, RemotePage, RemoteSnapshot, accounts_page, first_page, page_of,
};
use crate::App;
use crate::commands;
use crate::error::AppError;
use crate::instances::window::Show;
use crate::model::{BulkProgress, BulkRequest, ClientMetrics, InstanceState, RemoteRole, SameServer};

fn accounts_first_page(app: &AppHandle, more: &mut Pages) -> Vec<RemoteAccount> {
    let accounts = app.state::<App>().db.accounts().unwrap_or_default().iter().map(RemoteAccount::from).collect();
    first_page(accounts, more, accounts_page)
}

fn groups_first_page(app: &AppHandle, more: &mut Pages) -> Vec<RemoteGroup> {
    first_page(app.state::<App>().db.groups().unwrap_or_default().iter().map(Into::into).collect(), more, page_of(RemotePage::Groups))
}

fn instances_first_page(app: &AppHandle, more: &mut Pages) -> Vec<RemoteInstance> {
    first_page(app.state::<App>().tracker.list().iter().map(Into::into).collect(), more, page_of(RemotePage::Instances))
}

fn metrics_first_page(app: &AppHandle, more: &mut Pages) -> Vec<ClientMetrics> {
    let s = app.state::<App>();
    first_page(s.metrics.sample(&s.tracker.clients()), more, page_of(RemotePage::Metrics))
}

fn bulk_first_page(progress: &BulkProgress, more: &mut Pages) -> RemoteBulk {
    let mut bulk = RemoteBulk::from(progress);
    bulk.items = first_page(std::mem::take(&mut bulk.items), more, page_of(RemotePage::BulkItems));
    bulk
}

/// A full snapshot of RoALT's state for an authenticated remote viewer, then the rest of each of
/// its lists. Only what the dashboard shows: never cookies, vault secrets, notes or link codes.
pub fn build_snapshot(app: &AppHandle, pc_name: &str, role: RemoteRole) -> Vec<RemoteEvent> {
    let s = app.state::<App>();
    let vault_locked = s.vault.is_locked();
    let windows_locked = crate::session::locked();
    let mut more = Pages::default();
    let accounts = accounts_first_page(app, &mut more);
    let groups = groups_first_page(app, &mut more);
    // Favourites contain encrypted link targets: empty while vault is locked.
    let favourites = if vault_locked {
        Vec::new()
    } else {
        commands::favourites_list(app.state::<App>()).unwrap_or_default().iter().map(Into::into).collect()
    };
    let favourites = first_page(favourites, &mut more, page_of(RemotePage::Favourites));
    let instances = instances_first_page(app, &mut more);
    let metrics = metrics_first_page(app, &mut more);
    let system_usage = s.metrics.system();
    let bulk_progress = s.bulk.current().map(|p| bulk_first_page(&p, &mut more));
    let shutdown_at = *s.shutdown.lock().unwrap_or_else(|e| e.into_inner());
    let crashes = s.db.crashes().unwrap_or_default().iter().take(40).map(Into::into).collect();
    let crashes = first_page(crashes, &mut more, page_of(RemotePage::Crashes));

    let data = RemoteSnapshot {
        pc_name: pc_name.to_owned(),
        app_version: env!("CARGO_PKG_VERSION").to_owned(),
        role,
        vault_locked,
        windows_locked,
        accounts,
        groups,
        favourites,
        instances,
        metrics,
        system_usage,
        bulk_progress,
        shutdown_at,
        crashes,
    };
    more.after(|batch| RemoteEvent::Snapshot { batch, data })
}

/// `AccountsUpdated` with the first pages of accounts and groups, then the rest.
pub fn accounts_events(app: &AppHandle) -> Vec<RemoteEvent> {
    let mut more = Pages::default();
    let accounts = accounts_first_page(app, &mut more);
    let groups = groups_first_page(app, &mut more);
    more.after(|batch| RemoteEvent::AccountsUpdated { batch, accounts, groups })
}

/// `InstancesUpdated` with the first page of clients, then the rest.
pub fn instances_events(app: &AppHandle) -> Vec<RemoteEvent> {
    let mut more = Pages::default();
    let instances = instances_first_page(app, &mut more);
    more.after(|batch| RemoteEvent::InstancesUpdated { batch, instances })
}

/// `MetricsUpdated` with the first page of client metrics, then the rest.
pub fn metrics_events(app: &AppHandle) -> Vec<RemoteEvent> {
    let s = app.state::<App>();
    let mut more = Pages::default();
    let metrics = metrics_first_page(app, &mut more);
    let (system_usage, windows_locked, vault_locked) = (s.metrics.system(), crate::session::locked(), s.vault.is_locked());
    let shutdown_at = *s.shutdown.lock().unwrap_or_else(|e| e.into_inner());
    more.after(|batch| RemoteEvent::MetricsUpdated { batch, metrics, system_usage, windows_locked, vault_locked, shutdown_at })
}

/// `BulkProgress` with the first page of its items, then the rest.
pub fn bulk_events(progress: &BulkProgress) -> Vec<RemoteEvent> {
    let mut more = Pages::default();
    let progress = bulk_first_page(progress, &mut more);
    more.after(|batch| RemoteEvent::BulkProgress { batch, progress })
}

fn ok_res(req_id: u32, data: Value) -> RemoteEvent {
    RemoteEvent::RpcResult { req_id, ok: true, data: Some(data), error: None, code: None }
}

fn err_res(req_id: u32, code: &str, message: impl Into<String>) -> RemoteEvent {
    RemoteEvent::RpcResult { req_id, ok: false, data: None, error: Some(message.into()), code: Some(code.to_owned()) }
}

fn from_app_err(req_id: u32, e: AppError) -> RemoteEvent {
    let code = match &e {
        AppError::Locked => "vault_locked",
        AppError::NotFound => "not_found",
        AppError::Invalid(_) => "invalid",
        _ => "failed",
    };
    err_res(req_id, code, e.message())
}

fn parse_show(raw: &str) -> Option<Show> {
    match raw {
        "focus" => Some(Show::Focus),
        "minimize" => Some(Show::Minimize),
        "restore" => Some(Show::Restore),
        _ => None,
    }
}

/// Validates role/vault permissions and executes one allowlisted `RemoteCommand`.
pub async fn execute(
    app: &AppHandle,
    device_label: &str,
    role: RemoteRole,
    req_id: u32,
    cmd: RemoteCommand,
) -> RemoteEvent {
    if role == RemoteRole::Readonly && !cmd.is_readonly() {
        return err_res(req_id, "readonly", "This paired device has view-only permissions.");
    }

    match cmd {
        RemoteCommand::Ping => ok_res(req_id, json!({ "pong": true })),
        RemoteCommand::RequestSnapshot | RemoteCommand::SubscribePreviews { .. } | RemoteCommand::PreviewAck { .. } => {
            // Handled directly by the session loop in `remote::mod`.
            ok_res(req_id, json!({ "ok": true }))
        }
        RemoteCommand::TargetParse { input } => {
            let parsed = commands::target_parse(input);
            ok_res(req_id, json!({ "parsed": parsed }))
        }
        RemoteCommand::Launch { account_id, target, fps_cap } => {
            log::info!("remote ({device_label}): launch account {account_id}");
            match commands::launch(app.clone(), app.state::<App>(), account_id, target, fps_cap).await {
                Ok(info) => ok_res(req_id, json!({ "launch": info })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::BulkLaunch { request } => {
            log::info!("remote ({device_label}): bulk launch {} accounts", request.account_ids.len());
            match commands::bulk_launch(app.clone(), request) {
                Ok(batch_id) => ok_res(req_id, json!({ "batchId": batch_id })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::LaunchFavourite { favourite_id, account_ids, fps_cap } => {
            log::info!("remote ({device_label}): launch favourite {favourite_id}");
            match launch_favourite(app, favourite_id, account_ids, fps_cap).await {
                Ok(data) => ok_res(req_id, data),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::BulkCancel { batch_id } => {
            log::info!("remote ({device_label}): cancel bulk {batch_id}");
            match commands::bulk_cancel(app.state::<App>(), batch_id) {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceClose { id, force } => {
            log::info!("remote ({device_label}): close instance {id} (force={force})");
            match commands::instance_close(app.state::<App>(), id, force).await {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstancesCloseAll => {
            log::info!("remote ({device_label}): close all instances");
            match commands::instances_close_all(app.state::<App>()).await {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceMove { id, target, fps_cap } => {
            log::info!("remote ({device_label}): move/rejoin instance {id}");
            match commands::instance_move(app.clone(), app.state::<App>(), id, target, fps_cap).await {
                Ok(info) => ok_res(req_id, json!({ "launch": info })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceRejoin { id } => {
            log::info!("remote ({device_label}): rejoin instance {id}");
            let Some(inst) = app.state::<App>().tracker.get(&id) else {
                return err_res(req_id, "not_found", "That client is gone.");
            };
            let launched = if inst.state == InstanceState::Exited {
                commands::launch(app.clone(), app.state::<App>(), inst.account_id, inst.target, inst.fps_cap).await
            } else {
                commands::instance_move(app.clone(), app.state::<App>(), id, inst.target, None).await
            };
            match launched {
                Ok(info) => ok_res(req_id, json!({ "launch": info })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceSetAntiAfk { id, enabled } => {
            match commands::instance_set_anti_afk(app.clone(), app.state::<App>(), id, enabled) {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceSetLimits { id, limits } => {
            match commands::instance_set_limits(app.clone(), app.state::<App>(), id, limits).await {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceTrim { id } => {
            match commands::instance_trim(app.clone(), app.state::<App>(), id).await {
                Ok((before, after)) => ok_res(req_id, json!({ "beforeMb": before, "afterMb": after })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceShow { id, what } => {
            let Some(show) = parse_show(&what) else {
                return err_res(req_id, "invalid", "Invalid window action.");
            };
            match commands::instance_show(app.state::<App>(), id, show).await {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstancesShowAll { what } => {
            let Some(show) = parse_show(&what) else {
                return err_res(req_id, "invalid", "Invalid window action.");
            };
            match commands::instances_show_all(app.state::<App>(), show).await {
                Ok(()) => ok_res(req_id, json!({ "ok": true })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceMicro { id, on } => {
            match commands::instance_micro(app.clone(), app.state::<App>(), id, on, None).await {
                Ok((w, h)) => ok_res(req_id, json!({ "width": w, "height": h })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstancesTile { micro } => {
            match commands::instances_tile(app.clone(), app.state::<App>(), micro).await {
                Ok(n) => ok_res(req_id, json!({ "tiled": n })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InstanceStopRelaunch { id } => {
            commands::instance_stop_relaunch(app.clone(), app.state::<App>(), id);
            ok_res(req_id, json!({ "ok": true }))
        }
        RemoteCommand::InstancesRetryRelaunch => {
            commands::instances_retry_relaunch();
            ok_res(req_id, json!({ "ok": true }))
        }
        RemoteCommand::PanicKill { confirmed } => {
            if !confirmed {
                return err_res(req_id, "invalid", "Panic kill requires explicit confirmation.");
            }
            log::warn!("remote ({device_label}): PANIC KILL triggered");
            match commands::panic_kill(app.state::<App>()).await {
                Ok(status) => ok_res(req_id, json!({ "multi": status })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::ShutdownSchedule { in_secs, close_clients } => {
            log::warn!("remote ({device_label}): schedule PC shutdown in {in_secs}s");
            match commands::shutdown_schedule(app.clone(), app.state::<App>(), None, Some(in_secs), close_clients) {
                Ok(at) => ok_res(req_id, json!({ "shutdownAt": at })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::ShutdownCancel => {
            log::info!("remote ({device_label}): cancel PC shutdown");
            match commands::shutdown_cancel(app.state::<App>()) {
                Ok(_) => ok_res(req_id, json!({ "shutdownAt": Value::Null })),
                Err(e) => from_app_err(req_id, e),
            }
        }
        RemoteCommand::InputTouch { .. } | RemoteCommand::InputKey { .. } | RemoteCommand::InputCamera { .. } => input(app, uuid::Uuid::nil(), uuid::Uuid::nil(), role, req_id, cmd),
        RemoteCommand::WebRtcSignal { .. } => {
            err_res(req_id, "not_implemented", "Live WebRTC interactive control is planned for Phase 2.")
        }
    }
}

async fn launch_favourite(app: &AppHandle, favourite_id: i64, account_ids: Vec<i64>, fps_cap: Option<u32>) -> Result<Value, AppError> {
    let s = app.state::<App>();
    let row = s.db.favourites()?.into_iter().find(|f| f.id == favourite_id).ok_or(AppError::NotFound)?;
    let fav = commands::favourite_of(&s, row)?;
    let ids = if account_ids.is_empty() { commands::favourite_accounts(&s, &fav)? } else { account_ids };
    let fps_cap = fps_cap.or(fav.options.fps_cap);
    match ids.as_slice() {
        [] => Err(AppError::Invalid("This favourite has no accounts. Select accounts first.".into())),
        [one] => Ok(json!({ "launch": commands::launch(app.clone(), app.state::<App>(), *one, fav.target, fps_cap).await? })),
        _ => {
            let request = BulkRequest {
                account_ids: ids,
                target: fav.target,
                delay_ms: s.db.settings()?.launch_delay_ms,
                same_server: fav.options.same_server.unwrap_or(SameServer::Off),
                fps_cap,
            };
            Ok(json!({ "batchId": commands::bulk_launch(app.clone(), request)? }))
        }
    }
}

pub fn is_release(cmd: &RemoteCommand) -> bool {
    match cmd {
        RemoteCommand::InputKey { down, .. } => !down,
        RemoteCommand::InputTouch { what, .. } | RemoteCommand::InputCamera { what, .. } => what == "up",
        _ => false,
    }
}

/// A finger into one client from session `owner` of paired `device`. Run straight from the session loop, not spawned:
/// the input worker gets events in the order they came, so a lift never overtakes its touch. A
/// release passes even while Windows is locked (on its own budget, `DeviceBudget::releases`), and
/// only lets go of what that session holds; a view-only session holds nothing, so its releases do
/// nothing.
pub fn input(app: &AppHandle, owner: uuid::Uuid, device: uuid::Uuid, role: RemoteRole, req_id: u32, cmd: RemoteCommand) -> RemoteEvent {
    use crate::instances::remote_input::{self as ri, Phase};
    let release = is_release(&cmd);
    if release {
        if role == RemoteRole::Readonly {
            return ok_res(req_id, json!({ "ok": true }));
        }
    } else {
        if role == RemoteRole::Readonly {
            return err_res(req_id, "readonly", "This paired device has view-only permissions.");
        }
        if crate::session::locked() {
            return err_res(req_id, "windows_locked", "Windows is locked on the PC: it takes no input until it is unlocked.");
        }
    }
    if let RemoteCommand::InputKey { id, code, down, repeat } = cmd {
        let Some(key) = ri::MoveKey::parse(&code) else { return err_res(req_id, "invalid", "Only the movement keys can be held.") };
        let Some(client) = input_client(&app.state::<App>().tracker, &id, release) else {
            return err_res(req_id, "not_found", "That client has no window.");
        };
        return queued(req_id, ri::key(owner, device, client, key, down, repeat));
    }
    let (id, touch, x, y, what) = match cmd {
        RemoteCommand::InputTouch { id, touch, x, y, what } => (id, Some(touch), x, y, what),
        RemoteCommand::InputCamera { id, x, y, what } => (id, None, x, y, what),
        _ => return err_res(req_id, "invalid", "Not an input command."),
    };
    let phase = match what.as_str() {
        "down" => Phase::Down,
        "move" => Phase::Move,
        "up" => Phase::Up,
        _ => return err_res(req_id, "invalid", "Invalid touch action."),
    };
    if !x.is_finite() || !y.is_finite() {
        return err_res(req_id, "invalid", "Invalid touch position.");
    }
    let Some(client) = input_client(&app.state::<App>().tracker, &id, release) else {
        return err_res(req_id, "not_found", "That client has no window.");
    };
    let sent = match touch {
        Some(finger) => ri::touch(owner, device, client, finger, x, y, phase),
        None => ri::camera_drag(owner, device, client, x, y, phase),
    };
    queued(req_id, sent)
}

/// The client a press goes to; a lift goes through even once the tracker lost the client's window.
fn input_client(tracker: &crate::instances::Tracker, id: &str, release: bool) -> Option<crate::instances::remote_input::Client> {
    if release { Some(crate::instances::remote_input::Client::NONE) } else { tracker.window_client(id) }
}

fn queued(req_id: u32, sent: bool) -> RemoteEvent {
    if sent {
        ok_res(req_id, json!({ "ok": true }))
    } else {
        err_res(req_id, "busy", "The PC is still working through your earlier input. Try again.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readonly_classification_blocks_all_mutations() {
        assert!(RemoteCommand::Ping.is_readonly());
        assert!(RemoteCommand::RequestSnapshot.is_readonly());
        assert!(RemoteCommand::SubscribePreviews { targets: vec![] }.is_readonly());
        assert!(RemoteCommand::PreviewAck { instance_id: String::new(), seq: 0 }.is_readonly());
        assert!(RemoteCommand::TargetParse { input: "123".into() }.is_readonly());

        assert!(!RemoteCommand::InstancesCloseAll.is_readonly());
        assert!(!RemoteCommand::PanicKill { confirmed: true }.is_readonly());
        assert!(!RemoteCommand::ShutdownCancel.is_readonly());
        assert!(!RemoteCommand::InstanceClose { id: "a".into(), force: false }.is_readonly());
        assert!(!RemoteCommand::LaunchFavourite { favourite_id: 1, account_ids: vec![], fps_cap: None }.is_readonly());
        assert!(!RemoteCommand::InstanceRejoin { id: "a".into() }.is_readonly());
    }

    #[test]
    fn only_lifts_count_as_releases() {
        let key = |down| RemoteCommand::InputKey { id: "a".into(), code: "KeyW".into(), down, repeat: false };
        let touch = |what: &str| RemoteCommand::InputTouch { id: "a".into(), touch: 1, x: 0.5, y: 0.5, what: what.into() };
        let camera = |what: &str| RemoteCommand::InputCamera { id: "a".into(), x: 0.5, y: 0.5, what: what.into() };
        assert!(is_release(&key(false)) && is_release(&touch("up")) && is_release(&camera("up")));
        assert!(!is_release(&key(true)) && !is_release(&touch("down")) && !is_release(&camera("move")));
        assert!(!is_release(&RemoteCommand::InstancesCloseAll));
    }

    #[test]
    fn a_lift_goes_through_for_a_client_the_tracker_no_longer_has() {
        let tracker = crate::instances::Tracker::new();
        assert_eq!(input_client(&tracker, "gone", true), Some(crate::instances::remote_input::Client::NONE));
        assert_eq!(input_client(&tracker, "gone", false), None);
    }
}
