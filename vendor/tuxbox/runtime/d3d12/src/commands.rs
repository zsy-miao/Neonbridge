//! Guest command lists and queues. Embedded guest COM pointers are translated
//! before a batch reaches the desktop API.

mod recording;
mod barriers;
mod copies;
mod markers;
mod submit;
mod present;
use barriers::transition;
use barriers::enhanced_barriers;
use barriers::flush_texture_cache;
use barriers::extended_cache_barrier;
use copies::copy_texture;
use copies::copy_resource;
use markers::write_marker;
use markers::write_value64;
use submit::translate_batch;
use submit::reject;
use submit::execute_frame;
use submit::execute;
use submit::queue_fence;
use submit::signal_x;
use present::present;

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
mod copy_memory;
pub(crate) mod list_memory;
pub(crate) mod queue_trace;
mod media;
mod memory_wait;
pub(crate) mod page_mapping;
mod queries;
mod ray_dispatch;
mod raytracing;
pub(crate) mod serialized_blas;
mod standard;
mod stencil;
mod tiles;

enum ListState {
    Fresh(ID3D12CommandAllocator, Option<ID3D12PipelineState>),
    Open,
    Closed,
}

struct Recording {
    prefix: memory_wait::Prefix,
    /// Fence label writes and waits, for explaining a stalled wait.
    labels: Vec<memory_wait::Label>,
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
    media: Option<media::Recording>,
    placed_reads: Vec<Arc<Texture>>,
    placed_writes: Vec<Arc<Texture>>,
    bound_placed_targets: Vec<Arc<Texture>>,
    resource_heap: Option<descriptors::ResourceHeap>,
    descriptor_arena: Option<Arc<device::descriptor_paging::Arena>>,
    descriptor_leases: device::descriptor_paging::DescriptorLeases,
    indexed_arenas: Vec<Arc<device::descriptor_paging::Arena>>,
    scalar_descriptors: std::collections::HashSet<descriptors::ScalarDescriptor>,
    sampled_heaps: Vec<Range<usize>>,
    phase: ListState,
    error: HRESULT,
    /// `RecordingGuard` has logged the current error.
    error_reported: bool,
}

impl Recording {

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

#[track_caller]
fn recording(list: &List) -> RecordingGuard<'_> {
    RecordingGuard(
        list.state
            .recording
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()),
        std::panic::Location::caller(),
    )
}

/// The locked recording state. A command that puts the list into its error
/// state is reported as it returns, while its title caller is still known.
struct RecordingGuard<'a>(MutexGuard<'a, Recording>, &'static std::panic::Location<'static>);

impl std::ops::Deref for RecordingGuard<'_> {
    type Target = Recording;
    fn deref(&self) -> &Recording {
        &self.0
    }
}

impl std::ops::DerefMut for RecordingGuard<'_> {
    fn deref_mut(&mut self) -> &mut Recording {
        &mut self.0
    }
}

impl Drop for RecordingGuard<'_> {
    fn drop(&mut self) {
        if self.0.error.is_ok() {
            self.0.error_reported = false;
        } else if !self.0.error_reported {
            self.0.error_reported = true;
            static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let count = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
            if count <= 32 || count.is_power_of_two() {
                crate::diagnostics::log(&format!(
                    "command list failed {:#010x} (#{count}) in the command at {}:{}, guest caller {:#x}",
                    self.0.error.0, self.1.file(), self.1.line(),
                    crate::survey::title_rva().unwrap_or(0),
                ));
            }
        }
    }
}

struct RetireQueueFence {
    fence: ID3D12Fence,
    next: std::sync::atomic::AtomicU64,
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
    retire_fence: OnceLock<RetireQueueFence>,
}

impl QueueState {
    fn retire_fence(&self, queue: &ID3D12CommandQueue) -> windows::core::Result<&RetireQueueFence> {
        if let Some(rf) = self.retire_fence.get() {
            return Ok(rf);
        }
        let mut device: Option<ID3D12Device> = None;
        unsafe { queue.GetDevice(&mut device)? };
        let device = device.ok_or(E_FAIL)?;
        let fence: ID3D12Fence = unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        Ok(self.retire_fence.get_or_init(|| RetireQueueFence {
            fence,
            next: std::sync::atomic::AtomicU64::new(0),
        }))
    }
}

type Queue = Proxy<ID3D12CommandQueue, QueueState>;
const _: () = assert!(mem::offset_of!(Queue, state) + mem::offset_of!(QueueState, kind) == 0x10);

fn parent(device: &Option<IUnknown>) -> *mut c_void {
    device.as_ref().map_or(ptr::null_mut(), Interface::as_raw)
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
        state.labels.clear();
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
        state.trace_target = None;
        state.trace_samples.clear();
        state.placed_reads.clear();
        state.placed_writes.clear();
        state.bound_placed_targets.clear();
        state.resource_heap = None;
        state.descriptor_arena = None;
        state.descriptor_leases.clear();
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
    if state.error.is_err() {
        memory_wait::note_list(
            ptr::from_ref(list).addr(),
            memory_wait::Fate::Dropped(state.error.0),
            &state.labels,
        );
        // A rejected list drops all its work, including writes other queues
        // wait for, so always say so.
        static REJECTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let count = REJECTED.fetch_add(1, Ordering::Relaxed) + 1;
        if count <= 16 || count.is_power_of_two() {
            crate::diagnostics::log(&format!(
                "graphics command list Close rejected {:#010x} (#{count}, list {list:p}, guest caller {:#x})",
                state.error.0, crate::survey::title_rva().unwrap_or(0),
            ));
        }
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

// DriverHintX (Scarlett slot 101, GDK d3d12_xs.h). The desktop driver manages
// its own hardware policy, so console tuning hints have no effect. The title
// calls it with feature 2 and value 1 (RVA 0x50a6651) and ignores the result.
unsafe extern "system" fn driver_hint(_list: &List, _feature: u64, _value: u64) -> u64 {
    0
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

/// `D3D12XBOX_WRITE_VALUE_BOP_FLAG_GPU_TIMESTAMP`.
const BOP_GPU_TIMESTAMP: u32 = 1;

// XboxBvhBuilder emits this when its inline packet space is exhausted, with
// the same words it otherwise writes as a 0x4003cc00 packet: counts X, Y, Z
// and the hardware COMPUTE_DISPATCH_INITIATOR flags word. When bit 5
// (USE_THREAD_DIMENSIONS, 0x20) is set, X/Y/Z count threads and are divided
// by the bound compute PSO's [numthreads] (rounded up).
unsafe extern "system" fn dispatch_flagged(list: &List, x: u32, y: u32, z: u32, flags: u32) {
    unsafe { compute::dispatch_flagged(list, x, y, z, flags) }
}

static LIST_SLOTS: Slots<149> = {
    let mut slots = child_slots::<ID3D12GraphicsCommandList, ListData, 149>();
    slots[8] = crate::slot!(get_type);
    slots[9] = crate::slot!(close);
    slots[10] = crate::slot!(reset);
    slots[11] = crate::slot!(standard::clear_state);
    slots[12] = crate::slot!(graphics::draw);
    slots[13] = crate::slot!(graphics::draw_indexed);
    slots[14] = crate::slot!(compute::dispatch);
    slots[15] = crate::slot!(compute::copy_buffer);
    slots[16] = crate::slot!(standard::copy_texture_region);
    slots[17] = crate::slot!(standard::copy_whole_resource);
    slots[18] = crate::slot!(standard::copy_tiles);
    slots[19] = crate::slot!(standard::resolve_subresource);
    slots[20] = crate::slot!(graphics::topology);
    slots[21] = crate::slot!(graphics::viewports);
    slots[22] = crate::slot!(graphics::scissors);
    slots[23] = crate::slot!(graphics::blend_factor);
    slots[24] = crate::slot!(graphics::stencil_ref);
    slots[25] = crate::slot!(graphics::pipeline);
    slots[26] = crate::slot!(transition);
    slots[27] = crate::slot!(standard::execute_bundle);
    slots[28] = crate::slot!(queries::query::<true>);
    slots[29] = crate::slot!(queries::query::<false>);
    slots[30] = crate::slot!(queries::resolve);
    slots[31] = crate::slot!(standard::set_predication);
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
    slots[49] = crate::slot!(standard::stream_output);
    slots[50] = crate::slot!(graphics::targets);
    slots[51] = crate::slot!(depth::clear);
    slots[52] = crate::slot!(clear_rtv);
    slots[53] = crate::slot!(depth::clear_uav::<false>);
    slots[54] = crate::slot!(depth::clear_uav::<true>);
    slots[55] = crate::slot!(discard);
    slots[56] = crate::slot!(standard::set_marker);
    slots[57] = crate::slot!(standard::begin_event);
    slots[58] = crate::slot!(standard::end_event);
    // Slots after EndEvent differ between Xbox generations. Keep traced
    // assignments, including slot 59 and the slot 62 memory copy.
    slots[59] = crate::slot!(execute_indirect);
    // CopyMemoryToMemoryX; ProjectScarlett reads textures back through it
    // (title RVA 0x979650). Device11 and extended lists copy this table.
    slots[62] = crate::slot!(copy_memory::copy_memory);
    slots[67] = crate::slot!(flush_texture_cache);
    slots[68] = crate::slot!(graphics::depth_bounds);
    slots[71] = crate::slot!(write_marker);
    slots[72] = crate::slot!(write_value64);
    slots[74] = crate::slot!(memory_wait::wait_value64);
    slots[80] = crate::slot!(dispatch_flagged);
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
    // XboxBvhBuilder's batched builds take descriptor memory from the list.
    slots[148] = crate::slot!(list_memory::allocate_slot);
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
    // Scarlett's list adds InsertGpuMarker and InsertTimingMarker after
    // EndEvent (GDK d3d12_xs.h), so ExecuteIndirect moves from slot 59 to 61
    // and CopyMemoryToMemoryX (slot 62) follows it. ProjectScarlett calls it
    // from title RVA 0x248cbc1 with a one-command signature.
    slots[61] = crate::slot!(execute_indirect);
    slots[101] = crate::slot!(driver_hint);
    // The entries below were traced under the older numbering (Frosty:
    // CopyResourceX 84, DXR 117 to 121, DispatchMesh 127), two lower than the
    // Scarlett layout in GDK d3d12_xs.h. The functions and signatures are the
    // same, so a Scarlett title reaches them at these slots.
    slots[86] = crate::slot!(copy_resource);
    slots[119] = crate::slot!(build_raytracing::single);
    slots[120] = crate::slot!(raytracing::postbuild);
    slots[121] = crate::slot!(raytracing::copy);
    slots[122] = crate::slot!(ray_dispatch::pipeline);
    slots[123] = crate::slot!(ray_dispatch::dispatch);
    slots[129] = crate::slot!(graphics::dispatch_mesh);
    // SyncTopOfPipeX and SyncBottomOfPipeX; ProjectScarlett calls slot 131
    // from title RVA 0x1560994. The batched build moves to slot 133.
    slots[131] = crate::slot!(extended_cache_barrier);
    slots[132] = crate::slot!(extended_cache_barrier);
    slots[133] = crate::slot!(build_raytracing::batch);
    slots[70] = crate::slot!(graphics::depth_bounds);
    slots[75] = crate::slot!(graphics::force_shader_depth_order);
    slots[142] = crate::slot!(graphics::sample_mask);
    slots
};

static EXTENDED_LIST_SLOTS: Slots<149> = DEVICE11_LIST_SLOTS;

unsafe fn guest_list<'a>(object: *mut c_void, device: *mut c_void) -> Option<&'a List> {
    unsafe {
        unwrap(object, device, &LIST_SLOTS)
            .or_else(|| unwrap(object, device, &DEVICE11_LIST_SLOTS))
            .or_else(|| unwrap(object, device, &EXTENDED_LIST_SLOTS))
            .or_else(|| unwrap(object, device, &media::SLOTS))
    }
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

static QUEUE_SLOTS: Slots<64> = {
    let mut slots = child_slots::<ID3D12CommandQueue, QueueState, 64>();
    // Tile mapping sits before the marker methods, so both queue generations share it.
    slots[8] = crate::slot!(tiles::update);
    slots[9] = crate::slot!(tiles::copy);
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
    slots[8] = crate::slot!(tiles::update);
    slots[9] = crate::slot!(tiles::copy);
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
            labels: Vec::new(),
            timestamp_page: None,
            prefix: memory_wait::Prefix::default(),
            resources: allocator.state.clone(),
            scene_commands: Vec::new(),
            bindings: compute_bindings::Bindings::new(initial),
            stencil_slot: initial.and_then(|p| p.state.stencil_slot),
            stencil_targets: None,
            dsv_bound: false,
            display_bound: false,
            trace_target: None,
            trace_samples: Vec::new(),
            placed_reads: Vec::new(),
            placed_writes: Vec::new(),
            bound_placed_targets: Vec::new(),
            resource_heap: None,
            descriptor_arena: None,
            descriptor_leases: Default::default(),
            indexed_arenas: Vec::new(),
            scalar_descriptors: Default::default(),
            sampled_heaps: Vec::new(),
            media: compact.then(|| media::Recording::new(allocator)),
            phase: ListState::Fresh(allocator.native.clone(), initial.map(|p| p.native.clone())),
            error: S_OK,
            error_reported: false,
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
        retire_fence: OnceLock::new(),
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
