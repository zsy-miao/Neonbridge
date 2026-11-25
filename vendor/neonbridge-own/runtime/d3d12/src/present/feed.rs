//! The game feed tap (TUXBOX_FEED=1, set by the launcher's NEONBRIDGE_FEED=1).
//!
//! While the launcher's feed server has a viewer, every presented frame due at
//! the feed's rate is copied from the swap chain's back buffer (what reaches
//! the window) into one of three readback buffers in the present's own list.
//! A worker thread waits for that present's fence, scales the frame down by an
//! integer step to at most the feed's width and publishes it as bgr0 into a
//! shared-memory double buffer (TUXBOX_FEED_NAME, default Local\NeonbridgeFeed)
//! that the server reads. Present never waits: with every readback buffer in
//! flight, or with no viewer, the frame is not captured.
//!
//! Shared memory layout (little endian, all fields naturally aligned):
//!   0  u32 magic "NBFD"        4  u32 version (1)
//!   8  u64 viewer tick (server: GetTickCount64 of its last viewer)
//!   16 u32 wanted fps (server; 0 = 30)   20 u32 wanted width (server; 0 = 1280)
//!   24 u32 latest slot (0 or 1)          32 u64 frames published
//!   64 + slot * SLOT: u64 seq (odd while written), u32 width, u32 height,
//!                     then width * height * 4 bytes of B, G, R, 0
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

pub(crate) const MAX_W: usize = 1920;
pub(crate) const MAX_H: usize = 1080;
const HEADER: usize = 64;
const SLOT_HEADER: usize = 64;
const SLOT: usize = SLOT_HEADER + MAX_W * MAX_H * 4;
const SIZE: usize = HEADER + 2 * SLOT;
const RING: usize = 3;

pub(super) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_FEED").is_ok_and(|v| v.trim() == "1"))
}

struct Shared(*mut u8);
// SAFETY: the view stays mapped for the process; every field is accessed atomically
// or inside the slot's seqlock.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn u32(&self, at: usize) -> &AtomicU32 {
        unsafe { &*(self.0.add(at) as *const AtomicU32) }
    }
    fn u64(&self, at: usize) -> &AtomicU64 {
        unsafe { &*(self.0.add(at) as *const AtomicU64) }
    }
}

fn shared() -> Option<&'static Shared> {
    static MAP: std::sync::OnceLock<Option<Shared>> = std::sync::OnceLock::new();
    MAP.get_or_init(|| unsafe {
        use windows::Win32::System::Memory::*;
        let name = std::env::var("TUXBOX_FEED_NAME").unwrap_or_else(|_| r"Local\NeonbridgeFeed".into());
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mapping = CreateFileMappingW(
            windows::Win32::Foundation::INVALID_HANDLE_VALUE, None, PAGE_READWRITE,
            (SIZE as u64 >> 32) as u32, SIZE as u32, windows::core::PCWSTR(wide.as_ptr()));
        let Ok(mapping) = mapping else {
            crate::diagnostics::log_line(format_args!("game feed: shared memory {name} unavailable; no feed this run"));
            return None;
        };
        let view = MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, SIZE);
        if view.Value.is_null() {
            return None;
        }
        // The mapping handle stays open for the process.
        let shared = Shared(view.Value.cast());
        shared.u32(4).store(1, Ordering::Relaxed);
        shared.u32(0).store(u32::from_le_bytes(*b"NBFD"), Ordering::Release);
        crate::diagnostics::log_line(format_args!(
            "game feed: frames go to {name} while the feed has a viewer (TUXBOX_FEED=1)"));
        Some(shared)
    }).as_ref()
}

fn tick() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

struct Readback {
    buffer: ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    busy: std::sync::Arc<AtomicBool>,
}

#[derive(Default)]
struct Tap {
    ring: Vec<Readback>,
    /// (width, height, format) the ring was made for.
    made_for: (u64, u32, i32),
    last: u64,
}

static TAP: Mutex<Option<Tap>> = Mutex::new(None);

/// A capture recorded into the present's list, sent to the worker once the
/// present's fence value is known.
pub(super) struct Pending {
    buffer: ID3D12Resource,
    layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    busy: std::sync::Arc<AtomicBool>,
}

/// Record a copy of the back buffer (in PRESENT) into a free readback buffer
/// when a viewer is watching and a frame is due.
pub(super) fn record(list: &ID3D12GraphicsCommandList, buffer: &ID3D12Resource) -> Option<Pending> {
    if !enabled() {
        return None;
    }
    let shared = shared()?;
    let now = tick();
    // test107b: a --feed-only feed in another process had a viewer, but no frame
    // reached it. Say whether the game ever sees the viewer tick.
    static SEEN: AtomicBool = AtomicBool::new(false);
    static CHECKS: AtomicU64 = AtomicU64::new(0);
    let viewer = shared.u64(8).load(Ordering::Acquire);
    let checks = CHECKS.fetch_add(1, Ordering::Relaxed) + 1;
    if now.saturating_sub(viewer) > 5000 {
        if !SEEN.load(Ordering::Relaxed) && matches!(checks, 600 | 6000 | 60000) {
            crate::diagnostics::log_line(format_args!(
                "game feed: no viewer after {checks} presents (viewer tick {viewer}, now {now})"));
        }
        return None;
    }
    if !SEEN.swap(true, Ordering::Relaxed) {
        crate::diagnostics::log_line(format_args!("game feed: a viewer is watching (tick {} ms old); capturing", now.saturating_sub(viewer)));
    }
    let fps = match shared.u32(16).load(Ordering::Relaxed) { 0 => 30, n => n.clamp(1, 60) };
    let mut guard = TAP.lock().unwrap_or_else(|e| e.into_inner());
    let tap = guard.get_or_insert_with(Tap::default);
    if now.saturating_sub(tap.last) + 2 < u64::from(1000 / fps) {
        return None;
    }
    let desc = unsafe { buffer.GetDesc() };
    let key = (desc.Width, desc.Height, desc.Format.0);
    if tap.made_for != key {
        // Old buffers still in flight keep their own references.
        tap.ring.clear();
        tap.made_for = key;
    }
    let device: ID3D12Device = unsafe {
        let mut device = None;
        list.GetDevice(&mut device).ok()?;
        device?
    };
    if tap.ring.is_empty() {
        let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut total = 0;
        unsafe { device.GetCopyableFootprints(&desc, 0, 1, 0, Some(&mut layout), None, None, Some(&mut total)) };
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_READBACK, ..Default::default() };
        let buffer_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: total, Height: 1, DepthOrArraySize: 1,
            MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
        };
        for _ in 0..RING {
            let mut made: Option<ID3D12Resource> = None;
            unsafe {
                device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer_desc,
                    D3D12_RESOURCE_STATE_COPY_DEST, None, &mut made).ok()?;
            }
            tap.ring.push(Readback { buffer: made?, layout, busy: Default::default() });
        }
    }
    let slot = tap.ring.iter().find(|r| !r.busy.load(Ordering::Acquire))?;
    slot.busy.store(true, Ordering::Release);
    tap.last = now;
    let barrier = |before, after| D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                Subresource: 0, StateBefore: before, StateAfter: after,
            }),
        },
    };
    // SAFETY: the list is open and both resources belong to its device; the
    // owned pointers in the barrier and copy unions are released after recording.
    unsafe {
        let mut to = [barrier(D3D12_RESOURCE_STATE_PRESENT, D3D12_RESOURCE_STATE_COPY_SOURCE)];
        list.ResourceBarrier(&to);
        let mut dst = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(slot.buffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: slot.layout },
        };
        let mut src = D3D12_TEXTURE_COPY_LOCATION {
            pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
        };
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        let mut back = [barrier(D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_PRESENT)];
        list.ResourceBarrier(&back);
        for b in to.iter_mut().chain(back.iter_mut()) {
            std::mem::ManuallyDrop::drop(&mut (*b.Anonymous.Transition).pResource);
        }
        std::mem::ManuallyDrop::drop(&mut dst.pResource);
        std::mem::ManuallyDrop::drop(&mut src.pResource);
    }
    Some(Pending { buffer: slot.buffer.clone(), layout: slot.layout, busy: slot.busy.clone() })
}

struct Job(Pending, ID3D12Fence, u64);
// SAFETY: D3D12 resources and fences are free-threaded.
unsafe impl Send for Job {}

impl Pending {
    /// Hand the capture to the worker once its present's fence value is signalled.
    /// A capture whose list never ran is dropped by the caller, freeing its buffer.
    pub(super) fn submit(self, fence: &ID3D12Fence, value: u64) {
        use std::sync::mpsc;
        static WORKER: std::sync::OnceLock<Mutex<mpsc::SyncSender<Job>>> = std::sync::OnceLock::new();
        let sender = WORKER.get_or_init(|| {
            let (sender, jobs) = mpsc::sync_channel::<Job>(RING);
            let _ = std::thread::Builder::new().name("TuxBox game feed".into()).spawn(move || {
                for Job(pending, fence, value) in jobs {
                    // A null event blocks this worker only; device removal ends it too.
                    if unsafe { fence.SetEventOnCompletion(value, windows::Win32::Foundation::HANDLE::default()) }.is_ok()
                        && unsafe { fence.GetCompletedValue() } != u64::MAX
                    {
                        publish(&pending);
                    }
                }
            });
            Mutex::new(sender)
        });
        let _ = sender.lock().unwrap_or_else(|e| e.into_inner()).try_send(Job(self, fence.clone(), value));
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.busy.store(false, Ordering::Release);
    }
}

/// Scale the read-back frame down by an integer step and publish it as bgr0.
fn publish(pending: &Pending) {
    let Some(shared) = shared() else { return };
    let f = pending.layout.Footprint;
    let mut mapped = std::ptr::null_mut();
    if unsafe { pending.buffer.Map(0, None, Some(&mut mapped)) }.is_err() || mapped.is_null() {
        return;
    }
    let (w, h, pitch, format) = (f.Width as usize, f.Height as usize, f.RowPitch as usize, f.Format.0);
    let data = unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), pitch * h) };
    let wanted = match shared.u32(20).load(Ordering::Relaxed) as usize { 0 => 1280, n => n.clamp(160, MAX_W) };
    let step = w.div_ceil(wanted).max(h.div_ceil(MAX_H)).max(1);
    let (ow, oh) = ((w / step) & !1, (h / step) & !1);
    if ow == 0 || oh == 0 {
        unsafe { pending.buffer.Unmap(0, None) };
        return;
    }
    let latest = shared.u32(24).load(Ordering::Acquire) as usize & 1;
    let slot = 1 - latest;
    let base = HEADER + slot * SLOT;
    let seq = shared.u64(base);
    seq.fetch_add(1, Ordering::AcqRel); // odd: being written
    // SAFETY: the slot's pixels lie inside the view; the reader checks seq.
    let out = unsafe { std::slice::from_raw_parts_mut(shared.0.add(base + SLOT_HEADER), ow * oh * 4) };
    let half = step / 2;
    for y in 0..oh {
        let sy = y * step + half;
        let row = &mut out[y * ow * 4..(y + 1) * ow * 4];
        for x in 0..ow {
            let sx = x * step + half;
            let px = match format {
                // B8G8R8A8 / X8 and R8G8B8A8: direct bytes.
                87 | 88 | 90 | 91 => { let at = sy * pitch + sx * 4; [data[at], data[at + 1], data[at + 2]] }
                27..=29 => { let at = sy * pitch + sx * 4; [data[at + 2], data[at + 1], data[at]] }
                _ => {
                    let [r, g, b] = crate::diagnostics::ctl::rgb(data, sx, sy, pitch, format).unwrap_or_default();
                    [b, g, r]
                }
            };
            row[x * 4..x * 4 + 4].copy_from_slice(&[px[0], px[1], px[2], 0]);
        }
    }
    unsafe { pending.buffer.Unmap(0, None) };
    shared.u32(base + 8).store(ow as u32, Ordering::Relaxed);
    shared.u32(base + 12).store(oh as u32, Ordering::Relaxed);
    seq.fetch_add(1, Ordering::AcqRel); // even: complete
    shared.u32(24).store(slot as u32, Ordering::Release);
    let frames = shared.u64(32).fetch_add(1, Ordering::AcqRel) + 1;
    // The counter outlives this game when a --feed-only feed holds the mapping.
    static OURS: AtomicU64 = AtomicU64::new(0);
    if OURS.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::diagnostics::log_line(format_args!(
            "game feed: first frame {w}x{h} format {format} sent as {ow}x{oh} (step {step}, frame {frames} of the mapping)"));
    }
}
