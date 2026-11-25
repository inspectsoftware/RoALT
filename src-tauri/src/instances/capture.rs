//! Live pictures of client windows through Windows.Graphics.Capture, which copies what the
//! compositor already shows and never touches the client's own frames. (`PrintWindow` with
//! `PW_RENDERFULLCONTENT`, used before, made Roblox windows flash white while the dashboard
//! drew them.)
//!
//! One worker thread owns a Direct3D device and a capture per window the dashboard asks for; a
//! capture nobody asked for in `IDLE` is closed. Each picture is scaled down on the GPU (mipmaps)
//! and only that small copy is read back, once the GPU is done with it: the worker never waits for
//! a GPU busy with the games, it looks again `SOON`. Asking (`request`) never waits for the worker
//! and the newest picture is kept per window and size (`last`), so a stuck worker only makes pictures older,
//! and they outlive the capture. Where Windows can't capture without its yellow frame around the game
//! (before Windows 11 / Windows 10 21H2 server), nothing is captured for the dashboard: the frame would
//! sit around the client the user plays. There (`unsupported`) Windows draws the cards' pictures
//! (`thumbs`); only the Remote Dashboard (`remote`) captures, with the frame, while a phone watches.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Win32::Foundation::{HMODULE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_FLAG_DO_NOT_WAIT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_WAS_STILL_DRAWING, IDXGIDevice};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::UI::WindowsAndMessaging::IsWindow;
use windows::core::{Interface, Result, factory};

use super::window::{Shot, Thumb};

/// A capture nobody asked a picture of for this long is closed.
const IDLE: Duration = Duration::from_secs(5);
/// How soon the worker looks again at a copy the GPU hasn't finished, or a capture that has no frame yet.
const SOON: Duration = Duration::from_millis(15);
/// How long a new capture is looked at `SOON` for its first frame, without being asked again.
const FIRST_WAIT: Duration = Duration::from_secs(1);
/// A window Windows refused to capture is tried again after this long.
const RETRY: Duration = Duration::from_secs(5);
const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

struct Request {
    hwnd: isize,
    max_w: u32,
    /// For the Remote Dashboard, which may capture with Windows' yellow frame (`BORDERED`).
    remote: bool,
}

/// The newest pictures of each window, one per size asked for; a window that is gone loses them in
/// the sweep (`run`).
static SHOTS: LazyLock<Mutex<HashMap<isize, Pictures>>> = LazyLock::new(Mutex::default);
/// Numbers pictures from 1, so the dashboard can skip one it has (0: it has none).
static NEXT_SEQ: AtomicU32 = AtomicU32::new(1);
/// Capturing doesn't work on this PC: the worker is gone.
static UNAVAILABLE: AtomicBool = AtomicBool::new(false);
/// Windows here only captures with its yellow border: the desktop dashboard captures nothing (it
/// uses `thumbs`), the Remote Dashboard does, the user having accepted the border while a phone watches.
static BORDERED: AtomicBool = AtomicBool::new(false);

/// This request may not capture: the worker is gone, or it's the desktop's and only bordered capture works.
fn barred(remote: bool) -> bool {
    UNAVAILABLE.load(Ordering::Relaxed) || (!remote && BORDERED.load(Ordering::Relaxed))
}
/// No Direct3D device could be made, the last time one was wanted.
static NO_GPU: AtomicBool = AtomicBool::new(false);
/// Windows that exist but whose capture Windows refused, and when; each leaves with its first picture.
static REFUSED: LazyLock<Mutex<HashMap<isize, Instant>>> = LazyLock::new(Mutex::default);

#[derive(Default)]
struct Pictures {
    width: u32,
    by_level: HashMap<u32, Shot>,
}

impl Pictures {
    fn put(&mut self, width: u32, level: u32, shot: Shot) {
        if self.width != width {
            self.by_level.clear();
            self.width = width;
        }
        self.by_level.insert(level, shot);
    }

    fn pick(&self, max_w: Option<u32>, remote: bool) -> Option<&Shot> {
        let newest = |w: u32| self.by_level.values().filter(|s| s.thumb.width <= w).max_by_key(|s| s.seq);
        let Some(w) = max_w else { return newest(u32::MAX) };
        self.by_level
            .get(&level_for(self.width, w))
            .filter(|s| s.thumb.width <= w)
            .or_else(|| newest(w))
            .or_else(|| if remote { None } else { newest(u32::MAX) })
    }
}

fn shots() -> MutexGuard<'static, HashMap<isize, Pictures>> {
    SHOTS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn refused() -> MutexGuard<'static, HashMap<isize, Instant>> {
    REFUSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Asks for a new picture of the window, at most `max_w` pixels wide (a power-of-two step down from
/// its size); `last` has it once a frame came. The window must not be minimized (nothing is drawn
/// then). Nothing where capturing doesn't work (`barred`). ponytail: the queue has no bound, a stuck
/// worker costs some 16 bytes per request.
pub fn request(hwnd: HWND, max_w: u32, remote: bool) {
    if barred(remote) {
        return;
    }
    static WORKER: OnceLock<Sender<Request>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("capture".into()).spawn(move || run(&rx)) {
            log::warn!("live previews off: {e}");
        }
        tx
    });
    if worker.send(Request { hwnd: hwnd.0 as isize, max_w, remote }).is_err() {
        static DEAD: std::sync::Once = std::sync::Once::new();
        DEAD.call_once(|| log::warn!("live previews off: the capture worker is not running"));
        UNAVAILABLE.store(true, Ordering::Relaxed);
    }
}

/// The window's newest picture at most `max_w` wide (`Pictures::pick`), however old (none for the
/// desktop where only bordered capture works: a picture taken for a phone would freeze on the card
/// once the phone stops watching).
pub fn last(hwnd: HWND, max_w: Option<u32>, remote: bool) -> Option<Shot> {
    if barred(remote) {
        return None;
    }
    shots().get(&(hwnd.0 as isize)).and_then(|p| p.pick(max_w, remote)).cloned()
}

/// Whether the window can't be captured here: not on this PC, no Direct3D device, or Windows
/// refused this window and has given no picture of it since.
pub fn unsupported(hwnd: HWND, remote: bool) -> bool {
    barred(remote) || NO_GPU.load(Ordering::Relaxed) || refused().contains_key(&(hwnd.0 as isize))
}

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

/// One window being captured, with its GPU copies.
struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// The frame pool's size.
    size: SizeInt32,
    /// The client area and its levels down to the deepest asked for (rebuilt when its size changes).
    mips: Option<Mips>,
    levels: HashMap<u32, Level>,
    /// The widths asked for in the last `IDLE`, and when: frames that come unasked are scaled to them too.
    wants: HashMap<u32, Instant>,
    used: Instant,
    started: Instant,
    /// A frame came since the capture started.
    framed: bool,
    /// The frame whose copy the GPU may still be making, kept open until it is read back.
    pending: Option<Direct3D11CaptureFrame>,
}

impl Capture {
    /// The worker should look again `SOON`, unasked.
    fn waiting(&self) -> bool {
        self.pending.is_some() || (!self.framed && self.started.elapsed() < FIRST_WAIT)
    }
}

struct Mips {
    width: u32,
    height: u32,
    count: u32,
    texture: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
}

struct Level {
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
    unread: bool,
}

fn run(requests: &Receiver<Request>) {
    // SAFETY: this thread's own COM apartment, for the capture API; never the UI thread's.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        log::debug!("live previews: CoInitializeEx failed: {e}");
    }
    if !borderless() {
        log::info!("live previews: Windows here draws a yellow border, so only the Remote Dashboard captures (with it)");
        BORDERED.store(true, Ordering::Relaxed);
    }
    let mut gpu: Option<Gpu> = None;
    let mut gpu_warned = false;
    let mut captures: HashMap<isize, Capture> = HashMap::new();
    let mut swept = Instant::now();
    loop {
        // Only which widths each window is asked at matters: a backlog is gone in one go.
        let mut wanted: HashMap<isize, HashSet<u32>> = HashMap::new();
        let wait = if captures.values().any(Capture::waiting) { SOON } else { IDLE };
        match requests.recv_timeout(wait) {
            Ok(r) => {
                // The desktop's requests sent before `BORDERED` was known are dropped here.
                let bordered = BORDERED.load(Ordering::Relaxed);
                for r in std::iter::once(r).chain(requests.try_iter()).filter(|r| r.remote || !bordered) {
                    wanted.entry(r.hwnd).or_default().insert(r.max_w);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // SAFETY: asks our own device one question. Asked apart from frames: a lost device has none.
        if gpu.as_ref().is_some_and(|g| unsafe { g.device.GetDeviceRemovedReason() }.is_err()) {
            log::info!("live previews: the Direct3D device was lost, making a new one");
            gpu = None;
            captures.drain().for_each(|(_, c)| close(&c));
        }
        if !wanted.is_empty() && gpu.is_none() {
            gpu = new_gpu()
                .map_err(|e| {
                    NO_GPU.store(true, Ordering::Relaxed);
                    if gpu_warned {
                        log::debug!("live previews: no Direct3D device: {e}");
                    } else {
                        gpu_warned = true;
                        log::warn!("live previews: no Direct3D device: {e}");
                    }
                })
                .ok();
            NO_GPU.store(gpu.is_none(), Ordering::Relaxed);
        }
        if let Some(g) = &gpu {
            for (hwnd, widths) in wanted {
                serve(g, &mut captures, hwnd, widths);
            }
            for (&hwnd, c) in captures.iter_mut().filter(|(_, c)| c.waiting()) {
                step(g, hwnd, c);
            }
        }
        if swept.elapsed() >= IDLE {
            swept = Instant::now();
            captures.retain(|_, c| {
                let keep = c.used.elapsed() < IDLE;
                if !keep {
                    close(c);
                }
                keep
            });
            // SAFETY: asks whether a handle still names a window.
            let exists = |h: &isize| unsafe { IsWindow(Some(HWND(*h as _))) }.as_bool();
            shots().retain(|h, _| exists(h));
            refused().retain(|h, _| exists(h));
        }
    }
}

fn close(c: &Capture) {
    if let Some(f) = &c.pending {
        let _ = f.Close();
    }
    let _ = c.session.Close();
    let _ = c.pool.Close();
}

/// Whether Windows lets this app capture without the yellow border (asked once, and needed for
/// `SetIsBorderRequired` to work).
fn borderless() -> bool {
    GraphicsCaptureSession::IsSupported()
        .inspect_err(|e| log::warn!("live previews: capture support unknown: {e}"))
        .unwrap_or(false)
        && GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
            .and_then(|op| op.join())
            .inspect_err(|e| log::warn!("live previews: borderless access not granted: {e}"))
            .is_ok_and(|s| s == AppCapabilityAccessStatus::Allowed)
}

fn new_gpu() -> Result<Gpu> {
    let (mut device, mut context) = (None, None);
    // SAFETY: plain device creation; both out-pointers are locals.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )?;
    }
    let device: ID3D11Device = device.ok_or_else(windows::core::Error::empty)?;
    let context = context.ok_or_else(windows::core::Error::empty)?;
    // SAFETY: wraps our own device for the WinRT capture API.
    let winrt = unsafe { CreateDirect3D11DeviceFromDXGIDevice(&device.cast::<IDXGIDevice>()?)? }.cast()?;
    Ok(Gpu { device, context, winrt })
}

/// Starts capturing the window if need be, then `step`s it.
fn serve(gpu: &Gpu, captures: &mut HashMap<isize, Capture>, hwnd: isize, widths: HashSet<u32>) {
    if let std::collections::hash_map::Entry::Vacant(slot) = captures.entry(hwnd) {
        if refused().get(&hwnd).is_some_and(|at| at.elapsed() < RETRY) {
            return;
        }
        match start(gpu, HWND(hwnd as _)) {
            Ok(c) => {
                slot.insert(c);
            }
            Err(e) => {
                // SAFETY: asks whether a handle still names a window. One that closed says nothing about capturing.
                if unsafe { IsWindow(Some(HWND(hwnd as _))) }.as_bool() {
                    refused().insert(hwnd, Instant::now());
                }
                log::debug!("live preview: can't capture window {hwnd:#x}: {e}");
                return;
            }
        }
    }
    let Some(c) = captures.get_mut(&hwnd) else { return };
    c.used = Instant::now();
    c.wants.extend(widths.into_iter().map(|w| (w, c.used)));
    step(gpu, hwnd, c);
}

/// Reads back the copy the GPU has finished, if any; else, with none on its way, starts the copy
/// of the window's newest frame, if one arrived. Never waits for the GPU.
fn step(gpu: &Gpu, hwnd: isize, c: &mut Capture) {
    if c.pending.is_some() {
        read_back(gpu, hwnd, c);
        if c.pending.is_some() {
            return;
        }
    }
    // Only the newest frame matters; older ones go back to the pool.
    let mut newest: Option<Direct3D11CaptureFrame> = None;
    while let Ok(frame) = c.pool.TryGetNextFrame() {
        if let Some(old) = newest.replace(frame) {
            let _ = old.Close();
        }
    }
    let Some(frame) = newest else { return };
    c.framed = true;
    match copy(gpu, HWND(hwnd as _), c, &frame) {
        Ok(true) => {
            c.pending = Some(frame);
            // An idle GPU is done at once.
            read_back(gpu, hwnd, c);
        }
        Ok(false) => {
            let _ = frame.Close();
        }
        Err(e) => {
            log::debug!("live preview: frame of window {hwnd:#x} not copied: {e}");
            let _ = frame.Close();
        }
    }
}

/// Each level's copy of the pending frame as a picture once the GPU has made it; the frame stays
/// pending while any isn't.
fn read_back(gpu: &Gpu, hwnd: isize, c: &mut Capture) {
    let source_w = c.mips.as_ref().map_or(0, |m| m.width);
    for (&level, s) in c.levels.iter_mut().filter(|(_, s)| s.unread) {
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: maps our own staging texture for reading without waiting; it is unmapped below.
        let read = unsafe { gpu.context.Map(&s.staging, 0, D3D11_MAP_READ, D3D11_MAP_FLAG_DO_NOT_WAIT.0 as u32, Some(&mut mapped)) };
        match read {
            Err(e) if e.code() == DXGI_ERROR_WAS_STILL_DRAWING => continue,
            Err(e) => log::debug!("live preview: frame of window {hwnd:#x} not read: {e}"),
            Ok(()) => {
                let (w, h, pitch) = (s.width as usize, s.height as usize, mapped.RowPitch as usize);
                // SAFETY: a mapped texture of `h` (at least 1) rows, each `pitch` bytes apart and `w * 4` long.
                let data = unsafe { std::slice::from_raw_parts(mapped.pData as *const u8, pitch * (h - 1) + w * 4) };
                let rgba = rgba_of(data, pitch, w, h);
                // SAFETY: mapped above; `data` isn't used after this.
                unsafe { gpu.context.Unmap(&s.staging, 0) };
                let shot = Shot { thumb: Arc::new(Thumb { width: s.width, height: s.height, rgba }), seq: NEXT_SEQ.fetch_add(1, Ordering::Relaxed), at: Instant::now() };
                shots().entry(hwnd).or_default().put(source_w, level, shot);
                refused().remove(&hwnd);
            }
        }
        s.unread = false;
    }
    if c.levels.values().all(|s| !s.unread)
        && let Some(f) = c.pending.take()
    {
        let _ = f.Close();
    }
}

/// `h` rows of BGRX pixels, `pitch` bytes apart, as opaque RGBA rows top-down (pure).
fn rgba_of(data: &[u8], pitch: usize, w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for p in data[y * pitch..y * pitch + w * 4].as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    out
}

fn start(gpu: &Gpu, hwnd: HWND) -> Result<Capture> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: a capture item for a window handle; fails for a window that is gone.
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
    let size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt, FORMAT, 2, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    if let Err(e) = session.SetIsBorderRequired(false) {
        log::debug!("live preview: window {:#x} keeps the yellow border: {e}", hwnd.0 as isize);
    }
    let _ = session.SetIsCursorCaptureEnabled(false);
    session.StartCapture()?;
    let now = Instant::now();
    Ok(Capture { pool, session, size, mips: None, levels: HashMap::new(), wants: HashMap::new(), used: now, started: now, framed: false, pending: None })
}

/// Starts the GPU's copies of the frame's client area, one per level the asked widths need, for
/// `read_back`; false for an empty window, or the first frame after a resize.
fn copy(gpu: &Gpu, hwnd: HWND, c: &mut Capture, frame: &Direct3D11CaptureFrame) -> Result<bool> {
    let content = frame.ContentSize()?;
    if content.Width != c.size.Width || content.Height != c.size.Height {
        // The window was resized: later frames come at its new size.
        c.pool.Recreate(&gpu.winrt, FORMAT, 2, content)?;
        c.size = content;
        return Ok(false);
    }
    if content.Width <= 0 || content.Height <= 0 {
        return Ok(false);
    }
    let area = super::window::client_in_frame(hwnd).map_or_else(|| whole(content), |(client, visible)| client_box(visible, client, content));
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    c.wants.retain(|_, at| at.elapsed() < IDLE);
    let wanted: HashSet<u32> = c.wants.keys().map(|&max_w| level_for(w, max_w)).collect();
    let Some(&deepest) = wanted.iter().max() else { return Ok(false) };
    c.levels.retain(|level, _| wanted.contains(level));
    if !c.mips.as_ref().is_some_and(|m| m.width == w && m.height == h && m.count > deepest) {
        c.mips = Some(mips(gpu, w, h, deepest + 1)?);
    }
    for &level in &wanted {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        if !c.levels.get(&level).is_some_and(|s| s.width == lw && s.height == lh) {
            c.levels.insert(level, Level { staging: staging(gpu, lw, lh)?, width: lw, height: lh, unread: false });
        }
    }
    let m = c.mips.as_ref().expect("made above");
    let access = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
    // SAFETY: the frame's own Direct3D texture, alive while `frame` is (kept open until read back).
    let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
    // SAFETY: copies between textures of this device sized as made in `mips` and `staging`; `area` lies inside the frame.
    unsafe {
        gpu.context.CopySubresourceRegion(&m.texture, 0, 0, 0, 0, &source, 0, Some(&area));
        gpu.context.GenerateMips(&m.view);
        for (&level, s) in &mut c.levels {
            gpu.context.CopySubresourceRegion(&s.staging, 0, 0, 0, 0, &m.texture, level, None);
            s.unread = true;
        }
        // Sent now: else the GPU starts on it only once something waits for it.
        gpu.context.Flush();
    }
    Ok(true)
}

fn whole(content: SizeInt32) -> D3D11_BOX {
    D3D11_BOX { left: 0, top: 0, front: 0, right: content.Width.max(0) as u32, bottom: content.Height.max(0) as u32, back: 1 }
}

fn client_box(frame: RECT, client: RECT, content: SizeInt32) -> D3D11_BOX {
    let (cw, ch) = (content.Width.max(0), content.Height.max(0));
    let (left, top) = ((client.left - frame.left).clamp(0, cw), (client.top - frame.top).clamp(0, ch));
    let (right, bottom) = ((client.right - frame.left).clamp(left, cw), (client.bottom - frame.top).clamp(top, ch));
    if right == left || bottom == top {
        return whole(content);
    }
    D3D11_BOX { left: left as u32, top: top as u32, front: 0, right: right as u32, bottom: bottom as u32, back: 1 }
}

/// The mip level whose width is at most `max_w` (pure): halving `w` that many times.
fn level_for(w: u32, max_w: u32) -> u32 {
    let mut level = 0;
    while (w >> level) > max_w.max(1) && (w >> (level + 1)) > 0 {
        level += 1;
    }
    level
}

fn texture(w: u32, h: u32, mips: u32, usage: D3D11_USAGE, bind: u32, cpu: u32, misc: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: mips,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: misc,
    }
}

fn mips(gpu: &Gpu, width: u32, height: u32, count: u32) -> Result<Mips> {
    let desc = texture(width, height, count, D3D11_USAGE_DEFAULT, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, 0, D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32);
    let (mut texture, mut view) = (None, None);
    // SAFETY: plain resource creation on our device into locals.
    unsafe {
        gpu.device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        let made = texture.as_ref().ok_or_else(windows::core::Error::empty)?;
        gpu.device.CreateShaderResourceView(made, None, Some(&mut view))?;
    }
    let none = windows::core::Error::empty;
    Ok(Mips { width, height, count, texture: texture.ok_or_else(none)?, view: view.ok_or_else(none)? })
}

fn staging(gpu: &Gpu, width: u32, height: u32) -> Result<ID3D11Texture2D> {
    let desc = texture(width, height, 1, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32, 0);
    let mut staging = None;
    // SAFETY: plain resource creation on our device into a local.
    unsafe { gpu.device.CreateTexture2D(&desc, None, Some(&mut staging))? };
    staging.ok_or_else(windows::core::Error::empty)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Instant;

    use windows::Graphics::SizeInt32;
    use windows::Win32::Foundation::RECT;

    use super::{Pictures, Shot, Thumb, client_box, last, level_for, request, rgba_of};

    #[test]
    fn rows_turn_opaque_rgba_without_their_padding() {
        // Two rows of one pixel, 8 bytes apart.
        assert_eq!(rgba_of(&[1, 2, 3, 0, 9, 9, 9, 9, 4, 5, 6, 7], 8, 1, 2), [3, 2, 1, 255, 6, 5, 4, 255]);
    }

    /// Captures the taskbar for real (needs a desktop session): `cargo test -- --ignored captures`.
    #[test]
    #[ignore]
    fn captures_a_real_window() {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        use windows::core::w;
        // SAFETY: looks up a window by class name.
        // `ROALT_TEST_HWND` picks another window (Windows 10 refuses to capture the taskbar).
        let hwnd = match std::env::var("ROALT_TEST_HWND") {
            Ok(h) => windows::Win32::Foundation::HWND(h.parse::<isize>().expect("a window handle") as _),
            Err(_) => unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.expect("a taskbar"),
        };
        // The first request starts the capture; frames come once it runs.
        let mut shot = None;
        for _ in 0..20 {
            request(hwnd, 480, true);
            shot = last(hwnd, Some(480), true);
            if shot.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let t = shot
            .unwrap_or_else(|| {
                panic!(
                    "no picture of the taskbar: bordered={} unavailable={} no_gpu={} refused={}",
                    super::BORDERED.load(std::sync::atomic::Ordering::Relaxed),
                    super::UNAVAILABLE.load(std::sync::atomic::Ordering::Relaxed),
                    super::NO_GPU.load(std::sync::atomic::Ordering::Relaxed),
                    super::refused().contains_key(&(hwnd.0 as isize)),
                )
            })
            .thumb;
        assert!(t.width <= 480 && t.width > 0 && t.height > 0);
        assert_eq!(t.rgba.len(), (t.width * t.height * 4) as usize);
        assert!(t.rgba.chunks(4).any(|p| p[..3] != t.rgba[..3]), "not one flat colour");
    }

    /// Brightness of the window in `ROALT_TEST_HWND` over 6 s of pictures, 15 a second: median,
    /// max and how many were far brighter than usual (white flashes).
    #[test]
    #[ignore]
    fn measures_flashes() {
        let Ok(h) = std::env::var("ROALT_TEST_HWND") else { return };
        let hwnd = windows::Win32::Foundation::HWND(h.parse::<isize>().unwrap() as _);
        let mut means = Vec::new();
        let mut pictures = std::collections::HashSet::new();
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(6) {
            request(hwnd, 480, true);
            if let Some(t) = last(hwnd, Some(480), true).map(|s| s.thumb) {
                pictures.insert(t.rgba.clone());
                let sum: u64 = t.rgba.chunks(4).map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum();
                means.push(sum as f64 / 3.0 / (t.rgba.len() / 4) as f64);
            }
            std::thread::sleep(std::time::Duration::from_millis(66));
        }
        means.sort_by(f64::total_cmp);
        let median = means[means.len() / 2];
        let flashes = means.iter().filter(|m| **m > median + 60.0).count();
        println!("FLASHES samples={} different={} median={median:.0} max={:.0} flashes={flashes}", means.len(), pictures.len(), means.last().unwrap());
    }

    fn shot(width: u32, seq: u32) -> Shot {
        Shot { thumb: Arc::new(Thumb { width, height: 1, rgba: Vec::new() }), seq, at: Instant::now() }
    }

    #[test]
    fn each_size_keeps_its_own_picture_and_none_is_too_wide() {
        let mut p = Pictures::default();
        p.put(1920, 2, shot(480, 1));
        p.put(1920, 3, shot(240, 2));
        assert_eq!(p.pick(Some(480), true).map(|s| s.seq), Some(1));
        assert_eq!(p.pick(Some(320), true).map(|s| s.seq), Some(2), "level 3, 240 wide");
        assert_eq!(p.pick(Some(640), true).map(|s| s.seq), Some(1), "level 2's own picture");
        assert_eq!(p.pick(Some(200), true).map(|s| s.seq), None, "nothing narrow enough for a phone");
        assert_eq!(p.pick(Some(200), false).map(|s| s.seq), Some(2), "the desktop takes the newest");
        assert_eq!(p.pick(None, true).map(|s| s.seq), Some(2));
        p.put(1280, 1, shot(640, 3));
        assert_eq!(p.pick(Some(480), true).map(|s| s.seq), None, "the old size's pictures are gone");
    }

    #[test]
    fn the_client_area_is_cut_out_of_the_frame() {
        let size = SizeInt32 { Width: 816, Height: 639 };
        let frame = RECT { left: 100, top: 50, right: 916, bottom: 689 };
        let client = RECT { left: 108, top: 81, right: 908, bottom: 681 };
        let b = client_box(frame, client, size);
        assert_eq!((b.left, b.top, b.right, b.bottom), (8, 31, 808, 631));
        let off = client_box(frame, RECT { left: 5000, top: 5000, right: 5100, bottom: 5100 }, size);
        assert_eq!((off.left, off.top, off.right, off.bottom), (0, 0, 816, 639), "the whole frame");
    }

    #[test]
    fn halves_until_it_fits() {
        assert_eq!(level_for(400, 480), 0);
        assert_eq!(level_for(1020, 480), 2, "1020 → 510 → 255");
        assert_eq!(level_for(1920, 480), 2, "exactly 480");
        assert_eq!(level_for(3840, 160), 5);
        assert_eq!(level_for(1, 0), 0);
    }
}
