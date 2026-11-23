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
    null_pipeline: bool,
    media: Option<media::Recording>,
    placed_reads: Vec<Arc<Texture>>,
    placed_writes: Vec<Arc<Texture>>,
    bound_placed_targets: Vec<Arc<Texture>>,
    resource_heap: Option<descriptors::ResourceHeap>,
    descriptor_arena: Option<Arc<device::descriptor_paging::Arena>>,
    indexed_arenas: Vec<Arc<device::descriptor_paging::Arena>>,
    scalar_descriptors: std::collections::HashSet<descriptors::ScalarDescriptor>,
    sampled_heaps: Vec<Range<usize>>,
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
        let trace = self.bindings.draw_trace.clone();
        let draw = || {
            draw();
            if let Some(trace) = trace { trace.recorded(); }
        };
        if let Some(table) = self.bindings.reflected_table::<false>() {
            if self.stencil_slot.is_some() { self.error = E_NOTIMPL; return; }
            if let Some(binding) = table.index_map {
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
            ) { self.error = error.code(); }
            return;
        }
        if self.descriptor_arena.is_some() && !self.bindings.supports_paged_draw::<false>() {
            self.error = E_NOTIMPL;
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
            }
        } else {
            draw();
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

    fn ensure_open(&mut self, native: &ID3D12GraphicsCommandList) -> bool {
        if self.error.is_err() {
            return false;
        }
        match &self.phase {
            ListState::Fresh(allocator, initial) => {
                // The factory closed the empty native list to release its allocator.
                // Reopen with its original bindings when the first operation records.
                if let Err(error) = unsafe { native.Reset(allocator, initial.as_ref()) } {
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

fn recording(list: &List) -> MutexGuard<'_, Recording> {
    list.state
        .recording
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
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
        return E_INVALIDARG;
    };
    // SAFETY: the guest holds the initial pipeline through this reset.
    let initial = match unsafe { device::compute::pipeline(initial, device) } {
        Ok(pipeline) => pipeline,
        Err(error) => return error,
    };
    let mut state = recording(list);
    if matches!(state.phase, ListState::Open) {
        return E_FAIL;
    }
    let native = &list.native;
    let hr = unsafe {
        (native.vtable().Reset)(
            native.as_raw(),
            allocator.native.as_raw(),
            initial.map_or(ptr::null_mut(), |p| p.native.as_raw()),
        )
    };
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
        state.stencil_slot = initial.and_then(|p| p.state.stencil_slot);
        if let Some(targets) = &mut state.stencil_targets {
            targets.unbind();
        }
        state.dsv_bound = false;
        state.placed_reads.clear();
        state.placed_writes.clear();
        state.bound_placed_targets.clear();
        state.resource_heap = None;
        state.descriptor_arena = None;
        state.indexed_arenas.clear();
        state.scalar_descriptors.clear();
        state.sampled_heaps.clear();
        state.error = S_OK;
    }
    hr
}

unsafe extern "system" fn close(list: &List) -> HRESULT {
    let mut state = recording(list);
    match state.phase {
        ListState::Closed => return E_FAIL,
        ListState::Fresh(..) => {} // The initial empty recording is already closed on the host.
        ListState::Open => {
            if let Err(error) = state.prefix.finish(&list.native) { state.error = error.code(); }
            if let Err(error) = unsafe { list.native.Close() } {
                state.error = error.code();
            }
        }
    }
    state.phase = ListState::Closed;
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
            host.before = device::host_state(host.before, texture);
            host.after = device::host_state(host.after, texture);
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
            } else if different && host.before == host.after {
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
            Ok(())
        },
    );
    if let Err(error) = result {
        state.error = error;
    }
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
    if !matches!(mask, 0xc0_0000 | 0x3000) {
        crate::diagnostics::recording_error(format_args!(
            "texture cache flush mask {mask:#x} address {address:#x} bytes {bytes:#x}"
        ));
        state.error = E_NOTIMPL;
        return;
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
    if !state.can_record(&list.native) {
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

// Record the title's four-byte progress marker at the requested GPU address.
unsafe extern "system" fn write_marker(list: &List, address: u64, value: u32, flags: u32, reserved: u32) {
    let mut state = recording(list);
    if !state.ensure_open(&list.native) { return; }
    if address == 0 || !address.is_multiple_of(4) || flags != 0 || reserved != 0 {
        state.error = E_INVALIDARG;
        return;
    }
    let result = (|| -> windows::core::Result<()> {
        let (resource, dest) = unsafe { device::command_buffer_address(parent(&list.parent), address, 4)? };
        if let Some(buffer) = &resource { raytracing::retain(&state, &[buffer])?; }
        let write = D3D12_WRITEBUFFERIMMEDIATE_PARAMETER { Dest: dest, Value: value };
        state.prefix.push(&list.native, memory_wait::Packet::Marker(write))
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
    if options != 0 { return E_NOTIMPL; }
    unsafe { reset(list, allocator, initial) }
}

// Device11 titles use the new raster-state slots even when requesting the
// base graphics-list IID. The List6-only cache barriers remain separate.
static DEVICE11_LIST_SLOTS: Slots<149> = {
    let mut slots = LIST_SLOTS;
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

unsafe extern "system" fn execute_frame(
    queue: &Queue, count: u32, lists: *const *mut c_void, frame_token: u64, flags: u32,
) {
    // Both flags select how the console assembles its GPU packets. Bit 0
    // suppresses a hardware buffer switch and bit 1 chains indirect buffers.
    // The desktop queue owns its packet storage and takes the ordered list
    // array in one submission. The frame token labels that submission.
    if queue.state.kind != D3D12_COMMAND_LIST_TYPE_DIRECT || flags & !3 != 0 {
        crate::diagnostics::recording_error(format_args!("extended command-list submission flags {flags:#x}"));
        reject(queue, E_NOTIMPL);
        return;
    }
    unsafe { execute_batch(queue, count, lists, Some((frame_token, flags))) };
}

unsafe extern "system" fn execute(queue: &Queue, count: u32, lists: *const *mut c_void) {
    unsafe { execute_batch(queue, count, lists, None) };
}

unsafe fn execute_batch(queue: &Queue, count: u32, lists: *const *mut c_void, frame: Option<(u64,u32)>) {
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
            Ok(list.native.as_raw())
        },
        |host, (mut textures, mut writes, shader_views, scene_lists, splits)| {
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
                        memory_wait::submit(queue, host, splits)?;
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
        reject(queue, error);
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
    if count != 1 {
        return E_NOTIMPL;
    }
    let plane = unsafe { &*planes };
    if plane.resource_count == 0 || plane.resources.is_null() {
        return E_INVALIDARG;
    }
    if plane.resource_count != 1 {
        return E_NOTIMPL;
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
            return E_NOTIMPL;
        }
    }
    let result = queue.state.presentation.present(&queue.native, source);
    // Frame pacing must advance even if the host present failed; the title
    // otherwise waits forever for this token's flip.
    unsafe { device::frame_events::display_flip(parent(&queue.parent), plane.token) };
    if result == S_OK {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        static EARLY: [std::sync::Once;4] = [const { std::sync::Once::new() };4];
        if COUNT.load(Ordering::Relaxed) < EARLY.len() {
            let index = COUNT.fetch_add(1, Ordering::Relaxed);
            if let Some(site) = EARLY.get(index) {
                crate::diagnostics::log_once(site, || format!("completed PresentX #{} token={} source={:p}",index+1,plane.token,unsafe { *plane.resources }));
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
            stencil_slot: initial.and_then(|p| p.state.stencil_slot),
            stencil_targets: None,
            dsv_bound: false,
            display_bound: false,
            null_pipeline: false,
            placed_reads: Vec::new(),
            placed_writes: Vec::new(),
            bound_placed_targets: Vec::new(),
            resource_heap: None,
            descriptor_arena: None,
            indexed_arenas: Vec::new(),
            scalar_descriptors: Default::default(),
            sampled_heaps: Vec::new(),
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
