//! Device memory heaps and resources sharing their native GPU allocation.
mod import;
mod virtual_place;
mod pages;
use import::trace_import_failure;
pub(super) use import::import_buffer_with_heap;
use import::import_with_commit;
use import::buffer_standin;
pub(super) use import::ImportedHeap;
pub(super) use virtual_place::place_virtual;
pub(super) use virtual_place::place_virtual_layout;
pub(super) use pages::register_pages;
pub(super) use pages::PagePools;

use super::{Device, Out, RESOURCE, identity, wrap_pageable};
use crate::proxy::{Proxy, Slots, check_output, child_slots, unwrap};
use core::ffi::c_void;
use crate::commands::page_mapping::{GPU_APERTURE, ImportPin};
use std::sync::Arc;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::{ID3D12Pageable, *};
use windows::core::{GUID, HRESULT, Interface};

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

fn commit_guest_pages(start: u64, bytes: u64) -> bool {
    use windows::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, PAGE_READWRITE};
    if bytes == 0 || bytes > 1 << 30 || start.checked_add(bytes).is_none() {
        return false;
    }
    // Commit only this view. The surrounding reservation stays a reservation.
    !unsafe { VirtualAlloc(Some(start as _), bytes as usize, MEM_COMMIT, PAGE_READWRITE) }.is_null()
}

fn import_or_back(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES, source: &'static str)
    -> windows::core::Result<ID3D12Resource> {
    match import_with_commit(device, address, desc, state, source) {
        Ok((native, _)) => Ok(native),
        Err(error) => buffer_standin(device, address, desc, state, source, error),
    }
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

pub(super) extern "system" fn unregister_pages(device: &Device, token: usize) {
    device.state.page_pools.pools.lock().unwrap_or_else(|e| e.into_inner()).remove(&token);
}

#[path = "heap_page_cache.rs"]
mod heap_page_cache;
use heap_page_cache::{PageCache, span_key};

struct PagePool {
    start: u64,
    end: u64,
    pages: PageCache<ImportedHeap>,
}
