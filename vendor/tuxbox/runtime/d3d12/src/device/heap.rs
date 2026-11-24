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
static HEAP: Slots<9> = {
    let mut table = child_slots::<ID3D12Heap, D3D12_HEAP_DESC, 9>();
    table[8] = crate::slot!(get_desc);
    table
};

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
    validate_shared_range(base, base.checked_add(size).ok_or(E_INVALIDARG)?)?;
    // A guest can place thousands of tiny resources in one multi-GiB XMem
    // reservation. Pin only the pages needed by this resource on the adapter.
    let heap = if let Ok(sized) = device.native.cast::<ID3D12Device13>() {
        device.state.page_pools.import(&sized, base, size, pin)?
    } else {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(base as _), &mut region, size_of_val(&region)) } == 0
            || region.AllocationBase as u64 != base || region.RegionSize as u64 != size {
            return Err(E_NOTIMPL.into());
        }
        let host: ID3D12Device3 = device.native.cast()?;
        let native = unsafe { host.OpenExistingHeapFromAddress(region.AllocationBase) }
            .inspect_err(|e| import_failure("OpenExistingHeapFromAddress", base, size, e))?;
        crate::diagnostics::memory::note(crate::diagnostics::memory::Category::HeapImport, size);
        Arc::new(ImportedHeap { native, _pin: pin })
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
        let native = import_buffer(device, address, desc, state);
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
            return Ok((heap, offset));
        }
        // The title reuses guest memory for a larger or differently placed
        // image. Cover every overlapping heap's range with a new heap so later
        // placements here alias each other. Earlier images keep their heap.
        let (mut start, mut stop) = (address, end);
        for (&base, entry) in heaps.range(..end) {
            if let Some(heap) = entry.upgrade() && heap.end > address {
                start = start.min(base);
                stop = stop.max(heap.end);
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
        crate::diagnostics::memory::note(crate::diagnostics::memory::Category::RenderHeap, desc.SizeInBytes);
        let heap = Arc::new(RenderHeap { native: native.ok_or(E_POINTER)?, end: stop });
        heaps.insert(start, Arc::downgrade(&heap));
        Ok((heap, address - start))
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
        if let Err(e) = validate_shared_range(address, end) {
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
fn import_failure(call: &str, base: u64, size: u64, error: &windows::core::Error) {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut r = MEMORY_BASIC_INFORMATION::default();
    unsafe { VirtualQuery(Some(base as _), &mut r, size_of_val(&r)) };
    crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
        "{call}({base:#x}, {size:#x}) failed {:#010x}: region base={:p} alloc={:p} size={:#x} state={:#x} protect={:#x} type={:#x}",
        error.code().0, r.BaseAddress, r.AllocationBase, r.RegionSize, r.State.0, r.Protect.0, r.Type.0));
}

fn validate_shared_range(start: u64, end: u64) -> windows::core::Result<()> {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualQuery};
    let mut cursor = start;
    while cursor < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(cursor as _), &mut region, size_of_val(&region)) } == 0
            || region.State != MEM_COMMIT || region.Protect != PAGE_READWRITE {
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
    if let Err(e) = validate_shared_range(address, end) { return e.code(); }
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
    start: u64,
    end: u64,
    last_page: Option<(u64, std::sync::Weak<ImportedHeap>)>,
}

impl PagePools {
    fn register(&self, start: u64, end: u64) -> usize {
        let pool = Box::new(PagePool { start, end, last_page: None });
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
        use std::collections::HashMap;
        use std::sync::{Mutex, Weak};
        static LIVE: Mutex<Option<HashMap<(usize, u64, u64), Weak<ImportedHeap>>>> = Mutex::new(None);
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
        static OPENING: (Mutex<Vec<(usize, u64, u64)>>, std::sync::Condvar) = (Mutex::new(Vec::new()), std::sync::Condvar::new());
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
            let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
            let map = live.get_or_insert_with(HashMap::new);
            if map.len() >= 1 << 16 {
                map.retain(|_, h| h.strong_count() != 0);
            }
            map.insert(key, Arc::downgrade(heap));
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
        static REFUSED: std::sync::Mutex<Vec<(u64, u64, std::time::Instant)>> = std::sync::Mutex::new(Vec::new());
        let open = || -> windows::core::Result<Arc<ImportedHeap>> {
            {
                let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
                refused.retain(|(.., at)| at.elapsed() < std::time::Duration::from_secs(5));
                if refused.iter().any(|&(b, s, _)| b == base && s == size) {
                    return Err(windows::Win32::Foundation::E_INVALIDARG.into());
                }
            }
            let mut native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) };
            // The title maps one 64 KiB page per call, so a GPU range is many OS
            // views and no heap spans it. xmem rejoins views of one section at
            // consecutive offsets into one view on request; then retry once.
            if let Err(e) = &native && e.code() == windows::Win32::Foundation::E_INVALIDARG
                && size > u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) && join_page_range(base, size)
            {
                native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) };
            }
            let native = native
                .inspect_err(|e| {
                    import_failure("OpenExistingHeapFromAddress1", base, size, e);
                    if e.code() == windows::Win32::Foundation::E_INVALIDARG {
                        let mut refused = REFUSED.lock().unwrap_or_else(|e| e.into_inner());
                        if refused.len() < 4096 { refused.push((base, size, std::time::Instant::now())); }
                    }
                })?;
            crate::diagnostics::memory::note(crate::diagnostics::memory::Category::HeapImport, size);
            Ok(Arc::new(ImportedHeap { native, _pin: pin }))
        };
        if size == u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) {
            let mut pools = self.0.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pool) = pools.values_mut().find(|pool| pool.start <= base && base + size <= pool.end) {
                // Nearby suballocations often share one page. The resources
                // keep their own heap references after this cache advances.
                if let Some((cached, heap)) = &pool.last_page && *cached == base && let Some(heap) = heap.upgrade() {
                    return Ok(heap);
                }
                let key = core::ptr::from_ref(pool.as_ref()).addr();
                if !super::views::view_lockfree() {
                    let heap = open()?;
                    pool.last_page = Some((base, Arc::downgrade(&heap)));
                    return Ok(heap);
                }
                // Open without the pool lock (an import takes milliseconds and
                // every recording thread's views wait on it; rebox b077562).
                drop(pools);
                let heap = open()?;
                let mut pools = self.0.lock().unwrap_or_else(|e| e.into_inner());
                if let Some(pool) = pools.get_mut(&key) {
                    pool.last_page = Some((base, Arc::downgrade(&heap)));
                }
                return Ok(heap);
            }
        }
        open()
    }
}
