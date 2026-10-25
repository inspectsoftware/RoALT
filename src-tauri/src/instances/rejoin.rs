//! Auto-rejoin: the tracker loop hands over clients that ended unexpectedly
//! (`Tracker::take_rejoins`); with `settings.auto_rejoin` on, each is launched again into the
//! same target after the launch delay (`launcher::bulk::jittered(settings.launch_delay_ms)`).

use tauri::{AppHandle, Manager};

use super::tracker::{MAX_REJOINS, Rejoin};
use crate::App;
use crate::commands::{self, notice};
use crate::launcher::bulk::{jittered, name_of};
use crate::model::{AccountStatus, AccountSummary, Instance, InstanceState};

/// Rejoins take turns: launches go one at a time (DESIGN §6), each after its own delay.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Called by the tracker loop after each poll with what `take_rejoins` returned. When
/// auto-rejoin is off, does nothing. Otherwise, per rejoin, on the async runtime: wait the
/// launch delay; then skip it (logged, plus an `app://notice`) when auto-rejoin was turned off
/// meanwhile, the vault is locked, a batch is running, the account is gone, is no longer the
/// same user or its cookie isn't valid, or the account already has a client starting, joining
/// or in game; else `commands::launch_tracked(account, target, no favourite)` and
/// `tracker.set_rejoins(new id, r.rejoins)`. A failed launch is an `app://notice` (error) and
/// is not retried here (its client never started, so it can't end unexpectedly).
pub fn handle(app: &AppHandle, rejoins: Vec<Rejoin>) {
    let s = app.state::<App>();
    if rejoins.is_empty() || !s.db.settings().is_ok_and(|v| v.auto_rejoin) {
        return;
    }
    for r in rejoins {
        // Who the account is now, so an id removed and reused during the wait is told apart.
        let user_id = s.db.account(r.account_id).ok().map(|a| a.user_id);
        let app = app.clone();
        tauri::async_runtime::spawn(async move { rejoin(&app, r, user_id).await });
    }
}

async fn rejoin(app: &AppHandle, r: Rejoin, user_id: Option<i64>) {
    let _turn = TURN.lock().await;
    let s = app.state::<App>();
    tokio::time::sleep(jittered(s.db.settings().map_or(6_000, |v| v.launch_delay_ms))).await;
    let account = s.db.account(r.account_id).ok().filter(|a| Some(a.user_id) == user_id);
    let name = account.as_ref().map_or_else(|| "An account".to_owned(), name_of);
    if let Some(why) = skip_reason(&s, &r, account.as_ref()) {
        log::info!("auto-rejoin of instance {} skipped: {why}", r.from);
        notice(app, false, format!("{name} wasn't rejoined: {why}."));
        return;
    }
    match commands::launch_tracked(app, &s, r.account_id, r.target, None).await {
        Ok((_, inst)) => {
            s.tracker.set_rejoins(&inst.id, r.rejoins);
            commands::instances_changed(app);
            log::info!("auto-rejoin {}/{MAX_REJOINS}: instance {} relaunched as {}", r.rejoins, r.from, inst.id);
        }
        Err(e) => {
            log::warn!("auto-rejoin of instance {} failed: {e}", r.from);
            notice(app, true, format!("{name} couldn't rejoin: {e}"));
        }
    }
}

/// Why the rejoin can't go ahead now (None: launch it). `account` is None when it was removed
/// or its id now belongs to another user.
fn skip_reason(s: &App, r: &Rejoin, account: Option<&AccountSummary>) -> Option<&'static str> {
    let busy = |i: &Instance| {
        i.account_id == r.account_id && matches!(i.state, InstanceState::Starting | InstanceState::Joining | InstanceState::InGame)
    };
    if !s.db.settings().is_ok_and(|v| v.auto_rejoin) {
        Some("auto-rejoin was turned off")
    } else if s.vault.is_locked() {
        Some("the vault is locked")
    } else if s.bulk.running() {
        Some("a batch is running")
    } else if account.is_none() {
        Some("it was removed")
    } else if account.is_some_and(|a| a.status != AccountStatus::Valid) {
        Some("its cookie isn't valid. Sign in to it again")
    } else if s.tracker.list().iter().any(busy) {
        Some("it already has a client running")
    } else if !s.multi.is_active() && s.tracker.any_running() {
        // Without the multi-instance lock the new client would replace the running one.
        Some("another client is running and multi-instance is off")
    } else {
        None
    }
}
