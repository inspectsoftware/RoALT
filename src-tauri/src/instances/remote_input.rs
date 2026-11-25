//! Touch from the Remote Dashboard (`remote`) into one client, as real Windows touch input
//! (`InjectTouchInput`): Windows turns a tap into a click. Roblox on a PC without a touchscreen
//! never shows its own thumbstick (measured 2026-11-22), so the phone draws one and holds the
//! movement keys (`MoveKey`) instead; a drag turns the camera as Roblox on a PC does it, with the
//! right mouse button held (`camera`). Events run in the order they came, one at a time, on a thread of their
//! own (bringing a client to the front blocks briefly, `afk::to_front`), from a queue of at most `QUEUE`.
//! A hung client is never brought forward, and should that thread still be stuck on one event for
//! `STALE` (or past the longest restore of a minimized client), another one lets go of every key
//! and the right button (`deadman`).
//! Every finger still down is sent again each `KEEP_ALIVE` (Windows cancels a contact that stops
//! getting frames). Input lands only on the client's own area (`point_at`, `hits`). Anything held is
//! let go when the phone stops refreshing it for `STALE` (it re-sends held keys and drags), when its
//! client leaves the front, and when the phone goes (`release_owner`: only what that session holds,
//! and its events queued before that; `release_all` for everyone; `release_now` when RoALT exits,
//! after which nothing is pressed again), so nothing stays pressed in a game nobody watches. A
//! refresh never presses a key again that was let go meanwhile, and a lift never moves. While
//! Windows is locked nothing is pressed or moved, also what was queued before the lock. Each device
//! (all of its sessions together) may have at most `DEVICE_QUEUE` presses and as many lifts waiting
//! and `DEVICE_FINGERS` fingers down, so one can't crowd out another, and a press only goes to the
//! client that was checked when it was sent (same pid and start time).

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::thread::sleep;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSE_EVENT_FLAGS, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEINPUT, SendInput, VIRTUAL_KEY,
};
use windows::Win32::UI::Input::Pointer::{
    InitializeTouchInjection, InjectTouchInput, POINTER_FLAG_DOWN, POINTER_FLAG_INCONTACT, POINTER_FLAG_INRANGE, POINTER_FLAG_UP, POINTER_FLAG_UPDATE,
    POINTER_FLAGS, POINTER_INFO, POINTER_TOUCH_INFO, TOUCH_FEEDBACK_NONE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow, PT_TOUCH, SetCursorPos, TOUCH_MASK_CONTACTAREA, TOUCH_MASK_ORIENTATION, TOUCH_MASK_PRESSURE, WindowFromPoint,
};

use uuid::Uuid;

use super::{afk, process, window};

/// A restored client's time to draw again before it takes the input.
const RESTORED: Duration = Duration::from_millis(300);
/// Fingers held still are sent again this often.
const KEEP_ALIVE: Duration = Duration::from_millis(60);
const STALE: Duration = Duration::from_millis(750);
const QUEUE: usize = 64;
const DEVICE_QUEUE: usize = QUEUE / 4;
/// Sessions whose release is remembered, so the events each queued before it are dropped.
const GONE_KEPT: usize = 256;
/// Windows' own limit for injected touch is 256; a phone has ten fingers.
const MAX_FINGERS: usize = 10;
const DEVICE_FINGERS: usize = MAX_FINGERS / 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Down,
    Move,
    Up,
}

/// The only keys a phone may hold: the joystick's four and the jump button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MoveKey {
    W,
    A,
    S,
    D,
    Jump,
}

impl MoveKey {
    const ALL: [Self; 5] = [Self::W, Self::A, Self::S, Self::D, Self::Jump];

    fn bit(self) -> u8 {
        1 << self as u8
    }

    /// From the dashboard's `KeyboardEvent.code`-style name (pure).
    pub fn parse(code: &str) -> Option<Self> {
        Some(match code {
            "KeyW" => Self::W,
            "KeyA" => Self::A,
            "KeyS" => Self::S,
            "KeyD" => Self::D,
            "Space" => Self::Jump,
            _ => return None,
        })
    }

    /// Set-1 scan code, as a keyboard sends it.
    fn scan(self) -> u16 {
        match self {
            Self::W => 0x11,
            Self::A => 0x1E,
            Self::S => 0x1F,
            Self::D => 0x20,
            Self::Jump => 0x39,
        }
    }
}

/// The client an event is for: its pid and the start time it had when the event was checked.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Client {
    pub pid: u32,
    pub started: Option<u64>,
}

impl Client {
    /// For a lift: it lets go of what its session holds wherever that is, so it needs no client.
    pub const NONE: Client = Client { pid: 0, started: None };
}

/// Each event carries its `owner`, the remote session that sent it.
enum Event {
    /// `finger`: the phone's id for it; `x`, `y`: 0..=1 across the client area as the phone sees it
    /// (`capture` takes that area).
    Touch { owner: Uuid, client: Client, finger: u32, x: f32, y: f32, phase: Phase },
    /// `repeat`: the phone re-sending a key it still holds, never a new press.
    Key { owner: Uuid, client: Client, key: MoveKey, down: bool, repeat: bool },
    /// A camera drag: `x`, `y` where the finger is, as for `Touch`.
    Camera { owner: Uuid, client: Client, x: f32, y: f32, phase: Phase },
}

impl Event {
    fn owner(&self) -> Uuid {
        match self {
            Self::Touch { owner, .. } | Self::Key { owner, .. } | Self::Camera { owner, .. } => *owner,
        }
    }

    fn is_release(&self) -> bool {
        match self {
            Self::Touch { phase, .. } | Self::Camera { phase, .. } => *phase == Phase::Up,
            Self::Key { down, .. } => !down,
        }
    }
}

/// The camera drag under way: whose, its client's window, where the finger was last (a screen
/// pixel), and when the phone last said so.
struct Drag {
    owner: Uuid,
    pid: u32,
    hwnd: HWND,
    at: POINT,
    seen: Instant,
}

/// One finger on the glass: the phone's id, Windows' contact id (0..MAX_FINGERS), its client's
/// window, where, and when the phone last said so.
struct Finger {
    owner: Uuid,
    device: Uuid,
    phone: u32,
    contact: u32,
    hwnd: HWND,
    at: POINT,
    seen: Instant,
}

struct Key {
    owner: Uuid,
    key: MoveKey,
    hwnd: HWND,
    seen: Instant,
}

#[derive(Default)]
struct Held {
    fingers: Vec<Finger>,
    keys: Vec<Key>,
    drag: Option<Drag>,
}

impl Held {
    fn release(&mut self, whose: impl Fn(Uuid) -> bool) {
        let mine: Vec<usize> = (0..self.keys.len()).rev().filter(|&i| whose(self.keys[i].owner)).collect();
        for i in mine {
            let_go(&mut self.keys, i);
        }
        if self.drag.as_ref().is_some_and(|d| whose(d.owner)) {
            self.drag = None;
            right_button(false);
        }
        let lifted: Vec<(Uuid, u32)> = self.fingers.iter().filter(|f| whose(f.owner)).map(|f| (f.owner, f.phone)).collect();
        for (owner, phone) in lifted {
            lift(&mut self.fingers, owner, phone);
        }
    }

    fn expire(&mut self, now: Instant) {
        if self.fingers.is_empty() && self.keys.is_empty() && self.drag.is_none() {
            return;
        }
        // SAFETY: reads which window is in front.
        let front = unsafe { GetForegroundWindow() };
        let gone = |hwnd: HWND, seen: Instant| hwnd != front || stale(seen, now);
        let lost: Vec<usize> = (0..self.keys.len()).rev().filter(|&i| gone(self.keys[i].hwnd, self.keys[i].seen)).collect();
        for i in lost {
            let_go(&mut self.keys, i);
        }
        if self.drag.as_ref().is_some_and(|d| gone(d.hwnd, d.seen)) {
            self.drag = None;
            right_button(false);
        }
        let lost: Vec<(Uuid, u32)> = self.fingers.iter().filter(|f| gone(f.hwnd, f.seen)).map(|f| (f.owner, f.phone)).collect();
        for (owner, phone) in lost {
            lift(&mut self.fingers, owner, phone);
        }
    }
}

/// Drops one held key; the key goes up unless another session still holds it.
fn let_go(keys: &mut Vec<Key>, i: usize) {
    let k = keys.remove(i);
    if !keys.iter().any(|other| other.key == k.key) {
        send_key(k.key, false);
    }
}

fn stale(seen: Instant, now: Instant) -> bool {
    now.saturating_duration_since(seen) >= STALE
}

fn superseded(released: &[(Uuid, u64)], owner: Uuid, seq: u64) -> bool {
    released.iter().any(|&(o, at)| o == owner && seq < at)
}

/// Notes that `owner` let go at `at`: one entry per session, however often it lets go.
fn remember(released: &mut Vec<(Uuid, u64)>, owner: Uuid, at: u64) {
    match released.iter_mut().find(|(o, _)| *o == owner) {
        Some(entry) => entry.1 = entry.1.max(at),
        None => released.push((owner, at)),
    }
}

/// Whether an event may run now: while Windows is locked, or RoALT is exiting, only lifts do (pure).
fn runs(ev: &Event, blocked: bool) -> bool {
    !blocked || ev.is_release()
}

fn blocked() -> bool {
    CLOSED.load(Ordering::SeqCst) || crate::session::locked()
}

fn take_slot(queued: &mut Vec<(Uuid, usize, usize)>, device: Uuid, release: bool) -> bool {
    let i = match queued.iter().position(|(d, _, _)| *d == device) {
        Some(i) => i,
        None => {
            queued.push((device, 0, 0));
            queued.len() - 1
        }
    };
    let count = if release { &mut queued[i].2 } else { &mut queued[i].1 };
    if *count >= DEVICE_QUEUE {
        return false;
    }
    *count += 1;
    true
}

fn free_slot(queued: &mut Vec<(Uuid, usize, usize)>, device: Uuid, release: bool) {
    if let Some(i) = queued.iter().position(|(d, _, _)| *d == device) {
        let count = if release { &mut queued[i].2 } else { &mut queued[i].1 };
        *count = count.saturating_sub(1);
        if queued[i].1 == 0 && queued[i].2 == 0 {
            queued.swap_remove(i);
        }
    }
}

fn finger_free(down: &[Finger], device: Uuid) -> bool {
    down.len() < MAX_FINGERS && down.iter().filter(|f| f.device == device).count() < DEVICE_FINGERS
}

fn same_client(client: Client) -> bool {
    process::start_time(client.pid) == client.started
}

/// Whether a remote session holds a key, finger or camera drag right now (anti-AFK leaves the
/// focus alone meanwhile).
pub fn holding() -> bool {
    HOLDING.load(Ordering::SeqCst)
}

type Queued = (u32, u64, Uuid, Event);

static CLOSED: AtomicBool = AtomicBool::new(false);
static HOLDING: AtomicBool = AtomicBool::new(false);
static QUEUED: Mutex<Vec<(Uuid, usize, usize)>> = Mutex::new(Vec::new());
static GENERATION: AtomicU32 = AtomicU32::new(0);
static SEQ: AtomicU64 = AtomicU64::new(0);
static TX: OnceLock<SyncSender<Queued>> = OnceLock::new();
static LEFT: Mutex<Vec<(Uuid, u64)>> = Mutex::new(Vec::new());
static LET_GO_AT: Mutex<u32> = Mutex::new(0);
static LET_GO: Condvar = Condvar::new();
/// The movement keys (`MoveKey::bit`) and the right button (`RIGHT_BUTTON`) this process holds down.
static PRESSED: AtomicU8 = AtomicU8::new(0);
const RIGHT_BUTTON: u8 = 1 << 7;
/// When the event the input thread is running goes stale (`clock_ms`), or 0 while it waits.
static BUSY_UNTIL: AtomicU64 = AtomicU64::new(0);
static EPOCH: OnceLock<Instant> = OnceLock::new();

fn clock_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

fn busy_until(now: u64, wait: Duration) -> u64 {
    now + wait.as_millis() as u64
}

fn busy_for(wait: Duration) {
    BUSY_UNTIL.store(busy_until(clock_ms(), wait), Ordering::SeqCst);
}

fn restoring() -> Duration {
    STALE + window::UNHIDE_TIME + RESTORED
}

fn worker() -> &'static SyncSender<Queued> {
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Queued>(QUEUE);
        let started = std::thread::Builder::new().name("remote-input".into()).spawn(move || {
            // SAFETY: plain call; sets up touch injection for this process.
            // Without it touches fail (logged per frame); the movement keys still work.
            if let Err(e) = unsafe { InitializeTouchInjection(MAX_FINGERS as u32, TOUCH_FEEDBACK_NONE) } {
                log::warn!("remote touch: touch injection not available: {e}");
            }
            let mut held = Held::default();
            let mut generation = GENERATION.load(Ordering::SeqCst);
            let mut released: Vec<(Uuid, u64)> = Vec::new();
            loop {
                let next = rx.recv_timeout(KEEP_ALIVE);
                busy_for(STALE);
                let now_generation = GENERATION.load(Ordering::SeqCst);
                if now_generation != generation {
                    generation = now_generation;
                    held.release(|_| true);
                    *LET_GO_AT.lock().unwrap_or_else(PoisonError::into_inner) = generation;
                    LET_GO.notify_all();
                }
                let left = std::mem::take(&mut *LEFT.lock().unwrap_or_else(PoisonError::into_inner));
                if !left.is_empty() {
                    held.release(|owner| left.iter().any(|&(o, _)| o == owner));
                    for (owner, at) in left {
                        remember(&mut released, owner, at);
                    }
                    let over = released.len().saturating_sub(GONE_KEPT);
                    released.drain(..over);
                }
                let locked = blocked();
                if locked {
                    held.release(|_| true);
                }
                if let Ok((_, _, device, ev)) = &next {
                    free_slot(&mut QUEUED.lock().unwrap_or_else(PoisonError::into_inner), *device, ev.is_release());
                }
                match next {
                    Ok((g, seq, _, ev)) if g != generation || superseded(&released, ev.owner(), seq) || !runs(&ev, locked) => {}
                    Ok((_, _, _, Event::Key { owner, client, key, down, repeat })) => hold(&mut held.keys, owner, client, key, down, repeat),
                    Ok((_, _, _, Event::Camera { owner, client, x, y, phase })) => camera(&mut held.drag, owner, client, x, y, phase),
                    Ok((_, _, device, Event::Touch { owner, client, finger, x, y, phase })) => {
                        touch_event(&mut held.fingers, (owner, device), client, finger, x, y, phase);
                    }
                    Err(RecvTimeoutError::Timeout) if !held.fingers.is_empty() => inject(&held.fingers, None),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                held.expire(Instant::now());
                HOLDING.store(!held.fingers.is_empty() || !held.keys.is_empty() || held.drag.is_some(), Ordering::SeqCst);
                BUSY_UNTIL.store(0, Ordering::SeqCst);
            }
        });
        if let Err(e) = started {
            log::warn!("remote touch off: {e}");
        }
        if let Err(e) = std::thread::Builder::new().name("remote-input-deadman".into()).spawn(deadman) {
            log::warn!("remote input: no deadman: {e}");
        }
        tx
    })
}

/// Lets go of every key and the right button, and drops what the input thread holds and has
/// queued, whenever that thread is stuck on one event for `STALE` (a window it waits for hung).
/// Touch contacts need nothing: Windows cancels one that stops getting frames.
fn deadman() {
    let mut fired = 0;
    loop {
        sleep(KEEP_ALIVE);
        if overdue(BUSY_UNTIL.load(Ordering::SeqCst), &mut fired, clock_ms()) {
            log::warn!("remote input: the input thread is stuck; letting go of everything");
            release_all();
            lift_pressed();
            HOLDING.store(false, Ordering::SeqCst);
        }
    }
}

/// Whether the event that goes stale at `deadline` is overdue at `now` and wasn't let go of yet (pure).
fn overdue(deadline: u64, fired: &mut u64, now: u64) -> bool {
    let due = deadline != 0 && now >= deadline && *fired != deadline;
    if due {
        *fired = deadline;
    }
    due
}

/// Sends a key-up for every movement key and the right button still down; a second up does nothing.
fn lift_pressed() {
    let pressed = PRESSED.swap(0, Ordering::SeqCst);
    for key in held_keys(pressed) {
        send_key(key, false);
    }
    if pressed & RIGHT_BUTTON != 0 {
        right_button(false);
    }
}

fn held_keys(pressed: u8) -> impl Iterator<Item = MoveKey> {
    MoveKey::ALL.into_iter().filter(move |k| pressed & k.bit() != 0)
}

fn mark(pressed: &AtomicU8, bit: u8, down: bool) {
    if down {
        pressed.fetch_or(bit, Ordering::SeqCst);
    } else {
        pressed.fetch_and(!bit, Ordering::SeqCst);
    }
}

fn send(device: Uuid, event: Event) -> bool {
    if CLOSED.load(Ordering::SeqCst) {
        return false;
    }
    let (owner, release) = (event.owner(), event.is_release());
    if !take_slot(&mut QUEUED.lock().unwrap_or_else(PoisonError::into_inner), device, release) {
        if release {
            release_owner(owner);
        }
        return release;
    }
    let seq = SEQ.fetch_add(1, Ordering::SeqCst);
    let Err(e) = worker().try_send((GENERATION.load(Ordering::SeqCst), seq, device, event)) else { return true };
    free_slot(&mut QUEUED.lock().unwrap_or_else(PoisonError::into_inner), device, release);
    log::debug!("remote input: queue full, event dropped");
    if release && matches!(e, TrySendError::Full(_)) {
        release_owner(owner);
        return true;
    }
    false
}

/// `owner` is the session, `device` the paired device it belongs to: a device's sessions share
/// its queue and fingers. A lift over the device's share lets go of all the session holds.
pub fn touch(owner: Uuid, device: Uuid, client: Client, finger: u32, x: f32, y: f32, phase: Phase) -> bool {
    send(device, Event::Touch { owner, client, finger, x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0), phase })
}

pub fn camera_drag(owner: Uuid, device: Uuid, client: Client, x: f32, y: f32, phase: Phase) -> bool {
    send(device, Event::Camera { owner, client, x: x.clamp(0.0, 1.0), y: y.clamp(0.0, 1.0), phase })
}

pub fn key(owner: Uuid, device: Uuid, client: Client, key: MoveKey, down: bool, repeat: bool) -> bool {
    send(device, Event::Key { owner, client, key, down, repeat })
}

/// Lifts every finger and key anyone still has down (Windows locked, the relay dropped, Remote
/// paused) and drops the events still queued, within one `KEEP_ALIVE` or once the event being run
/// is done. Never waits.
pub fn release_all() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
}

/// As `release_all`, for what one session holds and has queued so far (it left, its role
/// changed, or it let go faster than its budget); what it sends afterwards still goes through.
/// Never touches another session's input.
pub fn release_owner(owner: Uuid) {
    if TX.get().is_none() {
        return;
    }
    let at = SEQ.load(Ordering::SeqCst);
    remember(&mut LEFT.lock().unwrap_or_else(PoisonError::into_inner), owner, at);
}

/// Lets go of everything held now, waiting up to `wait` for the input thread to do it (keys and
/// the right button go up from here if it doesn't), and takes no input afterwards: RoALT is
/// exiting or restarting, and the thread dies with the process.
pub fn release_now(wait: Duration) {
    CLOSED.store(true, Ordering::SeqCst);
    if TX.get().is_none() {
        return;
    }
    let target = GENERATION.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
    let done = LET_GO_AT.lock().unwrap_or_else(PoisonError::into_inner);
    drop(LET_GO.wait_timeout_while(done, wait, |at| (at.wrapping_sub(target) as i32) < 0));
    lift_pressed();
}

fn touch_event(down: &mut Vec<Finger>, (owner, device): (Uuid, Uuid), client: Client, finger: u32, x: f32, y: f32, phase: Phase) {
    let known = down.iter().position(|f| f.owner == owner && f.phone == finger);
    match (phase, known) {
        (Phase::Down, None) => {
            if !finger_free(down, device) || !front(client) {
                return;
            }
            let Some((hwnd, _)) = process::main_window(client.pid) else { return };
            let Some(at) = point_at(hwnd, x, y).filter(|&at| hits(hwnd, at)) else { return };
            if blocked() {
                return;
            }
            let contact = (0..MAX_FINGERS as u32).find(|c| !down.iter().any(|f| f.contact == *c)).unwrap_or(0);
            down.push(Finger { owner, device, phone: finger, contact, hwnd, at, seen: Instant::now() });
            inject(down, Some((contact, POINTER_FLAG_DOWN | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT)));
        }
        (Phase::Move, Some(i)) => {
            let hwnd = down[i].hwnd;
            if !in_front(hwnd) {
                lift(down, owner, finger);
                return;
            }
            down[i].seen = Instant::now();
            if let Some(at) = point_at(hwnd, x, y) {
                down[i].at = at;
                inject(down, None);
            }
        }
        (Phase::Up, Some(_)) => lift(down, owner, finger),
        // A move or lift for a finger that never got down (its client wasn't in front), or a second down.
        _ => {}
    }
}

/// A finger dragged on the picture turns the camera: the right button goes down where it landed,
/// each move turns by as many pixels as the finger went over the window, and it lets go at the end.
/// Moves are relative: while the right button is held Roblox pins the cursor and reads how far the
/// mouse went. One drag at a time, moved and let go only by the session that started it; a let-go
/// never moves (the phone sends its last stretch as a move first), and a drag whose client left the
/// front is let go instead of moved. A move never carries the cursor past the client's edge: until the
/// client pulls it back, the turn stops there instead of letting go.
fn camera(drag: &mut Option<Drag>, owner: Uuid, client: Client, x: f32, y: f32, phase: Phase) {
    match drag_step(drag.as_ref().map(|d| (d.owner, d.pid)), owner, client.pid, phase) {
        DragStep::Ignore => {}
        DragStep::Start => {
            if !front(client) {
                return;
            }
            let Some((hwnd, _)) = process::main_window(client.pid) else { return };
            let Some(at) = point_at(hwnd, x, y).filter(|&at| hits(hwnd, at)) else { return };
            // SAFETY: plain call; a physical-pixel screen point (this process is DPI aware).
            let _ = unsafe { SetCursorPos(at.x, at.y) };
            sleep(Duration::from_millis(20));
            if blocked() || !cursor_on(hwnd) {
                return;
            }
            right_button(true);
            *drag = Some(Drag { owner, pid: client.pid, hwnd, at, seen: Instant::now() });
        }
        DragStep::Turn => {
            let Some(d) = drag.as_mut() else { return };
            let Some((r, _)) = window::client_in_frame(d.hwnd).filter(|_| in_front(d.hwnd)) else {
                *drag = None;
                right_button(false);
                return;
            };
            d.seen = Instant::now();
            let at = scale(r, x, y);
            let (dx, dy) = cursor().filter(|&cur| hits(d.hwnd, cur)).map_or((0, 0), |cur| stop_at_edge(r, cur, at.x - d.at.x, at.y - d.at.y));
            d.at = at;
            if dx != 0 || dy != 0 {
                mouse(dx, dy, MOUSEEVENTF_MOVE);
            }
        }
        DragStep::LetGo => {
            *drag = None;
            right_button(false);
        }
    }
}

#[derive(Debug, PartialEq)]
enum DragStep {
    Ignore,
    Start,
    Turn,
    LetGo,
}

/// What a camera event does to the drag under way, `(owner, pid)` (pure).
fn drag_step(current: Option<(Uuid, u32)>, owner: Uuid, pid: u32, phase: Phase) -> DragStep {
    match (phase, current) {
        (Phase::Down, None) => DragStep::Start,
        (Phase::Move, Some((o, p))) if o == owner && p == pid => DragStep::Turn,
        (Phase::Up, Some((o, _))) if o == owner => DragStep::LetGo,
        _ => DragStep::Ignore,
    }
}

fn right_button(down: bool) {
    mark(&PRESSED, RIGHT_BUTTON, down);
    mouse(0, 0, if down { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP });
}

fn mouse(dx: i32, dy: i32, flags: MOUSE_EVENT_FLAGS) {
    let input = INPUT { r#type: INPUT_MOUSE, Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, dwFlags: flags, ..Default::default() } } };
    // SAFETY: one fully initialised mouse INPUT and its size, as SendInput requires.
    if unsafe { SendInput(&[input], size_of::<INPUT>() as i32) } == 0 {
        log::debug!("remote input: no mouse input sent: {}", std::io::Error::last_os_error());
    }
}

/// Presses, refreshes or lets go of a movement key; a let-go always goes out, so no key stays down.
/// A refresh (`repeat`) of a key that was let go meanwhile (its client left the front, or it went
/// stale) does nothing: only a fresh press brings the client forward and presses again.
fn hold(keys: &mut Vec<Key>, owner: Uuid, client: Client, key: MoveKey, down: bool, repeat: bool) {
    let known = keys.iter().position(|k| k.owner == owner && k.key == key);
    match (down, known) {
        (true, Some(i)) => keys[i].seen = Instant::now(),
        (true, None) if repeat => {}
        (true, None) => {
            // Keys go to the window in front: only press while the client is it.
            if !front(client) {
                return;
            }
            let Some((hwnd, _)) = process::main_window(client.pid) else { return };
            if blocked() {
                return;
            }
            let pressed = keys.iter().any(|k| k.key == key);
            keys.push(Key { owner, key, hwnd, seen: Instant::now() });
            if !pressed {
                send_key(key, true);
            }
        }
        (false, Some(i)) => let_go(keys, i),
        (false, None) => {}
    }
}

fn send_key(key: MoveKey, down: bool) {
    mark(&PRESSED, key.bit(), down);
    let flags = if down { KEYEVENTF_SCANCODE } else { KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP };
    afk::key(VIRTUAL_KEY(0), key.scan(), flags);
}

/// Lifts one finger: a frame where it goes up and the others stay, then it is gone.
fn lift(down: &mut Vec<Finger>, owner: Uuid, phone: u32) {
    let Some(i) = down.iter().position(|f| f.owner == owner && f.phone == phone) else { return };
    inject(down, Some((down[i].contact, POINTER_FLAG_UP)));
    down.remove(i);
}

/// One frame with every finger down (Windows wants them all in each frame): `change` is the one
/// going down or up, the rest are held where they are.
fn inject(down: &[Finger], change: Option<(u32, POINTER_FLAGS)>) {
    let frame: Vec<POINTER_TOUCH_INFO> = down
        .iter()
        .map(|f| {
            let flags = match change {
                Some((c, flags)) if c == f.contact => flags,
                _ => POINTER_FLAG_UPDATE | POINTER_FLAG_INRANGE | POINTER_FLAG_INCONTACT,
            };
            contact(f.contact, f.at, flags)
        })
        .collect();
    // SAFETY: a slice of fully initialised touch contacts, as InjectTouchInput requires.
    if let Err(e) = unsafe { InjectTouchInput(&frame) } {
        log::debug!("remote touch: frame not injected: {e}");
    }
}

/// A finger of an ordinary size and pressure at `at` (pure).
fn contact(id: u32, at: POINT, flags: POINTER_FLAGS) -> POINTER_TOUCH_INFO {
    POINTER_TOUCH_INFO {
        pointerInfo: POINTER_INFO { pointerType: PT_TOUCH, pointerId: id, pointerFlags: flags, ptPixelLocation: at, ..Default::default() },
        touchMask: TOUCH_MASK_CONTACTAREA | TOUCH_MASK_ORIENTATION | TOUCH_MASK_PRESSURE,
        rcContact: RECT { left: at.x - 2, top: at.y - 2, right: at.x + 2, bottom: at.y + 2 },
        orientation: 90,
        pressure: 32_000,
        ..Default::default()
    }
}

fn in_front(hwnd: HWND) -> bool {
    // SAFETY: reads which window is in front.
    unsafe { GetForegroundWindow() == hwnd }
}

/// The client's window in front, restored first if minimized; false if the pid is no longer that
/// client, it has no window, Windows wouldn't let it come forward (touch lands on whatever window is
/// at the spot), or nothing may be pressed by the time it is there.
fn front(client: Client) -> bool {
    if !same_client(client) || process::hung(client.pid) {
        return false;
    }
    let pid = client.pid;
    let Some((hwnd, minimized)) = process::main_window(pid) else { return false };
    if !minimized && in_front(hwnd) {
        return !blocked();
    }
    if minimized {
        busy_for(restoring());
        window::unhide(pid, hwnd);
        sleep(RESTORED);
        busy_for(STALE);
    }
    if blocked() {
        return false;
    }
    let ok = afk::to_front(hwnd);
    if !ok {
        log::info!("remote touch: client {pid} couldn't be brought to the front");
    }
    ok && !blocked()
}

/// Whether the cursor is on `hwnd`'s client area, with `hwnd` in front and nothing over it there.
fn cursor_on(hwnd: HWND) -> bool {
    cursor().is_some_and(|at| in_front(hwnd) && window::client_in_frame(hwnd).is_some_and(|(r, _)| inside(r, at)) && hits(hwnd, at))
}

fn cursor() -> Option<POINT> {
    let mut at = POINT::default();
    // SAFETY: writes the cursor's screen position into `at`.
    unsafe { GetCursorPos(&mut at) }.is_ok().then_some(at)
}

fn stop_at_edge(r: RECT, cur: POINT, dx: i32, dy: i32) -> (i32, i32) {
    if !inside(r, cur) {
        return (0, 0);
    }
    ((cur.x + dx).min(r.right - 1).max(r.left) - cur.x, (cur.y + dy).min(r.bottom - 1).max(r.top) - cur.y)
}

fn inside(r: RECT, at: POINT) -> bool {
    (r.left..r.right).contains(&at.x) && (r.top..r.bottom).contains(&at.y)
}

fn hits(hwnd: HWND, at: POINT) -> bool {
    // SAFETY: reads which top-level window is at a screen point.
    unsafe { GetAncestor(WindowFromPoint(at), GA_ROOT) == hwnd }
}

/// `x`, `y` (0..=1 across the client area, which is what the capture shows) as a screen pixel.
fn point_at(hwnd: HWND, x: f32, y: f32) -> Option<POINT> {
    window::client_in_frame(hwnd).map(|(client, _)| scale(client, x, y))
}

/// `x`, `y` (0..=1) inside `r`, on its last pixel at 1 (pure).
fn scale(r: RECT, x: f32, y: f32) -> POINT {
    POINT {
        x: r.left + (x * (r.right - r.left - 1).max(0) as f32).round() as i32,
        y: r.top + (y * (r.bottom - r.top - 1).max(0) as f32).round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_joystick_and_jump_keys_parse() {
        assert_eq!(MoveKey::parse("KeyW").map(MoveKey::scan), Some(0x11));
        assert_eq!(MoveKey::parse("KeyD").map(MoveKey::scan), Some(0x20));
        assert_eq!(MoveKey::parse("Space").map(MoveKey::scan), Some(0x39));
        assert_eq!(MoveKey::parse("KeyQ"), None);
        assert_eq!(MoveKey::parse("Escape"), None);
    }

    #[test]
    fn corners_of_the_picture_land_on_the_client_area() {
        let r = RECT { left: 100, top: 50, right: 900, bottom: 650 };
        assert_eq!(scale(r, 0.0, 0.0), POINT { x: 100, y: 50 });
        assert_eq!(scale(r, 1.0, 1.0), POINT { x: 899, y: 649 });
        assert_eq!(scale(r, 0.5, 0.5), POINT { x: 500, y: 350 });
    }

    /// Whether Windows lets this PC inject touch at all (touches nothing): `cargo test -- --ignored touch_injection`.
    #[test]
    #[ignore]
    fn touch_injection_is_available() {
        // SAFETY: plain call.
        unsafe { InitializeTouchInjection(MAX_FINGERS as u32, TOUCH_FEEDBACK_NONE) }.expect("touch injection");
    }

    #[test]
    fn only_the_session_that_started_a_drag_moves_or_ends_it_and_a_lift_never_moves() {
        let (mine, theirs) = (Uuid::new_v4(), Uuid::new_v4());
        assert_eq!(drag_step(None, mine, 7, Phase::Down), DragStep::Start);
        assert_eq!(drag_step(Some((mine, 7)), theirs, 7, Phase::Down), DragStep::Ignore);
        assert_eq!(drag_step(Some((mine, 7)), mine, 7, Phase::Move), DragStep::Turn);
        assert_eq!(drag_step(Some((mine, 7)), theirs, 7, Phase::Move), DragStep::Ignore);
        assert_eq!(drag_step(Some((mine, 7)), mine, 8, Phase::Move), DragStep::Ignore);
        assert_eq!(drag_step(Some((mine, 7)), theirs, 7, Phase::Up), DragStep::Ignore);
        assert_eq!(drag_step(Some((mine, 7)), mine, 7, Phase::Up), DragStep::LetGo);
        assert_eq!(drag_step(None, mine, 7, Phase::Up), DragStep::Ignore);
        assert_eq!(drag_step(None, mine, 7, Phase::Move), DragStep::Ignore);
    }

    #[test]
    fn a_refreshed_key_that_was_let_go_is_not_pressed_again() {
        let mut keys = Vec::new();
        hold(&mut keys, Uuid::new_v4(), Client::NONE, MoveKey::W, true, true);
        assert!(keys.is_empty(), "a refresh must not bring a client forward or press");
    }

    #[test]
    fn a_press_for_a_pid_that_now_belongs_to_another_process_goes_nowhere() {
        let me = Client { pid: std::process::id(), started: process::start_time(std::process::id()) };
        assert!(same_client(me));
        assert!(!same_client(Client { started: me.started.map(|t| t + 1), ..me }));
        let mut keys = Vec::new();
        hold(&mut keys, Uuid::new_v4(), Client { started: me.started.map(|t| t + 1), ..me }, MoveKey::W, true, false);
        assert!(keys.is_empty());
    }

    #[test]
    fn one_device_cannot_fill_the_queue_for_another_with_several_sessions_or_lifts() {
        let (flooder, other) = (Uuid::new_v4(), Uuid::new_v4());
        let mut queued = Vec::new();
        let presses = (0..3 * QUEUE).filter(|_| take_slot(&mut queued, flooder, false)).count();
        assert_eq!(presses, DEVICE_QUEUE, "its sessions share one share");
        let lifts = (0..3 * QUEUE).filter(|_| take_slot(&mut queued, flooder, true)).count();
        assert_eq!(lifts, DEVICE_QUEUE);
        assert!(presses + lifts < QUEUE);
        assert!(take_slot(&mut queued, other, false));
        for _ in 0..presses {
            free_slot(&mut queued, flooder, false);
        }
        for _ in 0..lifts {
            free_slot(&mut queued, flooder, true);
        }
        assert_eq!(queued, vec![(other, 1, 0)]);
    }

    #[test]
    fn one_device_holds_at_most_half_the_fingers() {
        let (greedy, other) = (Uuid::new_v4(), Uuid::new_v4());
        let finger = |device, contact| Finger { owner: Uuid::new_v4(), device, phone: contact, contact, hwnd: HWND::default(), at: POINT::default(), seen: Instant::now() };
        let mut down: Vec<Finger> = (0..DEVICE_FINGERS as u32).map(|c| finger(greedy, c)).collect();
        assert!(!finger_free(&down, greedy));
        assert!(finger_free(&down, other));
        down.extend((0..DEVICE_FINGERS as u32).map(|c| finger(other, c + 10)));
        assert!(!finger_free(&down, Uuid::new_v4()), "nobody gets past the ten Windows takes");
    }

    #[test]
    fn nothing_is_queued_once_roalt_is_exiting() {
        release_now(Duration::ZERO);
        assert!(blocked());
        assert!(!key(Uuid::new_v4(), Uuid::new_v4(), Client::NONE, MoveKey::W, true, false));
        assert!(!touch(Uuid::new_v4(), Uuid::new_v4(), Client::NONE, 0, 0.5, 0.5, Phase::Up));
    }

    #[test]
    fn held_input_goes_stale_after_three_quarters_of_a_second() {
        let t0 = Instant::now();
        assert!(!stale(t0, t0 + Duration::from_millis(749)));
        assert!(stale(t0, t0 + STALE));
        assert!(!stale(t0 + Duration::from_secs(1), t0));
    }

    #[test]
    fn a_release_drops_only_what_the_owner_queued_before_it() {
        let (owner, other) = (Uuid::new_v4(), Uuid::new_v4());
        let released = [(owner, 10)];
        assert!(superseded(&released, owner, 9));
        assert!(!superseded(&released, owner, 10), "input sent after a release (rejoin, role change) must apply");
        assert!(!superseded(&released, other, 3));
    }

    #[test]
    fn a_session_letting_go_over_and_over_keeps_one_entry_and_never_reaches_others() {
        let (flooder, other) = (Uuid::new_v4(), Uuid::new_v4());
        let mut released = vec![(other, 3)];
        for at in 0..10_000 {
            remember(&mut released, flooder, at);
        }
        remember(&mut released, flooder, 5);
        assert_eq!(released, vec![(other, 3), (flooder, 9_999)]);
        assert!(!superseded(&released, other, 3), "another session's later input still applies");
    }

    #[test]
    fn nothing_but_lifts_runs_while_windows_is_locked() {
        let o = Uuid::new_v4();
        let press = Event::Key { owner: o, client: Client::NONE, key: MoveKey::W, down: true, repeat: false };
        let lift = Event::Touch { owner: o, client: Client::NONE, finger: 0, x: 0.0, y: 0.0, phase: Phase::Up };
        assert!(runs(&press, false) && !runs(&press, true));
        assert!(runs(&lift, true));
        assert!(!runs(&Event::Camera { owner: o, client: Client::NONE, x: 0.0, y: 0.0, phase: Phase::Move }, true));
    }

    #[test]
    fn lifts_are_releases_and_presses_are_not() {
        let o = Uuid::new_v4();
        assert!(Event::Key { owner: o, client: Client::NONE, key: MoveKey::W, down: false, repeat: false }.is_release());
        assert!(Event::Camera { owner: o, client: Client::NONE, x: 0.0, y: 0.0, phase: Phase::Up }.is_release());
        assert!(!Event::Touch { owner: o, client: Client::NONE, finger: 0, x: 0.0, y: 0.0, phase: Phase::Move }.is_release());
    }

    #[test]
    fn a_stuck_input_thread_is_let_go_of_once_per_stuck_event() {
        let mut fired = 0;
        assert!(!overdue(0, &mut fired, 10_000), "a thread waiting for events is not stuck");
        assert!(!overdue(1_000, &mut fired, 999));
        assert!(overdue(1_000, &mut fired, 1_000));
        assert!(!overdue(1_000, &mut fired, 5_000), "the same stuck event is let go of once");
        assert!(overdue(6_000, &mut fired, 6_100), "a later stuck event is let go of again");
    }

    #[test]
    fn restoring_a_minimized_client_for_as_long_as_it_may_take_is_not_a_stuck_thread() {
        let mut fired = 0;
        let longest = (window::UNHIDE_TIME + RESTORED).as_millis() as u64;
        assert!(!overdue(busy_until(0, restoring()), &mut fired, longest));
        assert!(overdue(busy_until(0, STALE), &mut fired, longest), "an ordinary event that long is");
    }

    #[test]
    fn what_is_pressed_is_tracked_so_exit_can_lift_it() {
        let pressed = AtomicU8::new(0);
        mark(&pressed, MoveKey::W.bit(), true);
        mark(&pressed, MoveKey::Jump.bit(), true);
        mark(&pressed, RIGHT_BUTTON, true);
        mark(&pressed, MoveKey::W.bit(), false);
        mark(&pressed, MoveKey::A.bit(), false);
        let bits = pressed.load(Ordering::SeqCst);
        assert_eq!(held_keys(bits).collect::<Vec<_>>(), [MoveKey::Jump]);
        assert_ne!(bits & RIGHT_BUTTON, 0);
        assert!(MoveKey::ALL.iter().all(|k| k.bit() & RIGHT_BUTTON == 0));
        mark(&pressed, RIGHT_BUTTON, false);
        assert_eq!(pressed.load(Ordering::SeqCst), MoveKey::Jump.bit());
    }

    #[test]
    fn a_drag_point_is_on_the_client_area_only_inside_it() {
        let r = RECT { left: 100, top: 50, right: 900, bottom: 650 };
        assert!(inside(r, POINT { x: 100, y: 50 }) && inside(r, POINT { x: 899, y: 649 }));
        assert!(!inside(r, POINT { x: 900, y: 300 }) && !inside(r, POINT { x: 500, y: 49 }));
    }

    #[test]
    fn a_camera_turn_stops_at_the_client_edge_and_waits_while_the_cursor_is_off_it() {
        let r = RECT { left: 100, top: 50, right: 900, bottom: 650 };
        let mid = POINT { x: 500, y: 350 };
        assert_eq!(stop_at_edge(r, mid, 40, -30), (40, -30));
        assert_eq!(stop_at_edge(r, mid, 1_000, -1_000), (399, -300));
        assert_eq!(stop_at_edge(r, POINT { x: 899, y: 50 }, 25, -25), (0, 0));
        assert_eq!(stop_at_edge(r, POINT { x: 899, y: 50 }, -25, 25), (-25, 25));
        assert_eq!(stop_at_edge(r, POINT { x: 950, y: 350 }, -25, 0), (0, 0));
    }

    #[test]
    fn a_contact_is_a_small_touch_around_the_point() {
        let c = contact(3, POINT { x: 10, y: 20 }, POINTER_FLAG_DOWN);
        assert_eq!(c.pointerInfo.pointerType, PT_TOUCH);
        assert_eq!(c.pointerInfo.pointerId, 3);
        assert_eq!(c.rcContact, RECT { left: 8, top: 18, right: 12, bottom: 22 });
    }
}
