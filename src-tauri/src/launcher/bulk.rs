//! Bulk launch: one batch at a time, strictly one account after another (DESIGN.md §6).
//!
//! The runner, per batch:
//! 1. Same server. `PickFirst` with a public-game target: resolve the target with the first
//!    account's cookie (`super::resolve`); a `Join::Game` means a public game, so
//!    `pick_server` chooses one server and every account is sent there as
//!    `LaunchTarget::Server { place_id, job_id }` (launch data is dropped, RequestGameJob has
//!    none). No server with room → `FollowFirst`, with a note. Any other `Join` (a job, a
//!    private server, a user) already is one server: same-server is ignored.
//!    `FollowFirst`: the first account launches with the target as given; its server is read
//!    from its tracked instance (`Instance.job_id`, up to 90 s), else from its own presence
//!    (its own cookie, `game_id`); the rest are sent there. Not found → the rest launch with
//!    the target as given, with a note.
//! 2. Per account: stage `Ticket`, then `commands::launch_tracked` (the favourite is touched
//!    for the first started client only), then `Starting` with its instance id; later stages
//!    mirror that instance (`stage_of`).
//! 3. Between launches: wait until the previous client's log is bound (its instance left
//!    `starting`, at most 30 s), then `delay_ms` ± 1 s (`jittered`). After the last launch,
//!    keep mirroring until every started client is in game or ended (at most 120 s), then
//!    `done = true`.
//! 4. Stops: `AppError::Challenge`, or an instance note saying it signed in as another
//!    account → that item `Failed`, the rest `Cancelled`, `stopped` says why and what to do.
//!    `CookieInvalid` and other errors fail only that item; the batch goes on. Cancel stops
//!    after the current step (never mid-launch): the rest `Cancelled`, `stopped = "Cancelled"`.
//! 5. Every change emits `bulk://progress` with the whole `BulkProgress`, and `Bulk` keeps the
//!    latest copy for `bulk_current`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::AppHandle;

use crate::error::{AppError, Result};
use crate::model::{BulkProgress, BulkRequest, BulkStage, GameServer, Instance};

/// The running or last batch.
pub struct Bulk {
    inner: Mutex<Option<Batch>>,
}

struct Batch {
    progress: BulkProgress,
    cancel: Arc<AtomicBool>,
}

impl Bulk {
    pub fn new() -> Bulk {
        Bulk { inner: Mutex::new(None) }
    }

    /// A copy of the running or last batch.
    pub fn current(&self) -> Option<BulkProgress> {
        todo!()
    }

    /// Asks the running batch to stop after its current step. `NotFound` when `batch_id` isn't
    /// the current batch; Ok (no-op) when it already finished.
    pub fn cancel(&self, batch_id: &str) -> Result<()> {
        let _ = (batch_id, Ordering::SeqCst);
        todo!()
    }
}

impl Default for Bulk {
    fn default() -> Self {
        Self::new()
    }
}

/// Validates `req` and starts the runner (module docs) on the async runtime; returns the new
/// batch id (a UUID). Fails before anything starts when:
/// - `account_ids` is empty or has duplicates → `Invalid`;
/// - an id isn't a stored account with a valid cookie → `Invalid("<name> can't launch: ...")`
///   (`NotFound` for an unknown id);
/// - a batch is still running → `Invalid("A batch is already running.")`;
/// - more than one account while multi-instance isn't active (after trying to take it, see
///   `commands::retake_multi`) → `Invalid("Turn on multi-instance to launch several accounts.")`
///   or the lock's own message when Roblox is running;
/// - the vault is locked → `Locked`.
///
/// `delay_ms` is clamped to 2 000..=60 000.
pub fn start(app: AppHandle, req: BulkRequest) -> Result<String> {
    let _ = (app, req, AppError::NotFound, Arc::new(AtomicBool::new(false)));
    todo!()
}

/// The fullest server that still fits `n` more players with one seat to spare; else the
/// fullest that fits exactly; None when none fits. Ids that aren't GUIDs are skipped.
pub fn choose_server(servers: &[GameServer], n: usize) -> Option<String> {
    let _ = (servers, n);
    todo!()
}

/// `delay_ms` (clamped 2 000..=60 000) plus a random ±1 s, never below 1 s.
pub fn jittered(delay_ms: u32) -> Duration {
    let _ = delay_ms;
    todo!()
}

/// The batch stage a started client is at: `starting` → `Starting`, `joining` → `Joining`,
/// `inGame` → `InGame`, `disconnected` / `exited` → `Failed` with the instance's note (or
/// "Disconnected" / "Closed").
pub fn stage_of(instance: &Instance) -> BulkStage {
    let _ = instance;
    todo!()
}
