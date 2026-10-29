//! Auto-rejoin and auto-restart: the tracker loop hands over clients Roblox disconnected, that
//! crashed, hung or stayed over the memory limit (`Tracker::take_rejoins`). With
//! `settings.auto_rejoin` (disconnects), `settings.auto_restart_on_crash` (crashes, hangs) or
//! `settings.mem_restart_mb` (memory) on, each still open is closed and the account launched again into the same target after a backoff
//! (`launcher::bulk::jittered(backoff(settings.launch_delay_ms, recent))`), within the crash
//! breaker: at most `settings.breaker_max` relaunches of one account per
//! `settings.breaker_window_min` minutes.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};

use super::tracker::Rejoin;
use crate::App;
use crate::commands::{self, notice};
use crate::launcher::bulk::{jittered, name_of};
use crate::model::{AccountStatus, AccountSummary, CrashKind, Instance, InstanceState, Settings};

/// Rejoins take turns: launches go one at a time (DESIGN §6), each after its own delay.
static TURN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// The crash breaker's memory: when each account was last relaunched, oldest first. Changed
/// only while holding `TURN`.
static RECENT: Mutex<Vec<(i64, VecDeque<Instant>)>> = Mutex::new(Vec::new());

/// Called by the tracker loop after each poll with what `take_rejoins` returned. Drops those
/// whose setting is off (`enabled`) or whose account allows disconnects, silently. Otherwise,
/// per rejoin, on the async runtime: wait the backoff; drop it silently when it is no longer a
/// rejoin case (`still_wants_rejoin`: back in a game, responding again, closed by the user,
/// cleared); mark it in flight (`Bulk::begin_rejoin`); skip it (logged, plus an `app://notice`)
/// when its setting was turned off meanwhile, the vault is locked, a batch is running, the
/// account is gone, allows disconnects, is no longer the same user or its cookie isn't valid,
/// or the account has another client starting, joining or in game; give up (note "Gave up
/// after N ...", error notice) when the breaker trips; else close the old client if it still
/// runs (graceful for `close_grace_s`, then forced; marked as closed by RoALT), then
/// `commands::launch_tracked(account, target, no favourite, the old client's FPS cap)` and
/// `tracker.set_rejoins(new id, r.rejoins)`. A failed launch is an `app://notice` (error) and
/// is not retried here (its client never started, so it can't end unexpectedly).
pub fn handle(app: &AppHandle, rejoins: Vec<Rejoin>) {
    if rejoins.is_empty() {
        return;
    }
    let s = app.state::<App>();
    let v = match s.db.settings() {
        Ok(v) => v,
        Err(e) => {
            log::error!("auto-rejoin/restart: could not read settings, {} pending dropped: {e}", rejoins.len());
            return;
        }
    };
    for r in rejoins.into_iter().filter(|r| enabled(&v, r.cause)) {
        // Who the account is now, so an id removed and reused during the wait is told apart.
        let account = s
            .db
            .account(r.account_id)
            .inspect_err(|e| {
                if !matches!(e, crate::error::AppError::NotFound) {
                    log::warn!("auto-rejoin: could not read account {}: {e}", r.account_id);
                }
            })
            .ok();
        if account.as_ref().is_some_and(|a| a.allow_disconnect) {
            continue;
        }
        let user_id = account.map(|a| a.user_id);
        let app = app.clone();
        tauri::async_runtime::spawn(async move { rejoin(&app, r, user_id).await });
    }
}

/// Disconnects follow auto-rejoin; crashes and hangs auto-restart; memory the memory limit.
fn enabled(v: &Settings, cause: CrashKind) -> bool {
    match cause {
        CrashKind::Disconnect => v.auto_rejoin,
        CrashKind::Crash | CrashKind::Hang => v.auto_restart_on_crash,
        CrashKind::Mem => v.mem_restart_mb > 0,
    }
}

async fn rejoin(app: &AppHandle, r: Rejoin, user_id: Option<i64>) {
    let s = app.state::<App>();
    let v = s.db.settings().unwrap_or_else(|e| {
        log::warn!("auto-rejoin of instance {}: could not read settings, using defaults: {e}", r.from);
        Settings::default()
    });
    let window = Duration::from_secs(u64::from(v.breaker_window_min) * 60);
    let n = with_recent(r.account_id, |t| recent(t, Instant::now(), window));
    // The backoff is waited before taking a turn, so one account's doesn't hold up the rest.
    tokio::time::sleep(jittered(backoff(v.launch_delay_ms, n))).await;
    let _turn = TURN.lock().await;
    // Until the launch is done, batches and single launches wait (DESIGN §6: one at a time).
    // A Roblox install waits out first: after `busy` is set, one starting now sees this rejoin.
    let wait_until = Instant::now() + crate::launcher::update::INSTALL_TIMEOUT;
    let busy = loop {
        let busy = s.bulk.begin_rejoin();
        if busy.is_none() || !s.update.installing() || Instant::now() > wait_until {
            break busy;
        }
        drop(busy);
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    // Back in a game (the player pressed Reconnect), responding again, closed by the user or
    // cleared: nothing to do.
    if !s.tracker.still_wants_rejoin(&r.from) {
        return;
    }
    let account = s
        .db
        .account(r.account_id)
        .inspect_err(|e| {
            if !matches!(e, crate::error::AppError::NotFound) {
                log::warn!("auto-rejoin: could not read account {}: {e}", r.account_id);
            }
        })
        .ok()
        .filter(|a| Some(a.user_id) == user_id);
    let name = account.as_ref().map_or_else(|| tr!("An account"), name_of);
    let rejoin = r.cause == CrashKind::Disconnect;
    let again = if rejoin { "rejoins" } else { "restarts" };
    let kills = s.bulk.kills();
    if let Some(why) = skip_reason(&s, &r, account.as_ref(), busy.is_none()) {
        log::info!("auto-{again} of instance {} skipped: {why}", r.from);
        let text = if rejoin {
            tr!("{name} wasn't rejoined: {why}.", name = name, why = why)
        } else {
            tr!("{name} wasn't restarted: {why}.", name = name, why = why)
        };
        notice(app, false, text);
        return;
    }
    let n = with_recent(r.account_id, |t| recent(t, Instant::now(), window));
    if n >= v.breaker_max {
        log::info!("instance {}: crash breaker tripped ({n} relaunches in {} min)", r.from, v.breaker_window_min);
        let (note, text) = if rejoin {
            (
                trn!(n, "Gave up after {n} rejoin", "Gave up after {n} rejoins"),
                trn!(
                    n,
                    "{name} wasn't rejoined: it needed {n} rejoin in {minutes} minutes, so RoALT gave up.",
                    "{name} wasn't rejoined: it needed {n} rejoins in {minutes} minutes, so RoALT gave up.",
                    name = name,
                    minutes = v.breaker_window_min
                ),
            )
        } else {
            (
                trn!(n, "Gave up after {n} restart", "Gave up after {n} restarts"),
                trn!(
                    n,
                    "{name} wasn't restarted: it needed {n} restart in {minutes} minutes, so RoALT gave up.",
                    "{name} wasn't restarted: it needed {n} restarts in {minutes} minutes, so RoALT gave up.",
                    name = name,
                    minutes = v.breaker_window_min
                ),
            )
        };
        s.tracker.give_up(&r.from, note);
        commands::instances_changed(app);
        notice(app, true, text);
        if !account.as_ref().is_some_and(|a| a.discord_muted) {
            let name = crate::discord::name_of(account.as_ref(), r.account_id);
            crate::discord::notify(app, crate::discord::Event::Breaker { name, count: n, window_min: v.breaker_window_min });
        }
        return;
    }
    with_recent(r.account_id, |t| t.push_back(Instant::now()));
    // The client stuck on its disconnect dialog or hung; its exit is ours, not unexpected.
    if let Some((pid, started)) = s.tracker.mark_restart(&r.from, Some(r.cause)) {
        let grace = Duration::from_secs(u64::from(v.close_grace_s));
        let closed =
            tauri::async_runtime::spawn_blocking(move || crate::instances::process::close(pid, started, false, grace)).await;
        if let Ok(Err(e)) | Err(e) = closed.map_err(|e| crate::error::AppError::Internal(e.to_string())) {
            log::warn!("auto-{again}: closing instance {} failed: {e}", r.from);
            s.tracker.clear_closing(&r.from);
            let text = if rejoin {
                tr!("{name} couldn't be rejoined: its old client didn't close.", name = name)
            } else {
                tr!("{name} couldn't be restarted: its old client didn't close.", name = name)
            };
            notice(app, true, text);
            return;
        }
        // A hung client is still live in the tracker until the next poll ends it; launching
        // before that would be refused as "already running".
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.tracker.get(&r.from).is_some_and(|i| i.state != InstanceState::Exited) && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if s.tracker.get(&r.from).is_some_and(|i| i.state != InstanceState::Exited) {
            log::debug!("auto-{again}: instance {} still not exited after 5 s; launching anyway", r.from);
        }
    }
    // Kill all pressed while the old client was closing.
    if s.bulk.kills() != kills {
        log::debug!("auto-{again} of instance {} dropped: Kill all was pressed", r.from);
        return;
    }
    // The launch's own FPS cap carries over.
    let fps_cap = s.tracker.get(&r.from).and_then(|i| i.fps_cap);
    match commands::launch_tracked(app, &s, r.account_id, r.target, None, None, fps_cap).await {
        Ok((_, inst)) => {
            s.tracker.set_rejoins(&inst.id, r.rejoins);
            commands::instances_changed(app);
            log::info!("auto-{again} ({}/{} in the window): instance {} relaunched as {}", n + 1, v.breaker_max, r.from, inst.id);
            if !account.as_ref().is_some_and(|a| a.discord_muted) {
                let name = crate::discord::name_of(account.as_ref(), r.account_id);
                crate::discord::notify(app, crate::discord::Event::Restart { name, cause: r.cause });
            }
        }
        Err(e) => {
            log::warn!("auto-{again} of instance {} failed: {e}", r.from);
            let text = if rejoin {
                tr!("{name} couldn't be rejoined: {why}", name = name, why = e.message())
            } else {
                tr!("{name} couldn't be restarted: {why}", name = name, why = e.message())
            };
            notice(app, true, text);
        }
    }
}

/// Runs `f` on the account's relaunch times.
fn with_recent<T>(account_id: i64, f: impl FnOnce(&mut VecDeque<Instant>) -> T) -> T {
    let mut all = RECENT.lock().unwrap_or_else(|e| e.into_inner());
    let i = match all.iter().position(|(a, _)| *a == account_id) {
        Some(i) => i,
        None => {
            all.push((account_id, VecDeque::new()));
            all.len() - 1
        }
    };
    f(&mut all[i].1)
}

/// Forgets relaunches older than `window`; how many are left (pure).
fn recent(times: &mut VecDeque<Instant>, now: Instant, window: Duration) -> u32 {
    while times.front().is_some_and(|&t| now.saturating_duration_since(t) >= window) {
        times.pop_front();
    }
    times.len() as u32
}

/// The wait before a relaunch: the launch delay doubled for each recent relaunch of the
/// account (`jittered` then caps it at 60 s).
fn backoff(delay_ms: u32, recent: u32) -> u32 {
    delay_ms.saturating_mul(1 << recent.min(16))
}

/// Why the rejoin can't go ahead now (None: launch it). `account` is None when it was removed
/// or its id now belongs to another user; `batch`: a batch is running.
fn skip_reason(s: &App, r: &Rejoin, account: Option<&AccountSummary>, batch: bool) -> Option<String> {
    let busy = |i: &Instance| {
        i.id != r.from
            && i.account_id == r.account_id
            && matches!(i.state, InstanceState::Starting | InstanceState::Joining | InstanceState::InGame)
    };
    let others = |i: &Instance| i.id != r.from && i.state != InstanceState::Exited;
    if !s.db.settings().inspect_err(|e| log::warn!("auto-rejoin of instance {}: could not read settings: {e}", r.from)).is_ok_and(|v| enabled(&v, r.cause)) {
        Some(match r.cause {
            CrashKind::Disconnect => tr!("auto-rejoin was turned off"),
            CrashKind::Mem => tr!("the memory restart was turned off"),
            _ => tr!("auto-restart was turned off"),
        })
    } else if s.vault.is_locked() {
        Some(tr!("the vault is locked"))
    } else if batch {
        Some(tr!("a batch is running"))
    } else if account.is_none() {
        Some(tr!("it was removed"))
    } else if account.is_some_and(|a| a.allow_disconnect) {
        Some(tr!("it is set to allow disconnects"))
    } else if account.is_some_and(|a| a.status != AccountStatus::Valid) {
        Some(tr!("its cookie isn't valid. Sign in to it again"))
    } else if s.tracker.list().iter().any(busy) {
        Some(tr!("it already has a client running"))
    } else if !s.multi.is_active() && s.tracker.list().iter().any(others) {
        // Without the multi-instance lock the new client would replace the running one.
        Some(tr!("another client is running and multi-instance is off"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_breaker_counts_relaunches_within_the_window() {
        let t0 = Instant::now();
        let min = Duration::from_secs(60);
        let window = 10 * min;
        let mut times = VecDeque::new();
        assert_eq!(recent(&mut times, t0, window), 0);
        for m in [0, 4, 8] {
            times.push_back(t0 + m * min);
        }
        assert_eq!(recent(&mut times, t0 + 9 * min, window), 3, "3 in 10 minutes: a breaker of 3 trips");
        // The first one leaves the window at 10 minutes, the next at 14.
        assert_eq!(recent(&mut times, t0 + 10 * min, window), 2);
        assert_eq!(recent(&mut times, t0 + 14 * min, window), 1);
        assert_eq!(times.front(), Some(&(t0 + 8 * min)));
        assert_eq!(recent(&mut times, t0 + 60 * min, window), 0);
        assert!(times.is_empty());
    }

    #[test]
    fn the_backoff_doubles_and_is_capped() {
        assert_eq!(backoff(6_000, 0), 6_000);
        assert_eq!(backoff(6_000, 1), 12_000);
        assert_eq!(backoff(6_000, 3), 48_000);
        assert_eq!(backoff(6_000, 40), backoff(6_000, 16));
        assert_eq!(backoff(u32::MAX, 1), u32::MAX, "saturates");
        // jittered caps it at a minute, ± 1 s.
        assert!((59_000..=61_000).contains(&jittered(backoff(6_000, 5)).as_millis()));
    }

    #[test]
    fn disconnects_follow_auto_rejoin_and_the_rest_auto_restart() {
        let v = |auto_rejoin, auto_restart_on_crash| Settings { auto_rejoin, auto_restart_on_crash, ..Settings::default() };
        assert!(enabled(&v(true, false), CrashKind::Disconnect));
        assert!(!enabled(&v(true, false), CrashKind::Crash) && !enabled(&v(true, false), CrashKind::Hang));
        assert!(enabled(&v(false, true), CrashKind::Crash) && enabled(&v(false, true), CrashKind::Hang));
        assert!(!enabled(&v(false, true), CrashKind::Disconnect));
        assert!(!enabled(&v(true, true), CrashKind::Mem));
        assert!(enabled(&Settings { mem_restart_mb: 4_096, ..Settings::default() }, CrashKind::Mem));
    }
}
