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
mod cb_blend;
mod compile_budget;
mod plane_size;
pub(crate) mod alloc_swap;
mod fallback;
pub(crate) mod blas;
pub(crate) mod compute;
pub(crate) mod committed_writeback;
mod compute_scene;
mod compute_table;
mod mesh_table;
pub(crate) mod depth;
mod descriptor_copy;
pub(crate) mod desc_table;
pub(crate) mod view_formats;
pub(crate) mod descriptor_paging;
mod dxgi;
mod features;
pub(crate) mod format;
pub(crate) mod frame_events;
mod fence;
pub(crate) use fence::hang_lines as fence_hang_lines;
mod graphics;
mod heap;
mod reserved;
mod derived;
pub(crate) mod indirect;
pub(crate) mod media;
pub(crate) mod placement;
pub(crate) mod queries;
pub(crate) mod ray_build;
mod ray_compile;
mod ray_pipeline;
pub(crate) mod rectangle;
pub(crate) mod shader_cache;
mod state_object;
pub(crate) mod stencil;
pub(crate) mod null_kinds;
pub(crate) mod pipeline_cache;
mod stream;
mod title_memory;
pub(crate) use title_memory::{commit_lazy, view_state};
mod views;
pub(crate) use views::{clear_buffer_alias, command_buffer_address, host_cpu_descriptor, is_constant_buffer, is_null_rtv, refresh_shadows, note_shadow_write, flush_desc_table, desc_table_address, view_formats_address, write_back_shadows};

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
    fn render_heap(&self) -> Option<&std::sync::Arc<heap::RenderHeap>> {
        match &self.backing {
            Backing::Addressed(address, _) => address.render_heap(),
            _ => None,
        }
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

/// Xbox lets several lists record from one allocator at once; desktop D3D12
/// refuses the second list's Reset with E_INVALIDARG (the title asserts on it).
/// Extra host allocators per guest allocator take those lists and are reset
/// together with it.
struct Overflow(usize, ID3D12CommandAllocator);
// SAFETY: D3D12 allocators are free-threaded objects; the guest serialises
// their use exactly as it does for the primary allocator.
unsafe impl Send for Overflow {}
static OVERFLOW: std::sync::Mutex<Vec<Overflow>> = std::sync::Mutex::new(Vec::new());

/// Host allocators already added for `guest` beyond its primary one.
/// Dead shadows and cached imports over [lo, hi) released (views.rs).
pub(crate) fn release_shadows_over(lo: u64, hi: u64) -> usize {
    views::release_shadows_over(lo, hi)
}

pub(crate) fn overflow_allocators(guest: &Allocator) -> Vec<ID3D12CommandAllocator> {
    let key = guest as *const Allocator as usize;
    let table = OVERFLOW.lock().unwrap_or_else(|e| e.into_inner());
    table.iter().filter(|o| o.0 == key).map(|o| o.1.clone()).collect()
}

/// A new host allocator of `kind` for `guest`, kept until the guest is released.
pub(crate) fn add_overflow_allocator(guest: &Allocator, kind: D3D12_COMMAND_LIST_TYPE) -> windows::core::Result<ID3D12CommandAllocator> {
    let mut device: Option<ID3D12Device> = None;
    unsafe { guest.native.GetDevice(&mut device)? };
    let allocator: ID3D12CommandAllocator = unsafe { device.ok_or(windows::Win32::Foundation::E_FAIL)?.CreateCommandAllocator(kind)? };
    let key = guest as *const Allocator as usize;
    let mut table = OVERFLOW.lock().unwrap_or_else(|e| e.into_inner());
    table.push(Overflow(key, allocator.clone()));
    let count = table.iter().filter(|o| o.0 == key).count();
    drop(table);
    crate::diagnostics::log_line(format_args!(
        "allocator {key:#x} shared by recording lists: added host allocator #{count}"
    ));
    Ok(allocator)
}

unsafe extern "system" fn reset_allocator(this: &Allocator) -> HRESULT {
    let mut sources = this.state.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(status) = alloc_swap::reset(this) {
        if status.is_ok() {
            sources.clear();
        }
        return status;
    }
    let current = alloc_swap::current(this);
    let host = &current;
    let status = unsafe { (host.vtable().Reset)(host.as_raw()) };
    if status.is_ok() {
        sources.clear();
        // An overflow allocator still bound to an open list refuses; it is
        // reset on a later guest reset instead.
        for extra in overflow_allocators(this) {
            let _ = unsafe { extra.Reset() };
        }
    }
    status
}

// The Xbox callback supplies additional command-buffer pages on demand.
// Native D3D12 allocators obtain their own storage, so recording translated
// commands never requests a guest page.
/// Allocator slot 9. It returned nothing, so the title read whatever RAX held.
/// leonida-ce 6e02c937 (GPL-3): the title calls it in a loop over its blocks (a3 steps
/// by 0x30) and retried about 2.5 million times while the answer was a failure. It
/// answers S_OK now; the first calls are logged (20 lines at most).
/// TUXBOX_ALLOC_SLOT9_OK=0 answers E_NOTIMPL instead.
extern "system" fn set_allocator_callback(_: &Allocator, a2: *const c_void, a3: *mut c_void) -> HRESULT {
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let ok = crate::env_toggle!("TUXBOX_ALLOC_SLOT9_OK") != Some("0");
    if n <= 4 || (n.is_power_of_two() && n < 1 << 16) {
        crate::diagnostics::log_line(format_args!(
            "allocator slot 9 call {n}: a2={a2:p} a3={a3:p}, answered {} (leonida-ce 6e02c937; TUXBOX_ALLOC_SLOT9_OK=0 E_NOTIMPL)",
            if ok { "S_OK" } else { "E_NOTIMPL" }));
    }
    if ok { windows::Win32::Foundation::S_OK } else { windows::Win32::Foundation::E_NOTIMPL }
}

/// Last Release: forget the guest's host allocators (alloc_swap::released) and
/// its OVERFLOW entries, which are keyed by address and would otherwise pass
/// to a later allocator at the same address.
unsafe extern "system" fn release_allocator(this: *mut Allocator) -> u32 {
    unsafe {
        crate::proxy::release_with(this, || {
            if !alloc_swap::release_enabled() {
                return;
            }
            let guest = &*this;
            let key = this as usize;
            let overflow: Vec<ID3D12CommandAllocator> = {
                let mut table = OVERFLOW.lock().unwrap_or_else(|e| e.into_inner());
                let mut gone = Vec::new();
                table.retain(|o| if o.0 == key { gone.push(o.1.clone()); false } else { true });
                gone
            };
            alloc_swap::released(guest, overflow);
        })
    }
}

static ALLOCATOR: Slots<16> = {
    let mut slots = child_slots::<ID3D12CommandAllocator, MediaResources, 16>();
    slots[2] = crate::slot!(release_allocator);
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
    if !pc_mappable(host) {
        return E_INVALIDARG;
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

/// Marks a reserved resource from its creation (xcl cf244b0/7d834d4 idea:
/// GetHeapProperties on one is an invalid call too, debug layer 901). The
/// mark is private data, so it goes with the object and a later resource at
/// the same address is never taken for it.
const RESERVED_MARK: windows::core::GUID = windows::core::GUID::from_u128(0x7b0e_52c1_3a4f_4d2e_9f61_8c3d_5a2b_e914);

pub(crate) fn mark_reserved(resource: &ID3D12Resource) {
    let one = 1u8;
    let _ = unsafe { resource.SetPrivateData(&RESERVED_MARK, 1, Some(core::ptr::from_ref(&one).cast())) };
}

fn is_reserved(resource: &ID3D12Resource) -> bool {
    let (mut size, mut value) = (1u32, 0u8);
    unsafe { resource.GetPrivateData(&RESERVED_MARK, &mut size, Some(core::ptr::from_mut(&mut value).cast())) }.is_ok()
}

/// xcl's idea (21de1a8): a resource in a DEFAULT heap, a custom heap without
/// CPU pages, or a reserved one (no heap) has no CPU view on the PC, and its
/// Map is an invalid call (debug layer 822 / 824) the runtime answers with
/// E_INVALIDARG. Those get E_INVALIDARG without reaching it.
/// TUXBOX_MAP_HEAP_CHECK=0 always calls the PC's Map.
fn pc_mappable(host: &ID3D12Resource) -> bool {
    if crate::env_toggle!("TUXBOX_MAP_HEAP_CHECK") == Some("0") {
        return true;
    }
    let mut props = D3D12_HEAP_PROPERTIES::default();
    let mappable = !is_reserved(host)
        && unsafe { host.GetHeapProperties(Some(&mut props), None) }.is_ok()
        && match props.Type {
            D3D12_HEAP_TYPE_DEFAULT => false,
            D3D12_HEAP_TYPE_CUSTOM => props.CPUPageProperty != D3D12_CPU_PAGE_PROPERTY_NOT_AVAILABLE,
            _ => true,
        };
    if !mappable {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "Map of a resource with no CPU pages (heap type {}) answered E_INVALIDARG without the PC's Map ({n} so far; TUXBOX_MAP_HEAP_CHECK=0 off)", props.Type.0));
        }
    }
    mappable
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
            // Its host GPU address is not CPU memory; see whether the title
            // ever reads one (palmetto e604c28a follow-up).
            static HOST_VA: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = HOST_VA.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 32 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "committed buffer {:p} ({} bytes) hands the title its host GPU address {address:#x}, not CPU memory ({n} such buffers)",
                    this.native.as_raw(), desc.Width));
            }
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
    _samplers: Option<desc_table::HeapGuard>,
    _view_formats: Option<view_formats::HeapGuard>,
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
    let proxy: Option<&Resource> = unsafe { unwrap(object, device, &RESOURCE) };
    // Every command taking a resource unwraps it here (heap::residency).
    if let Some(heap) = proxy.and_then(|p| p.state.render_heap()) {
        heap.touch();
    }
    proxy
}

/// A guest heap's native heap (tile mappings, TuxBox aeed457).
pub(crate) unsafe fn memory_heap<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a ID3D12Heap> {
    unsafe { heap::memory_heap(object, device) }
}

/// Reserved resources and tile mappings are on (TUXBOX_RESERVED).
pub(crate) fn reserved_on() -> bool {
    reserved::on()
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

/// leonida-ce 1403e90e: a refused descriptor heap is logged with its
/// description, the host increment and the step that refused it.
/// TUXBOX_HEAP_FAIL_LOG=0 off.
unsafe extern "system" fn create_heap(
    this: &Device,
    desc: *const D3D12_DESCRIPTOR_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let result = unsafe { create_heap_logged(this, desc, iid, out) };
    if result.is_err() && crate::env_toggle!("TUXBOX_HEAP_FAIL_LOG") != Some("0") {
        static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 16 || n.is_power_of_two() {
            match unsafe { desc.as_ref() } {
                Some(d) => {
                    let increment = if d.Type.0 <= D3D12_DESCRIPTOR_HEAP_TYPE_DSV.0 && d.Type.0 >= 0 {
                        unsafe { this.native.GetDescriptorHandleIncrementSize(d.Type) }
                    } else { 0 };
                    crate::diagnostics::log_line(format_args!(
                        "CreateDescriptorHeap failed {:#010x}: type {} count {} flags {:#x} mask {:#x} (increment {increment}, {} bytes; {n} so far)",
                        result.0 as u32, d.Type.0, d.NumDescriptors, d.Flags.0, d.NodeMask,
                        d.NumDescriptors as u64 * increment as u64));
                }
                None => crate::diagnostics::log_line(format_args!(
                    "CreateDescriptorHeap failed {:#010x}: no description ({n} so far)", result.0 as u32)),
            }
        }
    }
    result
}

unsafe fn create_heap_logged(
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
        // Shader-visible sampler heaps get the D3D12 maximum: the slots above
        // the title's hold host samplers for literal S# words (desc_table).
        let visible_samplers = D3D12_DESCRIPTOR_HEAP_DESC { NumDescriptors: desc_table::SAMPLER_HEAP_SLOTS, ..*desc };
        let desc = if desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
            && desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
            && desc.NumDescriptors <= desc_table::SAMPLER_HEAP_SLOTS { &visible_samplers } else { desc };
        match unsafe { this.native.CreateDescriptorHeap::<ID3D12DescriptorHeap>(desc) } {
            Ok(native) => {
                // Host contents of unwritten slots are undefined.
                if desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV {
                    null_kinds::fill(&this.native, &native, desc.NumDescriptors, desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE);
                }
                (native, None)
            }
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
    let cpu_start = arena.as_ref().map_or_else(|| unsafe { native.GetCPUDescriptorHandleForHeapStart().ptr }, |arena| arena.base());
    let samplers = (desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER).then(|| this.state.desc_table.sampler_heap(
        &this.native, cpu_start, desc.NumDescriptors, desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
    ));
    // Typed view codes of a released heap's handles (and its table) go with it.
    let view_formats = (desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV).then(|| {
        let stride = unsafe { this.native.GetDescriptorHandleIncrementSize(desc.Type) } as usize;
        let native_start = unsafe { native.GetCPUDescriptorHandleForHeapStart() }.ptr;
        let native_count = unsafe { native.GetDesc() }.NumDescriptors as usize;
        let mut ranges = vec![native_start..native_start + native_count * stride];
        if let Some(arena) = &arena {
            let staging = arena.cpu(arena.base()).ptr;
            ranges.push(staging..staging + desc.NumDescriptors as usize * stride);
        }
        view_formats::HeapGuard::new(ranges)
    });
    let state = DescriptorHeapState {
        desc: *desc,
        cpu_start,
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
        _samplers: samplers,
        _view_formats: view_formats,
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

/// Desktop D3D12 takes a committed resource on an UPLOAD heap only in GENERIC_READ
/// and on a READBACK heap only in COPY_DEST; any other initial state, COMMON
/// included, is E_INVALIDARG. Other heaps (and the custom L0 heap of a linear
/// readback image) keep the title's state. Ported from X2W 3556bc28 (lukaslw-lab,
/// MIT). TUXBOX_COMMITTED_HEAP_STATE=0 passes the title's state through.
fn committed_state(heap: D3D12_HEAP_TYPE, state: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_STATES {
    if crate::env_toggle!("TUXBOX_COMMITTED_HEAP_STATE") == Some("0") {
        return state;
    }
    match heap {
        D3D12_HEAP_TYPE_UPLOAD => D3D12_RESOURCE_STATE_GENERIC_READ,
        D3D12_HEAP_TYPE_READBACK => D3D12_RESOURCE_STATE_COPY_DEST,
        _ => state,
    }
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
            committed_state(host_heap.Type, host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER)),
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
    // Console-only queue flags have no host meaning; the desktop's one flag
    // (DISABLE_GPU_TIMEOUT) is kept (xbox-emulator #338).
    let masked = D3D12_COMMAND_QUEUE_DESC {
        Flags: D3D12_COMMAND_QUEUE_FLAGS(desc.Flags.0 & D3D12_COMMAND_QUEUE_FLAG_DISABLE_GPU_TIMEOUT.0),
        ..*desc
    };
    if masked.Flags != desc.Flags {
        static DROPPED: std::sync::Once = std::sync::Once::new();
        let flags = desc.Flags.0;
        crate::diagnostics::log_once(&DROPPED, || format!("queue flags {flags:#x}: console-only bits dropped"));
    }
    let desc = &masked;
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

/// Device slot 109, a console extension called as (this, out) whose
/// measured callers read back one dword (palmetto 57767366, re-derived).
/// Zero is published only into a caller stack slot: some call sites pass an
/// object field, and a dword written there would cut a pointer in half.
/// TUXBOX_SLOT109=0 leaves it unmapped (answers 0, writes nothing).
unsafe extern "system" fn slot109(this: usize, out: usize, a: usize, b: usize) -> i32 {
    let frame = core::ptr::addr_of!(out) as usize;
    let on = crate::env_toggle!("TUXBOX_SLOT109") != Some("0");
    let written = on && stack_slot(frame, out);
    if written {
        // A dword on the caller's own stack, within 16 MiB of this frame.
        unsafe { (out as *mut u32).write_unaligned(0) };
    }
    static CALLS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
        crate::diagnostics::log_line(format_args!(
            "device slot 109 ({this:#x}, {out:#x}, {a:#x}, {b:#x}) from title RVA {:#x}: {}",
            crate::survey::title_rva().unwrap_or(0),
            if written { "zero published to the caller's stack slot" } else { "nothing written (not a stack address)" }));
    }
    0
}

fn stack_slot(frame: usize, address: usize) -> bool {
    address != 0 && address.abs_diff(frame) < 16 << 20
}

#[cfg(test)]
#[test]
fn slot109_writes_only_near_its_own_frame() {
    assert!(stack_slot(0x7ff_e000, 0x7ff_e100));
    assert!(stack_slot(0x7ff_e000, 0x7ff_e000 - 0x10_0000));
    assert!(!stack_slot(0x7ff_e000, 0x2_0000_0000));
    assert!(!stack_slot(0x7ff_e000, 0));
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
            initial.map(|p| compute::current(p).0),
        )
    } {
        Ok(list) => list,
        Err(error) => return error.code(),
    };
    crate::dred::name(&list, "title");
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

/// CreateDerivedPipelineStateX(base, patch count, patches, riid, out): the
/// base pipeline with 0x14C-byte tagged patches (part 0 depth-stencil, 1
/// rasterizer, 2 blend, 3 sample mask; xcl device_x.rs). Fallback until the
/// base descriptions are kept: the base pipeline itself (patches ignored,
/// logged), so the title gets a working pipeline instead of a null one.
/// TUXBOX_DERIVED_PSO=fail answers E_FAIL.
unsafe extern "system" fn derived_pipeline(
    device: &Device, base: *mut c_void, count: u32, patches: *const u8, riid: *const GUID, out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(core::ptr::null_mut()) };
    if base.is_null() || riid.is_null() || crate::env_toggle!("TUXBOX_DERIVED_PSO") == Some("fail") {
        return windows::Win32::Foundation::E_FAIL;
    }
    if let Some(status) = unsafe { derived::create(device, base, count, patches, riid, out) } {
        return status;
    }
    static LOGGED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
        let parts: Vec<u32> = (0..count.min(8) as usize)
            .filter(|_| !patches.is_null())
            .map(|i| unsafe { patches.add(i * 0x14C).cast::<u32>().read_unaligned() })
            .collect();
        crate::diagnostics::log_line(format_args!(
            "CreateDerivedPipelineStateX: base {base:p}, {count} patch(es) parts {parts:?} ignored; the base pipeline is returned"));
    }
    type Query = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT;
    // SAFETY: base is a live COM object (our pipeline proxy); slot 0 is QueryInterface.
    unsafe { (*base.cast::<*const Query>()).read()(base, riid, out) }
}

// 192 slots, past the title's highest device slot (Leonida's retail boot chain):
// a shorter table hands the title a null pointer instead of a logged E_NOTIMPL.
static DEVICE: Slots<192> = {
    let mut slots = object_slots::<ID3D12Device, dxgi::DxgiDevice, 192>();
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
    slots[25] = crate::slot!(allocation::info_watched);
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
    slots[55] = crate::slot!(derived_pipeline);
    slots[57] = crate::slot!(compute::deserialize_pipeline_x::<57>);
    slots[60] = crate::slot!(graphics::create_pipeline);
    slots[61] = crate::slot!(views::sampler_x);
    slots[62] = crate::slot!(compute::create_pipeline_x);
    slots[67] = crate::slot!(compute::deserialize_pipeline_x::<67>);
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
    slots[108] = crate::slot!(allocation::info2_watched);
    slots[109] = crate::slot!(slot109);
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
    ID3D12Device12::IID,
];

/// TUXBOX_DEVICE12=0: the device stops at ID3D12Device11, as before.
fn answers_device12() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_DEVICE12").is_ok_and(|v| v.trim() == "0"))
}

#[cfg(test)]
mod device_levels {
    use super::*;

    /// Every level the title may hold the device by (GTA VI: Device11), by
    /// its console IID where it differs (Device8), in order (VIctory 717f322).
    #[test]
    fn the_device_answers_every_level() {
        let console: [u128; 10] = [
            0x189819f1_1db6_4b57_be54_1821339b85f7,
            0x77acce80_638e_4e65_8895_c1f23386863e,
            0x30baa41e_b15b_475c_a0bb_1af5c5b64328,
            0x81dadc15_2bad_4392_93c5_101345c4aa98,
            0xe865df17_a9ee_46f9_a463_3098315aa2e5,
            0x8b4f173b_2fea_4b80_8f58_4307191ab95d,
            0xc70b221b_40e4_4a17_89af_025a0727a6dc,
            0x5c014b53_68a1_4b9b_8bd1_dd6046b9358b,
            0x728f1fa2_ea11_4a9f_9b52_9b42a484d5c9,
            0x5405c344_d457_444e_b4dd_2366e45aee39,
        ];
        for (level, iid) in console.iter().enumerate() {
            assert_eq!(DEVICE_INTERFACES[level], GUID::from_u128(*iid), "level {level}");
        }
        assert!(DEVICE_INTERFACES.contains(&ID3D12Device10::IID));
        assert!(DEVICE_INTERFACES.contains(&ID3D12Device11::IID));
        assert!(DEVICE_INTERFACES.contains(&ID3D12Device12::IID));
    }
}

/// # Safety
/// `adapter` must be null or a borrowed DXGI adapter. The caller must supply
/// a cleared, writable output pointer for the new guest device reference.
pub(crate) unsafe fn create(adapter: *mut c_void, requested: GUID, out: Out) -> HRESULT {
    crate::dred::select_sdk();
    crate::dred::configure();
    crate::diagnostics::crash::install();
    crate::diagnostics::ctl::install();
    crate::diagnostics::teb::run();
    // Audio endpoint probe as early as we run: the title may open its endpoint before the first frame.
    let _ = std::thread::Builder::new().name("TuxBox audio probe".into()).spawn(crate::diagnostics::audio::install_wasapi);
    // A null adapter picks the high-performance GPU (or TUXBOX_ADAPTER), not
    // D3D12's adapter 0, which is the integrated GPU on many laptops.
    let chosen = if adapter.is_null() { adapters::choose() } else { None };
    let mut adapter = chosen.as_ref().map_or(adapter, |a| a.as_raw());
    let make = |adapter: *mut c_void| -> windows::core::Result<Option<ID3D12Device>> {
        // SAFETY: The public factory validated its arguments; the adapter stays borrowed.
        // A failed debug device falls through to the normal one (test88).
        if let Some(Ok(created)) = crate::dred::debug_device(unsafe { IUnknown::from_raw_borrowed(&adapter) }) {
            return Ok(Some(created));
        }
        let mut native = None;
        unsafe { D3D12CreateDevice::<_, ID3D12Device>(IUnknown::from_raw_borrowed(&adapter), D3D_FEATURE_LEVEL_12_0, &mut native)? };
        Ok(native)
    };
    let warp;
    let native = match make(adapter) {
        Ok(native) => native,
        // No usable hardware device: the WARP software rasterizer, slow but
        // running (TUXBOX_WARP=0 fails as before).
        // A device refused under the Agility SDK (test91: INVALID_REDIST) is
        // the SDK's fault, and the title dies on WARP: report it instead.
        Err(error) if crate::dred::sdk_selected() => {
            crate::diagnostics::log_line(format_args!(
                "D3D12CreateDevice failed {:?} under the Agility SDK chosen by SetSDKVersion; not falling back to WARP. Unset TUXBOX_D3D12_SDK_GLOBAL (or TUXBOX_D3D12_SDK)", error.code()));
            return error.code();
        }
        Err(error) => match adapters::warp() {
            Some(software) => {
                crate::diagnostics::log_line(format_args!("D3D12CreateDevice failed {:?}; falling back to WARP", error.code()));
                warp = software;
                adapter = warp.as_raw();
                match make(adapter) { Ok(native) => native, Err(_) => return error.code() }
            }
            None => return error.code(),
        },
    };
    let Some(native) = native else {
        return E_POINTER;
    };
    crate::diagnostics::device_created(&native);
    crate::dred::watch(&native);
    crate::dred::capture_messages(&native);
    // SAFETY: the caller holds the selected adapter through factory return.
    let pipeline_device = native.cast::<ID3D12Device2>().ok();
    let interfaces: &'static [GUID] = if answers_device12() && native.cast::<ID3D12Device12>().is_ok() {
        DEVICE_INTERFACES
    } else if native.cast::<ID3D12Device11>().is_ok() {
        &DEVICE_INTERFACES[..12]
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
    crate::diagnostics::memory::start(unsafe { native.GetAdapterLuid() });
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
        if status.is_ok() && let Some(d) = unsafe { desc.as_ref() } {
            crate::diagnostics::memory::note(crate::diagnostics::memory::Category::CommittedResource, d.Width * u64::from(d.Height) * u64::from(d.DepthOrArraySize));
        }
        report(if EXTENDED { 76 } else { 27 }, status, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn reserved(
        device: &Device, desc: *const D3D12_RESOURCE_DESC, initial: D3D12_RESOURCE_STATES,
        clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(30, unsafe { super::reserved::create(device, desc, initial, clear, iid, out) }, initial.0, desc)
    }

    pub(super) unsafe extern "system" fn place(
        device: &Device, heap: *mut c_void, offset: u64, desc: *const D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES, clear: *const D3D12_CLEAR_VALUE, iid: *const GUID, out: Out,
    ) -> HRESULT {
        report(29, unsafe { heap::place(device, heap, offset, desc, initial, clear, iid, out) }, initial.0, desc)
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
        let place = || unsafe { heap::place_virtual_layout(device, address, desc, layout, clear, iid, out) };
        let status = retry_out_of_memory(device, 133, place);
        if status.is_ok() && let Some(d) = unsafe { desc.as_ref() } {
            // Description size (texels or buffer bytes), not the allocation.
            crate::diagnostics::memory::note(crate::diagnostics::memory::Category::PlacedLayout,
                d.Width * u64::from(d.Height) * u64::from(d.DepthOrArraySize));
        }
        report(133, status, layout.0, desc)
    }

    /// Out of memory (test39: slot 133 1024x1024 format 98, then the title's
    /// assert): release dead shadow copies and unheld imports and try once
    /// more, logged. TUXBOX_OOM_RETRY=0 answers the first failure.
    pub(super) fn retry_out_of_memory(device: &Device, slot: u32, call: impl Fn() -> HRESULT) -> HRESULT {
        use windows::Win32::Foundation::E_OUTOFMEMORY;
        let status = call();
        if status != E_OUTOFMEMORY || crate::env_toggle!("TUXBOX_OOM_RETRY") == Some("0") {
            return status;
        }
        let released = super::views::release_dead_shadows(device);
        let again = call();
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "resource slot {slot} out of memory: released {released} dead shadows/imports, retry {again:?} ({n} so far; TUXBOX_OOM_RETRY=0 off)"));
        }
        again
    }
}

/// Committed read-write memory inside the console GPU window [2 GiB,
/// 0x1f_e000_0000) (xmem's WINDOW), where title addresses resolve for the
/// GPU; 0 on failure.
/// (live, dead kept) shadow copy bytes, for the memory stats line.
pub(crate) fn shadow_bytes() -> (u64, u64) {
    views::shadow_bytes()
}

pub(crate) fn title_window_alloc(bytes: usize) -> usize {
    use windows::Win32::System::Memory::*;
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut requirements = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: (1usize << 31) as *mut c_void,
        HighestEndingAddress: (0x1f_e000_0000usize - 1) as *mut c_void,
        Alignment: 0,
    };
    let mut param = MEM_EXTENDED_PARAMETER::default();
    param.Anonymous1._bitfield = MemExtendedParameterAddressRequirements.0 as u64;
    param.Anonymous2.Pointer = (&raw mut requirements).cast();
    // SAFETY: the requirement outlives the call; a null address lets the OS choose.
    let base = unsafe {
        VirtualAlloc2(Some(GetCurrentProcess()), None, bytes, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE.0, Some(&mut [param]))
    };
    base as usize
}

mod adapters {
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::Interface;

    /// TUXBOX_ADAPTER=<index> or a name substring; otherwise the
    /// high-performance preference. None leaves D3D12's default.
    pub(super) fn choose() -> Option<IDXGIAdapter1> {
        let factory: IDXGIFactory6 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.ok()?;
        let wanted = std::env::var("TUXBOX_ADAPTER").ok().map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
        let list: Vec<IDXGIAdapter1> = (0..16)
            .map_while(|i| unsafe { factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) }.ok())
            .filter(|a| unsafe { a.GetDesc1() }.is_ok_and(|d| d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0))
            .collect();
        let name = |a: &IDXGIAdapter1| unsafe { a.GetDesc1() }.map(|d| {
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            String::from_utf16_lossy(&d.Description[..n])
        }).unwrap_or_default();
        let pick = match &wanted {
            Some(w) if w.parse::<usize>().is_ok() => list.get(w.parse::<usize>().unwrap()).cloned(),
            Some(w) => list.iter().find(|a| name(a).to_lowercase().contains(&w.to_lowercase())).cloned(),
            None => list.first().cloned(),
        };
        let all: Vec<String> = list.iter().map(name).collect();
        crate::diagnostics::log_line(format_args!("adapters by performance {all:?}; using {:?}{}",
            pick.as_ref().map(name), wanted.map(|w| format!(" (TUXBOX_ADAPTER={w})")).unwrap_or_default()));
        pick
    }

    pub(super) fn warp() -> Option<IDXGIAdapter> {
        if std::env::var("TUXBOX_WARP").is_ok_and(|v| v.trim() == "0") {
            return None;
        }
        let factory: IDXGIFactory4 = unsafe { CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)) }.ok()?;
        unsafe { factory.EnumWarpAdapter::<IDXGIAdapter>() }.ok().map(|a| a.cast().unwrap_or(a))
    }
}
