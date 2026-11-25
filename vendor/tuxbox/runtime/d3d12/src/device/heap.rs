//! Device memory heaps and resources sharing their native GPU allocation.
use super::{Device, Out, RESOURCE, identity, wrap_pageable};
use crate::proxy::{Proxy, Slots, check_output, child_slots, unwrap};
use core::ffi::c_void;
use crate::commands::page_mapping::{GPU_APERTURE, ImportPin};
use std::sync::Arc;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::{ID3D12Pageable, *};
use windows::core::{GUID, HRESULT, Interface};

// This trace identifies which provider path called the native import API.
// Opt-in and capped; never log the host address or alter the return code.
fn trace_import_failure(source: &'static str, api: &'static str, size: u64, status: HRESULT) {
    use crate::diagnostics::{Seen, Sighting};
    static ENABLED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("TUXBOX_TRACE_HEAP_IMPORTS").as_deref() == Ok("1")
    });
    static SEEN: Seen<(&'static str, &'static str, u64, u32, u64)> = Seen::new(256);
    if !*ENABLED || !crate::diagnostics::logging() { return; }
    let caller = crate::survey::title_rva().unwrap_or(0) as u64;
    let key = (source, api, size, status.0 as u32, caller);
    match SEEN.sight(&key) {
        Sighting::New => crate::diagnostics::log(&format!(
            "heap import failure source={source} api={api} bytes={size} hr={:#010x} caller={caller:#x}", status.0 as u32
        )),
        Sighting::Full => crate::diagnostics::log("heap import trace capped at 256 distinct failures"),
        Sighting::Repeat => (),
    }
}

// Distinguish a successful import from a successful but unrelated device
// allocation. Keep default behaviour unchanged; strict mode is for diagnosis.
fn trace_buffer_path(path: &'static str, count: &std::sync::atomic::AtomicU64, source: &'static str, bytes: u64) {
    use std::sync::atomic::Ordering;
    static ENABLED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("TUXBOX_TRACE_BUFFER_BACKING").as_deref() == Ok("1")
    });
    if !*ENABLED || !crate::diagnostics::logging() { return; }
    let n = count.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log(&format!("buffer backing path={path} count={n} source={source} bytes={bytes}"));
    }
}

fn strict_buffer_import() -> bool {
    static STRICT: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| {
        std::env::var("TUXBOX_REQUIRE_BUFFER_IMPORT").as_deref() == Ok("1")
    });
    *STRICT
}

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
// GetDesc is slot 8. A protected-session query and a GPU address follow it.
// A shorter table would read off the end, so those calls fail closed.
static HEAP: Slots<11> = {
    let mut table = child_slots::<ID3D12Heap, D3D12_HEAP_DESC, 11>();
    table[8] = crate::slot!(get_desc);
    table
};

pub(super) unsafe fn memory_heap<'a>(
    object: *mut c_void,
    owner: *mut c_void,
) -> Option<&'a ID3D12Heap> {
    // SAFETY: unwrap checks the heap vtable and the owning device.
    unsafe { unwrap::<ID3D12Heap, D3D12_HEAP_DESC>(object, owner, &HEAP).map(|heap| &heap.native) }
}

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
pub(super) fn import_buffer_with_heap(
    device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES,
    source: &'static str,
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
    // Console-only bits stay on the guest's description, not the host's.
    let host_desc = D3D12_RESOURCE_DESC {
        Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::format::HOST_FLAGS)
            | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | tight,
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
        device.state.page_pools.import(&sized, base, size, pin, source)?
    } else {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(base as _), &mut region, size_of_val(&region)) } == 0
            || region.AllocationBase as u64 != base || region.RegionSize as u64 != size {
            return Err(E_NOTIMPL.into());
        }
        let host: ID3D12Device3 = device.native.cast()?;
        let native = unsafe { host.OpenExistingHeapFromAddress(region.AllocationBase) }.map_err(|error| {
            trace_import_failure(source, "legacy", size, error.code());
            error
        })?;
        Arc::new(ImportedHeap { native, _pin: pin })
    };
    let mut resource = None;
    unsafe { device.native.CreatePlacedResource(&heap.native, offset, &host_desc, state, None, &mut resource)? };
    let resource: ID3D12Resource = resource.ok_or(E_POINTER)?;
    // Placed resources borrow their heap. Keep the imported CPU allocation's
    // heap alive through the final native resource reference (including views).
    attach_import(&resource, &heap)?;
    static IMPORTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    trace_buffer_path("imported", &IMPORTED, source, desc.Width);
    Ok((resource, heap))
}

fn commit_guest_pages(start: u64, bytes: u64) -> bool {
    use windows::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, PAGE_READWRITE};
    if bytes == 0 || bytes > 1 << 30 || start.checked_add(bytes).is_none() {
        return false;
    }
    // Commit only this view. The surrounding reservation stays a reservation.
    !unsafe { VirtualAlloc(Some(start as _), bytes as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

fn device_buffer(device: &Device, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES)
    -> windows::core::Result<ID3D12Resource> {
    let host = D3D12_RESOURCE_DESC {
        Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::format::HOST_FLAGS),
        ..*desc
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native = None;
    unsafe { device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &host, state, None, &mut native)? };
    native.ok_or(E_POINTER.into())
}

// A late commitment must return the heap as well as the resource. The
// layout-based guest address registry uses that heap to reject stale imports.
fn import_with_commit(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES, source: &'static str)
    -> windows::core::Result<(ID3D12Resource, Arc<ImportedHeap>)> {
    match import_buffer_with_heap(device, address, desc, state, source) {
        Ok(import) => Ok(import),
        Err(error) => {
            let base = address & !0xffff;
            let offset = address - base;
            // Match the native import's size/alignment query. Do not commit
            // guessed ranges for invalid descriptors or overflowing addresses.
            let tight = if offset != 0 { D3D12_RESOURCE_FLAGS(0x400) } else { D3D12_RESOURCE_FLAG_NONE };
            let legacy = D3D12_RESOURCE_DESC {
                Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::format::HOST_FLAGS),
                ..*desc
            };
            let host = D3D12_RESOURCE_DESC {
                Flags: legacy.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | tight,
                ..legacy
            };
            let info = unsafe { device.native.GetResourceAllocationInfo(0, &[host]) };
            let size = (info.SizeInBytes != u64::MAX && info.Alignment != 0
                && offset.is_multiple_of(info.Alignment))
                .then(|| offset.checked_add(info.SizeInBytes)?.checked_next_multiple_of(65536))
                .flatten()
                .or_else(|| {
                    // Preserve the legacy commitment attempt for stand-in
                    // compatibility when host import alignment is unavailable.
                    // Strict diagnostic mode never commits a guessed span.
                    if strict_buffer_import() { return None; }
                    let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(&legacy)) };
                    let bytes = if info.SizeInBytes == u64::MAX { desc.Width } else { info.SizeInBytes };
                    offset.checked_add(bytes)?.checked_next_multiple_of(65536)
                });
            if let Some(size) = size
                && commit_guest_pages(base, size)
                && let Ok(import) = import_buffer_with_heap(device, address, desc, state, source) {
                static COMMITTED: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&COMMITTED, || format!(
                    "placed buffer at {address:#x} bytes={} committed reserved guest pages", desc.Width));
                static RETRIED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                trace_buffer_path("commit_retry", &RETRIED, source, desc.Width);
                return Ok(import);
            }
            Err(error)
        }
    }
}

fn buffer_standin(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES,
                  source: &'static str, error: windows::core::Error) -> windows::core::Result<ID3D12Resource> {
    static FALLBACK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static REJECTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if strict_buffer_import() {
        trace_buffer_path("strict_rejected", &REJECTED, source, desc.Width);
        return Err(error);
    }
    let native = device_buffer(device, desc, state)?;
    trace_buffer_path("device_standin", &FALLBACK, source, desc.Width);
    static DEVICE: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&DEVICE, || format!(
        "placed buffer at {address:#x} bytes={} is not importable guest memory ({:?}); using a device buffer",
        desc.Width, error.code()));
    Ok(native)
}

fn import_or_back(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES, source: &'static str)
    -> windows::core::Result<ID3D12Resource> {
    match import_with_commit(device, address, desc, state, source) {
        Ok((native, _)) => Ok(native),
        Err(error) => buffer_standin(device, address, desc, state, source, error),
    }
}

impl ImportedHeap {
    /// False once a page remap replaced the pages this heap was imported from.
    pub(super) fn is_live(&self) -> bool {
        self._pin.is_live()
    }

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
        let native = import_or_back(device, address, desc, state, "place_virtual");
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
        && super::placement::console_layout(desc.Layout.0) {
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
    let (native, heap, delta) = match import_with_commit(device, base, &host, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout") {
        Ok((native, heap)) => (native, heap, delta),
        Err(error) => {
            // If the widened import is unavailable, keep the old direct
            // import as a fallback. A real import must never be registered
            // as the zero-address device-local stand-in.
            if base != address || host.Flags != desc.Flags {
                if let Ok((native, heap)) = import_with_commit(device, address, desc, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout") {
                    let buffer = super::views::TextureAddress::with_heap(
                        device, &native, address as usize, false, None, None, None, 0, Some(heap),
                    );
                    return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                        super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
                }
            }
            let native = match buffer_standin(device, address, desc, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout", error.clone()) {
                Ok(native) => native,
                Err(fallback) => {
                    crate::diagnostics::log(&format!(
                        "placed buffer at {address:#x} bytes={} native offset={delta:#x}: import {:?} stand-in {:?}",
                        desc.Width, error.code(), fallback.code()));
                    return fallback.code();
                }
            };
            let buffer = super::views::TextureAddress::at(device, &native, 0, false, None);
            return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) };
        }
    };
    let buffer = super::views::TextureAddress::with_heap(
        device, &native, address as usize, false, None, None, None, delta, Some(heap),
    );
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
        super::ResourceState::from(super::Backing::Addressed(buffer, *desc)), out) }
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
            && base <= address && end <= heap.end && (address - base).is_multiple_of(alignment) {
            return Ok((heap, address - base));
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
        let overlapped: Vec<u64> = heaps.range(start..stop).map(|(&base, _)| base).collect();
        for base in overlapped {
            heaps.remove(&base);
        }
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: stop - start, Alignment: alignment.max(65536),
            Properties: D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() },
            Flags: if mixed { D3D12_HEAP_FLAG_ALLOW_ALL_BUFFERS_AND_TEXTURES } else { D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES },
        };
        let mut native = None;
        unsafe { device.CreateHeap(&desc, &mut native)? };
        let heap = Arc::new(RenderHeap { native: native.ok_or(E_POINTER)?, end: stop });
        heaps.insert(start, Arc::downgrade(&heap));
        Ok((heap, address - start))
    }
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
        if allocation.Alignment == 0 || allocation.SizeInBytes == u64::MAX || !address.is_multiple_of(allocation.Alignment) {
            crate::diagnostics::recording_error(format_args!("adapter image requirements {:#x}/{:#x}, array={} mips={} format={} samples={}/{}", allocation.SizeInBytes, allocation.Alignment, desc.DepthOrArraySize, desc.MipLevels, desc.Format.0, desc.SampleDesc.Count, desc.SampleDesc.Quality));
            return Err(E_INVALIDARG.into());
        }
        let renderable = (host.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL)).0 != 0;
        step = "guest memory";
        crate::diagnostics::watch("device-memory image", address, allocation.SizeInBytes, || format!(
            "{}x{}x{} format={} flags={:#x}", desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Flags.0));
        let footprint = address.checked_add(allocation.SizeInBytes).ok_or(E_INVALIDARG)?;
        if validate_shared_range(address, footprint).is_err() {
            let base = address & !0xffff;
            let end = footprint.saturating_add(0xffff) & !0xffff;
            if !commit_guest_pages(base, end.saturating_sub(base))
                || validate_shared_range(address, footprint).is_err() {
                static RESERVED: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&RESERVED, || format!(
                    "placed GPU image at {address:#x} is not committed guest memory; keeping a device image"));
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
    // A float depth plane with a writable companion is not created through this entry point.
    let paired = super::depth::paired(desc);
    let translated = super::depth::primary(desc).filter(|_| !paired || desc.Format == super::format::D16_UNORM_S8_UINT);
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
    } else if let Some(primary) = translated.as_ref().filter(|_| paired) {
        match super::depth::Pair::new(&device.native, heap, desc, primary) {
            Ok(pair) => super::Backing::Depth(Box::new(pair)),
            Err(error) => return error.code(),
        }
    } else if translated.is_some() {
        super::Backing::Translated(*desc)
    } else {
        super::Backing::ConsoleLayout(desc.Layout, desc.Flags)
    };
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device, super::ResourceState::from(backing), out) }
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
    device.state.page_pools.pools.lock().unwrap_or_else(|e| e.into_inner()).remove(&token);
}

#[path = "heap_page_cache.rs"]
mod heap_page_cache;
use heap_page_cache::{PageCache, span_key};

// Registration returns an opaque cookie. Import the actual slices when a
// resource uses them, so a large title arena doesn't pin untouched pages.
#[derive(Default)]
pub(super) struct PagePools {
    pools: std::sync::Mutex<std::collections::HashMap<usize, Box<PagePool>>>,
    // Single-page imports outside every registered pool. Titles place many
    // resources on the same page: ProjectScarlett 1.0.2.1 imported one page
    // 9,352 times in its first minute. Each fresh host heap is another driver
    // buffer, and RADV passes every live buffer with each submission, so the
    // kernel's per-submit validation grows with the import count.
    //
    // Imports of several pages share this cache under a key that also holds
    // their page count. The same title views five fixed ranges of 448 KiB to
    // 1.2 MiB about five times a frame each, and its recording allocators
    // keep every import for 216 frames: about 4,500 live buffers.
    loose: std::sync::Mutex<PageCache<ImportedHeap>>,
}

struct PagePool {
    start: u64,
    end: u64,
    pages: PageCache<ImportedHeap>,
}

impl PagePools {
    fn register(&self, start: u64, end: u64) -> usize {
        let pool = Box::new(PagePool { start, end, pages: PageCache::default() });
        let token = core::ptr::from_ref(pool.as_ref()).addr();
        self.pools.lock().unwrap_or_else(|e| e.into_inner()).insert(token, pool);
        token
    }

    fn import(&self, device: &ID3D12Device13, base: u64, size: u64, pin: ImportPin, source: &'static str) -> windows::core::Result<Arc<ImportedHeap>> {
        let open = || -> windows::core::Result<Arc<ImportedHeap>> {
            let native = unsafe { device.OpenExistingHeapFromAddress1(base as _, size as usize) }.map_err(|error| {
                trace_import_failure(source, "sized", size, error.code());
                error
            })?;
            Ok(Arc::new(ImportedHeap { native, _pin: pin }))
        };
        if size == u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT) {
            use std::sync::atomic::AtomicU64;
            let mut pools = self.pools.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pool) = pools.values_mut().find(|pool| pool.start <= base && base + size <= pool.end) {
                static OPEN: AtomicU64 = AtomicU64::new(0);
                static HIT: AtomicU64 = AtomicU64::new(0);
                return reuse_page(&mut pool.pages, base, open, [("pool_open", &OPEN), ("pool_hit", &HIT)], source, size);
            }
            drop(pools);
            static OPEN: AtomicU64 = AtomicU64::new(0);
            static HIT: AtomicU64 = AtomicU64::new(0);
            let mut loose = self.loose.lock().unwrap_or_else(|e| e.into_inner());
            return reuse_page(&mut loose, base, open, [("page_open", &OPEN), ("page_hit", &HIT)], source, size);
        }
        if let Some(key) = span_key(base, size) {
            use std::sync::atomic::AtomicU64;
            static OPEN: AtomicU64 = AtomicU64::new(0);
            static HIT: AtomicU64 = AtomicU64::new(0);
            let mut loose = self.loose.lock().unwrap_or_else(|e| e.into_inner());
            return reuse_page(&mut loose, key, open, [("span_open", &OPEN), ("span_hit", &HIT)], source, size);
        }
        open()
    }
}

// Interleaved allocations can revisit a page whose heap is still owned by a
// resource. Reuse only that exact live import, while its pages are unchanged.
fn reuse_page(
    pages: &mut PageCache<ImportedHeap>, base: u64,
    open: impl FnOnce() -> windows::core::Result<Arc<ImportedHeap>>,
    [(open_path, opens), (hit_path, hits)]: [(&'static str, &std::sync::atomic::AtomicU64); 2],
    source: &'static str, size: u64,
) -> windows::core::Result<Arc<ImportedHeap>> {
    let mut opened = false;
    let result = pages.get_or_open(base, ImportedHeap::is_live, || {
        opened = true;
        open()
    });
    // The native resource owns the Arc; the cache is weak.
    // Count only successful imports and hits, not failed opens.
    if result.is_ok() {
        if opened {
            trace_buffer_path(open_path, opens, source, size);
        } else {
            trace_buffer_path(hit_path, hits, source, size);
        }
    }
    result
}
