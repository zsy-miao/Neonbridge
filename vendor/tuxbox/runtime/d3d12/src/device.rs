//! Xbox device and pageable-object slots for the x64 guest ABI.
//!
//! Each entry borrows its receiver for the duration of the COM call. The guest
//! holds live references to its arguments and provides storage for output
//! pointers. Only the guest device identity is handed back to child objects.

mod queues;
mod resources;
use queues::supported_queue_kind;
use queues::create_queue;
use queues::ComputeQueueDesc;
use queues::create_queue_x;
use queues::ListDesc;
use queues::create_list_x;
use resources::map_resource;
use resources::unmap_resource;
use resources::resource_desc;
use resources::heap_properties;
use resources::gpu_address;
use resources::create_heap;
pub(crate) use resources::host_state;
use resources::create_resource;

use core::ffi::c_void;
use core::ptr::NonNull;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER, LUID};
use windows::Win32::Graphics::{Direct3D::D3D_FEATURE_LEVEL_12_0, Direct3D12::*};
use windows::core::{GUID, HRESULT, IUnknown, Interface, InterfaceRef, Result};

use crate::commands::{wrap_command_list, wrap_command_queue};
use crate::proxy::{Proxy, Slot, Slots, allocate, check_output, child_slots, object_slots, unwrap};

mod allocation;
mod fallback;
pub(crate) mod blas;
pub(crate) mod compute;
mod compute_scene;
mod compute_table;
mod mesh_table;
pub(crate) mod depth;
mod descriptor_copy;
pub(crate) mod descriptor_paging;
mod dxgi;
mod features;
pub(crate) mod format;
mod reserved;
pub(crate) mod frame_events;
mod fence;
mod graphics;
mod heap;
pub(crate) mod indirect;
pub(crate) mod media;
pub(crate) mod placement;
pub(crate) mod queries;
pub(crate) mod ray_build;
mod ray_compile;
mod ray_pipeline;
pub(crate) mod rectangle;
mod shader_cache;
mod state_object;
pub(crate) mod stencil;
mod stream;
mod title_memory;
mod views;
pub(crate) use views::{clear_buffer_alias, command_buffer_address, is_constant_buffer, linear_resource, snapshot_cbv_mirrors, LinearResource};

type Device = Proxy<ID3D12Device, dxgi::DxgiDevice>;
pub(crate) enum Backing {
    Native,
    ConsoleLayout(D3D12_TEXTURE_LAYOUT, D3D12_RESOURCE_FLAGS),
    Addressed(std::sync::Arc<views::TextureAddress>, D3D12_RESOURCE_DESC),
    Placed(std::sync::Arc<placement::Texture>),
    Depth(Box<depth::Pair>),
    /// A host image in a different format family; reports the guest description.
    Translated(D3D12_RESOURCE_DESC),
}
impl Backing {
    pub(crate) fn placed(&self) -> Option<&std::sync::Arc<placement::Texture>> {
        match self {
            Self::Placed(texture) => Some(texture),
            _ => None,
        }
    }
    pub(crate) fn depth(&self) -> Option<&depth::Pair> {
        match self {
            Self::Depth(pair) => Some(pair),
            Self::Addressed(address, _) => address.depth.as_deref(),
            _ => None,
        }
    }
}
pub(crate) struct ResourceState {
    backing: Backing,
    texture_address: std::sync::OnceLock<std::sync::Arc<views::TextureAddress>>,
    blas_buffer: Option<blas::Registration>,
    stencil: std::sync::OnceLock<std::sync::Arc<stencil::Surface>>,
}
impl ResourceState {
    /// Guest description of an image whose host format family differs.
    pub(crate) fn translated(&self) -> Option<&D3D12_RESOURCE_DESC> {
        match &self.backing {
            Backing::Translated(guest) | Backing::Addressed(_, guest)
                if !depth::paired(guest) && format::substitute(guest).is_some() => Some(guest),
            _ => None,
        }
    }
    /// A Scarlett depth image whose unordered access has no host backing yet:
    /// its UAVs are null and unordered-access states use COMMON on the host.
    pub(crate) fn unbacked_uav(&self) -> bool {
        self.translated().is_some_and(format::is_depth)
    }
    pub(crate) fn buffer_window(&self) -> Option<(u64, u64)> {
        if let Backing::Addressed(address, desc) = &self.backing
            && address.buffer_offset != 0
        {
            Some((address.buffer_offset, desc.Width))
        } else {
            None
        }
    }
}
impl From<Backing> for ResourceState {
    fn from(backing: Backing) -> Self {
        Self {
            backing,
            texture_address: Default::default(),
            blas_buffer: None,
            stencil: Default::default(),
        }
    }
}
impl core::ops::Deref for ResourceState {
    type Target = Backing;
    fn deref(&self) -> &Backing {
        &self.backing
    }
}

pub(crate) type Resource = Proxy<ID3D12Resource, ResourceState>;
type Out = *mut *mut c_void;

pub(crate) unsafe fn ray_state<'a>(
    guest: *mut c_void,
    owner: *mut c_void,
) -> Option<&'a ID3D12StateObject> {
    unsafe { state_object::native(guest, owner) }
}

fn identity(device: &Device) -> InterfaceRef<'_, IUnknown> {
    // SAFETY: A device proxy begins with the device table, whose first three
    // slots implement IUnknown. The borrowed identity cannot outlive device.
    unsafe { InterfaceRef::from_raw(NonNull::from(device).cast()) }
}

unsafe fn wrap_pageable<N: Interface, S>(
    created: Result<N>,
    table: &'static [Slot],
    parent: &Device,
    state: S,
    out: Out,
) -> HRESULT {
    match created {
        Ok(native) => unsafe {
            allocate(
                table,
                &[ID3D12DeviceChild::IID, ID3D12Pageable::IID, N::IID],
                native,
                Some(&identity(parent)),
                state,
                out,
            )
        },
        Err(error) => error.code(),
    }
}

pub(crate) type MediaResources = std::sync::Arc<std::sync::Mutex<Vec<ID3D12Pageable>>>;
pub(crate) type Allocator = Proxy<ID3D12CommandAllocator, MediaResources>;

unsafe extern "system" fn reset_allocator(this: &Allocator) -> HRESULT {
    let host = &this.native;
    let mut sources = this.state.lock().unwrap_or_else(|e| e.into_inner());
    let status = unsafe { (host.vtable().Reset)(host.as_raw()) };
    if status.is_ok() {
        sources.clear();
        crate::commands::list_memory::release(std::sync::Arc::as_ptr(&this.state) as usize);
    }
    status
}

// The Xbox callback supplies additional command-buffer pages on demand.
// Native D3D12 allocators obtain their own storage, so recording translated
// commands never requests a guest page.
extern "system" fn set_allocator_callback(_: &Allocator, _: *const c_void, _: *mut c_void) {}

static ALLOCATOR: Slots<16> = {
    let mut slots = child_slots::<ID3D12CommandAllocator, MediaResources, 16>();
    slots[8] = crate::slot!(reset_allocator);
    slots[9] = crate::slot!(set_allocator_callback);
    slots
};

static RESOURCE: Slots<32> = {
    let mut slots = child_slots::<ID3D12Resource, ResourceState, 32>();
    slots[8] = crate::slot!(map_resource);
    slots[9] = crate::slot!(unmap_resource);
    slots[10] = crate::slot!(resource_desc);
    slots[11] = crate::slot!(gpu_address);
    slots[14] = crate::slot!(heap_properties);
    slots
};

// Scarlett returns the address directly in RAX. The native method writes a
// D3D12_CPU_DESCRIPTOR_HANDLE to a hidden return buffer.
pub(crate) struct DescriptorHeapState {
    cpu_start: usize,
    pub(crate) arena: Option<std::sync::Arc<descriptor_paging::Arena>>,
    pub(crate) desc: D3D12_DESCRIPTOR_HEAP_DESC,
    pub(crate) placed_views: Option<placement::DescriptorRange<placement::PlacedView>>,
    _depth_views: Option<placement::HeapViews<stencil::Surface>>,
    _placed_targets: Option<placement::HeapViews<placement::Texture>>,
    _imported_views: Option<placement::DescriptorRange<views::DescriptorBacking>>,
    _scene_views: Option<placement::DescriptorRange<blas::SceneView>>,
}
type DescriptorHeap = Proxy<ID3D12DescriptorHeap, DescriptorHeapState>;

unsafe extern "system" fn cpu_handle(this: &DescriptorHeap) -> usize {
    this.state.cpu_start
}

unsafe extern "system" fn gpu_handle(this: &DescriptorHeap) -> u64 {
    this.state.arena.as_ref().map_or_else(|| unsafe { this.native.GetGPUDescriptorHandleForHeapStart().ptr }, |arena| arena.base() as u64)
}

// FD3D12SubAllocatedOnlineHeap::AllocateBlock passes writable storage in RDX.
unsafe extern "system" fn heap_desc(
    this: &DescriptorHeap,
    out: *mut D3D12_DESCRIPTOR_HEAP_DESC,
) -> *mut D3D12_DESCRIPTOR_HEAP_DESC {
    unsafe { out.write(this.state.desc) };
    out
}

static HEAP: Slots<16> = {
    let mut slots = child_slots::<ID3D12DescriptorHeap, DescriptorHeapState, 16>();
    slots[8] = crate::slot!(heap_desc);
    slots[9] = crate::slot!(cpu_handle);
    slots[10] = crate::slot!(gpu_handle);
    slots
};

pub(crate) unsafe fn descriptor_heap<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a DescriptorHeap> {
    unsafe { unwrap(object, device, &HEAP) }
}

/// Borrows a native resource while the caller holds the guest object alive.
///
/// # Safety
/// Non-null objects must have a readable COM vtable pointer and stay alive for
/// the returned borrow. `device` is the expected guest device identity.
pub(crate) unsafe fn resource<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a ID3D12Resource> {
    unsafe { resource_proxy(object, device) }.map(|proxy| &proxy.native)
}

pub(crate) unsafe fn resource_proxy<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a Resource> {
    unsafe { unwrap(object, device, &RESOURCE) }
}

pub(crate) unsafe fn memory_heap<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a ID3D12Heap> {
    unsafe { heap::memory_heap(object, device) }
}

/// Borrows a native fence under the same ownership contract as [`resource`].
pub(crate) unsafe fn fence<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a ID3D12Fence> {
    unsafe { fence::native(object, device) }
}

/// Borrows a native allocator under the same ownership contract as [`resource`].
pub(crate) unsafe fn allocator<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a Allocator> {
    unsafe { unwrap(object, device, &ALLOCATOR) }
}

unsafe extern "system" fn create_allocator(
    this: &Device,
    kind: D3D12_COMMAND_LIST_TYPE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandAllocator>(iid, out) };
    if status.is_err() {
        return status;
    }
    if !supported_queue_kind(kind) {
        return E_NOTIMPL;
    }
    unsafe {
        wrap_pageable(
            this.native
                .CreateCommandAllocator::<ID3D12CommandAllocator>(kind),
            &ALLOCATOR,
            this,
            MediaResources::default(),
            out,
        )
    }
}

unsafe extern "system" fn increment_size(this: &Device, kind: D3D12_DESCRIPTOR_HEAP_TYPE) -> u32 {
    unsafe { this.native.GetDescriptorHandleIncrementSize(kind) }
}

pub(crate) const BUFFER_READ_BITS: i32 =
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER.0 | D3D12_RESOURCE_STATE_INDEX_BUFFER.0;

unsafe extern "system" fn create_fence(
    this: &Device,
    initial: u64,
    flags: D3D12_FENCE_FLAGS,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Fence>(iid, out) };
    if status.is_err() {
        return status;
    }
    unsafe {
        wrap_pageable(
            this.native.CreateFence::<ID3D12Fence>(initial, flags),
            &fence::TABLE,
            this,
            fence::State::new(),
            out,
        )
    }
}

// The guest base slot uses a hidden LUID output pointer after the receiver.
unsafe extern "system" fn adapter_luid(this: &Device, result: *mut LUID) -> *mut LUID {
    unsafe { result.write(this.native.GetAdapterLuid()) };
    result
}

const _: () = assert!(size_of::<ListDesc>() == 16 && size_of::<ComputeQueueDesc>() == 16);

pub(crate) type DisposeCallback = unsafe extern "system" fn(*mut c_void, *mut c_void);

/// Accept the controlling guest device identity. Other COM object layouts
/// cannot contain the device's callback slot.
pub(crate) unsafe fn set_dispose_callback(
    device: *mut c_void, callback: Option<DisposeCallback>, context: *mut c_void,
) -> HRESULT {
    if device.is_null() || unsafe { device.cast::<*const Slot>().read_unaligned() } != DEVICE.as_ptr() {
        return E_INVALIDARG;
    }
    let state = unsafe { &(*device.cast::<Device>()).state };
    *state.disposal.lock().unwrap_or_else(|e| e.into_inner()) =
        callback.map(|callback| (callback, context.expose_provenance()));
    HRESULT(0)
}

// The desktop driver manages its own hardware policy. Console tuning hints
// have no required result or output object.
extern "system" fn set_driver_hint(_: &Device, _: u32, _: u32) {}

unsafe extern "system" fn release_device(this: *mut Device) -> u32 {
    unsafe { crate::proxy::release_with(this, || {
        let hook = (*this).state.disposal.lock().unwrap_or_else(|e| e.into_inner()).take();
        if let Some((callback, context)) = hook {
            callback(this.cast(), core::ptr::with_exposed_provenance_mut(context));
        }
    }) }
}

/// Add this thread's pipeline to the per-run failure record if it failed or got
/// a stand-in.
///
/// # Safety
/// The guest shaders given to `failures::stage` must still be readable.
pub(super) unsafe fn record_pipeline(path: &str, status: HRESULT) {
    // SAFETY: forwarded from the caller.
    let started = unsafe { crate::failures::finish(path, status.0, shader_cache::source_digest) };
    if let Some(file) = started {
        crate::diagnostics::log(&format!("pipeline failures are recorded in {}", file.display()));
    }
}

static DEVICE: Slots<160> = {
    let mut slots = object_slots::<ID3D12Device, dxgi::DxgiDevice, 160>();
    slots[0] = crate::slot!(dxgi::query);
    slots[2] = crate::slot!(release_device);
    slots[8] = crate::slot!(create_queue);
    slots[9] = crate::slot!(create_allocator);
    slots[13] = crate::slot!(features::query);
    slots[14] = crate::slot!(create_heap);
    slots[15] = crate::slot!(increment_size);
    slots[16] = crate::slot!(compute::create_root_signature);
    slots[17] = crate::slot!(views::cbv);
    slots[18] = crate::slot!(views::srv);
    slots[19] = crate::slot!(views::uav);
    slots[20] = crate::slot!(views::rtv);
    slots[21] = crate::slot!(views::dsv);
    slots[22] = crate::slot!(views::sampler);
    slots[23] = crate::slot!(descriptor_copy::ranges);
    slots[24] = crate::slot!(descriptor_copy::simple);
    slots[25] = crate::slot!(allocation::info);
    slots[26] = crate::slot!(allocation::custom_heap_properties);
    slots[27] = crate::slot!(traced::committed::<false>);
    slots[28] = crate::slot!(heap::create);
    slots[29] = crate::slot!(traced::place);
    slots[30] = crate::slot!(traced::reserved);
    slots[34] = crate::slot!(reserved::make_resident);
    slots[35] = crate::slot!(reserved::evict);
    slots[36] = crate::slot!(create_fence);
    slots[38] = crate::slot!(allocation::footprints);
    slots[39] = crate::slot!(queries::create_heap);
    slots[41] = crate::slot!(indirect::create_signature);
    slots[42] = crate::slot!(reserved::tiling);
    slots[43] = crate::slot!(adapter_luid);
    slots[44] = crate::slot!(traced::place_virtual);
    slots[48] = crate::slot!(create_queue_x);
    // Precompiled graphics PSO with separate driver metadata and GPU shader addresses.
    slots[57] = crate::slot!(compute::deserialize_pipeline_x);
    slots[60] = crate::slot!(graphics::create_pipeline);
    slots[61] = crate::slot!(views::sampler_x);
    slots[62] = crate::slot!(compute::create_pipeline_x);
    slots[67] = crate::slot!(compute::deserialize_pipeline_x);
    slots[69] = crate::slot!(set_driver_hint);
    slots[70] = crate::slot!(views::address_srv);
    slots[71] = crate::slot!(views::address_uav);
    slots[73] = crate::slot!(create_list_x);
    slots[74] = crate::slot!(heap::register_pages);
    slots[75] = crate::slot!(heap::unregister_pages);
    slots[76] = crate::slot!(traced::committed::<true>);
    slots[78] = crate::slot!(frame_events::interval);
    slots[79] = crate::slot!(frame_events::schedule);
    slots[80] = crate::slot!(frame_events::wait);
    slots[81] = crate::slot!(frame_events::statistics);
    slots[85] = crate::slot!(heap::residency_priority);
    slots[86] = crate::slot!(stream::create_pipeline);
    slots[101] = crate::slot!(ray_pipeline::create);
    slots[102] = crate::slot!(ray_build::prebuild);
    slots[106] = crate::slot!(ray_pipeline::add);
    slots[108] = crate::slot!(allocation::info2);
    slots[115] = crate::slot!(state_object::deserialize);
    slots[127] = crate::slot!(traced::committed_layout);
    slots[133] = crate::slot!(traced::place_virtual_layout);
    slots
};

pub(crate) const DEVICE_INTERFACES: &[GUID] = &[
    ID3D12Device::IID,
    ID3D12Device1::IID,
    ID3D12Device2::IID,
    ID3D12Device3::IID,
    ID3D12Device4::IID,
    ID3D12Device5::IID,
    ID3D12Device6::IID,
    ID3D12Device7::IID,
    state_object::DEVICE8,
    ID3D12Device9::IID,
    ID3D12Device10::IID,
    ID3D12Device11::IID,
];

/// # Safety
/// `adapter` must be null or a borrowed DXGI adapter. The caller must supply
/// a cleared, writable output pointer for the new guest device reference.
pub(crate) unsafe fn create(adapter: *mut c_void, requested: GUID, out: Out) -> HRESULT {
    let mut native = None;
    // SAFETY: The public factory validated its arguments. A null adapter
    // selects D3D12's default, otherwise it remains borrowed for this call.
    if let Err(error) = unsafe {
        D3D12CreateDevice::<_, ID3D12Device>(
            IUnknown::from_raw_borrowed(&adapter),
            D3D_FEATURE_LEVEL_12_0,
            &mut native,
        )
    } {
        return error.code();
    }
    let Some(native) = native else {
        return E_POINTER;
    };
    crate::diagnostics::device_created(&native);
    // SAFETY: the caller holds the selected adapter through factory return.
    let pipeline_device = native.cast::<ID3D12Device2>().ok();
    let interfaces: &'static [GUID] = if native.cast::<ID3D12Device11>().is_ok() {
        DEVICE_INTERFACES
    } else if native.cast::<ID3D12Device10>().is_ok() {
        &DEVICE_INTERFACES[..11]
    } else if native.cast::<ID3D12Device9>().is_ok() {
        &DEVICE_INTERFACES[..10]
    } else if native.cast::<ID3D12Device8>().is_ok() {
        &DEVICE_INTERFACES[..9]
    } else if native.cast::<ID3D12Device7>().is_ok() {
        &DEVICE_INTERFACES[..8]
    } else if native.cast::<ID3D12Device6>().is_ok() {
        &DEVICE_INTERFACES[..7]
    } else if native.cast::<ID3D12Device5>().is_ok() {
        &DEVICE_INTERFACES[..6]
    } else if pipeline_device.is_some() {
        &[ID3D12Device::IID, ID3D12Device1::IID, ID3D12Device2::IID]
    } else {
        &[ID3D12Device::IID]
    };
    if !interfaces.contains(&requested) {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }
    let state = match dxgi::DxgiDevice::new(
        unsafe { IUnknown::from_raw_borrowed(&adapter) },
        pipeline_device,
        requested == ID3D12Device11::IID,
    ) {
        Ok(state) => state,
        Err(err) => return err.code(),
    };
    unsafe { allocate(&DEVICE, interfaces, native, None, state, out) }
}

/// Resource-creation slots, reporting failed descriptions (see diagnostics).
mod traced {
    use super::{Device, Out, create_resource, heap};
    use core::ffi::c_void;
    use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT};
    use windows::core::{GUID, HRESULT};

    fn report(slot: u32, status: HRESULT, initial: i32, desc: *const D3D12_RESOURCE_DESC) -> HRESULT {
        crate::diagnostics::resource_failure(slot, status, initial, unsafe { desc.as_ref() });
        status
    }

    pub(super) unsafe extern "system" fn committed<const EXTENDED: bool>(
        device: &Device, properties: *const D3D12_HEAP_PROPERTIES, flags: D3D12_HEAP_FLAGS,
        desc: *const D3D12_RESOURCE_DESC, initial: D3D12_RESOURCE_STATES,
        clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        let status = unsafe { create_resource::<EXTENDED>(device, properties, flags, desc, initial, clear, iid, out) };
        report(if EXTENDED { 76 } else { 27 }, status, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn place(
        device: &Device, heap: *mut c_void, offset: u64, desc: *const D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES, clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(29, unsafe { heap::place(device, heap, offset, desc, initial, clear, iid, out) }, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn reserved(
        device: &Device, desc: *const D3D12_RESOURCE_DESC, initial: D3D12_RESOURCE_STATES,
        clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(30, unsafe { super::reserved::create(device, desc, initial, clear, iid, out) }, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn place_virtual(
        device: &Device, address: u64, desc: *const D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES, clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(44, unsafe { heap::place_virtual(device, address, desc, initial, clear, iid, out) }, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn committed_layout(
        device: &Device, properties: *const D3D12_HEAP_PROPERTIES, flags: D3D12_HEAP_FLAGS,
        desc: *const D3D12_RESOURCE_DESC, initial: D3D12_BARRIER_LAYOUT, clear: *const D3D12_CLEAR_VALUE,
        protected: *mut c_void, count: u32, formats: *const DXGI_FORMAT, iid: *const GUID, out: Out,
    ) -> HRESULT {
        let status = unsafe {
            heap::committed_layout(device, properties, flags, desc, initial, clear, protected, count, formats, iid, out)
        };
        report(127, status, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn place_virtual_layout(
        device: &Device, address: u64, desc: *const D3D12_RESOURCE_DESC, layout: D3D12_BARRIER_LAYOUT,
        clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(133, unsafe { heap::place_virtual_layout(device, address, desc, layout, clear, iid, out) }, layout.0, desc)
    }
}
