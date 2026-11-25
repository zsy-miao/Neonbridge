//! Importing title memory as a buffer.
use super::*;

// This trace identifies which provider path called the native import API.
// Opt-in and capped; never log the host address or alter the return code.
pub(super) fn trace_import_failure(source: &'static str, api: &'static str, size: u64, status: HRESULT) {
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

// This holder is also the key for the weak single-page import cache. Its
// last native resource releases both the heap reference and the VA pin.
pub(in super::super) struct ImportedHeap {
    pub(super) native: ID3D12Heap,
    pub(super) _pin: ImportPin,
}

// Slot 44 places a tiled resource at a title-selected virtual address. An
// unrelated allocation would lose texture contents and subsequent page remaps.
pub(in super::super) fn import_buffer_with_heap(
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
        Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::super::format::HOST_FLAGS)
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

pub(super) fn device_buffer(device: &Device, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES)
    -> windows::core::Result<ID3D12Resource> {
    let host = D3D12_RESOURCE_DESC {
        Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::super::format::HOST_FLAGS),
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
pub(super) fn import_with_commit(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES, source: &'static str)
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
                Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & super::super::format::HOST_FLAGS),
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

pub(super) fn buffer_standin(device: &Device, address: u64, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES,
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

impl ImportedHeap {
    /// False once a page remap replaced the pages this heap was imported from.
    pub(in super::super) fn is_live(&self) -> bool {
        self._pin.is_live()
    }

    pub(in super::super) fn buffer_alias(self: &Arc<Self>, device: &ID3D12Device, offset: u64,
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
