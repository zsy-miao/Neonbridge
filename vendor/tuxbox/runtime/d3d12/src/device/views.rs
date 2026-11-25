//! Standard Xbox resource views and samplers, device slots 17 through 22.
mod byte_offset;
mod cbv_snapshot;
mod shadow;
pub(super) use byte_offset::AddressViews;
pub(crate) use shadow::Shadows;

use super::{Device, identity, resource};
use crate::proxy::fail_call;
use core::ffi::c_void;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

fn check_destination(handle: D3D12_CPU_DESCRIPTOR_HANDLE) {
    if handle.ptr == 0 {
        fail_call(format_args!("null D3D12 descriptor destination"));
    }
}

// A null resource and an explicit description create a typed null descriptor.
// Non-null resources are borrowed from their guest proxies throughout the call.
unsafe fn view_resource<'a, D>(
    device: &Device,
    object: *mut c_void,
    desc: *const D,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Option<&'a ID3D12Resource> {
    check_destination(handle);
    if object.is_null() {
        if desc.is_null() {
            fail_call(format_args!(
                "null D3D12 view needs an explicit description"
            ));
        }
        return None;
    }
    Some(
        unsafe { resource(object, identity(device).as_raw()) }
            .unwrap_or_else(|| fail_call(format_args!("invalid D3D12 view resource"))),
    )
}

pub(super) unsafe extern "system" fn cbv(
    device: &Device,
    desc: *const D3D12_CONSTANT_BUFFER_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    super::view_formats::write(handle.ptr, 0);
    check_destination(handle);
    device.state.placements.views.set(handle.ptr, None, false);
    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, None); }
    device.state.blas.views.store(handle.ptr, None);
    // Xbox uses the actual constant data length. A desktop CBV's size is in
    // 256-byte units. Keep the address at the start chosen by the guest.
    let mut aligned = unsafe { desc.as_ref() }.copied();
    let mut owner = None;
    if let Some(value) = &mut aligned {
        // ScarlettCompat 2ffcb48 (MIT): a size past 64 KiB, or one not a
        // multiple of 256, is a debug-layer error that can remove the device.
        // A shader reads at most 4096 constants, so the view stops there.
        value.SizeInBytes = value.SizeInBytes
            .min(D3D12_REQ_CONSTANT_BUFFER_ELEMENT_COUNT * 16)
            .next_multiple_of(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT);
        let address = value.BufferLocation;
        if address != 0 && value.SizeInBytes != 0 {
            let guest = unsafe { &*desc }.SizeInBytes.max(1);
            let copied = (cbv_snapshot::on()
                && crate::commands::page_mapping::remapped_import_over(address, address.saturating_add(u64::from(value.SizeInBytes))))
                .then(|| cbv_snapshot::copy(device, address, u64::from(guest), u64::from(value.SizeInBytes)).ok())
                .flatten();
            if let Some((gpu, chunk)) = copied {
                value.BufferLocation = gpu;
                owner = Some(chunk);
            } else {
            match buffer_at(device, address, u64::from(value.SizeInBytes), 1, false) {
                Ok(buffer) => {
                    if let Some(buffer) = &buffer {
                        value.BufferLocation = unsafe { buffer.native.GetGPUVirtualAddress() } + buffer.offset(address);
                    }
                    owner = buffer;
                }
                Err(e) if !cbv_fallback() => fail_call(format_args!("CBV buffer {address:#x}, {} bytes: {:?}", value.SizeInBytes, e.code())),
                // A range over title pages no heap opens (two views that would
                // not join): read it from a copy, which imports piece by piece.
                Err(e) => match shadow::view(device, address, u64::from(guest)) {
                    Ok(copy) => {
                        cbv_fell_back(address, guest, e.code(), true);
                        value.BufferLocation = unsafe { copy.native.GetGPUVirtualAddress() };
                        owner = Some(copy);
                    }
                    Err(_) => {
                        cbv_fell_back(address, guest, e.code(), false);
                        value.BufferLocation = 0;
                    }
                },
            }
            }
            // Xbox constant buffers sit on 16-byte boundaries; read one that
            // misses a 256-byte host boundary from a copy that starts on one.
            if value.BufferLocation != 0
                && !value.BufferLocation.is_multiple_of(u64::from(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT))
            {
                match shadow::view(device, address, u64::from(guest)) {
                    Ok(copy) => {
                        value.BufferLocation = unsafe { copy.native.GetGPUVirtualAddress() };
                        owner = Some(copy);
                    }
                    Err(e) if !cbv_fallback() => fail_call(format_args!("CBV copy {address:#x}, {guest} bytes: {:?}", e.code())),
                    Err(e) => {
                        cbv_fell_back(address, guest, e.code(), false);
                        value.BufferLocation = 0;
                        owner = None;
                    }
                }
            }
        }
    }
    // The 256-byte round-up must not run past the buffer under the view (a
    // debug-layer error that can remove the device): it stops at the last
    // whole 256 bytes inside it, a null CBV when none fit.
    // TUXBOX_CBV_CLAMP_END=0 keeps the rounded size.
    if let (Some(value), Some(buffer)) = (&mut aligned, &owner)
        && value.BufferLocation != 0 && crate::env_toggle!("TUXBOX_CBV_CLAMP_END") != Some("0")
    {
        let (start, width) = unsafe { (buffer.native.GetGPUVirtualAddress(), buffer.native.GetDesc().Width) };
        if let Some(room) = (start + width).checked_sub(value.BufferLocation)
            && u64::from(value.SizeInBytes) > room
        {
            value.SizeInBytes = (room / 256 * 256) as u32;
            if value.SizeInBytes == 0 {
                value.BufferLocation = 0;
            }
        }
    }
    // A desktop runtime removes the device for an invalid CBV description.
    if let (Some(value), Some(guest)) = (&aligned, unsafe { desc.as_ref() })
        && (!value.BufferLocation.is_multiple_of(256) || !value.SizeInBytes.is_multiple_of(256)
            || value.SizeInBytes > 65536 || (value.BufferLocation != 0 && owner.is_none()))
    {
        crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
            "invalid CBV: guest {:#x}+{} host {:#x}+{} imported={}",
            guest.BufferLocation, guest.SizeInBytes, value.BufferLocation, value.SizeInBytes, owner.is_some()));
    }
    // A view left without a buffer is written as a null CBV, not a size at
    // address zero.
    if let Some(value) = &mut aligned && value.BufferLocation == 0 {
        *value = D3D12_CONSTANT_BUFFER_VIEW_DESC::default();
    }
    device.state.imported_views.store(handle.ptr, Some(DescriptorBacking::ConstantBuffer { _address: owner }));
    unsafe {
        device
            .native
            .CreateConstantBufferView(aligned.as_ref().map(core::ptr::from_ref), handle)
    };
}

/// (live, dead kept) shadow copy bytes at the last check.
pub(crate) fn shadow_bytes() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (shadow::LIVE_BYTES.load(Relaxed), shadow::DEAD_BYTES.load(Relaxed))
}

/// Releases dead shadows and cached imports over [lo, hi) (shadow::release_over).
pub(crate) fn release_shadows_over(lo: u64, hi: u64) -> usize {
    shadow::release_over(lo, hi)
}

/// Releases dead shadow copies and unheld imports (see shadow::release_dead).
pub(crate) fn release_dead_shadows(device: &Device) -> usize {
    shadow::release_dead(&device.state.shadows)
}

/// TUXBOX_VIEW_LOCKFREE=0: shadow copies and single-page heap imports are
/// made with their cache lock held, as before (rebox b077562).
pub(crate) fn view_lockfree() -> bool {
    crate::env_toggle!("TUXBOX_VIEW_LOCKFREE") != Some("0")
}

/// A CBV over a range that cannot be imported falls back to a copy, else a
/// null CBV (reads zero), logged; never a fail-fast. TUXBOX_CBV_FALLBACK=0
/// fails fast as before.
fn cbv_fallback() -> bool {
    crate::env_toggle!("TUXBOX_CBV_FALLBACK") != Some("0")
}

fn cbv_fell_back(address: u64, bytes: u32, code: windows::core::HRESULT, copied: bool) {
    view_fell_back("CBV", address, u64::from(bytes), code, copied);
}

fn view_fell_back(what: &str, address: u64, bytes: u64, code: windows::core::HRESULT, copied: bool) {
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if logged_fallback(n) {
        crate::diagnostics::log_line(format_args!(
            "{what} {address:#x}+{bytes} not importable ({code:?}) from title RVA {:#x}: {} ({n} view fallbacks so far; TUXBOX_CBV_FALLBACK=0 fails fast)",
            crate::survey::title_rva().unwrap_or(0),
            if copied { "read from a copy" } else { "null view" }));
    }
}

/// First 8, then powers of two.
fn logged_fallback(n: u64) -> bool {
    n <= 8 || n.is_power_of_two()
}

#[cfg(test)]
#[test]
fn cbv_fallback_logs_first_eight_then_powers_of_two() {
    assert!(logged_fallback(1) && logged_fallback(8) && logged_fallback(16) && logged_fallback(1024));
    assert!(!logged_fallback(9) && !logged_fallback(17) && !logged_fallback(1000));
}

// Desktop textures have no buffer GPU VA. Reserve address space on demand
// for the Xbox descriptor API, which identifies the original texture by VA.
/// A placed buffer over separately mapped title pages that no heap can open:
/// a device-local copy refreshed before and written back after each submission.
pub(super) fn shadow_buffer(device: &Device, address: u64, bytes: u64)
    -> windows::core::Result<(ID3D12Resource, std::sync::Arc<TextureAddress>)> {
    let view = shadow::writable_view(device, address, bytes)?;
    Ok((view.native.clone(), view))
}

/// The title dereferences console image GPU addresses on the CPU (palmetto
/// e604c28a: unified memory). A committed texture's address is backed by
/// zeroed read/write memory covering both the host and the console size, so
/// those reads see memory, not a fault. The GPU does not write these pages:
/// GPU-written contents stay in the native texture.
/// Off by default (port audit): the zeroed backing hides a title CPU read of
/// an image address instead of finding why it reads one; a read faults as
/// before. TUXBOX_BACK_IMAGES=1 backs them.
fn back_images() -> bool {
    crate::env_toggle!("TUXBOX_BACK_IMAGES") == Some("1")
}

/// Whether `address` lies in the backing of a committed image's address
/// (TUXBOX_BACK_IMAGES): those pages are not title memory to import.
fn backed_image(device: &Device, address: u64, allocation: usize) -> bool {
    device.state.texture_addresses.preceding(address as usize)
        .is_some_and(|entry| entry.owns_address && entry.base == allocation)
}

fn backed_size(host: u64, console: u64) -> u64 {
    let size = if host == u64::MAX { console } else { host.max(console) };
    size.max(1).next_multiple_of(0x1_0000)
}

#[cfg(test)]
#[test]
fn a_backed_image_covers_the_larger_footprint_in_whole_64k_units() {
    assert_eq!(backed_size(0x2_0000, 0x1_8000), 0x2_0000);
    assert_eq!(backed_size(0x1_0000, 0x2_4000), 0x3_0000);
    assert_eq!(backed_size(u64::MAX, 0x1000), 0x1_0000);
    assert_eq!(backed_size(0, 0), 0x1_0000);
}

pub(crate) struct TextureAddress {
    pub(super) base: usize,
    pub(super) buffer_offset: u64,
    native: ID3D12Resource,
    registry: std::sync::Arc<super::placement::Views<Self>>,
    owns_address: bool,
    pub(super) depth: Option<Box<super::depth::Pair>>,
    _render_heap: Option<std::sync::Arc<super::heap::RenderHeap>>,
    _title_memory: Option<super::title_memory::GuestMemory>,
    buffer_heap: Option<std::sync::Arc<super::heap::ImportedHeap>>,
}

impl TextureAddress {
    pub(super) fn render_heap(&self) -> Option<&std::sync::Arc<super::heap::RenderHeap>> {
        self._render_heap.as_ref()
    }

    fn offset(&self, address: u64) -> u64 {
        self.buffer_offset + (address - self.base as u64)
    }

    /// A view backed by `native` at `base` that address lookups never find.
    fn unlisted(device: &Device, native: &ID3D12Resource, base: usize) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            base, buffer_offset: 0, native: native.clone(), registry: device.state.texture_addresses.clone(), owns_address: false,
            _title_memory: None, _render_heap: None, depth: None, buffer_heap: None,
        })
    }

    pub(super) fn register(device: &Device, native: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC) -> std::sync::Arc<Self> {
        use windows::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE};
        let host = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(desc)) }.SizeInBytes;
        let console = super::allocation::console_footprint(std::slice::from_ref(desc)).map_or(0, |i| i.SizeInBytes);
        let size = backed_size(host, console);
        let writeback = back_images() && super::committed_writeback::enabled();
        let base = if writeback {
            // No access until the CPU touches it: the fault reads the GPU's
            // contents back (committed_writeback).
            unsafe { VirtualAlloc(None, size as usize, MEM_RESERVE | MEM_COMMIT, PAGE_NOACCESS) }.addr()
        } else if back_images() {
            unsafe { VirtualAlloc(None, size as usize, MEM_RESERVE | MEM_COMMIT, PAGE_READWRITE) }.addr()
        } else {
            unsafe { VirtualAlloc(None, size as usize, MEM_RESERVE, PAGE_NOACCESS) }.addr()
        };
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!(
            "image GPU address {base:#x} ({size} bytes) for a committed {}x{} texture is {} (TUXBOX_BACK_IMAGES)",
            desc.Width, desc.Height, if writeback { "written back on CPU access" } else if back_images() { "backed by zeroed readable memory" } else { "reserved, no access" }));
        if writeback && base != 0
            && let Ok(host) = windows::core::Interface::cast::<ID3D12Device>(&device.native)
        {
            super::committed_writeback::register(&host, native, desc, base, size as usize);
        }
        Self::at(device, native, base, true, None)
    }

    pub(super) fn at(device: &Device, native: &ID3D12Resource, base: usize, owns_address: bool, memory: Option<super::title_memory::GuestMemory>) -> std::sync::Arc<Self> {
        Self::with_heap(device, native, base, owns_address, memory, None, None, 0, None)
    }

    pub(super) fn with_heap(device: &Device, native: &ID3D12Resource, base: usize, owns_address: bool,
        memory: Option<super::title_memory::GuestMemory>, heap: Option<std::sync::Arc<super::heap::RenderHeap>>, depth: Option<Box<super::depth::Pair>>, buffer_offset: u64,
        buffer_heap: Option<std::sync::Arc<super::heap::ImportedHeap>>) -> std::sync::Arc<Self> {
        let entry = std::sync::Arc::new(Self {
            base, buffer_offset, native: native.clone(), registry: device.state.texture_addresses.clone(), owns_address, _title_memory: memory, _render_heap: heap, depth, buffer_heap,
        });
        if base != 0 {
            entry.registry.set(base, Some(&entry));
        }
        entry
    }
}

impl Drop for TextureAddress {
    fn drop(&mut self) {
        use windows::Win32::System::Memory::{VirtualFree, MEM_RELEASE};
        if self.base != 0 {
            self.registry.remove_if(self.base, self);
            if self.owns_address {
                super::committed_writeback::unregister(self.base);
                let _ = unsafe { VirtualFree(self.base as _, 0, MEM_RELEASE) };
            }
        }
    }
}

// An address-only descriptor can be the sole owner of its backing resource.
#[derive(Clone)]
pub(crate) enum DescriptorBacking {
    ConstantBuffer { _address: Option<std::sync::Arc<TextureAddress>> },
    Registered { _address: std::sync::Arc<TextureAddress> },
    BufferAlias { native: ID3D12Resource, original: std::sync::Arc<TextureAddress> },
    Placed { _texture: std::sync::Arc<super::placement::Texture> },
}

/// The host CPU descriptor behind a title CPU handle. On Device11 every
/// CBV/SRV/UAV heap, CPU-only ones included, is an arena in title memory,
/// so a raw title handle is not a host descriptor (leonida #416).
pub(crate) unsafe fn host_cpu_descriptor(device: *mut c_void, cpu: D3D12_CPU_DESCRIPTOR_HANDLE) -> D3D12_CPU_DESCRIPTOR_HANDLE {
    let device = unsafe { &*device.cast::<Device>() };
    device.state.descriptor_arenas.copy_source(cpu)
}

pub(crate) unsafe fn clear_buffer_alias(
    device: *mut c_void, cpu: usize, resource: &super::Resource,
) -> windows::core::Result<Option<ID3D12Resource>> {
    let device = unsafe { &*device.cast::<Device>() };
    let Some(DescriptorBacking::BufferAlias { native, original }) = device.state.imported_views.lookup(cpu)
        else { return Ok(None) };
    if !matches!(&resource.state.backing, super::Backing::Addressed(a, _) if std::sync::Arc::ptr_eq(a, &original)) {
        return Err(windows::Win32::Foundation::E_INVALIDARG.into());
    }
    Ok(Some(native))
}

/// The guest device owns the descriptor metadata for ordinary heaps and for
/// the staging handles of virtualized heaps. Descriptor copies preserve it.
/// Refresh shadowed buffer views before `queue` runs guest work.
pub(crate) unsafe fn refresh_shadows(owner: &windows::core::IUnknown, queue: &ID3D12CommandQueue) -> windows::core::Result<()> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    unsafe { shadow::refresh(device, queue) }
}

/// Upload the literal sampler lookup table ahead of a submission's lists.
pub(crate) unsafe fn flush_desc_table(owner: &windows::core::IUnknown, queue: &ID3D12CommandQueue) -> windows::core::Result<()> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    unsafe { device.state.desc_table.flush(&device.native, queue) }
}

/// The literal sampler lookup table's GPU address for a draw being recorded.
pub(crate) unsafe fn desc_table_address(owner: &windows::core::IUnknown) -> windows::core::Result<u64> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    device.state.desc_table.address(&device.native)
}

/// The typed view format table of the shader-visible heap at `start` and its
/// GPU address, for a draw being recorded (view_formats).
pub(crate) unsafe fn view_formats_address(
    owner: &windows::core::IUnknown, start: usize, count: u32, stride: usize,
) -> windows::core::Result<(ID3D12Resource, u64)> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    super::view_formats::address(&device.native, start, count, stride)
}

/// A command or view may write `resource` (a raw native resource).
pub(crate) unsafe fn note_shadow_write(owner: *mut core::ffi::c_void, resource: *mut core::ffi::c_void) {
    let device = unsafe { &*owner.cast::<Device>() };
    shadow::mark_written(device, resource);
}

pub(crate) unsafe fn write_back_shadows(owner: &windows::core::IUnknown, queue: &ID3D12CommandQueue) -> windows::core::Result<()> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    unsafe { shadow::write_back(device, queue) }
}

pub(crate) unsafe fn is_constant_buffer(owner: &windows::core::IUnknown, handle: usize) -> bool {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    matches!(device.state.imported_views.lookup(handle), Some(DescriptorBacking::ConstantBuffer { .. }))
}

#[repr(C)]
pub(super) struct AddressSrv {
    view: D3D12_SHADER_RESOURCE_VIEW_DESC,
    address: u64,
}
const _: () = assert!(std::mem::offset_of!(AddressSrv, address) == 40);

fn uav_imports() -> bool {
    crate::env_toggle!("TUXBOX_UAV_IMPORTS") != Some("0")
}

fn buffer_at(device: &Device, address: u64, bytes: u64, stride: u32, required: bool)
    -> windows::core::Result<Option<std::sync::Arc<TextureAddress>>> {
    use windows::Win32::Foundation::E_INVALIDARG;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    crate::commands::page_mapping::GPU_APERTURE.wait(address,bytes)?;
    super::title_memory::follow_remaps();
    if let Some(entry) = device.state.texture_addresses.preceding(address as usize)
        // An import whose pages the title remapped since maps the old ones
        // (heap.rs retire_imports_over): a new import replaces it below.
        && !entry.buffer_heap.as_ref().is_some_and(|heap| heap.stale())
    {
        let host = unsafe { entry.native.GetDesc() };
        if host.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
            && entry.offset(address).is_multiple_of(u64::from(stride))
            && entry.offset(address).checked_add(bytes).is_some_and(|end| end <= host.Width) {
            return Ok(Some(entry));
        }
    }
    // FirstElement is measured in whole records. If the guest's suballocation
    // starts between records relative to a 64 KiB page, place the buffer at
    // the guest byte offset using the adapter's tight buffer alignment.
    let nearest = address & !0xffff;
    let tight = !(address - nearest).is_multiple_of(u64::from(stride));
    // A buffer only the GPU writes is still reserved in a lazy title view.
    super::commit_lazy(address as usize, bytes as usize);
    // A lazy view xmem knows committed answers without the VirtualQuery,
    // which scans the whole region (test137: tens of ms on the page pool).
    let view = super::view_state(address as usize, bytes as usize);
    let mut region = MEMORY_BASIC_INFORMATION::default();
    let allocation = if view.committed() && view.one_view() {
        Some(view.base)
    } else if unsafe { VirtualQuery(Some(address as _), &mut region, size_of_val(&region)) } != 0 && region.State == MEM_COMMIT {
        Some(region.AllocationBase.addr())
    } else {
        None
    };
    if allocation.is_none_or(|base| backed_image(device, address, base)) {
        return if required { Err(E_INVALIDARG.into()) } else { Ok(None) };
    }
    let mut host = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1,
        // D3D12_RESOURCE_FLAG_USE_TIGHT_ALIGNMENT (Agility SDK 617).
        // test120: every address UAV landed on an imported buffer flagged
        // only ALLOW_CROSS_ADAPTER and bound a null view, so the world's
        // compute writes went nowhere. Imported buffers now also allow
        // unordered access (legal for a cross-adapter buffer);
        // TUXBOX_UAV_IMPORTS=0 imports them without it, as before.
        Flags: (if tight { D3D12_RESOURCE_FLAGS(0x400) } else { D3D12_RESOURCE_FLAG_NONE })
            | if uav_imports() { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE },
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let page = if tight {
        let probe = D3D12_RESOURCE_DESC { Flags: host.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER, ..host };
        let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(&probe)) };
        if info.Alignment == 0 || info.SizeInBytes == u64::MAX { return Err(E_INVALIDARG.into()); }
        let origin = region.AllocationBase.addr() as u64;
        // Search no further than a single period of the alignment congruence.
        let base = (0..info.Alignment.min((address - origin) / u64::from(stride) + 1))
            .map(|n| address - n * u64::from(stride))
            .find(|&base| (base - origin).is_multiple_of(info.Alignment));
        // No host placement boundary is a whole number of records away.
        let Some(base) = base else { return shadow::view(device, address, bytes).map(Some) };
        base
    } else { nearest };
    host.Width = bytes.checked_add(address - page).ok_or(E_INVALIDARG)?;
    if !tight { host.Width = host.Width.checked_next_multiple_of(65536).ok_or(E_INVALIDARG)?; }
    // A range spanning several separately mapped guest views cannot be opened as
    // one heap; read it through a shadow copy instead.
    match super::heap::import_buffer_with_heap(device, page, &host, D3D12_RESOURCE_STATE_COMMON) {
        Ok((native, heap)) => Ok(Some(TextureAddress::with_heap(device, &native, page as usize, false, None, None, None, 0, Some(heap)))),
        Err(e) if e.code() == E_INVALIDARG => shadow::view(device, address, bytes).map(Some),
        Err(e) => Err(e),
    }
}

/// Buffers that command lists bound recently. texture_addresses holds only
/// weak entries, so a bind that kept just the native resource let its
/// TextureAddress (and any shadow copy) die at once: every vertex, index and
/// value-write bind re-imported the pages, about 5-8 ms each (test9: 140 of
/// 156 ms/frame), and a shadow made for a bind was gone before its
/// submission refreshed it. Holding the most recent ones keeps both alive.
#[derive(Default)]
pub(crate) struct RecentBuffers(std::sync::Mutex<std::collections::VecDeque<std::sync::Arc<TextureAddress>>>);

impl RecentBuffers {
    const CAPACITY: usize = 1024;

    fn keep(&self, buffer: &std::sync::Arc<TextureAddress>) {
        let mut recent = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // Hits mostly repeat a buffer bound moments ago.
        if recent.iter().rev().take(64).any(|held| std::sync::Arc::ptr_eq(held, buffer)) {
            return;
        }
        if recent.len() >= Self::CAPACITY {
            recent.pop_front();
        }
        recent.push_back(buffer.clone());
    }
}

pub(crate) unsafe fn command_buffer_address(device: *mut c_void, address: u64, bytes: u64) -> windows::core::Result<(Option<ID3D12Resource>, u64)> {
    let device = unsafe { &*device.cast::<Device>() };
    let Some(buffer) = buffer_at(device, address, bytes, 1, false)? else { return Ok((None, address)) };
    device.state.recent_buffers.keep(&buffer);
    let host = unsafe { buffer.native.GetGPUVirtualAddress() } + buffer.offset(address);
    Ok((Some(buffer.native.clone()), host))
}

fn buffer_stride(view: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> Option<u32> {
    let buffer = unsafe { view.Anonymous.Buffer };
    element_stride(view.Format.0, buffer.StructureByteStride)
}

fn element_stride(format: i32, structure_stride: u32) -> Option<u32> {
    if structure_stride != 0 {
        return (structure_stride <= 2048).then_some(structure_stride);
    }
    Some(match format {
        1..=4 => 16, 5..=8 => 12, 9..=22 => 8, 23..=47 | 67 | 87..=93 => 4,
        48..=59 | 85 | 86 | 115 => 2, 60..=65 => 1, _ => return None,
    })
}

fn rebase_view(window: (u64, u64), format: i32, structure_stride: u32, first: u64, elements: u32) -> u64 {
    let (prefix, width) = window;
    let stride = element_stride(format, structure_stride).map(u64::from)
        .unwrap_or_else(|| fail_call(format_args!("unsupported placed buffer view stride for format {format}")));
    if !prefix.is_multiple_of(stride) || first.checked_add(u64::from(elements)).is_none_or(|end| end > width / stride) {
        fail_call(format_args!("placed buffer view outside element range: prefix={prefix}, width={width}, stride={stride}, first={first}, count={elements}"));
    }
    first + prefix / stride
}

/// TUXBOX_ADDRESS_VIEW_NULL=0: an address buffer SRV outside its bounds
/// fails the call as before. On: it is logged and bound as a null view.
fn null_bad_address_views() -> bool {
    crate::env_toggle!("TUXBOX_ADDRESS_VIEW_NULL") != Some("0")
}

/// Why a buffer SRV made from a title GPU address is out of bounds (the raw
/// view checks of Xbox2Windows ee730d22, MIT, re-stated): no elements, an
/// unknown stride, elements past the resource width, or a raw view whose
/// address is not dword aligned. A raw start off 16 bytes is not refused:
/// the byte-offset path serves it.
fn address_view_fault(address: u64, width: u64, format: i32, structure_stride: u32, first: u64, count: u32, raw: bool) -> Option<&'static str> {
    if count == 0 {
        return Some("no elements");
    }
    let raw_stride = raw.then_some(4);
    let Some(stride) = raw_stride.or_else(|| element_stride(format, structure_stride)).map(u64::from) else {
        return Some("unsupported stride");
    };
    if raw && !address.is_multiple_of(4) {
        return Some("raw view address not dword aligned");
    }
    if first.checked_add(u64::from(count)).is_none_or(|end| end > width / stride) {
        return Some("elements past the resource");
    }
    None
}

#[cfg(test)]
mod address_view_tests {
    use super::address_view_fault;

    #[test]
    fn address_buffer_views_stay_in_bounds() {
        const R32_TYPELESS: i32 = 39;
        assert_eq!(address_view_fault(0x1000, 64, R32_TYPELESS, 0, 0, 16, true), None);
        assert_eq!(address_view_fault(0x1000, 64, R32_TYPELESS, 0, 0, 0, true), Some("no elements"));
        assert_eq!(address_view_fault(0x1000, 64, R32_TYPELESS, 0, 1, 16, true), Some("elements past the resource"));
        assert_eq!(address_view_fault(0x1002, 64, R32_TYPELESS, 0, 0, 4, true), Some("raw view address not dword aligned"));
        assert_eq!(address_view_fault(0x1004, 64, R32_TYPELESS, 0, 0, 4, true), None);
        assert_eq!(address_view_fault(0x1000, 64, 0, 0, 0, 1, false), Some("unsupported stride"));
        assert_eq!(address_view_fault(0x1000, 64, 0, 16, 3, 1, false), None);
        assert_eq!(address_view_fault(0x1000, 64, 0, 16, u64::MAX, 1, false), Some("elements past the resource"));
    }
}

enum PlacedView<'a> {
    Elements(u64, Option<ID3D12Resource>),
    /// A read-only view served from a device-local copy of the guest bytes.
    Copied(u64, std::sync::Arc<TextureAddress>),
    Displaced(&'a AddressViews, u64, u32),
}

fn placed_view<'a>(
    device: &'a Device, address: &TextureAddress, width: u64, format: i32, stride: u32,
    first: u64, count: u32, view_flags: i32, flags: D3D12_RESOURCE_FLAGS,
) -> PlacedView<'a> {
    // Check the guest view against its own allocation before importing an alias.
    let first = rebase_view((0, width), format, stride, first, count);
    let element = element_stride(format, stride).expect("validated element stride");
    if address.buffer_offset.is_multiple_of(element.into()) {
        return PlacedView::Elements(first + address.buffer_offset / u64::from(element), None);
    }
    let mode = AddressViews::get(device);
    if mode.supports(format, view_flags, stride, count) {
        return PlacedView::Displaced(mode, address.buffer_offset, element);
    }
    // FirstElement advances by whole records. An alternate buffer placed on
    // the same XMem pages can supply the missing byte displacement.
    let base = address.base as u64;
    let heap = address.buffer_heap.as_ref().expect("placed buffer retains its imported heap");
    let heap_base = (base - address.buffer_offset) & !65535;
    match heap.buffer_alias(&device.native, base - heap_base, width, element, flags) {
        Ok((alias, extra)) => PlacedView::Elements(first + extra, Some(alias)),
        // Cross-adapter placement pins aliases to 64 KiB; the view uses a
        // copy of the same bytes that starts at the guest VA instead.
        Err(_) => match if flags == D3D12_RESOURCE_FLAG_NONE { shadow::view(device, base, width) } else { shadow::writable_view(device, base, width) } {
            Ok(copy) => PlacedView::Copied(first, copy),
            Err(e) => fail_call(format_args!("buffer view copy at {base:#x} width={width} stride={element}: {:?}", e.code())),
        },
    }
}

pub(super) unsafe extern "system" fn address_srv(
    device: &Device,
    resource_desc: &D3D12_RESOURCE_DESC,
    desc: &AddressSrv,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    check_destination(handle);
    crate::commands::page_mapping::GPU_APERTURE.wait(desc.address,1)
        .unwrap_or_else(|e| fail_call(format_args!("address SRV pending GPU mapping: {:?}", e.code())));
    let mut view = desc.view;
    let mut byte_view = None;
    let buffer_view = resource_desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
        && view.ViewDimension == D3D12_SRV_DIMENSION_BUFFER;
    if desc.address != 0 && buffer_view && null_bad_address_views() {
        let buffer = unsafe { view.Anonymous.Buffer };
        if let Some(fault) = address_view_fault(desc.address, resource_desc.Width, view.Format.0, buffer.StructureByteStride,
            buffer.FirstElement, buffer.NumElements, buffer.Flags == D3D12_BUFFER_SRV_FLAG_RAW)
        {
            static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "address SRV at {:#x} ({} bytes, format {}, first {}, count {}) refused: {fault}; bound a null view ({n} so far; TUXBOX_ADDRESS_VIEW_NULL=0 fails the call)",
                    desc.address, resource_desc.Width, view.Format.0, buffer.FirstElement, buffer.NumElements));
            }
            device.state.imported_views.store(handle.ptr, None);
            device.state.blas.views.store(handle.ptr, None);
            device.state.placements.views.set(handle.ptr, None, false);
            if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, None); }
            unsafe { device.native.CreateShaderResourceView(None, Some(&view), handle) };
            return;
        }
    }
    let imported = if desc.address != 0 && buffer_view {
        let stride = buffer_stride(&view).unwrap_or_else(|| fail_call(format_args!("unsupported address SRV format {}", view.Format.0)));
        let buffer = unsafe { view.Anonymous.Buffer };
        rebase_view((0, resource_desc.Width), view.Format.0, buffer.StructureByteStride, buffer.FirstElement, buffer.NumElements);
        // Byte-offset descriptors can share the original pages even when no
        // host placement boundary is a whole number of records from the VA.
        let mode = AddressViews::get(device);
        let placement_stride = if mode.supports(view.Format.0, buffer.Flags.0, buffer.StructureByteStride, buffer.NumElements) { 1 } else { stride };
        let region = || {
            use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
            let mut r = MEMORY_BASIC_INFORMATION::default();
            unsafe { VirtualQuery(Some(desc.address as _), &mut r, size_of_val(&r)) };
            (r.AllocationBase.addr(), r.BaseAddress.addr(), r.RegionSize, r.State.0, r.Protect.0)
        };
        let entry = match buffer_at(device, desc.address, resource_desc.Width, placement_stride, true) {
            Ok(entry) => entry.expect("required buffer import"),
            Err(e) if !cbv_fallback() => fail_call(format_args!("import SRV at {:#x}, {} bytes stride={stride} fmt={} first={} count={} failed {:?} ({}; region {:#x?}, placement stride {})",
                desc.address, resource_desc.Width, view.Format.0, buffer.FirstElement, buffer.NumElements, e.code(), mode.name(), region(), placement_stride)),
            // Title pages no heap opens (views that would not join): read the
            // buffer from a copy that starts at the guest VA, else bind a
            // null view; logged, never a fail-fast (TUXBOX_CBV_FALLBACK=0).
            Err(e) => match shadow::view(device, desc.address, resource_desc.Width.max(1)) {
                Ok(copy) => {
                    view_fell_back("address SRV", desc.address, resource_desc.Width, e.code(), true);
                    copy
                }
                Err(_) => {
                    view_fell_back("address SRV", desc.address, resource_desc.Width, e.code(), false);
                    device.state.imported_views.store(handle.ptr, None);
                    device.state.blas.views.store(handle.ptr, None);
                    device.state.placements.views.set(handle.ptr, None, false);
                    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, None); }
                    super::view_formats::write(handle.ptr, 0);
                    unsafe { device.native.CreateShaderResourceView(None, Some(&view), handle) };
                    return;
                }
            },
        };
        let offset = entry.offset(desc.address);
        if offset.is_multiple_of(u64::from(stride)) {
            view.Anonymous.Buffer.FirstElement = buffer.FirstElement + offset / u64::from(stride);
        } else {
            byte_view = Some((mode, offset, stride));
        }
        Some(entry)
    } else { None };
    let existing = if buffer_view { None } else {
        device.state.texture_addresses.preceding(desc.address as usize).filter(|entry| {
            unsafe { entry.native.GetDesc() }.Dimension == resource_desc.Dimension && entry.offset(desc.address) == 0
        })
    };
    let placed = if desc.address != 0 && existing.is_none() && imported.is_none() {
        let made = device.state.placements.texture(&device.native, desc.address, resource_desc, D3D12_RESOURCE_STATE_COMMON, None);
        // Out of memory or refused: bind a null view (reads zero), logged,
        // instead of failing fast (test38: 1024x1024 format 71, 0x8007000E).
        if let Err(e) = &made && cbv_fallback() {
            view_fell_back("address-only texture SRV", desc.address, resource_desc.Width, e.code(), false);
            device.state.imported_views.store(handle.ptr, None);
            device.state.blas.views.store(handle.ptr, None);
            device.state.placements.views.set(handle.ptr, None, false);
            if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, None); }
            super::view_formats::write(handle.ptr, 0);
            unsafe { device.native.CreateShaderResourceView(None, Some(&view), handle) };
            return;
        }
        Some(made
            .unwrap_or_else(|e| fail_call(format_args!("address-only texture SRV at {:#x}, {}x{} format={} layout={:#x} dimension={} slices={} mips={} samples={}/{} alignment={:#x} flags={:#x} view={}: {:?}", desc.address, resource_desc.Width, resource_desc.Height, resource_desc.Format.0, resource_desc.Layout.0, resource_desc.Dimension.0, resource_desc.DepthOrArraySize, resource_desc.MipLevels, resource_desc.SampleDesc.Count, resource_desc.SampleDesc.Quality, resource_desc.Alignment, resource_desc.Flags.0, view.ViewDimension.0, e.code()))))
    } else {
        None
    };
    let owner = placed.clone().map(|texture| DescriptorBacking::Placed { _texture: texture })
        .or_else(|| imported.as_ref().or(existing.as_ref()).cloned()
            .map(|address| DescriptorBacking::Registered { _address: address }));
    device.state.imported_views.store(handle.ptr, owner);
    device.state.blas.views.store(handle.ptr, None);
    device.state.placements.views.set(handle.ptr, placed.as_ref(), false);
    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, existing.as_ref().and_then(|e| e.render_heap())); }
    let native = existing.as_ref().map(|e| &e.native).or_else(|| placed.as_ref().map(|p| &p.native)).or_else(|| imported.as_ref().map(|e| &e.native));
    crate::diagnostics::display::texture_view(handle.ptr, native, Some(view.Format.0));
    super::view_formats::write(handle.ptr, super::view_formats::srv_code(Some(&view)));
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_srv(device, native.expect("buffer import retained"), view, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("byte-offset SRV at {:#x} offset={offset} stride={stride}: {:?}", desc.address, e.code())));
    } else {
        let mut view = view;
        super::placement::pages::clamp_view(native, &mut view);
        unsafe { device.native.CreateShaderResourceView(native, Some(&view), handle) };
        report_bad_view("address SRV", &device.native, native, Some(&view));
    }
}

/// Once, when a view creation has just removed the device ("bad view create"),
/// log the view and resource descriptions that caused it.
fn report_bad_view(kind: &str, device: &ID3D12Device, resource: Option<&ID3D12Resource>, view: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REPORTED: AtomicBool = AtomicBool::new(false);
    let reason = unsafe { device.GetDeviceRemovedReason() };
    if REPORTED.load(Ordering::Relaxed) || reason.is_ok() || REPORTED.swap(true, Ordering::Relaxed) {
        return;
    }
    // The removal is usually asynchronous (a GPU fault or hang) and this view
    // merely the first call to notice it.
    let reason = reason.err().map(|e| e.code().0 as u32).unwrap_or(0);
    let res = resource.map(|r| unsafe { r.GetDesc() });
    let view = view.map(|v| {
        let detail = match v.ViewDimension {
            D3D12_SRV_DIMENSION_BUFFER => format!("{:?}", unsafe { v.Anonymous.Buffer }),
            D3D12_SRV_DIMENSION_TEXTURE2D => format!("{:?}", unsafe { v.Anonymous.Texture2D }),
            D3D12_SRV_DIMENSION_TEXTURE2DARRAY => format!("{:?}", unsafe { v.Anonymous.Texture2DArray }),
            D3D12_SRV_DIMENSION_TEXTURECUBE => format!("{:?}", unsafe { v.Anonymous.TextureCube }),
            D3D12_SRV_DIMENSION_TEXTURE3D => format!("{:?}", unsafe { v.Anonymous.Texture3D }),
            _ => String::new(),
        };
        format!("format={} dimension={} mapping={:#x} {detail}", v.Format.0, v.ViewDimension.0, v.Shader4ComponentMapping)
    });
    crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
        "{kind} creation found the device removed (reason {reason:#x}; TUXBOX_DRED=1 for breadcrumbs and the faulting address): view {view:?}; resource {:?}",
        res.map(|r| format!("dimension={} {}x{}x{} mips={} format={} flags={:#x} layout={}", r.Dimension.0, r.Width, r.Height, r.DepthOrArraySize, r.MipLevels, r.Format.0, r.Flags.0, r.Layout.0))));
}

/// xcl's idea (re-derived from 615cb20): an SRV of a depth buffer made with
/// no description. The host cannot view a depth or typeless format, so the
/// view reads the depth plane (R32_FLOAT, R16_UNORM, R24_UNORM_X8 or
/// R32_FLOAT_X8X24) over every mip and slice, multisampled when the buffer
/// is (Texture2DMS / Texture2DMSArray). Before, the host call was invalid,
/// and a substituted Scarlett depth buffer stopped the title.
/// TUXBOX_DEPTH_DEFAULT_SRV=0 passes the null description on as before.
fn default_depth_srv(resource: &ID3D12Resource) -> Option<D3D12_SHADER_RESOURCE_VIEW_DESC> {
    use windows::Win32::Graphics::Dxgi::Common::*;
    if crate::env_toggle!("TUXBOX_DEPTH_DEFAULT_SRV") == Some("0") {
        return None;
    }
    let r = unsafe { resource.GetDesc() };
    if r.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || !r.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL) {
        return None;
    }
    let format = match r.Format {
        DXGI_FORMAT_D32_FLOAT | DXGI_FORMAT_R32_TYPELESS => DXGI_FORMAT_R32_FLOAT,
        DXGI_FORMAT_D16_UNORM | DXGI_FORMAT_R16_TYPELESS => DXGI_FORMAT_R16_UNORM,
        DXGI_FORMAT_D24_UNORM_S8_UINT | DXGI_FORMAT_R24G8_TYPELESS => DXGI_FORMAT_R24_UNORM_X8_TYPELESS,
        DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_R32G8X24_TYPELESS => DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
        _ => return None,
    };
    let slices = u32::from(r.DepthOrArraySize);
    let mut view = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        ..Default::default()
    };
    match (r.SampleDesc.Count > 1, slices > 1) {
        (true, false) => view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2DMS,
        (true, true) => {
            view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2DMSARRAY;
            view.Anonymous.Texture2DMSArray = D3D12_TEX2DMS_ARRAY_SRV { FirstArraySlice: 0, ArraySize: slices };
        }
        (false, false) => {
            view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
            view.Anonymous.Texture2D = D3D12_TEX2D_SRV { MipLevels: u32::MAX, ..Default::default() };
        }
        (false, true) => {
            view.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2DARRAY;
            view.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_SRV { MipLevels: u32::MAX, ArraySize: slices, ..Default::default() };
        }
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "depth SRV with no description: format {} viewed as {} dimension {} ({n} so far; TUXBOX_DEPTH_DEFAULT_SRV=0 off)",
            r.Format.0, format.0, view.ViewDimension.0));
    }
    Some(view)
}

pub(super) unsafe extern "system" fn srv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_SHADER_RESOURCE_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    let resource = unsafe { view_resource(device, object, desc, handle) };
    let proxy = unsafe { super::resource_proxy(object, identity(device).as_raw()) };
    let mut alias = None;
    let mut byte_view = None;
    let mut adjusted;
    let desc = if let Some(super::Backing::Addressed(address, original)) = proxy.map(|p| &p.state.backing)
        && address.buffer_offset != 0
    {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer SRV requires a buffer description")); };
        if view.ViewDimension != D3D12_SRV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer SRV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        match placed_view(device, address, original.Width, adjusted.Format.0,
            b.StructureByteStride, b.FirstElement, b.NumElements, b.Flags.0, D3D12_RESOURCE_FLAG_NONE) {
            PlacedView::Elements(first, native) => {
                b.FirstElement = first;
                alias = native.map(|native| (native, address.clone()));
            }
            PlacedView::Copied(first, copy) => {
                b.FirstElement = first;
                alias = Some((copy.native.clone(), copy));
            }
            PlacedView::Displaced(mode, offset, stride) => byte_view = Some((mode, offset, stride)),
        }
        &adjusted
    } else { desc };
    let translated;
    let substitute = proxy.and_then(|p| p.state.translated());
    let original_view = unsafe { desc.as_ref() }.copied();
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::format::view_format(substitute.is_some(), view.Format) != view.Format => {
            translated = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: super::format::view_format(substitute.is_some(), view.Format), ..*view
            };
            &raw const translated
        }
        None if let Some(view) = resource.and_then(default_depth_srv) => {
            translated = view;
            &raw const translated
        }
        None if substitute.is_some_and(super::format::is_depth) => {
            fail_call(format_args!("Scarlett depth SRV needs an explicit plane format"))
        }
        _ => desc,
    };
    crate::diagnostics::a8::srv(original_view.as_ref(), resource, unsafe { desc.as_ref() }.map_or(0, |view| view.Format.0 as u32));
    crate::diagnostics::display::texture_view(handle.ptr, resource, unsafe { desc.as_ref() }.map(|view| view.Format.0));
    super::view_formats::write(handle.ptr, super::view_formats::srv_code(unsafe { desc.as_ref() }));
    let texture = proxy.and_then(|resource| resource.state.placed());
    let scene = device.state.blas.scene_view(proxy, unsafe { desc.as_ref() });
    device.state.blas.views.store(handle.ptr, scene);
    device.state.placements.views.set(handle.ptr, texture, false);
    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, proxy.and_then(|p| p.state.render_heap())); }
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_srv(device, resource.expect("placed buffer"), unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed SRV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        let target = alias.as_ref().map(|(native, _)| native).or(resource);
        // A texture whose mip 0 was left unmapped is viewed from mip 1.
        let clamped;
        let desc = match unsafe { desc.as_ref() } {
            Some(view) => {
                let mut v = *view;
                super::placement::pages::clamp_view(target, &mut v);
                clamped = v;
                &raw const clamped
            }
            None => desc,
        };
        unsafe { device.native.CreateShaderResourceView(target, Some(desc), handle) };
        report_bad_view("SRV", &device.native, target, unsafe { desc.as_ref() });
    }
    device.state.imported_views.store(handle.ptr, alias.map(|(native, original)| DescriptorBacking::BufferAlias { native, original })
        .or_else(|| slot_holds().then(|| texture.cloned()).flatten().map(|t| DescriptorBacking::Placed { _texture: t })));
}

/// A descriptor slot holding a view of a placed texture keeps the texture
/// (and so its native copy, span or pages) alive until the slot is rewritten
/// or its heap freed: retired textures were released while the title's heaps
/// still held views of them and the GPU faulted (rebox 7745cef, GPL-2.0,
/// credit to its authors). The address-only SRV already did this.
/// TUXBOX_SLOT_HOLDS_TEXTURE=0 keeps only weak references, as before.
fn slot_holds() -> bool {
    crate::env_toggle!("TUXBOX_SLOT_HOLDS_TEXTURE") != Some("0")
}

pub(super) unsafe extern "system" fn uav(
    device: &Device,
    object: *mut c_void,
    counter: *mut c_void,
    desc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    super::view_formats::write(handle.ptr, super::view_formats::uav_code(unsafe { desc.as_ref() }));
    let owner = identity(device).as_raw();
    let proxy = unsafe { super::resource_proxy(object, owner) };
    let placed = proxy.and_then(|p| p.state.placed());
    let window = proxy.and_then(|p| p.state.buffer_window());
    let counter_window = unsafe { super::resource_proxy(counter, owner) }.and_then(|p| p.state.buffer_window());
    // The guest's own view, before any placement rebasing.
    if crate::diagnostics::display::trace_views()
        && let Some(view) = unsafe { desc.as_ref() }.filter(|v| v.ViewDimension == D3D12_UAV_DIMENSION_BUFFER)
    {
        let b = unsafe { view.Anonymous.Buffer };
        let stride = if b.Flags == D3D12_BUFFER_UAV_FLAG_RAW { Some(4) } else { element_stride(view.Format.0, b.StructureByteStride) };
        let title = proxy.and_then(|p| match &p.state.backing {
            super::Backing::Addressed(address, _) => Some(address.base as u64),
            _ => None,
        });
        let host = || unsafe { super::resource(object, owner) }.map(|r| unsafe { r.GetGPUVirtualAddress() });
        if let (Some(stride), Some(base)) = (stride, title.or_else(host)) {
            let stride = u64::from(stride);
            crate::diagnostics::display::buffer_view(handle.ptr, crate::diagnostics::display::BufferView {
                kind: "uav", address: base + b.FirstElement * stride, bytes: u64::from(b.NumElements) * stride, title: title.is_some(),
            });
        }
    }
    let mut adjusted;
    let mut uav_alias = None;
    let mut byte_view = None;
    let desc = if window.is_some() || counter_window.is_some() {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer UAV requires a buffer description")); };
        if view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer UAV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        if let Some(super::Backing::Addressed(address, original)) = proxy.map(|p| &p.state.backing)
            && window.is_some()
        {
            if !original.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
                fail_call(format_args!("placed buffer was not created for unordered access"));
            }
            match placed_view(device, address, original.Width, adjusted.Format.0,
                b.StructureByteStride, b.FirstElement, b.NumElements, b.Flags.0, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
                PlacedView::Elements(first, alias) => {
                    b.FirstElement = first;
                    uav_alias = alias.map(|alias| (alias, address.clone()));
                }
                PlacedView::Copied(first, copy) => {
                    b.FirstElement = first;
                    uav_alias = Some((copy.native.clone(), copy));
                }
                PlacedView::Displaced(mode, offset, stride) => byte_view = Some((mode, offset, stride)),
            }
        }
        if let Some((prefix, width)) = counter_window {
            if b.CounterOffsetInBytes.checked_add(4).is_none_or(|end| end > width) {
                fail_call(format_args!("placed UAV counter outside resource"));
            }
            b.CounterOffsetInBytes += prefix;
            if !b.CounterOffsetInBytes.is_multiple_of(D3D12_UAV_COUNTER_PLACEMENT_ALIGNMENT.into()) {
                fail_call(format_args!("unaligned placed UAV counter offset {}",b.CounterOffsetInBytes));
            }
        }
        &adjusted
    } else { desc };
    device.state.placements.views.set(handle.ptr, placed, true);
    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, proxy.and_then(|p| p.state.render_heap())); }
    device.state.blas.views.store(handle.ptr, None);
    device.state.imported_views.store(handle.ptr, uav_alias.as_ref().map(|(alias, original)|
        DescriptorBacking::BufferAlias { native: alias.clone(), original: original.clone() })
        .or_else(|| slot_holds().then(|| placed.cloned()).flatten().map(|t| DescriptorBacking::Placed { _texture: t })));
    let buffer = unsafe { view_resource(device, object, desc, handle) };
    let buffer = uav_alias.as_ref().map(|(alias, _)| alias).or(buffer);
    if let Some(resource) = buffer { shadow::mark_written(device, resource.as_raw()); }
    let counter = if counter.is_null() {
        None
    } else {
        if buffer.is_none() {
            fail_call(format_args!("null UAV resource with a counter"));
        }
        Some(
            unsafe { resource(counter, identity(device).as_raw()) }
                .unwrap_or_else(|| fail_call(format_args!("invalid UAV counter resource"))),
        )
    };
    let substitute = unsafe { super::resource_proxy(object, identity(device).as_raw()) }.and_then(|r| r.state.translated());
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::format::view_format(substitute.is_some(), view.Format) != view.Format => {
            translated = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: super::format::view_format(substitute.is_some(), view.Format), ..*view
            };
            &raw const translated
        }
        // Desktop D3D12 has no typed B5G5R5A1 UAV; the image is castable to
        // R16_UINT (allocation::packed_uav_formats), so view its bits as that.
        Some(view) if view.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B5G5R5A1_UNORM => {
            translated = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R16_UINT, ..*view
            };
            &raw const translated
        }
        _ => desc,
    };
    if let Some(guest) = substitute.filter(|g| super::format::is_depth(g)) {
        // The host depth image has no writable view yet. Keep the descriptor
        // defined (null: reads return zero, writes are dropped) and report it.
        let Some(view) = (unsafe { desc.as_ref() }) else {
            fail_call(format_args!("UAV on Scarlett depth format {} needs a description", guest.Format.0));
        };
        static DEPTH_UAV: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&DEPTH_UAV, || format!(
            "null UAV for Scarlett depth format={} {}x{}x{} flags={:#x}: view format={} dimension={}",
            guest.Format.0, guest.Width, guest.Height, guest.DepthOrArraySize, guest.Flags.0,
            view.Format.0, view.ViewDimension.0
        ));
        // A null view has no planes: a stencil-plane slice removes the device.
        let mut null = *view;
        match null.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE2D => null.Anonymous.Texture2D.PlaneSlice = 0,
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => null.Anonymous.Texture2DArray.PlaneSlice = 0,
            _ => {}
        }
        unsafe { checked_uav(device, None, None, &null, handle) };
        return;
    }
    if let Some(pair) = unsafe { super::resource_proxy(object, identity(device).as_raw()) }
        .and_then(|r| r.state.depth())
    {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
        };
        let Some(view) = (unsafe { desc.as_ref() }) else {
            fail_call(format_args!("depth UAV needs a typed plane view"));
        };
        if counter.is_some()
            || view.ViewDimension != D3D12_UAV_DIMENSION_TEXTURE2D
            // Either the plane format or the float format of the depth plane.
            || !matches!(view.Format, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS | DXGI_FORMAT_R32_FLOAT)
            || unsafe {
                view.Anonymous.Texture2D.MipSlice != 0 || view.Anonymous.Texture2D.PlaneSlice != 0
            }
        {
            fail_call(format_args!(
                "unsupported depth UAV plane view: format={} dimension={} mip/plane={:?} counter={} on {}x{}x{}",
                view.Format.0, view.ViewDimension.0,
                (view.ViewDimension == D3D12_UAV_DIMENSION_TEXTURE2D)
                    .then(|| unsafe { (view.Anonymous.Texture2D.MipSlice, view.Anonymous.Texture2D.PlaneSlice) }),
                counter.is_some(), pair.guest.Width, pair.guest.Height, pair.guest.DepthOrArraySize
            ));
        }
        let host = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ..*view
        };
        unsafe { checked_uav(device, Some(&pair.image), None, &host, handle) };
        return;
    }
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_uav(device, buffer.expect("placed buffer"), counter, unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed UAV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        unsafe { checked_uav(device, buffer, counter.as_ref().copied(), desc, handle) };
    }
}

/// Whether this device can make a typed UAV in `format` and store through it
/// (D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW and
/// D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE), asked once per format.
fn typed_store(device: &ID3D12Device, format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT) -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static KNOWN: [AtomicU8; 256] = [const { AtomicU8::new(0) }; 256];
    let Some(slot) = KNOWN.get(format.0 as usize) else { return true };
    match slot.load(Ordering::Relaxed) {
        1 => return true,
        2 => return false,
        _ => {}
    }
    let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT { Format: format, ..Default::default() };
    let asked = unsafe {
        device.CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT, (&raw mut support).cast(), size_of_val(&support) as u32)
    };
    let yes = asked.is_err()
        || (support.Support1.contains(D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW)
            && support.Support2.contains(D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE));
    slot.store(if yes { 1 } else { 2 }, Ordering::Relaxed);
    yes
}

/// The typed format a UAV stores through, when it is typed: its own, or the
/// resource's when it names none. Raw and structured buffer views are untyped.
fn uav_typed_format(resource: Option<&ID3D12Resource>, view: &D3D12_UNORDERED_ACCESS_VIEW_DESC) -> Option<windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT> {
    if view.ViewDimension == D3D12_UAV_DIMENSION_BUFFER {
        let buffer = unsafe { view.Anonymous.Buffer };
        if view.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN || buffer.StructureByteStride != 0
            || buffer.Flags.contains(D3D12_BUFFER_UAV_FLAG_RAW)
        {
            return None;
        }
        return Some(view.Format);
    }
    let format = if view.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN { unsafe { resource?.GetDesc() }.Format } else { view.Format };
    (format != windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN).then_some(format)
}

fn typed_uav_guard() -> bool {
    crate::env_toggle!("TUXBOX_TYPED_UAV_GUARD") != Some("0")
}

/// CreateUnorderedAccessView that logs, once, the call that removes the device.
///
/// A typed UAV in a format this device cannot store through (B5G5R5A1, say,
/// which the console stores and NVIDIA does not) is an invalid description
/// whatever the resource's family or castable list, and NVIDIA removes the
/// device at startup for it (VIctory-zhai ca168de and dbd4fc2, GPL-3.0, with
/// credit). It is written as a null view of R32_UINT, and the first one of
/// each format is logged with the resource: the shader's stores to it are
/// lost until that format is given a storable host twin.
/// TUXBOX_TYPED_UAV_GUARD=0 passes it to the driver.
unsafe fn checked_uav(device: &Device, resource: Option<&ID3D12Resource>, counter: Option<&ID3D12Resource>,
    desc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC, handle: D3D12_CPU_DESCRIPTOR_HANDLE) {
    let mut refused = None;
    if let Some(view) = unsafe { desc.as_ref() } && typed_uav_guard()
        && let Some(format) = uav_typed_format(resource, view)
        && !typed_store(&device.native, format)
    {
        static LOGGED: [std::sync::atomic::AtomicBool; 256] = [const { std::sync::atomic::AtomicBool::new(false) }; 256];
        if LOGGED.get(format.0 as usize).is_some_and(|l| !l.swap(true, std::sync::atomic::Ordering::Relaxed)) {
            let res = resource.map(|r| unsafe { r.GetDesc() });
            crate::diagnostics::log_line(format_args!(
                "typed UAV in format {} refused: this device cannot store through it; written null, so its stores are lost (view dimension {}; resource {:?}; TUXBOX_TYPED_UAV_GUARD=0 passes it on)",
                format.0, view.ViewDimension.0,
                res.map(|r| format!("dimension={} {}x{}x{} format={} flags={:#x}", r.Dimension.0, r.Width, r.Height, r.DepthOrArraySize, r.Format.0, r.Flags.0))));
        }
        let mut null = *view;
        null.Format = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT;
        match null.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE2D => null.Anonymous.Texture2D.PlaneSlice = 0,
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => null.Anonymous.Texture2DArray.PlaneSlice = 0,
            _ => {}
        }
        refused = Some(null);
    }
    let (resource, counter, desc) = match &refused {
        Some(null) => (None, None, core::ptr::from_ref(null)),
        None => (resource, counter, desc),
    };
    let before = unsafe { device.native.GetDeviceRemovedReason() }.is_ok();
    unsafe { device.native.CreateUnorderedAccessView(resource, counter, Some(desc), handle) };
    if before && unsafe { device.native.GetDeviceRemovedReason() }.is_err() {
        static BAD_UAV: std::sync::Once = std::sync::Once::new();
        let res = resource.map(|r| unsafe { r.GetDesc() });
        let view = unsafe { desc.as_ref() }.map(|v| format!("format={} dimension={} {}", v.Format.0, v.ViewDimension.0, match v.ViewDimension {
            D3D12_UAV_DIMENSION_BUFFER => format!("{:?}", unsafe { v.Anonymous.Buffer }),
            D3D12_UAV_DIMENSION_TEXTURE2D => format!("{:?}", unsafe { v.Anonymous.Texture2D }),
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => format!("{:?}", unsafe { v.Anonymous.Texture2DArray }),
            D3D12_UAV_DIMENSION_TEXTURE3D => format!("{:?}", unsafe { v.Anonymous.Texture3D }),
            _ => String::new(),
        }));
        crate::diagnostics::log_once(&BAD_UAV, || format!(
            "UAV creation removed the device: view {view:?}; counter {}; resource {:?}", counter.is_some(),
            res.map(|r| format!("dimension={} {}x{}x{} mips={} format={} flags={:#x} layout={}", r.Dimension.0, r.Width, r.Height, r.DepthOrArraySize, r.MipLevels, r.Format.0, r.Flags.0, r.Layout.0))));
    }
}

pub(super) unsafe extern "system" fn rtv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_RENDER_TARGET_VIEW_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let resource = unsafe { view_resource(device, object, desc, handle) };
    let proxy = unsafe { super::resource_proxy(object, identity(device).as_raw()) };
    crate::diagnostics::display::capture_target(handle.ptr, resource);
    crate::diagnostics::display::rtv(handle.ptr, proxy.map(|p| p.native.as_raw()));
    device.state.placements.targets.set(handle.ptr, proxy.and_then(|p| p.state.placed()));
    if super::heap::residency::tracking() { device.state.placements.render_targets.set(handle.ptr, proxy.and_then(|p| p.state.render_heap())); }
    // A null RTV has no host resource to match, so give it the host format a
    // real one would get (B4G4R4A4 is not renderable on desktop) and plane 0.
    let null = resource.is_none();
    null_rtv(handle.ptr, null);
    let substitute = null || proxy.is_some_and(|p| p.state.translated().is_some());
    let mut translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if null || super::format::view_format(substitute, view.Format) != view.Format => {
            translated = D3D12_RENDER_TARGET_VIEW_DESC { Format: super::format::view_format(substitute, view.Format), ..*view };
            if null {
                match translated.ViewDimension {
                    D3D12_RTV_DIMENSION_TEXTURE2D => translated.Anonymous.Texture2D.PlaneSlice = 0,
                    D3D12_RTV_DIMENSION_TEXTURE2DARRAY => translated.Anonymous.Texture2DArray.PlaneSlice = 0,
                    _ => {}
                }
            }
            &raw const translated
        }
        _ => desc,
    };
    let mut adjusted;
    let desc = if let Some(window) = proxy.and_then(|p| p.state.buffer_window()) {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer RTV requires a buffer description")); };
        if view.ViewDimension != D3D12_RTV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer RTV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        b.FirstElement = rebase_view(window, adjusted.Format.0, 0, b.FirstElement, b.NumElements);
        &adjusted
    } else { desc };
    unsafe {
        device
            .native
            .CreateRenderTargetView(resource, Some(desc), handle)
    };
}

/// palmetto cf825083 (GPL-3): the title clears render-target views it created
/// from a null resource (the console clears nothing there). Handles last
/// written as null views are kept so the clear can be skipped.
fn null_rtvs() -> &'static std::sync::Mutex<std::collections::HashSet<usize>> {
    static SET: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<usize>>> = std::sync::OnceLock::new();
    SET.get_or_init(Default::default)
}

fn null_rtv(handle: usize, null: bool) {
    let mut set = null_rtvs().lock().unwrap_or_else(|e| e.into_inner());
    if null { set.insert(handle); } else if !set.is_empty() { set.remove(&handle); }
}

/// Whether the RTV at `handle` was last created from a null resource.
pub(crate) fn is_null_rtv(handle: usize) -> bool {
    let set = null_rtvs().lock().unwrap_or_else(|e| e.into_inner());
    !set.is_empty() && set.contains(&handle)
}

pub(super) unsafe extern "system" fn dsv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let resource = unsafe { view_resource(device, object, desc, handle) };
    if super::heap::residency::tracking() { device.state.placements.render_targets.set(handle.ptr,
        unsafe { super::resource_proxy(object, identity(device).as_raw()) }.and_then(|p| p.state.render_heap())); }
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::format::host_format(view.Format) != view.Format => {
            translated = D3D12_DEPTH_STENCIL_VIEW_DESC { Format: super::format::host_format(view.Format), ..*view };
            &raw const translated
        }
        None if let Some(guest) = unsafe { super::resource_proxy(object, identity(device).as_raw()) }
            .and_then(|p| p.state.translated()) => {
            // A typeless host image needs the typed depth format the guest implied.
            let mut view = D3D12_DEPTH_STENCIL_VIEW_DESC { Format: super::format::host_format(guest.Format), ..Default::default() };
            let layers = u32::from(guest.DepthOrArraySize);
            match (guest.Dimension, guest.SampleDesc.Count > 1) {
                (D3D12_RESOURCE_DIMENSION_TEXTURE1D, _) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE1D,
                (D3D12_RESOURCE_DIMENSION_TEXTURE1D, _) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE1DARRAY;
                    view.Anonymous.Texture1DArray = D3D12_TEX1D_ARRAY_DSV { MipSlice: 0, FirstArraySlice: 0, ArraySize: layers };
                }
                (_, false) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D,
                (_, true) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DMS,
                (_, false) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DARRAY;
                    view.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_DSV { MipSlice: 0, FirstArraySlice: 0, ArraySize: layers };
                }
                (_, true) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DMSARRAY;
                    view.Anonymous.Texture2DMSArray = D3D12_TEX2DMS_ARRAY_DSV { FirstArraySlice: 0, ArraySize: layers };
                }
            }
            translated = view;
            &raw const translated
        }
        _ => desc,
    };
    if super::stencil::needs_emulation(device) {
        let source = unsafe { super::resource_proxy(object, identity(device).as_raw()) };
        let surface = source
            .filter(|p| super::stencil::supported_view(&p.native, unsafe { desc.as_ref() }))
            .map(|p| {
                p.state
                    .stencil
                    .get_or_init(|| std::sync::Arc::new(super::stencil::Surface::new(&p.native)))
            });
        device.state.depth_views.set(handle.ptr, surface);
    }
    unsafe {
        device
            .native
            .CreateDepthStencilView(resource, Some(desc), handle)
    };
}

pub(super) unsafe extern "system" fn sampler(
    device: &Device,
    desc: *const D3D12_SAMPLER_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    check_destination(handle);
    if desc.is_null() {
        fail_call(format_args!("null D3D12 sampler description"));
    }
    let mut host = unsafe { *desc };
    host.Filter = super::features::host_filter(&device.native, host.Filter);
    unsafe { device.native.CreateSampler(&host, handle) };
    device.state.desc_table.created(handle.ptr, unsafe { &*desc });
}

// Xbox extended sampler state separates the minification, magnification and
// mip filters. Border color, LOD range and comparison are ordinary D3D values.
pub(super) unsafe extern "system" fn sampler_x(
    device: &Device, words: *const u32, handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    if words.is_null() {
        static NULL: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&NULL, || "null extended sampler skipped".into());
        return;
    }
    let w = unsafe { core::slice::from_raw_parts(words, 21) };
    // The raw words of the first calls settle the layout against the reading
    // as a desktop D3D12_SAMPLER_DESC (leonida PR 7115) in one run.
    static TRACED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if TRACED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
        crate::diagnostics::log_line(format_args!("device slot 61 sampler words {w:08x?}"));
    }
    let comparison = w[16] != D3D12_COMPARISON_FUNC_NEVER.0 as u32 && w[16] != 0;
    // Unknown reductions and filters fall back to standard / linear (logged
    // once each) instead of stopping the title; TUXBOX_UNMAPPED=failfast stops.
    let unknown = |what: &str, value: u32, fallback: i32| -> i32 {
        if crate::env_toggle!("TUXBOX_UNMAPPED") == Some("failfast") {
            fail_call(format_args!("unsupported sampler {what} {value}"));
        }
        static SEEN: std::sync::Mutex<Vec<(String, u32)>> = std::sync::Mutex::new(Vec::new());
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if seen.len() < 32 && !seen.iter().any(|(w, v)| w == what && *v == value) {
            seen.push((what.into(), value));
            crate::diagnostics::log_line(format_args!("unsupported sampler {what} {value}; using {fallback}"));
        }
        fallback
    };
    let reduction = if comparison { 1 } else { match w[5] { 0 => 0, 1 => 2, 2 => 3, other => unknown("reduction", other, 0) } };
    let linear = |filter: u32| match filter { 0 | 1 => 0, 2 | 3 => 1, other => unknown("filter", other, 1) };
    // An anisotropic sampler keeps its mip filter: 0x54 with a point mip
    // (VIctory 170ab23); features::host_filter falls back to 0x55.
    let bits = if w[1] == 3 && w[2] == 3 { 0x54 | linear(w[3]) } else {
        linear(w[1]) << 4 | linear(w[2]) << 2 | linear(w[3])
    };
    let host = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER(bits | reduction << 7),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(w[6] as _),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(w[7] as _),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(w[8] as _),
        MinLOD: f32::from_bits(w[9]), MaxLOD: f32::from_bits(w[10]),
        MipLODBias: f32::from_bits(w[11]) + f32::from_bits(w[12]),
        MaxAnisotropy: w[13].clamp(1, 16),
        ComparisonFunc: D3D12_COMPARISON_FUNC(w[16] as _),
        BorderColor: [w[17], w[18], w[19], w[20]].map(f32::from_bits),
    };
    let host = if crate::env_toggle!("TUXBOX_SAMPLER_SANITIZE") == Some("0") { host } else { sanitized(host, w[3], comparison) };
    unsafe { sampler(device, &host, handle) };
}

/// A console sampler's values the host accepts (VIctory 43b7c08, GPL-3.0,
/// PR #474, credit to its authors). The console takes any LOD bound, bias
/// and address mode word; D3D12 wants finite LOD bounds with min <= max, a
/// bias within D3D12_MIP_LOD_BIAS_MIN..MAX, address modes 1..=5 and a valid
/// comparison function, and a sampler outside that is undefined (debug
/// layer error, driver-specific results). Without a mip filter (mip word 0)
/// the console clamps sampling to the minimum LOD, where ours sampled every
/// mip with a point mip filter. NaN bounds fall back to 0 (min) and the min
/// (max), an inverted range clamps max up to min, an unknown address mode
/// clamps, and a plain sampler compares with NEVER. The filter decode is
/// unchanged. TUXBOX_SAMPLER_SANITIZE=0 passes the words through as before.
fn sanitized(mut host: D3D12_SAMPLER_DESC, mip: u32, comparison: bool) -> D3D12_SAMPLER_DESC {
    let finite = |value: f32, fallback: f32| if value.is_nan() { fallback } else { value.clamp(-D3D12_FLOAT32_MAX, D3D12_FLOAT32_MAX) };
    let min_lod = finite(host.MinLOD, 0.0);
    host.MinLOD = min_lod;
    host.MaxLOD = if mip == 0 { min_lod } else { finite(host.MaxLOD, min_lod).max(min_lod) };
    host.MipLODBias = if host.MipLODBias.is_nan() { 0.0 } else { host.MipLODBias.clamp(D3D12_MIP_LOD_BIAS_MIN, D3D12_MIP_LOD_BIAS_MAX) };
    for mode in [&mut host.AddressU, &mut host.AddressV, &mut host.AddressW] {
        if !(1..=5).contains(&mode.0) {
            *mode = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
        }
    }
    if !comparison || !(1..=8).contains(&host.ComparisonFunc.0) {
        host.ComparisonFunc = D3D12_COMPARISON_FUNC_NEVER;
    }
    host
}

/// Device slot 71: CreateUnorderedAccessView over a GPU address (GTA VI
/// game+0x511f261 builds a buffer description with UAV flags 0x20004, then
/// this view description with the address at +40 and a counter address at
/// +48). Before, the slot was unmapped and answered S_OK, so the descriptor
/// was never written. test114/115 lost the device with a page fault at
/// GPU VA 0. Buffer views import the title buffer (or a written-back copy),
/// anything else binds a null view, logged. TUXBOX_ADDRESS_UAV=0 leaves the
/// descriptor unwritten as before.
#[repr(C)]
pub(super) struct AddressUav {
    view: D3D12_UNORDERED_ACCESS_VIEW_DESC,
    address: u64,
    counter: u64,
}
const _: () = assert!(std::mem::offset_of!(AddressUav, address) == 40);

/// The D3D12 rules for a buffer UAV on `host`: the buffer allows unordered
/// access and is not on an upload or readback heap; RAW views are
/// R32_TYPELESS (UNKNOWN is fixed up) with no stride and a 16-byte
/// aligned start; structured views have UNKNOWN format and a stride that
/// is a multiple of 4; typed views have a format and no stride (and not a
/// 96-bit format, which has no typed UAV); the elements are not empty and
/// fit the host buffer.
fn check_address_uav(host: &ID3D12Resource, view: &mut D3D12_UNORDERED_ACCESS_VIEW_DESC, element: u64) -> Result<(), String> {
    let desc = unsafe { host.GetDesc() };
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER {
        return Err("host resource is not a buffer".into());
    }
    if (desc.Flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).0 == 0 {
        return Err(format!("host buffer lacks ALLOW_UNORDERED_ACCESS (flags {:#x})", desc.Flags.0));
    }
    let mut props = D3D12_HEAP_PROPERTIES::default();
    if unsafe { host.GetHeapProperties(Some(&mut props), None) }.is_ok()
        && (props.Type == D3D12_HEAP_TYPE_UPLOAD || props.Type == D3D12_HEAP_TYPE_READBACK)
    {
        return Err(format!("host buffer is on heap type {}", props.Type.0));
    }
    let buffer = unsafe { &mut view.Anonymous.Buffer };
    if buffer.NumElements == 0 {
        return Err("no elements".into());
    }
    if buffer.Flags == D3D12_BUFFER_UAV_FLAG_RAW {
        if view.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN {
            view.Format = windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_TYPELESS;
        }
        if view.Format != windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_TYPELESS || buffer.StructureByteStride != 0 {
            return Err(format!("RAW view with format {} and stride {}", view.Format.0, buffer.StructureByteStride));
        }
        if !(buffer.FirstElement * 4).is_multiple_of(16) {
            return Err(format!("RAW view starts at byte {:#x}, not 16-byte aligned", buffer.FirstElement * 4));
        }
    } else if buffer.Flags.0 != 0 {
        return Err(format!("buffer UAV flags {:#x}", buffer.Flags.0));
    } else if buffer.StructureByteStride != 0 {
        if view.Format != windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN || !buffer.StructureByteStride.is_multiple_of(4) {
            return Err(format!("structured view with format {} and stride {}", view.Format.0, buffer.StructureByteStride));
        }
    } else if view.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN || (5..=8).contains(&view.Format.0) {
        return Err(format!("typed view with format {}", view.Format.0));
    }
    let end = buffer.FirstElement.checked_add(u64::from(buffer.NumElements)).and_then(|e| e.checked_mul(element));
    if end.is_none_or(|end| end > desc.Width) {
        return Err(format!("elements {}+{} x {element} bytes outside the {:#x}-byte host buffer", buffer.FirstElement, buffer.NumElements, desc.Width));
    }
    Ok(())
}

pub(super) unsafe extern "system" fn address_uav(
    device: &Device,
    resource_desc: &D3D12_RESOURCE_DESC,
    desc: &AddressUav,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    if crate::env_toggle!("TUXBOX_ADDRESS_UAV") == Some("0") {
        return;
    }
    let _write = super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    check_destination(handle);
    let mut view = desc.view;
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    super::view_formats::write(handle.ptr, super::view_formats::uav_code(Some(&view)));
    device.state.blas.views.store(handle.ptr, None);
    device.state.placements.views.set(handle.ptr, None, true);
    if super::heap::residency::tracking() { device.state.placements.render_views.set(handle.ptr, None); }
    let null = |why: &str, view: &D3D12_UNORDERED_ACCESS_VIEW_DESC| {
        static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "address UAV at {:#x} ({} bytes, format {}, dimension {}, counter {:#x}): {why}; bound a null view ({n} so far; TUXBOX_ADDRESS_UAV=0 leaves it unwritten)",
                desc.address, resource_desc.Width, view.Format.0, view.ViewDimension.0, desc.counter));
        }
        device.state.imported_views.store(handle.ptr, None);
        // A typed R32_UINT buffer view: the title's own description may be
        // the invalid part (RAW with UNKNOWN, a stride with a format).
        let null = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R32_UINT,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            ..Default::default()
        };
        unsafe { device.native.CreateUnorderedAccessView(None, None, Some(&null), handle) };
    };
    if resource_desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER {
        return null("not a buffer view (texture address UAVs are not supported yet)", &view);
    }
    if desc.address == 0 {
        return null("address 0", &view);
    }
    let buffer = unsafe { view.Anonymous.Buffer };
    if desc.counter != 0 || buffer.CounterOffsetInBytes != 0 {
        return null("UAV counters are not supported here", &view);
    }
    let raw = buffer.Flags == D3D12_BUFFER_UAV_FLAG_RAW;
    let Some(element) = (if raw { Some(4) } else { element_stride(view.Format.0, buffer.StructureByteStride) }) else {
        return null("unsupported format", &view);
    };
    let element = u64::from(element);
    if buffer.FirstElement.checked_add(u64::from(buffer.NumElements)).and_then(|end| end.checked_mul(element))
        .is_none_or(|end| end > resource_desc.Width)
    {
        return null("elements outside the buffer", &view);
    }
    if crate::commands::page_mapping::GPU_APERTURE.wait(desc.address, 1).is_err() {
        return null("GPU mapping still pending", &view);
    }
    let width = resource_desc.Width.max(1);
    // An earlier import made without unordered access (before
    // TUXBOX_UAV_IMPORTS, or a placed resource's) cannot hold the view:
    // it goes through the written-back copy instead.
    let uav_capable = |e: &TextureAddress| (unsafe { e.native.GetDesc() }.Flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).0 != 0;
    let entry = match buffer_at(device, desc.address, width, element as u32, false) {
        Ok(Some(entry)) if entry.offset(desc.address).is_multiple_of(element) && uav_capable(&entry) => {
            unsafe { view.Anonymous.Buffer.FirstElement += entry.offset(desc.address) / element };
            entry
        }
        // No host buffer, or one the view cannot start in on a whole
        // element: a written-back copy that starts at the guest VA.
        _ => match shadow::writable_view(device, desc.address, width) {
            Ok(copy) => {
                view_fell_back("address UAV", desc.address, width, windows::Win32::Foundation::S_OK, true);
                copy
            }
            Err(e) => return null(&format!("no host buffer or copy ({:?})", e.code()), &view),
        },
    };
    // test118: the first address UAV served was followed at once by
    // DEVICE_REMOVED (INVALID_CALL). The view is checked against the
    // debug layer's rules for the host buffer it lands on; any failure
    // binds a null view (logged), never an invalid one.
    // TUXBOX_ADDRESS_UAV_CHECKS=0 creates the view unchecked, as 0173b3f did.
    if crate::env_toggle!("TUXBOX_ADDRESS_UAV_CHECKS") != Some("0") {
        if let Err(why) = check_address_uav(&entry.native, &mut view, element) {
            return null(&why, &view);
        }
    }
    shadow::mark_written(device, entry.native.as_raw());
    unsafe { device.native.CreateUnorderedAccessView(Some(&entry.native), None, Some(&view), handle) };
    device.state.imported_views.store(handle.ptr, Some(DescriptorBacking::Registered { _address: entry }));
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "address UAV served (device slot 71): {:#x}, {} bytes, format {} (TUXBOX_ADDRESS_UAV=0 off)",
        desc.address, resource_desc.Width, view.Format.0));
}
