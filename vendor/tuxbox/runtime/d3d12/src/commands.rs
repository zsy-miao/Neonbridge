//! Guest command lists and queues. Embedded guest COM pointers are translated
//! before a batch reaches the desktop API.

use core::{ffi::c_void, mem, ops::Range, ptr, slice};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, RECT, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows::core::{HRESULT, IUnknown, Interface, s};

use crate::device;
use crate::device::placement::{self, Texture};
use crate::present::Presentation;
use crate::proxy::{Proxy, Slots, allocate, child_slots, unwrap};

mod build_raytracing;
pub(crate) use build_raytracing::LIST6;
mod compute;
mod compute_bindings;
mod depth;
mod enhanced;
mod descriptors;
mod graphics;
mod indirect_clamp;
mod media;
mod memory_wait;
pub(crate) mod page_mapping;
mod queries;
mod ray_dispatch;
mod raytracing;
pub(crate) mod serialized_blas;
mod stencil;

enum ListState {
    Fresh(ID3D12CommandAllocator, Option<ID3D12PipelineState>),
    Open,
    Closed,
}

/// Count draws and dispatches dropped for a pipeline still being made.
fn dropped_for_pending() {
    static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let dropped = DROPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if dropped.is_power_of_two() {
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("dropped {dropped} draws/dispatches whose pipeline was still being made")
        });
    }
}

struct Recording {
    prefix: memory_wait::Prefix,
    timestamp_page: Option<(ID3D12QueryHeap, u32)>,
    resources: device::MediaResources,
    scene_commands: Vec<raytracing::SceneCommand>,
    bindings: compute_bindings::Bindings,
    stencil_slot: Option<u32>,
    stencil_targets: Option<stencil::Targets>,
    dsv_bound: bool,
    display_bound: bool,
    trace_target: Option<ID3D12Resource>,
    trace_samples: Vec<(String, crate::present::sampler::Sample)>,
    null_pipeline: bool,
    /// The bound pipeline is lazy and still being made: draws and
    /// dispatches are dropped until the list binds it again once it exists.
    pipeline_pending: bool,
    /// This recording's indirect execution budget (see indirect_clamp).
    clamp_budget: Option<ID3D12Resource>,
    media: Option<media::Recording>,
    placed_reads: Vec<Arc<Texture>>,
    placed_writes: Vec<Arc<Texture>>,
    bound_placed_targets: Vec<Arc<Texture>>,
    resource_heap: Option<descriptors::ResourceHeap>,
    descriptor_arena: Option<Arc<device::descriptor_paging::Arena>>,
    indexed_arenas: Vec<Arc<device::descriptor_paging::Arena>>,
    scalar_descriptors: std::collections::HashSet<descriptors::ScalarDescriptor>,
    sampled_heaps: Vec<Range<usize>>,
    /// TUXBOX_LIST_RING=1: guest caller RVAs of the latest recording calls,
    /// dumped when the host Close rejects the list.
    ring: std::collections::VecDeque<usize>,
    /// Writes this list records to the page of a stalled GPU memory wait.
    stalled_writes: Vec<u64>,
    /// Queries begun and not yet ended, ended at Close: an open query makes
    /// the host Close fail.
    open_queries: Vec<(ID3D12QueryHeap, D3D12_QUERY_TYPE, u32)>,
    /// BEGIN_ONLY transitions without their END_ONLY yet, finished at Close.
    split_barriers: Vec<TransitionBarrier>,
    phase: ListState,
    error: HRESULT,
}

impl Recording {
    fn track_texture(accesses: &mut Vec<Arc<Texture>>, texture: &Arc<Texture>) -> Result<(), HRESULT> {
        if !accesses.iter().any(|known| Arc::ptr_eq(known, texture)) {
            accesses.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
            accesses.push(Arc::clone(texture));
        }
        Ok(())
    }

    fn prepare_copy(
        &mut self,
        destination: Option<&Arc<Texture>>,
        source: Option<&Arc<Texture>>,
    ) -> Result<(), HRESULT> {
        for (accesses, texture) in [
            (&mut self.placed_reads, source),
            (&mut self.placed_writes, destination),
        ] {
            if let Some(texture) = texture {
                Self::track_texture(accesses, texture)?;
            }
        }
        Ok(())
    }

    fn compute_dispatch(&mut self, list: &List, dispatch: impl FnOnce()) {
        if self.pipeline_pending {
            dropped_for_pending();
            return;
        }
        if let Some(table) = self.bindings.reflected_table::<true>() {
            if let Err(error) = self.bindings.with_reflected_table::<true>(
                table, list, self.descriptor_arena.as_deref(), self.resource_heap.as_ref(),
                &mut self.sampled_heaps, &self.resources, dispatch,
            ) {
                self.error = error.code();
            }
            return;
        }
        if self.descriptor_arena.is_some() && !self.bindings.supports_paged_draw::<true>() {
            self.error = E_NOTIMPL;
            return;
        }
        if let Err(error) = self.bindings.refresh_inline_tables::<true>(self.descriptor_arena.as_deref()) {
            self.error = error.code();
            return;
        }
        self.sample_tables::<true>();
        let Some(root) = self.bindings.scene_root() else {
            dispatch();
            return;
        };
        let result = self.bindings.scene_resource(list)
            .and_then(|scene| {
                if let Some([instances, normals]) = &scene.geometry {
                    raytracing::retain(self, &[&scene.scene.0, &instances.0, &normals.0])?;
                } else {
                    raytracing::retain(self, &[&scene.scene.0])?;
                }
                Ok(scene)
            })
            .and_then(|scene| {
                let va = scene.scene.1;
                let geometry = scene.geometry.as_ref().map(|[instances, normals]| [instances.1, normals.1]);
                if scene.recheck_on_submit()
                    && !matches!(self.scene_commands.last(), Some(raytracing::SceneCommand::Check(last)) if *last == scene)
                {
                    self.scene_commands.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    self.scene_commands.push(raytracing::SceneCommand::Check(scene));
                }
                unsafe { self.bindings.dispatch_scene(&list.native, root, va, geometry, dispatch) };
                crate::diagnostics::log_once(&root.program.first_dispatch, || {
                    format!("recorded inline compute shader {} dispatch with scene GPU VA {va:#x}", root.program.source_hash)
                });
                Ok(())
            });
        if let Err(error) = result {
            crate::diagnostics::recording_error(format_args!(
                "inline ray scene binding: {:?}", error.code()
            ));
            self.error = error.code();
        }
    }

    fn draw(&mut self, list: &List, rasterizes: bool, draw: impl FnOnce()) {
        if self.pipeline_pending {
            dropped_for_pending();
            return;
        }
        if rasterizes {
            crate::diagnostics::display::draw(self.display_bound, self.null_pipeline);
        }
        if rasterizes {
            for target in &self.bound_placed_targets {
                if let Err(error) = Self::track_texture(&mut self.placed_writes, target) {
                    self.error = error;
                    return;
                }
            }
        }
        let trace = |state: &mut Self| {
            if let Some(trace) = &state.bindings.draw_trace {
                trace.recorded(|| state.bindings.raster_inputs(state.resource_heap.as_ref(), state.descriptor_arena.as_deref(), state.display_bound));
                if let Some(target) = &state.trace_target
                    && let Some(label) = trace.sample_label()
                    && let Some(sample) = crate::present::sampler::record(
                        &list.native, target, D3D12_RESOURCE_STATE_RENDER_TARGET,
                    )
                {
                    state.trace_samples.push((label, sample));
                }
            }
        };
        if let Some(table) = self.bindings.reflected_table::<false>() {
            if self.stencil_slot.is_some() { self.error = E_NOTIMPL; return; }
            if let Some(binding) = table.index_map.filter(|b| b.scalar_mask != 0) {
                // These PSOs have one descriptor table followed by one root
                // constants parameter. The native ISA selects its scalar
                // buffer with a word (and optional mask) from that parameter.
                let descriptor = self.bindings.graphics_constant(1, binding.scalar_word)
                    .and_then(|word| self.resource_heap.as_ref()?.scalar_descriptor(
                        word & binding.scalar_mask, self.descriptor_arena.as_deref(),
                    ));
                let Some(descriptor) = descriptor else { self.error = E_INVALIDARG; return };
                if self.scalar_descriptors.try_reserve(1).is_err() {
                    self.error = E_OUTOFMEMORY;
                    return;
                }
                self.scalar_descriptors.insert(descriptor);
                if let Some(arena) = &self.descriptor_arena
                    && !self.indexed_arenas.iter().any(|used| Arc::ptr_eq(used, arena))
                {
                    if self.indexed_arenas.try_reserve(1).is_err() {
                        self.error = E_OUTOFMEMORY;
                        return;
                    }
                    self.indexed_arenas.push(arena.clone());
                }
            }
            if let Err(error) = self.bindings.with_reflected_table::<false>(
                table, list, self.descriptor_arena.as_deref(), self.resource_heap.as_ref(), &mut self.sampled_heaps, &self.resources, draw,
            ) {
                self.error = error.code();
                if crate::diagnostics::display::trace_views() {
                    crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                        "reflected raster binding failed {:#010x} guest RVA {:#x}", self.error.0,
                        crate::survey::title_rva().unwrap_or(0),
                    ));
                }
            } else { trace(self); }
            return;
        }
        if self.descriptor_arena.is_some() && !self.bindings.supports_paged_draw::<false>() {
            self.error = E_NOTIMPL;
            return;
        }
        if let Err(error) = self.bindings.refresh_inline_tables::<false>(self.descriptor_arena.as_deref()) {
            self.error = error.code();
            if crate::diagnostics::display::trace_views() {
                crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                    "original raster binding failed {:#010x} guest RVA {:#x}", self.error.0,
                    crate::survey::title_rva().unwrap_or(0),
                ));
            }
            return;
        }
        self.sample_tables::<false>();
        if let Some(slot) = self.stencil_slot
            && self.dsv_bound
        {
            let status = self
                .stencil_targets
                .as_ref()
                .ok_or_else(|| windows::core::Error::from(E_NOTIMPL))
                .and_then(|t| t.draw(&list.native, slot, draw));
            if let Err(e) = status {
                self.error = e.code();
            } else { trace(self); }
        } else {
            draw();
            trace(self);
        }
    }
    /// End what the title left open: a begun query or a split barrier
    /// without its end half makes the host Close fail.
    fn balance(&mut self, native: &ID3D12GraphicsCommandList) {
        if !self.open_queries.is_empty() || !self.split_barriers.is_empty() {
            static ONCE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&ONCE, || format!(
                "closing a list with {} open queries and {} unfinished split barriers; ended them",
                self.open_queries.len(), self.split_barriers.len()));
        }
        for (heap, kind, index) in self.open_queries.drain(..) {
            unsafe { native.EndQuery(&heap, kind, index) };
        }
        let ends: Vec<TransitionBarrier> = self.split_barriers.drain(..)
            .map(|b| TransitionBarrier { flags: D3D12_RESOURCE_BARRIER_FLAG_END_ONLY, ..b })
            .collect();
        if !ends.is_empty() {
            unsafe { (native.vtable().ResourceBarrier)(native.as_raw(), ends.len() as u32, ends.as_ptr().cast()) };
        }
    }

    fn sample_tables<const COMPUTE: bool>(&mut self) {
        if let Some(heap) = &self.resource_heap
            && self.bindings.sample_tables::<COMPUTE>(heap, &mut self.sampled_heaps).is_none()
        {
            self.sample_heap();
        }
    }

    fn sample_heap(&mut self) {
        if let Some(heap) = &self.resource_heap {
            descriptors::include_range(&mut self.sampled_heaps, heap.cpu.clone());
        }
    }

    fn can_record(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if !self.ensure_open(native) { return false; }
        if let Err(error) = self.prefix.finish(native) {
            self.error = error.code();
            return false;
        }
        true
    }

    /// A graphics-only call on a compute or copy list: the core runtime
    /// refuses the whole list at Close, so drop just the call.
    fn can_record_graphics(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if !self.can_record(native) { return false; }
        let kind = unsafe { native.GetType() };
        if kind == D3D12_COMMAND_LIST_TYPE_DIRECT { return true; }
        static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = DROPPED.fetch_add(1, Ordering::Relaxed);
        if n < 8 || (n + 1).is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "dropped a graphics-only call on a type {} list from guest RVA {:#x} ({} so far)",
                kind.0, crate::survey::title_rva().unwrap_or(0), n + 1
            ));
        }
        false
    }

    fn can_record_for<const COMPUTE: bool>(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if COMPUTE { self.can_record_shader(native) } else { self.can_record_graphics(native) }
    }

    /// Shader state and dispatches: a copy list refuses them like graphics.
    fn can_record_shader(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if unsafe { native.GetType() } == D3D12_COMMAND_LIST_TYPE_COPY {
            return self.can_record_graphics(native);
        }
        self.can_record(native)
    }

    /// A call our own checks refused (E_NOTIMPL / E_INVALIDARG) is dropped
    /// alone: latching it silenced the rest of the list and the submission
    /// dropped the whole batch, so frames stayed black. Draws and dispatches
    /// wait for the next pipeline bind, in case the refused call was state
    /// they depend on. TUXBOX_STRICT_LISTS=1 keeps the old latch.
    fn forgive(&mut self) {
        if !matches!(self.phase, ListState::Open)
            || !matches!(self.error, E_NOTIMPL | E_INVALIDARG)
            || strict_lists()
        {
            return;
        }
        static FORGIVEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = FORGIVEN.fetch_add(1, Ordering::Relaxed);
        if n < 8 || (n + 1).is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "dropped one refused recording call {:#010x}, list continues ({} so far)", self.error.0, n + 1
            ));
        }
        self.error = S_OK;
        self.pipeline_pending = true;
    }

    fn ensure_open(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        self.forgive();
        if self.error.is_err() {
            return false;
        }
        if ring_enabled() {
            if self.ring.len() >= 32 { self.ring.pop_front(); }
            self.ring.push_back(crate::survey::title_pc());
        }
        match &self.phase {
            ListState::Fresh(allocator, initial) => {
                // The factory closed the empty native list to release its allocator.
                // Reopen with its original bindings when the first operation records.
                if let Err(error) = unsafe { native.Reset(allocator, initial.as_ref()) } {
                    crate::diagnostics::log_line(format_args!("fresh list reopen failed {:#010x}", error.code().0));
                    self.error = error.code();
                    return false;
                }
                self.phase = ListState::Open;
            }
            ListState::Open => {}
            ListState::Closed => self.error = E_FAIL,
        }
        self.error.is_ok()
    }
}

/// TUXBOX_XARGS_STRICT=1: slots 67, 71 and 72 refuse GTA's argument shapes
/// again (unknown cache masks, marker flags, plain 64-bit value writes).
pub(super) fn xargs_strict() -> bool {
    crate::env_toggle!("TUXBOX_XARGS_STRICT").is_some_and(|v| v != "0")
}

fn strict_lists() -> bool {
    static STRICT: OnceLock<bool> = OnceLock::new();
    *STRICT.get_or_init(|| std::env::var_os("TUXBOX_STRICT_LISTS").is_some_and(|v| v != "0"))
}

fn ring_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("TUXBOX_LIST_RING").is_some_and(|v| v != "0"))
}

// Xbox callers can emit native command packets when space is available.
// Frosty's topology path compares +0x18 with +0x30 at RVA 0x57f02fe.
// Empty bounds select the Xbox vtable path, which records desktop commands.
#[repr(C)]
struct ListData {
    reserved0: [usize; 2],
    write_cursor: usize,
    reserved1: [usize; 2],
    packet_end: usize,
    reserved2: [u8; 0x540 - 0x38],
    // The guest writes these pipeline packet words before it checks the
    // packet bounds and calls slot 25. Keep them separate from host state.
    pipeline_packets: core::cell::UnsafeCell<[u8; 32]>,
    recording: Mutex<Recording>,
    extended: OnceLock<Option<ID3D12GraphicsCommandList1>>,
    enhanced_barriers: OnceLock<Option<ID3D12GraphicsCommandList7>>,
    mesh: OnceLock<Option<ID3D12GraphicsCommandList6>>,
    raytracing: Option<ID3D12GraphicsCommandList4>,
}

type List = Proxy<ID3D12GraphicsCommandList, ListData>;
const _: () =
    assert!(mem::offset_of!(List, state) + mem::offset_of!(ListData, pipeline_packets) == 0x540);
const _: () =
    assert!(mem::offset_of!(List, state) + mem::offset_of!(ListData, write_cursor) == 0x18);
const _: () = assert!(mem::offset_of!(List, state) + mem::offset_of!(ListData, packet_end) == 0x30);

/// The list's recording state, held for one call; the hold time feeds the
/// heartbeat's "record" cost.
struct Held<'a>(MutexGuard<'a, Recording>, #[allow(dead_code)] crate::diagnostics::cost::Timer);
impl core::ops::Deref for Held<'_> {
    type Target = Recording;
    fn deref(&self) -> &Recording { &self.0 }
}
impl core::ops::DerefMut for Held<'_> {
    fn deref_mut(&mut self) -> &mut Recording { &mut self.0 }
}

#[track_caller]
fn recording(list: &List) -> Held<'_> {
    let timer = crate::diagnostics::cost::sited(crate::diagnostics::cost::Bucket::Record);
    Held(list.state.recording.lock().unwrap_or_else(|poison| poison.into_inner()), timer)
}

// FD3D12CommandListFence::Signal reads the type at queue+0x10 before
// selecting the flush flags. The queue's Xbox prefix precedes private state.
#[repr(C)]
struct QueueState {
    reserved: usize,
    kind: D3D12_COMMAND_LIST_TYPE,
    // ExecuteCommandLists is void. Report a rejected batch on the next fence
    // signal or present, and never signal completion for missing work.
    placement: placement::QueueAccess,
    submission: Mutex<bool>, // Whether any native GPU operation has been submitted.
    page_worker: OnceLock<page_mapping::Worker>,
    wait_worker: OnceLock<memory_wait::Worker>,
    presentation: Presentation,
}

type Queue = Proxy<ID3D12CommandQueue, QueueState>;
const _: () = assert!(mem::offset_of!(Queue, state) + mem::offset_of!(QueueState, kind) == 0x10);

fn parent(device: &Option<IUnknown>) -> *mut c_void {
    device.as_ref().map_or(ptr::null_mut(), Interface::as_raw)
}

// Validate and translate the entire batch before submission. Small batches use
// stack storage and larger ones can report E_OUTOFMEMORY to the guest.
fn translate_batch<T: Default, G, C>(
    guest: &[G],
    mut context: C,
    mut translate: impl FnMut(&G, &mut C) -> Result<T, HRESULT>,
    submit: impl FnOnce(&[T], C) -> Result<(), HRESULT>,
) -> Result<(), HRESULT> {
    let mut stack: [T; 8];
    let mut heap = Vec::new();
    let host = if guest.len() <= 8 {
        stack = std::array::from_fn(|_| T::default());
        &mut stack[..guest.len()]
    } else {
        heap.try_reserve_exact(guest.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        heap.resize_with(guest.len(), T::default);
        heap.as_mut_slice()
    };
    for (dst, src) in host.iter_mut().zip(guest) {
        *dst = translate(src, &mut context)?;
    }
    submit(host, context)
}

unsafe extern "system" fn get_type(list: &List) -> D3D12_COMMAND_LIST_TYPE {
    unsafe { list.native.GetType() }
}

unsafe extern "system" fn reset(
    list: &List,
    allocator: *mut c_void,
    initial: *mut c_void,
) -> HRESULT {
    let device = parent(&list.parent);
    let Some(allocator) = (unsafe { device::allocator(allocator, device) }) else {
        crate::diagnostics::log_line(format_args!("list reset: {allocator:p} is not a known allocator"));
        return E_INVALIDARG;
    };
    // SAFETY: the guest holds the initial pipeline through this reset.
    let initial = match unsafe { device::compute::pipeline(initial, device) } {
        Ok(pipeline) => pipeline,
        Err(error) => return error,
    };
    let mut state = recording(list);
    if matches!(state.phase, ListState::Open) {
        // Pooled titles reset a list they never closed (rebox closes and retries).
        // Close the host recording and discard it; the reset below replaces it.
        let _ = state.prefix.finish(&list.native);
        let closed = unsafe { list.native.Close() };
        static OPEN: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&OPEN, || format!(
            "list reset while recording: closed the host list first ({:?})", closed.err().map(|e| e.code())));
        state.phase = ListState::Closed;
    }
    let native = &list.native;
    let pipeline = initial.map_or(ptr::null_mut(), |p| device::compute::current(p).0.as_raw());
    let host_reset = |host: &ID3D12CommandAllocator| unsafe {
        (native.vtable().Reset)(native.as_raw(), host.as_raw(), pipeline)
    };
    let mut hr = host_reset(&allocator.native);
    if hr == E_INVALIDARG && state.error.is_err() {
        // A host Close that failed (an invalid call was recorded) leaves the
        // list unresettable forever; the title asserts on the Reset. Swap in
        // a fresh host list, already open on this allocator. Retrying on
        // overflow allocators first only made a new allocator per reset.
        hr = match unsafe { rebuild_native(list, allocator, pipeline) } {
            Ok(()) => S_OK,
            Err(error) => error.code(),
        };
        static REBUILT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = REBUILT.fetch_add(1, Ordering::Relaxed);
        if n < 4 || (n + 1).is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "rebuilt host list {list:p} after a failed Close ({:#010x}): {:#010x} ({} so far)",
                state.error.0, hr.0, n + 1
            ));
        }
    }
    if hr == E_INVALIDARG {
        // The allocator is bound to another recording list (legal on Xbox).
        for extra in device::overflow_allocators(allocator) {
            hr = host_reset(&extra);
            if hr != E_INVALIDARG {
                break;
            }
        }
        if hr == E_INVALIDARG {
            hr = match device::add_overflow_allocator(allocator, unsafe { native.GetType() }) {
                Ok(extra) => host_reset(&extra),
                Err(error) => error.code(),
            };
        }
    }
    if hr.is_ok() {
        state.phase = ListState::Open;
        state.resources = allocator.state.clone();
        state.timestamp_page = None;
        state.prefix = memory_wait::Prefix::default();
        state.scene_commands.clear();
        state.bindings = compute_bindings::Bindings::new(initial);
        if state.media.is_some() {
            state.media = Some(media::Recording::new(allocator));
        }
        state.stencil_slot = initial.and_then(|p| device::compute::current(p).1.stencil_slot);
        state.pipeline_pending = initial.is_some_and(|p| device::compute::current(p).2);
        state.clamp_budget = None;
        if let Some(targets) = &mut state.stencil_targets {
            targets.unbind();
        }
        state.dsv_bound = false;
        state.trace_target = None;
        state.trace_samples.clear();
        state.placed_reads.clear();
        state.placed_writes.clear();
        state.bound_placed_targets.clear();
        state.resource_heap = None;
        state.descriptor_arena = None;
        state.indexed_arenas.clear();
        state.scalar_descriptors.clear();
        state.sampled_heaps.clear();
        state.ring.clear();
        state.stalled_writes.clear();
        state.open_queries.clear();
        state.split_barriers.clear();
        state.error = S_OK;
    }
    if hr.is_err() {
        crate::diagnostics::log_line(format_args!(
            "list reset failed {:#010x}: list type {} phase {:?} error {:#010x}",
            hr.0, unsafe { native.GetType() }.0, mem::discriminant(&state.phase), state.error.0
        ));
    }
    hr
}

/// Replace a list's host command list with a new one, open on `allocator`.
/// The guest is inside Reset, so no other thread records on this list, and
/// the old host list never executed (its Close failed).
unsafe fn rebuild_native(list: &List, allocator: &device::Allocator, pipeline: *mut c_void) -> windows::core::Result<()> {
    let old = &list.native;
    let mut device: Option<ID3D12Device> = None;
    unsafe { old.GetDevice(&mut device)? };
    let device = device.ok_or(E_FAIL)?;
    let kind = unsafe { old.GetType() };
    let pso = unsafe { ID3D12PipelineState::from_raw_borrowed(&pipeline) };
    let fresh: ID3D12GraphicsCommandList = match unsafe { device.CreateCommandList(0, kind, &allocator.native, pso) } {
        Ok(fresh) => fresh,
        Err(_) => {
            let extra = device::add_overflow_allocator(allocator, kind)?;
            unsafe { device.CreateCommandList(0, kind, &extra, pso)? }
        }
    };
    let raytracing = if list.state.raytracing.is_some() { Some(fresh.cast::<ID3D12GraphicsCommandList4>()?) } else { None };
    // SAFETY: see above; these fields are only read by recording calls.
    let old = unsafe {
        drop(ptr::replace(ptr::addr_of!(list.state.extended).cast_mut(), OnceLock::new()));
        drop(ptr::replace(ptr::addr_of!(list.state.enhanced_barriers).cast_mut(), OnceLock::new()));
        drop(ptr::replace(ptr::addr_of!(list.state.mesh).cast_mut(), OnceLock::new()));
        drop(ptr::replace(ptr::addr_of!(list.state.raytracing).cast_mut(), raytracing));
        ptr::replace(ptr::addr_of!(list.native).cast_mut(), fresh)
    };
    retire(old);
    Ok(())
}

/// The replaced host list may still be executing its previous, successful
/// recording. Keep the last few alive instead of releasing them at once.
fn retire(old: ID3D12GraphicsCommandList) {
    struct Retired(#[allow(dead_code)] ID3D12GraphicsCommandList);
    // SAFETY: only held and released; D3D12 objects are free-threaded.
    unsafe impl Send for Retired {}
    static RETIRED: Mutex<std::collections::VecDeque<Retired>> = Mutex::new(std::collections::VecDeque::new());
    let mut retired = RETIRED.lock().unwrap_or_else(|e| e.into_inner());
    if retired.len() >= 64 {
        retired.pop_front();
    }
    retired.push_back(Retired(old));
}

unsafe extern "system" fn close(list: &List) -> HRESULT {
    let _cost = crate::diagnostics::cost::timer(crate::diagnostics::cost::Bucket::Close);
    let mut state = recording(list);
    state.forgive();
    match state.phase {
        ListState::Closed => return E_FAIL,
        ListState::Fresh(..) => {} // The initial empty recording is already closed on the host.
        ListState::Open => {
            if let Err(error) = state.prefix.finish(&list.native) { state.error = error.code(); }
            state.balance(&list.native);
            if let Err(error) = unsafe { list.native.Close() } {
                static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = FAILED.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 8 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "host list Close failed {:#010x} (recorded error {:#010x}, list type {}, last guest callers {:x?}) ({n} so far)",
                        error.code().0, state.error.0, unsafe { list.native.GetType() }.0, state.ring
                    ));
                }
                state.error = error.code();
            }
        }
    }
    state.phase = ListState::Closed;
    if state.error.is_err() && crate::diagnostics::display::trace_views() {
        crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
            "graphics command list Close rejected {:#010x} (list {list:p}, guest caller {:#x})",
            state.error.0, crate::survey::title_rva().unwrap_or(0),
        ));
    }
    state.error
}

// The COM pointer in a barrier or copy location is borrowed by the desktop
// call. Raw fields allow us to replace it without taking an owned reference.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct TransitionBarrier {
    kind: D3D12_RESOURCE_BARRIER_TYPE,
    flags: D3D12_RESOURCE_BARRIER_FLAGS,
    resource: *mut c_void,
    subresource: u32,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
}

// Both members start with the guest tag and flags. Aliasing barriers carry
// two independently optional resources, with no change of resource state.
#[repr(C)]
#[derive(Clone, Copy)]
struct AliasingBarrier {
    kind: D3D12_RESOURCE_BARRIER_TYPE,
    flags: D3D12_RESOURCE_BARRIER_FLAGS,
    before: *mut c_void,
    after: *mut c_void,
}

#[repr(C)]
union RecordedBarrier {
    kind: D3D12_RESOURCE_BARRIER_TYPE,
    transition: TransitionBarrier,
    aliasing: AliasingBarrier,
}

impl Default for RecordedBarrier {
    fn default() -> Self {
        Self {
            transition: TransitionBarrier::default(),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CopyLocation {
    resource: *mut c_void,
    kind: D3D12_TEXTURE_COPY_TYPE,
    data: D3D12_TEXTURE_COPY_LOCATION_0,
}

const _: () = {
    assert!(size_of::<RecordedBarrier>() == size_of::<D3D12_RESOURCE_BARRIER>());
    assert!(
        mem::offset_of!(AliasingBarrier, before)
            == mem::offset_of!(D3D12_RESOURCE_BARRIER, Anonymous)
    );
    assert!(
        mem::offset_of!(AliasingBarrier, after)
            == mem::offset_of!(AliasingBarrier, before)
                + mem::offset_of!(D3D12_RESOURCE_ALIASING_BARRIER, pResourceAfter)
    );
    assert!(size_of::<TransitionBarrier>() == size_of::<D3D12_RESOURCE_BARRIER>());
    assert!(align_of::<TransitionBarrier>() == align_of::<D3D12_RESOURCE_BARRIER>());
    assert!(
        mem::offset_of!(TransitionBarrier, resource)
            == mem::offset_of!(D3D12_RESOURCE_BARRIER, Anonymous)
    );
    assert!(size_of::<CopyLocation>() == size_of::<D3D12_TEXTURE_COPY_LOCATION>());
    assert!(align_of::<CopyLocation>() == align_of::<D3D12_TEXTURE_COPY_LOCATION>());
    assert!(
        mem::offset_of!(CopyLocation, data)
            == mem::offset_of!(D3D12_TEXTURE_COPY_LOCATION, Anonymous)
    );
    assert!(size_of::<CopyLocation>() == 48 && mem::offset_of!(CopyLocation, data) == 16);
};

unsafe extern "system" fn transition(
    list: &List,
    count: u32,
    barriers: *const D3D12_RESOURCE_BARRIER,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) || count == 0 {
        return;
    }
    if barriers.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    // The guest supplies count readable barriers. Read a transition payload
    // only after checking the union tag.
    let guest = unsafe { slice::from_raw_parts(barriers, count as usize) };
    let kind = unsafe { list.native.GetType() };
    let mut splits: Vec<TransitionBarrier> = Vec::new();
    let result = translate_batch(
        guest,
        Vec::new(),
        |barrier, handoffs| {
            if barrier.Type == D3D12_RESOURCE_BARRIER_TYPE_ALIASING
                && barrier.Flags == D3D12_RESOURCE_BARRIER_FLAG_NONE
            {
                let alias = unsafe { &barrier.Anonymous.Aliasing };
                let native = |guest: &Option<ID3D12Resource>| match guest {
                    Some(object) => {
                        unsafe { device::resource(object.as_raw(), parent(&list.parent)) }
                            .map(Interface::as_raw)
                            .ok_or(E_INVALIDARG)
                    }
                    None => Ok(ptr::null_mut()),
                };
                return Ok(RecordedBarrier {
                    aliasing: AliasingBarrier {
                        kind: barrier.Type,
                        flags: barrier.Flags,
                        before: native(&alias.pResourceBefore)?,
                        after: native(&alias.pResourceAfter)?,
                    },
                });
            }
            if barrier.Type == D3D12_RESOURCE_BARRIER_TYPE_UAV
                && barrier.Flags == D3D12_RESOURCE_BARRIER_FLAG_NONE
            {
                // A null UAV orders all UAV accesses. Only the first pointer of
                // this union member is initialized by the guest.
                let guest = unsafe { &barrier.Anonymous.UAV.pResource };
                let resource = match guest.as_ref() {
                    Some(object) => {
                        let r = unsafe {
                            device::resource_proxy(object.as_raw(), parent(&list.parent))
                        }
                        .ok_or(E_INVALIDARG)?;
                        if device::blas::is_structure(r) {
                            // A guest AS pool may contain individually relocated structures.
                            // Order their host writes even when the barrier was recorded first.
                            ptr::null_mut()
                        } else {
                            r.state
                                .depth()
                                .map_or(&r.native, |pair| &pair.image)
                                .as_raw()
                        }
                    }
                    None => ptr::null_mut(),
                };
                return Ok(RecordedBarrier {
                    transition: TransitionBarrier {
                        kind: barrier.Type,
                        flags: barrier.Flags,
                        resource,
                        ..Default::default()
                    },
                });
            }
            if barrier.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION
                || !matches!(
                    barrier.Flags,
                    D3D12_RESOURCE_BARRIER_FLAG_NONE
                        | D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY
                        | D3D12_RESOURCE_BARRIER_FLAG_END_ONLY
                )
            {
                crate::diagnostics::recording_error(format_args!(
                    "resource barrier type {} flags {:#x}",
                    barrier.Type.0, barrier.Flags.0
                ));
                return Err(E_NOTIMPL);
            }
            let mut host = unsafe { ptr::from_ref(barrier).cast::<TransitionBarrier>().read() };
            let resource = unsafe { device::resource_proxy(host.resource, parent(&list.parent)) }
                .ok_or(E_INVALIDARG)?;
            let different = host.before != host.after;
            let needs_dimension = (host.before.0 | host.after.0) & device::BUFFER_READ_BITS != 0;
            let texture = needs_dimension
                && unsafe { resource.native.GetDesc().Dimension }
                    != D3D12_RESOURCE_DIMENSION_BUFFER;
            // TUXBOX_LIST_STATE_FIX=0 records the states unrestricted.
            let restrict = crate::env_toggle!("TUXBOX_LIST_STATE_FIX") != Some("0");
            let fix = |state| if restrict { list_state(state, kind) } else { state };
            host.before = fix(device::host_state(host.before, texture));
            host.after = fix(device::host_state(host.after, texture));
            if resource.state.unbacked_uav() {
                // The host depth image has no unordered access; see unbacked_uav.
                for state in [&mut host.before, &mut host.after] {
                    if *state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS {
                        *state = D3D12_RESOURCE_STATE_COMMON;
                    }
                }
            }
            if let Some(pair) = resource.state.depth()
                && (host.before == D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                    || host.after == D3D12_RESOURCE_STATE_UNORDERED_ACCESS)
            {
                if !matches!(
                    host.subresource,
                    0 | 1 | D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES
                ) {
                    return Err(E_INVALIDARG);
                }
                if host.flags != D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY && different {
                    handoffs.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    let index = unsafe { ptr::from_ref(barrier).offset_from(barriers) } as usize;
                    handoffs.push((index, &resource.native, pair));
                }
                host.resource = ptr::null_mut();
            } else if let Some(placed) = resource.state.placed() {
                if !placement::supported_state(host.before, placed.flags())
                    || !placement::supported_state(host.after, placed.flags())
                {
                    crate::diagnostics::recording_error(format_args!(
                        "placed texture transition {:#x} to {:#x}",
                        host.before.0, host.after.0
                    ));
                    return Err(E_NOTIMPL);
                }
                // Read promotions can accumulate implicitly. An explicit barrier
                // orders writes against other uses within the same batch.
                host.resource = if different
                    && [host.before, host.after].iter().any(|state|
                        matches!(*state, D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_RENDER_TARGET))
                {
                    resource.native.as_raw()
                } else {
                    ptr::null_mut()
                };
            } else if host.before == host.after {
                // The core runtime refuses a transition between equal states.
                // UA to UA orders UAV accesses; a split half has nothing to do.
                if host.before == D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                    && host.flags == D3D12_RESOURCE_BARRIER_FLAG_NONE
                {
                    return Ok(RecordedBarrier {
                        transition: TransitionBarrier {
                            kind: D3D12_RESOURCE_BARRIER_TYPE_UAV,
                            flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
                            resource: resource.native.as_raw(),
                            ..Default::default()
                        },
                    });
                }
                host.resource = ptr::null_mut();
            } else {
                host.resource = resource.native.as_raw();
            }
            Ok(RecordedBarrier { transition: host })
        },
        |host, handoffs| unsafe {
            let mut handoffs = handoffs.into_iter().peekable();
            let mut first = 0;
            let submit = |run: &[RecordedBarrier]| {
                if !run.is_empty() {
                    (list.native.vtable().ResourceBarrier)(
                        list.native.as_raw(),
                        run.len() as u32,
                        run.as_ptr().cast(),
                    );
                }
            };
            for (index, barrier) in host.iter().enumerate() {
                if barrier.kind == D3D12_RESOURCE_BARRIER_TYPE_TRANSITION
                    && barrier.transition.resource.is_null()
                {
                    submit(&host[first..index]);
                    if let Some((_, resource, pair)) = handoffs.next_if(|(at, _, _)| *at == index) {
                        depth::handoff(&list.native, resource, pair, &barrier.transition);
                    }
                    first = index + 1;
                }
            }
            submit(&host[first..]);
            for barrier in host {
                let t = barrier.transition;
                if barrier.kind == D3D12_RESOURCE_BARRIER_TYPE_TRANSITION && !t.resource.is_null()
                    && t.flags != D3D12_RESOURCE_BARRIER_FLAG_NONE
                {
                    splits.push(t);
                }
            }
            Ok(())
        },
    );
    if let Err(error) = result {
        state.error = error;
    }
    for t in splits {
        let same = |b: &TransitionBarrier| b.resource == t.resource && b.subresource == t.subresource;
        if t.flags == D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY {
            state.split_barriers.push(t);
        } else if let Some(at) = state.split_barriers.iter().position(same) {
            state.split_barriers.swap_remove(at);
        }
    }
}

/// Legacy resource state bits the desktop runtime knows, excluding the
/// video-queue states (no graphics, compute or copy list accepts those).
const LIST_STATES: i32 = 0x3fff
    | D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE.0
    | D3D12_RESOURCE_STATE_SHADING_RATE_SOURCE.0;
const WRITE_STATES: i32 = D3D12_RESOURCE_STATE_RENDER_TARGET.0
    | D3D12_RESOURCE_STATE_UNORDERED_ACCESS.0
    | D3D12_RESOURCE_STATE_DEPTH_WRITE.0
    | D3D12_RESOURCE_STATE_STREAM_OUT.0
    | D3D12_RESOURCE_STATE_COPY_DEST.0
    | D3D12_RESOURCE_STATE_RESOLVE_DEST.0;
const COMPUTE_STATES: i32 = D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER.0
    | D3D12_RESOURCE_STATE_UNORDERED_ACCESS.0
    | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0
    | D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT.0
    | D3D12_RESOURCE_STATE_COPY_DEST.0
    | D3D12_RESOURCE_STATE_COPY_SOURCE.0
    | D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE.0;

/// Restrict a barrier state to what the core runtime accepts on this list
/// type. An invalid state makes the host Close fail with E_INVALIDARG and
/// drops the whole list. Compute lists take shader reads as non-pixel reads
/// and copy lists only copy states; one write state excludes all others.
fn list_state(state: D3D12_RESOURCE_STATES, kind: D3D12_COMMAND_LIST_TYPE) -> D3D12_RESOURCE_STATES {
    let mut bits = state.0 & LIST_STATES;
    match kind {
        D3D12_COMMAND_LIST_TYPE_COMPUTE => {
            if bits & (D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_DEPTH_READ.0) != 0 {
                bits |= D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0;
            }
            bits &= COMPUTE_STATES;
        }
        D3D12_COMMAND_LIST_TYPE_COPY => {
            bits &= D3D12_RESOURCE_STATE_COPY_DEST.0 | D3D12_RESOURCE_STATE_COPY_SOURCE.0;
        }
        _ => {}
    }
    let writes = bits & WRITE_STATES;
    if writes != 0 && bits != writes & writes.wrapping_neg() {
        bits = writes & writes.wrapping_neg();
    }
    if bits != state.0 {
        static SEEN: Mutex<Vec<(i32, i32)>> = Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() < 16 && !seen.contains(&(kind.0, state.0)) {
            seen.push((kind.0, state.0));
            crate::diagnostics::log_line(format_args!(
                "barrier state {:#x} on a type {} list recorded as {bits:#x}", state.0, kind.0
            ));
        }
    }
    D3D12_RESOURCE_STATES(bits)
}

fn enhanced_barriers(native: &ID3D12GraphicsCommandList) -> Option<ID3D12GraphicsCommandList7> {
    if !matches!(
        unsafe { native.GetType() },
        D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_COMPUTE
    ) {
        return None;
    }
    let mut device: Option<ID3D12Device> = None;
    unsafe { native.GetDevice(&mut device) }.ok()?;
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
    unsafe {
        device?.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS12,
            ptr::from_mut(&mut options).cast(),
            size_of_val(&options) as u32,
        )
    }
    .ok()?;
    if options.EnhancedBarriersSupported.as_bool() {
        native.cast().ok()
    } else {
        None
    }
}

unsafe extern "system" fn flush_texture_cache(list: &List, mask: u32, address: u64, bytes: u64) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    // The mask selects the caches to flush. Both can use the same host
    // global memory barrier, which also covers subranges and empty ranges.
    if !matches!(mask, 0xc0_0000 | 0x3000) && xargs_strict() {
        crate::diagnostics::recording_error(format_args!("texture cache flush mask {mask:#x}"));
        state.error = E_NOTIMPL;
        return;
    }
    if !matches!(mask, 0xc0_0000 | 0x3000) {
        // A global barrier covers every cache (rebox and xcl ignore the mask).
        static OTHER: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&OTHER, || format!(
            "texture cache flush mask {mask:#x} recorded as a global barrier"));
    }
    if address.checked_add(bytes).is_none() {
        crate::diagnostics::recording_error(format_args!(
            "overflowing texture cache flush address {address:#x} bytes {bytes:#x}"
        ));
        state.error = E_INVALIDARG;
        return;
    }
    if address != 0 || bytes != 0xff_ffff_ff00 {
        static RANGE_FLUSH: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&RANGE_FLUSH, || {
            format!("texture cache range flush mask {mask:#x} address {address:#x} bytes {bytes:#x}")
        });
    }
    global_cache_barrier(list, &mut state);
}

// The extended list accepts separate cache/engine masks before the VA range.
// A host global barrier covers the full range and every participating stage.
unsafe extern "system" fn extended_cache_barrier(
    list: &List, _cache: u32, _engines: u32, address: u64, bytes: u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if address.checked_add(bytes).is_none() {
        state.error = E_INVALIDARG;
        return;
    }
    global_cache_barrier(list, &mut state);
}

fn global_cache_barrier(list: &List, state: &mut Recording) {
    let Some(native) = list
        .state
        .enhanced_barriers
        .get_or_init(|| enhanced_barriers(&list.native))
    else {
        crate::diagnostics::recording_error(format_args!("enhanced global barriers unavailable"));
        state.error = E_NOTIMPL;
        return;
    };
    let barrier = D3D12_GLOBAL_BARRIER {
        SyncBefore: D3D12_BARRIER_SYNC_ALL,
        SyncAfter: D3D12_BARRIER_SYNC_ALL,
        AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
        AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
    };
    let group = D3D12_BARRIER_GROUP {
        Type: D3D12_BARRIER_TYPE_GLOBAL,
        NumBarriers: 1,
        Anonymous: D3D12_BARRIER_GROUP_0 {
            pGlobalBarriers: &barrier,
        },
    };
    // SAFETY: the native open list consumes the stack-backed group during Barrier.
    unsafe { native.Barrier(&[group]) };
}

unsafe extern "system" fn clear_rtv(
    list: &List,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    color: *const f32,
    count: u32,
    rects: *const RECT,
) {
    let mut state = recording(list);
    if !state.can_record_graphics(&list.native) {
        return;
    }
    if handle.ptr == 0 || color.is_null() || (count != 0 && rects.is_null()) {
        state.error = E_INVALIDARG;
        return;
    }
    crate::diagnostics::display::clear(handle.ptr);
    if let Some(target) = unsafe { placement::render_target(parent(&list.parent), handle.ptr) }
        && let Err(error) = Recording::track_texture(&mut state.placed_writes, &target)
    {
        state.error = error;
        return;
    }
    unsafe {
        (list.native.vtable().ClearRenderTargetView)(
            list.native.as_raw(),
            handle,
            color,
            count,
            rects,
        )
    };
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "recorded render target clear list={list:p} rtv={:#x} rgba={:?}", handle.ptr, unsafe { &*color.cast::<[f32;4]>() }
    ));
}

// HandleDiscardResources at Frosty RVA 0x5787b84 supplies the native region
// layout (rect count and pointer, first subresource and subresource count).
unsafe extern "system" fn discard(
    list: &List,
    object: *mut c_void,
    region: *const D3D12_DISCARD_REGION,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let Some(resource) = (unsafe { device::resource_proxy(object, parent(&list.parent)) }) else {
        state.error = E_INVALIDARG;
        return;
    };
    if (unsafe { region.as_ref() }).is_some_and(|r| r.NumRects != 0 && r.pRects.is_null()) {
        state.error = E_INVALIDARG;
        return;
    }
    // CPU-backed read-only textures acquire their contents at submission.
    // Keeping the old bytes of a discarded read-only region is permitted.
    if resource.state.placed().is_none() && resource.state.buffer_window().is_none() {
        unsafe {
            (list.native.vtable().DiscardResource)(
                list.native.as_raw(),
                resource.native.as_raw(),
                region,
            )
        };
    }
}

unsafe fn copy_location<'a>(
    location: *const CopyLocation,
    device: *mut c_void,
) -> Option<(CopyLocation, Option<&'a Arc<Texture>>)> {
    let location = unsafe { location.as_ref() }?;
    if !matches!(
        location.kind,
        D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX | D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT
    ) {
        return None;
    }
    let resource = unsafe { device::resource_proxy(location.resource, device) }?;
    let mut host = CopyLocation { resource: resource.native.as_raw(), ..*location };
    if let Some((base, length)) = resource.state.buffer_window() {
        if host.kind != D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT { return None; }
        let placed = unsafe { &mut host.data.PlacedFootprint };
        let f = &placed.Footprint;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: if f.Depth > 1 { D3D12_RESOURCE_DIMENSION_TEXTURE3D } else { D3D12_RESOURCE_DIMENSION_TEXTURE2D },
            Width: f.Width.into(), Height: f.Height, DepthOrArraySize: f.Depth.try_into().ok()?,
            MipLevels: 1, Format: f.Format, SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let mut owner: Option<ID3D12Device> = None;
        let (mut rows, mut row_bytes) = (0, 0);
        unsafe {
            resource.native.GetDevice(&mut owner).ok()?;
            owner?.GetCopyableFootprints(&desc, 0, 1, 0, None, Some(&mut rows), Some(&mut row_bytes), None);
        }
        let pitch = u64::from(f.RowPitch);
        let lines = u64::from(rows).checked_mul(u64::from(f.Depth))?;
        let bytes = lines.checked_sub(1)?.checked_mul(pitch)?.checked_add(row_bytes)?;
        if row_bytes > pitch || placed.Offset.checked_add(bytes)? > length { return None; }
        placed.Offset = placed.Offset.checked_add(base)?;
    }
    Some((host, resource.state.placed()))
}

unsafe extern "system" fn copy_texture(
    list: &List,
    dst: *const CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const CopyLocation,
    region: *const D3D12_BOX,
    flags: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if flags != 0 {
        crate::diagnostics::recording_error(format_args!("CopyTextureRegion flags {flags:#x}"));
        state.error = E_NOTIMPL;
        return;
    }
    let device = parent(&list.parent);
    let (Some((dst, destination)), Some((src, source))) =
        (unsafe { copy_location(dst, device) }, unsafe {
            copy_location(src, device)
        })
    else {
        state.error = E_INVALIDARG;
        return;
    };
    if let Err(error) = state.prepare_copy(destination, source) {
        state.error = error;
        return;
    }
    crate::diagnostics::display::copy(dst.resource);
    unsafe {
        (list.native.vtable().CopyTextureRegion)(
            list.native.as_raw(),
            ptr::from_ref(&dst).cast(),
            x,
            y,
            z,
            ptr::from_ref(&src).cast(),
            region,
        );
    }
}

// Frosty's pool allocator and its buffer and texture copy routines all use
// slot 84 with destination, source and zero options.
unsafe extern "system" fn copy_resource(
    list: &List,
    dst: *mut c_void,
    src: *mut c_void,
    options: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if options != 0 {
        crate::diagnostics::recording_error(format_args!(
            "slot 84 CopyResource options {options:#x}"
        ));
        state.error = E_NOTIMPL;
        return;
    }
    let owner = parent(&list.parent);
    // SAFETY: each guest argument is a live COM object or null. Only resource
    // proxies owned by the list's device yield borrowed native interfaces.
    let (Some(dst), Some(src)) = (unsafe { device::resource_proxy(dst, owner) }, unsafe {
        device::resource_proxy(src, owner)
    }) else {
        state.error = E_INVALIDARG;
        return;
    };
    crate::diagnostics::display::copy(dst.native.as_raw());
    let (d, s) = unsafe { (dst.native.GetDesc(), src.native.GetDesc()) };
    if dst.state.buffer_window().is_some() || src.state.buffer_window().is_some() {
        let (to, size) = dst.state.buffer_window().unwrap_or((0, d.Width));
        let (from, other) = src.state.buffer_window().unwrap_or((0, s.Width));
        if d.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || s.Dimension != d.Dimension || size != other
            || (dst.native == src.native && to < from + size && from < to + size) {
            state.error = E_INVALIDARG;
            return;
        }
        unsafe { list.native.CopyBufferRegion(&dst.native, to, &src.native, from, size) };
        return;
    }
    if dst.native == src.native
        || d.Dimension != s.Dimension
        || d.DepthOrArraySize != s.DepthOrArraySize
        || d.MipLevels != s.MipLevels
        || d.SampleDesc != s.SampleDesc
        // D3D12 also allows reinterpret copies between some differently sized
        // block-compressed and uncompressed textures. Leave those to the driver.
        || (d.Format == s.Format && (d.Width != s.Width || d.Height != s.Height))
    {
        state.error = E_INVALIDARG;
        return;
    }
    if let Err(error) = state.prepare_copy(dst.state.placed(), src.state.placed()) {
        state.error = error;
        return;
    }
    // SAFETY: both native resources belong to this open command list's device.
    // The guest supplies their copy states via barriers and owns their lifetimes.
    unsafe { list.native.CopyResource(&dst.native, &src.native) };
    static FIRST_COPY: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST_COPY, || {
        format!(
            "recorded slot 84 CopyResource (dimension {}, {} x {} x {}, {} mips)",
            s.Dimension.0, s.Width, s.Height, s.DepthOrArraySize, s.MipLevels
        )
    });
}

// Frosty's RHIDispatchIndirectComputeShader calls slot 59 (RVA 0x57876df).
unsafe extern "system" fn execute_indirect(
    list: &List,
    signature: *mut c_void,
    max_count: u32,
    arguments: *mut c_void,
    argument_offset: u64,
    count_buffer: *mut c_void,
    count_offset: u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let owner = parent(&list.parent);
    // Each object is borrowed from a live guest reference for this call.
    let (Some(signature), Some(arguments)) = (
        unsafe { device::indirect::signature(signature, owner) },
        unsafe { device::resource_proxy(arguments, owner) },
    ) else {
        state.error = E_INVALIDARG;
        return;
    };
    let bytes = u64::from(signature.state.stride) * u64::from(max_count);
    let Some(argument_offset) = compute::buffer_offset(arguments, argument_offset, bytes) else {
        state.error = E_INVALIDARG;
        return;
    };
    let (count, count_offset) = if count_buffer.is_null() { (None, count_offset) } else {
        let Some(buffer) = (unsafe { device::resource_proxy(count_buffer, owner) }) else { state.error = E_INVALIDARG; return; };
        let Some(offset) = compute::buffer_offset(buffer, count_offset, 4) else { state.error = E_INVALIDARG; return; };
        (Some(&buffer.native), offset)
    };
    if !(argument_offset | count_offset).is_multiple_of(4) { state.error = E_INVALIDARG; return; }
    if max_count != 0 && !state.pipeline_pending && clamp_enabled()
        && let Some(host) = list.state.raytracing.as_ref()
    {
        let clamped = indirect_clamp::bound(
            &list.native, &mut state.clamp_budget, signature.state.kind, signature.state.stride, max_count,
            indirect_clamp::Region { buffer: &arguments.native, offset: argument_offset },
            count.map(|buffer| indirect_clamp::Region { buffer, offset: count_offset }),
        );
        match clamped {
            Ok((ran, made)) => {
                if let Some(made) = made
                    && let Err(error) = raytracing::retain(&state, &[&made])
                {
                    state.error = error.code();
                    return;
                }
                // The clamp replaced the compute root and the pipeline.
                if ran { unsafe { state.bindings.restore(host) }; }
            }
            Err(error) => { state.error = error.code(); return; }
        }
    }
    let execute = || unsafe {
        list.native.ExecuteIndirect(
            &signature.native,
            max_count,
            &arguments.native,
            argument_offset,
            count,
            count_offset,
        );
        static FIRST_DRAW: std::sync::Once = std::sync::Once::new();
        static FIRST_COMPUTE: std::sync::Once = std::sync::Once::new();
        let site = if signature.state.kind == D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH { &FIRST_COMPUTE } else { &FIRST_DRAW };
        crate::diagnostics::log_once(site, || format!(
            "recorded ExecuteIndirect list={list:p} kind={} max_count={max_count}", signature.state.kind.0
        ));
    };
    // Ray shader records can carry local descriptor tables.
    if signature.state.kind == D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS {
        state.sample_heap();
    }
    if signature.state.kind == D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH {
        state.compute_dispatch(list, execute);
    } else {
        let rasterizes = max_count != 0 && signature.state.kind != D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_RAYS;
        state.draw(list, rasterizes, execute);
    }
}

/// TUXBOX_INDIRECT_CLAMP=0 executes indirect arguments as the title wrote them.
fn clamp_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("TUXBOX_INDIRECT_CLAMP").is_none_or(|v| v != "0"))
}

// Record the title's four-byte progress marker at the requested GPU address.
unsafe extern "system" fn write_marker(list: &List, address: u64, value: u32, flags: u32, reserved: u32) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) { return; }
    if address == 0 || !address.is_multiple_of(4) {
        state.error = E_INVALIDARG;
        return;
    }
    if (flags != 0 || reserved != 0) && xargs_strict() {
        state.error = E_INVALIDARG;
        return;
    }
    if flags != 0 || reserved != 0 {
        // Cache-flush and engine selectors; the host write is ordered anyway
        // (leonida-ce ignores them too).
        static FLAGS: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FLAGS, || format!("marker write flags {flags:#x} {reserved:#x} ignored"));
    }
    memory_wait::note_write(list, &mut state, "marker write", address, u64::from(value));
    let result = (|| -> windows::core::Result<()> {
        let (resource, dest) = unsafe { device::command_buffer_address(parent(&list.parent), address, 4)? };
        if let Some(buffer) = &resource { raytracing::retain(&state, &[buffer])?; }
        let write = D3D12_WRITEBUFFERIMMEDIATE_PARAMETER { Dest: dest, Value: value };
        state.prefix.push(&list.native, memory_wait::Packet::Marker(write))
    })();
    if let Err(error) = result { state.error = error.code(); }
}

/// List slot 62 on the newer console layout (60 on the older one):
/// CopyMemoryToMemoryX(destination, source, size, flags), a GPU copy between
/// two title GPU addresses. Both translate to imported host buffers; buffers
/// promote from COMMON to COPY_SOURCE / COPY_DEST implicitly.
unsafe extern "system" fn copy_memory(list: &List, destination: u64, source: u64, size: u64, flags: u32) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) { return; }
    static WARN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let warn = |what: std::fmt::Arguments| if WARN.fetch_add(1, Ordering::Relaxed) < 16 {
        crate::diagnostics::log_line(format_args!("CopyMemoryToMemoryX {source:#x} -> {destination:#x} ({size:#x} bytes): {what}"));
    };
    if size == 0 || destination == 0 || source == 0 {
        return warn(format_args!("empty, skipped"));
    }
    if flags != 0 {
        static FLAGS: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FLAGS, || format!("CopyMemoryToMemoryX flags {flags:#x} ignored"));
    }
    if destination < source.saturating_add(size) && source < destination.saturating_add(size) {
        return warn(format_args!("overlapping ranges, skipped"));
    }
    memory_wait::note_write(list, &mut state, "memory copy", destination, size);
    let owner = parent(&list.parent);
    let result = (|| -> windows::core::Result<()> {
        let (Some(dst), dst_va) = (unsafe { device::command_buffer_address(owner, destination, size)? }) else {
            warn(format_args!("destination is not GPU memory, skipped"));
            return Ok(());
        };
        let (Some(src), src_va) = (unsafe { device::command_buffer_address(owner, source, size)? }) else {
            warn(format_args!("source is not GPU memory, skipped"));
            return Ok(());
        };
        raytracing::retain(&state, &[&dst, &src])?;
        // SAFETY: both buffers hold the translated ranges and are retained by the list.
        unsafe {
            let dst_offset = dst_va - dst.GetGPUVirtualAddress();
            let src_offset = src_va - src.GetGPUVirtualAddress();
            list.native.CopyBufferRegion(&dst, dst_offset, &src, src_offset, size);
        }
        Ok(())
    })();
    if let Err(error) = result { state.error = error.code(); }
}

static LIST_SLOTS: Slots<149> = {
    let mut slots = child_slots::<ID3D12GraphicsCommandList, ListData, 149>();
    slots[8] = crate::slot!(get_type);
    slots[9] = crate::slot!(close);
    slots[10] = crate::slot!(reset);
    slots[12] = crate::slot!(graphics::draw);
    slots[13] = crate::slot!(graphics::draw_indexed);
    slots[14] = crate::slot!(compute::dispatch);
    slots[15] = crate::slot!(compute::copy_buffer);
    slots[20] = crate::slot!(graphics::topology);
    slots[21] = crate::slot!(graphics::viewports);
    slots[22] = crate::slot!(graphics::scissors);
    slots[23] = crate::slot!(graphics::blend_factor);
    slots[24] = crate::slot!(graphics::stencil_ref);
    slots[25] = crate::slot!(graphics::pipeline);
    slots[26] = crate::slot!(transition);
    slots[28] = crate::slot!(queries::query::<true>);
    slots[29] = crate::slot!(queries::query::<false>);
    slots[30] = crate::slot!(queries::resolve);
    // The Xbox root binding slots after ResourceBarrier are shifted by four.
    slots[32] = crate::slot!(descriptors::bind_heaps);
    slots[33] = crate::slot!(compute::bind_root);
    slots[34] = crate::slot!(graphics::root);
    slots[35] = crate::slot!(descriptors::table::<true>);
    slots[36] = crate::slot!(descriptors::table::<false>);
    slots[37] = crate::slot!(compute::one_constant::<true>);
    slots[38] = crate::slot!(compute::one_constant::<false>);
    slots[39] = crate::slot!(compute::constants::<true>);
    slots[40] = crate::slot!(compute::constants::<false>);
    slots[41] = crate::slot!(compute::constant_buffer::<true>);
    slots[42] = crate::slot!(compute::constant_buffer::<false>);
    slots[43] = crate::slot!(compute::shader_resource::<true>);
    slots[44] = crate::slot!(compute::shader_resource::<false>);
    slots[45] = crate::slot!(compute::unordered_access::<true>);
    slots[46] = crate::slot!(compute::unordered_access::<false>);
    slots[47] = crate::slot!(graphics::index_buffer);
    slots[48] = crate::slot!(graphics::vertex_buffers);
    slots[50] = crate::slot!(graphics::targets);
    slots[51] = crate::slot!(depth::clear);
    slots[52] = crate::slot!(clear_rtv);
    slots[53] = crate::slot!(depth::clear_uav::<false>);
    slots[54] = crate::slot!(depth::clear_uav::<true>);
    slots[55] = crate::slot!(discard);
    slots[59] = crate::slot!(execute_indirect);
    slots[67] = crate::slot!(flush_texture_cache);
    slots[68] = crate::slot!(graphics::depth_bounds);
    slots[71] = crate::slot!(write_marker);
    slots[72] = crate::slot!(queries::timestamp_at);
    slots[74] = crate::slot!(memory_wait::wait_value64);
    slots[83] = crate::slot!(copy_texture);
    slots[84] = crate::slot!(copy_resource);
    // GTA's texture uploader uses this entry for a placed footprint to a
    // subresource, with the same trailing box and copy flags as slot 83.
    slots[85] = crate::slot!(copy_texture);
    slots[96] = crate::slot!(compute::async_compute_budget);
    slots[117] = crate::slot!(build_raytracing::single);
    slots[118] = crate::slot!(raytracing::postbuild);
    slots[119] = crate::slot!(raytracing::copy);
    slots[120] = crate::slot!(ray_dispatch::pipeline);
    slots[121] = crate::slot!(ray_dispatch::dispatch);
    slots[126] = crate::slot!(graphics::shading_rate);
    slots[127] = crate::slot!(graphics::dispatch_mesh);
    slots[131] = crate::slot!(build_raytracing::batch);
    slots[139] = crate::slot!(reset_extended);
    slots[146] = crate::slot!(enhanced::barrier);
    slots
};

unsafe extern "system" fn reset_extended(list: &List, allocator: *mut c_void, initial: *mut c_void, options: usize) -> HRESULT {
    // The console options have no desktop equivalent; reset as plain Reset does
    // (leonida-ce does the same). Log the first few distinct values only.
    if options != 0 {
        use std::sync::{LazyLock, Mutex};
        static SEEN: LazyLock<Mutex<std::collections::BTreeSet<usize>>> = LazyLock::new(Default::default);
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() < 8 && seen.insert(options) {
            crate::diagnostics::log_once(&std::sync::Once::new(), || format!("ResetX options={options:#x} ignored"));
        }
    }
    unsafe { reset(list, allocator, initial) }
}

/// Slot 131 on the Device11 table: a batched acceleration-structure build
/// (count, entries) or the extended cache barrier (cache, engines, address,
/// bytes). A build's entry array is readable memory; the barrier's engine
/// mask is not a pointer to `count` readable entries.
unsafe extern "system" fn slot131(list: &List, a: u32, b: usize, c: u64, d: u64) {
    let entries = b as *const build_raytracing::Entry;
    let bytes = (a as usize).checked_mul(size_of::<build_raytracing::Entry>());
    // TUXBOX_SLOT131=build|barrier forces one form; unset tells them apart.
    let forced = crate::env_toggle!("TUXBOX_SLOT131");
    if forced == Some("build")
        || (forced != Some("barrier") && a != 0 && b > 0xffff && bytes.is_some_and(|n| accessible(b, n, false)))
    {
        unsafe { build_raytracing::batch(list, a, entries) };
        return;
    }
    static BARRIER: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&BARRIER, || format!(
        "list slot 131 recorded as cache barrier (cache {a:#x} engines {b:#x} address {c:#x} bytes {d:#x})"));
    unsafe { extended_cache_barrier(list, a, b as u32, c, d) };
}

// Device11 titles use the new raster-state slots even when requesting the
// base graphics-list IID. The List6-only cache barriers remain separate.
static DEVICE11_LIST_SLOTS: Slots<149> = {
    let mut slots = LIST_SLOTS;
    // GTA VI calls slot 131 on this table as the extended cache barrier
    // (cache 0xc280, engines 0x20000, address 0, bytes 0xffffffffff00 at
    // game+0x1560994); sending it to the batched AS build faulted reading
    // 0xc280 "entries" at 0x20000. Slot 131 tells the two forms apart.
    slots[131] = crate::slot!(slot131);
    slots[132] = crate::slot!(extended_cache_barrier);
    // GTA VI (test10, title RVA 0x9796a0): CopyMemoryToMemoryX, older #60.
    slots[62] = crate::slot!(copy_memory);
    slots[70] = crate::slot!(graphics::depth_bounds);
    slots[75] = crate::slot!(graphics::force_shader_depth_order);
    slots[142] = crate::slot!(graphics::sample_mask);
    slots
};

static EXTENDED_LIST_SLOTS: Slots<149> = {
    let mut slots = DEVICE11_LIST_SLOTS;
    slots[131] = crate::slot!(extended_cache_barrier);
    slots[132] = crate::slot!(extended_cache_barrier);
    slots
};

fn reject(queue: &Queue, error: HRESULT) {
    let _ = queue.state.placement.error.compare_exchange(
        S_OK.0,
        error.0,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    unsafe {
        OutputDebugStringA(s!(
            "Project TuxBox: direct queue rejected a guest command-list batch\n"
        ))
    };
}

/// A bad batch drops only itself: latching the error on the queue made every
/// later submission return early, so the title kept presenting black frames.
/// Device loss still latches, since nothing after it can run.
fn drop_batch(queue: &Queue, error: HRESULT) {
    use windows::Win32::Graphics::Dxgi::{DXGI_ERROR_DEVICE_HUNG, DXGI_ERROR_DEVICE_REMOVED, DXGI_ERROR_DEVICE_RESET};
    if matches!(error, DXGI_ERROR_DEVICE_REMOVED | DXGI_ERROR_DEVICE_HUNG | DXGI_ERROR_DEVICE_RESET) {
        reject(queue, error);
        return;
    }
    static DROPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = DROPPED.fetch_add(1, Ordering::Relaxed);
    if n < 4 || (n + 1).is_power_of_two() {
        crate::diagnostics::log_line(format_args!("dropped a command-list batch: {error:?} ({} so far)", n + 1));
    }
}

/// Committed memory of `len` bytes at `at`, readable (and writable if asked).
fn accessible(at: usize, len: usize, write: bool) -> bool {
    use windows::Win32::System::Memory::*;
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if at == 0 || unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } == 0 {
        return false;
    }
    let protect = region.Protect;
    let readable = PAGE_READONLY | PAGE_READWRITE | PAGE_WRITECOPY
        | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    let writable = PAGE_READWRITE | PAGE_WRITECOPY | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    region.State == MEM_COMMIT
        && (protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0
        && (protect & if write { writable } else { readable }).0 != 0
        && at.checked_add(len).is_some_and(|end| end <= region.BaseAddress as usize + region.RegionSize)
}

// Device11 queue slot 35. xcl and rebox call it ExecuteCommandListsX(count,
// lists, value, flags); palmetto (the GTA VI runtime) treats it as a clock
// calibration that writes two QPC ticks to the pointer in the second
// argument, and the title faults on stack garbage when they stay unwritten.
// Tell the two apart by whether the second argument holds our list proxies.
unsafe extern "system" fn execute_frame(
    queue: &Queue, count: u32, lists: *const *mut c_void, frame_token: u64, flags: u32,
) {
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "queue slot 35 first call: a1={count:#x} a2={lists:p} a3={frame_token:#x} a4={flags:#x}"));
    let device = parent(&queue.parent);
    // TUXBOX_QSLOT35=pair|exec forces one form; unset tells them apart.
    let forced = crate::env_toggle!("TUXBOX_QSLOT35");
    let first_is_list = forced != Some("pair") && (forced == Some("exec") || (1..=256).contains(&count)
        && accessible(lists as usize, count as usize * size_of::<*mut c_void>(), false)
        && {
            let item = unsafe { *lists };
            accessible(item as usize, size_of::<usize>(), false)
                && unsafe {
                    unwrap::<ID3D12GraphicsCommandList, ListData>(item, device, &LIST_SLOTS)
                        .or_else(|| unwrap(item, device, &DEVICE11_LIST_SLOTS))
                        .or_else(|| unwrap(item, device, &EXTENDED_LIST_SLOTS))
                }
                .is_some()
        });
    if !first_is_list {
        if accessible(lists as usize, 16, true) {
            let mut ticks = 0i64;
            let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut ticks) };
            let out = lists.cast_mut().cast::<i64>();
            unsafe { out.write_unaligned(ticks); out.add(1).write_unaligned(ticks); }
        }
        static CALIBRATION: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&CALIBRATION, || format!(
            "queue slot 35 treated as clock calibration (a1={count:#x} a2={lists:p})"));
        return;
    }
    // Both flags select how the console assembles its GPU packets. Bit 0
    // suppresses a hardware buffer switch and bit 1 chains indirect buffers.
    // The desktop queue owns its packet storage and takes the ordered list
    // array in one submission. The frame token labels that submission.
    if queue.state.kind != D3D12_COMMAND_LIST_TYPE_DIRECT || flags & !3 != 0 {
        crate::diagnostics::recording_error(format_args!("extended command-list submission flags {flags:#x}"));
        // Drop this batch only: latching the queue error dropped every
        // later submission too.
        drop_batch(queue, E_NOTIMPL);
        return;
    }
    unsafe { execute_batch(queue, count, lists, Some((frame_token, flags))) };
}

unsafe extern "system" fn execute(queue: &Queue, count: u32, lists: *const *mut c_void) {
    unsafe { execute_batch(queue, count, lists, None) };
}

unsafe fn execute_batch(queue: &Queue, count: u32, lists: *const *mut c_void, frame: Option<(u64,u32)>) {
    let _cost = crate::diagnostics::cost::timer(crate::diagnostics::cost::Bucket::Submit);
    let mut submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *submitted = true;
    if queue.state.placement.error.load(Ordering::Acquire) < 0 || count == 0 {
        return;
    }
    if lists.is_null() {
        reject(queue, E_INVALIDARG);
        return;
    }
    let guest = unsafe { slice::from_raw_parts(lists, count as usize) };
    let mut list_index = 0;
    let trace_samples = std::cell::RefCell::new(Vec::new());
    let result = translate_batch(
        guest,
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), memory_wait::Splits::new()),
        |&item, (textures, writes, shader_views, scene_lists, splits)| {
            let list: &List = unsafe {
                unwrap(item, parent(&queue.parent), &LIST_SLOTS)
                    .or_else(|| unwrap(item, parent(&queue.parent), &DEVICE11_LIST_SLOTS))
                    .or_else(|| unwrap(item, parent(&queue.parent), &EXTENDED_LIST_SLOTS))
                    .or_else(|| unwrap(item, parent(&queue.parent), &media::SLOTS))
            }
            .ok_or(E_INVALIDARG)?;
            let state = recording(list);
            if !state.stalled_writes.is_empty() {
                static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                if LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
                    crate::diagnostics::log_line(format_args!(
                        "list {list:p} writing {:x?} (pages of stalled GPU memory waits) submitted on queue {:p} type {} (thread {})",
                        state.stalled_writes, queue, queue.state.kind.0,
                        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }));
                }
            }
            if !matches!(state.phase, ListState::Closed)
                || state.error.is_err()
                || unsafe { list.native.GetType() } != queue.state.kind
            {
                return Err(E_INVALIDARG);
            }
            // A descriptor write after Close can exhaust the map's native
            // heap. Never submit an indexed access with an unpublished entry.
            for arena in &state.indexed_arenas {
                arena.check_index_capacity().map_err(|e| e.code())?;
            }
            for descriptor in &state.scalar_descriptors {
                let owner = list.parent.as_ref().ok_or(E_INVALIDARG)?;
                if !unsafe { device::is_constant_buffer(owner, descriptor.cpu) } {
                    crate::diagnostics::recording_error(format_args!(
                        "indexed shader scalar load requires a CBV at logical resource descriptor {}", descriptor.index
                    ));
                    return Err(E_INVALIDARG);
                }
            }
            splits.try_reserve(state.prefix.segments.len()).map_err(|_| E_OUTOFMEMORY)?;
            splits.extend(state.prefix.segments.iter().map(|s| (list_index, s.clone())));
            list_index += 1;
            if !state.scene_commands.is_empty() {
                scene_lists.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                scene_lists.push(list);
            }
            textures
                .try_reserve(state.placed_reads.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            textures.extend(state.placed_reads.iter().cloned());
            writes
                .try_reserve(state.placed_writes.len())
                .map_err(|_| E_OUTOFMEMORY)?;
            writes.extend(state.placed_writes.iter().cloned());
            shader_views.extend(state.sampled_heaps.iter().cloned());
            trace_samples.borrow_mut().extend(state.trace_samples.iter().cloned());
            Ok(list.native.as_raw())
        },
        |host, (mut textures, mut writes, shader_views, scene_lists, splits)| {
            let _site = crate::diagnostics::cost::site();
            unsafe {
                placement::submit(
                    parent(&queue.parent),
                    &queue.native,
                    &queue.state.placement,
                    &mut textures,
                    &mut writes,
                    &shader_views,
                    || {
                        // Placement dependencies are settled. Publish scene writes and
                        // snapshot each instance table in recorded order across lists.
                        let submission = if scene_lists.is_empty() {
                            None
                        } else {
                            let owner = queue.parent.as_ref().ok_or(E_INVALIDARG)?;
                            Some(device::blas::begin_submission(owner))
                        };
                        for list in scene_lists {
                            let state = recording(list);
                            for command in &state.scene_commands {
                                command.prepare(list)?;
                            }
                        }
                        if let Some(owner) = queue.parent.as_ref() {
                            let _site = crate::diagnostics::cost::site();
                            device::refresh_shadows(owner, &queue.native)?;
                            device::flush_desc_table(owner, &queue.native)?;
                        }
                        {
                            let _site = crate::diagnostics::cost::site();
                            memory_wait::submit(queue, host, splits)?;
                        }
                        if let Some(owner) = queue.parent.as_ref() {
                            let _site = crate::diagnostics::cost::site();
                            device::write_back_shadows(owner, &queue.native)?;
                        }
                        if !trace_samples.borrow().is_empty() {
                            let mut device: Option<ID3D12Device> = None;
                            queue.native.GetDevice(&mut device)?;
                            let device = device.ok_or(E_INVALIDARG)?;
                            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
                            queue.native.Signal(&fence, 1)?;
                            fence.SetEventOnCompletion(1, windows::Win32::Foundation::HANDLE::default())?;
                            for (label, sample) in trace_samples.borrow().iter() {
                                sample.report(label);
                            }
                        }
                        if let Some(submission) = submission {
                            submission.commit();
                        }
                        crate::diagnostics::submitted(queue.state.kind, count);
                        Ok(())
                    },
                )
            }
            .map_err(|error| error.code())
        },
    );
    if let Err(error) = result {
        drop_batch(queue, error);
    } else if let Some((token, flags)) = frame {
        static FIRST: [std::sync::Once;4] = [const { std::sync::Once::new() };4];
        crate::diagnostics::log_once(&FIRST[flags as usize], || format!(
            "submitted frame batch flags={flags:#x} token={token} count={count} first_lists={:?}",
            &guest[..guest.len().min(4)]
        ));
    }
}

unsafe extern "system" fn queue_fence<const WAIT: bool>(
    queue: &Queue,
    fence: *mut c_void,
    value: u64,
) -> HRESULT {
    let mut submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *submitted = true;
    let hr = HRESULT(queue.state.placement.error.load(Ordering::Acquire));
    if hr.is_err() {
        return hr;
    }
    let owner = parent(&queue.parent);
    let native;
    let fence = match unsafe { device::fence(fence, owner) } {
        Some(fence) => fence,
        None => {
            native = match unsafe { device::media::native_child::<ID3D12Fence>(fence, owner) } {
                Ok(fence) => fence,
                Err(error) => return error.code(),
            };
            &native
        }
    };
    let method = if WAIT {
        queue.native.vtable().Wait
    } else {
        queue.native.vtable().Signal
    };
    let status = unsafe { method(queue.native.as_raw(), fence.as_raw(), value) };
    if !WAIT && status.is_ok() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "queued guest fence signal value={value} queue_type={} fence={:p}", queue.state.kind.0, fence.as_raw()
        ));
    }
    status
}

// Frosty's direct/compute path requests its flush before the fence and the
// copy path passes zero. Native queue Signal orders completion of prior work.
unsafe extern "system" fn signal_x(
    queue: &Queue,
    fence: *mut c_void,
    value: u64,
    flush: u32,
) -> HRESULT {
    if !matches!(flush, 0 | 0x0800_0000) {
        return E_NOTIMPL;
    }
    unsafe { queue_fence::<false>(queue, fence, value) }
}

#[repr(C)]
struct PresentPlane {
    token: u64,
    resource_count: u32,
    padding: u32,
    resources: *const *mut c_void,
}

const _: () = assert!(mem::offset_of!(PresentPlane, resources) == 16);

// Frosty submits a 72-byte plane and a separate 32-byte parameter record
// at RVA 0x5764f32. Older TuxBox guests supply only the plane prefix.
#[repr(C)]
struct RetailPresentPlane {
    prefix: PresentPlane,
    source_rects: *const RECT,
    destination: *const [f32; 4],
    color_space: u32,
    remaining: [u32; 7],
}

#[repr(C)]
struct PresentParameters {
    immediate_threshold_percent: f32,
    options: [u32; 7],
}

const _: () = assert!(mem::size_of::<RetailPresentPlane>() == 72);
const _: () = assert!(mem::size_of::<PresentParameters>() == 32);

unsafe extern "system" fn present(
    queue: &Queue,
    count: u32,
    planes: *const PresentPlane,
    params: *const PresentParameters,
) -> HRESULT {
    let result = unsafe { present_planes(queue, count, planes, params) };
    if result.is_err() {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!("PresentX failed planes={count} HRESULT={:#010x}", result.0));
        if result == windows::Win32::Graphics::Dxgi::DXGI_ERROR_ACCESS_LOST {
            // The desktop window was closed. A console title has no such event
            // and would otherwise wait for this frame's flip until its own
            // watchdog fires. Treat the close as a request to quit.
            crate::diagnostics::log_once(&std::sync::Once::new(), || "presentation window closed; exiting title".into());
            // Compiled host pipelines and the used list, kept for the next run.
            device::pipeline_cache::save_now();
            use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            let _ = unsafe { TerminateProcess(GetCurrentProcess(), 0) };
        }
        // A rejected present still ends this frame for the title's pacing.
        if count >= 1 && let Some(plane) = unsafe { planes.as_ref() } {
            unsafe { device::frame_events::display_flip(parent(&queue.parent), plane.token) };
        }
    }
    result
}

unsafe fn present_planes(
    queue: &Queue, count: u32, planes: *const PresentPlane, params: *const PresentParameters,
) -> HRESULT {
    let mut submitted = queue
        .state
        .submission
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    *submitted = true;
    let hr = HRESULT(queue.state.placement.error.load(Ordering::Acquire));
    if hr.is_err() {
        return hr;
    }
    if queue.state.kind != D3D12_COMMAND_LIST_TYPE_DIRECT {
        return E_NOTIMPL;
    }
    if count == 0 || planes.is_null() {
        return E_INVALIDARG;
    }
    // Show plane 0 (rebox does the same); overlay planes and extra
    // resources are dropped with a log line.
    let plane = unsafe { &*planes };
    if plane.resource_count == 0 || plane.resources.is_null() {
        return E_INVALIDARG;
    }
    if count != 1 || plane.resource_count != 1 {
        static EXTRA: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&EXTRA, || format!(
            "present: showing plane 0 resource 0 of {count} planes, {} resources", plane.resource_count));
    }
    let Some(source) = (unsafe { device::resource(*plane.resources, parent(&queue.parent)) })
    else {
        return E_INVALIDARG;
    };
    if let Some(params) = unsafe { params.as_ref() } {
        if !(0.0..=100.0).contains(&params.immediate_threshold_percent) {
            return E_INVALIDARG;
        }
        let full = unsafe { &*planes.cast::<RetailPresentPlane>() };
        let desc = unsafe { source.GetDesc() };
        let rect = unsafe { full.source_rects.as_ref() };
        let destination = unsafe { full.destination.as_ref() };
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "present plane {}x{} source {rect:?} destination {destination:?} color {} parameters {:?}",
            desc.Width, desc.Height, full.color_space, params.options
        ));
        if rect.is_some_and(|r| r.left != 0 || r.top != 0 || r.right as u64 != desc.Width || r.bottom as u32 != desc.Height)
            || destination.is_some_and(|r| *r != [0.0, 0.0, 1.0, 1.0])
            || full.color_space != 0 || full.remaining != [0; 7] || params.options != [0; 7] {
            // Presented as a full-plane copy anyway; the first mismatch is logged above.
            static PARTIAL: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&PARTIAL, || "present: source rect, destination, color space or options ignored".to_owned());
        }
    }
    let result = {
        let _cost = crate::diagnostics::cost::timer(crate::diagnostics::cost::Bucket::Present);
        queue.state.presentation.present(&queue.native, source)
    };
    // Frame pacing must advance even if the host present failed; the title
    // otherwise waits forever for this token's flip.
    unsafe { device::frame_events::display_flip(parent(&queue.parent), plane.token) };
    crate::device::shader_cache::frame_presented();
    if result == S_OK {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static EARLY: [std::sync::Once;4] = [const { std::sync::Once::new() };4];
        let index = COUNT.fetch_add(1, Ordering::Relaxed);
        if let Some(site) = EARLY.get(index) {
            crate::diagnostics::log_once(site, || format!("completed PresentX #{} token={} source={:p}",index+1,plane.token,unsafe { *plane.resources }));
        } else {
            // A heartbeat every 10 s with the frame rate since the last one: the
            // early lines alone made a running game look stalled, and a count
            // heartbeat stayed silent when slow frames never reached it.
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0); // ms since START
            static LAST_INDEX: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let now = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
            let last = LAST.load(Ordering::Relaxed);
            if now >= last + 10_000 && LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                let frames = (index + 1).saturating_sub(LAST_INDEX.swap(index + 1, Ordering::Relaxed));
                let fps = if last == 0 { 0.0 } else { frames as f64 * 1000.0 / (now - last) as f64 };
                let frame_ms = if fps > 0.0 { 1000.0 / fps } else { 0.0 };
                let gpu = crate::diagnostics::gpu_clock::take().map_or("-".into(), |ms| format!("{ms:.1}"));
                crate::diagnostics::log_line(format_args!(
                    "completed PresentX #{} ({fps:.1} fps; ms/frame: {})", index + 1, crate::diagnostics::cost::split(frames)));
                // Separate line: the frame's GPU latency and every thread's CPU time.
                crate::diagnostics::log_line(format_args!(
                    "frame time {frame_ms:.1} ms: gpu latency {gpu} ms (present signal to GPU done); cpu ms/frame: {}",
                    crate::diagnostics::threads::split(frames.max(1) as f64, frame_ms)));
            }
        }
    }
    result
}

static QUEUE_SLOTS: Slots<64> = {
    let mut slots = child_slots::<ID3D12CommandQueue, QueueState, 64>();
    slots[10] = crate::slot!(execute);
    slots[14] = crate::slot!(queue_fence::<false>);
    slots[15] = crate::slot!(queue_fence::<true>);
    slots[16] = crate::slot!(queries::frequency);
    slots[17] = crate::slot!(queries::calibrate);
    slots[23] = crate::slot!(signal_x);
    // CopyPageMappingsBatchX: the Device11 table's slot 31 less this table's
    // two-slot shift (rebox maps it at 29 on the older queue table too).
    slots[29] = crate::slot!(page_mapping::copy);
    slots[30] = crate::slot!(present);
    slots
};

static DEVICE11_QUEUE_SLOTS: Slots<64> = {
    let mut slots = child_slots::<ID3D12CommandQueue, QueueState, 64>();
    slots[10] = crate::slot!(execute);
    slots[16] = crate::slot!(queue_fence::<false>);
    slots[17] = crate::slot!(queue_fence::<true>);
    slots[18] = crate::slot!(queries::frequency);
    slots[19] = crate::slot!(queries::calibrate);
    slots[25] = crate::slot!(signal_x);
    slots[31] = crate::slot!(page_mapping::copy);
    slots[32] = crate::slot!(present);
    slots[35] = crate::slot!(execute_frame);
    slots
};

pub(crate) unsafe fn wrap_command_list(
    native: ID3D12GraphicsCommandList,
    allocator: &device::Allocator,
    initial: Option<&device::compute::Pipeline>,
    device: &IUnknown,
    compact: bool,
    extended: bool,
    device11: bool,
    out: *mut *mut c_void,
) -> HRESULT {
    let raytracing = if compact {
        None
    } else {
        native.cast::<ID3D12GraphicsCommandList4>().ok()
    };
    let has_list4 = raytracing.is_some();
    let has_list6 = has_list4 && native.cast::<ID3D12GraphicsCommandList6>().is_ok();
    if extended && !has_list6 {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }
    let state = ListData {
        reserved0: [0; 2],
        write_cursor: 0,
        reserved1: [0; 2],
        packet_end: 0,
        reserved2: [0; 0x540 - 0x38],
        pipeline_packets: core::cell::UnsafeCell::new([0; 32]),
        extended: OnceLock::new(),
        enhanced_barriers: OnceLock::new(),
        mesh: OnceLock::new(),
        raytracing,
        recording: Mutex::new(Recording {
            timestamp_page: None,
            prefix: memory_wait::Prefix::default(),
            resources: allocator.state.clone(),
            scene_commands: Vec::new(),
            bindings: compute_bindings::Bindings::new(initial),
            stencil_slot: initial.and_then(|p| device::compute::current(p).1.stencil_slot),
            stencil_targets: None,
            dsv_bound: false,
            display_bound: false,
            trace_target: None,
            trace_samples: Vec::new(),
            null_pipeline: false,
            pipeline_pending: initial.is_some_and(|p| device::compute::current(p).2),
            clamp_budget: None,
            placed_reads: Vec::new(),
            placed_writes: Vec::new(),
            bound_placed_targets: Vec::new(),
            resource_heap: None,
            descriptor_arena: None,
            indexed_arenas: Vec::new(),
            scalar_descriptors: Default::default(),
            sampled_heaps: Vec::new(),
            ring: std::collections::VecDeque::new(),
            stalled_writes: Vec::new(),
            open_queries: Vec::new(),
            split_barriers: Vec::new(),
            media: compact.then(|| media::Recording::new(allocator)),
            phase: ListState::Fresh(allocator.native.clone(), initial.map(|p| p.native.clone())),
            error: S_OK,
        }),
    };
    unsafe {
        allocate(
            if compact { &media::SLOTS } else if extended { &EXTENDED_LIST_SLOTS }
            else if device11 { &DEVICE11_LIST_SLOTS } else { &LIST_SLOTS },
            if compact {
                &[
                    ID3D12DeviceChild::IID,
                    ID3D12CommandList::IID,
                    device::media::DMA_IID,
                ]
            } else if has_list6 {
                &[
                    ID3D12DeviceChild::IID,
                    ID3D12CommandList::IID,
                    ID3D12GraphicsCommandList::IID,
                    ID3D12GraphicsCommandList4::IID,
                    build_raytracing::LIST6,
                ]
            } else if has_list4 {
                &[
                    ID3D12DeviceChild::IID,
                    ID3D12CommandList::IID,
                    ID3D12GraphicsCommandList::IID,
                    ID3D12GraphicsCommandList4::IID,
                ]
            } else {
                &[
                    ID3D12DeviceChild::IID,
                    ID3D12CommandList::IID,
                    ID3D12GraphicsCommandList::IID,
                ]
            },
            native,
            Some(device),
            state,
            out,
        )
    }
}

pub(crate) unsafe fn wrap_command_queue(
    native: ID3D12CommandQueue,
    device: &IUnknown,
    device11_queue: bool,
    out: *mut *mut c_void,
) -> HRESULT {
    let state = QueueState {
        reserved: 0,
        kind: unsafe { native.GetDesc() }.Type,
        placement: placement::QueueAccess::default(),
        submission: Mutex::new(false),
        page_worker: OnceLock::new(),
        wait_worker: OnceLock::new(),
        presentation: Presentation::default(),
    };
    unsafe {
        allocate(
            if device11_queue { &DEVICE11_QUEUE_SLOTS } else { &QUEUE_SLOTS },
            &[
                ID3D12DeviceChild::IID,
                ID3D12Pageable::IID,
                ID3D12CommandQueue::IID,
            ],
            native,
            Some(device),
            state,
            out,
        )
    }
}
