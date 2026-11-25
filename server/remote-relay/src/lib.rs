//! Stateless, blind, in-memory WebSocket relay for RoALT Remote Dashboard.
//!
//! Stores nothing on disk and never sees plaintext application data, cookies, or keys.
//! Routes authenticated encrypted binary frames between RoALT PCs and browser viewers,
//! and hosts short-lived (120 s) pairing rooms.

use std::collections::{HashMap, HashSet, VecDeque};
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket};
use axum::extract::{ConnectInfo, State, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use dashmap::DashMap;
use dashmap::mapref::entry::Entry;
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::{Notify, mpsc};
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Lowest wire protocol version accepted in `pc_hello`, `subscribe` and `pair_join`.
pub const MIN_PROTO: u32 = 2;
/// Largest binary frame (header included) a PC may send; larger ones get `frame_too_large`.
pub const MAX_PC_FRAME: usize = 256 * 1024;
/// Largest binary frame (header included) a viewer may send; viewers only send commands and handshakes.
pub const MAX_VIEWER_FRAME: usize = 64 * 1024;
/// Largest text control frame; larger ones get `frame_too_large`.
pub const MAX_TEXT_FRAME: usize = 32 * 1024;
/// Hard WebSocket message and frame cap: a message above it ends the socket instead of getting
/// `frame_too_large`. Kept just above `MAX_PC_FRAME` so an unfinished message pins little memory.
pub const MAX_WS_MESSAGE: usize = MAX_PC_FRAME + 1024;
/// Maximum paired PCs one viewer socket may subscribe to simultaneously.
pub const MAX_SUBSCRIPTIONS: usize = 16;
/// Maximum viewer sockets attached to one PC at a time.
pub const MAX_VIEWERS_PER_PC: usize = 16;
/// Maximum sockets attached to one PC with the same room token (one device); a newer one replaces the oldest.
pub const MAX_SOCKETS_PER_TOKEN: usize = 3;
/// Maximum authorized device token hashes registered per PC.
pub const MAX_TOKENS_PER_PC: usize = 64;
/// Pairing room time-to-live.
pub const PAIR_TTL: Duration = Duration::from_secs(120);
/// Time a PC gives a joiner it approved to confirm (`pair_extend`), at most this past `PAIR_TTL`.
pub const PAIR_CONFIRM: Duration = Duration::from_secs(60);
/// Largest `data` in a `pair_join` or `pair_msg`; larger ones get `frame_too_large`.
pub const MAX_PAIR_DATA: usize = 8 * 1024;
/// Wire version byte for binary routed frames: `[0x01][16-byte target uuid][opaque ciphertext]`.
pub const BIN_VERSION: u8 = 0x01;
const BIN_HEADER_LEN: usize = 17;

const PAIR_RATE_WINDOW: Duration = Duration::from_secs(60);
const PAIR_RATE_MAX: u32 = 30;
const PAIR_RATE_MAX_V6_48: u32 = 60;
const MANUAL_MISS_WINDOW: Duration = Duration::from_secs(60);
/// Wrong manual codes per window, relay-wide; once spent, new manual-code joins are refused.
pub const MANUAL_MISS_MAX: u32 = 600;
/// One IPv4 /24 or IPv6 /32 may spend at most this share (1/n) of `MANUAL_MISS_MAX`.
pub const MANUAL_MISS_SHARE: u32 = 16;
const RANK_REFRESH: Duration = Duration::from_millis(5);
const SHED_TRIES: usize = 8;
const SHED_CHUNK: usize = MAX_PC_FRAME;
const SLOTS_PER_CONNECTION: usize = 16;
const PRUNE_GAP: Duration = Duration::from_secs(1);
const MAX_ROOM_JOINS: u8 = 4;
const MISROUTE_WINDOW: Duration = Duration::from_secs(10);
const MISROUTE_MAX: u32 = 64;
const IP_SLOT_COOLDOWN: Duration = Duration::from_secs(60);
const MAX_REFUSED_REPLIES: usize = 64;
const MAX_REFUSALS: usize = 4 * MAX_SUBSCRIPTIONS;

const CONTROL_ALLOWANCE: usize = 64 * 1024;
const MESSAGE_OVERHEAD: usize = 256;
const RATE_ERR_INTERVAL: Duration = Duration::from_secs(1);
const MAX_THROTTLE_STEP: Duration = Duration::from_secs(1);
const WS_BUFFER: usize = 16 * 1024;
const DASHBOARD_ORIGIN: &str = "https://remote.roalt.dev";
const KICK_REASONS: [&str; 2] = ["paused", "device_sessions"];

/// Token bucket sizes for inbound traffic on one connection.
#[derive(Clone, Copy, Debug)]
pub struct RateLimit {
    pub bytes_per_sec: f64,
    pub burst_bytes: f64,
    pub msgs_per_sec: f64,
    pub burst_msgs: f64,
}

/// Relay limits and timeouts. `Default` holds the production values.
#[derive(Clone, Debug)]
pub struct RelayConfig {
    pub registration_timeout: Duration,
    pub idle_timeout: Duration,
    pub ping_interval: Duration,
    pub write_timeout: Duration,
    /// A socket whose oldest queued message waited this long is too slow to read and is closed.
    pub queue_age_limit: Duration,
    /// A viewer attached to no PC, waiting for no online PC that accepts its token, and holding no
    /// pairing room, or a PC accepting no device token and holding no pairing room, gets
    /// `no_purpose` and is closed after this long.
    pub purposeless_timeout: Duration,
    /// A subscriber the PC has refused (wrong or revoked token) for this long is dropped from its wait list.
    pub stale_subscription: Duration,
    /// When it is reached, a newcomer from an address group holding fewer connections than the
    /// largest one closes that group's newest socket and takes its place.
    pub max_connections: usize,
    /// Per IPv4 address or IPv6 /64. When it is full, a newcomer replaces that address's socket
    /// that has sent nothing for longest, if that is more than a ping interval and a fifth.
    pub max_connections_per_ip: usize,
    /// Per IPv6 /56, across its /64s.
    pub max_connections_per_v6_56: usize,
    /// Per site: an IPv4 /24 or an IPv6 /48.
    pub max_connections_per_site: usize,
    /// Per IPv6 /32, across its /48s.
    pub max_connections_per_v6_32: usize,
    /// An address group that closed a socket to get in closes another one only this much later.
    pub eviction_interval: Duration,
    /// When it is full, a routed frame frees room from the largest other peer stream of the socket.
    pub socket_queue_budget: usize,
    /// When it is full, a routed frame first frees room from the largest address group that holds
    /// at least the receiver's own, compared where the two diverge.
    pub global_queue_budget: usize,
    pub pc_rate: RateLimit,
    pub viewer_rate: RateLimit,
    /// Shared by every socket of one IPv4 address or IPv6 /64; coarser groups get a multiple of it.
    pub ip_rate: RateLimit,
    /// `pc_tokens`, `pair_open` and `subscribe` per socket (bytes ignored).
    pub control_rate: RateLimit,
    /// New connections per IPv4 address or IPv6 /64 (bytes ignored). A coarser group gets this times
    /// its connection cap over `max_connections_per_ip`, and holds up only a subgroup that has been
    /// connecting more than its share of that, the subgroups it has not seen yet counting as one, so
    /// a busy neighbour can't use up a small client's share and rotating through fresh prefixes is
    /// still limited.
    pub connect_rate: RateLimit,
    /// Lowest RoALT version (`pc_hello.app`) accepted, if set.
    pub min_app_version: Option<(u32, u32, u32)>,
    /// Peers (besides loopback) whose `X-Real-IP` header is trusted.
    pub trusted_proxies: Vec<IpAddr>,
}

impl Default for RelayConfig {
    fn default() -> Self {
        Self {
            registration_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(70),
            ping_interval: Duration::from_secs(25),
            write_timeout: Duration::from_secs(10),
            queue_age_limit: Duration::from_secs(15),
            purposeless_timeout: Duration::from_secs(30),
            stale_subscription: Duration::from_secs(120),
            max_connections: 4096,
            max_connections_per_ip: 32,
            max_connections_per_v6_56: 64,
            max_connections_per_site: 256,
            max_connections_per_v6_32: 1024,
            eviction_interval: Duration::from_secs(10),
            socket_queue_budget: 2 * 1024 * 1024,
            global_queue_budget: 256 * 1024 * 1024,
            pc_rate: RateLimit {
                bytes_per_sec: 4.0 * 1024.0 * 1024.0,
                burst_bytes: 8.0 * 1024.0 * 1024.0,
                msgs_per_sec: 400.0,
                burst_msgs: 800.0,
            },
            viewer_rate: RateLimit {
                bytes_per_sec: 256.0 * 1024.0,
                burst_bytes: 1024.0 * 1024.0,
                msgs_per_sec: 200.0,
                burst_msgs: 400.0,
            },
            ip_rate: RateLimit {
                bytes_per_sec: 8.0 * 1024.0 * 1024.0,
                burst_bytes: 16.0 * 1024.0 * 1024.0,
                msgs_per_sec: 1000.0,
                burst_msgs: 2000.0,
            },
            control_rate: RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 4.0, burst_msgs: 8.0 },
            connect_rate: RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 10.0, burst_msgs: 20.0 },
            min_app_version: None,
            trusted_proxies: Vec::new(),
        }
    }
}

impl RelayConfig {
    /// Defaults overridden by `RELAY_MAX_CONNS`, `RELAY_MAX_CONNS_PER_IP`, `RELAY_MIN_APP_VERSION`
    /// (`major.minor.patch`) and `RELAY_TRUSTED_PROXIES` (comma-separated IPs).
    pub fn from_env() -> Self {
        let mut cfg = Self::default();
        if let Some(n) = env_usize("RELAY_MAX_CONNS") {
            cfg.max_connections = n;
        }
        if let Some(n) = env_usize("RELAY_MAX_CONNS_PER_IP") {
            cfg.max_connections_per_ip = n;
        }
        let (total, per_ip) = (cfg.max_connections, cfg.max_connections_per_ip);
        cfg.max_connections_per_v6_56 = (total / 64).max(per_ip);
        cfg.max_connections_per_site = (total / 16).max(per_ip);
        cfg.max_connections_per_v6_32 = (total / 4).max(per_ip);
        if let Ok(raw) = std::env::var("RELAY_MIN_APP_VERSION")
            && !raw.trim().is_empty()
        {
            cfg.min_app_version =
                Some(parse_version(&raw).unwrap_or_else(|| panic!("RELAY_MIN_APP_VERSION must look like 1.4.0, got {raw:?}")));
        }
        if let Ok(list) = std::env::var("RELAY_TRUSTED_PROXIES") {
            cfg.trusted_proxies = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    s.parse::<IpAddr>()
                        .unwrap_or_else(|_| panic!("RELAY_TRUSTED_PROXIES must list IP addresses, got {s:?}"))
                        .to_canonical()
                })
                .collect();
        }
        cfg
    }
}

fn env_usize(name: &str) -> Option<usize> {
    let raw = std::env::var(name).ok()?;
    Some(raw.trim().parse().unwrap_or_else(|_| panic!("{name} must be a positive integer, got {raw:?}")))
}

/// `major.minor.patch`, ignoring any `-pre`/`+build` suffix.
pub fn parse_version(raw: &str) -> Option<(u32, u32, u32)> {
    let core = raw.trim().split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

#[derive(Clone)]
pub struct RelayState {
    config: Arc<RelayConfig>,
    pcs: Arc<DashMap<Uuid, PcEntry>>,
    viewers: Arc<DashMap<Uuid, ViewerEntry>>,
    subscribers: Arc<DashMap<Uuid, HashSet<Uuid>>>,
    pair_rooms: Arc<DashMap<String, PairRoom>>,
    manual_codes: Arc<DashMap<String, CodeOwner>>,
    pair_rate: Arc<DashMap<IpAddr, (Instant, u32)>>,
    pair_rate_v6_48: Arc<DashMap<IpAddr, (Instant, u32)>>,
    manual_misses: Arc<Mutex<(Instant, u32)>>,
    manual_misses_by_net: Arc<DashMap<IpAddr, (Instant, u32)>>,
    connections: Arc<AtomicUsize>,
    address_slots: Arc<DashMap<AddressKey, Arc<Group>>>,
    admission: Arc<Mutex<Option<Instant>>>,
    shedder: Arc<Shedder>,
    queued_bytes: Arc<AtomicUsize>,
    control_bytes: Arc<AtomicUsize>,
    warned_untrusted_proxy: Arc<AtomicBool>,
}

type AddressKey = (IpAddr, u8);

struct PcEntry {
    conn_id: Uuid,
    secret_hash: String,
    allowed_tokens: HashSet<String>,
    outbox: Outbox,
    attached_viewers: HashMap<Uuid, Attached>,
}

struct Attached {
    token_hash: String,
    since: Instant,
}

struct ViewerEntry {
    outbox: Outbox,
    subscriptions: HashMap<Uuid, Subscription>,
    refused_since: HashMap<Uuid, Instant>,
}

#[derive(Clone)]
struct Subscription {
    device_id: Uuid,
    token_hash: String,
    since: Instant,
    kicked: bool,
    restart: bool,
}

struct PairRoom {
    pc_id: Uuid,
    pc_conn_id: Uuid,
    manual_code: Option<String>,
    opened: Instant,
    expires_at: Instant,
    viewer_id: Option<Uuid>,
    joins: u8,
}

struct Group {
    conns: AtomicUsize,
    queued: AtomicUsize,
    rate: Mutex<Inflow>,
    slot: Mutex<Slot>,
}

struct Slot {
    connects: Bucket,
    tried: Decay,
    newcomers: Decay,
    last_seen: Instant,
    last_evicted: Option<Instant>,
}

impl Group {
    fn conns(&self) -> usize {
        self.conns.load(Ordering::Acquire)
    }

    fn slot(&self) -> std::sync::MutexGuard<'_, Slot> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn rate(&self) -> std::sync::MutexGuard<'_, Inflow> {
        self.rate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn swapped_within(&self, window: Duration, now: Instant) -> bool {
        self.slot().last_evicted.is_some_and(|t| now.duration_since(t) < window)
    }
}

struct AddressLevel {
    key: AddressKey,
    max_conns: usize,
    rate_scale: f64,
}

struct CodeOwner {
    pair_id: String,
    pc_id: Uuid,
}

impl Default for RelayState {
    fn default() -> Self {
        Self::with_config(RelayConfig::default())
    }
}

impl RelayState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config(config: RelayConfig) -> Self {
        Self {
            config: Arc::new(config),
            pcs: Arc::default(),
            viewers: Arc::default(),
            subscribers: Arc::default(),
            pair_rooms: Arc::default(),
            manual_codes: Arc::default(),
            pair_rate: Arc::default(),
            pair_rate_v6_48: Arc::default(),
            manual_misses: Arc::new(Mutex::new((Instant::now(), 0))),
            manual_misses_by_net: Arc::default(),
            connections: Arc::default(),
            address_slots: Arc::default(),
            admission: Arc::default(),
            shedder: Arc::default(),
            queued_bytes: Arc::default(),
            control_bytes: Arc::default(),
            warned_untrusted_proxy: Arc::default(),
        }
    }

    pub fn stats(&self) -> (usize, usize, usize) {
        (self.pcs.len(), self.viewers.len(), self.pair_rooms.len())
    }

    /// Bytes of routed frames and replies currently queued for delivery across all sockets.
    pub fn queued_bytes(&self) -> usize {
        self.queued_bytes.load(Ordering::Acquire)
    }

    /// Bytes of the relay's own messages and pairing payloads currently queued, kept apart from
    /// `queued_bytes` so they never take room from routed frames.
    pub fn control_bytes(&self) -> usize {
        self.control_bytes.load(Ordering::Acquire)
    }

    /// Open WebSocket connections.
    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::Acquire)
    }

    /// Removes expired pairing rooms and stale rate-limit buckets.
    pub fn sweep_expired(&self) {
        let now = Instant::now();
        let expired: Vec<String> =
            self.pair_rooms.iter().filter(|r| r.expires_at <= now).map(|r| r.key().clone()).collect();
        for pair_id in expired {
            if let Some((_, room)) = self.pair_rooms.remove_if(&pair_id, |_, r| r.expires_at <= now) {
                room_closed(self, &pair_id, room);
            }
        }
        self.pair_rate.retain(|_, (since, _)| now.duration_since(*since) < PAIR_RATE_WINDOW);
        self.manual_misses_by_net.retain(|_, (since, _)| now.duration_since(*since) < MANUAL_MISS_WINDOW);
        self.pair_rate_v6_48.retain(|_, (since, _)| now.duration_since(*since) < PAIR_RATE_WINDOW);
        self.address_slots.retain(|_, g| g.conns() > 0 || now.duration_since(g.slot().last_seen) < IP_SLOT_COOLDOWN);
    }

    fn check_pair_rate(&self, ip: IpAddr, prefix: Option<IpAddr>) -> bool {
        count_pair_try(&self.pair_rate, ip, PAIR_RATE_MAX)
            && prefix.is_none_or(|prefix| count_pair_try(&self.pair_rate_v6_48, prefix, PAIR_RATE_MAX_V6_48))
    }

    fn manual_guesses_left(&self, net: IpAddr) -> bool {
        let mut window = self.manual_misses.lock().unwrap_or_else(PoisonError::into_inner);
        if window.0.elapsed() >= MANUAL_MISS_WINDOW {
            *window = (Instant::now(), 0);
        }
        let net_misses =
            self.manual_misses_by_net.get(&net).filter(|e| e.0.elapsed() < MANUAL_MISS_WINDOW).map_or(0, |e| e.1);
        net_misses < MANUAL_MISS_MAX / MANUAL_MISS_SHARE && (window.1 < MANUAL_MISS_MAX || net_misses == 0)
    }

    fn note_manual_miss(&self, net: IpAddr) {
        let mut window = self.manual_misses.lock().unwrap_or_else(PoisonError::into_inner);
        window.1 = window.1.saturating_add(1);
        drop(window);
        let now = Instant::now();
        let mut entry = self.manual_misses_by_net.entry(net).or_insert((now, 0));
        if now.duration_since(entry.0) >= MANUAL_MISS_WINDOW {
            *entry = (now, 0);
        }
        entry.1 = entry.1.saturating_add(1);
    }

    fn address_levels(&self, ip: IpAddr) -> Vec<AddressLevel> {
        let cfg = &self.config;
        let level = |key, max_conns, rate_scale| AddressLevel { key, max_conns, rate_scale };
        match ip.to_canonical() {
            IpAddr::V4(v4) => vec![
                level((IpAddr::V4(v4), 32), cfg.max_connections_per_ip, 1.0),
                level(v4_prefix(v4, 24), cfg.max_connections_per_site, 4.0),
            ],
            IpAddr::V6(v6) => vec![
                level((v6_prefix(v6, 64), 64), cfg.max_connections_per_ip, 1.0),
                level((v6_prefix(v6, 56), 56), cfg.max_connections_per_v6_56, 2.0),
                level((v6_prefix(v6, 48), 48), cfg.max_connections_per_site, 4.0),
                level((v6_prefix(v6, 32), 32), cfg.max_connections_per_v6_32, 8.0),
            ],
        }
    }

    fn new_group(&self, level: &AddressLevel) -> Group {
        let connect_scale = (level.max_conns as f64 / self.config.max_connections_per_ip.max(1) as f64).max(1.0);
        let connect = self.config.connect_rate;
        Group {
            conns: AtomicUsize::new(0),
            queued: AtomicUsize::new(0),
            rate: Mutex::new(Inflow::new(scaled(self.config.ip_rate, level.rate_scale))),
            slot: Mutex::new(Slot {
                connects: Bucket::new(RateLimit {
                    msgs_per_sec: connect.msgs_per_sec * connect_scale,
                    burst_msgs: connect.burst_msgs * connect_scale,
                    ..connect
                }),
                tried: Decay::default(),
                newcomers: Decay::default(),
                last_seen: Instant::now(),
                last_evicted: None,
            }),
        }
    }

    fn admit(&self, ip: IpAddr) -> Result<ConnGuard, StatusCode> {
        let levels = self.address_levels(ip);
        let now = Instant::now();
        let mut pruned = self.admission.lock().unwrap_or_else(PoisonError::into_inner);
        let known: Vec<Option<Arc<Group>>> = levels.iter().map(|l| self.address_slots.get(&l.key).map(|g| g.clone())).collect();
        if connects_spent(&known, now) {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        if known.iter().any(Option::is_none) && !self.room_for_groups(&mut pruned, now) {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }
        let groups: Vec<Arc<Group>> = levels
            .iter()
            .enumerate()
            .map(|(depth, level)| {
                let group = self.address_slots.entry(level.key).or_insert_with(|| Arc::new(self.new_group(level))).clone();
                let mut slot = group.slot();
                slot.last_seen = now;
                slot.connects.take_at(0, now);
                slot.tried.add(1.0, now);
                if depth > 0 && known[depth - 1].is_none() {
                    slot.newcomers.add(1.0, now);
                }
                drop(slot);
                group
            })
            .collect();
        let full = (0..levels.len()).find(|&depth| groups[depth].conns() >= levels[depth].max_conns);
        let seated = match full {
            Some(0) => self.replace_silent(&groups),
            Some(depth) => self.trim_for(&groups, Some(depth), now),
            None if self.connections() >= self.config.max_connections => {
                if !self.trim_for(&groups, None, now) {
                    return Err(StatusCode::SERVICE_UNAVAILABLE);
                }
                true
            }
            None => true,
        };
        if !seated {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        self.connections.fetch_add(1, Ordering::AcqRel);
        let mut child_opened = false;
        for group in &groups {
            if child_opened {
                group.rate().children += 1;
            }
            child_opened = group.conns.fetch_add(1, Ordering::AcqRel) == 0;
        }
        let seat = Arc::new(Seat { connections: self.connections.clone(), groups, released: AtomicBool::new(false) });
        Ok(ConnGuard { seat, ip: ip_bucket(ip), prefix: v6_prefix_48(ip), net: network_of(ip) })
    }

    fn room_for_groups(&self, pruned: &mut Option<Instant>, now: Instant) -> bool {
        let max = self.config.max_connections.saturating_mul(SLOTS_PER_CONNECTION);
        if self.address_slots.len() < max {
            return true;
        }
        if pruned.is_some_and(|t| now.duration_since(t) < PRUNE_GAP) {
            return false;
        }
        *pruned = Some(now);
        self.address_slots.retain(|_, g| g.conns() > 0);
        self.address_slots.len() < max
    }

    fn replace_silent(&self, mine: &[Arc<Group>]) -> bool {
        let dead_after = self.config.ping_interval + self.config.ping_interval / 5;
        let victim = self
            .shedder
            .queues
            .iter()
            .filter(|q| !q.killed.load(Ordering::Acquire) && q.seat.groups.first().is_some_and(|g| Arc::ptr_eq(g, &mine[0])))
            .map(|q| (q.silent_for(), q.clone()))
            .filter(|(silent, _)| *silent >= dead_after)
            .max_by_key(|(silent, _)| *silent);
        victim.is_some_and(|(_, lanes)| {
            lanes.kill();
            true
        })
    }

    fn trim_for(&self, mine: &[Arc<Group>], scope: Option<usize>, now: Instant) -> bool {
        let window = self.config.eviction_interval;
        let mut best: Option<([usize; 4], Instant, Arc<Lanes>, usize)> = None;
        for entry in self.shedder.queues.iter() {
            let q = entry.value();
            let theirs = &q.seat.groups;
            if q.killed.load(Ordering::Acquire)
                || scope.is_some_and(|depth| theirs.len() != mine.len() || !Arc::ptr_eq(&theirs[depth], &mine[depth]))
            {
                continue;
            }
            let Some(at) = theirs.iter().rev().zip(mine.iter().rev()).position(|(t, m)| !Arc::ptr_eq(t, m)) else {
                continue;
            };
            let (their_group, my_group) = (&theirs[theirs.len() - 1 - at], &mine[mine.len() - 1 - at]);
            if their_group.conns() <= my_group.conns() + 1 || my_group.swapped_within(window, now) {
                continue;
            }
            let mut rank = [0; 4];
            for (r, g) in rank.iter_mut().zip(theirs.iter().rev()) {
                *r = g.conns();
            }
            if best.as_ref().is_none_or(|(r, opened, ..)| (rank, q.opened) > (*r, *opened)) {
                best = Some((rank, q.opened, q.clone(), at));
            }
        }
        let Some((_, _, victim, at)) = best else {
            return false;
        };
        victim.kill();
        mine[mine.len() - 1 - at].slot().last_evicted = Some(now);
        true
    }

    fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let peer = peer.to_canonical();
        let forwarded = headers
            .get("x-real-ip")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<IpAddr>().ok());
        let Some(forwarded) = forwarded else {
            return peer;
        };
        if peer.is_loopback() || self.config.trusted_proxies.contains(&peer) {
            return forwarded.to_canonical();
        }
        if !self.warned_untrusted_proxy.swap(true, Ordering::Relaxed) {
            warn!(%peer, "ignoring X-Real-IP from an untrusted peer; list the proxy in RELAY_TRUSTED_PROXIES");
        }
        peer
    }
}

fn count_pair_try(map: &DashMap<IpAddr, (Instant, u32)>, key: IpAddr, max: u32) -> bool {
    let now = Instant::now();
    let mut entry = map.entry(key).or_insert((now, 0));
    if now.duration_since(entry.0) >= PAIR_RATE_WINDOW {
        *entry = (now, 1);
        true
    } else if entry.1 < max {
        entry.1 += 1;
        true
    } else {
        false
    }
}

fn scaled(rate: RateLimit, k: f64) -> RateLimit {
    let up = |x: f64| (x * k).min(f64::MAX);
    RateLimit {
        bytes_per_sec: up(rate.bytes_per_sec),
        burst_bytes: up(rate.burst_bytes),
        msgs_per_sec: up(rate.msgs_per_sec),
        burst_msgs: up(rate.burst_msgs),
    }
}

fn network_of(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4_prefix(v4, 24).0,
        IpAddr::V6(v6) => v6_prefix(v6, 32),
    }
}

fn v4_prefix(v4: std::net::Ipv4Addr, bits: u8) -> (IpAddr, u8) {
    (IpAddr::V4(std::net::Ipv4Addr::from(u32::from(v4) & (!0u32 << (32 - u32::from(bits))))), bits)
}

struct Seat {
    connections: Arc<AtomicUsize>,
    groups: Vec<Arc<Group>>,
    released: AtomicBool,
}

impl Seat {
    fn release(&self) {
        if self.released.swap(true, Ordering::AcqRel) {
            return;
        }
        let now = Instant::now();
        let mut child_closed = false;
        for group in &self.groups {
            if child_closed {
                let mut rate = group.rate();
                rate.children = rate.children.saturating_sub(1);
            }
            group.slot().last_seen = now;
            child_closed = group.conns.fetch_sub(1, Ordering::AcqRel) == 1;
        }
        self.connections.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Rate-limit and connection-cap key: the IPv4 address, or the IPv6 /64 prefix.
pub fn ip_bucket(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => v6_prefix(v6, 64),
        v4 => v4,
    }
}

fn origin_allowed(origin: &str) -> bool {
    if origin == DASHBOARD_ORIGIN {
        return true;
    }
    let Some(authority) = origin.strip_prefix("https://").or_else(|| origin.strip_prefix("http://")) else {
        return false;
    };
    let host = match authority.split_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        Some(_) => return false,
        None => authority,
    };
    host == "localhost" || host == "127.0.0.1"
}

fn v6_prefix(v6: Ipv6Addr, bits: u8) -> IpAddr {
    IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (!0u128 << (128 - u32::from(bits)))))
}

/// The IPv6 /48 an address belongs to; none for IPv4.
fn v6_prefix_48(ip: IpAddr) -> Option<IpAddr> {
    match ip.to_canonical() {
        IpAddr::V6(v6) => Some(v6_prefix(v6, 48)),
        IpAddr::V4(_) => None,
    }
}

struct ConnGuard {
    seat: Arc<Seat>,
    ip: IpAddr,
    prefix: Option<IpAddr>,
    net: IpAddr,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.seat.release();
    }
}

pub fn router(state: RelayState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/ws/remote", get(ws_handler))
        .with_state(state)
}

async fn healthz(State(state): State<RelayState>) -> impl IntoResponse {
    let (pcs, viewers, pair_rooms) = state.stats();
    Json(serde_json::json!({
        "ok": true,
        "pcs": pcs,
        "viewers": viewers,
        "pairRooms": pair_rooms,
        "connections": state.connections(),
        "queuedBytes": state.queued_bytes(),
        "controlBytes": state.control_bytes(),
    }))
}

async fn ws_handler(
    State(state): State<RelayState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if headers.get(header::ORIGIN).is_some_and(|o| !o.to_str().is_ok_and(origin_allowed)) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let ip = state.client_ip(addr.ip(), &headers);
    let guard = match state.admit(ip) {
        Ok(guard) => guard,
        Err(status) => return status.into_response(),
    };
    ws.max_message_size(MAX_WS_MESSAGE)
        .max_frame_size(MAX_WS_MESSAGE)
        .read_buffer_size(WS_BUFFER)
        .write_buffer_size(WS_BUFFER)
        .max_write_buffer_size(2 * MAX_WS_MESSAGE)
        .on_upgrade(move |socket| handle_socket(socket, state, guard))
}

#[derive(Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ClientMsg {
    PcHello {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
        #[serde(rename = "pcSecretHash")]
        pc_secret_hash: String,
        #[serde(rename = "allowedTokens", default)]
        allowed_tokens: Vec<String>,
        #[serde(default)]
        proto: Option<u32>,
        #[serde(default)]
        app: Option<String>,
    },
    PcTokens {
        #[serde(rename = "allowedTokens")]
        allowed_tokens: Vec<String>,
    },
    PairOpen {
        #[serde(rename = "pairId")]
        pair_id: String,
        #[serde(rename = "manualCode", default)]
        manual_code: Option<String>,
    },
    PairClose {
        #[serde(rename = "pairId")]
        pair_id: String,
    },
    PairExtend {
        #[serde(rename = "pairId")]
        pair_id: String,
    },
    PairMsg {
        #[serde(rename = "pairId")]
        pair_id: String,
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
        data: String,
    },
    KickViewer {
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
        #[serde(default)]
        code: Option<String>,
    },
    PairJoin {
        #[serde(rename = "pairId")]
        pair_id: String,
        data: String,
        #[serde(default)]
        proto: Option<u32>,
    },
    Subscribe {
        pcs: Vec<serde_json::Value>,
        #[serde(default)]
        proto: Option<u32>,
    },
    Unsubscribe {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
    },
    Ping,
}

impl ClientMsg {
    fn may_change_purpose(&self) -> bool {
        matches!(
            self,
            Self::PcTokens { .. } | Self::PairOpen { .. } | Self::PairJoin { .. } | Self::Subscribe { .. }
        )
    }

    fn throttled(&self, role: Role) -> bool {
        match self {
            Self::PcTokens { .. } | Self::PairOpen { .. } => true,
            Self::Subscribe { .. } => !matches!(role, Role::Unregistered),
            _ => false,
        }
    }
}

#[derive(Deserialize)]
struct SubItem {
    #[serde(rename = "pcId")]
    pc_id: Uuid,
    #[serde(rename = "deviceId")]
    device_id: Uuid,
    #[serde(rename = "roomToken")]
    room_token: String,
    #[serde(default)]
    restart: bool,
}

#[derive(Serialize)]
#[serde(tag = "t", rename_all = "snake_case")]
enum ServerMsg<'a> {
    PcReady {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
    },
    PcOnline {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
    },
    PcOffline {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
    },
    PcErr {
        #[serde(rename = "pcId")]
        pc_id: Uuid,
        code: &'a str,
    },
    ViewerJoined {
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
        #[serde(rename = "deviceId")]
        device_id: Uuid,
        #[serde(rename = "tokenHash")]
        token_hash: &'a str,
    },
    ViewerLeft {
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
    },
    PairJoined {
        #[serde(rename = "pairId")]
        pair_id: &'a str,
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
        data: &'a str,
    },
    PairMsg {
        #[serde(rename = "pairId")]
        pair_id: &'a str,
        data: &'a str,
    },
    PairOpened {
        #[serde(rename = "pairId")]
        pair_id: &'a str,
    },
    PairLeft {
        #[serde(rename = "pairId")]
        pair_id: &'a str,
        #[serde(rename = "viewerId")]
        viewer_id: Uuid,
    },
    PcTokensOk,
    PairErr {
        code: &'a str,
    },
    Err {
        code: &'a str,
        #[serde(rename = "pairId", skip_serializing_if = "Option::is_none")]
        pair_id: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        min: Option<&'a str>,
    },
    Pong,
}

fn err(code: &str) -> ServerMsg<'_> {
    ServerMsg::Err { code, pair_id: None, min: None }
}

/// The only pcId a PC may claim: the first 16 bytes of SHA-256 over its secret hash. Knowing a pcId
/// (every paired device does) is then not enough to squat it while the PC is offline.
pub fn pc_id_for_secret_hash(secret_hash_hex: &str) -> Uuid {
    let d = Sha256::digest(secret_hash_hex.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&d[..16]);
    Uuid::from_bytes(b)
}

pub fn sha256_hex(s: &str) -> String {
    hex::encode(Sha256::digest(s.as_bytes()))
}

fn is_valid_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn token_set(raw: Vec<String>) -> HashSet<String> {
    raw.into_iter()
        .take(MAX_TOKENS_PER_PC)
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| is_valid_hex64(t))
        .collect()
}

/// A pairId is 16..=64 lowercase hex characters, so it can never also read as a manual code
/// (at most 12 characters), and a typed code always resolves through `manual_codes`.
fn sanitize_pair_id(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    ((16..=64).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

fn sanitize_manual_code(raw: &str) -> Option<String> {
    let s: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if (6..=12).contains(&s.len()) { Some(s) } else { None }
}

fn message_len(msg: &Message) -> usize {
    match msg {
        Message::Text(t) => t.len(),
        Message::Binary(b) => b.len(),
        Message::Ping(p) | Message::Pong(p) => p.len(),
        Message::Close(_) => 0,
    }
}

struct Meter {
    socket: AtomicUsize,
    socket_limit: usize,
    groups: Vec<Arc<Group>>,
    global: Arc<AtomicUsize>,
    global_limit: usize,
    control: AtomicUsize,
    control_total: Arc<AtomicUsize>,
}

type Rank = [(usize, usize); 5];
type Ranked = Arc<[Arc<Lanes>]>;

impl Meter {
    fn outranks(&self, mine: &Meter) -> bool {
        match self.groups.iter().zip(&mine.groups).find(|(theirs, ours)| !Arc::ptr_eq(theirs, ours)) {
            Some((theirs, ours)) => theirs.queued.load(Ordering::Acquire) >= ours.queued.load(Ordering::Acquire),
            None => self.socket.load(Ordering::Acquire) >= mine.socket.load(Ordering::Acquire),
        }
    }

    fn rank(&self) -> Rank {
        let mut rank = [(0, 0); 5];
        for (slot, group) in rank.iter_mut().zip(&self.groups) {
            *slot = (group.queued.load(Ordering::Acquire), Arc::as_ptr(group) as usize);
        }
        rank[self.groups.len().min(4)] = (self.socket.load(Ordering::Acquire), std::ptr::from_ref(self) as usize);
        rank
    }

    fn charge(self: &Arc<Self>, budget: Budget, cost: usize) -> Charge {
        if budget != Budget::Relay {
            for group in &self.groups {
                group.queued.fetch_add(cost, Ordering::AcqRel);
            }
        }
        if budget != Budget::Routed {
            self.control_total.fetch_add(cost, Ordering::AcqRel);
        }
        Charge { meter: self.clone(), budget, cost, queued_at: Instant::now() }
    }
}

#[derive(Default)]
struct Shedder {
    queues: DashMap<Uuid, Arc<Lanes>>,
    ranking: Mutex<Option<(Instant, Ranked)>>,
    unranked: Arc<AtomicBool>,
}

impl Shedder {
    fn ranked(&self, rebuild: bool) -> Ranked {
        let mut ranking = self.ranking.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        if let Some((at, order)) = ranking.as_ref()
            && !rebuild
            && now.duration_since(*at) < RANK_REFRESH
        {
            return order.clone();
        }
        self.unranked.store(false, Ordering::Release);
        let mut ranked: Vec<(Rank, Arc<Lanes>)> = self
            .queues
            .iter()
            .filter(|q| !q.killed.load(Ordering::Acquire) && q.sheddable.load(Ordering::Acquire) > 0)
            .map(|q| (q.meter.rank(), q.clone()))
            .collect();
        ranked.sort_unstable_by_key(|(rank, _)| std::cmp::Reverse(*rank));
        let order: Ranked = ranked.into_iter().map(|(_, lanes)| lanes).collect();
        *ranking = Some((now, order.clone()));
        order
    }

    fn make_room(&self, mine: &Lanes, cost: usize) -> bool {
        self.shed_from(&self.ranked(false), mine, cost) || (self.unranked.load(Ordering::Acquire) && self.shed_from(&self.ranked(true), mine, cost))
    }

    fn shed_from(&self, order: &Ranked, mine: &Lanes, cost: usize) -> bool {
        let victims: Vec<&Arc<Lanes>> = order
            .iter()
            .filter(|theirs| {
                !std::ptr::eq(theirs.as_ref(), mine) && !theirs.killed.load(Ordering::Acquire) && theirs.sheddable.load(Ordering::Acquire) > 0
            })
            .take(SHED_TRIES)
            .filter(|theirs| theirs.meter.outranks(&mine.meter))
            .collect();
        if victims.iter().map(|v| v.sheddable.load(Ordering::Acquire)).sum::<usize>() < cost {
            return false;
        }
        let target = (cost * 4).min(SHED_CHUNK).max(cost);
        let mut freed = 0;
        for victim in victims {
            freed += victim.shed(target - freed);
            if freed >= cost {
                break;
            }
        }
        freed >= cost
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Budget {
    Relay,
    Payload,
    Routed,
}

struct Charge {
    meter: Arc<Meter>,
    budget: Budget,
    cost: usize,
    queued_at: Instant,
}

impl Drop for Charge {
    fn drop(&mut self) {
        let meter = &self.meter;
        if self.budget != Budget::Relay {
            meter.socket.fetch_sub(self.cost, Ordering::AcqRel);
            for group in &meter.groups {
                group.queued.fetch_sub(self.cost, Ordering::AcqRel);
            }
        }
        if self.budget == Budget::Routed {
            meter.global.fetch_sub(self.cost, Ordering::AcqRel);
        } else {
            meter.control.fetch_sub(self.cost, Ordering::AcqRel);
            meter.control_total.fetch_sub(self.cost, Ordering::AcqRel);
        }
    }
}

fn reserve(counter: &AtomicUsize, cost: usize, limit: usize) -> bool {
    counter
        .try_update(Ordering::AcqRel, Ordering::Acquire, |q| q.checked_add(cost).filter(|n| *n <= limit))
        .is_ok()
}

type Queued = (Message, Charge);

struct Stream<T> {
    bytes: usize,
    items: VecDeque<(usize, T)>,
}

struct Streams<T> {
    closed: bool,
    turn: VecDeque<Uuid>,
    queues: HashMap<Uuid, Stream<T>>,
}

impl<T> Default for Streams<T> {
    fn default() -> Self {
        Self { closed: false, turn: VecDeque::new(), queues: HashMap::new() }
    }
}

impl<T> Streams<T> {
    fn push(&mut self, peer: Uuid, item: T, cost: usize) -> bool {
        if self.closed {
            return false;
        }
        let stream = self.queues.entry(peer).or_insert_with(|| Stream { bytes: 0, items: VecDeque::new() });
        if stream.items.is_empty() {
            self.turn.push_back(peer);
        }
        stream.bytes += cost;
        stream.items.push_back((cost, item));
        true
    }

    fn pop(&mut self) -> Option<(usize, T)> {
        let peer = self.turn.pop_front()?;
        let stream = self.queues.get_mut(&peer)?;
        let item = stream.items.pop_front();
        if let Some((cost, _)) = &item {
            stream.bytes -= cost;
        }
        if stream.items.is_empty() {
            self.queues.remove(&peer);
        } else {
            self.turn.push_back(peer);
        }
        item
    }

    fn bytes(&self, peer: &Uuid) -> usize {
        self.queues.get(peer).map_or(0, |stream| stream.bytes)
    }

    fn largest(&self, except: Option<Uuid>) -> Option<(Uuid, usize)> {
        self.queues
            .iter()
            .filter(|(peer, stream)| stream.bytes > 0 && except != Some(**peer))
            .map(|(peer, stream)| (*peer, stream.bytes))
            .max_by_key(|(_, bytes)| *bytes)
    }

    fn shed_from(&mut self, peer: Uuid, target: usize) -> usize {
        let Some(stream) = self.queues.get_mut(&peer) else {
            return 0;
        };
        let mut kept = Vec::new();
        let mut freed = 0;
        while freed < target
            && stream.bytes > 0
            && let Some((cost, item)) = stream.items.pop_front()
        {
            if cost == 0 {
                kept.push((cost, item));
            } else {
                freed += cost;
                stream.bytes -= cost;
            }
        }
        for item in kept.into_iter().rev() {
            stream.items.push_front(item);
        }
        if stream.items.is_empty() {
            self.queues.remove(&peer);
            self.turn.retain(|p| *p != peer);
        }
        freed
    }

    fn close(&mut self) {
        self.closed = true;
        self.turn.clear();
        self.queues.clear();
    }
}

fn frame_cost(item: &Queued) -> usize {
    if item.1.budget == Budget::Routed && matches!(item.0, Message::Binary(_)) { item.1.cost } else { 0 }
}

struct Lanes {
    streams: Mutex<Streams<Queued>>,
    sheddable: AtomicUsize,
    ready: Notify,
    purpose: Notify,
    meter: Arc<Meter>,
    killed: AtomicBool,
    stop: Notify,
    seat: Arc<Seat>,
    opened: Instant,
    heard: AtomicU64,
    unranked: Arc<AtomicBool>,
}

impl Lanes {
    fn streams(&self) -> std::sync::MutexGuard<'_, Streams<Queued>> {
        self.streams.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn queue(&self, peer: Uuid, item: Queued) -> bool {
        let cost = frame_cost(&item);
        let mut streams = self.streams();
        let queued = streams.push(peer, item, cost);
        if queued && self.sheddable.fetch_add(cost, Ordering::AcqRel) == 0 {
            self.unranked.store(true, Ordering::Release);
        }
        queued
    }

    fn pop(&self) -> Option<Queued> {
        let mut streams = self.streams();
        let (cost, item) = streams.pop()?;
        self.sheddable.fetch_sub(cost, Ordering::AcqRel);
        Some(item)
    }

    fn shed(&self, target: usize) -> usize {
        let mut streams = self.streams();
        let mut freed = 0;
        while freed < target
            && let Some((peer, _)) = streams.largest(None)
        {
            freed += streams.shed_from(peer, target - freed);
        }
        self.sheddable.fetch_sub(freed, Ordering::AcqRel);
        freed
    }

    fn shed_for(&self, peer: Uuid, cost: usize) -> bool {
        let mut streams = self.streams();
        let mine = streams.bytes(&peer);
        let Some((other, bytes)) = streams.largest(Some(peer)) else {
            return false;
        };
        if bytes <= mine + cost {
            return false;
        }
        let freed = streams.shed_from(other, cost);
        self.sheddable.fetch_sub(freed, Ordering::AcqRel);
        freed > 0
    }

    fn close(&self) {
        let mut streams = self.streams();
        streams.close();
        self.sheddable.store(0, Ordering::Release);
    }

    fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        self.close();
        self.seat.release();
        self.stop.notify_one();
    }

    fn hear(&self) {
        self.heard.store(u64::try_from(self.opened.elapsed().as_millis()).unwrap_or(u64::MAX), Ordering::Release);
    }

    fn silent_for(&self) -> Duration {
        self.opened.elapsed().saturating_sub(Duration::from_millis(self.heard.load(Ordering::Acquire)))
    }
}

struct Inbox {
    rx: mpsc::UnboundedReceiver<Queued>,
    lanes: Arc<Lanes>,
    shedder: Arc<Shedder>,
    key: Uuid,
}

impl Inbox {
    async fn next(&mut self) -> Option<(Message, Instant, bool)> {
        let ((msg, charge), from_peer) = self.pop().await?;
        Some((msg, charge.queued_at, from_peer))
    }

    async fn pop(&mut self) -> Option<(Queued, bool)> {
        loop {
            match self.rx.try_recv() {
                Ok(item) => return Some((item, false)),
                Err(TryRecvError::Disconnected) => return None,
                Err(TryRecvError::Empty) => {}
            }
            if let Some(item) = self.lanes.pop() {
                return Some((item, true));
            }
            tokio::select! {
                biased;
                item = self.rx.recv() => return item.map(|item| (item, false)),
                () = self.lanes.ready.notified() => {}
            }
        }
    }
}

impl Drop for Inbox {
    fn drop(&mut self) {
        self.lanes.close();
        self.shedder.queues.remove(&self.key);
    }
}

#[derive(Clone)]
struct Outbox {
    tx: mpsc::UnboundedSender<Queued>,
    lanes: Arc<Lanes>,
    shedder: Arc<Shedder>,
}

impl Outbox {
    fn new(state: &RelayState, seat: Arc<Seat>) -> (Self, Inbox) {
        let (tx, rx) = mpsc::unbounded_channel();
        let cfg = &state.config;
        let meter = Meter {
            socket: AtomicUsize::new(0),
            socket_limit: cfg.socket_queue_budget,
            groups: seat.groups.iter().rev().cloned().collect(),
            global: state.queued_bytes.clone(),
            global_limit: cfg.global_queue_budget,
            control: AtomicUsize::new(0),
            control_total: state.control_bytes.clone(),
        };
        let lanes = Arc::new(Lanes {
            streams: Mutex::new(Streams::default()),
            sheddable: AtomicUsize::new(0),
            ready: Notify::new(),
            purpose: Notify::new(),
            meter: Arc::new(meter),
            killed: AtomicBool::new(false),
            stop: Notify::new(),
            seat,
            opened: Instant::now(),
            heard: AtomicU64::new(0),
            unranked: state.shedder.unranked.clone(),
        });
        let key = Uuid::new_v4();
        state.shedder.queues.insert(key, lanes.clone());
        let shedder = state.shedder.clone();
        (Self { tx, lanes: lanes.clone(), shedder: shedder.clone() }, Inbox { rx, lanes, shedder, key })
    }

    fn push(&self, msg: Message, peer: Option<Uuid>) -> bool {
        let cost = message_len(&msg) + MESSAGE_OVERHEAD;
        let meter = &self.lanes.meter;
        let socket = || reserve(&meter.socket, cost, meter.socket_limit);
        if !(socket() || peer.is_some_and(|peer| self.lanes.shed_for(peer, cost) && socket())) {
            return false;
        }
        let global = || reserve(&meter.global, cost, meter.global_limit);
        if !(global() || (peer.is_some() && self.shedder.make_room(&self.lanes, cost) && global())) {
            meter.socket.fetch_sub(cost, Ordering::AcqRel);
            return false;
        }
        self.enqueue(msg, meter.charge(Budget::Routed, cost), peer)
    }

    fn push_control(&self, msg: Message, peer: Option<Uuid>, attributed: bool) -> bool {
        let cost = message_len(&msg) + MESSAGE_OVERHEAD;
        let meter = &self.lanes.meter;
        if !reserve(&meter.control, cost, CONTROL_ALLOWANCE) {
            return false;
        }
        if attributed && !reserve(&meter.socket, cost, meter.socket_limit) {
            meter.control.fetch_sub(cost, Ordering::AcqRel);
            return false;
        }
        let budget = if attributed { Budget::Payload } else { Budget::Relay };
        self.enqueue(msg, meter.charge(budget, cost), peer)
    }

    fn enqueue(&self, msg: Message, charge: Charge, peer: Option<Uuid>) -> bool {
        let Some(peer) = peer else {
            return self.tx.send((msg, charge)).is_ok();
        };
        let queued = self.lanes.queue(peer, (msg, charge));
        if queued {
            self.lanes.ready.notify_one();
        }
        queued
    }

    fn routed(&self, peer: Uuid, msg: Message) -> bool {
        self.push(msg, Some(peer))
    }

    fn control(&self, msg: Message) {
        self.push_control(msg, None, false);
    }

    fn json(&self, msg: &ServerMsg<'_>) {
        if let Ok(text) = serde_json::to_string(msg) {
            self.control(Message::Text(text.into()));
        }
    }

    fn payload(&self, msg: &ServerMsg<'_>) {
        if let Ok(text) = serde_json::to_string(msg) {
            self.push_control(Message::Text(text.into()), None, true);
        }
    }

    fn about(&self, peer: Uuid, msg: &ServerMsg<'_>) {
        if let Ok(text) = serde_json::to_string(msg) {
            self.push_control(Message::Text(text.into()), Some(peer), false);
        }
        self.lanes.purpose.notify_one();
    }

    fn notice(&self, code: &str) {
        self.json(&err(code));
    }

    fn close(&self) {
        self.control(Message::Close(None));
    }
}

async fn write_loop(mut sink: SplitSink<WebSocket, Message>, mut inbox: Inbox, write_timeout: Duration, queue_age_limit: Duration) {
    let lanes = inbox.lanes.clone();
    let pump = async {
        while let Some((msg, queued_at, from_peer)) = inbox.next().await {
            if queued_at.elapsed() > queue_age_limit {
                if from_peer {
                    continue;
                }
                debug!("closing a socket that reads too slowly");
                break;
            }
            let closing = matches!(msg, Message::Close(_));
            let sent = tokio::time::timeout(write_timeout, sink.send(msg)).await;
            if closing || !matches!(sent, Ok(Ok(()))) {
                break;
            }
        }
    };
    let shed = tokio::select! {
        () = pump => false,
        () = lanes.stop.notified() => true,
    };
    drop(inbox);
    if !shed {
        let _ = tokio::time::timeout(write_timeout, sink.close()).await;
    }
}

struct Bucket {
    limit: RateLimit,
    bytes: f64,
    msgs: f64,
    last: Instant,
}

impl Bucket {
    fn new(limit: RateLimit) -> Self {
        Self { limit, bytes: limit.burst_bytes, msgs: limit.burst_msgs, last: Instant::now() }
    }

    fn take(&mut self, bytes: usize) -> bool {
        self.take_at(bytes, Instant::now())
    }

    fn refill(&mut self, now: Instant) {
        let dt = now.saturating_duration_since(self.last).as_secs_f64();
        self.last = now;
        self.bytes = (self.bytes + dt * self.limit.bytes_per_sec).min(self.limit.burst_bytes);
        self.msgs = (self.msgs + dt * self.limit.msgs_per_sec).min(self.limit.burst_msgs);
    }

    fn take_at(&mut self, bytes: usize, now: Instant) -> bool {
        self.refill(now);
        let bytes = (bytes as f64).min(self.limit.burst_bytes);
        if self.bytes < bytes || self.msgs < 1.0 {
            return false;
        }
        self.bytes -= bytes;
        self.msgs -= 1.0;
        true
    }

    /// How long until `take_at(bytes)` can succeed.
    fn wait_at(&mut self, bytes: usize, now: Instant) -> Duration {
        self.refill(now);
        let bytes = (bytes as f64).min(self.limit.burst_bytes);
        let for_bytes = (bytes - self.bytes).max(0.0) / self.limit.bytes_per_sec;
        let for_msgs = (1.0 - self.msgs).max(0.0) / self.limit.msgs_per_sec;
        Duration::from_secs_f64(for_bytes.max(for_msgs).min(MAX_THROTTLE_STEP.as_secs_f64()))
    }

    fn charge_at(&mut self, bytes: usize, now: Instant) {
        self.refill(now);
        self.bytes = (self.bytes - bytes as f64).max(-self.limit.burst_bytes);
        self.msgs = (self.msgs - 1.0).max(-self.limit.burst_msgs);
    }
}

struct Decay {
    value: f64,
    at: Instant,
}

impl Default for Decay {
    fn default() -> Self {
        Self { value: 0.0, at: Instant::now() }
    }
}

impl Decay {
    fn at(&mut self, now: Instant) -> f64 {
        let dt = now.saturating_duration_since(self.at).as_secs_f64();
        self.at = now;
        self.value *= (-dt).exp();
        self.value
    }

    fn add(&mut self, amount: f64, now: Instant) {
        self.at(now);
        self.value += amount;
    }
}

fn fair_share(rate: f64, siblings: usize, limit: f64) -> bool {
    rate * siblings.max(1) as f64 <= limit
}

struct Inflow {
    bucket: Bucket,
    recent: Decay,
    children: usize,
}

impl Inflow {
    fn new(limit: RateLimit) -> Self {
        Self { bucket: Bucket::new(limit), recent: Decay::default(), children: 0 }
    }

    fn recent_at(&mut self, now: Instant) -> f64 {
        self.recent.at(now)
    }

    fn charge_at(&mut self, bytes: usize, now: Instant) {
        self.recent.add(bytes as f64, now);
        self.bucket.charge_at(bytes, now);
    }

    fn fair_for(&self, child: f64) -> bool {
        fair_share(child, self.children, self.bucket.limit.bytes_per_sec)
    }
}

fn connects_spent(known: &[Option<Arc<Group>>], now: Instant) -> bool {
    let mut child: Option<Option<f64>> = None;
    for group in known {
        let Some(group) = group else {
            child = Some(None);
            continue;
        };
        let siblings = group.rate().children;
        let mut slot = group.slot();
        let limit = slot.connects.limit.msgs_per_sec;
        let fair = match child {
            None => false,
            Some(Some(tried)) => fair_share(tried, siblings, limit),
            Some(None) => fair_share(slot.newcomers.at(now), siblings + 1, limit),
        };
        if !fair && !slot.connects.wait_at(0, now).is_zero() {
            return true;
        }
        child = Some(Some(slot.tried.at(now)));
    }
    false
}

fn inflow_wait(groups: &[Arc<Group>], len: usize, now: Instant) -> Duration {
    let mut child = None;
    for group in groups {
        let mut rate = group.rate();
        if !child.is_some_and(|recent| rate.fair_for(recent)) {
            let wait = rate.bucket.wait_at(len, now);
            if !wait.is_zero() {
                return wait;
            }
        }
        child = Some(rate.recent_at(now));
    }
    Duration::ZERO
}

fn inflow_charge(groups: &[Arc<Group>], len: usize, now: Instant) {
    for group in groups {
        group.rate().charge_at(len, now);
    }
}

#[derive(Clone, Copy)]
enum Role {
    Unregistered,
    Pc { pc_id: Uuid },
    Viewer,
}

#[derive(PartialEq, Eq)]
enum Flow {
    Continue,
    Close,
}

struct Conn {
    id: Uuid,
    role: Role,
    outbox: Outbox,
    ip: IpAddr,
    prefix: Option<IpAddr>,
    net: IpAddr,
    bucket: Bucket,
    groups: Vec<Arc<Group>>,
    control: Bucket,
    last_rate_err: Option<Instant>,
    misroutes: (Instant, u32),
    open_pair: Option<String>,
    joined_rooms: HashSet<String>,
}

async fn handle_socket(socket: WebSocket, state: RelayState, guard: ConnGuard) {
    let cfg = state.config.clone();
    let (sink, mut stream) = socket.split();
    let (outbox, rx) = Outbox::new(&state, guard.seat.clone());
    let lanes = outbox.lanes.clone();
    let mut writer = tokio::spawn(write_loop(sink, rx, cfg.write_timeout, cfg.queue_age_limit));
    let opened = Instant::now();
    let mut conn = Conn {
        id: Uuid::new_v4(),
        role: Role::Unregistered,
        outbox,
        ip: guard.ip,
        prefix: guard.prefix,
        net: guard.net,
        bucket: Bucket::new(cfg.viewer_rate),
        groups: guard.seat.groups.clone(),
        control: Bucket::new(cfg.control_rate),
        last_rate_err: None,
        misroutes: (opened, 0),
        open_pair: None,
        joined_rooms: HashSet::new(),
    };
    let mut last_inbound = opened;
    let mut purposeless_since: Option<Instant> = None;
    let mut purposeful: Option<bool> = None;
    let mut ping = tokio::time::interval_at((opened + cfg.ping_interval).into(), cfg.ping_interval);
    let mut writer_done = false;

    loop {
        let mut deadline = last_inbound + cfg.idle_timeout;
        match conn.role {
            Role::Unregistered => deadline = deadline.min(opened + cfg.registration_timeout),
            Role::Viewer | Role::Pc { .. } if !*purposeful.get_or_insert_with(|| conn.has_purpose(&state)) => {
                let since = *purposeless_since.get_or_insert_with(Instant::now);
                deadline = deadline.min(since + cfg.purposeless_timeout);
            }
            _ => purposeless_since = None,
        }
        tokio::select! {
            _ = &mut writer => {
                writer_done = true;
                break;
            }
            _ = ping.tick() => {
                conn.outbox.control(Message::Ping(Bytes::new()));
                purposeful = None;
            }
            () = lanes.purpose.notified() => purposeful = None,
            () = tokio::time::sleep_until(deadline.into()) => {
                let idle = Instant::now() >= last_inbound + cfg.idle_timeout;
                if idle || matches!(conn.role, Role::Unregistered) {
                    break;
                }
                if purposeless_since.is_some() && !conn.has_purpose(&state) {
                    conn.outbox.json(&err("no_purpose"));
                    break;
                }
                purposeless_since = None;
                purposeful = None;
            }
            next = stream.next() => {
                let Some(Ok(msg)) = next else { break };
                // Only whole data messages count: control frames may be interleaved with an
                // unfinished fragmented message, which must not keep a socket alive.
                if matches!(msg, Message::Text(_) | Message::Binary(_)) {
                    last_inbound = Instant::now();
                    lanes.hear();
                }
                conn.throttle(message_len(&msg)).await;
                if conn.on_message(&state, msg) == Flow::Close {
                    break;
                }
            }
        }
    }

    cleanup(&state, &conn);
    conn.outbox.close();
    drop(conn);
    if !writer_done && tokio::time::timeout(cfg.write_timeout, &mut writer).await.is_err() {
        writer.abort();
    }
    drop(guard);
}

impl Conn {
    /// Waits until this socket's and its address groups' budgets cover a message, then charges
    /// them. The stream isn't read meanwhile, so TCP slows the sender down to its budget.
    async fn throttle(&mut self, len: usize) {
        loop {
            let now = Instant::now();
            let mut wait = self.bucket.wait_at(len, now);
            if wait.is_zero() {
                wait = inflow_wait(&self.groups, len, now);
            }
            if wait.is_zero() {
                inflow_charge(&self.groups, len, now);
                self.bucket.take_at(len, now);
                return;
            }
            self.rate_limited(now);
            tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
        }
    }

    fn rate_limited(&mut self, now: Instant) {
        if self.last_rate_err.is_none_or(|t| now.duration_since(t) >= RATE_ERR_INTERVAL) {
            self.last_rate_err = Some(now);
            self.outbox.notice("rate_limited");
        }
    }

    fn control_allowed(&mut self) -> bool {
        let ok = self.control.take(0);
        if !ok {
            self.rate_limited(Instant::now());
        }
        ok
    }

    fn has_purpose(&self, state: &RelayState) -> bool {
        let room_held = self.open_pair.as_ref().is_some_and(|id| state.pair_rooms.get(id).is_some_and(|r| r.pc_conn_id == self.id));
        if let Role::Pc { pc_id } = self.role {
            return room_held || state.pcs.get(&pc_id).is_some_and(|pc| pc.conn_id == self.id && !pc.allowed_tokens.is_empty());
        }
        let subscribed = state.viewers.get(&self.id).is_some_and(|v| {
            v.subscriptions.iter().any(|(pc_id, s)| {
                state.pcs.get(pc_id).is_some_and(|pc| {
                    pc.attached_viewers.contains_key(&self.id) || (!s.kicked && pc.allowed_tokens.contains(&s.token_hash))
                })
            })
        });
        subscribed || self.joined_rooms.iter().any(|id| state.pair_rooms.get(id).is_some_and(|r| r.viewer_id == Some(self.id)))
    }

    /// A viewer's frame for a target it isn't attached to: a few happen when a PC just left, a
    /// steady stream is a client probing, which ends the socket. A PC's frames for viewers that
    /// left are only dropped: any device can attach and leave at will, and must not get the PC
    /// disconnected that way.
    fn misrouted(&mut self) -> Flow {
        let now = Instant::now();
        if now.duration_since(self.misroutes.0) >= MISROUTE_WINDOW {
            self.misroutes = (now, 0);
        }
        self.misroutes.1 += 1;
        if self.misroutes.1 > MISROUTE_MAX { Flow::Close } else { Flow::Continue }
    }

    fn on_message(&mut self, state: &RelayState, msg: Message) -> Flow {
        match msg {
            Message::Text(text) => self.on_text(state, &text),
            Message::Binary(bytes) => self.on_binary(state, &bytes),
            Message::Close(_) => Flow::Close,
            Message::Ping(_) | Message::Pong(_) => Flow::Continue,
        }
    }

    fn on_text(&mut self, state: &RelayState, text: &str) -> Flow {
        if text.len() > MAX_TEXT_FRAME {
            self.outbox.notice("frame_too_large");
            return Flow::Continue;
        }
        match serde_json::from_str::<ClientMsg>(text) {
            Ok(msg) => self.on_control(state, msg),
            Err(_) => {
                self.outbox.notice("bad_json");
                Flow::Continue
            }
        }
    }

    fn on_binary(&mut self, state: &RelayState, bytes: &[u8]) -> Flow {
        let max = match self.role {
            Role::Unregistered => return Flow::Close,
            Role::Pc { .. } => MAX_PC_FRAME,
            Role::Viewer => MAX_VIEWER_FRAME,
        };
        if bytes.len() > max {
            self.outbox.notice("frame_too_large");
            return Flow::Continue;
        }
        if bytes.len() < BIN_HEADER_LEN {
            return Flow::Continue;
        }
        if bytes[0] != BIN_VERSION {
            self.outbox.notice("bad_version");
            return Flow::Continue;
        }
        let Ok(target) = Uuid::from_slice(&bytes[1..BIN_HEADER_LEN]) else {
            return Flow::Continue;
        };
        let routed = match self.role {
            Role::Pc { pc_id } => route_to_viewer(state, pc_id, self.id, target, bytes),
            Role::Viewer => route_to_pc(state, self.id, target, bytes),
            Role::Unregistered => false,
        };
        if routed || matches!(self.role, Role::Pc { .. }) { Flow::Continue } else { self.misrouted() }
    }

    fn proto_ok(&self, proto: Option<u32>) -> bool {
        let ok = proto.is_some_and(|p| p >= MIN_PROTO);
        if !ok {
            self.outbox.json(&err("upgrade_required"));
        }
        ok
    }

    fn ensure_viewer(&mut self, state: &RelayState) {
        if matches!(self.role, Role::Unregistered) {
            state
                .viewers
                .insert(self.id, ViewerEntry { outbox: self.outbox.clone(), subscriptions: HashMap::new(), refused_since: HashMap::new() });
            self.role = Role::Viewer;
        }
    }

    fn on_control(&mut self, state: &RelayState, msg: ClientMsg) -> Flow {
        if msg.throttled(self.role) && !self.control_allowed() {
            return Flow::Continue;
        }
        if msg.may_change_purpose() {
            self.outbox.lanes.purpose.notify_one();
        }
        match msg {
            ClientMsg::Ping => {
                self.outbox.json(&ServerMsg::Pong);
                Flow::Continue
            }
            ClientMsg::PcHello { pc_id, pc_secret_hash, allowed_tokens, proto, app } => {
                self.pc_hello(state, pc_id, pc_secret_hash, allowed_tokens, proto, app.as_deref())
            }
            ClientMsg::PcTokens { allowed_tokens } => self.pc_tokens(state, allowed_tokens),
            ClientMsg::PairOpen { pair_id, manual_code } => self.pair_open(state, &pair_id, manual_code.as_deref()),
            ClientMsg::PairClose { pair_id } => {
                if matches!(self.role, Role::Pc { .. })
                    && let Some(pid) = sanitize_pair_id(&pair_id)
                {
                    remove_owned_pair_room(state, &pid, self.id);
                    if self.open_pair.as_deref() == Some(pid.as_str()) {
                        self.open_pair = None;
                        self.outbox.lanes.purpose.notify_one();
                    }
                }
                Flow::Continue
            }
            ClientMsg::PairMsg { pair_id, viewer_id, data } => {
                let Role::Pc { pc_id } = self.role else {
                    return Flow::Continue;
                };
                let Some(pid) = sanitize_pair_id(&pair_id) else {
                    return Flow::Continue;
                };
                if data.len() > MAX_PAIR_DATA {
                    self.outbox.notice("frame_too_large");
                    return Flow::Continue;
                }
                let allowed = state
                    .pair_rooms
                    .get(&pid)
                    .is_some_and(|r| r.pc_id == pc_id && r.pc_conn_id == self.id && r.viewer_id == Some(viewer_id));
                if allowed && let Some(viewer_out) = state.viewers.get(&viewer_id).map(|v| v.outbox.clone()) {
                    viewer_out.payload(&ServerMsg::PairMsg { pair_id: &pid, data: &data });
                }
                Flow::Continue
            }
            ClientMsg::PairExtend { pair_id } => {
                if matches!(self.role, Role::Pc { .. })
                    && let Some(pid) = sanitize_pair_id(&pair_id)
                {
                    extend_pair_room(state, &pid, self.id);
                }
                Flow::Continue
            }
            ClientMsg::KickViewer { viewer_id, code } => {
                let Role::Pc { pc_id } = self.role else {
                    return Flow::Continue;
                };
                let removed = state.pcs.get_mut(&pc_id).is_some_and(|mut pc| {
                    pc.conn_id == self.id && pc.attached_viewers.remove(&viewer_id).is_some()
                });
                if removed {
                    mark_kicked(state, viewer_id, pc_id);
                    let reason = KICK_REASONS.into_iter().find(|r| code.as_deref() == Some(*r));
                    if let Some(viewer_out) = state.viewers.get(&viewer_id).map(|v| v.outbox.clone()) {
                        viewer_out.about(pc_id, &reason.map_or(ServerMsg::PcOffline { pc_id }, |code| ServerMsg::PcErr { pc_id, code }));
                    }
                    self.outbox.about(viewer_id, &ServerMsg::ViewerLeft { viewer_id });
                    admit_waiting(state, pc_id);
                }
                Flow::Continue
            }
            ClientMsg::PairJoin { pair_id, data, proto } => self.pair_join(state, &pair_id, &data, proto),
            ClientMsg::Subscribe { pcs, proto } => self.subscribe(state, pcs, proto),
            ClientMsg::Unsubscribe { pc_id } => {
                let subscribed = state.viewers.get_mut(&self.id).is_some_and(|mut v| v.subscriptions.remove(&pc_id).is_some());
                if subscribed {
                    self.outbox.lanes.purpose.notify_one();
                    unindex_subscriber(state, pc_id, self.id);
                    detach_viewer_from_pc(state, self.id, pc_id);
                }
                Flow::Continue
            }
        }
    }

    fn pc_hello(
        &mut self,
        state: &RelayState,
        pc_id: Uuid,
        pc_secret_hash: String,
        allowed_tokens: Vec<String>,
        proto: Option<u32>,
        app: Option<&str>,
    ) -> Flow {
        if !matches!(self.role, Role::Unregistered) {
            self.outbox.json(&err("already_registered"));
            return Flow::Continue;
        }
        if !self.proto_ok(proto) {
            return Flow::Close;
        }
        if let Some(min) = state.config.min_app_version
            && app.and_then(parse_version).is_none_or(|v| v < min)
        {
            let min = format!("{}.{}.{}", min.0, min.1, min.2);
            self.outbox.json(&ServerMsg::Err { code: "upgrade_required", pair_id: None, min: Some(&min) });
            return Flow::Close;
        }
        let secret_hash = pc_secret_hash.to_ascii_lowercase();
        if !is_valid_hex64(&secret_hash) {
            self.outbox.json(&err("bad_secret_hash"));
            return Flow::Close;
        }
        if pc_id != pc_id_for_secret_hash(&secret_hash) {
            self.outbox.json(&err("forbidden"));
            return Flow::Close;
        }
        let entry = PcEntry {
            conn_id: self.id,
            secret_hash,
            allowed_tokens: token_set(allowed_tokens),
            outbox: self.outbox.clone(),
            attached_viewers: HashMap::new(),
        };
        match state.pcs.entry(pc_id) {
            Entry::Occupied(mut e) => {
                if e.get().secret_hash != entry.secret_hash {
                    self.outbox.json(&err("forbidden"));
                    return Flow::Close;
                }
                e.insert(entry).outbox.close();
            }
            Entry::Vacant(e) => {
                e.insert(entry);
            }
        }
        self.role = Role::Pc { pc_id };
        self.outbox.lanes.purpose.notify_one();
        self.bucket = Bucket::new(state.config.pc_rate);
        self.outbox.json(&ServerMsg::PcReady { pc_id });
        for viewer_id in subscribers_of(state, pc_id) {
            if let Some(mut v) = state.viewers.get_mut(&viewer_id)
                && let Some(sub) = v.subscriptions.get_mut(&pc_id)
            {
                sub.kicked = false;
            }
        }
        admit_waiting(state, pc_id);
        debug!(%pc_id, conn_id = %self.id, "PC registered");
        Flow::Continue
    }

    fn pc_tokens(&mut self, state: &RelayState, allowed_tokens: Vec<String>) -> Flow {
        let Role::Pc { pc_id } = self.role else {
            self.outbox.json(&err("not_pc"));
            return Flow::Continue;
        };
        let attached: Vec<Uuid> = {
            let Some(mut pc) = state.pcs.get_mut(&pc_id) else {
                return Flow::Continue;
            };
            if pc.conn_id != self.id {
                return Flow::Close;
            }
            pc.allowed_tokens = token_set(allowed_tokens);
            pc.attached_viewers.keys().copied().collect()
        };
        for viewer_id in attached {
            reconcile(state, pc_id, viewer_id);
        }
        admit_waiting(state, pc_id);
        self.outbox.json(&ServerMsg::PcTokensOk);
        Flow::Continue
    }

    fn pair_open(&mut self, state: &RelayState, pair_id: &str, manual_code: Option<&str>) -> Flow {
        let Role::Pc { pc_id } = self.role else {
            self.outbox.json(&err("not_pc"));
            return Flow::Continue;
        };
        let Some(pair_id) = sanitize_pair_id(pair_id) else {
            self.outbox.json(&ServerMsg::PairErr { code: "bad_pair_id" });
            return Flow::Continue;
        };
        if let Some(previous) = self.open_pair.take() {
            remove_owned_pair_room(state, &previous, self.id);
        }
        let manual = manual_code.and_then(sanitize_manual_code);
        if open_pair_room(state, pc_id, self.id, &pair_id, manual) {
            self.outbox.json(&ServerMsg::PairOpened { pair_id: &pair_id });
            self.open_pair = Some(pair_id);
        } else {
            self.outbox.json(&ServerMsg::Err { code: "pair_taken", pair_id: Some(&pair_id), min: None });
        }
        Flow::Continue
    }

    fn pair_join(&mut self, state: &RelayState, pair_id: &str, data: &str, proto: Option<u32>) -> Flow {
        if matches!(self.role, Role::Pc { .. }) {
            self.outbox.json(&err("wrong_role"));
            return Flow::Continue;
        }
        if !self.proto_ok(proto) {
            return Flow::Close;
        }
        if data.len() > MAX_PAIR_DATA {
            self.outbox.notice("frame_too_large");
            return Flow::Continue;
        }
        let (found, manual) = lookup_pair(state, pair_id);
        let rejoin = found.as_ref().is_some_and(|id| state.pair_rooms.get(id).is_some_and(|r| r.viewer_id == Some(self.id)));
        if rejoin && !self.control_allowed() {
            return Flow::Continue;
        }
        if !rejoin && (!state.check_pair_rate(self.ip, self.prefix) || (manual && !state.manual_guesses_left(self.net))) {
            self.outbox.json(&ServerMsg::PairErr { code: "rate_limited" });
            return Flow::Close;
        }
        self.ensure_viewer(state);

        let Some(room_id) = found else {
            if manual {
                state.note_manual_miss(self.net);
            }
            self.outbox.json(&ServerMsg::PairErr { code: "not_found" });
            return Flow::Continue;
        };
        let (pc_id, contended) = {
            let Some(mut room) = state.pair_rooms.get_mut(&room_id) else {
                self.outbox.json(&ServerMsg::PairErr { code: "not_found" });
                return Flow::Continue;
            };
            if room.expires_at <= Instant::now() {
                drop(room);
                remove_pair_room(state, &room_id);
                self.outbox.json(&ServerMsg::PairErr { code: "expired" });
                return Flow::Continue;
            }
            let contended = room.viewer_id.is_some_and(|v| v != self.id);
            if !contended {
                if room.viewer_id.is_none() {
                    room.joins = 0;
                }
                if room.joins >= MAX_ROOM_JOINS {
                    drop(room);
                    self.outbox.json(&ServerMsg::PairErr { code: "busy" });
                    return Flow::Continue;
                }
                room.joins += 1;
                room.viewer_id = Some(self.id);
            }
            (room.pc_id, contended)
        };
        if !contended {
            self.joined_rooms.retain(|id| state.pair_rooms.contains_key(id));
            self.joined_rooms.insert(room_id.clone());
        }

        match state.pcs.get(&pc_id).map(|pc| pc.outbox.clone()) {
            Some(pc_out) => pc_out.payload(&ServerMsg::PairJoined { pair_id: &room_id, viewer_id: self.id, data }),
            None => self.outbox.json(&ServerMsg::PairErr { code: "not_found" }),
        }
        if contended {
            self.outbox.json(&ServerMsg::PairErr { code: "busy" });
        }
        Flow::Continue
    }

    fn subscribe(&mut self, state: &RelayState, items: Vec<serde_json::Value>, proto: Option<u32>) -> Flow {
        if matches!(self.role, Role::Pc { .. }) {
            self.outbox.json(&err("wrong_role"));
            return Flow::Continue;
        }
        if !self.proto_ok(proto) {
            return Flow::Close;
        }
        self.ensure_viewer(state);

        let (mut subs, refused) = parse_subscriptions(items);
        let mut old = Vec::new();
        if let Some(mut v) = state.viewers.get_mut(&self.id) {
            for (pc_id, sub) in &mut subs {
                if let Some(kept) = v.subscriptions.get(pc_id) {
                    sub.since = kept.since;
                    sub.kicked = kept.kicked && kept.token_hash == sub.token_hash && !sub.restart;
                }
            }
            old = v.subscriptions.keys().copied().collect();
            v.subscriptions = subs.clone();
        }
        for pc_id in old {
            if !subs.contains_key(&pc_id) {
                unindex_subscriber(state, pc_id, self.id);
                detach_viewer_from_pc(state, self.id, pc_id);
            }
        }

        for (&pc_id, sub) in &subs {
            state.subscribers.entry(pc_id).or_default().insert(self.id);
            if sub.kicked {
                self.outbox.about(pc_id, &ServerMsg::PcOffline { pc_id });
                continue;
            }
            match try_attach(state, pc_id, self.id, sub) {
                Attach::New { pc_out, evicted } => {
                    clear_refusal(state, pc_id, self.id);
                    evict(state, pc_id, &pc_out, evicted);
                    announce(&self.outbox, &pc_out, pc_id, self.id, sub);
                }
                Attach::Already(pc_out) => {
                    clear_refusal(state, pc_id, self.id);
                    if sub.restart {
                        announce(&self.outbox, &pc_out, pc_id, self.id, sub);
                    }
                }
                Attach::Full => self.outbox.about(pc_id, &ServerMsg::PcErr { pc_id, code: "full" }),
                Attach::Dropped(pc_out) => {
                    self.outbox.about(pc_id, &ServerMsg::PcOffline { pc_id });
                    pc_out.about(self.id, &ServerMsg::ViewerLeft { viewer_id: self.id });
                    admit_waiting(state, pc_id);
                }
                Attach::Refused { unauthorized } => {
                    if unauthorized && note_refusal(state, pc_id, self.id) >= state.config.stale_subscription {
                        unindex_subscriber(state, pc_id, self.id);
                    }
                    self.outbox.about(pc_id, &ServerMsg::PcOffline { pc_id });
                }
            }
        }
        for pc_id in refused.into_iter().take(MAX_REFUSED_REPLIES) {
            self.outbox.json(&ServerMsg::PcErr { pc_id, code: "too_many" });
        }
        Flow::Continue
    }
}

fn parse_subscriptions(items: Vec<serde_json::Value>) -> (HashMap<Uuid, Subscription>, Vec<Uuid>) {
    let mut subs = HashMap::new();
    let mut refused = Vec::new();
    for item in items {
        let Ok(item) = serde_json::from_value::<SubItem>(item) else {
            continue;
        };
        if subs.contains_key(&item.pc_id) || refused.contains(&item.pc_id) {
            continue;
        }
        if subs.len() >= MAX_SUBSCRIPTIONS {
            refused.push(item.pc_id);
            continue;
        }
        subs.insert(
            item.pc_id,
            Subscription {
                device_id: item.device_id,
                token_hash: sha256_hex(&item.room_token),
                since: Instant::now(),
                kicked: false,
                restart: item.restart,
            },
        );
    }
    (subs, refused)
}

fn rewrap(source: Uuid, bytes: &[u8]) -> Message {
    let mut out = Vec::with_capacity(bytes.len());
    out.push(BIN_VERSION);
    out.extend_from_slice(source.as_bytes());
    out.extend_from_slice(&bytes[BIN_HEADER_LEN..]);
    Message::Binary(out.into())
}

fn route_to_viewer(state: &RelayState, pc_id: Uuid, conn_id: Uuid, viewer_id: Uuid, bytes: &[u8]) -> bool {
    let attached =
        state.pcs.get(&pc_id).is_some_and(|pc| pc.conn_id == conn_id && pc.attached_viewers.contains_key(&viewer_id));
    let Some(viewer_out) = attached.then(|| state.viewers.get(&viewer_id).map(|v| v.outbox.clone())).flatten() else {
        return false;
    };
    viewer_out.routed(pc_id, rewrap(pc_id, bytes));
    true
}

fn route_to_pc(state: &RelayState, viewer_id: Uuid, pc_id: Uuid, bytes: &[u8]) -> bool {
    let pc_out = state.pcs.get(&pc_id).filter(|pc| pc.attached_viewers.contains_key(&viewer_id)).map(|pc| pc.outbox.clone());
    let Some(pc_out) = pc_out else {
        return false;
    };
    pc_out.routed(viewer_id, rewrap(viewer_id, bytes));
    true
}

enum Attach {
    New { pc_out: Outbox, evicted: Option<Uuid> },
    Already(Outbox),
    Full,
    Dropped(Outbox),
    Refused { unauthorized: bool },
}

fn try_attach(state: &RelayState, pc_id: Uuid, viewer_id: Uuid, sub: &Subscription) -> Attach {
    let Some(mut pc) = state.pcs.get_mut(&pc_id) else {
        return Attach::Refused { unauthorized: false };
    };
    if !pc.allowed_tokens.contains(&sub.token_hash) {
        return if pc.attached_viewers.remove(&viewer_id).is_some() {
            Attach::Dropped(pc.outbox.clone())
        } else {
            Attach::Refused { unauthorized: true }
        };
    }
    if pc.attached_viewers.contains_key(&viewer_id) {
        return Attach::Already(pc.outbox.clone());
    }
    let oldest_same_token = pc
        .attached_viewers
        .iter()
        .filter(|(_, a)| a.token_hash == sub.token_hash)
        .map(|(id, a)| (*id, a.since))
        .collect::<Vec<_>>();
    let evicted = if oldest_same_token.len() >= MAX_SOCKETS_PER_TOKEN {
        let oldest = oldest_same_token.iter().min_by_key(|(_, since)| *since).map(|(id, _)| *id);
        if let Some(id) = oldest {
            pc.attached_viewers.remove(&id);
        }
        oldest
    } else if pc.attached_viewers.len() >= MAX_VIEWERS_PER_PC {
        return Attach::Full;
    } else {
        None
    };
    pc.attached_viewers.insert(viewer_id, Attached { token_hash: sub.token_hash.clone(), since: Instant::now() });
    Attach::New { pc_out: pc.outbox.clone(), evicted }
}

/// Tells a socket that a newer one with the same device token took its place.
fn evict(state: &RelayState, pc_id: Uuid, pc_out: &Outbox, evicted: Option<Uuid>) {
    let Some(viewer_id) = evicted else {
        return;
    };
    mark_kicked(state, viewer_id, pc_id);
    if let Some(viewer_out) = state.viewers.get(&viewer_id).map(|v| v.outbox.clone()) {
        viewer_out.about(pc_id, &ServerMsg::PcErr { pc_id, code: "replaced" });
    }
    pc_out.about(viewer_id, &ServerMsg::ViewerLeft { viewer_id });
}

fn mark_kicked(state: &RelayState, viewer_id: Uuid, pc_id: Uuid) {
    if let Some(mut v) = state.viewers.get_mut(&viewer_id)
        && let Some(sub) = v.subscriptions.get_mut(&pc_id)
    {
        sub.kicked = true;
    }
}

fn clear_refusal(state: &RelayState, pc_id: Uuid, viewer_id: Uuid) {
    if let Some(mut v) = state.viewers.get_mut(&viewer_id) {
        v.refused_since.remove(&pc_id);
    }
}

fn announce(viewer_out: &Outbox, pc_out: &Outbox, pc_id: Uuid, viewer_id: Uuid, sub: &Subscription) {
    viewer_out.about(pc_id, &ServerMsg::PcOnline { pc_id });
    pc_out.about(viewer_id, &ServerMsg::ViewerJoined { viewer_id, device_id: sub.device_id, token_hash: &sub.token_hash });
}

/// Attaches a subscribed viewer if the PC lets it in now; true when it was newly attached.
fn reconcile(state: &RelayState, pc_id: Uuid, viewer_id: Uuid) -> bool {
    let sub = state
        .viewers
        .get(&viewer_id)
        .and_then(|v| v.subscriptions.get(&pc_id).map(|s| (s.clone(), v.outbox.clone())));
    let Some((sub, viewer_out)) = sub else {
        unindex_subscriber(state, pc_id, viewer_id);
        detach_viewer_from_pc(state, viewer_id, pc_id);
        return false;
    };
    if sub.kicked {
        return false;
    }
    match try_attach(state, pc_id, viewer_id, &sub) {
        Attach::New { pc_out, evicted } => {
            clear_refusal(state, pc_id, viewer_id);
            evict(state, pc_id, &pc_out, evicted);
            announce(&viewer_out, &pc_out, pc_id, viewer_id, &sub);
            true
        }
        Attach::Dropped(pc_out) => {
            viewer_out.about(pc_id, &ServerMsg::PcOffline { pc_id });
            pc_out.about(viewer_id, &ServerMsg::ViewerLeft { viewer_id });
            admit_waiting(state, pc_id);
            false
        }
        Attach::Refused { unauthorized: true } => {
            if note_refusal(state, pc_id, viewer_id) >= state.config.stale_subscription {
                unindex_subscriber(state, pc_id, viewer_id);
            }
            false
        }
        Attach::Already(_) | Attach::Full | Attach::Refused { unauthorized: false } => false,
    }
}

fn note_refusal(state: &RelayState, pc_id: Uuid, viewer_id: Uuid) -> Duration {
    let now = Instant::now();
    let Some(mut v) = state.viewers.get_mut(&viewer_id) else {
        return Duration::ZERO;
    };
    if let Some(since) = v.refused_since.get(&pc_id) {
        return now.duration_since(*since);
    }
    if v.refused_since.len() >= MAX_REFUSALS {
        return Duration::MAX;
    }
    v.refused_since.insert(pc_id, now);
    Duration::ZERO
}

/// After slots on a PC free up, attaches the subscribers still waiting for it, longest waiting
/// first, until the PC is full again (one that replaces a socket of its own device fills none).
fn admit_waiting(state: &RelayState, pc_id: Uuid) {
    let attached = |viewer_id: &Uuid| state.pcs.get(&pc_id).is_some_and(|pc| pc.attached_viewers.contains_key(viewer_id));
    let mut waiting: Vec<(Instant, Uuid)> = subscribers_of(state, pc_id)
        .into_iter()
        .filter(|viewer_id| !attached(viewer_id))
        .filter_map(|viewer_id| {
            let since = state.viewers.get(&viewer_id)?.subscriptions.get(&pc_id).filter(|s| !s.kicked)?.since;
            Some((since, viewer_id))
        })
        .collect();
    waiting.sort();
    for (_, viewer_id) in waiting {
        if state.pcs.get(&pc_id).is_none_or(|pc| pc.attached_viewers.len() >= MAX_VIEWERS_PER_PC) {
            return;
        }
        reconcile(state, pc_id, viewer_id);
    }
}

fn subscribers_of(state: &RelayState, pc_id: Uuid) -> Vec<Uuid> {
    state.subscribers.get(&pc_id).map(|s| s.iter().copied().collect()).unwrap_or_default()
}

fn unindex_subscriber(state: &RelayState, pc_id: Uuid, viewer_id: Uuid) {
    if let Some(mut set) = state.subscribers.get_mut(&pc_id) {
        set.remove(&viewer_id);
    }
    state.subscribers.remove_if(&pc_id, |_, set| set.is_empty());
}

fn detach_viewer_from_pc(state: &RelayState, viewer_id: Uuid, pc_id: Uuid) {
    let pc_out = state
        .pcs
        .get_mut(&pc_id)
        .and_then(|mut pc| pc.attached_viewers.remove(&viewer_id).map(|_| pc.outbox.clone()));
    if let Some(pc_out) = pc_out {
        pc_out.about(viewer_id, &ServerMsg::ViewerLeft { viewer_id });
        admit_waiting(state, pc_id);
    }
}

fn lookup_pair(state: &RelayState, raw: &str) -> (Option<String>, bool) {
    if let Some(id) = sanitize_pair_id(raw) {
        return (state.pair_rooms.contains_key(&id).then_some(id), false);
    }
    let code = sanitize_manual_code(raw);
    (code.as_ref().and_then(|code| state.manual_codes.get(code).map(|o| o.pair_id.clone())), code.is_some())
}

fn open_pair_room(state: &RelayState, pc_id: Uuid, conn_id: Uuid, pair_id: &str, manual: Option<String>) -> bool {
    let opened = Instant::now();
    let room = PairRoom {
        pc_id,
        pc_conn_id: conn_id,
        manual_code: manual.clone(),
        opened,
        expires_at: opened + PAIR_TTL,
        viewer_id: None,
        joins: 0,
    };
    let replaced = match state.pair_rooms.entry(pair_id.to_owned()) {
        Entry::Occupied(mut e) => {
            if e.get().pc_id != pc_id {
                return false;
            }
            Some(e.insert(room))
        }
        Entry::Vacant(e) => {
            e.insert(room);
            None
        }
    };
    if let Some(old) = replaced {
        room_closed(state, pair_id, old);
    }
    let Some(code) = manual else {
        return true;
    };
    let claimed = match state.manual_codes.entry(code) {
        Entry::Occupied(mut e) => {
            let owner = e.get();
            let held_by_other = owner.pc_id != pc_id
                && state.pair_rooms.get(&owner.pair_id).is_some_and(|r| r.pc_id == owner.pc_id);
            if !held_by_other {
                e.insert(CodeOwner { pair_id: pair_id.to_owned(), pc_id });
            }
            !held_by_other
        }
        Entry::Vacant(e) => {
            e.insert(CodeOwner { pair_id: pair_id.to_owned(), pc_id });
            true
        }
    };
    if !claimed {
        state.pair_rooms.remove_if(pair_id, |_, r| r.pc_conn_id == conn_id);
    }
    claimed
}

fn extend_pair_room(state: &RelayState, pair_id: &str, conn_id: Uuid) -> bool {
    let now = Instant::now();
    let Some(mut room) =
        state.pair_rooms.get_mut(pair_id).filter(|r| r.pc_conn_id == conn_id && r.viewer_id.is_some() && r.expires_at > now)
    else {
        return false;
    };
    let latest = room.opened + PAIR_TTL + PAIR_CONFIRM;
    room.expires_at = room.expires_at.max(now + PAIR_CONFIRM).min(latest);
    true
}

fn release_code(state: &RelayState, pair_id: &str, code: Option<String>) {
    if let Some(code) = code {
        state.manual_codes.remove_if(&code, |_, owner| owner.pair_id == pair_id);
    }
}

fn room_closed(state: &RelayState, pair_id: &str, room: PairRoom) {
    release_code(state, pair_id, room.manual_code);
    if let Some(viewer_out) = room.viewer_id.and_then(|v| state.viewers.get(&v).map(|v| v.outbox.clone())) {
        viewer_out.json(&ServerMsg::PairErr { code: "closed" });
        viewer_out.lanes.purpose.notify_one();
    }
}

fn remove_pair_room(state: &RelayState, pair_id: &str) {
    if let Some((_, room)) = state.pair_rooms.remove(pair_id) {
        room_closed(state, pair_id, room);
    }
}

fn remove_owned_pair_room(state: &RelayState, pair_id: &str, conn_id: Uuid) {
    if let Some((_, room)) = state.pair_rooms.remove_if(pair_id, |_, r| r.pc_conn_id == conn_id) {
        room_closed(state, pair_id, room);
    }
}

fn cleanup(state: &RelayState, conn: &Conn) {
    for pair_id in &conn.joined_rooms {
        let pc_id = state.pair_rooms.get_mut(pair_id).and_then(|mut room| {
            (room.viewer_id == Some(conn.id)).then(|| {
                room.viewer_id = None;
                room.pc_id
            })
        });
        if let Some(pc_out) = pc_id.and_then(|pc_id| state.pcs.get(&pc_id).map(|pc| pc.outbox.clone())) {
            pc_out.json(&ServerMsg::PairLeft { pair_id, viewer_id: conn.id });
        }
    }
    match conn.role {
        Role::Pc { pc_id } => {
            if let Some(pair_id) = &conn.open_pair {
                remove_owned_pair_room(state, pair_id, conn.id);
            }
            if let Some((_, pc)) = state.pcs.remove_if(&pc_id, |_, pc| pc.conn_id == conn.id) {
                for viewer_id in pc.attached_viewers.keys() {
                    if let Some(viewer_out) = state.viewers.get(viewer_id).map(|v| v.outbox.clone()) {
                        viewer_out.about(pc_id, &ServerMsg::PcOffline { pc_id });
                    }
                }
                debug!(%pc_id, conn_id = %conn.id, "PC disconnected");
            }
        }
        Role::Viewer => {
            if let Some((_, viewer)) = state.viewers.remove(&conn.id) {
                for &pc_id in viewer.subscriptions.keys() {
                    unindex_subscriber(state, pc_id, conn.id);
                    detach_viewer_from_pc(state, conn.id, pc_id);
                }
            }
        }
        Role::Unregistered => {}
    }
}

/// Spawns the periodic background sweeper for expired pairing rooms.
pub fn spawn_sweeper(state: RelayState) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(30));
        loop {
            tick.tick().await;
            state.sweep_expired();
        }
    });
}

/// Binds and serves the relay on `addr`.
pub async fn serve(addr: SocketAddr, state: RelayState) -> std::io::Result<()> {
    spawn_sweeper(state.clone());
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(
        %addr,
        max_connections = state.config.max_connections,
        max_connections_per_ip = state.config.max_connections_per_ip,
        trusted_proxies = ?state.config.trusted_proxies,
        "roalt-remote-relay listening"
    );
    axum::serve(listener, router(state).into_make_service_with_connect_info::<SocketAddr>())
        .with_graceful_shutdown(async {
            // `docker stop` sends SIGTERM, not Ctrl+C.
            #[cfg(unix)]
            {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("SIGTERM handler");
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            warn!("shutting down roalt-remote-relay");
        })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::FutureExt;
    use serde_json::json;

    #[test]
    fn ipv6_is_bucketed_by_slash_64() {
        let a: IpAddr = "2a02:4780:c:5355::1".parse().unwrap();
        let b: IpAddr = "2a02:4780:c:5355:ffff:1:2:3".parse().unwrap();
        let c: IpAddr = "2a02:4780:c:5356::1".parse().unwrap();
        assert_eq!(ip_bucket(a), ip_bucket(b));
        assert_ne!(ip_bucket(a), ip_bucket(c));
        let mapped: IpAddr = "::ffff:203.0.113.7".parse().unwrap();
        assert_eq!(ip_bucket(mapped), "203.0.113.7".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn origin_allow_list() {
        assert!(origin_allowed("https://remote.roalt.dev"));
        assert!(origin_allowed("http://localhost:5175"));
        assert!(origin_allowed("https://127.0.0.1:8443"));
        assert!(origin_allowed("http://localhost"));
        assert!(!origin_allowed("https://roalt.dev"));
        assert!(!origin_allowed("https://remote.roalt.dev.evil.com"));
        assert!(!origin_allowed("http://localhost.evil.com"));
        assert!(!origin_allowed("http://localhost:"));
        assert!(!origin_allowed("http://localhost:80@evil.com"));
        assert!(!origin_allowed("null"));
        assert!(!origin_allowed("http://remote.roalt.dev"));
    }

    #[test]
    fn x_real_ip_trusted_only_from_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", "198.51.100.9".parse().unwrap());
        let proxy: IpAddr = "172.31.88.1".parse().unwrap();
        let stranger: IpAddr = "203.0.113.5".parse().unwrap();
        let client: IpAddr = "198.51.100.9".parse().unwrap();

        let state = RelayState::with_config(RelayConfig { trusted_proxies: vec![proxy], ..RelayConfig::default() });
        assert_eq!(state.client_ip("127.0.0.1".parse().unwrap(), &headers), client);
        assert_eq!(state.client_ip("::ffff:127.0.0.1".parse().unwrap(), &headers), client);
        assert_eq!(state.client_ip(proxy, &headers), client);
        assert_eq!(state.client_ip(stranger, &headers), stranger);
        assert_eq!(state.client_ip(stranger, &HeaderMap::new()), stranger);
    }

    #[test]
    fn token_bucket_refills_and_caps() {
        let limit = RateLimit { bytes_per_sec: 100.0, burst_bytes: 200.0, msgs_per_sec: 10.0, burst_msgs: 2.0 };
        let mut bucket = Bucket::new(limit);
        let t0 = bucket.last;
        assert!(bucket.take_at(150, t0));
        assert!(!bucket.take_at(100, t0));
        assert!(bucket.take_at(50, t0));
        assert!(!bucket.take_at(1, t0));
        assert!(bucket.take_at(100, t0 + Duration::from_secs(1)));
        assert!(bucket.take_at(500, t0 + Duration::from_secs(60)), "a message over the burst costs the whole burst");
        assert!(!bucket.take_at(1, t0 + Duration::from_secs(60)));
    }

    #[test]
    fn token_bucket_says_how_long_to_wait() {
        let limit = RateLimit { bytes_per_sec: 100.0, burst_bytes: 200.0, msgs_per_sec: 10.0, burst_msgs: 1.0 };
        let mut bucket = Bucket::new(limit);
        let t0 = bucket.last;
        assert_eq!(bucket.wait_at(50, t0), Duration::ZERO);
        assert!(bucket.take_at(50, t0));
        assert_eq!(bucket.wait_at(50, t0), Duration::from_millis(100));
        assert!(bucket.take_at(150, t0 + Duration::from_millis(100)));
        assert_eq!(bucket.wait_at(100, t0 + Duration::from_millis(100)), Duration::from_millis(900));
        assert_eq!(bucket.wait_at(1, t0 + Duration::from_secs(5)), Duration::ZERO);
    }

    #[test]
    fn pair_ids_and_manual_codes_never_overlap() {
        assert_eq!(sanitize_pair_id(" 0123456789ABCDEF "), Some("0123456789abcdef".into()));
        assert_eq!(sanitize_pair_id("abcd-2345"), None);
        assert_eq!(sanitize_pair_id("abcd2345"), None);
        assert_eq!(sanitize_pair_id("pair-victim-0001"), None);
        assert_eq!(sanitize_manual_code("abcd-2345"), Some("ABCD2345".into()));
        assert!(sanitize_manual_code("0123456789abcdef").is_none());
    }

    #[test]
    fn app_versions_parse() {
        assert_eq!(parse_version("1.4.0"), Some((1, 4, 0)));
        assert_eq!(parse_version("1.10.2-beta.1"), Some((1, 10, 2)));
        assert!(parse_version("1.4").is_none() && parse_version("1.4.0.1").is_none() && parse_version("x").is_none());
        assert!(parse_version("1.3.11") < parse_version("1.4.0"));
    }

    #[test]
    fn subscribe_items_parse_individually() {
        let good = Uuid::new_v4();
        let items = vec![
            serde_json::json!({ "pcId": "not-a-uuid", "deviceId": Uuid::new_v4(), "roomToken": "t" }),
            serde_json::json!({ "pcId": Uuid::new_v4(), "deviceId": Uuid::new_v4(), "roomToken": 5 }),
            serde_json::json!({ "deviceId": Uuid::new_v4(), "roomToken": "t" }),
            serde_json::json!({ "pcId": good, "deviceId": Uuid::new_v4(), "roomToken": "t" }),
        ];
        let (subs, refused) = parse_subscriptions(items);
        assert_eq!(subs.len(), 1);
        assert!(subs.contains_key(&good));
        assert!(refused.is_empty());
    }

    #[test]
    fn peers_streams_take_turns_and_keep_their_own_order() {
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let mut streams = Streams::default();
        for n in 1..=3 {
            assert!(streams.push(a, ("a", n), 1));
        }
        assert!(streams.push(b, ("b", 1), 1));
        let order: Vec<_> = std::iter::from_fn(|| streams.pop().map(|(_, item)| item)).collect();
        assert_eq!(order, [("a", 1), ("b", 1), ("a", 2), ("a", 3)]);
        streams.close();
        assert!(!streams.push(a, ("a", 4), 1));
        assert!(streams.pop().is_none());
    }

    fn detached(groups: Vec<Arc<Group>>) -> Arc<Seat> {
        Arc::new(Seat { connections: Arc::default(), groups, released: AtomicBool::new(true) })
    }

    fn group(state: &RelayState) -> Arc<Group> {
        let level = AddressLevel { key: (IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED), 0), max_conns: usize::MAX, rate_scale: 1.0 };
        Arc::new(state.new_group(&level))
    }

    fn outbox(state: &RelayState, groups: Vec<Arc<Group>>) -> (Outbox, Inbox) {
        Outbox::new(state, detached(groups))
    }

    fn frame() -> Message {
        Message::Binary(vec![0u8; 500 - MESSAGE_OVERHEAD].into())
    }

    fn queued(out: &Outbox) -> usize {
        out.lanes.meter.socket.load(Ordering::Acquire)
    }

    #[tokio::test]
    async fn a_flooding_peer_delays_another_by_one_frame_at_most() {
        let state = RelayState::new();
        let (out, mut inbox) = outbox(&state, Vec::new());
        let (flood, quiet) = (Uuid::new_v4(), Uuid::new_v4());
        let frame = |n: u8| Message::Binary(vec![n; 8].into());
        for _ in 0..20 {
            assert!(out.routed(flood, frame(1)));
        }
        assert!(out.routed(quiet, frame(2)));
        out.json(&ServerMsg::Pong);
        let mut first = Vec::new();
        for _ in 0..3 {
            first.push(inbox.next().await.unwrap().0);
        }
        assert!(matches!(&first[0], Message::Text(_)), "the socket's own messages go first");
        assert_eq!(first[1], frame(1));
        assert_eq!(first[2], frame(2));
    }

    #[test]
    fn connection_buckets_outlive_the_last_socket() {
        let config = RelayConfig {
            connect_rate: RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 0.001, burst_msgs: 2.0 },
            ..RelayConfig::default()
        };
        let state = RelayState::with_config(config);
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        drop(state.admit(ip).unwrap());
        drop(state.admit(ip).unwrap());
        assert_eq!(state.admit(ip).err(), Some(StatusCode::TOO_MANY_REQUESTS), "reconnecting must not refill the bucket");
        state.sweep_expired();
        assert_eq!(state.admit(ip).err(), Some(StatusCode::TOO_MANY_REQUESTS));
    }

    #[test]
    fn pairing_guesses_are_also_counted_per_v6_48() {
        let state = RelayState::new();
        let prefix = v6_prefix_48("2001:db8:1::1".parse().unwrap());
        let tries = (0..PAIR_RATE_MAX_V6_48 + 5)
            .filter(|i| {
                let ip: IpAddr = format!("2001:db8:1:{i:x}::1").parse().unwrap();
                state.check_pair_rate(ip_bucket(ip), prefix)
            })
            .count();
        assert_eq!(tries, PAIR_RATE_MAX_V6_48 as usize);
    }

    #[test]
    fn outbox_enforces_socket_and_global_budgets() {
        let state = RelayState::with_config(RelayConfig { socket_queue_budget: 1000, global_queue_budget: 1800, ..RelayConfig::default() });
        let shared = group(&state);
        let (a, rx_a) = outbox(&state, vec![shared.clone()]);
        let (b, _rx_b) = outbox(&state, vec![shared.clone()]);
        let (c, _rx_c) = outbox(&state, vec![group(&state)]);
        let reply = || Message::Binary(vec![0u8; 400 - MESSAGE_OVERHEAD].into());

        assert!(a.push(reply(), None));
        assert!(a.push(reply(), None));
        assert!(!a.push(reply(), None), "its socket is full");
        assert!(b.push(reply(), None));
        assert!(b.push(reply(), None));
        assert!(!c.push(reply(), None), "the relay is full");
        assert_eq!(state.queued_bytes(), 1600);
        assert_eq!(shared.queued.load(Ordering::Acquire), 1600);
        drop(rx_a);
        assert_eq!(state.queued_bytes(), 800);
        assert_eq!(shared.queued.load(Ordering::Acquire), 800);
        assert_eq!(queued(&c), 0);
    }

    #[tokio::test]
    async fn a_full_relay_sheds_the_largest_group_and_never_a_smaller_one() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 3000, ..RelayConfig::default() });
        let (flooder, _rx_flooder) = outbox(&state, vec![group(&state)]);
        let (honest, _rx_honest) = outbox(&state, vec![group(&state)]);
        let peer = Uuid::new_v4();

        for _ in 0..6 {
            assert!(flooder.routed(peer, frame()));
        }
        flooder.about(peer, &ServerMsg::PcOffline { pc_id: peer });
        assert!(!flooder.routed(peer, frame()), "the largest group only hurts itself");
        assert!(honest.routed(peer, frame()), "a smaller group sheds the largest");
        assert_eq!(queued(&flooder), 1000, "only the oldest frames go, a few times what the push needs");
        assert!(!flooder.lanes.killed.load(Ordering::Acquire));
        let kept: Vec<bool> =
            flooder.lanes.streams().queues[&peer].items.iter().map(|(_, (m, _))| matches!(m, Message::Text(_))).collect();
        assert_eq!(kept, [false, false, true], "the newest frames and relay messages about a peer are kept");
    }

    #[tokio::test]
    async fn a_shed_socket_loses_frames_of_its_largest_stream_first() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 3000, ..RelayConfig::default() });
        let (victim, _rx_victim) = outbox(&state, vec![group(&state)]);
        let (pusher, _rx_pusher) = outbox(&state, vec![group(&state)]);
        let (flooding, quiet) = (Uuid::new_v4(), Uuid::new_v4());
        for _ in 0..5 {
            assert!(victim.routed(flooding, frame()));
        }
        assert!(victim.routed(quiet, frame()));
        assert!(pusher.routed(Uuid::new_v4(), frame()));
        let streams = victim.lanes.streams();
        assert_eq!(streams.queues[&flooding].items.len(), 1);
        assert_eq!(streams.queues[&quiet].items.len(), 1, "another peer's frames are kept");
        assert_eq!(victim.lanes.sheddable.load(Ordering::Acquire), 1000);
    }

    #[tokio::test]
    async fn a_socket_with_nothing_to_shed_is_skipped_not_closed() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 2000, ..RelayConfig::default() });
        let (stuck, _rx_stuck) = outbox(&state, vec![group(&state)]);
        let (honest, _rx_honest) = outbox(&state, vec![group(&state)]);
        for _ in 0..4 {
            assert!(stuck.push(frame(), None));
        }
        assert!(!honest.routed(Uuid::new_v4(), frame()));
        assert!(!stuck.lanes.killed.load(Ordering::Acquire));
        assert_eq!(state.queued_bytes(), 2000);
    }

    #[tokio::test]
    async fn a_push_over_its_own_socket_budget_sheds_nobody_else() {
        let state = RelayState::with_config(RelayConfig { socket_queue_budget: 1000, global_queue_budget: 3000, ..RelayConfig::default() });
        let (victim, _rx_victim) = outbox(&state, vec![group(&state)]);
        let (other, _rx_other) = outbox(&state, vec![group(&state)]);
        let (pusher, _rx_pusher) = outbox(&state, vec![group(&state)]);
        let peer = Uuid::new_v4();
        for out in [&victim, &other] {
            assert!(out.routed(Uuid::new_v4(), frame()));
            assert!(out.routed(Uuid::new_v4(), frame()));
        }
        assert!(pusher.routed(peer, frame()));
        assert!(pusher.routed(peer, frame()));
        assert_eq!(state.queued_bytes(), 3000);
        assert!(!pusher.routed(peer, frame()), "its socket is full of its own stream");
        assert_eq!((queued(&victim), queued(&other)), (1000, 1000), "nobody was shed for a push that failed anyway");
    }

    #[tokio::test]
    async fn a_socket_full_of_heavy_pcs_still_takes_another_pcs_frame() {
        let state = RelayState::with_config(RelayConfig { socket_queue_budget: 8 * 1000, ..RelayConfig::default() });
        let (viewer, _rx) = outbox(&state, vec![group(&state)]);
        let sized = |cost: usize| Message::Binary(vec![0u8; cost - MESSAGE_OVERHEAD].into());
        let heavy: Vec<Uuid> = (0..8).map(|_| Uuid::new_v4()).collect();
        for pc in &heavy {
            assert!(viewer.routed(*pc, sized(1000)));
        }
        assert!(!viewer.routed(heavy[0], sized(500)), "a heavy PC over the socket budget only drops its own frame");
        let quiet = Uuid::new_v4();
        assert!(viewer.routed(quiet, sized(500)), "a light PC's frame gets in");
        assert_eq!(viewer.lanes.streams().queues[&quiet].items.len(), 1);
        assert_eq!(viewer.lanes.streams().queues.len(), 8, "the oldest frame of one heavy PC made room");
        assert_eq!(queued(&viewer), 7500);
    }

    type Socket = (ConnGuard, Outbox, Inbox);

    fn socket_from(state: &RelayState, ip: &str) -> Socket {
        let guard = state.admit(ip.parse().unwrap()).unwrap();
        let (out, rx) = Outbox::new(state, guard.seat.clone());
        (guard, out, rx)
    }

    fn fill(socket: &Socket, frames: usize) {
        for _ in 0..frames {
            assert!(socket.1.routed(Uuid::new_v4(), frame()));
        }
    }

    fn held(socket: &Socket) -> usize {
        queued(&socket.1)
    }

    #[tokio::test]
    async fn frames_being_written_do_not_pin_the_relay_budget() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 4000, ..RelayConfig::default() });
        let mut writing = Vec::new();
        let mut slow = Vec::new();
        for net in 0..8 {
            let mut socket = socket_from(&state, &format!("198.51.{net}.1"));
            fill(&socket, 1);
            writing.push(socket.2.next().await.unwrap().0);
            slow.push(socket);
        }
        assert_eq!(state.queued_bytes(), 0, "a frame handed to the socket no longer holds the relay budget");
        let honest = socket_from(&state, "192.0.2.9");
        fill(&honest, 1);
        assert_eq!(writing.len(), 8);
    }

    #[tokio::test]
    async fn a_neighbour_of_a_prefix_filling_the_relay_still_gets_its_frames() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 6000, ..RelayConfig::default() });
        let neighbour = socket_from(&state, "203.0.113.50");
        let attackers: Vec<Socket> =
            ["203.0.113.1", "203.0.113.2", "198.51.100.1", "198.51.100.2"].iter().map(|ip| socket_from(&state, ip)).collect();
        attackers.iter().for_each(|a| fill(a, 3));
        assert_eq!(state.queued_bytes(), 6000);
        fill(&neighbour, 1);
        let outsider = socket_from(&state, "192.0.2.9");
        fill(&outsider, 1);
        assert_eq!(attackers.iter().map(held).sum::<usize>(), 4500);
    }

    #[tokio::test]
    async fn inside_a_prefix_the_largest_tenant_is_shed_not_the_largest_socket() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 4000, ..RelayConfig::default() });
        let honest = socket_from(&state, "2001:db8:1::1");
        fill(&honest, 2);
        let tenant: Vec<Socket> = (0..6).map(|i| socket_from(&state, &format!("2001:db8:9:{i:x}::1"))).collect();
        tenant.iter().for_each(|s| fill(s, 1));
        assert_eq!(state.queued_bytes(), 4000);
        fill(&honest, 1);
        assert_eq!(held(&honest), 1500);
        assert_eq!(tenant.iter().map(held).sum::<usize>(), 2500, "the /48 holding most pays, though its sockets are small");
    }

    #[tokio::test]
    async fn inside_a_carrier_prefix_the_largest_customer_is_shed_not_its_neighbours() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 9000, ..RelayConfig::default() });
        let honest: Vec<Socket> = (1..=3).map(|site| socket_from(&state, &format!("2001:db8:{site:x}::1"))).collect();
        honest.iter().for_each(|s| fill(s, 4));
        let attacker = socket_from(&state, "2001:db8:9::1");
        fill(&attacker, 6);
        assert_eq!(state.queued_bytes(), 9000);

        assert!(!attacker.1.routed(Uuid::new_v4(), frame()), "the largest customer only hurts itself");
        assert!(honest.iter().all(|s| held(s) == 2000));

        let newcomer = socket_from(&state, "203.0.113.9");
        fill(&newcomer, 1);
        assert_eq!(held(&attacker), 1000, "the carrier's largest customer is shed");
        assert!(honest.iter().all(|s| held(s) == 2000));
    }

    #[tokio::test]
    async fn many_small_addresses_in_one_prefix_are_weighed_as_that_prefix() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 10_000, ..RelayConfig::default() });
        let sybils: Vec<Socket> = (0..16).map(|i| socket_from(&state, &format!("2001:db8:a:{i:x}::1"))).collect();
        sybils.iter().for_each(|s| fill(s, 1));
        let honest = socket_from(&state, "2001:db9:1::1");
        fill(&honest, 4);
        assert_eq!(state.queued_bytes(), 10_000);

        let sybil_viewer = socket_from(&state, "2001:db8:a:99::1");
        fill(&sybil_viewer, 1);
        assert_eq!(held(&honest), 2000, "the prefix filling the relay pays for its own frames");
        assert_eq!(sybils.iter().filter(|s| held(s) == 0).count(), 1);

        let honest_viewer = socket_from(&state, "2001:db9:2::1");
        fill(&honest_viewer, 1);
        assert_eq!(held(&honest), 2000);
        let sybil_side = sybils.iter().chain([&sybil_viewer]);
        assert_eq!(sybil_side.map(held).sum::<usize>(), 7500, "a smaller prefix sheds the larger one");
    }

    #[tokio::test]
    async fn a_frame_bigger_than_any_one_victims_backlog_still_gets_room() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 16 * 500, ..RelayConfig::default() });
        let attackers: Vec<Socket> = (1..=16).map(|i| socket_from(&state, &format!("203.0.113.{i}"))).collect();
        attackers.iter().for_each(|a| fill(a, 1));
        let mut honest = socket_from(&state, "192.0.2.9");
        let (peer, big) = (Uuid::new_v4(), Message::Binary(vec![0u8; 2000 - MESSAGE_OVERHEAD].into()));
        let mut delivered = 0;
        for _ in 0..50 {
            if honest.1.routed(peer, big.clone()) {
                delivered += 1;
                honest.2.next().await;
            }
            for a in &attackers {
                let _ = a.1.routed(peer, frame());
            }
        }
        assert_eq!(delivered, 50, "a prefix filling the relay in small pieces is still shed for a bigger frame");
    }

    #[tokio::test]
    async fn a_socket_that_was_empty_when_the_ranking_was_built_is_shed_once_it_is_refreshed() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 1000, ..RelayConfig::default() });
        let attacker = socket_from(&state, "203.0.113.1");
        let honest = socket_from(&state, "192.0.2.9");
        let _ = state.shedder.ranked(false);
        fill(&attacker, 2);
        std::thread::sleep(RANK_REFRESH * 2);
        assert!(honest.1.routed(Uuid::new_v4(), frame()), "a backlog that grew after the last ranking is shed after the next one");
    }

    #[tokio::test]
    async fn the_shed_ranking_holds_only_sockets_with_queued_frames() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 1000, ..RelayConfig::default() });
        let idle: Vec<Socket> = (1..=32).map(|i| socket_from(&state, &format!("198.51.100.{i}"))).collect();
        idle[0].1.payload(&ServerMsg::PairMsg { pair_id: "00000000000000a1", data: "x" });
        let attacker = socket_from(&state, "203.0.113.1");
        fill(&attacker, 2);
        assert_eq!(state.shedder.ranked(false).len(), 1, "idle sockets and unsheddable payloads are never sorted");
        let honest = socket_from(&state, "192.0.2.9");
        assert!(honest.1.routed(Uuid::new_v4(), frame()));
    }

    #[tokio::test]
    async fn a_push_that_no_victim_set_can_cover_sheds_nobody() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 2000, ..RelayConfig::default() });
        let attackers: Vec<Socket> = (1..=4).map(|i| socket_from(&state, &format!("203.0.113.{i}"))).collect();
        attackers.iter().for_each(|a| fill(a, 1));
        let honest = socket_from(&state, "192.0.2.9");
        assert!(!honest.1.routed(Uuid::new_v4(), Message::Binary(vec![0u8; 4000].into())));
        assert_eq!(attackers.iter().map(held).sum::<usize>(), 2000, "nothing was thrown away for a frame that can't fit");
    }

    #[test]
    fn only_messages_that_can_change_a_sockets_purpose_recheck_it() {
        let parse = |raw: &str| serde_json::from_str::<ClientMsg>(raw).unwrap();
        assert!(!parse(r#"{"t":"ping"}"#).may_change_purpose());
        assert!(!parse(r#"{"t":"kick_viewer","viewerId":"00000000-0000-0000-0000-000000000001"}"#).may_change_purpose());
        assert!(parse(r#"{"t":"subscribe","pcs":[]}"#).may_change_purpose());
    }

    fn conn_on(state: &RelayState, socket: &Socket) -> Conn {
        Conn {
            id: Uuid::new_v4(),
            role: Role::Unregistered,
            outbox: socket.1.clone(),
            ip: socket.0.ip,
            prefix: socket.0.prefix,
            net: socket.0.net,
            bucket: Bucket::new(state.config.viewer_rate),
            groups: socket.0.seat.groups.clone(),
            control: Bucket::new(state.config.control_rate),
            last_rate_err: None,
            misroutes: (Instant::now(), 0),
            open_pair: None,
            joined_rooms: HashSet::new(),
        }
    }

    fn rechecked(socket: &Socket) -> bool {
        socket.1.lanes.purpose.notified().now_or_never().is_some()
    }

    #[test]
    fn dropped_or_empty_control_messages_do_not_recheck_purpose() {
        let state = RelayState::new();
        let socket = socket_from(&state, "192.0.2.1");
        let mut conn = conn_on(&state, &socket);
        let rechecks = (0..20)
            .filter(|_| {
                conn.on_text(&state, r#"{"t":"subscribe","pcs":[],"proto":2}"#);
                rechecked(&socket)
            })
            .count();
        assert!(rechecks <= 10, "only the subscribes the control budget lets through recheck, not {rechecks}");
        conn.on_text(&state, &json!({ "t": "unsubscribe", "pcId": Uuid::new_v4() }).to_string());
        assert!(!rechecked(&socket), "unsubscribing from a PC it never subscribed to changes nothing");
    }

    #[test]
    fn only_a_registration_or_closing_the_pcs_own_room_rechecks_purpose() {
        let state = RelayState::new();
        let close = r#"{"t":"pair_close","pairId":"0123456789abcdef0123456789abcdef"}"#;
        let secret = "ab".repeat(32);
        let hello = json!({ "t": "pc_hello", "pcId": pc_id_for_secret_hash(&secret), "pcSecretHash": secret, "proto": 2 }).to_string();

        let viewer = socket_from(&state, "192.0.2.1");
        let mut conn = conn_on(&state, &viewer);
        conn.on_text(&state, r#"{"t":"subscribe","pcs":[],"proto":2}"#);
        let _ = rechecked(&viewer);
        let rechecks = (0..200)
            .filter(|i| {
                conn.on_text(&state, if i % 2 == 0 { close } else { &hello });
                rechecked(&viewer)
            })
            .count();
        assert_eq!(rechecks, 0, "a viewer's pair_close and pc_hello change nothing");

        let pc = socket_from(&state, "192.0.2.2");
        let mut conn = conn_on(&state, &pc);
        conn.on_text(&state, &hello);
        assert!(rechecked(&pc), "registering gives the PC its first purpose check");
        conn.on_text(&state, &hello);
        conn.on_text(&state, close);
        assert!(!rechecked(&pc), "a repeat hello and closing a room it doesn't hold change nothing");
        conn.on_text(&state, r#"{"t":"pair_open","pairId":"0123456789abcdef0123456789abcdef"}"#);
        let _ = rechecked(&pc);
        conn.on_text(&state, close);
        assert!(rechecked(&pc), "closing its own room can end its purpose");
    }

    #[test]
    fn slow_readers_spamming_pings_hold_none_of_the_relay_budget() {
        let budget = 1024 * 1024;
        let state = RelayState::with_config(RelayConfig { socket_queue_budget: budget, global_queue_budget: budget, ..RelayConfig::default() });
        let readers: Vec<Socket> = (0..16).map(|net| socket_from(&state, &format!("198.51.{net}.1"))).collect();
        for reader in &readers {
            let mut conn = conn_on(&state, reader);
            for _ in 0..10_000 {
                conn.on_text(&state, r#"{"t":"ping"}"#);
            }
            assert!(reader.1.lanes.meter.control.load(Ordering::Acquire) <= CONTROL_ALLOWANCE);
        }
        assert_eq!(state.queued_bytes(), 0);
        let honest = socket_from(&state, "192.0.2.9");
        fill(&honest, budget / 500);
        assert_eq!(state.queued_bytes(), budget / 500 * 500, "an honest reader still gets the whole relay budget");
    }

    #[test]
    fn refusals_are_remembered_for_a_bounded_number_of_pcs() {
        let state = RelayState::new();
        let viewer_id = Uuid::new_v4();
        let (out, _rx) = outbox(&state, Vec::new());
        state.viewers.insert(viewer_id, ViewerEntry { outbox: out, subscriptions: HashMap::new(), refused_since: HashMap::new() });
        for _ in 0..MAX_REFUSALS {
            assert_eq!(note_refusal(&state, Uuid::new_v4(), viewer_id), Duration::ZERO);
        }
        assert_eq!(note_refusal(&state, Uuid::new_v4(), viewer_id), Duration::MAX, "past the cap a refused PC isn't waited on");
        assert_eq!(state.viewers.get(&viewer_id).unwrap().refused_since.len(), MAX_REFUSALS);
    }

    #[tokio::test]
    async fn the_shed_ranking_is_shared_and_rebuilt_at_most_once_per_interval() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 8000, ..RelayConfig::default() });
        let sockets: Vec<Socket> = (0..16).map(|i| socket_from(&state, &format!("198.51.{i}.1"))).collect();
        sockets.iter().for_each(|s| fill(s, 1));
        let first = state.shedder.ranked(false);
        for socket in &sockets {
            let _ = socket.1.routed(Uuid::new_v4(), frame());
        }
        assert!(Arc::ptr_eq(&first, &state.shedder.ranked(false)), "pushes share one ranking");
        std::thread::sleep(RANK_REFRESH * 2);
        assert!(!Arc::ptr_eq(&first, &state.shedder.ranked(false)));
    }

    #[tokio::test]
    async fn relay_messages_never_take_room_from_routed_frames() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 2000, ..RelayConfig::default() });
        let (flooder, _rx_flooder) = outbox(&state, vec![group(&state)]);
        let (honest, _rx_honest) = outbox(&state, vec![group(&state)]);
        let data = "x".repeat(MAX_PAIR_DATA);
        for _ in 0..CONTROL_ALLOWANCE / MAX_PAIR_DATA + 2 {
            flooder.payload(&ServerMsg::PairMsg { pair_id: "00000000000000a1", data: &data });
        }
        assert!(flooder.lanes.meter.control.load(Ordering::Acquire) <= CONTROL_ALLOWANCE);
        assert!(state.control_bytes() > CONTROL_ALLOWANCE / 2);
        assert_eq!(state.queued_bytes(), 0);
        assert!(flooder.lanes.meter.groups[0].queued.load(Ordering::Acquire) > 0, "a client's payload counts for its receiver's address");
        for _ in 0..4 {
            assert!(honest.routed(Uuid::new_v4(), frame()));
        }
        assert_eq!(queued(&honest), 2000);
    }

    #[test]
    fn a_light_address_is_not_held_up_by_heavy_neighbours_in_its_prefix() {
        let rate = RateLimit { bytes_per_sec: 1000.0, burst_bytes: 1000.0, msgs_per_sec: 1e9, burst_msgs: 1e9 };
        let state = RelayState::with_config(RelayConfig { ip_rate: rate, ..RelayConfig::default() });
        let heavy: Vec<ConnGuard> = (1..=8).map(|site| state.admit(format!("2001:db8:{site:x}::1").parse().unwrap()).unwrap()).collect();
        let light = state.admit("2001:db8:ff::1".parse().unwrap()).unwrap();
        let heavy_neighbour = state.admit("2001:db8:1:5::1".parse().unwrap()).unwrap();
        let now = Instant::now();
        for guard in &heavy {
            assert_eq!(inflow_wait(&guard.seat.groups, 1000, now), Duration::ZERO);
            inflow_charge(&guard.seat.groups, 1000, now);
        }
        assert!(inflow_wait(&heavy_neighbour.seat.groups, 500, now) > Duration::ZERO, "a heavy site waits on its /32");
        assert_eq!(inflow_wait(&light.seat.groups, 500, now), Duration::ZERO, "a light one does not");
    }

    #[tokio::test]
    async fn relay_messages_have_their_own_allowance_per_socket() {
        let state = RelayState::with_config(RelayConfig { global_queue_budget: 2000, ..RelayConfig::default() });
        let (filler, _rx_filler) = outbox(&state, vec![group(&state)]);
        let (flooder, _rx_flooder) = outbox(&state, vec![group(&state)]);
        let (honest, _rx_honest) = outbox(&state, vec![group(&state)]);
        for _ in 0..4 {
            assert!(filler.routed(Uuid::new_v4(), frame()));
        }
        let peer = Uuid::new_v4();
        for _ in 0..CONTROL_ALLOWANCE / 100 {
            flooder.about(peer, &ServerMsg::PcOffline { pc_id: peer });
        }
        assert!(flooder.lanes.meter.control.load(Ordering::Acquire) <= CONTROL_ALLOWANCE);
        honest.about(peer, &ServerMsg::PcOnline { pc_id: peer });
        assert_eq!(honest.lanes.streams().queues[&peer].items.len(), 1, "a flooder can't crowd out another socket's relay messages");
    }

    #[test]
    fn address_groups_nest_from_fine_to_coarse() {
        let state = RelayState::new();
        let keys: Vec<AddressKey> =
            state.address_levels("2001:db8:1:2:3:4:5:6".parse().unwrap()).into_iter().map(|l| l.key).collect();
        let expected: Vec<AddressKey> = vec![
            ("2001:db8:1:2::".parse().unwrap(), 64),
            ("2001:db8:1::".parse().unwrap(), 56),
            ("2001:db8:1::".parse().unwrap(), 48),
            ("2001:db8::".parse().unwrap(), 32),
        ];
        assert_eq!(keys, expected);
        let mapped: Vec<AddressKey> =
            state.address_levels("::ffff:203.0.113.7".parse().unwrap()).into_iter().map(|l| l.key).collect();
        assert_eq!(mapped, vec![("203.0.113.7".parse().unwrap(), 32), ("203.0.113.0".parse().unwrap(), 24)]);
    }

    #[test]
    fn a_carrier_prefix_admits_hundreds_of_subscribers() {
        let state = RelayState::new();
        let guards: Vec<_> =
            (0..300).map(|i| state.admit(format!("2001:db8:{i:x}:1::1").parse().unwrap()).expect("admitted")).collect();
        assert_eq!(state.connections(), guards.len());
    }

    fn unlimited() -> RateLimit {
        RateLimit { bytes_per_sec: f64::MAX, burst_bytes: f64::MAX, msgs_per_sec: 1e9, burst_msgs: 1e9 }
    }

    #[test]
    fn one_tenant_with_several_64s_does_not_lock_out_its_48_neighbour() {
        let state = RelayState::with_config(RelayConfig { max_connections_per_ip: 16, ..RelayConfig::default() });
        let mut held = Vec::new();
        for net in 0..4 {
            let ip: IpAddr = format!("2001:db8:1:{net:x}::1").parse().unwrap();
            for _ in 0..16 {
                held.push(state.admit(ip).expect("within the /64 cap"));
            }
            for _ in 0..25 {
                assert_eq!(state.admit(ip).err(), Some(StatusCode::TOO_MANY_REQUESTS));
            }
        }
        assert_eq!(state.admit("2001:db8:1:7::1".parse().unwrap()).err(), Some(StatusCode::TOO_MANY_REQUESTS), "the /56 is full");
        for _ in 0..4 {
            held.push(state.admit("2001:db8:1:100::1".parse().unwrap()).expect("a neighbour /56 still connects"));
        }
    }

    #[test]
    fn fresh_64s_spend_their_prefixes_connect_budget() {
        let state = RelayState::new();
        let admitted = (0..200).filter(|i| state.admit(format!("2001:db8:1:{i:x}::1").parse().unwrap()).is_ok()).count();
        let burst = state.config.connect_rate.burst_msgs as usize * state.config.max_connections_per_v6_56 / state.config.max_connections_per_ip;
        assert!((burst..=burst + 2).contains(&admitted), "{admitted} fresh /64s got in, the /56 allows {burst}");
        assert!(state.admit("2001:db8:1:100::1".parse().unwrap()).is_ok(), "another /56 of the /48 is not held up");
        assert!(state.admit("2001:db8:1:0::1".parse().unwrap()).is_ok(), "nor a /64 the relay already knows");
    }

    #[test]
    fn fresh_64s_in_known_56s_still_spend_the_48s_connect_budget() {
        let state = RelayState::new();
        let seeds: Vec<ConnGuard> =
            (0..100u32).filter_map(|s56| state.admit(format!("2001:db8:1:{:x}::1", s56 << 8).parse().unwrap()).ok()).collect();
        let mut admitted = 0;
        for s56 in 0..100u32 {
            for s64 in 1..60u32 {
                admitted += usize::from(state.admit(format!("2001:db8:1:{:x}::1", (s56 << 8) | s64).parse().unwrap()).is_ok());
            }
        }
        let burst = state.config.connect_rate.burst_msgs as usize * state.config.max_connections_per_site / state.config.max_connections_per_ip;
        assert!(seeds.len() + admitted <= burst + 10, "{} seeds and {admitted} fresh /64s got in, the /48 allows {burst}", seeds.len());
    }

    #[test]
    fn neighbours_spending_a_24s_connect_budget_do_not_lock_out_its_other_addresses() {
        let state = RelayState::new();
        let mut held: Vec<ConnGuard> = (0..4).map(|_| state.admit("198.51.100.200".parse().unwrap()).unwrap()).collect();
        for host in 1..=8 {
            held.extend((0..4).map(|_| state.admit(format!("198.51.100.{host}").parse().unwrap()).unwrap()));
        }
        std::thread::sleep(Duration::from_millis(1500));
        for host in 1..=8 {
            for _ in 0..40 {
                held.extend(state.admit(format!("198.51.100.{host}").parse().unwrap()));
            }
        }
        assert_eq!(state.admit("198.51.100.1".parse().unwrap()).err(), Some(StatusCode::TOO_MANY_REQUESTS));
        held.push(state.admit("198.51.100.77".parse().unwrap()).expect("an address the relay has not seen yet"));
        held.push(state.admit("198.51.100.200".parse().unwrap()).expect("a neighbour the relay knows"));
    }

    #[test]
    fn rotating_through_fresh_prefixes_keeps_the_address_table_bounded() {
        let state = RelayState::with_config(RelayConfig { max_connections: 4, connect_rate: unlimited(), ..RelayConfig::default() });
        let cap = 4 * SLOTS_PER_CONNECTION;
        for i in 0..1000u32 {
            drop(state.admit(format!("2001:db8:{:x}:{:x}::1", i >> 8, i & 0xff).parse().unwrap()));
            assert!(state.address_slots.len() <= cap + 4, "{} address groups after {i} rotations", state.address_slots.len());
        }
    }

    #[test]
    fn concurrent_upgrades_never_overshoot_an_address_cap() {
        let state = RelayState::with_config(RelayConfig { max_connections_per_ip: 4, connect_rate: unlimited(), ..RelayConfig::default() });
        let barrier = std::sync::Barrier::new(32);
        let admitted = std::thread::scope(|scope| {
            let tries: Vec<_> = (0..32)
                .map(|_| {
                    scope.spawn(|| {
                        barrier.wait();
                        state.admit("203.0.113.77".parse().unwrap()).ok()
                    })
                })
                .collect();
            tries.into_iter().filter_map(|t| t.join().unwrap()).collect::<Vec<_>>()
        });
        assert_eq!(admitted.len(), 4);
    }

    fn killed(sockets: &[Socket]) -> usize {
        sockets.iter().filter(|s| s.1.lanes.killed.load(Ordering::Acquire)).count()
    }

    #[tokio::test]
    async fn a_full_address_replaces_its_longest_silent_socket() {
        let ping = Duration::from_millis(100);
        let state = RelayState::with_config(RelayConfig { max_connections_per_ip: 2, ping_interval: ping, ..RelayConfig::default() });
        let held: Vec<Socket> = (0..2).map(|_| socket_from(&state, "192.0.2.7")).collect();
        assert_eq!(state.admit("192.0.2.7".parse().unwrap()).err(), Some(StatusCode::TOO_MANY_REQUESTS), "both are live");
        tokio::time::sleep(ping * 2).await;
        held[0].1.lanes.hear();
        let newcomer = state.admit("192.0.2.7".parse().unwrap()).expect("a dead socket of the address makes room");
        assert!(!held[0].1.lanes.killed.load(Ordering::Acquire) && held[1].1.lanes.killed.load(Ordering::Acquire));
        assert_eq!(state.connections(), 2);
        drop(newcomer);
    }

    #[tokio::test]
    async fn a_full_relay_makes_room_for_a_smaller_group_once_per_interval() {
        let state = RelayState::with_config(RelayConfig { max_connections: 4, ..RelayConfig::default() });
        let held: Vec<Socket> = (0..4).map(|_| socket_from(&state, "192.0.2.1")).collect();
        assert_eq!(state.admit("192.0.2.1".parse().unwrap()).err(), Some(StatusCode::SERVICE_UNAVAILABLE));
        let newcomer = state.admit("198.51.100.1".parse().unwrap()).expect("a smaller group takes the newest seat");
        assert!(held[3].1.lanes.killed.load(Ordering::Acquire));
        assert_eq!(state.connections(), 4, "the closed socket gives its seat up at once");
        for host in 2..22 {
            assert_eq!(state.admit(format!("198.51.100.{host}").parse().unwrap()).err(), Some(StatusCode::SERVICE_UNAVAILABLE));
        }
        assert_eq!(killed(&held), 1, "open-and-close churn from one network evicts once per interval");
        drop(newcomer);
    }

    fn fill_48(state: &RelayState) -> Vec<Socket> {
        let mut held = Vec::new();
        for i in 0..4u32 {
            for j in 0..4u32 {
                for _ in 0..16 {
                    held.push(socket_from(state, &format!("2001:db8:1:{:x}::1", (i << 8) | j)));
                }
            }
        }
        held
    }

    #[tokio::test]
    async fn a_tenant_keeping_its_sockets_young_cannot_hold_a_full_48() {
        let state = RelayState::with_config(RelayConfig { connect_rate: unlimited(), ..RelayConfig::default() });
        let held = fill_48(&state);
        assert_eq!(held.len(), 256);
        let neighbour = state.admit("2001:db8:1:ff00::1".parse().unwrap()).expect("a neighbour /56 trims the largest, young or not");
        assert_eq!(killed(&held), 1);
        assert_eq!(state.admit("2001:db8:1:ff02::1".parse().unwrap()).err(), Some(StatusCode::TOO_MANY_REQUESTS), "once per interval");
        drop(neighbour);
    }

    #[tokio::test]
    async fn trimming_its_own_64s_does_not_shut_a_48_for_its_neighbours() {
        let state = RelayState::with_config(RelayConfig { connect_rate: unlimited(), ..RelayConfig::default() });
        let held = fill_48(&state);
        let own = state.admit("2001:db8:1:5::1".parse().unwrap()).expect("a fresh /64 in a full /56 trims a sibling");
        assert_eq!(killed(&held), 1);
        let neighbour = state.admit("2001:db8:1:ff00::1".parse().unwrap()).expect("the /48 neighbour still gets in");
        assert_eq!(killed(&held), 2);
        drop((own, neighbour));
    }

    #[tokio::test]
    async fn the_largest_tenant_trimming_itself_every_interval_does_not_shut_its_48() {
        let state = RelayState::with_config(RelayConfig { connect_rate: unlimited(), ..RelayConfig::default() });
        let mut held = Vec::new();
        for s56 in 0..4u32 {
            for s64 in 0..2u32 {
                held.extend((0..32).map(|_| socket_from(&state, &format!("2001:db8:1:{:x}::1", (s56 << 8) | s64))));
            }
        }
        held.extend((4..8u32).map(|s56| socket_from(&state, &format!("2001:db8:1:{:x}::1", s56 << 8))));
        assert_eq!(killed(&held), 4, "each small /56 of the tenant trimmed one of its big ones");
        let neighbour = state.admit("2001:db8:1:ff00::1".parse().unwrap()).expect("a neighbour /56 still gets a seat");
        assert_eq!(killed(&held), 5);
        drop(neighbour);
    }

    #[tokio::test]
    async fn the_largest_tenants_trimming_themselves_do_not_shut_a_full_relay() {
        let state = RelayState::with_config(RelayConfig {
            max_connections: 64,
            max_connections_per_ip: 16,
            max_connections_per_site: 16,
            connect_rate: unlimited(),
            ..RelayConfig::default()
        });
        let mut held = Vec::new();
        for net in 0..4 {
            held.extend((0..16).map(|_| socket_from(&state, &format!("198.51.{net}.1"))));
        }
        held.extend((10..14).map(|net| socket_from(&state, &format!("198.51.{net}.1"))));
        assert_eq!(killed(&held), 4);
        let newcomer = state.admit("192.0.2.1".parse().unwrap()).expect("a newcomer from another /24 still gets a seat");
        assert_eq!(killed(&held), 5);
        drop(newcomer);
    }

    #[tokio::test]
    async fn a_full_prefix_trims_its_largest_customer_for_a_new_one() {
        let state = RelayState::with_config(RelayConfig {
            max_connections_per_v6_56: 192,
            max_connections_per_site: 192,
            max_connections_per_v6_32: 4 * 192,
            connect_rate: unlimited(),
            ..RelayConfig::default()
        });
        let mut held = Vec::new();
        for site in 0..4 {
            for n in 0..192 {
                held.push(socket_from(&state, &format!("2001:db8:{site:x}:{:x}::1", n / 16)));
            }
        }
        let fresh = state.admit("2001:db8:9::1".parse().unwrap()).expect("a new customer in the full /32 gets in");
        assert_eq!(killed(&held), 1);
        assert_eq!(state.admit("2001:db8:9:1::1".parse().unwrap()).err(), Some(StatusCode::TOO_MANY_REQUESTS), "that customer waits");
        let other = state.admit("2001:db8:a::1".parse().unwrap()).expect("another new customer trims another site");
        assert_eq!(killed(&held), 2);
        let sites: HashSet<IpAddr> =
            held.iter().filter(|s| s.1.lanes.killed.load(Ordering::Acquire)).map(|s| s.0.prefix.unwrap()).collect();
        assert_eq!(sites.len(), 2, "a site loses one socket per interval");
        drop((fresh, other));
    }

    #[test]
    fn one_network_spends_only_its_share_of_the_manual_code_budget() {
        let state = RelayState::new();
        let (a, b) = (network_of("203.0.113.5".parse().unwrap()), network_of("198.51.100.5".parse().unwrap()));
        assert_eq!(a, network_of("203.0.113.200".parse().unwrap()));
        for _ in 0..MANUAL_MISS_MAX / MANUAL_MISS_SHARE {
            assert!(state.manual_guesses_left(a));
            state.note_manual_miss(a);
        }
        assert!(!state.manual_guesses_left(a));
        assert!(state.manual_guesses_left(b));
    }

    #[test]
    fn many_guessing_networks_cannot_switch_manual_pairing_off_for_a_new_one() {
        let state = RelayState::new();
        let share = MANUAL_MISS_MAX / MANUAL_MISS_SHARE;
        for g in 0..20 {
            let net = network_of(format!("203.0.{g}.1").parse().unwrap());
            for _ in 0..share {
                state.note_manual_miss(net);
            }
        }
        assert!(!state.manual_guesses_left(network_of("203.0.5.1".parse().unwrap())));
        let guesser = network_of("203.0.200.1".parse().unwrap());
        state.note_manual_miss(guesser);
        assert!(!state.manual_guesses_left(guesser), "a network that guessed wrong waits once the budget is spent");
        assert!(state.manual_guesses_left(network_of("198.51.100.5".parse().unwrap())), "a network with no miss still pairs");
    }

    #[test]
    fn an_approved_pairing_gets_time_to_confirm_but_an_open_one_does_not() {
        let state = RelayState::new();
        let (pc_id, pc_conn, pair_id) = (Uuid::new_v4(), Uuid::new_v4(), "00000000000000e1");
        assert!(open_pair_room(&state, pc_id, pc_conn, pair_id, None));
        assert!(!extend_pair_room(&state, pair_id, pc_conn), "nobody joined yet");
        let opened = state.pair_rooms.get(pair_id).unwrap().opened;
        if let Some(mut room) = state.pair_rooms.get_mut(pair_id) {
            room.viewer_id = Some(Uuid::new_v4());
            room.expires_at = Instant::now() + Duration::from_secs(5);
        }
        assert!(!extend_pair_room(&state, pair_id, Uuid::new_v4()), "only the PC's own connection");
        assert!(extend_pair_room(&state, pair_id, pc_conn));
        let expires = state.pair_rooms.get(pair_id).unwrap().expires_at;
        assert!(expires >= Instant::now() + PAIR_CONFIRM - Duration::from_secs(1));
        assert!(expires <= opened + PAIR_TTL + PAIR_CONFIRM);
    }
}
