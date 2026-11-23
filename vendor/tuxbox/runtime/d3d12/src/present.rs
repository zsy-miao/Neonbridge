//! Present one RGBA8 or RGB10A2 guest texture through a double-buffered DXGI swap chain.

mod window;
use window::OutputWindow;

use std::mem::{ManuallyDrop, swap};
use std::sync::Mutex;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, HANDLE, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT, DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::*;
use windows::core::{HRESULT, Interface, Result};

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
        let desc = unsafe { source.GetDesc() };
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
            || !matches!(
                desc.Format,
                DXGI_FORMAT_R8G8B8A8_UNORM | DXGI_FORMAT_R10G10B10A2_UNORM
            )
            || desc.DepthOrArraySize != 1
            || desc.MipLevels != 1
            || desc.SampleDesc.Count != 1
            || desc.SampleDesc.Quality != 0
            || desc.Width > u64::from(u32::MAX)
        {
            return Err(E_NOTIMPL.into());
        }
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
    frames: [Frame; 2],
    window: OutputWindow,
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
                BufferCount: 2,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
                Scaling: DXGI_SCALING_STRETCH,
                AlphaMode: DXGI_ALPHA_MODE_IGNORE,
                ..Default::default()
            };
            let factory: IDXGIFactory2 = CreateDXGIFactory1()?;
            let base = factory.CreateSwapChainForHwnd(queue, window.hwnd(), &params, None, None)?;
            Ok(Self {
                chain: base.cast()?,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                frames: [Frame::new(device)?, Frame::new(device)?],
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
            if u64::from(self.width) != desc.Width
                || self.height != desc.Height
                || self.format != desc.Format
            {
                return Err(E_NOTIMPL.into());
            }
            // The maximum fence value reports device removal and must stay reserved.
            let next_value = self.next.checked_add(1).ok_or(E_FAIL)?;
            let index = self.chain.GetCurrentBackBufferIndex();
            let frame = self
                .frames
                .get_mut(index as usize)
                .ok_or(DXGI_ERROR_INVALID_CALL)?;
            wait_for(&self.fence, frame.retire)?;
            let buffer = self.chain.GetBuffer(index)?;
            frame.allocator.Reset()?;
            frame.list.Reset(&frame.allocator, None)?;
            record_copy(&frame.list, source, &buffer);
            let sample = sampler::due().then(|| sampler::record(&frame.list, source)).flatten();
            frame.list.Close()?;
            frame.source = Some(source.clone());
            queue.ExecuteCommandLists(&[Some(ID3D12CommandList::clone(&frame.list))]);
            if let Err(error) = queue.Signal(&self.fence, self.next) {
                self.failed_signal = Some((error.code(), queue.clone()));
                return Err(error);
            }
            frame.retire = self.next;
            if let Some(sample) = sample {
                wait_for(&self.fence, self.next)?;
                sample.report();
            }
            self.next = next_value;
            // Return DXGI status codes such as DXGI_STATUS_OCCLUDED to the guest.
            Ok(self.chain.Present(1, DXGI_PRESENT(0)))
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
mod sampler {
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

    pub(super) struct Sample {
        buffer: ID3D12Resource,
        layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        index: u64,
    }

    /// Record a copy of subresource 0 into a readback buffer, after
    /// record_copy has returned `source` to COMMON.
    pub(super) fn record(list: &ID3D12GraphicsCommandList, source: &ID3D12Resource) -> Option<Sample> {
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
            let mut to = [barrier(D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_SOURCE)];
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
            let mut back = [barrier(D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COMMON)];
            list.ResourceBarrier(&back);
            for b in to.iter_mut().chain(back.iter_mut()) {
                std::mem::ManuallyDrop::drop(&mut (*b.Anonymous.Transition).pResource);
            }
            let (mut dst, mut src) = (dst, src);
            std::mem::ManuallyDrop::drop(&mut dst.pResource);
            std::mem::ManuallyDrop::drop(&mut src.pResource);
        }
        Some(Sample { buffer, layout, index: INDEX.fetch_add(1, Ordering::Relaxed) })
    }

    impl Sample {
        /// Called after the GPU has finished this frame's commands.
        pub(super) fn report(&self) {
            let f = self.layout.Footprint;
            let mut mapped = std::ptr::null_mut();
            if unsafe { self.buffer.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
                return;
            }
            let (w, h, pitch) = (f.Width as usize, f.Height as usize, f.RowPitch as usize);
            let data = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), pitch * h) };
            let (mut lit, mut sum, mut grid) = (0u64, [0u64; 3], [[0u64; 4]; 4]);
            for y in (0..h).step_by(4) {
                for x in (0..w).step_by(4) {
                    let p = &data[y * pitch + x * 4..y * pitch + x * 4 + 4];
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
            let line = format!("present sample #{} {}x{} format={} lit={:.3} mean=({},{},{}) grid=[{}]",
                self.index, w, h, f.Format.0, lit as f64 / n as f64, sum[0] / n, sum[1] / n, sum[2] / n, cells.join(" | "));
            crate::diagnostics::log_once(&std::sync::Once::new(), || line);
            let summary = crate::diagnostics::display::summary();
            crate::diagnostics::log_once(&std::sync::Once::new(), || summary);
        }
    }
}
