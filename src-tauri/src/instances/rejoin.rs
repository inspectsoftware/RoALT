//! Auto-rejoin: the tracker loop hands over clients that ended unexpectedly
//! (`Tracker::take_rejoins`); with `settings.auto_rejoin` on, each is launched again into the
//! same target after the launch delay (`launcher::bulk::jittered(settings.launch_delay_ms)`).

use tauri::AppHandle;

use super::tracker::Rejoin;

/// Called by the tracker loop after each poll with what `take_rejoins` returned. When
/// auto-rejoin is off, does nothing. Otherwise, per rejoin, on the async runtime: wait the
/// launch delay; then skip it (logged, plus an `app://notice`) when auto-rejoin was turned off
/// meanwhile, the vault is locked, a batch is running, the account is gone, is no longer the
/// same user or its cookie isn't valid, or the account already has a client starting, joining
/// or in game; else `commands::launch_tracked(account, target, no favourite)` and
/// `tracker.set_rejoins(new id, r.rejoins)`. A failed launch is an `app://notice` (error) and
/// is not retried here (its client never started, so it can't end unexpectedly).
pub fn handle(app: &AppHandle, rejoins: Vec<Rejoin>) {
    let _ = (app, rejoins);
    todo!()
}
