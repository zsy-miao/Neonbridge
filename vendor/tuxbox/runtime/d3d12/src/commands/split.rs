//! Split a command list at a GPU memory wait recorded after work. Ported
//! from five-star-recomp 5ab7993 (ListSplits / ListReplayState /
//! ListSegments, GPL-3).
//!
//! A wait is honoured only at a host list boundary (memory_wait). A wait
//! recorded after rendering commands used to be skipped, which loosened the
//! ordering with the queue that signals it. With TUXBOX_SPLIT_AT_WAITS=1,
//! such a wait closes the host list recorded so far as a segment. Recording
//! then goes on in another host list on the same allocator, with the state
//! the list had set: descriptor heaps, root signatures and arguments, the
//! pipeline, IA/RS/OM state. At submission the segments run in order, each
//! wait before the segment it precedes. Open queries, unfinished split
//! barriers and predication are not carried over: a list holding any of them
//! is not split, and the wait is skipped as before. A recording splits at
//! most `CAPACITY` times. Five-star turned it off when the split lists'
//! waits on the frame cells stalled every queue for the full timeout; here
//! the GPU wait watchdog releases such a wait. It is on by default now and
//! splits up to 32 times: with it off (and at 4), the title's waits recorded
//! after rendering were skipped (test135-142: 32 and more per run), so the
//! compute queue's lighting ran before the graphics queue's depth pass wrote
//! what it waits for and read a cleared depth buffer (a black picture).
//! VIctory-fxceless orders every such wait (list.rs:1846-1856, queue.rs:84-121,
//! GPL-3.0, credit to its authors). TUXBOX_SPLIT_AT_WAITS=0 skips them as before.
use super::*;
use std::sync::atomic::AtomicU64;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY;

pub(super) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_SPLIT_AT_WAITS") != Some("0"))
}

const CAPACITY: usize = 32;

/// Host state a new host list does not inherit and the bindings do not
/// hold, as the host list got it. Tracked only while splitting is on.
#[derive(Default)]
pub(super) struct Replay {
    topology: Option<D3D_PRIMITIVE_TOPOLOGY>,
    viewports: Option<Vec<D3D12_VIEWPORT>>,
    scissors: Option<Vec<RECT>>,
    targets: Option<(Vec<D3D12_CPU_DESCRIPTOR_HANDLE>, u32, bool, Option<D3D12_CPU_DESCRIPTOR_HANDLE>)>,
    index: Option<Option<D3D12_INDEX_BUFFER_VIEW>>,
    vertex: std::collections::BTreeMap<u32, D3D12_VERTEX_BUFFER_VIEW>,
    blend: Option<[f32; 4]>,
    stencil: Option<u32>,
    depth_bounds: Option<(f32, f32)>,
    predicated: bool,
}

impl Replay {
    pub(super) fn topology(&mut self, topology: D3D_PRIMITIVE_TOPOLOGY) {
        if on() { self.topology = Some(topology); }
    }

    pub(super) unsafe fn viewports(&mut self, count: u32, views: *const D3D12_VIEWPORT) {
        if on() {
            self.viewports = Some(if count == 0 { Vec::new() } else { unsafe { slice::from_raw_parts(views, count as usize) }.to_vec() });
        }
    }

    pub(super) unsafe fn scissors(&mut self, count: u32, rects: *const RECT) {
        if on() {
            self.scissors = Some(if count == 0 { Vec::new() } else { unsafe { slice::from_raw_parts(rects, count as usize) }.to_vec() });
        }
    }

    pub(super) unsafe fn targets(&mut self, count: u32, rtvs: *const D3D12_CPU_DESCRIPTOR_HANDLE, contiguous: bool, dsv: *const D3D12_CPU_DESCRIPTOR_HANDLE) {
        if on() {
            // A contiguous range is named by its first handle.
            let handles = if count == 0 { 0 } else if contiguous { 1 } else { count as usize };
            let rtvs = if handles == 0 { Vec::new() } else { unsafe { slice::from_raw_parts(rtvs, handles) }.to_vec() };
            self.targets = Some((rtvs, count, contiguous, unsafe { dsv.as_ref() }.copied()));
        }
    }

    pub(super) fn index(&mut self, view: Option<D3D12_INDEX_BUFFER_VIEW>) {
        if on() { self.index = Some(view); }
    }

    pub(super) fn vertex(&mut self, start: u32, views: &[D3D12_VERTEX_BUFFER_VIEW]) {
        if on() {
            for (slot, view) in (start..).zip(views) {
                self.vertex.insert(slot, *view);
            }
        }
    }

    pub(super) unsafe fn blend(&mut self, factor: *const f32) {
        if on() {
            self.blend = Some(if factor.is_null() { [1.0; 4] } else { unsafe { *factor.cast::<[f32; 4]>() } });
        }
    }

    pub(super) fn stencil(&mut self, reference: u32) {
        if on() { self.stencil = Some(reference); }
    }

    pub(super) fn depth_bounds(&mut self, minimum: f32, maximum: f32) {
        if on() { self.depth_bounds = Some((minimum, maximum)); }
    }

    pub(super) fn predicated(&mut self, set: bool) {
        self.predicated = set;
    }

    unsafe fn apply(&self, list: &ID3D12GraphicsCommandList) {
        unsafe {
            if let Some(topology) = self.topology {
                list.IASetPrimitiveTopology(topology);
            }
            if let Some(index) = &self.index {
                list.IASetIndexBuffer(index.as_ref().map(core::ptr::from_ref));
            }
            for (&slot, view) in &self.vertex {
                list.IASetVertexBuffers(slot, Some(slice::from_ref(view)));
            }
            if let Some((rtvs, count, contiguous, dsv)) = &self.targets {
                let rtvs = if rtvs.is_empty() { ptr::null() } else { rtvs.as_ptr() };
                (list.vtable().OMSetRenderTargets)(list.as_raw(), *count, rtvs, (*contiguous).into(),
                    dsv.as_ref().map_or(ptr::null(), core::ptr::from_ref));
            }
            if let Some(views) = &self.viewports {
                (list.vtable().RSSetViewports)(list.as_raw(), views.len() as u32, views.as_ptr());
            }
            if let Some(rects) = &self.scissors {
                (list.vtable().RSSetScissorRects)(list.as_raw(), rects.len() as u32, rects.as_ptr());
            }
            if let Some(blend) = &self.blend {
                (list.vtable().OMSetBlendFactor)(list.as_raw(), blend.as_ptr());
            }
            if let Some(reference) = self.stencil {
                list.OMSetStencilRef(reference);
            }
            if let Some((minimum, maximum)) = self.depth_bounds
                && let Ok(list1) = list.cast::<ID3D12GraphicsCommandList1>()
            {
                list1.OMSetDepthBounds(minimum, maximum);
            }
        }
    }
}

/// The host lists this recording was split into and the ones earlier
/// recordings used, kept to record again.
#[derive(Default)]
pub(super) struct Segments {
    /// Closed host lists in recorded order, each followed by its wait; the
    /// list's current host list records the last segment.
    closed: Vec<(ID3D12GraphicsCommandList, Arc<memory_wait::Segment>)>,
    spares: Vec<ID3D12GraphicsCommandList>,
    /// The host allocator the recording uses.
    allocator: Option<ID3D12CommandAllocator>,
    /// The host allocator of the last Reset, split or not (reopen_closed).
    host: Option<ID3D12CommandAllocator>,
    pub(super) replay: Replay,
    /// Waits moved to the start of the current host list (hoist).
    extra: Vec<Arc<memory_wait::Segment>>,
}

/// VIctory-fxceless 7d7963e (list.rs:1839-1860, GPL-3.0, credit to its
/// authors): a wait the list cannot split at is moved to the start of its
/// current host list (holding that segment back), never dropped. Returns
/// false when TUXBOX_WAIT_HOIST=0 (skip as before) or nothing to attach to.
pub(super) fn hoist(state: &mut Recording, segment: Arc<memory_wait::Segment>) -> bool {
    if crate::env_toggle!("TUXBOX_WAIT_HOIST") == Some("0") {
        return false;
    }
    if state.segments.closed.is_empty() {
        if state.prefix.segments.try_reserve(1).is_err() { return false; }
        state.prefix.segments.push(segment);
    } else {
        state.segments.extra.push(segment);
    }
    true
}

/// Splits a recording may make: TUXBOX_SPLIT_CAP (default unlimited; was 32).
fn split_cap() -> usize {
    static CAP: OnceLock<usize> = OnceLock::new();
    *CAP.get_or_init(|| crate::env_toggle!("TUXBOX_SPLIT_CAP").and_then(|v| v.trim().parse().ok()).unwrap_or(usize::MAX))
}

impl Drop for Segments {
    fn drop(&mut self) {
        // A segment may still be executing.
        for (list, _) in self.closed.drain(..) {
            retire(list);
        }
        for list in self.spares.drain(..) {
            retire(list);
        }
    }
}

impl Segments {
    /// A new recording on `allocator`: the closed segments become spares.
    pub(super) fn reset(&mut self, allocator: Option<ID3D12CommandAllocator>) {
        self.host = allocator.clone();
        self.allocator = allocator.filter(|_| on());
        for (list, _) in self.closed.drain(..) {
            if self.spares.len() < CAPACITY { self.spares.push(list); } else { retire(list); }
        }
        self.replay = Replay::default();
        self.extra.clear();
    }

    /// The host allocator of the last Reset (reopen_closed).
    pub(super) fn host_allocator(&self) -> Option<ID3D12CommandAllocator> {
        self.host.clone()
    }

    /// The host list submitted first for this recording.
    pub(super) fn first(&self, native: &ID3D12GraphicsCommandList) -> *mut c_void {
        self.closed.first().map_or(native.as_raw(), |(list, _)| list.as_raw())
    }

    /// The later host lists of this recording, each with the wait before it.
    pub(super) fn tails(&self, native: &ID3D12GraphicsCommandList) -> Vec<(Arc<memory_wait::Segment>, *mut c_void)> {
        let lists = self.closed.iter().skip(1).map(|(list, _)| list.as_raw()).chain([native.as_raw()]);
        let mut tails: Vec<_> = self.closed.iter().map(|(_, segment)| segment.clone()).zip(lists).collect();
        tails.extend(self.extra.iter().map(|segment| (segment.clone(), native.as_raw())));
        tails
    }
}

/// Close the list's host list as a segment followed by `segment`'s wait and
/// record on in another. Ok(false) when this list cannot split.
pub(super) unsafe fn split(list: &List, state: &mut Recording, segment: Arc<memory_wait::Segment>) -> windows::core::Result<bool> {
    let kind = unsafe { list.native.GetType() };
    let segments = &state.segments;
    if !on()
        || kind == D3D12_COMMAND_LIST_TYPE_BUNDLE
        || segments.closed.len() >= split_cap()
        || !state.open_queries.is_empty()
        || !state.split_barriers.is_empty()
        || segments.replay.predicated
    {
        return Ok(false);
    }
    let Some(allocator) = segments.allocator.clone() else { return Ok(false) };
    let pipeline = match state.bindings.pipeline.as_ref() {
        Some(compute_bindings::Pipeline::Regular(pso, ..)) if !state.pipeline_pending => Some(pso.clone()),
        _ => None,
    };
    // Close first: the allocator records one list at a time.
    unsafe { list.native.Close()? };
    enhanced::closed_ok(list);
    let next = match state.segments.spares.pop() {
        Some(spare) => unsafe { spare.Reset(&allocator, pipeline.as_ref()) }.map(|()| spare),
        None => (|| {
            let mut device: Option<ID3D12Device> = None;
            unsafe { list.native.GetDevice(&mut device)? };
            let device = device.ok_or(E_FAIL)?;
            let fresh: ID3D12GraphicsCommandList = unsafe { device.CreateCommandList(0, kind, &allocator, pipeline.as_ref())? };
            crate::dred::name(&fresh, "title (split at a memory wait)");
            Ok(fresh)
        })(),
    };
    let next = match next {
        Ok(next) => next,
        Err(error) => {
            // The closed host list cannot record on: the list fails.
            crate::diagnostics::log_line(format_args!(
                "a list split at a GPU memory wait could not record on ({:#010x}); its recording is refused",
                error.code().0));
            return Err(error);
        }
    };
    let old = unsafe { swap_native(list, next.clone())? };
    state.segments.closed.push((old, segment));
    if kind != D3D12_COMMAND_LIST_TYPE_COPY {
        unsafe { state.bindings.restore_all(&next) };
    }
    if kind == D3D12_COMMAND_LIST_TYPE_DIRECT {
        unsafe { state.segments.replay.apply(&next) };
    }
    static SPLITS: AtomicU64 = AtomicU64::new(0);
    let n = SPLITS.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "split a list at a GPU memory wait after rendering commands (segment {}; {n} splits so far; TUXBOX_SPLIT_AT_WAITS=1)",
            state.segments.closed.len()));
    }
    Ok(true)
}

/// Expand each guest list's host list into its segments, with each wait
/// before the segment it precedes. `tails` maps a first host list to its later
/// host lists.
pub(super) fn expand(
    host: &[*mut c_void],
    splits: memory_wait::Splits,
    tails: &[(*mut c_void, Vec<(Arc<memory_wait::Segment>, *mut c_void)>)],
) -> (Vec<*mut c_void>, memory_wait::Splits) {
    let mut lists = Vec::with_capacity(host.len());
    let mut at = Vec::with_capacity(host.len());
    let mut waits = Vec::new();
    for &first in host {
        at.push(lists.len());
        lists.push(first);
        if let Some((_, later)) = tails.iter().find(|(list, _)| *list == first) {
            for (segment, list) in later {
                waits.push((lists.len(), segment.clone()));
                lists.push(*list);
            }
        }
    }
    let mut all: memory_wait::Splits = splits.into_iter().map(|(i, s)| (at[i], s)).collect();
    all.extend(waits);
    all.sort_by_key(|(i, _)| *i);
    (lists, all)
}
