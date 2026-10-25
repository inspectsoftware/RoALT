//! Bulk launch: one batch at a time, strictly one account after another (DESIGN.md §6).
//!
//! The runner, per batch:
//! 1. Same server (both modes): resolve the target with the first account's cookie
//!    (`super::resolve`); a `Join::Game` means a public game. Any other `Join` (a job, a
//!    private server, a user) already is one server: same-server is ignored. A target that
//!    can't be resolved here: ignored, with a note.
//!    `PickFirst`: `pick_server` reads one page of the place's servers (Recommended, 100, full
//!    ones left out, first account's cookie; OccupancyDesc if Roblox refuses Recommended) and
//!    `choose_server` picks one; every account is
//!    sent there as `LaunchTarget::Server { place_id, job_id }` (launch data is dropped,
//!    RequestGameJob has none). No server with room, or no list → `FollowFirst`, with a note.
//!    `FollowFirst`: the first account that starts a client launches with the target as given;
//!    its server is read from its tracked instance (`Instance.job_id`, up to 90 s), else from
//!    its own presence (its own cookie, `game_id`); the rest are sent there. Not found → the
//!    rest launch with the target as given, with a note.
//! 2. Per account: stage `Ticket`, then `commands::launch_tracked` (the favourite is touched
//!    for the first started client only), then `Starting` with its instance id; later stages
//!    mirror that instance (`stage_of`).
//! 3. Between launches: wait until the previous client's log is bound (its instance left
//!    `starting`, at most 30 s), then `delay_ms` ± 1 s (`jittered`). After the last launch,
//!    keep mirroring until every started client is in game or ended (at most 120 s), then
//!    `done = true`.
//! 4. Stops: `AppError::Challenge`, `AppError::Locked` (the vault locked mid-batch), or an
//!    instance note saying it signed in as another account → that item `Failed`, the rest
//!    `Cancelled`, `stopped` says why and what to do.
//!    `CookieInvalid` and other errors fail only that item; the batch goes on. Cancel stops
//!    after the current step (never mid-launch): the rest `Cancelled`, `stopped = "Cancelled"`.
//!    A stopped batch is done at once (its started clients stay in the instances list).
//! 5. Every change emits `bulk://progress` with the whole `BulkProgress`, and `Bulk` keeps the
//!    latest copy for `bulk_current`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use super::Join;
use super::target::is_uuid;
use crate::App;
use crate::error::{AppError, Result};
use crate::instances::WRONG_ACCOUNT;
use crate::model::{
    AccountStatus, AccountSummary, BulkItem, BulkProgress, BulkRequest, BulkStage, GameServer, Instance,
    InstanceState, LaunchTarget, SameServer, ServerOrder,
};

const EVENT: &str = "bulk://progress";
const MIN_DELAY: u32 = 2_000;
const MAX_DELAY: u32 = 60_000;
const CANCELLED: &str = "Cancelled";
/// How often a wait looks at the clients and the cancel flag.
const TICK: Duration = Duration::from_millis(500);
const BIND_WAIT: Duration = Duration::from_secs(30);
const FOLLOW_WAIT: Duration = Duration::from_secs(90);
const SETTLE_WAIT: Duration = Duration::from_secs(120);

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
        self.batch().as_ref().map(|b| b.progress.clone())
    }

    /// Asks the running batch to stop after its current step. `NotFound` when `batch_id` isn't
    /// the current batch; Ok (no-op) when it already finished.
    pub fn cancel(&self, batch_id: &str) -> Result<()> {
        match self.batch().as_ref() {
            Some(b) if b.progress.batch_id == batch_id => {
                b.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            _ => Err(AppError::NotFound),
        }
    }

    fn batch(&self) -> MutexGuard<'_, Option<Batch>> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
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
    let s = app.state::<App>();
    let ids = &req.account_ids;
    if ids.is_empty() {
        return Err(AppError::Invalid("Pick at least one account.".into()));
    }
    if ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id)) {
        return Err(AppError::Invalid("An account is in the list twice.".into()));
    }
    if s.vault.is_locked() {
        return Err(AppError::Locked);
    }
    let mut accounts = Vec::with_capacity(ids.len());
    for &id in ids {
        let a = s.db.account(id)?;
        let name = name_of(&a);
        if let Some(why) = cant_launch(a.status) {
            return Err(AppError::Invalid(format!("{name} can't launch: {why}")));
        }
        accounts.push(Account { id, user_id: a.user_id, name });
    }
    if ids.len() > 1 && !s.multi.is_active() {
        let v = s.db.settings()?;
        if !v.multi_instance {
            return Err(AppError::Invalid("Turn on multi-instance to launch several accounts.".into()));
        }
        s.multi.enable()?;
    }

    let progress = new_progress(Uuid::new_v4().to_string(), &req);
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut batch = s.bulk.batch();
        if batch.as_ref().is_some_and(|b| !b.progress.done) {
            return Err(AppError::Invalid("A batch is already running.".into()));
        }
        *batch = Some(Batch { progress: progress.clone(), cancel: cancel.clone() });
    }
    let _ = app.emit(EVENT, &progress);
    log::info!("bulk batch {}: {} account(s)", progress.batch_id, accounts.len());
    let run = Run { app: app.clone(), batch_id: progress.batch_id.clone(), cancel, accounts };
    tauri::async_runtime::spawn(run.run(req));
    Ok(progress.batch_id)
}

/// The fullest server that still fits `n` more players with one seat to spare; else the
/// fullest that fits exactly; None when none fits. Ids that aren't GUIDs are skipped.
pub fn choose_server(servers: &[GameServer], n: usize) -> Option<String> {
    let n = i64::try_from(n).ok()?;
    let fullest = |spare: i64| {
        servers
            .iter()
            .filter(|s| is_uuid(&s.id) && s.playing >= 0)
            .filter(|s| i64::from(s.playing) + n + spare <= i64::from(s.max_players))
            .min_by_key(|s| std::cmp::Reverse(s.playing))
    };
    fullest(1).or_else(|| fullest(0)).map(|s| s.id.clone())
}

/// `delay_ms` (clamped 2 000..=60 000) plus a random ±1 s, never below 1 s.
pub fn jittered(delay_ms: u32) -> Duration {
    let jitter = (Uuid::new_v4().as_u128() % 2_001) as i64 - 1_000;
    let ms = i64::from(delay_ms.clamp(MIN_DELAY, MAX_DELAY)) + jitter;
    Duration::from_millis(ms.max(1_000) as u64)
}

/// The batch stage a started client is at: `starting` → `Starting`, `joining` → `Joining`,
/// `inGame` → `InGame`, `disconnected` / `exited` → `Failed` with the instance's note (or
/// "Disconnected" / "Closed").
pub fn stage_of(instance: &Instance) -> BulkStage {
    let failed = |default: &str| BulkStage::Failed { message: instance.note.clone().unwrap_or_else(|| default.into()) };
    match instance.state {
        InstanceState::Starting => BulkStage::Starting,
        InstanceState::Joining => BulkStage::Joining,
        InstanceState::InGame => BulkStage::InGame,
        InstanceState::Disconnected => failed("Disconnected"),
        InstanceState::Exited => failed("Closed"),
    }
}

/// One account of a batch, as the runner needs it.
struct Account {
    id: i64,
    user_id: i64,
    name: String,
}

/// A running batch: owns nothing but its id; progress lives in `App.bulk`.
struct Run {
    app: AppHandle,
    batch_id: String,
    cancel: Arc<AtomicBool>,
    accounts: Vec<Account>,
}

impl Run {
    async fn run(self, req: BulkRequest) {
        let stop = match self.launch_all(&req).await {
            Some(why) => Some(why),
            None => self.wait(SETTLE_WAIT, || self.progress().is_none_or(|p| settled(&p))).await,
        };
        if stop.is_some() {
            log::info!("bulk batch {} stopped early", self.batch_id);
        }
        self.update(|p| {
            if let Some(why) = stop {
                halt(p, why);
            }
            p.done = true;
        });
    }

    /// Every launch in order; `Some(why)` when the batch has to stop.
    async fn launch_all(&self, req: &BulkRequest) -> Option<String> {
        let (mut server, mut follow) = (None, false);
        if req.same_server != SameServer::Off {
            self.set_stage(0, BulkStage::Ticket);
            (server, follow) = self.same_server(req).await;
            if let Some(found) = &server {
                self.update(|p| p.job_id = Some(found.1.clone()));
            }
        }
        let (mut prev, mut started) = (None::<String>, false);
        for (i, a) in self.accounts.iter().enumerate() {
            if i > 0 {
                if let Some(id) = prev.take() {
                    let bound = || self.instance(&id).is_none_or(|x| x.state != InstanceState::Starting);
                    if let Some(why) = self.wait(BIND_WAIT, bound).await {
                        return Some(why);
                    }
                }
                if let Some(why) = self.wait(jittered(req.delay_ms), || false).await {
                    return Some(why);
                }
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Some(CANCELLED.into());
            }

            self.set_stage(i, BulkStage::Ticket);
            let target = target_for(&req.target, server.as_ref());
            let favourite = if started { None } else { req.favourite_id };
            let s = self.app.state::<App>();
            match crate::commands::launch_tracked(&self.app, &s, a.id, target, favourite).await {
                Ok((info, inst)) => {
                    started = true;
                    self.update(|p| {
                        p.items[i].instance_id = Some(inst.id.clone());
                        p.items[i].stage = BulkStage::Starting;
                        if p.note.is_none() {
                            p.note = info.warning;
                        }
                    });
                    // The first client that starts leads when the rest follow it.
                    if std::mem::take(&mut follow) {
                        match self.followed(a, &inst.id).await {
                            Err(why) => return Some(why),
                            Ok(Some(found)) => {
                                self.update(|p| p.job_id = Some(found.1.clone()));
                                server = Some(found);
                            }
                            Ok(None) => self.note(format!(
                                "{}'s server couldn't be read, so the rest were matched by Roblox on their own.",
                                a.name
                            )),
                        }
                    }
                    prev = Some(inst.id);
                }
                Err(e) => {
                    self.set_stage(i, BulkStage::Failed { message: e.to_string() });
                    if let Some(why) = stop_reason(&e, &a.name) {
                        return Some(why);
                    }
                }
            }
        }
        None
    }

    /// The server everyone is sent to (`PickFirst`) and whether the rest follow the first
    /// started client instead (module docs, step 1).
    async fn same_server(&self, req: &BulkRequest) -> (Option<(i64, String)>, bool) {
        let s = self.app.state::<App>();
        let first = &self.accounts[0];
        let resolved = match super::account_cookie(&s, first.id) {
            Ok(cookie) => super::resolve(&s.api, first.id, &cookie, &req.target).await.map(|(join, _)| (join, cookie)),
            Err(e) => Err(e),
        };
        let (place_id, cookie) = match resolved {
            Ok((Join::Game { place_id, .. }, cookie)) => (place_id, cookie),
            Ok(_) => return (None, false),
            Err(e) => {
                self.note(format!("The target couldn't be checked ({e}), so each account joins on its own."));
                return (None, false);
            }
        };
        if req.same_server == SameServer::FollowFirst {
            return (None, true);
        }
        let n = self.accounts.len();
        let why = match pick_server(&s.api, first.id, &cookie, place_id, n).await {
            Ok(Some(job_id)) => return (Some((place_id, job_id)), false),
            Ok(None) => format!("No server had room for all {n} accounts"),
            Err(e) => format!("The server list couldn't be read ({e})"),
        };
        self.note(format!("{why}, so the rest follow the first account's server."));
        (None, true)
    }

    /// The server the leading client went to: from its log (up to 90 s), else from its own
    /// presence. `Err(why)` when the batch has to stop meanwhile.
    async fn followed(&self, a: &Account, instance_id: &str) -> std::result::Result<Option<(i64, String)>, String> {
        let known = |i: Instance| i.job_id.is_some() || matches!(i.state, InstanceState::Disconnected | InstanceState::Exited);
        if let Some(why) = self.wait(FOLLOW_WAIT, || self.instance(instance_id).is_none_or(known)).await {
            return Err(why);
        }
        let inst = self.instance(instance_id);
        if let Some(found) = inst.as_ref().and_then(|i| server_of(i.place_id, i.job_id.as_deref())) {
            return Ok(Some(found));
        }
        let s = self.app.state::<App>();
        let Ok(cookie) = super::account_cookie(&s, a.id) else { return Ok(None) };
        let rows = s.api.presence(a.id, &cookie, &[a.user_id]).await.unwrap_or_default();
        let Some(p) = rows.into_iter().find(|p| p.user_id == a.user_id) else { return Ok(None) };
        Ok(server_of(p.place_id.or(inst.and_then(|i| i.place_id)), p.game_id.as_deref()))
    }

    /// Mirrors the started clients until `done()` or `max`; `Some(why)` on cancel or when a
    /// client signed in as another account.
    async fn wait(&self, max: Duration, done: impl Fn() -> bool) -> Option<String> {
        let end = Instant::now() + max;
        loop {
            if let Some(why) = self.mirror() {
                return Some(why);
            }
            if self.cancel.load(Ordering::SeqCst) {
                return Some(CANCELLED.into());
            }
            let left = end.saturating_duration_since(Instant::now());
            if done() || left.is_zero() {
                return None;
            }
            tokio::time::sleep(left.min(TICK)).await;
        }
    }

    /// Every started client's state into its item; `Some(why)` when one must stop the batch.
    fn mirror(&self) -> Option<String> {
        let s = self.app.state::<App>();
        let mut stop = None;
        self.update(|p| {
            for (item, a) in p.items.iter_mut().zip(&self.accounts) {
                if let Some(inst) = item.instance_id.as_deref().and_then(|id| s.tracker.get(id)) {
                    let why = mirror_item(item, &inst, &a.name);
                    stop = stop.take().or(why);
                }
            }
        });
        stop
    }

    fn instance(&self, id: &str) -> Option<Instance> {
        self.app.state::<App>().tracker.get(id)
    }

    fn progress(&self) -> Option<BulkProgress> {
        self.app.state::<App>().bulk.current()
    }

    fn set_stage(&self, i: usize, stage: BulkStage) {
        self.update(|p| p.items[i].stage = stage);
    }

    /// Keeps the first note.
    fn note(&self, note: String) {
        self.update(|p| {
            p.note.get_or_insert(note);
        });
    }

    /// Changes this batch's progress and emits it when anything changed.
    fn update(&self, f: impl FnOnce(&mut BulkProgress)) {
        let s = self.app.state::<App>();
        let changed = {
            let mut batch = s.bulk.batch();
            let Some(b) = batch.as_mut().filter(|b| b.progress.batch_id == self.batch_id) else { return };
            let before = b.progress.clone();
            f(&mut b.progress);
            (b.progress != before).then(|| b.progress.clone())
        };
        if let Some(p) = changed {
            let _ = self.app.emit(EVENT, p);
        }
    }
}

/// One page of the place's servers (Recommended, full ones left out; the fullest first if
/// Roblox refuses Recommended for this account) and `choose_server`.
async fn pick_server(
    api: &crate::roblox_api::Client,
    account_id: i64,
    cookie: &crate::vault::Secret,
    place_id: i64,
    n: usize,
) -> Result<Option<String>> {
    let list = |order| api.servers_public(account_id, Some(cookie), place_id, order, true, 100, None);
    let page = match list(ServerOrder::Recommended).await {
        Err(AppError::Invalid(_)) => list(ServerOrder::OccupancyDesc).await?,
        r => r?,
    };
    Ok(choose_server(&page.servers, n))
}

/// A new batch: every account `Queued`, the delay clamped.
fn new_progress(batch_id: String, req: &BulkRequest) -> BulkProgress {
    let queued = |&account_id| BulkItem { account_id, stage: BulkStage::Queued, instance_id: None };
    BulkProgress {
        batch_id,
        target: req.target.clone(),
        same_server: req.same_server,
        delay_ms: req.delay_ms.clamp(MIN_DELAY, MAX_DELAY),
        job_id: None,
        items: req.account_ids.iter().map(queued).collect(),
        stopped: None,
        done: false,
        note: None,
    }
}

/// Alias, else display name, else username.
fn name_of(a: &AccountSummary) -> String {
    [&a.alias, &a.display_name, &a.username].into_iter().find(|n| !n.trim().is_empty()).cloned().unwrap_or_default()
}

/// Why an account can't be in a batch (None when its cookie is valid).
fn cant_launch(status: AccountStatus) -> Option<&'static str> {
    match status {
        AccountStatus::Valid => None,
        AccountStatus::Invalid => Some("its cookie is no longer valid. Sign in to it again."),
        AccountStatus::Challenged => Some("Roblox wants a human check. Sign in to it again."),
        AccountStatus::Unknown => Some("its cookie hasn't been checked yet. Validate it first."),
    }
}

/// A launch error that stops the batch (why and what to do); None when only that item fails.
fn stop_reason(e: &AppError, name: &str) -> Option<String> {
    match e {
        AppError::Challenge => Some(format!(
            "Roblox wants a human check for {name}. Sign in to it again with Add account > Sign in, then launch the rest."
        )),
        AppError::Locked => Some("The vault locked. Unlock it, then launch the rest.".into()),
        _ => None,
    }
}

/// Mirrors one started client into its item; `Some(why)` when it signed in as another account.
fn mirror_item(item: &mut BulkItem, inst: &Instance, name: &str) -> Option<String> {
    if inst.note.as_deref() == Some(WRONG_ACCOUNT) {
        item.stage = BulkStage::Failed { message: WRONG_ACCOUNT.into() };
        return Some(format!(
            "{name}'s client signed in as another account. Close that client, turn on Settings > Protect each \
             client's sign-in, then launch the rest."
        ));
    }
    item.stage = stage_of(inst);
    None
}

/// A server to send the rest to: a known place and a GUID job id.
fn server_of(place_id: Option<i64>, job_id: Option<&str>) -> Option<(i64, String)> {
    let job_id = job_id.filter(|j| is_uuid(j))?;
    Some((place_id.filter(|&p| p > 0)?, job_id.to_owned()))
}

/// What one account launches into: the shared server once known, else the target as given.
fn target_for(given: &LaunchTarget, server: Option<&(i64, String)>) -> LaunchTarget {
    match server {
        Some((place_id, job_id)) => LaunchTarget::Server { place_id: *place_id, job_id: job_id.clone() },
        None => given.clone(),
    }
}

/// Ends the batch early: what hasn't launched is `Cancelled`, `stopped` says why.
fn halt(p: &mut BulkProgress, why: String) {
    for item in &mut p.items {
        if matches!(item.stage, BulkStage::Queued | BulkStage::Ticket) {
            item.stage = BulkStage::Cancelled;
        }
    }
    p.stopped = Some(why);
}

/// Every started client is in game or ended.
fn settled(p: &BulkProgress) -> bool {
    p.items
        .iter()
        .filter(|i| i.instance_id.is_some())
        .all(|i| matches!(i.stage, BulkStage::InGame | BulkStage::Failed { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guid(n: u8) -> String {
        format!("aaaaaaaa-0000-4000-8000-0000000000{n:02}")
    }

    fn server(id: &str, playing: i32, max_players: i32) -> GameServer {
        GameServer { id: id.into(), playing, max_players, fps: None, ping: None, friend_count: None }
    }

    fn instance(state: InstanceState, note: Option<&str>) -> Instance {
        Instance {
            id: "i1".into(),
            account_id: 1,
            pid: 100,
            state,
            target: LaunchTarget::Place { place_id: 1818, launch_data: None },
            place_id: Some(1818),
            job_id: None,
            started_at: 0,
            ended_at: None,
            via_bootstrapper: false,
            note: note.map(Into::into),
        }
    }

    fn item(stage: BulkStage, started: bool) -> BulkItem {
        BulkItem { account_id: 1, stage, instance_id: started.then(|| "i1".into()) }
    }

    fn failed(message: &str) -> BulkStage {
        BulkStage::Failed { message: message.into() }
    }

    #[test]
    fn choose_server_prefers_the_fullest_with_a_spare_seat() {
        let (a, b, c, d) = (guid(1), guid(2), guid(3), guid(4));
        // n = 3: b (33/35) doesn't fit; d (32/35) fits exactly; a (30/35) and c (20/35) have a spare seat.
        let list = [server(&c, 20, 35), server(&b, 33, 35), server(&d, 32, 35), server(&a, 30, 35)];
        assert_eq!(choose_server(&list, 3), Some(a.clone()));
        // No spare seat anywhere: the fullest exact fit.
        let exact = [server(&b, 33, 35), server(&d, 32, 35), server(&c, 31, 34)];
        assert_eq!(choose_server(&exact, 3), Some(d.clone()));
        // Nothing fits.
        assert_eq!(choose_server(&[server(&b, 33, 35), server(&a, 35, 35)], 3), None);
        assert_eq!(choose_server(&[], 1), None);
        // Duplicates (pages repeat servers) change nothing.
        assert_eq!(choose_server(&[server(&a, 30, 35), server(&c, 20, 35), server(&a, 30, 35)], 3), Some(a.clone()));
    }

    #[test]
    fn choose_server_skips_ids_that_are_not_guids() {
        let bad = ["not-a-guid", "", "aaaaaaaa-0000-4000-8000-00000000001", "aaaaaaaa_0000_4000_8000_000000000001"];
        let mut list: Vec<GameServer> = bad.iter().map(|id| server(id, 30, 35)).collect();
        assert_eq!(choose_server(&list, 2), None);
        list.push(server(&guid(9), 5, 35));
        assert_eq!(choose_server(&list, 2), Some(guid(9)));
        // A negative player count is nonsense, not room.
        assert_eq!(choose_server(&[server(&guid(1), -5, 10)], 2), None);
    }

    #[test]
    fn jitter_stays_within_a_second_of_the_clamped_delay() {
        for _ in 0..500 {
            let ms = |d| jittered(d).as_millis();
            assert!((5_000..=7_000).contains(&ms(6_000)));
            assert!((1_000..=3_000).contains(&ms(0)));
            assert!((1_000..=3_000).contains(&ms(2_000)));
            assert!((59_000..=61_000).contains(&ms(u32::MAX)));
        }
        let spread: std::collections::HashSet<u128> = (0..50).map(|_| jittered(6_000).as_millis()).collect();
        assert!(spread.len() > 1, "jitter is random");
    }

    #[test]
    fn stages_mirror_the_instance() {
        use InstanceState::*;
        assert_eq!(stage_of(&instance(Starting, None)), BulkStage::Starting);
        assert_eq!(stage_of(&instance(Joining, None)), BulkStage::Joining);
        assert_eq!(stage_of(&instance(InGame, None)), BulkStage::InGame);
        assert_eq!(stage_of(&instance(Disconnected, None)), failed("Disconnected"));
        assert_eq!(stage_of(&instance(Exited, None)), failed("Closed"));
        assert_eq!(stage_of(&instance(Exited, Some("Closed unexpectedly"))), failed("Closed unexpectedly"));
        assert_eq!(stage_of(&instance(Disconnected, Some("Roblox wants a human check"))), failed("Roblox wants a human check"));
    }

    #[test]
    fn only_a_challenge_a_locked_vault_or_a_wrong_account_stops_the_batch() {
        let why = stop_reason(&AppError::Challenge, "Alt 1").unwrap();
        assert!(why.contains("Alt 1") && why.contains("Sign in"), "{why}");
        assert!(stop_reason(&AppError::Locked, "Alt 1").unwrap().contains("Unlock"));
        for e in [AppError::CookieInvalid, AppError::Network("x".into()), AppError::Roblox("HTTP 500".into())] {
            assert_eq!(stop_reason(&e, "Alt 1"), None, "{e}");
        }

        let mut it = item(BulkStage::Starting, true);
        assert_eq!(mirror_item(&mut it, &instance(InstanceState::InGame, None), "Alt 1"), None);
        assert_eq!(it.stage, BulkStage::InGame);
        let why = mirror_item(&mut it, &instance(InstanceState::Joining, Some(WRONG_ACCOUNT)), "Alt 1").unwrap();
        assert!(why.starts_with("Alt 1's client signed in as another account"), "{why}");
        assert_eq!(it.stage, failed(WRONG_ACCOUNT));
        // Other notes only mirror.
        assert_eq!(mirror_item(&mut it, &instance(InstanceState::Joining, Some("Roblox wants a human check")), "Alt 1"), None);
        assert_eq!(it.stage, BulkStage::Joining);
    }

    #[test]
    fn halting_cancels_only_what_has_not_launched() {
        let req = BulkRequest {
            account_ids: vec![1, 2, 3, 4, 5],
            target: LaunchTarget::Place { place_id: 1818, launch_data: None },
            delay_ms: 6_000,
            same_server: SameServer::Off,
            favourite_id: None,
        };
        let mut p = new_progress("b".into(), &req);
        p.items[0] = item(BulkStage::InGame, true);
        p.items[1] = item(failed("HTTP 500"), false);
        p.items[2] = item(BulkStage::Ticket, false);
        halt(&mut p, CANCELLED.into());
        let stages: Vec<_> = p.items.iter().map(|i| i.stage.clone()).collect();
        let cancelled = BulkStage::Cancelled;
        assert_eq!(stages, [BulkStage::InGame, failed("HTTP 500"), cancelled.clone(), cancelled.clone(), cancelled]);
        assert_eq!((p.stopped.as_deref(), p.done), (Some("Cancelled"), false));
    }

    #[test]
    fn settled_once_every_started_client_is_in_game_or_ended() {
        let req = BulkRequest {
            account_ids: vec![1, 2],
            target: LaunchTarget::FollowUser { user_id: 1 },
            delay_ms: 500,
            same_server: SameServer::PickFirst,
            favourite_id: Some(3),
        };
        let mut p = new_progress("b".into(), &req);
        assert_eq!(p.delay_ms, 2_000);
        assert_eq!(new_progress("b".into(), &BulkRequest { delay_ms: 90_000, ..req.clone() }).delay_ms, 60_000);
        assert!(p.items.iter().all(|i| i.stage == BulkStage::Queued && i.instance_id.is_none()));
        assert!(settled(&p), "nothing started");
        p.items = vec![item(BulkStage::InGame, true), item(BulkStage::Joining, true)];
        assert!(!settled(&p));
        p.items[1] = item(failed("Closed"), true);
        assert!(settled(&p));
        p.items[1] = item(failed("HTTP 500"), false);
        assert!(settled(&p));
    }

    #[test]
    fn the_rest_go_to_the_known_server_else_the_given_target() {
        let job = guid(7);
        assert_eq!(server_of(Some(1818), Some(&job)), Some((1818, job.clone())));
        assert_eq!(server_of(None, Some(&job)), None);
        assert_eq!(server_of(Some(0), Some(&job)), None);
        assert_eq!(server_of(Some(1818), None), None);
        assert_eq!(server_of(Some(1818), Some("1818' place 1")), None);

        let given = LaunchTarget::Place { place_id: 1818, launch_data: Some("x".into()) };
        assert_eq!(target_for(&given, None), given);
        let server = target_for(&given, Some(&(4483381587, job.clone())));
        assert_eq!(server, LaunchTarget::Server { place_id: 4483381587, job_id: job });
    }

    #[test]
    fn accounts_are_named_and_checked_like_the_ui() {
        let mut a = AccountSummary {
            id: 1,
            user_id: 2,
            username: "nova_alt01".into(),
            display_name: "Nova Alt".into(),
            alias: "Alt 01".into(),
            notes: String::new(),
            color: None,
            favourite: false,
            sort_order: 0,
            group_ids: vec![],
            status: AccountStatus::Valid,
            status_checked_at: None,
            created_at: 0,
        };
        assert_eq!(name_of(&a), "Alt 01");
        a.alias.clear();
        assert_eq!(name_of(&a), "Nova Alt");
        a.display_name = " ".into();
        assert_eq!(name_of(&a), "nova_alt01");
        assert_eq!(cant_launch(AccountStatus::Valid), None);
        for s in [AccountStatus::Invalid, AccountStatus::Challenged, AccountStatus::Unknown] {
            assert!(cant_launch(s).is_some(), "{s:?}");
        }
    }
}
