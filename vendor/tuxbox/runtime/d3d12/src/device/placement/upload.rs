//! Fence a private copy queue against the guest queues that read these textures.
use super::{Texture, committed, lock};
use core::mem::ManuallyDrop;
use std::{collections::VecDeque, sync::Arc};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, HANDLE};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_SAMPLE_DESC};
use windows::core::{HRESULT, Interface, Result};

pub(super) struct Change {
    pub(super) texture: Arc<Texture>,
    pub(super) level: usize,
    pub(super) bytes: Vec<u8>,
    /// `level_hash` of `bytes`
    pub(super) hash: u64,
}

// These COM objects must survive the asynchronous copy, even if the guest
// drops all associated resource and queue references immediately after Submit.
struct Upload {
    buffer: ID3D12Resource,
    /// The buffer's pool bucket (0: not pooled).
    bucket: u64,
    _allocator: ID3D12CommandAllocator,
    list: ID3D12CommandList,
    _textures: Vec<ID3D12Resource>,
}

pub(super) struct Transfer {
    queue: ID3D12CommandQueue,
    pub(super) fence: ID3D12Fence,
    next: u64,
    pub(super) failed: Option<HRESULT>,
    pending: VecDeque<(u64, Upload)>,
    scratch: Vec<u8>,
    /// Snapshot buffers given back by uploads, for the next refresh (at most
    /// SPARE_BYTES of capacity): test69's refresh allocated one per changed level.
    spare: Vec<Vec<u8>>,
    watch: super::watch::Watch,
    refreshes: u64,
    /// Where the next refresh starts in its texture list (see `refresh`).
    cursor: usize,
    pool: Vec<(u64, ID3D12Resource)>,
    pool_bytes: u64,
}

// Upload buffer recycling, from omonkeyz/xbox-emulator
// crates/compat/d3d12_xs/src/upload.rs (de1e1da, GPL-3.0-or-later):
// sizes round up to a power of two of at least 64 KiB, a buffer goes back to
// the pool when its copy's fence completes, and the pool holds at most
// 256 MiB. test49 created 5639 MiB of upload buffers, one per batch.
// TUXBOX_UPLOAD_POOL=0 makes a buffer of the exact size per batch as before.
const POOL_CAP_BYTES: u64 = 256 << 20;
const POOL_MIN_BUCKET: u64 = 64 << 10;
/// Capacity of snapshot buffers kept between refreshes (`Transfer::spare`).
const SPARE_BYTES: usize = 128 << 20;

fn upload_pool() -> bool {
    crate::env_toggle!("TUXBOX_UPLOAD_POOL") != Some("0")
}

fn pool_bucket(bytes: u64) -> u64 {
    bytes.max(POOL_MIN_BUCKET).next_power_of_two()
}

fn pool_take<T>(pool: &mut Vec<(u64, T)>, pool_bytes: &mut u64, size: u64) -> Option<T> {
    let i = pool.iter().rposition(|(s, _)| *s == size)?;
    let (s, b) = pool.swap_remove(i);
    *pool_bytes -= s;
    Some(b)
}

fn pool_give<T>(pool: &mut Vec<(u64, T)>, pool_bytes: &mut u64, size: u64, buf: T, cap: u64) -> bool {
    if *pool_bytes + size > cap {
        return false;
    }
    pool.push((size, buf));
    *pool_bytes += size;
    true
}

#[cfg(test)]
#[test]
fn upload_buckets_are_pooled_by_size_under_the_cap() {
    assert_eq!(pool_bucket(1), 64 << 10);
    assert_eq!(pool_bucket((64 << 10) + 1), 128 << 10);
    let (mut pool, mut bytes) = (Vec::new(), 0);
    assert!(pool_give(&mut pool, &mut bytes, 64, 'a', 128));
    assert!(pool_give(&mut pool, &mut bytes, 64, 'b', 128));
    assert!(!pool_give(&mut pool, &mut bytes, 64, 'c', 128));
    assert_eq!(pool_take(&mut pool, &mut bytes, 128), None);
    assert_eq!(pool_take(&mut pool, &mut bytes, 64), Some('b'));
    assert_eq!(bytes, 64);
}

/// test105: the loading screen's Bink movie came out datamoshed. Its frames are
/// 8-bit planes (Y, and Cr/Cb or CrCb) the CPU writes into placed textures, so
/// a plane left unread by this refresh keeps an old frame under the new ones.
/// Logs, for placed R8/R8G8 textures 256 wide or more, whether each refresh
/// read every mip, skipped some (unwritten per the write watch, or deferred by
/// the read budget) and how many changed: the first 64 lines, then every
/// 1024th, and again the first 64 once Bink has finished a decode (test142's
/// 64 all came before the movie's first frame). TUXBOX_PLANE_TRACE=0 is quiet.
fn plane_trace(texture: &Texture, skipped: usize, changed: usize, out_of_budget: bool) {
    use std::sync::atomic::{AtomicU64, Ordering};
    let desc = &texture.desc;
    if !matches!(desc.Format.0, 49 | 61) || desc.Width < 256 || crate::env_toggle!("TUXBOX_PLANE_TRACE") == Some("0") {
        return;
    }
    static N: AtomicU64 = AtomicU64::new(0);
    static DECODING: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let after_decode = super::bink_state().is_some_and(|(_, done)| done != 0)
        && DECODING.fetch_add(1, Ordering::Relaxed) < 64;
    if n >= 64 && n % 1024 != 0 && !after_decode {
        return;
    }
    let mips = texture.mips.len();
    let read = if skipped == 0 { "full".to_string() } else { format!("{skipped} of {mips} mips skipped") };
    crate::diagnostics::log_line(format_args!(
        "plane texture at title {:#x} ({}x{} format {}): refresh {read}, {changed} changed{}{} (line {})",
        texture.address, desc.Width, desc.Height, desc.Format.0,
        if out_of_budget { ", read budget spent" } else { "" }, super::bink_note(), n + 1));
}

impl Transfer {
    pub(super) fn new(device: &ID3D12Device) -> Result<Self> {
        let desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_COPY,
            ..Default::default()
        };
        Ok(Self {
            queue: unsafe { device.CreateCommandQueue(&desc)? },
            fence: unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? },
            next: 0,
            failed: None,
            pending: VecDeque::new(),
            scratch: Vec::new(),
            spare: Vec::new(),
            watch: Default::default(),
            refreshes: 0,
            cursor: 0,
            pool: Vec::new(),
            pool_bytes: 0,
        })
    }

    pub(super) fn tick(&mut self) -> Result<u64> {
        self.next = self.next.checked_add(1).ok_or(E_FAIL)?;
        Ok(self.next)
    }

    /// `own`: the submitting queue's reader fence, when a submission refreshes.
    pub(super) fn refresh(
        &mut self,
        device: &ID3D12Device,
        textures: &[Arc<Texture>],
        own: Option<&ID3D12Fence>,
    ) -> Result<()> {
        if let Some(error) = self.failed {
            return Err(error.into());
        }
        let completed = unsafe { self.fence.GetCompletedValue() };
        while self
            .pending
            .front()
            .is_some_and(|(tick, _)| *tick <= completed)
        {
            if let Some((_, upload)) = self.pending.pop_front()
                && upload.bucket != 0
            {
                pool_give(&mut self.pool, &mut self.pool_bytes, upload.bucket, upload.buffer, POOL_CAP_BYTES);
            }
        }
        let mut changes = Vec::new();
        let watch_site = crate::diagnostics::cost::site();
        let watch = super::watch::enabled().then(|| {
            self.watch.refresh(textures.iter().map(|t| t.watched_range()).collect());
            &self.watch
        });
        drop(watch_site);
        let read_site = crate::diagnostics::cost::site();
        let (mut skipped, mut mips) = (0, 0);
        let mut budget = super::refresh_read_budget();
        // test72 deferred 524288 levels: each refresh walked its textures in
        // the same order and spent the budget on the same leading ones, so
        // the tail never read. A refresh now starts where the last one ran
        // out, and stops reading after TUXBOX_REFRESH_READ_MS (4; 0: no time
        // limit) as well as at the byte budget. TUXBOX_REFRESH_ROTATE=0
        // always starts at the first texture.
        let rotate = crate::env_toggle!("TUXBOX_REFRESH_ROTATE") != Some("0");
        static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let ms = *MS.get_or_init(|| crate::env_toggle!("TUXBOX_REFRESH_READ_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(4));
        let count = textures.len();
        // A refresh over 1024 or more textures (a world load, a camera cut)
        // gets three times the bytes and time: at the fixed budget such a
        // batch took dozens of refreshes to catch up, drawing stale levels
        // meanwhile. VIctory-fxceless draw.rs:1528-1531 (GPL-3.0, credit to
        // its authors). TUXBOX_REFRESH_BUSY_SCALE=1 keeps the fixed budget.
        static SCALE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let scale: u64 = *SCALE.get_or_init(|| crate::env_toggle!("TUXBOX_REFRESH_BUSY_SCALE").and_then(|v| v.trim().parse().ok()).unwrap_or(3_u64).max(1));
        let (budget_scale, ms) = if count >= 1024 { (scale, ms.saturating_mul(scale)) } else { (1, ms) };
        budget = budget.saturating_mul(budget_scale);
        super::REFRESH_FULL.store(budget, std::sync::atomic::Ordering::Relaxed);
        let limited = budget != 0;
        let started = std::time::Instant::now();
        let start = if rotate && count != 0 { self.cursor % count } else { 0 };
        let mut stopped = None;
        for k in 0..count {
            let i = (start + k) % count;
            if limited && budget != 0 && ms != 0 && k != 0 && started.elapsed() >= std::time::Duration::from_millis(ms) {
                budget = 0;
            }
            let before = changes.len();
            let skip = textures[i].changed(&mut changes, &mut self.scratch, watch, &mut budget, &mut self.spare)?;
            skipped += skip;
            mips += textures[i].mips.len();
            plane_trace(&textures[i], skip, changes.len() - before, budget == 0 && limited);
            if limited && stopped.is_none() && budget == 0 {
                stopped = Some(i);
            }
        }
        drop(read_site);
        self.cursor = stopped.unwrap_or(start);
        self.refreshes += 1;
        if matches!(self.refreshes, 1 | 100 | 1000 | 10000 | 100000) {
            crate::diagnostics::log_line(format_args!(
                "placed textures refresh #{}: {skipped} of {mips} mips skipped by write watch (unwritten) or deferred, {} changed, {} textures so far re-read after another claimed their pages; video planes so far: {} read while Bink decodes (expected 0), {} put off by the Bink gate{}",
                self.refreshes, changes.len(), super::LOST_CLAIMS.load(std::sync::atomic::Ordering::Relaxed),
                super::PLANE_READ_DURING_DECODE.load(std::sync::atomic::Ordering::Relaxed),
                super::PLANE_GATED.load(std::sync::atomic::Ordering::Relaxed), if watch.is_none() { "; no write watch (TUXBOX_PLACED_WATCH=0 or xmem without TuxBoxTakeWriteWatch)" } else { "" }));
        }
        if changes.is_empty() {
            return Ok(());
        }
        if crate::diagnostics::display::trace_views() {
            for change in &changes {
                let desc = change.texture.desc;
                if desc.Height == 1 && desc.Width <= 1024 {
                    use windows::core::Interface;
                    crate::diagnostics::recording_error(format_args!(
                        "uploading changed narrow texture {:p}, guest {:#x}, {}x{} format {} subresource {}",
                        change.texture.native.as_raw(), change.texture.address, desc.Width,
                        desc.Height, desc.Format.0, change.level,
                    ));
                }
            }
        }
        let record_site = crate::diagnostics::cost::site();
        let upload = Upload::record(device, &changes, &mut self.pool, &mut self.pool_bytes)?;
        drop(record_site);
        for mips in changes.chunk_by(|a, b| Arc::ptr_eq(&a.texture, &b.texture)) {
            // A guest may use a different queue on each submission. Order
            // replacement writes after every outstanding reader of this texture.
            // test77: queues stalled 2 s behind an upload whose copy queue
            // waited on another queue's reader, itself waiting on the title's
            // GPU memory wait. The title orders other queues' reads before its
            // CPU writes itself, so a submission's upload waits only for its
            // own queue's readers (TUXBOX_PLACED_CROSS_WAITS=1: all).
            let contents = lock(&mips[0].texture.contents);
            for (fence, value) in &contents.readers {
                if own.is_some_and(|own| own != fence) && !super::cross_waits() {
                    continue;
                }
                if unsafe { fence.GetCompletedValue() } < *value {
                    unsafe {
                        self.queue.Wait(fence, *value)?;
                    }
                }
            }
        }
        let tick = self.tick()?;
        self.pending.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        // Pages mapped since (pages.rs) before the upload writes them.
        unsafe { super::pages::before_execute(&self.queue) };
        unsafe {
            self.queue.ExecuteCommandLists(&[Some(upload.list.clone())]);
            crate::dred::check(&self.queue, "placement upload");
        }
        self.pending.push_back((tick, upload));
        if let Err(error) = unsafe { self.queue.Signal(&self.fence, tick) } {
            self.failed = Some(error.code());
            return Err(error);
        }
        for change in changes {
            let mut state = lock(&change.texture.contents);
            state.previous[change.level] = Some(change.hash);
            state.uploaded = tick;
            state.uploaded_on = Some(self.fence.clone());
            state.readers.clear();
            drop(state);
            let held: usize = self.spare.iter().map(Vec::capacity).sum();
            if held + change.bytes.capacity() <= SPARE_BYTES && change.bytes.capacity() <= SPARE_BYTES / 4 {
                self.spare.push(change.bytes);
            }
        }
        Ok(())
    }
}

impl Drop for Transfer {
    fn drop(&mut self) {
        if let Some((tick, _)) = self.pending.back() {
            // Resignal in case the original signal failed after ExecuteCommandLists.
            unsafe {
                if self.fence.GetCompletedValue() < *tick
                    && self.queue.Signal(&self.fence, *tick).is_ok()
                {
                    let _ = self.fence.SetEventOnCompletion(*tick, HANDLE::default());
                }
            }
        }
    }
}

impl Upload {
    fn record(device: &ID3D12Device, changes: &[Change], pool: &mut Vec<(u64, ID3D12Resource)>, pool_bytes: &mut u64) -> Result<Self> {
        let alignment = u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT);
        let mut total = 0u64;
        let mut offsets = Vec::with_capacity(changes.len());
        for change in changes {
            let offset = total
                .checked_next_multiple_of(alignment)
                .ok_or(E_OUTOFMEMORY)?;
            offsets.push(offset);
            total = offset
                .checked_add(change.texture.mips[change.level].bytes)
                .ok_or(E_OUTOFMEMORY)?;
        }
        let size = usize::try_from(total)
            .ok()
            .filter(|n| *n <= isize::MAX as usize)
            .ok_or(E_OUTOFMEMORY)?;
        let bucket = if upload_pool() { pool_bucket(total) } else { 0 };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: if bucket != 0 { bucket } else { total },
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let buffer = match (bucket != 0).then(|| pool_take(pool, pool_bytes, bucket)).flatten() {
            Some(buffer) => buffer,
            None => committed(
                device,
                D3D12_HEAP_TYPE_UPLOAD,
                &desc,
                D3D12_RESOURCE_STATE_GENERIC_READ,
                None,
            )?,
        };
        let allocator = unsafe { device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)? };
        let list: ID3D12GraphicsCommandList =
            unsafe { device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)? };
        crate::dred::name(&list, "tuxbox placed texture upload");
        let mut mapped = core::ptr::null_mut();
        let empty = D3D12_RANGE::default();
        unsafe {
            buffer.Map(0, Some(&empty), Some(&mut mapped))?;
        }
        // SAFETY: this CPU-writeable buffer covers 'size' bytes; this command
        // list has not been executed and the pointer is scoped to the mapping.
        let output = unsafe { core::slice::from_raw_parts_mut(mapped.cast::<u8>(), size) };
        let mut destinations = Vec::new();
        let result: Result<()> = changes
            .iter()
            .zip(offsets)
            .try_for_each(|(change, offset)| {
                change.detile(&mut output[offset as usize..])?;
                let texture = &change.texture;
                let mut footprint = texture.mips[change.level].footprint;
                footprint.Offset = offset;
                let mut src = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(buffer.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: footprint,
                    },
                };
                let mut dst = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(texture.native.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        SubresourceIndex: change.level as u32,
                    },
                };
                // SAFETY: both active union fields match their location tags. The
                // batch retains source and destinations until its fence completes.
                unsafe {
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                    ManuallyDrop::drop(&mut src.pResource);
                    ManuallyDrop::drop(&mut dst.pResource);
                }
                if destinations.last() != Some(&texture.native) {
                    destinations.push(texture.native.clone());
                }
                Ok(())
            });
        unsafe {
            buffer.Unmap(0, None);
        }
        result?;
        unsafe {
            list.Close()?;
        }
        Ok(Self {
            buffer,
            bucket,
            _allocator: allocator,
            list: list.cast()?,
            _textures: destinations,
        })
    }
}

impl Change {
    fn detile(&self, output: &mut [u8]) -> Result<()> {
        self.texture.layout_runs(self.level, |tiled, linear| {
            output
                .get_mut(linear)
                .ok_or(E_INVALIDARG)?
                .copy_from_slice(self.bytes.get(tiled).ok_or(E_INVALIDARG)?);
            Ok(())
        })
    }
}

impl Texture {
    /// Corresponding contiguous byte runs in the tiled Xbox mip and in its
    /// D3D12 footprint. Upload and writeback must use the same addresses.
    pub(super) fn layout_runs(
        &self,
        level: usize,
        mut visit: impl FnMut(std::ops::Range<usize>, std::ops::Range<usize>) -> Result<()>,
    ) -> Result<()> {
        let mip = &self.mips[level];
        let element = mip.element_bytes as usize;
        let pitch = mip.footprint.Footprint.RowPitch as usize;
        let Some(plane) = &self.plane else {
            let source_pitch = ((mip.source.end - mip.source.start) / u64::from(mip.rows)) as usize;
            let row = mip.elements as usize * element;
            for y in 0..mip.rows as usize {
                visit(y * source_pitch..y * source_pitch + row, y * pitch..y * pitch + row)?;
            }
            return Ok(());
        };
        if self.desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
            for z in 0..mip.footprint.Footprint.Depth {
                for y in 0..mip.rows {
                    for x in 0..mip.elements {
                        let tiled = plane.element_offset_in_volume(z, mip.level, u64::from(x), y).ok_or(E_INVALIDARG)? as usize;
                        let linear = (z as usize * mip.rows as usize + y as usize) * pitch + x as usize * element;
                        visit(tiled..tiled + element, linear..linear + element)?;
                    }
                }
            }
            return Ok(());
        }
        // In layout 0x105, X occupies the low address bits through byte 15.
        // The display swizzle (0x10a) interleaves Y below 16 bytes for small
        // elements, so it is copied an element at a time.
        let run = if self.desc.Layout.0 == 0x100 {
            mip.elements as usize
        } else if self.desc.Layout.0 == 0x10a {
            1
        } else {
            16 / element
        };
        for y in 0..mip.rows {
            for x in (0..mip.elements).step_by(run) {
                let count = (mip.elements - x).min(run as u32) as usize * element;
                let tiled = plane
                    .element_offset_in_slice(mip.slice, mip.level, x.into(), y)
                    .and_then(|a| a.checked_sub(mip.source.start))
                    .ok_or(E_INVALIDARG)? as usize;
                let linear = y as usize * pitch + x as usize * element;
                visit(tiled..tiled + count, linear..linear + count)?;
            }
        }
        Ok(())
    }
}
