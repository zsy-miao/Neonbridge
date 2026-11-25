//! Host copies of constant and shader resource buffers.
use super::*;

pub(in super::super) unsafe extern "system" fn cbv(
    device: &Device,
    desc: *const D3D12_CONSTANT_BUFFER_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    check_destination(handle);
    device.state.placements.views.set(handle.ptr, None, false);
    device.state.blas.views.store(handle.ptr, None);
    // Xbox uses the actual constant data length. A desktop CBV's size is in
    // 256-byte units. Keep the address at the start chosen by the guest.
    // A failed import used to fail-fast (Scarlett 0xbb305ff70). Mirror that
    // one view into an upload buffer instead of failing the call.
    let mut aligned = unsafe { desc.as_ref() }.copied();
    let mut owner = None;
    if let Some(value) = &mut aligned {
        let guest_bytes = value.SizeInBytes;
        value.SizeInBytes = desktop_cbv_bytes(guest_bytes);
        let address = value.BufferLocation;
        if address != 0 && value.SizeInBytes != 0 {
            match buffer_at(device, address, u64::from(value.SizeInBytes), 1, false, D3D12_RESOURCE_FLAG_NONE) {
                Ok(found) => {
                    if let Some(buffer) = &found {
                        value.BufferLocation = unsafe { buffer.native.GetGPUVirtualAddress() } + buffer.offset(address);
                    }
                    owner = found;
                }
                Err(error) => match cbv_mirror(device, address, guest_bytes, value.SizeInBytes) {
                    Ok(created) => value.BufferLocation = created.gpu_address(),
                    Err(mirror_error) => fail_call(format_args!(
                        "CBV buffer {address:#x}, {} bytes: {:?} (import {:?})",
                        value.SizeInBytes, mirror_error.code(), error.code()
                    )),
                },
            }
        }
    }
    device.state.imported_views.store(handle.ptr, Some(DescriptorBacking::ConstantBuffer { _address: owner }));
    unsafe {
        device
            .native
            .CreateConstantBufferView(aligned.as_ref().map(core::ptr::from_ref), handle)
    };
}

// A guest CBV that ends too close to a 64 KiB page cannot be a desktop view
// in place: vkd3d requires a 256-byte size, and that window crosses into the
// next page. The upload buffer is 256-byte aligned and fully mapped. Execute
// copies the desktop window into it before the GPU reads the descriptor.
// Bytes past the guest length stay in that window, so the copy is the whole
// rounded size. Only the unreadable tail is left zero.
pub(in super::super) struct CbvMirror {
    pub(super) guest: u64,
    pub(super) mapped: usize,
    pub(super) capacity: usize,
    pub(super) buffer: ID3D12Resource,
}

impl CbvMirror {
    pub(super) fn gpu_address(&self) -> u64 {
        unsafe { self.buffer.GetGPUVirtualAddress() }
    }

    pub(super) fn snapshot(&self) -> usize {
        if self.mapped == 0 || self.capacity == 0 {
            return 0;
        }
        let readable = readable_prefix(self.guest, self.capacity);
        // SAFETY: readable_prefix stopped at the first page that is not a
        // committed readable guest mapping. `mapped` covers `capacity` bytes.
        // The tail past `readable` is cleared so an earlier, longer prefix
        // cannot linger in the desktop window.
        unsafe {
            let dst = self.mapped as *mut u8;
            if readable < self.capacity {
                core::ptr::write_bytes(dst.add(readable), 0, self.capacity - readable);
            }
            if readable > 0 {
                core::ptr::copy_nonoverlapping(self.guest as *const u8, dst, readable);
            }
        }
        readable
    }
}

impl Drop for CbvMirror {
    fn drop(&mut self) {
        if self.mapped != 0 {
            // SAFETY: Map succeeded and snapshot copies have finished. The
            // last Arc is dropping, so no descriptor still reads this mapping.
            unsafe { self.buffer.Unmap(0, None) };
            self.mapped = 0;
        }
    }
}

pub(super) fn readable_prefix(address: u64, bytes: usize) -> usize {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    let mut got = 0usize;
    while got < bytes {
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let at = address.saturating_add(got as u64);
        // SAFETY: VirtualQuery only reads the process memory map.
        let queried = unsafe { VirtualQuery(Some(at as _), &mut info, size_of_val(&info)) };
        let protect = info.Protect.0;
        if queried == 0 || info.State != MEM_COMMIT || protect & PAGE_GUARD.0 != 0 || protect & 0xff == PAGE_NOACCESS.0 {
            break;
        }
        let region_end = (info.BaseAddress as u64).saturating_add(info.RegionSize as u64);
        if region_end <= at {
            break;
        }
        let room = (region_end - at) as usize;
        got += room.min(bytes - got);
    }
    got
}

pub(super) fn cbv_mirrors() -> &'static std::sync::Mutex<Vec<std::sync::Arc<CbvMirror>>> {
    static MIRRORS: std::sync::LazyLock<std::sync::Mutex<Vec<std::sync::Arc<CbvMirror>>>> =
        std::sync::LazyLock::new(Default::default);
    &MIRRORS
}

pub(super) fn cbv_mirror(device: &Device, address: u64, guest_bytes: u32, desktop_bytes: u32) -> windows::core::Result<std::sync::Arc<CbvMirror>> {
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    static NOTED: AtomicU32 = AtomicU32::new(0);
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: u64::from(desktop_bytes),
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native: Option<ID3D12Resource> = None;
    // SAFETY: D3D12 borrows the descriptors only for this call.
    unsafe {
        device.native.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut native,
        )?;
    }
    let Some(buffer) = native else {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    };
    let mut mapped = core::ptr::null_mut();
    let unread = D3D12_RANGE { Begin: 0, End: 0 };
    // SAFETY: the upload buffer has `desktop_bytes` bytes and is not mapped yet.
    unsafe { buffer.Map(0, Some(&unread), Some(&mut mapped))? };
    if mapped.is_null() {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    }
    let capacity = desktop_bytes as usize;
    // SAFETY: Map returned a writable pointer of the committed width.
    unsafe { core::ptr::write_bytes(mapped.cast::<u8>(), 0, capacity) };
    let mirror = std::sync::Arc::new(CbvMirror {
        guest: address,
        mapped: mapped as usize,
        capacity,
        buffer,
    });
    let copied = mirror.snapshot();
    if NOTED.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::diagnostics::log(&format!(
            "CBV mirror {address:#x} guest={guest_bytes} desktop={desktop_bytes} copied={copied}"
        ));
    }
    cbv_mirrors().lock().unwrap_or_else(|error| error.into_inner()).push(mirror.clone());
    mirror_count().fetch_add(1, Ordering::Release);
    Ok(mirror)
}

pub(super) fn mirror_count() -> &'static std::sync::atomic::AtomicU32 {
    static COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    &COUNT
}

/// Refresh upload mirrors before a queue submission. The list is process-global
/// so the device object's layout stays unchanged. No mirrors means no lock:
/// an empty refresh on every submit was suspected in the account-picker stall,
/// and a boot with this call removed still stalled there.
pub(crate) fn snapshot_cbv_mirrors() {
    if mirror_count().load(std::sync::atomic::Ordering::Acquire) == 0 {
        return;
    }
    let mirrors = cbv_mirrors().lock().unwrap_or_else(|error| error.into_inner());
    let live: Vec<_> = mirrors.iter().cloned().collect();
    drop(mirrors);
    for mirror in live {
        mirror.snapshot();
    }
    let mirrors = srv_mirrors().lock().unwrap_or_else(|error| error.into_inner());
    let live: Vec<_> = mirrors.iter().cloned().collect();
    drop(mirrors);
    for mirror in live {
        mirror.snapshot();
    }
}

// Boot 190327 abort: an address SRV at 0xbbee8f770 lies in a reserved region,
// so the placed-buffer import returns E_INVALIDARG and fail_call aborts the
// title. Keep the view alive on an upload buffer, and refresh it before
// submit once the guest actually commits those pages.
pub(super) struct SrvMirror {
    pub(super) guest: u64,
    pub(super) mapped: usize,
    pub(super) capacity: usize,
    pub(super) buffer: ID3D12Resource,
}

impl Drop for SrvMirror {
    fn drop(&mut self) {
        if self.mapped != 0 {
            // SAFETY: Map succeeded and snapshot copies have finished.
            unsafe { self.buffer.Unmap(0, None) };
            self.mapped = 0;
        }
    }
}

impl SrvMirror {
    pub(super) fn snapshot(&self) -> usize {
        if self.mapped == 0 || self.capacity == 0 {
            return 0;
        }
        let readable = readable_prefix(self.guest, self.capacity);
        unsafe {
            let dst = self.mapped as *mut u8;
            if readable < self.capacity {
                core::ptr::write_bytes(dst.add(readable), 0, self.capacity - readable);
            }
            if readable > 0 {
                core::ptr::copy_nonoverlapping(self.guest as *const u8, dst, readable);
            }
        }
        readable
    }
}

pub(super) fn srv_mirrors() -> &'static std::sync::Mutex<Vec<std::sync::Arc<SrvMirror>>> {
    static MIRRORS: std::sync::LazyLock<std::sync::Mutex<Vec<std::sync::Arc<SrvMirror>>>> =
        std::sync::LazyLock::new(Default::default);
    &MIRRORS
}

pub(super) fn srv_mirror(device: &Device, address: u64, bytes: u64) -> windows::core::Result<std::sync::Arc<TextureAddress>> {
    use std::sync::atomic::Ordering;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    // A multi-gigabyte view of uncommitted VA is not a mirror candidate.
    if bytes == 0 || bytes > 1 << 30 {
        return Err(windows::Win32::Foundation::E_INVALIDARG.into());
    }
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native: Option<ID3D12Resource> = None;
    unsafe {
        device.native.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut native,
        )?;
    }
    let Some(buffer) = native else {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    };
    let mut mapped = core::ptr::null_mut();
    let unread = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { buffer.Map(0, Some(&unread), Some(&mut mapped))? };
    if mapped.is_null() {
        return Err(windows::Win32::Foundation::E_FAIL.into());
    }
    let capacity = bytes as usize;
    unsafe { core::ptr::write_bytes(mapped.cast::<u8>(), 0, capacity) };
    let mirror = std::sync::Arc::new(SrvMirror {
        guest: address,
        mapped: mapped as usize,
        capacity,
        buffer: buffer.clone(),
    });
    let copied = mirror.snapshot();
    static NOTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if NOTED.fetch_add(1, Ordering::Relaxed) < 4 {
        crate::diagnostics::log(&format!(
            "SRV mirror {address:#x} bytes={bytes} copied={copied}"
        ));
    }
    srv_mirrors().lock().unwrap_or_else(|error| error.into_inner()).push(mirror);
    mirror_count().fetch_add(1, Ordering::Release);
    Ok(TextureAddress::at(device, &buffer, address as usize, false, None))
}
