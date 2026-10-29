//! Live pictures of client windows through Windows.Graphics.Capture, which copies what the
//! compositor already shows and never touches the client's own frames. (`PrintWindow` with
//! `PW_RENDERFULLCONTENT`, used before, made Roblox windows flash white while the dashboard
//! drew them.)
//!
//! One worker thread owns a Direct3D device and a capture per window the dashboard asks for; a
//! capture nobody asked for in `IDLE` is closed. Each picture is scaled down on the GPU (mipmaps)
//! and only that small copy is read back. Borderless only: where Windows can't capture without its
//! yellow frame around the game (before Windows 11 / Windows 10 21H2 server), there is no picture.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use windows::Graphics::Capture::{Direct3D11CaptureFrame, Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows::Win32::Foundation::{HMODULE, HWND};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_MAP_READ,
    D3D11_MAPPED_SUBRESOURCE, D3D11_RESOURCE_MISC_GENERATE_MIPS, D3D11_SDK_VERSION, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT, D3D11_USAGE_STAGING,
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11ShaderResourceView, ID3D11Texture2D,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{Interface, Result, factory};

use super::window::{Thumb, to_rgba};

/// A capture nobody asked a picture of for this long is closed.
const IDLE: Duration = Duration::from_secs(5);
/// How long a caller waits for the worker.
const REPLY: Duration = Duration::from_secs(2);
const FORMAT: DirectXPixelFormat = DirectXPixelFormat::B8G8R8A8UIntNormalized;

struct Request {
    hwnd: isize,
    max_w: u32,
    reply: Sender<Option<Thumb>>,
}

/// The window's picture, at most `max_w` pixels wide (a power-of-two step down from its size);
/// None when it can't be captured. The window must not be minimized (nothing is drawn then).
pub fn picture(hwnd: HWND, max_w: u32) -> Option<Thumb> {
    static WORKER: OnceLock<Sender<Request>> = OnceLock::new();
    let worker = WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        if let Err(e) = std::thread::Builder::new().name("capture".into()).spawn(move || run(&rx)) {
            log::warn!("live previews off: {e}");
        }
        tx
    });
    let (reply, answer) = mpsc::channel();
    if worker.send(Request { hwnd: hwnd.0 as isize, max_w, reply }).is_err() {
        static DEAD: std::sync::Once = std::sync::Once::new();
        DEAD.call_once(|| log::warn!("live previews off: the capture worker is not running"));
        return None;
    }
    answer
        .recv_timeout(REPLY)
        .inspect_err(|e| {
            // At most one warning a minute: a stuck worker would otherwise warn on every request.
            static LAST: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);
            let mut last = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if last.is_none_or(|t| t.elapsed() >= Duration::from_secs(60)) {
                *last = Some(Instant::now());
                log::warn!("live preview: no answer from the capture worker: {e}");
            }
        })
        .ok()
        .flatten()
}

struct Gpu {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    winrt: IDirect3DDevice,
}

/// One window being captured, with its GPU copies and its last picture.
struct Capture {
    pool: Direct3D11CaptureFramePool,
    session: GraphicsCaptureSession,
    /// The frame pool's size.
    size: SizeInt32,
    /// The picture's size and the textures made for it (rebuilt when the window's size changes).
    scaled: Option<Scaled>,
    last: Option<Thumb>,
    used: Instant,
}

struct Scaled {
    size: SizeInt32,
    mips: ID3D11Texture2D,
    view: ID3D11ShaderResourceView,
    level: u32,
    staging: ID3D11Texture2D,
    width: u32,
    height: u32,
}

fn run(requests: &Receiver<Request>) {
    // SAFETY: this thread's own COM apartment, for the capture API; never the UI thread's.
    if let Err(e) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
        log::debug!("live previews: CoInitializeEx failed: {e}");
    }
    let borderless = borderless();
    if !borderless {
        log::info!("live previews off: Windows can't capture client windows without a yellow border here");
    }
    let mut gpu: Option<Gpu> = None;
    let mut gpu_warned = false;
    let mut captures: HashMap<isize, Capture> = HashMap::new();
    loop {
        match requests.recv_timeout(IDLE) {
            Ok(r) => {
                let picture = if borderless {
                    if gpu.is_none() {
                        gpu = new_gpu()
                            .map_err(|e| {
                                if gpu_warned {
                                    log::debug!("live previews: no Direct3D device: {e}");
                                } else {
                                    gpu_warned = true;
                                    log::warn!("live previews: no Direct3D device: {e}");
                                }
                            })
                            .ok();
                    }
                    gpu.as_ref().and_then(|g| serve(g, &mut captures, r.hwnd, r.max_w))
                } else {
                    None
                };
                let _ = r.reply.send(picture);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        captures.retain(|_, c| {
            let keep = c.used.elapsed() < IDLE;
            if !keep {
                let _ = c.session.Close();
                let _ = c.pool.Close();
            }
            keep
        });
    }
}

/// Whether Windows lets this app capture without the yellow border (asked once).
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

/// The window's latest picture: a new frame if one arrived, else the last one.
fn serve(gpu: &Gpu, captures: &mut HashMap<isize, Capture>, hwnd: isize, max_w: u32) -> Option<Thumb> {
    if !captures.contains_key(&hwnd) {
        match start(gpu, HWND(hwnd as _)) {
            Ok(c) => {
                captures.insert(hwnd, c);
            }
            Err(e) => {
                log::debug!("live preview: can't capture window {hwnd:#x}: {e}");
                return None;
            }
        }
    }
    let c = captures.get_mut(&hwnd)?;
    c.used = Instant::now();
    // Only the newest frame matters; older ones go back to the pool.
    let mut newest: Option<Direct3D11CaptureFrame> = None;
    while let Ok(frame) = c.pool.TryGetNextFrame() {
        if let Some(old) = newest.replace(frame) {
            let _ = old.Close();
        }
    }
    if let Some(frame) = newest {
        match scale(gpu, c, &frame, max_w) {
            Ok(t) => c.last = t,
            Err(e) => log::debug!("live preview: frame of window {hwnd:#x} not read: {e}"),
        }
        let _ = frame.Close();
    }
    c.last.clone()
}

fn start(gpu: &Gpu, hwnd: HWND) -> Result<Capture> {
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    // SAFETY: a capture item for a window handle; fails for a window that is gone.
    let item: GraphicsCaptureItem = unsafe { interop.CreateForWindow(hwnd)? };
    let size = item.Size()?;
    let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(&gpu.winrt, FORMAT, 2, size)?;
    let session = pool.CreateCaptureSession(&item)?;
    session.SetIsBorderRequired(false)?;
    let _ = session.SetIsCursorCaptureEnabled(false);
    session.StartCapture()?;
    Ok(Capture { pool, session, size, scaled: None, last: None, used: Instant::now() })
}

/// The frame scaled down to at most `max_w` wide; None for an empty window.
fn scale(gpu: &Gpu, c: &mut Capture, frame: &Direct3D11CaptureFrame, max_w: u32) -> Result<Option<Thumb>> {
    let content = frame.ContentSize()?;
    if content.Width != c.size.Width || content.Height != c.size.Height {
        // The window was resized: later frames come at its new size; this one keeps the last picture.
        c.pool.Recreate(&gpu.winrt, FORMAT, 2, content)?;
        c.size = content;
        return Ok(c.last.clone());
    }
    if content.Width <= 0 || content.Height <= 0 {
        return Ok(None);
    }
    let fits = |s: &Scaled| s.size.Width == content.Width && s.size.Height == content.Height && s.level == level_for(content.Width as u32, max_w);
    if !c.scaled.as_ref().is_some_and(fits) {
        c.scaled = Some(scaled(gpu, content, max_w)?);
    }
    let s = c.scaled.as_ref().expect("made above");
    let access = frame.Surface()?.cast::<IDirect3DDxgiInterfaceAccess>()?;
    // SAFETY: the frame's own Direct3D texture, alive while `frame` is.
    let source: ID3D11Texture2D = unsafe { access.GetInterface()? };
    let content_box = D3D11_BOX { left: 0, top: 0, front: 0, right: content.Width as u32, bottom: content.Height as u32, back: 1 };
    let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
    // SAFETY: copies between textures of this device sized as made in `scaled`, then reads the
    // mapped staging texture row by row within its pitch before unmapping it.
    unsafe {
        gpu.context.CopySubresourceRegion(&s.mips, 0, 0, 0, 0, &source, 0, Some(&content_box));
        gpu.context.GenerateMips(&s.view);
        gpu.context.CopySubresourceRegion(&s.staging, 0, 0, 0, 0, &s.mips, s.level, None);
        gpu.context.Map(&s.staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        let row = (s.width * 4) as usize;
        let mut bgra = Vec::with_capacity(row * s.height as usize);
        for y in 0..s.height as usize {
            bgra.extend_from_slice(std::slice::from_raw_parts((mapped.pData as *const u8).add(y * mapped.RowPitch as usize), row));
        }
        gpu.context.Unmap(&s.staging, 0);
        Ok(Some(Thumb { width: s.width, height: s.height, rgba: to_rgba(&bgra) }))
    }
}

/// The mip level whose width is at most `max_w` (pure): halving `w` that many times.
fn level_for(w: u32, max_w: u32) -> u32 {
    let mut level = 0;
    while (w >> level) > max_w.max(1) && (w >> (level + 1)) > 0 {
        level += 1;
    }
    level
}

fn scaled(gpu: &Gpu, size: SizeInt32, max_w: u32) -> Result<Scaled> {
    let (w, h) = (size.Width as u32, size.Height as u32);
    let level = level_for(w, max_w);
    let (width, height) = ((w >> level).max(1), (h >> level).max(1));
    let texture = |w, h, mips, usage, bind: u32, cpu: u32, misc: u32| D3D11_TEXTURE2D_DESC {
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
    };
    // Every level down to the one read back.
    let mips_desc = texture(w, h, level + 1, D3D11_USAGE_DEFAULT, (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32, 0, D3D11_RESOURCE_MISC_GENERATE_MIPS.0 as u32);
    let staging_desc = texture(width, height, 1, D3D11_USAGE_STAGING, 0, D3D11_CPU_ACCESS_READ.0 as u32, 0);
    let (mut mips, mut view, mut staging) = (None, None, None);
    // SAFETY: plain resource creation on our device into locals.
    unsafe {
        gpu.device.CreateTexture2D(&mips_desc, None, Some(&mut mips))?;
        let mips_ref = mips.as_ref().ok_or_else(windows::core::Error::empty)?;
        gpu.device.CreateShaderResourceView(mips_ref, None, Some(&mut view))?;
        gpu.device.CreateTexture2D(&staging_desc, None, Some(&mut staging))?;
    }
    let none = windows::core::Error::empty;
    Ok(Scaled { size, mips: mips.ok_or_else(none)?, view: view.ok_or_else(none)?, level, staging: staging.ok_or_else(none)?, width, height })
}

#[cfg(test)]
mod tests {
    use super::{level_for, picture};

    /// Captures the taskbar for real (needs a desktop session): `cargo test -- --ignored captures`.
    #[test]
    #[ignore]
    fn captures_a_real_window() {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        use windows::core::w;
        // SAFETY: looks up a window by class name.
        let hwnd = unsafe { FindWindowW(w!("Shell_TrayWnd"), None) }.expect("a taskbar");
        // The first request starts the capture; frames come once it runs.
        let mut thumb = None;
        for _ in 0..20 {
            thumb = picture(hwnd, 480);
            if thumb.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let t = thumb.expect("a picture of the taskbar");
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
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(6) {
            if let Some(t) = picture(hwnd, 480) {
                let sum: u64 = t.rgba.chunks(4).map(|p| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2])).sum();
                means.push(sum as f64 / 3.0 / (t.rgba.len() / 4) as f64);
            }
            std::thread::sleep(std::time::Duration::from_millis(66));
        }
        means.sort_by(f64::total_cmp);
        let median = means[means.len() / 2];
        let flashes = means.iter().filter(|m| **m > median + 60.0).count();
        println!("FLASHES samples={} median={median:.0} max={:.0} flashes={flashes}", means.len(), means.last().unwrap());
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
