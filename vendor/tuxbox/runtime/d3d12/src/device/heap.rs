//! Device memory heaps and resources sharing their native GPU allocation.
use super::{Device, Out, RESOURCE, identity, wrap_pageable};
use crate::proxy::{Proxy, Slots, check_output, child_slots, unwrap};
use core::ffi::c_void;
use crate::commands::page_mapping::{GPU_APERTURE, ImportPin};
use std::sync::Arc;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, Interface};

const IMPORTED_HEAP: GUID = GUID::from_u128(0xc2a3c751_1d00_470c_8efd_5da145efd165);

// This holder is also the key for the weak single-page import cache. Its
// last native resource releases both the heap reference and the VA pin.
pub(super) struct ImportedHeap {
    native: ID3D12Heap,
    _pin: ImportPin,
    /// Pages under it were remapped (retire_imports_over): it maps the old ones.
    stale: std::sync::atomic::AtomicBool,
}

impl ImportedHeap {
    fn new(native: ID3D12Heap, pin: ImportPin) -> Self {
        Self { native, _pin: pin, stale: Default::default() }
    }

    /// The title remapped pages under this heap since it was opened.
    pub(super) fn stale(&self) -> bool {
        self.stale.load(std::sync::atomic::Ordering::Acquire)
    }
}

fn attach_import(resource: &ID3D12Resource, heap: &Arc<ImportedHeap>) -> windows::core::Result<()> {
    static MARKER: Slots<8> = child_slots::<ID3D12Heap, Arc<ImportedHeap>, 8>();
    let mut out = core::ptr::null_mut();
    unsafe {
        crate::proxy::allocate(&MARKER, &[], heap.native.clone(), None, heap.clone(), &mut out).ok()?;
        let marker = windows::core::IUnknown::from_raw(out);
        resource.SetPrivateDataInterface(&IMPORTED_HEAP, &marker)
    }
}

type Heap = Proxy<ID3D12Heap, D3D12_HEAP_DESC>;
unsafe extern "system" fn get_desc(heap: &Heap, out: *mut D3D12_HEAP_DESC) -> *mut D3D12_HEAP_DESC {
    unsafe { out.write(heap.state) };
    out
}
// GetDesc is slot 8. A protected-session query and a GPU address follow it
// (TuxBox aeed457): a shorter table would read off the end, so those calls
// fail closed.
static HEAP: Slots<11> = {
    let mut table = child_slots::<ID3D12Heap, D3D12_HEAP_DESC, 11>();
    table[8] = crate::slot!(get_desc);
    table
};

/// The native heap of a guest heap proxy (TuxBox aeed457, tile mappings).
pub(super) unsafe fn memory_heap<'a>(
    object: *mut c_void,
    owner: *mut c_void,
) -> Option<&'a ID3D12Heap> {
    // SAFETY: unwrap checks the heap vtable and the owning device.
    unsafe { unwrap::<ID3D12Heap, D3D12_HEAP_DESC>(object, owner, &HEAP).map(|heap| &heap.native) }
}

/// A guest resource, heap, descriptor heap or query heap as a native
/// pageable (TuxBox aeed457, MakeResident/Evict).
pub(super) unsafe fn pageable(object: *mut c_void, owner: *mut c_void) -> Option<ID3D12Pageable> {
    // SAFETY: each unwrap checks the vtable and the owning device. These four
    // objects are pageable, and the cast keeps a reference for the caller.
    unsafe {
        if let Some(resource) = super::resource(object, owner) {
            resource.cast().ok()
        } else if let Some(heap) = unwrap::<ID3D12Heap, D3D12_HEAP_DESC>(object, owner, &HEAP) {
            heap.native.cast().ok()
        } else if let Some(heap) = super::descriptor_heap(object, owner) {
            heap.native.cast().ok()
        } else if let Some(heap) = super::queries::native(object, owner) {
            heap.cast().ok()
        } else {
            None
        }
    }
}

pub(super) unsafe extern "system" fn create(
    device: &Device,
    desc: *const D3D12_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Heap>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.SizeInBytes == 0 {
        return E_INVALIDARG;
    }
    if !matches!(
        desc.Properties.Type,
        D3D12_HEAP_TYPE_DEFAULT | D3D12_HEAP_TYPE_UPLOAD | D3D12_HEAP_TYPE_READBACK
    ) {
        return E_NOTIMPL;
    }
    let mut native = None;
    if let Err(error) = unsafe { device.native.CreateHeap::<ID3D12Heap>(desc, &mut native) } {
        return error.code();
    }
    let Some(native) = native else {
        return E_POINTER;
    };
    // GetDesc reflects the size and alignment the driver actually granted.
    let actual = unsafe { native.GetDesc() };
    unsafe { wrap_pageable(Ok(native), &HEAP, device, actual, out) }
}

// At RVA 0x57a4b6a Frosty calls Device5 slot 85 with a pageable and
// D3D12_RESIDENCY_PRIORITY_HIGH after allocating an acceleration-structure buffer.
pub(super) unsafe extern "system" fn residency_priority(
    device: &Device,
    count: u32,
    objects: *const *mut c_void,
    priorities: *const D3D12_RESIDENCY_PRIORITY,
) -> HRESULT {
    if count != 0 && (objects.is_null() || priorities.is_null()) {
        return E_INVALIDARG;
    }
    let Some(host) = device.state.pipeline_device.as_ref() else {
        return E_NOTIMPL;
    };
    let owner = identity(device).as_raw();
    let mut native = Vec::new();
    if native.try_reserve_exact(count as usize).is_err() {
        return E_OUTOFMEMORY;
    }
    for i in 0..count as usize {
        // SAFETY: the guest supplies count borrowed COM pointers. Each unwrap
        // verifies our vtable and owning device before accessing a native pointer.
        let object = unsafe { objects.add(i).read() };
        let pageable = unsafe {
            if let Some(resource) = super::resource(object, owner) {
                resource.as_raw()
            } else if let Some(heap) = unwrap::<ID3D12Heap, D3D12_HEAP_DESC>(object, owner, &HEAP) {
                heap.native.as_raw()
            } else if let Some(heap) = super::descriptor_heap(object, owner) {
                heap.native.as_raw()
            } else if let Some(heap) = super::queries::native(object, owner) {
                heap.as_raw()
            } else {
                return E_INVALIDARG;
            }
        };
        native.push(pageable);
    }
    // SAFETY: all four interface types derive from ID3D12Pageable. The guest
    // owns their references and the priority array until the host call returns.
    unsafe {
        (host.vtable().base__.SetResidencyPriority)(
            host.as_raw(),
            count,
            native.as_ptr(),
            priorities,
        )
    }
}

pub(super) unsafe extern "system" fn place(
    device: &Device,
    heap: *mut c_void,
    offset: u64,
    desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(heap): Option<&Heap> = (unsafe { unwrap(heap, identity(device).as_raw(), &HEAP) })
    else {
        return E_INVALIDARG;
    };
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if !matches!(
        desc.Dimension,
        D3D12_RESOURCE_DIMENSION_BUFFER
            | D3D12_RESOURCE_DIMENSION_TEXTURE1D
            | D3D12_RESOURCE_DIMENSION_TEXTURE2D
            | D3D12_RESOURCE_DIMENSION_TEXTURE3D
    ) {
        return E_NOTIMPL;
    }
    // A guest byte offset has to fit the host allocation, including padding.
    let initial = super::format::host_initial_state(desc, initial);
    let primary = super::depth::primary(desc);
    if primary.is_some()
        && (heap.state.Properties.Type != D3D12_HEAP_TYPE_DEFAULT
            || initial.contains(D3D12_RESOURCE_STATE_UNORDERED_ACCESS))
    {
        return E_NOTIMPL;
    }
    let mut host = primary.unwrap_or(*desc);
    host.Flags.0 &= super::format::HOST_FLAGS;
    let allocation = |host: &D3D12_RESOURCE_DESC| unsafe {
        device
            .native
            .GetResourceAllocationInfo(0, core::slice::from_ref(host))
    };
    let fits = |info: &D3D12_RESOURCE_ALLOCATION_INFO| {
        info.SizeInBytes != u64::MAX
            && info.Alignment != 0
            && offset.is_multiple_of(info.Alignment)
            && offset
                .checked_add(info.SizeInBytes)
                .is_some_and(|end| end <= heap.state.SizeInBytes)
    };
    let mut info = allocation(&host);
    let small_msaa = u64::from(D3D12_SMALL_MSAA_RESOURCE_PLACEMENT_ALIGNMENT);
    if host.SampleDesc.Count > 1
        && host.Alignment == 0
        && offset.is_multiple_of(small_msaa)
        && (!fits(&info) || info.Alignment > heap.state.Alignment)
    {
        // The guest can suballocate MSAA targets on 64 KiB boundaries. Ask
        // this adapter whether it can place this image at the exact guest offset.
        let small = D3D12_RESOURCE_DESC {
            Alignment: small_msaa,
            ..host
        };
        let small_info = allocation(&small);
        if small_info.Alignment == small_msaa && fits(&small_info) {
            host = small;
            info = small_info;
        }
    }
    let report = |step, status: HRESULT| {
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!(
                "placed resource {step} {:#010x} offset={offset:#x} heap={:#x}/{:#x} host={:#x}/{:#x} {}x{}x{} fmt={} samples={} flags={:#x} align={:#x} state={:#x} clear={:?}",
                status.0,
                heap.state.SizeInBytes,
                heap.state.Alignment,
                info.SizeInBytes,
                info.Alignment,
                host.Width,
                host.Height,
                host.DepthOrArraySize,
                host.Format.0,
                host.SampleDesc.Count,
                host.Flags.0,
                host.Alignment,
                initial.0,
                unsafe { clear.as_ref() }.map(|c| c.Format.0),
            )
        });
        status
    };
    if !fits(&info) {
        return report("range", E_INVALIDARG);
    }
    let host_clear = super::format::host_clear(clear);
    let mut native = None;
    if let Err(error) = unsafe {
        device.native.CreatePlacedResource::<_, ID3D12Resource>(
            &heap.native,
            offset,
            &host,
            super::host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER),
            Some(host_clear.as_ref().map_or(clear, core::ptr::from_ref)),
            &mut native,
        )
    } {
        return report("host", error.code());
    }
    unsafe {
        super::depth::wrap::<false>(
            device,
            &heap.state.Properties,
            desc,
            primary.as_ref(),
            initial,
            native,
            out,
        )
    }
}

// Slot 44 places a tiled resource at a title-selected virtual address. An
// unrelated allocation would lose texture contents and subsequent page remaps.
pub(super) fn import_buffer(
    device: &Device,
    address: u64,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
) -> windows::core::Result<ID3D12Resource> {
    import_buffer_with_heap(device, address, desc, state).map(|(resource, _)| resource)
}

pub(super) fn import_buffer_with_heap(
    device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES,
) -> windows::core::Result<(ID3D12Resource, Arc<ImportedHeap>)> {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

    let granularity = u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT);
    let base = address & !(granularity - 1);
    let offset = address - base;
    // The console also places small buffers directly at sub-page addresses.
    // Let the host validate its actual tight alignment for the selected offset.
    let tight = if !offset.is_multiple_of(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64) {
        D3D12_RESOURCE_FLAGS(0x400)
    } else { D3D12_RESOURCE_FLAG_NONE };
    let host_desc = D3D12_RESOURCE_DESC {
        Flags: desc.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | tight,
        ..*desc
    };
    let info = unsafe { device.native.GetResourceAllocationInfo(0, &[host_desc]) };
    if info.SizeInBytes == u64::MAX || info.Alignment == 0 || !offset.is_multiple_of(info.Alignment) {
        return Err(E_INVALIDARG.into());
    }
    let size = offset.checked_add(info.SizeInBytes)
        .and_then(|end| end.checked_next_multiple_of(granularity)).ok_or(E_INVALIDARG)?;
    crate::diagnostics::watch("imported buffer", address, info.SizeInBytes, || format!(
        "width={:#x} flags={:#x} state={:#x}", desc.Width, desc.Flags.0, state.0));
    let pin = GPU_APERTURE.pin(base, size)?;
    // The GPU reads and writes these pages: commit lazy title views (xmem
    // lazy_commit.rs) before the committed-pages check, which refused them
    // (test135: placed buffer at 0x19d100000 E_INVALIDARG).
    commit_title_range(base, size);
    validate_shared_range(base, base.checked_add(size).ok_or(E_INVALIDARG)?)?;
    // A guest can place thousands of tiny resources in one multi-GiB XMem
    // reservation. Pin only the pages needed by this resource on the adapter.
    let heap = if let Ok(sized) = device.native.cast::<ID3D12Device13>() {
        device.state.page_pools.import(&sized, base, size, pin)?
    } else {
        // Committed first: a part-committed view splits into several regions.
        commit_title_range(base, size);
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(base as _), &mut region, size_of_val(&region)) } == 0
            || region.AllocationBase as u64 != base || region.RegionSize as u64 != size {
            return Err(E_NOTIMPL.into());
        }
        let host: ID3D12Device3 = device.native.cast()?;
        let native = unsafe { host.OpenExistingHeapFromAddress(region.AllocationBase) }
            .inspect_err(|e| import_failure("OpenExistingHeapFromAddress", base, size, e))?;
        crate::diagnostics::memory::note(crate::diagnostics::memory::Category::HeapImport, size);
        Arc::new(ImportedHeap::new(native, pin))
    };
    let mut resource = None;
    unsafe { device.native.CreatePlacedResource(&heap.native, offset, &host_desc, state, None, &mut resource)? };
    let resource: ID3D12Resource = resource.ok_or(E_POINTER)?;
    // Placed resources borrow their heap. Keep the imported CPU allocation's
    // heap alive through the final native resource reference (including views).
    attach_import(&resource, &heap)?;
    Ok((resource, heap))
}

impl ImportedHeap {
    pub(super) fn buffer_alias(self: &Arc<Self>, device: &ID3D12Device, offset: u64,
        width: u64, stride: u32, flags: D3D12_RESOURCE_FLAGS,
    ) -> windows::core::Result<(ID3D12Resource, u64)> {
        let mut desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: width, Height: 1, DepthOrArraySize: 1,
            MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Flags: D3D12_RESOURCE_FLAGS(0x400) | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | flags,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let align = unsafe { device.GetResourceAllocationInfo(0, &[desc]) }.Alignment;
        if !align.is_power_of_two() { return Err(E_INVALIDARG.into()); }
        let stride = u64::from(stride);
        let first = (0..align.min(offset / stride + 1))
            .find(|n| (offset - n * stride).is_multiple_of(align)).ok_or(E_INVALIDARG)?;
        let start = offset - first * stride;
        desc.Width = width.checked_add(first * stride).ok_or(E_INVALIDARG)?;
        let info = unsafe { device.GetResourceAllocationInfo(0, &[desc]) };
        if info.Alignment == 0 || !start.is_multiple_of(info.Alignment)
            || start.checked_add(info.SizeInBytes).is_none_or(|end| end > unsafe { self.native.GetDesc() }.SizeInBytes) {
            return Err(E_INVALIDARG.into());
        }
        let mut alias = None;
        unsafe { device.CreatePlacedResource(&self.native, start, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut alias)? };
        let alias = alias.ok_or(E_POINTER)?;
        attach_import(&alias, self)?;
        Ok((alias, first))
    }
}

pub(super) unsafe extern "system" fn place_virtual(
    device: &Device,
    address: u64,
    desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if address == 0 {
        return E_INVALIDARG;
    }
    if let Err(error) = GPU_APERTURE.wait(address, 1) { return error.code(); }
    let state = super::host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER);
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER && clear.is_null() {
        let native = import_or_back(device, address, desc, state);
        return unsafe {
            wrap_pageable(native, &RESOURCE, device,
                super::ResourceState::from(super::Backing::ConsoleLayout(desc.Layout, desc.Flags)), out)
        };
    }
    if clear.is_null()
        && let Some(result) = unsafe { super::media::place(device, address, desc, state, out) }
    {
        return result;
    }
    let result = device
        .state
        .placements
        .texture(&device.native, address, desc, state, unsafe { clear.as_ref() });
    match result {
        Ok(texture) => {
            let native = texture.native.clone();
            return unsafe {
                wrap_pageable(
                    Ok(native),
                    &RESOURCE,
                    device,
                    super::ResourceState::from(super::Backing::Placed(texture)),
                    out,
                )
            };
        }
        Err(error) if error.code() != E_NOTIMPL => return error.code(),
        Err(_) => (),
    }
    static REPORTED: std::sync::Once = std::sync::Once::new();
    REPORTED.call_once(|| {
        let message = format!("Project TuxBox: unsupported CreatePlacedResourceX at {address:#x}, dim={} {}x{}x{} mips={} format={} layout={:#x} flags={:#x} state={:#x}\n\0",
            desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.MipLevels, desc.Format.0,
            desc.Layout.0, desc.Flags.0, initial.0);
        unsafe { windows::Win32::System::Diagnostics::Debug::OutputDebugStringA(windows::core::PCSTR(message.as_ptr())) };
    });
    E_NOTIMPL
}

// The layout-based Xbox placement API uses UNDEFINED for a buffer. On the
// legacy host buffer path this starts in COMMON, independent of any texture layout.
/// A committed device buffer standing in for title pages that neither open
/// as a heap nor copy.
fn device_buffer(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC)
    -> windows::core::Result<(ID3D12Resource, Arc<super::views::TextureAddress>)> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let host = D3D12_RESOURCE_DESC {
        Width: desc.Width.max(1).next_multiple_of(256),
        Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0),
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Alignment: 0, Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
        ..*desc
    };
    let mut native: Option<ID3D12Resource> = None;
    unsafe { device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &host, D3D12_RESOURCE_STATE_COMMON, None, &mut native)? };
    let native = native.ok_or(E_POINTER)?;
    crate::diagnostics::memory::note(crate::diagnostics::memory::Category::DeviceBuffer, host.Width);
    let buffer = super::views::TextureAddress::at(device, &native, address as usize, false, None);
    Ok((native, buffer))
}

pub(super) unsafe extern "system" fn place_virtual_layout(
    device: &Device, address: u64, desc: *const D3D12_RESOURCE_DESC,
    layout: D3D12_BARRIER_LAYOUT, clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID, out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() { return status; }
    let Some(desc) = (unsafe { desc.as_ref() }) else { return E_INVALIDARG };
    if let Err(error) = GPU_APERTURE.wait(address, 1) { return error.code(); }
    if address != 0 && desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER
        && matches!(desc.Layout.0, 0x100..=0x102 | 0x105 | 0x109 | 0x10a) {
        // Placed CPU-visible images promote from COMMON. The uploader handles
        // the guest's pitch or swizzle before a queue reads the texture.
        let state = match layout {
            // UNDEFINED contents need no particular starting layout.
            D3D12_BARRIER_LAYOUT_UNDEFINED | D3D12_BARRIER_LAYOUT_COMMON | D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COMMON | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COMMON | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS => D3D12_RESOURCE_STATE_COMMON,
            D3D12_BARRIER_LAYOUT_SHADER_RESOURCE | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_SHADER_RESOURCE
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_SHADER_RESOURCE => D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE,
            D3D12_BARRIER_LAYOUT_GENERIC_READ | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_GENERIC_READ
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_GENERIC_READ => D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_BARRIER_LAYOUT_RENDER_TARGET => D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_BARRIER_LAYOUT_COPY_SOURCE | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_SOURCE
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_SOURCE => D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_BARRIER_LAYOUT_COPY_DEST | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_DEST
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_DEST => D3D12_RESOURCE_STATE_COPY_DEST,
            _ => return E_NOTIMPL,
        };
        let texture = match device.state.placements.texture(&device.native, address, desc, state, unsafe { clear.as_ref() }) {
            Ok(texture) => texture,
            Err(error) => return error.code(),
        };
        return unsafe { wrap_pageable(Ok(texture.native.clone()), &RESOURCE, device,
            super::ResourceState::from(super::Backing::Placed(texture)), out) };
    }
    if address != 0 && desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN {
        return unsafe { place_gpu_target(device, address, desc, layout, clear, out) };
    }
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || !clear.is_null()
        || !matches!(layout, D3D12_BARRIER_LAYOUT_UNDEFINED | D3D12_BARRIER_LAYOUT_COMMON)
        || address == 0 {
        static UNKNOWN_PLACEMENT: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&UNKNOWN_PLACEMENT, || format!("placed resource dim={} {}x{} format={} flags={:#x} layout={:#x} initial={} clear={}", desc.Dimension.0, desc.Width, desc.Height, desc.Format.0, desc.Flags.0, desc.Layout.0, layout.0, !clear.is_null()));
        return E_NOTIMPL;
    }
    let mut host = *desc;
    host.Flags = D3D12_RESOURCE_FLAGS(host.Flags.0 & !0x2_0000);
    // A Series buffer can start between native placement boundaries. Bind a
    // slightly larger host buffer to the same pages and translate offsets at
    // every resource/view entry point. The guest still sees its original VA.
    let probe = D3D12_RESOURCE_DESC {
        Flags: host.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | D3D12_RESOURCE_FLAGS(0x400), ..host
    };
    let (base, delta) = if !address.is_multiple_of(65536) {
        let info = unsafe { device.native.GetResourceAllocationInfo(0, &[probe]) };
        let alignment = if info.SizeInBytes != u64::MAX && info.Alignment.is_power_of_two() {
            info.Alignment
        } else { 65536 };
        let delta = address % alignment;
        (address - delta, delta)
    } else { (address, 0) };
    let Some(width) = host.Width.checked_add(delta) else { return E_INVALIDARG };
    host.Width = width;
    let (native, heap) = match import_buffer_with_heap(device, base, &host, D3D12_RESOURCE_STATE_COMMON) {
        Ok(resource) => resource,
        Err(error) if error.code() == E_INVALIDARG => {
            // Title pages from different sections: no heap spans them. A
            // reserved buffer whose 64 KiB tiles map onto each page's own heap
            // aliases them without a copy; the device copy is the fallback
            // (its refresh and write-back cost ~1.3 GB per submission).
            // Off by default: test18 removed the device (DXGI_ERROR_INVALID_CALL)
            // on the first UpdateTileMappings onto pages opened from an address,
            // and the copy fallback cannot run on a removed device.
            if crate::env_toggle!("TUXBOX_TILED_ALIAS") == Some("1") {
                let tile_base = address & !0xffff;
                let tile_delta = address - tile_base;
                match desc.Width.checked_add(tile_delta).ok_or_else(|| E_INVALIDARG.into())
                    .and_then(|width| tiled_alias(device, tile_base, &D3D12_RESOURCE_DESC { Width: width, ..host }))
                {
                    Ok((native, heap)) => {
                        static TILED: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&TILED, || format!(
                            "placed buffer at {address:#x} ({} bytes) spans separately mapped pages; aliased by tile mappings", desc.Width));
                        let buffer = super::views::TextureAddress::with_heap(
                            device, &native, address as usize, false, None, None, None, tile_delta, Some(heap),
                        );
                        return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                            super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
                    }
                    Err(e) => {
                        static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                        if FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                            crate::diagnostics::log_line(format_args!(
                                "tile-mapped alias at {address:#x} ({} bytes) failed: {:?}; using a device copy", host.Width, e.code()));
                        }
                    }
                }
            }
            let (native, buffer) = match super::views::shadow_buffer(device, address, desc.Width) {
                Ok(shadow) => shadow,
                // Neither an import nor a copy of the title pages: a device
                // buffer of the size asked, not tied to the title pages, so
                // the title still gets its resource (test37: 4745040 bytes
                // failed 0x80070057 just before a crash in title code).
                // TUXBOX_BUFFER_DEVICE_FALLBACK=0 fails it as before.
                Err(e) if crate::env_toggle!("TUXBOX_BUFFER_DEVICE_FALLBACK") != Some("0") => {
                    match device_buffer(device, address, desc) {
                        Ok(made) => {
                            static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                            let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                            if n <= 8 || n.is_power_of_two() {
                                crate::diagnostics::log_line(format_args!(
                                    "placed buffer at {address:#x} ({} bytes): no import or copy ({:?}); a device buffer not tied to the title pages stands in ({n} so far; TUXBOX_BUFFER_DEVICE_FALLBACK=0 fails it)",
                                    desc.Width, e.code()));
                            }
                            made
                        }
                        Err(_) => return e.code(),
                    }
                }
                Err(e) => return e.code(),
            };
            static SHADOWED: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&SHADOWED, || format!("placed buffer at {address:#x} ({} bytes) spans separately mapped pages; using a device copy", desc.Width));
            return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
        }
        Err(error) if backstop_on() => {
            // TuxBox 6171f56 (Valdi320, Apache-2.0): a retry that commits the
            // pages and imports them is a real import of the title's memory,
            // but it was registered like the stand-in, at address 0 with no
            // heap, so its GPU address read 0 and views of the title address
            // never found it. The widened import is retried first, then the
            // exact one; either keeps the title address and its heap.
            let widened = if base != address || host.Flags != desc.Flags {
                import_with_commit(device, base, &host, D3D12_RESOURCE_STATE_COMMON).ok().map(|i| (i, delta))
            } else { None };
            let exact = || import_with_commit(device, address, desc, D3D12_RESOURCE_STATE_COMMON).ok().map(|i| (i, 0));
            if let Some(((native, heap), offset)) = widened.or_else(exact) {
                let buffer = super::views::TextureAddress::with_heap(
                    device, &native, address as usize, false, None, None, None, offset, Some(heap),
                );
                return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                    super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
            }
            let native = match stand_in_buffer(device, desc, D3D12_RESOURCE_STATE_COMMON) {
                Ok(native) => native,
                Err(fallback) => {
                    crate::diagnostics::recording_error(format_args!(
                        "placed buffer at {address:#x} bytes={} native offset={delta:#x}: import {:?} stand-in {:?}",
                        desc.Width, error.code(), fallback.code()));
                    return fallback.code();
                }
            };
            let buffer = super::views::TextureAddress::at(device, &native, 0, false, None);
            return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
        }
        Err(error) => {
            crate::diagnostics::recording_error(format_args!(
                "placed buffer import at {address:#x} bytes={} native offset={delta:#x}: {:?}", desc.Width, error.code()
            ));
            return error.code();
        }
    };
    let buffer = super::views::TextureAddress::with_heap(
        device, &native, address as usize, false, None, None, None, delta, Some(heap),
    );
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
        super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) }
}

/// TUXBOX_PLACED_BACKSTOP=0: a failed placed-buffer import fails the create
/// again instead of committing reserved guest pages or using a device buffer.
fn backstop_on() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_BACKSTOP") != Some("0")
}

fn commit_guest_pages(start: u64, bytes: u64) -> bool {
    use windows::Win32::System::Memory::{MEM_COMMIT, PAGE_READWRITE, VirtualAlloc};
    if bytes == 0 || bytes > 1 << 30 || start.checked_add(bytes).is_none() {
        return false;
    }
    // Commit only this view. The surrounding reservation stays a reservation.
    !unsafe { VirtualAlloc(Some(start as _), bytes as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

fn stand_in_buffer(device: &Device, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES)
    -> windows::core::Result<ID3D12Resource> {
    let host = D3D12_RESOURCE_DESC { Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & !0x2_0000), ..*desc };
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let mut native = None;
    unsafe { device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &host, state, None, &mut native)? };
    native.ok_or(E_POINTER.into())
}

/// A placed buffer over guest memory that will not import (a reserved, not
/// committed range): commit those pages and import again, else a device
/// buffer stands in. Copied from TuxBox aeed457 (brcly, Apache-2.0).
/// Imports the buffer at `address`, committing its reserved guest pages and
/// importing again if the first try fails; the heap comes back with it.
fn import_with_commit(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES)
    -> windows::core::Result<(ID3D12Resource, Arc<ImportedHeap>)> {
    let error = match import_buffer_with_heap(device, address, desc, state) {
        Ok(import) => return Ok(import),
        Err(error) => error,
    };
    if !backstop_on() {
        return Err(error);
    }
    let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(desc)) };
    let bytes = if info.SizeInBytes == u64::MAX { desc.Width } else { info.SizeInBytes };
    let base = address & !0xffff;
    let Some(end) = address.checked_add(bytes).and_then(|end| end.checked_next_multiple_of(0x10000)) else { return Err(error) };
    if commit_guest_pages(base, end - base)
        && let Ok(import) = import_buffer_with_heap(device, address, desc, state)
    {
        static COMMITTED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&COMMITTED, || format!(
            "placed buffer at {address:#x} bytes={} committed reserved guest pages (TUXBOX_PLACED_BACKSTOP=0 off)", desc.Width));
        return Ok(import);
    }
    Err(error)
}

fn import_or_back(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES)
    -> windows::core::Result<ID3D12Resource> {
    match import_with_commit(device, address, desc, state) {
        Ok((native, _)) => Ok(native),
        Err(error) if backstop_on() => {
            let native = stand_in_buffer(device, desc, state)?;
            static DEVICE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&DEVICE, || format!(
                "placed buffer at {address:#x} bytes={} is not importable guest memory ({:?}); using a device buffer (TUXBOX_PLACED_BACKSTOP=0 off)",
                desc.Width, error.code()));
            Ok(native)
        }
        Err(error) => Err(error),
    }
}

type ImportKey = (usize, u64, u64);

/// TuxBox 45c7395 (Valdi320, Apache-2.0) checks that an import made after a
/// queue-ordered XMem remap reads the new pages, with exactly one stale
/// import retired. Ours did not retire it: CopyPageMappingsBatchX replacing
/// pages under a live import (page_mapping.rs pinned_remapped; test137 did
/// 512+ of them) leaves that heap on the old physical pages, and while a
/// resource still held it, the live-import cache and a pool's last-page cache
/// handed the same stale heap to every later import of the range, so new
/// buffers read and wrote the pages the title had already replaced. Such a
/// remap now drops the cached heaps over the range; resources made before it
/// keep theirs, as on the console where they were created over the old pages.
/// TUXBOX_IMPORT_RETIRE=0 keeps reusing them, as before.
pub(crate) fn retire_imports_over(lo: u64, hi: u64) -> usize {
    if crate::env_toggle!("TUXBOX_IMPORT_RETIRE") == Some("0") {
        return 0;
    }
    RETIRED.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    let mut held: Vec<Arc<ImportedHeap>> = Vec::new();
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = live.as_mut() else { return 0 };
    let before = map.len();
    map.retain(|&(_, base, size), heap| {
        let over = base < hi && lo < base.saturating_add(size);
        if over && let Some(heap) = heap.upgrade() {
            // Views by address skip it from now on (views.rs buffer_at). The
            // upgrade is dropped after the lock: a last reference runs Drop.
            heap.stale.store(true, std::sync::atomic::Ordering::Release);
            held.push(heap);
        }
        !over
    });
    let retired = before - map.len();
    drop(live);
    drop(held);
    static TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = TOTAL.fetch_add(retired as u64, std::sync::atomic::Ordering::Relaxed) + retired as u64;
    static LINES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let line = LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if line <= 8 || line.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "heap import: pages under {lo:#x}..{hi:#x} remapped while imported: retired {retired} cached imports there, later imports open the new pages ({n} so far; TUXBOX_IMPORT_RETIRE=0 off)"));
    }
    retired
}

/// Bumped by each retirement: a pool's last-page cache from before it is stale.
static RETIRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Live imports by (device, base, size), reused while something holds them.
static LIVE: std::sync::Mutex<Option<std::collections::HashMap<ImportKey, std::sync::Weak<ImportedHeap>>>> = std::sync::Mutex::new(None);
/// Ranges being opened now (one opener each; see `PagePools::import`).
static OPENING: (std::sync::Mutex<Vec<ImportKey>>, std::sync::Condvar) = (std::sync::Mutex::new(Vec::new()), std::sync::Condvar::new());
/// test102 collapsed to 0.2 fps with its recording threads spending a third
/// of their time in OpenExistingHeapFromAddress1 for ranges they had opened
/// moments before: placed textures re-read after their pages were reclaimed
/// dropped the last reference to each import, so the next one opened it
/// again in the kernel. The last TUXBOX_IMPORT_KEEP (512; 0 off) imports
/// stay referenced for two seconds, so a live-import reuse finds them.
fn keep_recent(heap: &Arc<ImportedHeap>) {
    use std::collections::VecDeque;
    use std::time::{Duration, Instant};
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    static KEPT: std::sync::Mutex<VecDeque<(Instant, Arc<ImportedHeap>)>> = std::sync::Mutex::new(VecDeque::new());
    let limit = *LIMIT.get_or_init(|| crate::env_toggle!("TUXBOX_IMPORT_KEEP").and_then(|v| v.parse().ok()).unwrap_or(512));
    if limit == 0 {
        return;
    }
    let now = Instant::now();
    // Released outside the lock: a heap's last reference may go here.
    let mut gone = Vec::new();
    {
        let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
        while kept.front().is_some_and(|(at, _)| now.duration_since(*at) > Duration::from_secs(2)) || kept.len() >= limit {
            if let Some(old) = kept.pop_front() { gone.push(old); } else { break; }
        }
        kept.push_back((now, heap.clone()));
    }
    drop(gone);
}

/// Keys of LIVE in insertion order, one each, for the incremental sweep.
static SWEEP: std::sync::Mutex<std::collections::VecDeque<ImportKey>> = std::sync::Mutex::new(std::collections::VecDeque::new());

/// Past 65536 entries, each new import checks the next 16 keys in the sweep
/// order (dropping expired ones) instead of walking the whole map: with the
/// title holding every page live, the full retain ran on every import. TuxBox
/// 97642c7 (Valdi320, Apache-2.0), re-derived for this map: an entry is still
/// added at the limit, since a lost entry costs a repeat kernel open
/// (test102). TUXBOX_IMPORT_SWEEP=0 walks the whole map as before.
fn note_live(key: ImportKey, heap: &Arc<ImportedHeap>) {
    const LIMIT: usize = 1 << 16;
    const STEP: usize = 16;
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let map = live.get_or_insert_with(std::collections::HashMap::new);
    if let Some(entry) = map.get_mut(&key) {
        *entry = Arc::downgrade(heap);
        return;
    }
    let mut sweep = SWEEP.lock().unwrap_or_else(|e| e.into_inner());
    if map.len() >= LIMIT {
        if crate::env_toggle!("TUXBOX_IMPORT_SWEEP") == Some("0") {
            map.retain(|_, h| h.strong_count() != 0);
            sweep.retain(|k| map.contains_key(k));
        } else {
            for _ in 0..STEP.min(sweep.len()) {
                let Some(old) = sweep.pop_front() else { break };
                if map.get(&old).is_some_and(|h| h.strong_count() != 0) {
                    sweep.push_back(old);
                } else {
                    map.remove(&old);
                }
            }
        }
    }
    map.insert(key, Arc::downgrade(heap));
    sweep.push_back(key);
}

/// True when [base, base + size) lies in one OS allocation, the only range
/// OpenExistingHeapFromAddress1 accepts. test72 asked 192 times for 0x20000
/// bytes at 0xbfc930000, whose allocation there is one 64 KiB page.
fn one_allocation(base: u64, size: u64) -> bool {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    let end = base.saturating_add(size);
    let view = super::view_state(base as usize, size as usize);
    if view.committed() {
        // Covered by lazy views: one allocation only if one view holds it.
        return view.one_view();
    }
    let mut at = base;
    let mut allocation = None;
    while at < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } == 0 || region.State != MEM_COMMIT {
            return false;
        }
        let this = region.AllocationBase as u64;
        if *allocation.get_or_insert(this) != this {
            return false;
        }
        let next = region.BaseAddress as u64 + region.RegionSize as u64;
        if next <= at { return false; }
        at = next;
    }
    true
}

/// rebox's idea (re-derived from 19a69d0): a failed adoption is remembered by
/// its first 64 KiB page and its size for about a second in a fixed table (a
/// collision overwrites), so retries of that range skip the lock, the list
/// scan and the VirtualQuery walk. `record` adds it; otherwise it answers
/// whether the range failed recently. Keyed by the page alone, a refused range
/// also refused every smaller range at its start: shadow.rs split_import's
/// left halves never asked the driver down to the last page, and the shadow's
/// leftmost piece got the device stand-in (test134 lines 703-708).
/// TUXBOX_REFUSED_PAGES=0 off.
pub(super) fn refused_page(address: u64, size: u64, record: bool) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    const SLOTS: usize = 4096;
    static TABLE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static STAMP: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static SIZE: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    if crate::env_toggle!("TUXBOX_REFUSED_PAGES") == Some("0") { return false; }
    let page = (address >> 16) + 1;
    let slot = ((page ^ size.rotate_left(29)).wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 52) as usize % SLOTS;
    let now = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1;
    if record {
        TABLE[slot].store(page, Ordering::Relaxed);
        SIZE[slot].store(size, Ordering::Relaxed);
        STAMP[slot].store(now, Ordering::Relaxed);
        return true;
    }
    let hit = TABLE[slot].load(Ordering::Relaxed) == page && SIZE[slot].load(Ordering::Relaxed) == size && now.saturating_sub(STAMP[slot].load(Ordering::Relaxed)) < 1000;
    if hit {
        static HITS: AtomicU64 = AtomicU64::new(0);
        let n = HITS.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "heap import: {size:#x} bytes at {address:#x} failed adoption less than 1 s ago, not asked again ({n} so far; TUXBOX_REFUSED_PAGES=0 off)"));
        }
    }
    hit
}

/// Opens title memory as a heap (the kernel call), joining split views first.
fn open_heap(device: &ID3D12Device13, base: u64, size: u64, pin: ImportPin) -> windows::core::Result<Arc<ImportedHeap>> {
    use windows::Win32::Foundation::E_INVALIDARG;
    // A range the host refused is refused again for 5 s without asking:
    // each retry cost an OpenExistingHeapFromAddress1 before the caller
    // fell back to its shadow copy. The window lets a remapped range in.
    static REFUSED: std::sync::Mutex<Vec<(u64, u64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
    let refuse = || {
        refused_page(base, size, true);
        let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
        if refused.len() < 4096 { refused.push((base, size, std::time::Instant::now())); }
    };
    if refused_page(base, size, false) {
        return Err(E_INVALIDARG.into());
    }
    {
        let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
        refused.retain(|(.., at)| at.elapsed() < std::time::Duration::from_secs(5));
        if refused.iter().any(|&(b, s, _)| b == base && s == size) {
            return Err(E_INVALIDARG.into());
        }
    }
    // The title maps one 64 KiB page per call, so a GPU range is many OS
    // views and no heap spans it. xmem rejoins views of one section at
    // consecutive offsets into one view on request. A range still split is
    // refused without the kernel call (TUXBOX_IMPORT_PRECHECK=0 asks anyway).
    // Committed first: the precheck and the driver need committed pages.
    commit_title_range(base, size);
    let precheck = crate::env_toggle!("TUXBOX_IMPORT_PRECHECK") != Some("0");
    let multi = size > u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT);
    if precheck && multi && !one_allocation(base, size) && !(join_page_range(base, size) && one_allocation(base, size)) {
        static SPLIT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SPLIT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "heap import: {size:#x} bytes at {base:#x} span several allocations, read through a shadow copy without asking the driver ({n} so far; TUXBOX_IMPORT_PRECHECK=0 asks)"));
        }
        refuse();
        return Err(E_INVALIDARG.into());
    }
    let mut native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) };
    if !precheck && let Err(e) = &native && e.code() == E_INVALIDARG && multi && join_page_range(base, size) {
        native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) };
    }
    let native = native.inspect_err(|e| {
        import_failure("OpenExistingHeapFromAddress1", base, size, e);
        if e.code() == E_INVALIDARG { refuse(); }
    })?;
    crate::diagnostics::memory::note(crate::diagnostics::memory::Category::HeapImport, size);
    Ok(Arc::new(ImportedHeap::new(native, pin)))
}

/// Commits the lazy title pages of a range before the driver imports it.
fn commit_title_range(base: u64, size: u64) {
    super::commit_lazy(base as usize, size as usize);
}

/// Asks xmem to make the title range one OS view (TuxBoxJoinPageRange);
/// true when it now is. TUXBOX_JOIN_PAGES=0 never asks.
fn join_page_range(base: u64, size: u64) -> bool {
    type Join = unsafe extern "system" fn(usize, usize) -> i32;
    static JOIN: std::sync::OnceLock<Option<Join>> = std::sync::OnceLock::new();
    if crate::env_toggle!("TUXBOX_JOIN_PAGES") == Some("0") {
        return false;
    }
    let join = JOIN.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let module = GetModuleHandleW(windows::core::w!("xmem.dll")).ok()?;
        GetProcAddress(module, windows::core::s!("TuxBoxJoinPageRange")).map(|f| core::mem::transmute::<_, Join>(f))
    });
    let Some(join) = join else { return false };
    let result = unsafe { join(base as usize, size as usize) };
    static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n < 16 || (n + 1).is_power_of_two() {
        crate::diagnostics::log_line(format_args!("join title pages {base:#x} ({size:#x} bytes): {result} (#{})", n + 1));
    }
    result == 1
}

/// One queue and fence per device for tile mappings, waited on the CPU so the
/// mapping precedes every later use on any queue.
static TILE_QUEUES: std::sync::Mutex<Vec<(usize, ID3D12CommandQueue, ID3D12Fence, u64)>> = std::sync::Mutex::new(Vec::new());

/// A reserved buffer over title pages [base, base + width) whose tiles map,
/// page by page, onto each page's imported heap: the same bytes as the title's
/// pages, without one heap spanning them. Returns the first page's heap for
/// view aliases (they fail over to a view copy across pages).
fn tiled_alias(device: &Device, base: u64, host: &D3D12_RESOURCE_DESC) -> windows::core::Result<(ID3D12Resource, Arc<ImportedHeap>)> {
    const TILE: u64 = 65536;
    if !base.is_multiple_of(TILE) {
        return Err(E_INVALIDARG.into());
    }
    let width = host.Width.next_multiple_of(TILE);
    let tiles = width / TILE;
    if tiles == 0 || tiles > 4096 {
        return Err(E_INVALIDARG.into());
    }
    let page_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: TILE, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        ..Default::default()
    };
    let mut heaps = Vec::with_capacity(tiles as usize);
    for i in 0..tiles {
        let (_, heap) = import_buffer_with_heap(device, base + i * TILE, &page_desc, D3D12_RESOURCE_STATE_COMMON)?;
        heaps.push(heap);
    }
    let desc = D3D12_RESOURCE_DESC {
        Width: width,
        Flags: D3D12_RESOURCE_FLAGS(host.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0),
        Alignment: 0,
        ..page_desc
    };
    let mut resource: Option<ID3D12Resource> = None;
    unsafe { device.native.CreateReservedResource(&desc, D3D12_RESOURCE_STATE_COMMON, None, &mut resource)? };
    let resource = resource.ok_or(E_POINTER)?;
    super::mark_reserved(&resource);
    let owner = core::ptr::from_ref(device) as usize;
    let mut queues = TILE_QUEUES.lock().unwrap_or_else(|e| e.into_inner());
    if !queues.iter().any(|q| q.0 == owner) {
        let queue: ID3D12CommandQueue = unsafe { device.native.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT, ..Default::default() })? };
        let fence: ID3D12Fence = unsafe { device.native.CreateFence(0, D3D12_FENCE_FLAG_NONE)? };
        queues.push((owner, queue, fence, 0));
    }
    let entry = queues.iter_mut().find(|q| q.0 == owner).ok_or(E_POINTER)?;
    for (i, heap) in heaps.iter().enumerate() {
        let start = D3D12_TILED_RESOURCE_COORDINATE { X: i as u32, Y: 0, Z: 0, Subresource: 0 };
        let size = D3D12_TILE_REGION_SIZE { NumTiles: 1, UseBox: false.into(), Width: 1, Height: 1, Depth: 1 };
        let (offset, count) = (0u32, 1u32);
        unsafe { entry.1.UpdateTileMappings(&resource, 1, Some(&start), Some(&size), &heap.native, 1, None, Some(&offset), Some(&count), D3D12_TILE_MAPPING_FLAG_NONE) };
    }
    entry.3 += 1;
    unsafe {
        entry.1.Signal(&entry.2, entry.3)?;
        entry.2.SetEventOnCompletion(entry.3, windows::Win32::Foundation::HANDLE::default())?;
        device.native.GetDeviceRemovedReason()?;
    }
    drop(queues);
    // Tiles borrow their heaps: keep every page's import alive with the resource.
    for (i, heap) in heaps.iter().enumerate() {
        let key = GUID::from_u128(IMPORTED_HEAP.to_u128() ^ ((i as u128 + 1) << 64));
        static MARKER: Slots<8> = child_slots::<ID3D12Heap, Arc<ImportedHeap>, 8>();
        let mut out = core::ptr::null_mut();
        unsafe {
            crate::proxy::allocate(&MARKER, &[], heap.native.clone(), None, heap.clone(), &mut out).ok()?;
            let marker = windows::core::IUnknown::from_raw(out);
            resource.SetPrivateDataInterface(&key, &marker)?;
        }
    }
    Ok((resource, heaps.swap_remove(0)))
}

pub(super) struct RenderHeap {
    native: ID3D12Heap,
    end: u64,
    /// holds a render target or depth image (`rank`)
    renderable: std::sync::atomic::AtomicBool,
    bytes: u64,
    /// the native device it was made on
    device: usize,
    /// `now_ms` of its last use (`touch`)
    used: std::sync::atomic::AtomicU64,
    /// evicted by `residency::evict_idle`; read without its lock as a hint
    evicted: std::sync::atomic::AtomicBool,
    _live: crate::diagnostics::memory::Live,
}

// Device-local heaps for images placed at guest addresses whose texels the
// guest only reaches through the GPU. Keyed by guest address so later images
// at the same address alias, as they do in console memory.
#[derive(Default)]
pub(super) struct RenderHeaps(
    std::sync::Mutex<std::collections::BTreeMap<u64, std::sync::Weak<RenderHeap>>>,
    std::sync::OnceLock<bool>,
);

impl RenderHeaps {
    /// Resource heap tier 2 lets one heap hold every image category, so a
    /// render target and a sampled texture can alias the same guest range.
    fn mixed(&self, device: &ID3D12Device) -> bool {
        *self.1.get_or_init(|| {
            let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
            unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, (&raw mut options).cast(),
                size_of_val(&options) as u32) }.is_ok()
                && options.ResourceHeapTier.0 >= D3D12_RESOURCE_HEAP_TIER_2.0
        })
    }

    fn acquire(&self, device: &ID3D12Device, address: u64, end: u64, alignment: u64, renderable: bool)
        -> windows::core::Result<(std::sync::Arc<RenderHeap>, u64)> {
        use std::sync::Arc;
        let mixed = self.mixed(device);
        if !renderable && !mixed {
            return Err(E_NOTIMPL.into());
        }
        let mut heaps = self.0.lock().unwrap_or_else(|e| e.into_inner());
        heaps.retain(|_, entry| entry.strong_count() != 0);
        if let Some((&base, entry)) = heaps.range(..end).next_back()
            && let Some(heap) = entry.upgrade()
            && heap.end > address
            && end <= heap.end
            && let Some(offset) = heap_offset(base, address, alignment) {
            heap.touch();
            if renderable && !heap.renderable.swap(true, std::sync::atomic::Ordering::Relaxed) {
                rank(device, &heap.native, true, end - address);
            }
            return Ok((heap, offset));
        }
        // The title reuses guest memory for a larger or differently placed
        // image. Cover every overlapping heap's range with a new heap so later
        // placements here alias each other. Earlier images keep their heap.
        let (mut start, mut stop) = (address, end);
        let mut covered: Vec<(std::sync::Weak<RenderHeap>, u64)> = Vec::new();
        for (&base, entry) in heaps.range(..end) {
            if let Some(heap) = entry.upgrade() && heap.end > address {
                start = start.min(base);
                stop = stop.max(heap.end);
                covered.push((entry.clone(), heap.end - base));
            }
        }
        if !(address - start).is_multiple_of(alignment) {
            start = address;
        }
        // Each overlap made a heap over the union while the earlier heaps
        // stayed alive with their images, so a pool the title keeps placing
        // into grew a new union-sized heap each time (test47: device memory
        // +5 GB in 20 s). Past the cap the image gets a heap of its own size.
        let own = !union_fits(stop - start, end - address);
        if own {
            (start, stop) = (address, end);
            note_own_heap(stop - start);
        } else {
            let overlapped: Vec<u64> = heaps.range(start..stop).map(|(&base, _)| base).collect();
            for base in overlapped {
                heaps.remove(&base);
            }
        }
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: stop - start, Alignment: alignment.max(65536),
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() },
            Flags: if mixed { D3D12_HEAP_FLAG_ALLOW_ALL_BUFFERS_AND_TEXTURES } else { D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES },
        };
        let mut native = None;
        unsafe { device.CreateHeap(&desc, &mut native)? };
        let native = native.ok_or(E_POINTER)?;
        rank(device, &native, renderable, desc.SizeInBytes);
        let heap = Arc::new(RenderHeap { native, end: stop, renderable: renderable.into(),
            bytes: desc.SizeInBytes, device: device.as_raw() as usize,
            used: residency::now_ms().into(), evicted: false.into(),
            _live: crate::diagnostics::memory::live(crate::diagnostics::memory::Category::RenderHeap, desc.SizeInBytes) });
        heaps.insert(start, Arc::downgrade(&heap));
        drop(heaps);
        residency::adopt(&heap);
        note_shadowed(covered, desc.SizeInBytes);
        Ok((heap, address - start))
    }
}

impl RenderHeap {
    /// A use of an image in it: recorded now, made resident first if evicted.
    pub(super) fn touch(&self) {
        use std::sync::atomic::Ordering::{Relaxed, SeqCst};
        if !residency::tracking() {
            return;
        }
        // A use within the last 100 ms already keeps it from the evictor,
        // which only takes heaps unused for seconds: submissions touch every
        // render view in their tables, so most touches are skipped here.
        let now = residency::now_ms();
        if now.saturating_sub(self.used.load(Relaxed)) >= 100 {
            self.used.store(now, SeqCst);
        }
        if self.evicted.load(SeqCst) {
            residency::make_resident(self);
        }
    }
}

/// Render heaps evicted while unused, made resident again at their next use.
///
/// test94 sat 730 MiB over the device-local budget with 2.6 GB of render
/// heaps that nothing ever evicted. A render heap is used when:
/// * a resource proxy of an image in it is unwrapped (every command taking a
///   resource does that: copies, barriers, resolves, clears, presents);
/// * an RTV or DSV of one is bound or cleared (`placement::touch_target`);
/// * a submission's sampled descriptor ranges (the tables its draws and
///   dispatches bind, or the whole heap where the tables are unknown) hold
///   an SRV or UAV of one (`placement::submit`).
///
/// Once a second over budget, heaps unused for TUXBOX_SPAN_EVICT_S (10 s)
/// are evicted. A touch makes an evicted heap resident before the command
/// that uses it is recorded. An evictor marks a heap evicted and then reads
/// its use again, and a toucher records its use and then reads the mark
/// (both sequentially consistent), so one of them always sees the other.
/// A list submitted more than half that time after it was opened (one run
/// again without a new recording) holds evictions for that long, since its
/// direct uses were recorded when it was. TUXBOX_RENDER_HEAP_EVICT=0 off.
pub(super) mod residency {
    use super::RenderHeap;
    use std::sync::atomic::{AtomicU64, Ordering::{Relaxed, SeqCst}};
    use std::sync::{Arc, Mutex, Weak};
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::core::Interface;

    /// Held while heaps are evicted or made resident.
    static RESIDENCY: Mutex<()> = Mutex::new(());
    static LIVE: Mutex<Vec<Weak<RenderHeap>>> = Mutex::new(Vec::new());
    static HOLD_UNTIL: AtomicU64 = AtomicU64::new(0);
    static EVICTED: AtomicU64 = AtomicU64::new(0);
    static EVICTED_BYTES: AtomicU64 = AtomicU64::new(0);
    static RESTORED: AtomicU64 = AtomicU64::new(0);
    static PASSES: AtomicU64 = AtomicU64::new(0);

    pub(crate) fn now_ms() -> u64 {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
    }

    /// test97 fell from 35-40 fps to 15 with this on: every submit walked
    /// all render views of its descriptor heaps under the map's mutex, which
    /// every view creation and descriptor copy on the title's threads takes
    /// too. Use tracking and eviction stay off unless
    /// TUXBOX_RENDER_HEAP_EVICT=1.
    pub(crate) fn tracking() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| crate::env_toggle!("TUXBOX_RENDER_HEAP_EVICT") == Some("1"))
    }

    fn enabled() -> bool {
        tracking()
    }

    fn after_ms() -> u64 {
        (crate::device::placement::evict_after() * 1000.0) as u64
    }

    pub(super) fn adopt(heap: &Arc<RenderHeap>) {
        let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
        if live.len() >= 64 && live.len().is_power_of_two() {
            live.retain(|w| w.strong_count() != 0);
        }
        live.push(Arc::downgrade(heap));
    }

    pub(super) fn make_resident(heap: &RenderHeap) {
        let _held = RESIDENCY.lock().unwrap_or_else(|e| e.into_inner());
        if !heap.evicted.load(SeqCst) {
            return;
        }
        let mut device: Option<ID3D12Device> = None;
        let made = unsafe { heap.native.GetDevice(&mut device) }.and_then(|()| {
            let device = device.ok_or(windows::core::Error::from(windows::Win32::Foundation::E_POINTER))?;
            unsafe { device.MakeResident(&[Some(heap.native.cast::<ID3D12Pageable>()?)]) }
        });
        match made {
            Ok(()) => {
                heap.evicted.store(false, SeqCst);
                let n = RESTORED.fetch_add(1, Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "render heaps: an evicted heap ({} MiB) made resident again at its use ({n} so far)", heap.bytes >> 20));
                }
            }
            Err(e) => crate::diagnostics::log_line(format_args!(
                "render heaps: MakeResident of an evicted heap ({} MiB) failed {e}", heap.bytes >> 20)),
        }
    }

    /// A list opened `age` ago is being submitted.
    pub(crate) fn list_submitted(age: std::time::Duration) {
        let after = after_ms();
        if after != 0 && age.as_millis() as u64 > after / 2 {
            HOLD_UNTIL.fetch_max(now_ms() + after, Relaxed);
        }
    }

    /// Evicts this device's render heaps unused for TUXBOX_SPAN_EVICT_S.
    pub(crate) fn evict_idle(device: &ID3D12Device) {
        let after = after_ms();
        if !enabled() || after == 0 || now_ms() < HOLD_UNTIL.load(Relaxed) {
            return;
        }
        let _held = RESIDENCY.lock().unwrap_or_else(|e| e.into_inner());
        let id = device.as_raw() as usize;
        let mut chosen: Vec<Arc<RenderHeap>> = Vec::new();
        {
            let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
            live.retain(|w| w.strong_count() != 0);
            for heap in live.iter().filter_map(Weak::upgrade) {
                if heap.device != id || heap.evicted.load(SeqCst) {
                    continue;
                }
                let used = heap.used.load(SeqCst);
                if now_ms().saturating_sub(used) < after {
                    continue;
                }
                heap.evicted.store(true, SeqCst);
                if heap.used.load(SeqCst) != used {
                    heap.evicted.store(false, SeqCst);
                    continue;
                }
                chosen.push(heap);
            }
        }
        if chosen.is_empty() {
            return;
        }
        let list: Vec<Option<ID3D12Pageable>> = chosen.iter().map(|h| h.native.cast().ok()).collect();
        let bytes: u64 = chosen.iter().map(|h| h.bytes).sum();
        // Evict does not wait for the GPU: the video memory manager pages the
        // heaps out once the work using them is done (they have been unused
        // for `after`).
        if let Err(e) = unsafe { device.Evict(&list) } {
            for heap in &chosen {
                heap.evicted.store(false, SeqCst);
            }
            crate::diagnostics::log_line(format_args!("render heaps: Evict of {} idle heaps failed {e}", chosen.len()));
            return;
        }
        EVICTED.fetch_add(chosen.len() as u64, Relaxed);
        let total = EVICTED_BYTES.fetch_add(bytes, Relaxed) + bytes;
        let n = PASSES.fetch_add(1, Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "render heaps: evicted {} heaps ({} MiB) unused for {} s ({} heaps, {} MiB evicted so far, {} made resident again; TUXBOX_RENDER_HEAP_EVICT=0 off)",
                chosen.len(), bytes >> 20, after / 1000, EVICTED.load(Relaxed), total >> 20, RESTORED.load(Relaxed)));
        }
    }
}

/// test94 sat 730 MiB over the device-local budget with 2.6 GB of render
/// heaps at the default priority, which leaves the video memory manager no
/// hint about which heaps to demote. Nothing records which render heaps a
/// submission uses, so evicting them (as page pools and spans are) could
/// evict one in use (test59). Instead a heap of sampled images only gets
/// LOW priority and one holding a render target or depth image HIGH, so
/// sampled images go to system memory before the targets each frame
/// writes. TUXBOX_RENDER_HEAP_PRIORITY=0 leaves the default.
fn rank(device: &ID3D12Device, heap: &ID3D12Heap, renderable: bool, bytes: u64) {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static HIGH: AtomicU64 = AtomicU64::new(0);
    static LOW: AtomicU64 = AtomicU64::new(0);
    if crate::env_toggle!("TUXBOX_RENDER_HEAP_PRIORITY") == Some("0") {
        return;
    }
    let Ok(device1) = device.cast::<ID3D12Device1>() else { return };
    let Ok(pageable) = heap.cast::<ID3D12Pageable>() else { return };
    let priority = if renderable { D3D12_RESIDENCY_PRIORITY_HIGH } else { D3D12_RESIDENCY_PRIORITY_LOW };
    if let Err(e) = unsafe { device1.SetResidencyPriority(1, &Some(pageable), &priority) } {
        static FAILED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FAILED, || format!("render heaps: SetResidencyPriority failed {e}"));
        return;
    }
    let (counter, other) = if renderable { (&HIGH, &LOW) } else { (&LOW, &HIGH) };
    let mib = counter.fetch_add(bytes, Relaxed) + bytes >> 20;
    let before = mib.saturating_sub(bytes >> 20);
    if mib >> 9 != before >> 9 {
        crate::diagnostics::log_line(format_args!(
            "render heaps: {} MiB ranked {} (and {} MiB {}); sampled-only heaps LOW, render or depth targets HIGH (TUXBOX_RENDER_HEAP_PRIORITY=0 off)",
            mib, if renderable { "HIGH" } else { "LOW" }, other.load(Relaxed) >> 20, if renderable { "LOW" } else { "HIGH" }));
    }
}

/// Whether an image of `own` bytes may share a heap spanning `union`
/// bytes: at most twice its size or TUXBOX_RENDER_HEAP_UNION_MB (default
/// 64), whichever is larger. TUXBOX_RENDER_HEAP_UNION_MB=0 always unions.
fn union_fits(union: u64, own: u64) -> bool {
    static CAP: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let cap = *CAP.get_or_init(|| match crate::env_toggle!("TUXBOX_RENDER_HEAP_UNION_MB").and_then(|v| v.trim().parse::<u64>().ok()) {
        Some(0) => None,
        Some(mb) => Some(mb << 20),
        None => Some(64 << 20),
    });
    union_fits_cap(union, own, cap)
}

fn union_fits_cap(union: u64, own: u64, cap: Option<u64>) -> bool {
    cap.is_none_or(|cap| union <= cap.max(own.saturating_mul(2)))
}

/// Live render heaps a newer heap covers: images still holding an older heap
/// keep its memory while the newer heap backs the same guest range again (on
/// the console both alias one allocation). Logged every power of two of new
/// heaps from 64 on: how many such heaps are still alive and their MiB.
fn note_shadowed(covered: Vec<(std::sync::Weak<RenderHeap>, u64)>, size: u64) {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static SHADOWED: std::sync::Mutex<Vec<(std::sync::Weak<RenderHeap>, u64)>> = std::sync::Mutex::new(Vec::new());
    static MADE: AtomicU64 = AtomicU64::new(0);
    static MADE_BYTES: AtomicU64 = AtomicU64::new(0);
    let n = MADE.fetch_add(1, Relaxed) + 1;
    MADE_BYTES.fetch_add(size, Relaxed);
    let mut shadowed = SHADOWED.lock().unwrap_or_else(|e| e.into_inner());
    shadowed.extend(covered);
    if n >= 64 && n.is_power_of_two() {
        shadowed.retain(|(heap, _)| heap.strong_count() != 0);
        shadowed.sort_by_key(|(heap, _)| heap.as_ptr() as usize);
        shadowed.dedup_by(|a, b| std::sync::Weak::ptr_eq(&a.0, &b.0));
        let bytes: u64 = shadowed.iter().map(|(_, b)| b).sum();
        crate::diagnostics::log_line(format_args!(
            "render heaps: {n} made ({} MiB); {} still alive under a newer heap over their range ({} MiB held twice)",
            MADE_BYTES.load(Relaxed) >> 20, shadowed.len(), bytes >> 20));
    }
}

fn note_own_heap(bytes: u64) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "render heap union over the cap: image gets its own {bytes:#x}-byte heap and aliases nothing ({n} so far; TUXBOX_RENDER_HEAP_UNION_MB=0 always unions)"));
    }
}

#[cfg(test)]
#[test]
fn a_union_past_the_cap_is_refused() {
    let mib = 1u64 << 20;
    assert!(union_fits_cap(32 * mib, 16 * mib, Some(64 * mib)));
    assert!(!union_fits_cap(512 * mib, 16 * mib, Some(64 * mib)));
    assert!(union_fits_cap(200 * mib, 100 * mib, Some(64 * mib)));
    assert!(union_fits_cap(1 << 40, 1, None));
}

fn unaligned_fallback() -> bool {
    crate::env_toggle!("TUXBOX_UNALIGNED_IMAGE_FALLBACK") != Some("0")
}

/// Where in a heap starting at `base` an image at `address` goes: the
/// existing heap when the offset keeps `alignment`, else a heap of its own.
fn heap_offset(base: u64, address: u64, alignment: u64) -> Option<u64> {
    let offset = address.checked_sub(base)?;
    offset.is_multiple_of(alignment.max(1)).then_some(offset)
}

#[cfg(test)]
#[test]
fn a_misaligned_image_starts_its_own_heap() {
    assert_eq!(heap_offset(0xd4330_0000, 0xd433c_2000, 0x1_0000), None);
    assert_eq!(heap_offset(0xd433c_2000, 0xd433c_2000, 0x1_0000), Some(0));
    assert_eq!(heap_offset(0xd4330_0000, 0xd4332_0000, 0x1_0000), Some(0x2_0000));
}

unsafe fn place_gpu_target(
    device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC,
    initial: D3D12_BARRIER_LAYOUT, clear: *const D3D12_CLEAR_VALUE, out: Out,
) -> HRESULT {
    let initial = super::format::host_initial_layout(desc, initial);
    let primary = super::depth::primary(desc);
    if primary.is_some() && initial == D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS {
        return E_NOTIMPL;
    }
    let mut host = D3D12_RESOURCE_DESC1 {
        Dimension: desc.Dimension, Alignment: desc.Alignment, Width: desc.Width, Height: desc.Height,
        DepthOrArraySize: desc.DepthOrArraySize, MipLevels: desc.MipLevels, Format: desc.Format,
        SampleDesc: desc.SampleDesc, Layout: desc.Layout, Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & 0x1ff),
        ..Default::default()
    };
    if let Some(primary) = &primary {
        host.Format = primary.Format;
        host.Flags = primary.Flags;
    }
    let host_clear = super::format::host_clear(clear);
    let clear = host_clear.as_ref().map_or(clear, core::ptr::from_ref);
    let mut step = "alignment";
    let mut render_heap = None;
    let result = (|| -> windows::core::Result<ID3D12Resource> {
        let api: ID3D12Device13 = device.native.cast()?;
        let formats = super::allocation::packed_uav_formats(host.Format, host.Flags);
        let requirements = |desc: &D3D12_RESOURCE_DESC1| {
            super::allocation::packed_requirements(&device.native, 0, desc)
                .unwrap_or_else(|| unsafe { api.GetResourceAllocationInfo2(0, 1, desc, None) })
        };
        let mut allocation = requirements(&host);
        if desc.SampleDesc.Count == 1 && desc.Alignment == 0
            && address.is_multiple_of(u64::from(D3D12_SMALL_RESOURCE_PLACEMENT_ALIGNMENT))
            && allocation.Alignment > u64::from(D3D12_SMALL_RESOURCE_PLACEMENT_ALIGNMENT)
            && !address.is_multiple_of(allocation.Alignment)
        {
            let mut small = host;
            small.Alignment = u64::from(D3D12_SMALL_RESOURCE_PLACEMENT_ALIGNMENT);
            let small_allocation = requirements(&small);
            if small_allocation.SizeInBytes != u64::MAX && small_allocation.Alignment == small.Alignment {
                host = small;
                allocation = small_allocation;
            }
        }
        let small = u64::from(D3D12_SMALL_MSAA_RESOURCE_PLACEMENT_ALIGNMENT);
        if desc.SampleDesc.Count > 1 && desc.Alignment == 0 && address.is_multiple_of(small)
            && allocation.Alignment > small && !address.is_multiple_of(allocation.Alignment) {
            host.Alignment = small;
            allocation = requirements(&host);
        }
        // Console footprints let the title place images at 8 KiB (f2cc6d8),
        // below the host's 64 KiB. The image lives in a host heap of its own
        // anyway (the guest range is only its key), so a misaligned address
        // gets a heap that starts at it: offset 0 satisfies any alignment.
        let misaligned = allocation.Alignment != 0 && !address.is_multiple_of(allocation.Alignment);
        if misaligned && unaligned_fallback() && allocation.SizeInBytes != u64::MAX {
            static UNALIGNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = UNALIGNED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "placed GPU image at {address:#x} {}x{} format={} is not {:#x}-aligned: placed at the start of its own host heap ({n} so far; TUXBOX_UNALIGNED_IMAGE_FALLBACK=0 fails it)",
                    desc.Width, desc.Height, desc.Format.0, allocation.Alignment));
            }
        } else if allocation.Alignment == 0 || allocation.SizeInBytes == u64::MAX || misaligned {
            crate::diagnostics::recording_error(format_args!("adapter image requirements {:#x}/{:#x}, array={} mips={} format={} samples={}/{}", allocation.SizeInBytes, allocation.Alignment, desc.DepthOrArraySize, desc.MipLevels, desc.Format.0, desc.SampleDesc.Count, desc.SampleDesc.Quality));
            return Err(E_INVALIDARG.into());
        }
        let renderable = (host.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL)).0 != 0;
        step = "guest memory";
        crate::diagnostics::memory::note(crate::diagnostics::memory::Category::ImageInDeviceMemory, allocation.SizeInBytes);
        crate::diagnostics::watch("device-memory image", address, allocation.SizeInBytes, || format!(
            "{}x{}x{} format={} flags={:#x}", desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Flags.0));
        let end = address.checked_add(allocation.SizeInBytes).ok_or(E_INVALIDARG)?;
        if let Err(e) = validate_title_range(address, end, true) {
            // The image lives in device memory and the guest range is only
            // its key, so pages not (yet) committed read-write there do not
            // stop it (test36: 512x512 format 29 at 0xaf5300000);
            // TUXBOX_IMAGE_RANGE_FALLBACK=0 fails it as before.
            if crate::env_toggle!("TUXBOX_IMAGE_RANGE_FALLBACK") == Some("0") {
                return Err(e);
            }
            static UNMAPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = UNMAPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "placed GPU image at {address:#x}..{end:#x} {}x{} format={}: guest range not committed read-write; placed in device memory anyway ({n} so far; TUXBOX_IMAGE_RANGE_FALLBACK=0 fails it)",
                    desc.Width, desc.Height, desc.Format.0));
            }
        }
        // A heap opened from guest memory holds only buffers. Driver-tiled
        // images live in device memory; the guest reaches their texels through
        // GPU copies and views, not through its CPU mapping of this range.
        step = "host heap";
        let (entry, offset) = device.state.render_heaps.acquire(&device.native, address,
            address + allocation.SizeInBytes, allocation.Alignment, renderable)?;
        let heap = entry.native.clone();
        render_heap = Some(entry);
        step = "host placement";
        let mut resource = None;
        unsafe { api.CreatePlacedResource2(&heap, offset, &host, initial, clear.as_ref().map(core::ptr::from_ref),
            (!formats.is_empty()).then_some(formats), &mut resource)? };
        let resource: ID3D12Resource = resource.ok_or(E_POINTER)?;
        unsafe { resource.SetPrivateDataInterface(&IMPORTED_HEAP, &heap)? };
        if crate::diagnostics::display::enabled() && renderable && desc.Width >= 1920 {
            crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                "placed render image res={:#x} at {address:#x}+{:#x} {}x{} format={}",
                windows::core::Interface::as_raw(&resource) as usize, allocation.SizeInBytes, desc.Width, desc.Height, desc.Format.0));
        }
        if !renderable {
            static SAMPLED: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&SAMPLED, || format!(
                "driver-tiled sampled image in device memory at {address:#x}: {}x{} format={} (guest CPU writes to this range are not uploaded)",
                desc.Width, desc.Height, desc.Format.0));
        }
        Ok(resource)
    })();
    let native = match result {
        Ok(resource) => resource,
        Err(e) => {
            static BAD_IMAGE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&BAD_IMAGE, || format!("placed GPU image {step} at {address:#x} {}x{} format={} flags={:#x} alignment={} initial={} failed {:?}", desc.Width, desc.Height, desc.Format.0, desc.Flags.0, desc.Alignment, initial.0, e.code()));
            return e.code();
        }
    };
    let depth = if let Some(primary) = primary.as_ref().filter(|_| super::depth::paired(desc)) {
        let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT,
            CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
        match super::depth::Pair::new(&device.native, &heap, desc, primary) {
            Ok(pair) => Some(Box::new(pair)),
            Err(error) => return error.code(),
        }
    } else { None };
    let backing = super::views::TextureAddress::with_heap(device, &native, address as usize, false, None, render_heap, depth, 0, None);
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device, super::ResourceState::from(super::Backing::Addressed(backing, *desc)), out) }
}

pub(super) unsafe extern "system" fn committed_layout(
    device: &Device, heap: *const D3D12_HEAP_PROPERTIES, flags: D3D12_HEAP_FLAGS,
    guest: *const D3D12_RESOURCE_DESC, initial: D3D12_BARRIER_LAYOUT,
    clear: *const D3D12_CLEAR_VALUE, protected: *mut c_void, count: u32,
    formats: *const windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT, iid: *const GUID, out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() { return status; }
    let (Some(heap), Some(desc)) = (unsafe { (heap.as_ref(), guest.as_ref()) }) else { return E_INVALIDARG };
    if !protected.is_null() { return E_NOTIMPL; }
    if count > 0 && formats.is_null() { return E_INVALIDARG; }
    let packed = super::allocation::packed_uav_formats(desc.Format, desc.Flags);
    let castable = if count == 0 { (!packed.is_empty()).then_some(packed) }
        else { Some(unsafe { std::slice::from_raw_parts(formats, count as usize) }) };
    // Only the format translation applies here; a float depth plane with a
    // writable companion is not created through this entry point.
    let translated = super::depth::primary(desc).filter(|_| !super::depth::paired(desc));
    let initial = super::format::host_initial_layout(desc, initial);
    let host_clear = super::format::host_clear(clear);
    let clear = host_clear.as_ref().map_or(clear, core::ptr::from_ref);
    let host = D3D12_RESOURCE_DESC1 {
        Dimension: desc.Dimension, Alignment: desc.Alignment, Width: desc.Width, Height: desc.Height,
        DepthOrArraySize: desc.DepthOrArraySize, MipLevels: desc.MipLevels,
        Format: translated.map_or(desc.Format, |t| t.Format),
        SampleDesc: desc.SampleDesc, Layout: if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER && desc.Layout.0 >= 0x100 {
            D3D12_TEXTURE_LAYOUT_UNKNOWN
        } else { desc.Layout },
        Flags: translated.map_or(D3D12_RESOURCE_FLAGS(desc.Flags.0 & 0x1ff), |t| t.Flags), ..Default::default()
    };
    static FIRST_DISPLAY: std::sync::Once = std::sync::Once::new();
    if flags.contains(D3D12_HEAP_FLAG_ALLOW_DISPLAY) {
        crate::diagnostics::log_once(&FIRST_DISPLAY, || format!("display resource {}x{} format={} layout={:#x} flags={:#x} initial={}", desc.Width, desc.Height, desc.Format.0, desc.Layout.0, desc.Flags.0, initial.0));
    }
    let api: ID3D12Device10 = match device.native.cast() { Ok(device) => device, Err(e) => return e.code() };
    let mut native = None;
    if let Err(error) = unsafe { api.CreateCommittedResource3::<_, ID3D12Resource>(heap, flags, &host, initial, clear.as_ref().map(core::ptr::from_ref), None, castable, &mut native) } {
        crate::diagnostics::recording_error(format_args!("committed resource dim={} size={}x{} fmt={} layout={:#x} flags={:#x} initial={} heap={:#x} failed {:?}", desc.Dimension.0, desc.Width, desc.Height, desc.Format.0, desc.Layout.0, desc.Flags.0, initial.0, flags.0, error.code()));
        return error.code();
    }
    let Some(native) = native else { return E_POINTER; };
    if flags.contains(D3D12_HEAP_FLAG_ALLOW_DISPLAY) {
        crate::diagnostics::display::mark_resource(native.as_raw());
    }
    let backing = if flags.contains(D3D12_HEAP_FLAG_ALLOW_DISPLAY) {
        // Scanout allocations go through the title's D3D large-page allocator.
        // Keep the tiled guest extent and its native render target paired.
        let plane = match super::placement::color_plane(desc) {
            Ok(plane) => plane, Err(e) => { crate::diagnostics::recording_error(format_args!("display surface layout {:?}", e.code())); return e.code(); },
        };
        let memory = match super::title_memory::GuestMemory::allocate(plane.size as usize, 0x3080_8000) {
            Ok(memory) => memory, Err(e) => { crate::diagnostics::recording_error(format_args!("display allocation {} bytes failed {:?}", plane.size, e.code())); return e.code(); },
        };
        crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
            "display surface res={:#x} at {:#x}+{:#x}", native.as_raw() as usize, memory.address, plane.size));
        let address = super::views::TextureAddress::at(device, &native, memory.address, false, Some(memory));
        super::Backing::Addressed(address, *desc)
    } else if translated.is_some() {
        super::Backing::Translated(*desc)
    } else {
        super::Backing::ConsoleLayout(desc.Layout, desc.Flags)
    };
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device, super::ResourceState::from(backing), out) }
}

/// Neonbridge: report a refused CPU-memory heap import with the region's VirtualQuery view
/// (the host requires committed, private, read-write pages covering whole allocations).
/// A failed native import, logged once per (call, size, result, title
/// caller) for the first 256 such keys, with the title RVA that asked (TuxBox
/// 56cbb05, Valdi320, Apache-2.0: which path called the import). Each failure
/// was printed before (a fresh Once per call).
fn import_failure(call: &'static str, base: u64, size: u64, error: &windows::core::Error) {
    use crate::diagnostics::{Seen, Sighting};
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    static SEEN: Seen<(&'static str, u64, i32, usize)> = Seen::new(256);
    static FAILED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = FAILED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let caller = crate::survey::title_rva().unwrap_or(0);
    match SEEN.sight(&(call, size, error.code().0, caller)) {
        Sighting::New => {
            let mut r = MEMORY_BASIC_INFORMATION::default();
            unsafe { VirtualQuery(Some(base as _), &mut r, size_of_val(&r)) };
            crate::diagnostics::log_line(format_args!(
                "{call}({base:#x}, {size:#x}) failed {:#010x} for title RVA {caller:#x}: region base={:p} alloc={:p} size={:#x} state={:#x} protect={:#x} type={:#x} ({n} import failures so far)",
                error.code().0, r.BaseAddress, r.AllocationBase, r.RegionSize, r.State.0, r.Protect.0, r.Type.0));
        }
        Sighting::Full => crate::diagnostics::log_line(format_args!(
            "import failures: 256 distinct kinds logged, later new ones counted only ({n} so far)")),
        Sighting::Repeat => {}
    }
}

fn validate_shared_range(start: u64, end: u64) -> windows::core::Result<()> {
    validate_title_range(start, end, false)
}

/// Every page of `[start, end)` is committed read-write, or, with `lazy`, may
/// also be a reserved page of a read-write section view: xmem maps title views
/// of physical pages without committing them (lazy_commit.rs, xcl 38b05b7) and
/// commits each on first use. The GPU page pool is only registered as a range
/// here and its pages back video memory, so committing its 6.2 GB to pass the
/// check would undo that; test135 asserted at game+0x1e6c2c5 on the
/// E_INVALIDARG this check gave the reserved pool.
fn validate_title_range(start: u64, end: u64, lazy: bool) -> windows::core::Result<()> {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEM_MAPPED, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery};
    // Lazy read-write views xmem knows of pass without the VirtualQuery walk.
    let view = super::view_state(start as usize, end.saturating_sub(start) as usize);
    if view.committed() || (lazy && view.covered()) {
        return Ok(());
    }
    let mut cursor = start;
    while cursor < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(cursor as _), &mut region, size_of_val(&region)) } == 0 {
            return Err(E_INVALIDARG.into());
        }
        let committed = region.State == MEM_COMMIT && region.Protect == PAGE_READWRITE;
        let reserved_view = lazy && region.State == MEM_RESERVE && region.Type == MEM_MAPPED
            && region.AllocationProtect == PAGE_READWRITE;
        if !committed && !reserved_view {
            return Err(E_INVALIDARG.into());
        }
        cursor = (region.BaseAddress as u64).checked_add(region.RegionSize as u64).ok_or(E_INVALIDARG)?;
    }
    Ok(())
}

pub(super) unsafe extern "system" fn register_pages(
    device: &Device, address: u64, count: u32, out: *mut usize,
) -> HRESULT {
    if out.is_null() { return E_POINTER; }
    unsafe { out.write(0) };
    if address == 0 || !address.is_multiple_of(65536) || count == 0 { return E_INVALIDARG; }
    let bytes = u64::from(count) * 65536;
    let Some(end) = address.checked_add(bytes) else { return E_INVALIDARG; };
    if let Err(e) = validate_title_range(address, end, true) { return e.code(); }
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("registered Xbox GPU page pool: {count} pages at {address:#x}"));
    let token = device.state.page_pools.register(address, end);
    unsafe { out.write(token) };
    windows::Win32::Foundation::S_OK
}

pub(super) extern "system" fn unregister_pages(device: &Device, token: usize) {
    device.state.page_pools.0.lock().unwrap_or_else(|e| e.into_inner()).remove(&token);
}

// Registration returns an opaque cookie. Import the actual slices when a
// resource uses them, so a large title arena doesn't pin untouched pages.
#[derive(Default)]
pub(super) struct PagePools(std::sync::Mutex<std::collections::HashMap<usize, Box<PagePool>>>);

struct PagePool {
    /// Unique per registration: a pool registered after another was removed
    /// can reuse its Box address (the token), never its id.
    id: u64,
    start: u64,
    end: u64,
    /// The last single page opened, and the RETIRED count when it was.
    last_page: Option<(u64, std::sync::Weak<ImportedHeap>, u64)>,
}

impl PagePools {
    fn register(&self, start: u64, end: u64) -> usize {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pool = Box::new(PagePool { id, start, end, last_page: None });
        let token = core::ptr::from_ref(pool.as_ref()).addr();
        self.0.lock().unwrap_or_else(|e| e.into_inner()).insert(token, pool);
        token
    }

    /// A range a live import already covers exactly reuses that heap: test58
    /// spent 10% of the render thread opening heaps from addresses in the
    /// kernel, one per imported buffer. Only heaps something still holds are
    /// reused, so the title's memory there is the memory they map.
    /// TUXBOX_IMPORT_REUSE=0 opens every range again.
    fn import(&self, device: &ID3D12Device13, base: u64, size: u64, pin: ImportPin) -> windows::core::Result<Arc<ImportedHeap>> {
        use std::sync::Weak;
        static REUSED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if crate::env_toggle!("TUXBOX_IMPORT_REUSE") == Some("0") {
            return self.import_uncached(device, base, size, pin);
        }
        let key = (device.as_raw() as usize, base, size);
        if let Some(heap) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)).and_then(Weak::upgrade) {
            let n = REUSED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n.is_power_of_two() && n >= 64 {
                crate::diagnostics::log_line(format_args!("imported heaps reused for a range a live import covers: {n} (TUXBOX_IMPORT_REUSE=0 off)"));
            }
            return Ok(heap);
        }
        // test71 froze with five recording threads in the kernel opening heaps
        // at once. Only one thread opens a given range; the others wait for
        // it and take its heap. TUXBOX_IMPORT_SINGLE=0 lets each open its own.
        static WAITED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let single = crate::env_toggle!("TUXBOX_IMPORT_SINGLE") != Some("0");
        if single {
            let mut opening = OPENING.0.lock().unwrap_or_else(|e| e.into_inner());
            let mut waited = false;
            while opening.contains(&key) {
                waited = true;
                opening = OPENING.1.wait(opening).unwrap_or_else(|e| e.into_inner());
            }
            if waited {
                let n = WAITED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!("heap import: a thread waited for another's open of the same {size:#x}-byte range at {base:#x} ({n} so far; TUXBOX_IMPORT_SINGLE=0 off)"));
                }
                if let Some(heap) = LIVE.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)).and_then(Weak::upgrade) {
                    return Ok(heap);
                }
            }
            opening.push(key);
            let now = opening.len();
            if now > PEAK.fetch_max(now, std::sync::atomic::Ordering::Relaxed) && now >= 3 {
                crate::diagnostics::log_line(format_args!("heap import: {now} different ranges being opened at once (new peak)"));
            }
        }
        let heap = self.import_uncached(device, base, size, pin);
        if let Ok(heap) = &heap {
            note_live(key, heap);
            keep_recent(heap);
        }
        if single {
            OPENING.0.lock().unwrap_or_else(|e| e.into_inner()).retain(|k| *k != key);
            OPENING.1.notify_all();
        }
        heap
    }

    fn import_uncached(&self, device: &ID3D12Device13, base: u64, size: u64, pin: ImportPin) -> windows::core::Result<Arc<ImportedHeap>> {
        // A range the host refused is refused again for 5 s without asking:
        // each retry cost an OpenExistingHeapFromAddress1 before the caller
        // fell back to its shadow copy. The window lets a remapped range in.
        let open = || open_heap(device, base, size, pin);
        if size == u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) {
            let mut pools = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pool) = pools.values_mut().find(|pool| pool.start <= base && base + size <= pool.end) {
                // Nearby suballocations often share one page. The resources
                // keep their own heap references after this cache advances.
                let retired = RETIRED.load(std::sync::atomic::Ordering::Acquire);
                if let Some((cached, heap, at)) = &pool.last_page && *cached == base && *at == retired && let Some(heap) = heap.upgrade() {
                    return Ok(heap);
                }
                let (key, id) = (core::ptr::from_ref(pool.as_ref()).addr(), pool.id);
                if !super::views::view_lockfree() {
                    let heap = open()?;
                    pool.last_page = Some((base, Arc::downgrade(&heap), retired));
                    return Ok(heap);
                }
                // Open without the pool lock (an import takes milliseconds and
                // every recording thread's views wait on it; rebox b077562).
                drop(pools);
                let heap = open()?;
                let mut pools = self.0.lock().unwrap_or_else(|e| e.into_inner());
                // Only the pool the heap was opened for: one unregistered
                // meanwhile and another allocated at its address is not it.
                if let Some(pool) = pools.get_mut(&key).filter(|pool| pool.id == id) {
                    pool.last_page = Some((base, Arc::downgrade(&heap), retired));
                }
                return Ok(heap);
            }
        }
        open()
    }
}
