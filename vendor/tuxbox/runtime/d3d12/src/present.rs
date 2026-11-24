//! Present one RGBA8, BGRA8 (either sRGB or not), RGB10A2 or RGBA16F guest
//! texture through a triple-buffered DXGI swap chain.
//!
//! The chain is a composition swap chain shown through one DirectComposition
//! visual that is scaled and centred to keep the image's aspect in whatever
//! client area the window has (borderless fullscreen leaves black bars from
//! the window's class brush). Opt-in with TUXBOX_LETTERBOX=1 (the borderless
//! default showed no picture through it); off, or on a failure to build it,
//! the HWND swap chain is stretched to the client area.

mod feed;
mod overlay;
mod window;
use window::OutputWindow;

use std::mem::{ManuallyDrop, swap};
use std::sync::Mutex;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, HANDLE, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
    DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB, DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_FORMAT_R16G16B16A16_FLOAT, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice2, IDCompositionDevice, IDCompositionScaleTransform, IDCompositionTarget,
    IDCompositionVisual,
};
use windows::core::{HRESULT, Interface, Result};

/// A DirectComposition visual holding the swap chain, scaled and centred in
/// the window's client area.
struct Letterbox {
    device: IDCompositionDevice,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    scale: IDCompositionScaleTransform,
    /// (client width, client height, image width, image height) last laid out.
    laid_out: (i32, i32, u32, u32),
}

fn present_minimised() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_PRESENT_MINIMISED").map_or(true, |v| v.trim() != "0"))
}

fn occluded_status() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_OCCLUDED_STATUS").map_or(true, |v| v.trim() != "0"))
}

/// A lit source sample asks for a window sample at the next present.
static LIT_WINDOW_DUE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn window_reports() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_WINDOW_REPORTS").map_or(true, |v| v.trim() != "0"))
}

fn letterbox_enabled() -> bool {
    std::env::var("TUXBOX_LETTERBOX").is_ok_and(|v| v.trim() == "1")
}

impl Letterbox {
    /// SAFETY: `hwnd` is a live window owned by this process.
    unsafe fn new(hwnd: windows::Win32::Foundation::HWND, chain: &IDXGISwapChain1) -> Result<Self> {
        unsafe {
            let device: IDCompositionDevice = DCompositionCreateDevice2(None::<&windows::core::IUnknown>)?;
            let target = device.CreateTargetForHwnd(hwnd, true)?;
            let visual = device.CreateVisual()?;
            let scale = device.CreateScaleTransform()?;
            visual.SetContent(chain)?;
            visual.SetTransform(&scale)?;
            target.SetRoot(&visual)?;
            Ok(Self { device, _target: target, visual, scale, laid_out: (0, 0, 0, 0) })
        }
    }

    /// Fit a `width` x `height` image to the client area, keeping its aspect;
    /// committed only when the client or image size changed.
    unsafe fn layout(&mut self, hwnd: windows::Win32::Foundation::HWND, width: u32, height: u32) -> Result<()> {
        let mut client = windows::Win32::Foundation::RECT::default();
        unsafe { windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut client)? };
        let (cw, ch) = (client.right - client.left, client.bottom - client.top);
        if cw <= 0 || ch <= 0 || width == 0 || height == 0 || self.laid_out == (cw, ch, width, height) {
            return Ok(());
        }
        let factor = (cw as f32 / width as f32).min(ch as f32 / height as f32);
        let (shown_w, shown_h) = (width as f32 * factor, height as f32 * factor);
        // The offset applies after the transform: centre the scaled image.
        unsafe {
            self.scale.SetScaleX2(factor)?;
            self.scale.SetScaleY2(factor)?;
            self.visual.SetOffsetX2(((cw as f32 - shown_w) / 2.0).round())?;
            self.visual.SetOffsetY2(((ch as f32 - shown_h) / 2.0).round())?;
            self.device.Commit()?;
        }
        if self.laid_out.0 == 0 {
            crate::diagnostics::log_line(format_args!(
                "presentation letterboxed {width}x{height} into {cw}x{ch} at {:.0}x{:.0} (TUXBOX_LETTERBOX=1)",
                shown_w, shown_h
            ));
        }
        self.laid_out = (cw, ch, width, height);
        Ok(())
    }
}

/// The flip-model back-buffer format a plane copies into. Flip swap chains take
/// no sRGB formats; CopyResource between UNORM and UNORM_SRGB of one family is legal.
fn swap_format(format: DXGI_FORMAT) -> Option<DXGI_FORMAT> {
    Some(match format {
        DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_B8G8R8A8_UNORM | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => DXGI_FORMAT_B8G8R8A8_UNORM,
        DXGI_FORMAT_R10G10B10A2_UNORM => DXGI_FORMAT_R10G10B10A2_UNORM,
        // Xbox2Windows 99624a8d: typeless 8-bit and 10:10:10:2 surfaces have
        // one reading and copy into the UNORM back buffer of their family.
        // TUXBOX_PRESENT_TYPELESS=0 refuses them again.
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_TYPELESS if typeless() => DXGI_FORMAT_R8G8B8A8_UNORM,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_TYPELESS if typeless() => DXGI_FORMAT_B8G8R8A8_UNORM,
        windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R10G10B10A2_TYPELESS if typeless() => DXGI_FORMAT_R10G10B10A2_UNORM,
        DXGI_FORMAT_R16G16B16A16_FLOAT => DXGI_FORMAT_R16G16B16A16_FLOAT,
        _ => return None,
    })
}

fn typeless() -> bool {
    crate::env_toggle!("TUXBOX_PRESENT_TYPELESS") != Some("0")
}

#[derive(Default)]
pub(crate) struct Presentation {
    session: Mutex<Option<SwapChain>>,
}

impl Presentation {
    pub(crate) fn present(&self, queue: &ID3D12CommandQueue, source: &ID3D12Resource) -> HRESULT {
        self.present_texture(queue, source)
            .unwrap_or_else(|error| error.code())
    }

    fn present_texture(
        &self,
        queue: &ID3D12CommandQueue,
        source: &ID3D12Resource,
    ) -> Result<HRESULT> {
        // SAFETY: source is a live native resource held by the caller.
        let mut desc = unsafe { source.GetDesc() };
        let chain_format = swap_format(desc.Format);
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
            || chain_format.is_none()
            || desc.DepthOrArraySize != 1
            || desc.MipLevels != 1
            || desc.SampleDesc.Count != 1
            || desc.SampleDesc.Quality != 0
            || desc.Width > u64::from(u32::MAX)
        {
            static REFUSED: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&REFUSED, || format!(
                "present refused: {:?} {}x{} format={} arrays={} mips={} samples={}",
                desc.Dimension, desc.Width, desc.Height, desc.Format.0, desc.DepthOrArraySize, desc.MipLevels, desc.SampleDesc.Count));
            return Err(E_NOTIMPL.into());
        }
        // The swap chain only needs the plane's size and back-buffer format.
        desc.Format = chain_format.unwrap_or(desc.Format);
        let owner = device_of(source)?;
        let device = device_of(queue)?;
        if owner.as_raw() != device.as_raw() {
            return Err(E_INVALIDARG.into());
        }

        let mut session = self.session.lock().map_err(|_| E_FAIL)?;
        let chain = match session.as_mut() {
            Some(chain) => chain,
            None => session.insert(SwapChain::new(queue, &device, &desc)?),
        };
        let status = chain.present(queue, source, &desc)?;
        if status == S_OK {
            overlay::frame();
            crate::diagnostics::crash::presented();
            crate::diagnostics::presented(desc.Width, desc.Height);
        }
        Ok(status)
    }
}

fn device_of(child: &ID3D12DeviceChild) -> Result<ID3D12Device> {
    let mut device = None;
    // SAFETY: COM writes an owned device reference into the empty output slot.
    unsafe { child.GetDevice(&mut device) }.map_err(|_| E_INVALIDARG)?;
    device.ok_or_else(|| E_INVALIDARG.into())
}

struct Frame {
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    // ExecuteCommandLists does not retain resource references until GPU completion.
    source: Option<ID3D12Resource>,
    retire: u64,
}

impl Frame {
    fn new(device: &ID3D12Device) -> Result<Self> {
        // SAFETY: the fresh list and its allocator have the same device and type.
        unsafe {
            let allocator = device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?;
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)?;
            crate::dred::name(&list, "tuxbox present");
            list.Close()?;
            Ok(Self {
                allocator,
                list,
                source: None,
                retire: 0,
            })
        }
    }
}

struct SwapChain {
    chain: IDXGISwapChain3,
    fence: ID3D12Fence,
    frames: [Frame; 3],
    window: OutputWindow,
    letterbox: Option<Letterbox>,
    width: u32,
    height: u32,
    format: DXGI_FORMAT,
    next: u64,
    // Once execution has begun, a failed signal leaves the work without a
    // retirement point. Keep its queue for a final synchronization attempt.
    failed_signal: Option<(HRESULT, ID3D12CommandQueue)>,
}

impl SwapChain {
    fn new(
        queue: &ID3D12CommandQueue,
        device: &ID3D12Device,
        desc: &D3D12_RESOURCE_DESC,
    ) -> Result<Self> {
        let window = OutputWindow::acquire(desc.Width as u32, desc.Height)?;
        overlay::attach(window.hwnd());
        // SAFETY: DXGI copies the descriptor and retains the queue. The output
        // outlives the swap chain and its submitted frames.
        unsafe {
            let params = DXGI_SWAP_CHAIN_DESC1 {
                Width: desc.Width as u32,
                Height: desc.Height,
                Format: desc.Format,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                // Three buffers: the CPU's next present does not wait on the
                // previous frame's copy and flip (xcl #674).
                BufferCount: 3,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Scaling: DXGI_SCALING_STRETCH,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let composed = letterbox_enabled()
                .then(|| -> Result<(IDXGISwapChain1, Letterbox)> {
                    let chain = factory.CreateSwapChainForComposition(queue, &params, None)?;
                    let letterbox = Letterbox::new(window.hwnd(), &chain)?;
                    Ok((chain, letterbox))
                })
                .and_then(|built| {
                    built.map_err(|error| crate::diagnostics::log_line(format_args!(
                        "presentation: letterbox unavailable ({error}); stretching to the window"
                    ))).ok()
                });
            let (base, letterbox) = match composed {
                Some((chain, letterbox)) => (chain, Some(letterbox)),
                None => (factory.CreateSwapChainForHwnd(queue, window.hwnd(), &params, None, None)?, None),
            };
            // DXGI's own Alt+Enter would try exclusive fullscreen behind the
            // window hook's borderless toggle (xmem window.rs).
            let _ = factory.MakeWindowAssociation(window.hwnd(), DXGI_MWA_NO_ALT_ENTER);
            Ok(Self {
                chain: base.cast()?,
                letterbox,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                frames: [Frame::new(device)?, Frame::new(device)?, Frame::new(device)?],
                window,
                width: params.Width,
                height: params.Height,
                format: params.Format,
                next: 1,
                failed_signal: None,
            })
        }
    }

    fn present(
        &mut self,
        queue: &ID3D12CommandQueue,
        source: &ID3D12Resource,
        desc: &D3D12_RESOURCE_DESC,
    ) -> Result<HRESULT> {
        if let Some((failure, _)) = &self.failed_signal {
            return Err((*failure).into());
        }
        // SAFETY: IsWindow accepts an opaque previously returned HWND. The caller
        // holds the session lock for the entire copy, signal and present sequence.
        unsafe {
            if !self.window.is_open() {
                return Err(DXGI_ERROR_ACCESS_LOST.into());
            }
            // Minimised: nothing is shown, so skip the copy and the flip and
            // pace the title instead. Opt-in (TUXBOX_PRESENT_MINIMISED=0) until
            // A/B runs clear it of the borderless no-draw runs.
            if windows::Win32::UI::WindowsAndMessaging::IsIconic(self.window.hwnd()).as_bool() && !present_minimised() {
                std::thread::sleep(std::time::Duration::from_millis(16));
                return Ok(S_OK);
            }
            if u64::from(self.width) != desc.Width
                || self.height != desc.Height
                || self.format != desc.Format
            {
                // The title changed its presented size or format (xcl #711):
                // retire every frame, release back-buffer references, resize.
                let width = u32::try_from(desc.Width).map_err(|_| E_INVALIDARG)?;
                for frame in &mut self.frames {
                    wait_for(&self.fence, frame.retire)?;
                    frame.source = None;
                }
                self.chain.ResizeBuffers(0, width, desc.Height, desc.Format, DXGI_SWAP_CHAIN_FLAG(0))?;
                crate::diagnostics::log_line(format_args!(
                    "presentation resized {}x{} fmt={} -> {width}x{} fmt={}",
                    self.width, self.height, self.format.0, desc.Height, desc.Format.0
                ));
                self.width = width;
                self.height = desc.Height;
                self.format = desc.Format;
            }
            self.window.keep_fitted(self.width, self.height);
            if let Some(letterbox) = &mut self.letterbox {
                let _ = letterbox.layout(self.window.hwnd(), self.width, self.height);
            }
            // The maximum fence value reports device removal and must stay reserved.
            let next_value = self.next.checked_add(1).ok_or(E_FAIL)?;
            let index = self.chain.GetCurrentBackBufferIndex();
            let frame = self
                .frames
                .get_mut(index as usize)
                .ok_or(DXGI_ERROR_INVALID_CALL)?;
            let retire_started = std::time::Instant::now();
            wait_for(&self.fence, frame.retire)?;
            let retire_wait = retire_started.elapsed();
            let buffer = self.chain.GetBuffer(index)?;
            frame.allocator.Reset()?;
            frame.list.Reset(&frame.allocator, None)?;
            record_copy(&frame.list, source, &buffer);
            let sample = (sampler::due() || crate::diagnostics::ctl::screenshot_pending()).then(|| sampler::record(&frame.list, source, D3D12_RESOURCE_STATE_COMMON)).flatten();
            // What reaches the window: the swap chain's own back buffer after
            // the copy, at presents 1 and 60, at the presents after the first 5
            // lit source samples, and every 600 presents (test104's window
            // samples all came before the source lit up).
            let shown = window_reports() && (matches!(self.next, 1 | 60) || self.next % 600 == 0
                || LIT_WINDOW_DUE.swap(false, std::sync::atomic::Ordering::Relaxed));
            let window_sample = shown.then(|| sampler::record(&frame.list, &buffer, D3D12_RESOURCE_STATE_PRESENT)).flatten();
            let feed = feed::record(&frame.list, &buffer);
            frame.list.Close()?;
            frame.source = Some(source.clone());
            queue.ExecuteCommandLists(&[Some(ID3D12CommandList::clone(&frame.list))]);
            crate::dred::check(queue, "present copy");
            if let Err(error) = queue.Signal(&self.fence, self.next) {
                self.failed_signal = Some((error.code(), queue.clone()));
                return Err(error);
            }
            frame.retire = self.next;
            if let Some(feed) = feed {
                feed.submit(&self.fence, self.next);
            }
            crate::diagnostics::gpu_clock::track(&self.fence, self.next);
            if let Some(sample) = sample {
                wait_for(&self.fence, self.next)?;
                let lit = sample.report("present");
                static LIT_SOURCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                if lit > 0.01 && LIT_SOURCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 5 {
                    LIT_WINDOW_DUE.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            }
            let present_number = self.next;
            self.next = next_value;
            // Return DXGI status codes such as DXGI_STATUS_OCCLUDED to the guest.
            let present_started = std::time::Instant::now();
            let status = self.chain.Present(1, DXGI_PRESENT(0));
            present_split(retire_wait, present_started.elapsed(), self.next);
            if let Some(window_sample) = window_sample {
                let _ = wait_for(&self.fence, present_number);
                window_sample.report("window back buffer");
                let line = format!(
                    "presentation at present {present_number}: Present returned {:#010x}, {}x{} format {}, {}, letterbox {} (TUXBOX_WINDOW_REPORTS=0 off)",
                    status.0, self.width, self.height, self.format.0, self.window.describe(), self.letterbox.is_some());
                crate::diagnostics::log_once(&std::sync::Once::new(), || line);
            }
            if status.is_err() {
                static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!("presentation: swap chain Present failed {:#010x} ({n} so far)", status.0));
                }
            }
            uncloak_title_windows();
            // A title that sees DXGI_STATUS_OCCLUDED may stop presenting for good
            // (xvalkyrie): pace it briefly and report S_OK.
            // Opt-in (TUXBOX_OCCLUDED_STATUS=0) until A/B runs clear it.
            if status == windows::Win32::Foundation::DXGI_STATUS_OCCLUDED && !occluded_status() {
                std::thread::sleep(std::time::Duration::from_millis(16));
                return Ok(S_OK);
            }
            Ok(status)
        }
    }
    fn drain(&self) -> Result<()> {
        if let Some((_, queue)) = &self.failed_signal {
            // SAFETY: this marker follows the unfenced copy on its original queue.
            unsafe { queue.Signal(&self.fence, self.next)? };
            wait_for(&self.fence, self.next)?;
        }
        for frame in &self.frames {
            wait_for(&self.fence, frame.retire)?;
        }
        Ok(())
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        let session = self
            .session
            .get_mut()
            .unwrap_or_else(|poison| poison.into_inner());
        let Some(chain) = session.take() else { return };
        if chain.drain().is_err() {
            // SAFETY: the fence retains its device. Following device removal no
            // queued GPU work can continue accessing these allocations.
            let removed = device_of(&chain.fence)
                .is_ok_and(|device| unsafe { device.GetDeviceRemovedReason() }.is_err());
            if !removed {
                // No completion marker and a possibly running GPU: releasing the
                // source, swap buffers or allocator could free memory mid-copy.
                std::mem::forget(chain);
            }
        }
    }
}

/// Where a present's time goes: waiting for the back buffer's previous copy
/// to retire (GPU work queued ahead of it) or inside DXGI's Present (vsync,
/// compositor). Averages every 64 presents. test121-126 spent ~100 ms a
/// frame in present with the GPU 1% busy and no queue waits; this tells the
/// two apart. Diagnostic only.
fn present_split(retire: std::time::Duration, present: std::time::Duration, number: u64) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static RETIRE: AtomicU64 = AtomicU64::new(0);
    static PRESENT: AtomicU64 = AtomicU64::new(0);
    static MAX: AtomicU64 = AtomicU64::new(0);
    RETIRE.fetch_add(retire.as_micros() as u64, Ordering::Relaxed);
    PRESENT.fetch_add(present.as_micros() as u64, Ordering::Relaxed);
    MAX.fetch_max(present.as_micros() as u64, Ordering::Relaxed);
    if number % 64 == 0 {
        let (r, p, m) = (RETIRE.swap(0, Ordering::Relaxed), PRESENT.swap(0, Ordering::Relaxed), MAX.swap(0, Ordering::Relaxed));
        crate::diagnostics::log_line(format_args!(
            "present split over 64 presents (to #{number}): back-buffer retire wait {:.1} ms, DXGI Present {:.1} ms avg ({:.1} ms max)",
            r as f64 / 64e3, p as f64 / 64e3, m as f64 / 1e3));
    }
}

fn wait_for(fence: &ID3D12Fence, value: u64) -> Result<()> {
    if value == 0 {
        return Ok(());
    }
    // SAFETY: a null event handle requests a synchronous wait on a live fence.
    unsafe {
        let completed = fence.GetCompletedValue();
        if completed == u64::MAX {
            return Err(DXGI_ERROR_DEVICE_REMOVED.into());
        }
        if completed < value {
            fence.SetEventOnCompletion(value, HANDLE::default())?;
            // Device removal also ends the wait: the fence then reads the
            // maximum value, which completes no frame (xbox-emulator #351).
            if fence.GetCompletedValue() == u64::MAX {
                return Err(DXGI_ERROR_DEVICE_REMOVED.into());
            }
        }
    }
    Ok(())
}

fn record_copy(list: &ID3D12GraphicsCommandList, source: &ID3D12Resource, buffer: &ID3D12Resource) {
    let transition = |resource: &ID3D12Resource, before, after| D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: ManuallyDrop::new(Some(resource.clone())),
                Subresource: 0,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    let mut barriers = [
        transition(
            source,
            D3D12_RESOURCE_STATE_COMMON,
            D3D12_RESOURCE_STATE_COPY_SOURCE,
        ),
        transition(
            buffer,
            D3D12_RESOURCE_STATE_PRESENT,
            D3D12_RESOURCE_STATE_COPY_DEST,
        ),
    ];
    // SAFETY: both union arms are Transition. The list is open and its resources
    // belong to the same device. Release the two owned pointers in the union
    // after recording both transitions; D3D12 copies the barrier descriptors.
    unsafe {
        list.ResourceBarrier(&barriers);
        list.CopyResource(buffer, source);
        for barrier in &mut barriers {
            let transition = &mut *barrier.Anonymous.Transition;
            swap(&mut transition.StateBefore, &mut transition.StateAfter);
        }
        list.ResourceBarrier(&barriers);
        for barrier in &mut barriers {
            let transition = &mut *barrier.Anonymous.Transition;
            ManuallyDrop::drop(&mut transition.pResource);
        }
    }
}

/// `TUXBOX_TRACE_PRESENT=<n>`: every n-th presented source is read back and
/// summarized (share of non-black pixels, mean, coarse 4x4 brightness grid).
pub(crate) mod sampler {
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

    pub(super) fn due() -> bool {
        static EVERY: std::sync::LazyLock<u64> = std::sync::LazyLock::new(|| {
            std::env::var("TUXBOX_TRACE_PRESENT").ok().and_then(|v| v.parse().ok()).unwrap_or(0)
        });
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        *EVERY != 0 && n % *EVERY == 0
    }

    #[derive(Clone)]
    pub(crate) struct Sample {
        buffer: ID3D12Resource,
        layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        index: u64,
        source: usize,
    }

    /// Record a copy of subresource 0 into a readback buffer, after
    /// record_copy has returned `source` to COMMON.
    pub(crate) fn record(list: &ID3D12GraphicsCommandList, source: &ID3D12Resource, previous: D3D12_RESOURCE_STATES) -> Option<Sample> {
        static INDEX: AtomicU64 = AtomicU64::new(0);
        let device: ID3D12Device = unsafe {
            let mut device = None;
            list.GetDevice(&mut device).ok()?;
            device?
        };
        let desc = unsafe { source.GetDesc() };
        let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0;
        unsafe { device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut layout), None, None, Some(&mut total)) };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let buffer_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: total, Height: 1, DepthOrArraySize: 1,
            MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
        };
        let mut buffer: Option<ID3D12Resource> = None;
        unsafe {
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc,
                D3D12_RESOURCE_STATE_COPY_DEST, None, &mut buffer).ok()?;
        }
        let buffer = buffer?;
        let barrier = |before, after| D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: std::mem::ManuallyDrop::new(Some(source.clone())),
                    Subresource: 0, StateBefore: before, StateAfter: after,
                }),
            },
        };
        unsafe {
            let mut to = [barrier(previous, D3D12_RESOURCE_STATE_COPY_SOURCE)];
            list.ResourceBarrier(&to);
            let dst = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: layout },
            };
            let src = D3D12_TEXTURE_COPY_LOCATION {
                pResource: std::mem::ManuallyDrop::new(Some(source.clone())),
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
            };
            list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
            let mut back = [barrier(D3D12_RESOURCE_STATE_COPY_SOURCE, previous)];
            list.ResourceBarrier(&back);
            for b in to.iter_mut().chain(back.iter_mut()) {
                std::mem::ManuallyDrop::drop(&mut (*b.Anonymous.Transition).pResource);
            }
            let (mut dst, mut src) = (dst, src);
            std::mem::ManuallyDrop::drop(&mut dst.pResource);
            std::mem::ManuallyDrop::drop(&mut src.pResource);
        }
        Some(Sample { buffer, layout, index: INDEX.fetch_add(1, Ordering::Relaxed), source: windows::core::Interface::as_raw(source) as usize })
    }

    impl Sample {
        /// Called after the GPU has finished this frame's commands.
        /// Logs the summary; returns the share of lit pixels.
        pub(crate) fn report(&self, name: &str) -> f64 {
            let f = self.layout.Footprint;
            let mut mapped = std::ptr::null_mut();
            if unsafe { self.buffer.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
                return 0.0;
            }
            let (w, h, pitch) = (f.Width as usize, f.Height as usize, f.RowPitch as usize);
            let data = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), pitch * h) };
            crate::diagnostics::ctl::screenshot_frame(data, w, h, pitch, f.Format.0);
            let (mut lit, mut sum, mut grid) = (0u64, [0u64; 3], [[0u64; 4]; 4]);
            for y in (0..h).step_by(4) {
                for x in (0..w).step_by(4) {
                    // Decoded per format (16-bit integers, XR_BIAS 10:10:10:2, BGRA).
                    let p = crate::diagnostics::ctl::rgb(data, x, y, pitch, f.Format.0).unwrap_or_default();
                    let luma = u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]);
                    if luma > 6 { lit += 1; }
                    for c in 0..3 { sum[c] += u64::from(p[c]); }
                    grid[y * 4 / h][x * 4 / w] += luma;
                }
            }
            let n = ((w / 4) * (h / 4)).max(1) as u64;
            let cells: Vec<String> = grid.iter().map(|row| row.iter()
                .map(|&v| format!("{:3}", v / (n / 16).max(1) / 3)).collect::<Vec<_>>().join(" ")).collect();
            unsafe { self.buffer.Unmap(0, None) };
            let line = format!("{name} sample #{} res={:#x} {}x{} format={} lit={:.3} mean=({},{},{}) grid=[{}]",
                self.index, self.source, w, h, f.Format.0, lit as f64 / n as f64, sum[0] / n, sum[1] / n, sum[2] / n, cells.join(" | "));
            crate::diagnostics::log_once(&std::sync::Once::new(), || line);
            if name == "present" {
                let summary = crate::diagnostics::display::summary();
                crate::diagnostics::log_once(&std::sync::Once::new(), || summary);
            }
            lit as f64 / n as f64
        }
    }
}

/// xmem cloaks the title's top-level window from creation so its unpainted
/// client never flashes white; the first present shows it (TuxBoxUncloakWindows).
fn uncloak_title_windows() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        if let Ok(module) = GetModuleHandleW(windows::core::w!("xmem.dll"))
            && let Some(f) = GetProcAddress(module, windows::core::s!("TuxBoxUncloakWindows"))
        {
            let uncloak: unsafe extern "system" fn() = core::mem::transmute(f);
            uncloak();
        }
    });
}
