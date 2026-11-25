//! Xbox device and pageable-object slots for the x64 guest ABI.
//!
//! Each entry borrows its receiver for the duration of the COM call. The guest
//! holds live references to its arguments and provides storage for output
//! pointers. Only the guest device identity is handed back to child objects.

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

unsafe extern "system" fn map_resource(
    this: &Resource,
    subresource: u32,
    read: *const D3D12_RANGE,
    data: Out,
) -> HRESULT {
    let host = &this.native;
    if let Backing::Placed(texture) = &this.state.backing {
        // The host image is device memory; its contents live at the guest address.
        let Some(address) = texture.guest_pointer(subresource) else { return E_INVALIDARG };
        if !data.is_null() {
            unsafe { data.write(address as *mut c_void) };
        }
        return windows::Win32::Foundation::S_OK;
    }
    let Some((offset, width)) = this.state.buffer_window() else {
        let hr = unsafe { (host.vtable().Map)(host.as_raw(), subresource, read, data) };
        if hr.is_err() {
            let desc = unsafe { host.GetDesc() };
            let kind = match &this.state.backing {
                Backing::Native => "native", Backing::ConsoleLayout(..) => "console-layout",
                Backing::Addressed(..) => "addressed", Backing::Placed(_) => "placed",
                Backing::Depth(_) => "depth", Backing::Translated(_) => "translated",
            };
            let heap = unsafe { host.GetHeapProperties(None, None) }.map(|_| ()).is_ok();
            crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                "Map failed {:#010x}: {kind} dim={} {}x{}x{} format={} layout={:#x} flags={:#x} subresource={subresource} heap-properties={heap} title RVA {:#x}",
                hr.0, desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Layout.0, desc.Flags.0,
                crate::survey::title_rva().unwrap_or(0)));
        }
        return hr;
    };
    let Some(range) = buffer_map_range(offset, width, unsafe { read.as_ref() }) else {
        return E_INVALIDARG;
    };
    let hr = unsafe { (host.vtable().Map)(host.as_raw(), subresource, &range, data) };
    if hr.is_ok() && !data.is_null() {
        unsafe { data.write((*data).byte_add(offset as usize)) };
    }
    hr
}

unsafe extern "system" fn unmap_resource(
    this: &Resource,
    subresource: u32,
    written: *const D3D12_RANGE,
) {
    let host = &this.native;
    if let Some(texture) = this.state.placed() {
        if texture.guest_pointer(subresource).is_some()
            && unsafe { written.as_ref() }.is_none_or(|range| range.End > range.Begin)
            && let Some(parent) = &this.parent
        {
            // SAFETY: resource proxies are created with a retained guest device.
            let device = unsafe { &*parent.as_raw().cast::<Device>() };
            if device.state.device11_queue
                && let Err(error) = device.state.placements.finish_cpu_write(texture)
            {
                crate::diagnostics::recording_error(format_args!("placed texture Unmap upload: {error}"));
            }
        }
        return;
    }
    let window;
    let written = if let Some((offset, width)) = this.state.buffer_window() {
        window = buffer_map_range(offset, width, unsafe { written.as_ref() })
            .unwrap_or_else(|| crate::proxy::fail_call(format_args!("placed buffer Unmap range outside the resource")));
        &window
    } else { written };
    unsafe { (host.vtable().Unmap)(host.as_raw(), subresource, written) }
}

fn buffer_map_range(offset: u64, width: u64, range: Option<&D3D12_RANGE>) -> Option<D3D12_RANGE> {
    let (begin, end) = range.map_or((0, width), |r| (r.Begin as u64, r.End as u64));
    if begin >= end {
        return Some(D3D12_RANGE::default());
    }
    if end > width {
        return None;
    }
    Some(D3D12_RANGE { Begin: (offset + begin) as usize, End: (offset + end) as usize })
}

static RESOURCE: Slots<32> = {
    let mut slots = child_slots::<ID3D12Resource, ResourceState, 32>();
    slots[8] = crate::slot!(map_resource);
    slots[9] = crate::slot!(unmap_resource);
    slots[10] = crate::slot!(resource_desc);
    slots[11] = crate::slot!(gpu_address);
    slots[14] = crate::slot!(heap_properties);
    slots
};

unsafe extern "system" fn resource_desc(
    this: &Resource,
    out: *mut D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_DESC {
    unsafe {
        out.write(match &this.state.backing {
            Backing::Depth(pair) => pair.guest,
            Backing::Translated(guest) => *guest,
            Backing::Addressed(_, guest) => *guest,
            Backing::Placed(texture) => texture.desc,
            Backing::Native => this.native.GetDesc(),
            Backing::ConsoleLayout(layout, flags) => D3D12_RESOURCE_DESC {
                Layout: *layout,
                Flags: *flags,
                ..this.native.GetDesc()
            },
        })
    };
    out
}

unsafe extern "system" fn heap_properties(
    this: &Resource,
    props: *mut D3D12_HEAP_PROPERTIES,
    flags: *mut D3D12_HEAP_FLAGS,
) -> HRESULT {
    let native = &this.native;
    unsafe { (native.vtable().GetHeapProperties)(native.as_raw(), props, flags) }
}

unsafe extern "system" fn gpu_address(this: &Resource) -> u64 {
    if let Backing::Addressed(buffer, _) = &this.state.backing { return buffer.base as u64; }
    if let Some(placed) = this.state.placed() {
        return placed.address;
    }
    let desc = unsafe { this.native.GetDesc() };
    this.state.texture_address.get_or_init(|| {
        let owner = this.parent.as_ref().expect("resource belongs to a device");
        let device = unsafe { &*owner.as_raw().cast::<Device>() };
        if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
            let address = unsafe { this.native.GetGPUVirtualAddress() } as usize;
            views::TextureAddress::at(device, &this.native, address, false, None)
        } else {
            views::TextureAddress::register(device, &this.native, &desc)
        }
    }).base as u64
}

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

unsafe extern "system" fn create_heap(
    this: &Device,
    desc: *const D3D12_DESCRIPTOR_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12DescriptorHeap>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.NumDescriptors == 0 {
        return E_INVALIDARG;
    }
    if !matches!(
        desc.Type,
        D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
            | D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
            | D3D12_DESCRIPTOR_HEAP_TYPE_RTV
            | D3D12_DESCRIPTOR_HEAP_TYPE_DSV
    ) || !matches!(
        desc.Flags,
        D3D12_DESCRIPTOR_HEAP_FLAG_NONE | D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
    ) || desc.NodeMask > 1
    {
        return E_NOTIMPL;
    }
    if desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && matches!(
            desc.Type,
            D3D12_DESCRIPTOR_HEAP_TYPE_RTV | D3D12_DESCRIPTOR_HEAP_TYPE_DSV
        )
    {
        return E_INVALIDARG;
    }
    let limit = if desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        && desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && desc.NumDescriptors > 1_000_000 {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS19::default();
        unsafe { this.native.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS19,
            (&raw mut options).cast(), size_of_val(&options) as u32) }
            .map_or(1_000_000, |_| options.MaxViewDescriptorHeapSize.max(1_000_000))
    } else { desc.NumDescriptors };
    let needs_inline = this.state.device11_queue && desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    let (native, arena) = if desc.NumDescriptors > limit || needs_inline {
        match descriptor_paging::Arena::create(this, desc, limit) {
            Ok(arena) => (arena.visible().clone(), Some(arena)),
            Err(error) => return error.code(),
        }
    } else {
        match unsafe { this.native.CreateDescriptorHeap::<ID3D12DescriptorHeap>(desc) } {
            Ok(native) => (native, None),
            Err(error) => return error.code(),
        }
    };
    let resident_desc = D3D12_DESCRIPTOR_HEAP_DESC { NumDescriptors: limit, ..*desc };
    let placed_views = this.state.placements.views.heap(
        &this.native,
        &native,
        &resident_desc,
        D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    );
    let state = DescriptorHeapState {
        desc: *desc,
        cpu_start: arena.as_ref().map_or_else(|| unsafe { native.GetCPUDescriptorHandleForHeapStart().ptr }, |arena| arena.base()),
        arena,
        placed_views,
        _placed_targets: this.state.placements.targets.heap(&this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
        _imported_views: this.state.imported_views.heap(&this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
        _scene_views: this.state.blas.views.heap(
            &this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
        ),
        _depth_views: this.state.depth_views.heap(
            &this.native,
            &native,
            &resident_desc,
            D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
        ),
    };
    unsafe { wrap_pageable(Ok(native), &HEAP, this, state, out) }
}

unsafe extern "system" fn increment_size(this: &Device, kind: D3D12_DESCRIPTOR_HEAP_TYPE) -> u32 {
    unsafe { this.native.GetDescriptorHandleIncrementSize(kind) }
}

pub(crate) const BUFFER_READ_BITS: i32 =
    D3D12_RESOURCE_STATE_VERTEX_AND_CONSTANT_BUFFER.0 | D3D12_RESOURCE_STATE_INDEX_BUFFER.0;

pub(crate) fn host_state(state: D3D12_RESOURCE_STATES, texture: bool) -> D3D12_RESOURCE_STATES {
    // Frosty's SetSRVs emits these two tagged read states, including on compute lists.
    let bits = match state.0 {
        0x1200_00c0 => 0xc0,
        0x1200_0040 => 0x40,
        other => other,
    };
    D3D12_RESOURCE_STATES(if texture {
        bits & !BUFFER_READ_BITS
    } else {
        bits
    })
}

unsafe extern "system" fn create_resource<const EXTENDED: bool>(
    this: &Device,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    guest_desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let (Some(heap), Some(desc)) = (unsafe { (properties.as_ref(), guest_desc.as_ref()) }) else {
        return E_INVALIDARG;
    };
    if !matches!(flags, D3D12_HEAP_FLAG_NONE | D3D12_HEAP_FLAG_ALLOW_DISPLAY)
        || !matches!(
            heap.Type,
            D3D12_HEAP_TYPE_DEFAULT | D3D12_HEAP_TYPE_READBACK | D3D12_HEAP_TYPE_UPLOAD
        )
        || !matches!(
            desc.Dimension,
            D3D12_RESOURCE_DIMENSION_BUFFER
                | D3D12_RESOURCE_DIMENSION_TEXTURE1D
                | D3D12_RESOURCE_DIMENSION_TEXTURE2D
                | D3D12_RESOURCE_DIMENSION_TEXTURE3D
        )
    {
        return E_NOTIMPL;
    }
    if flags == D3D12_HEAP_FLAG_ALLOW_DISPLAY
        && (heap.Type != D3D12_HEAP_TYPE_DEFAULT
            || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D)
    {
        return E_INVALIDARG;
    }
    let mut host_desc = *desc;
    let mut host_heap = *heap;
    let linear_readback = heap.Type == D3D12_HEAP_TYPE_READBACK
        && desc.Layout.0 == 0x100
        && desc.Flags.0 == 0x300001
        && desc.MipLevels == 1
        && desc.DepthOrArraySize == 1
        && placement::color_plane(desc).is_ok();
    if linear_readback {
        // Desktop READBACK heaps accept only buffers. A custom L0 heap can
        // host a CPU-readable color image while retaining its render-target view.
        host_heap.Type = D3D12_HEAP_TYPE_CUSTOM;
        host_heap.CPUPageProperty = D3D12_CPU_PAGE_PROPERTY_WRITE_BACK;
        host_heap.MemoryPoolPreference = D3D12_MEMORY_POOL_L0;
        host_desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
        host_desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    }
    if EXTENDED {
        // Slot 76 extends the resource description at byte 52 (desktop padding).
        // The Scarlett upscaler writes DCC options there and a zero qword at 56.
        const _: () = assert!(core::mem::offset_of!(D3D12_RESOURCE_DESC, Flags) == 48);
        let bytes = guest_desc.cast::<u8>();
        let (dcc, extension) = unsafe {
            (
                bytes.add(52).cast::<u32>().read_unaligned(),
                bytes.add(56).cast::<u64>().read_unaligned(),
            )
        };
        if !matches!(dcc, 0 | 1 | 3) || extension != 0 {
            return E_NOTIMPL;
        }
        use allocation::DCC_FLAGS;
        if desc.Layout.0 == 0x101 || desc.Flags.0 & DCC_FLAGS != 0 {
            // A private committed texture exposes its logical contents through
            // D3D12. The host driver chooses its tiling and compression.
            if flags != D3D12_HEAP_FLAG_NONE
                || heap.Type != D3D12_HEAP_TYPE_DEFAULT
                || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                || desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER)
            {
                return E_NOTIMPL;
            }
            if desc.Layout.0 == 0x101 {
                host_desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
            }
            host_desc.Flags.0 &= !DCC_FLAGS;
        }
    }
    host_desc.Flags.0 &= format::HOST_FLAGS;
    let primary = depth::primary(desc);
    let initial = format::host_initial_state(desc, initial);
    if primary.is_some()
        && (flags != D3D12_HEAP_FLAG_NONE
            || heap.Type != D3D12_HEAP_TYPE_DEFAULT
            || initial.contains(D3D12_RESOURCE_STATE_UNORDERED_ACCESS))
    {
        return E_NOTIMPL;
    }
    let host_clear = format::host_clear(clear);
    let mut native = None;
    if let Err(error) = unsafe {
        this.native.CreateCommittedResource::<ID3D12Resource>(
            &host_heap,
            flags,
            primary.as_ref().unwrap_or(&host_desc),
            host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER),
            Some(host_clear.as_ref().map_or(clear, core::ptr::from_ref)),
            &mut native,
        )
    } {
        return error.code();
    }
    if linear_readback {
        return unsafe { depth::wrap::<true>(this, heap, desc, None, initial, native, out) };
    }
    unsafe { depth::wrap::<EXTENDED>(this, heap, desc, primary.as_ref(), initial, native, out) }
}

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

unsafe fn make_queue(this: &Device, desc: &D3D12_COMMAND_QUEUE_DESC, out: Out) -> HRESULT {
    match unsafe { this.native.CreateCommandQueue(desc) } {
        Ok(native) => unsafe { wrap_command_queue(native, &identity(this), this.state.device11_queue, out) },
        Err(error) => error.code(),
    }
}

fn supported_queue_kind(kind: D3D12_COMMAND_LIST_TYPE) -> bool {
    matches!(
        kind,
        D3D12_COMMAND_LIST_TYPE_DIRECT
            | D3D12_COMMAND_LIST_TYPE_COMPUTE
            | D3D12_COMMAND_LIST_TYPE_COPY
    )
}

fn unsupported_queue(slot: u32, words: [u32; 4]) -> HRESULT {
    use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
    use windows::core::PCSTR;
    let message = format!(
        "Project TuxBox: unsupported queue descriptor at slot {slot}, words {words:x?}\n\0"
    );
    unsafe { OutputDebugStringA(PCSTR(message.as_ptr())) };
    E_NOTIMPL
}

unsafe extern "system" fn create_queue(
    this: &Device,
    desc: *const D3D12_COMMAND_QUEUE_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandQueue>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if !supported_queue_kind(desc.Type)
        || !matches!(
            D3D12_COMMAND_QUEUE_PRIORITY(desc.Priority),
            D3D12_COMMAND_QUEUE_PRIORITY_NORMAL
                | D3D12_COMMAND_QUEUE_PRIORITY_HIGH
                | D3D12_COMMAND_QUEUE_PRIORITY_GLOBAL_REALTIME
        )
        || !matches!(
            desc.Flags,
            D3D12_COMMAND_QUEUE_FLAG_NONE | D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT
        )
        || desc.NodeMask > 1
    {
        return unsupported_queue(
            8,
            [
                desc.Type.0 as u32,
                desc.Priority as u32,
                desc.Flags.0 as u32,
                desc.NodeMask,
            ],
        );
    }
    unsafe { make_queue(this, desc, out) }
}

// Slot 48's field order differs from the desktop queue descriptor.
#[repr(C)]
struct ComputeQueueDesc {
    kind: D3D12_COMMAND_LIST_TYPE,
    options: u32,
    queue_index: u32,
    engine_index: u32,
}

unsafe extern "system" fn create_queue_x(
    this: &Device,
    desc: *const ComputeQueueDesc,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12CommandQueue>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.kind != D3D12_COMMAND_LIST_TYPE_COMPUTE
        || desc.options != 0
        || desc.queue_index > 3
    {
        return unsupported_queue(
            48,
            [
                desc.kind.0 as u32,
                desc.options,
                desc.queue_index,
                desc.engine_index,
            ],
        );
    }
    let native = D3D12_COMMAND_QUEUE_DESC {
        Type: D3D12_COMMAND_LIST_TYPE_COMPUTE,
        // The guest selects a logical compute queue on this adapter. Each
        // creation already returns a separate host command queue.
        ..Default::default()
    };
    unsafe { make_queue(this, &native, out) }
}

#[repr(C)]
struct ListDesc {
    kind: D3D12_COMMAND_LIST_TYPE,
    reserved1: u32,
    node_mask: u32,
    reserved3: u32,
}

const _: () = assert!(size_of::<ListDesc>() == 16 && size_of::<ComputeQueueDesc>() == 16);

unsafe extern "system" fn create_list_x(
    this: &Device,
    desc: *const ListDesc,
    guest_allocator: *mut c_void,
    initial: *mut c_void,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let compact = unsafe { iid.as_ref() } == Some(&media::DMA_IID);
    let extended = unsafe { iid.as_ref() } == Some(&crate::commands::LIST6);
    let host_iid = ID3D12GraphicsCommandList::IID;
    let status = unsafe {
        check_output::<ID3D12GraphicsCommandList>(if compact || extended { &host_iid } else { iid }, out)
    };
    if status.is_err() {
        return status;
    }
    if compact
        && unsafe { desc.as_ref() }.is_none_or(|desc| desc.kind != D3D12_COMMAND_LIST_TYPE_COPY)
    {
        return windows::Win32::Foundation::E_NOINTERFACE;
    }
    let parent = identity(this);
    let (Some(desc), Some(allocator)) =
        (unsafe { (desc.as_ref(), allocator(guest_allocator, parent.as_raw())) })
    else {
        return E_INVALIDARG;
    };
    // Electra's compact DMA list at RVA 0x19849c3 selects the default node
    // with {COPY,0,0,0}. Both default and explicit node one target node zero.
    let valid_node = desc.node_mask == 1 || (compact && desc.node_mask == 0);
    // GTA creates standard graphics lists with the console recording flags
    // 0x6. The host owns the command stream storage in either mode.
    let supported_flags = desc.reserved1 == 0 || (!compact && desc.reserved1 == 6)
        || (extended && desc.reserved1 == 2);
    if !supported_queue_kind(desc.kind) || !supported_flags || !valid_node || desc.reserved3 != 0
    {
        return E_NOTIMPL;
    }
    // SAFETY: the caller keeps its optional pipeline alive throughout creation.
    let initial = match unsafe { compute::pipeline(initial, parent.as_raw()) } {
        Ok(initial) => initial,
        Err(error) => return error,
    };
    let list: ID3D12GraphicsCommandList = match unsafe {
        this.native.CreateCommandList(
            desc.node_mask,
            desc.kind,
            &allocator.native,
            initial.map(|p| &p.native),
        )
    } {
        Ok(list) => list,
        Err(error) => return error.code(),
    };
    // Hold an empty native recording ready for a close-first guest, releasing
    // the allocator so reset-first callers can rewind it before their list.
    if let Err(error) = unsafe { list.Close() } {
        return error.code();
    }
    unsafe {
        wrap_command_list(
            list,
            allocator,
            initial,
            &parent,
            compact,
            extended,
            this.state.device11_queue,
            out,
        )
    }
}

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
