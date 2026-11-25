//! Views of buffers and textures named by guest address.
use super::*;

#[repr(C)]
pub(in super::super) struct AddressSrv {
    pub(super) view: D3D12_SHADER_RESOURCE_VIEW_DESC,
    pub(super) address: u64,
}

pub(super) fn buffer_at(device: &Device, address: u64, bytes: u64, stride: u32, required: bool, flags: D3D12_RESOURCE_FLAGS)
    -> windows::core::Result<Option<std::sync::Arc<TextureAddress>>> {
    use windows::Win32::Foundation::E_INVALIDARG;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    crate::commands::page_mapping::GPU_APERTURE.wait(address,bytes)?;
    if let Some(entry) = device.state.texture_addresses.preceding(address as usize).filter(|entry| !entry.is_stale()) {
        let host = unsafe { entry.native.GetDesc() };
        if host.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
            && host.Flags.0 & flags.0 == flags.0
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
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQuery(Some(address as _), &mut region, size_of_val(&region)) } == 0 || region.State != MEM_COMMIT {
        return if required { Err(E_INVALIDARG.into()) } else { Ok(None) };
    }
    let host = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let origin = region.AllocationBase.addr() as u64;
    let import = |page: u64, tight: bool| -> windows::core::Result<std::sync::Arc<TextureAddress>> {
        let mut host = host;
        // D3D12_RESOURCE_FLAG_USE_TIGHT_ALIGNMENT (Agility SDK 617).
        host.Flags = D3D12_RESOURCE_FLAGS(flags.0 | if tight { 0x400 } else { 0 });
        host.Width = bytes.checked_add(address - page).ok_or(E_INVALIDARG)?;
        if !tight { host.Width = host.Width.checked_next_multiple_of(65536).ok_or(E_INVALIDARG)?; }
        let (native, heap) = super::super::heap::import_buffer_with_heap(device, page, &host, D3D12_RESOURCE_STATE_COMMON, "view")?;
        Ok(TextureAddress::with_heap(device, &native, page as usize, false, None, None, None, 0, Some(heap)))
    };
    if !tight {
        return import(nearest, false).map(Some);
    }
    let mut probe = host;
    probe.Flags = D3D12_RESOURCE_FLAGS(flags.0 | 0x400) | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER;
    let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(&probe)) };
    let close = if info.Alignment == 0 || info.SizeInBytes == u64::MAX { None } else {
        // Search no further than a single period of the alignment congruence.
        (0..info.Alignment.min((address - origin) / u64::from(stride) + 1))
            .map(|n| address - n * u64::from(stride))
            .find(|&base| (base - origin).is_multiple_of(info.Alignment))
    };
    let first = match close {
        Some(page) => import(page, true),
        None => Err(E_INVALIDARG.into()),
    };
    let error = match first {
        Ok(entry) => return Ok(Some(entry)),
        Err(error) => error,
    };
    // No nearby start works. An earlier page boundary of the same allocation
    // can still put the view on a whole element.
    let earlier = earlier_page(address, stride, origin);
    let second = earlier.map(|page| import(page, false));
    static NOTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if NOTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 16 {
        crate::diagnostics::log(&format!(
            "buffer import at {address:#x} bytes={bytes:#x} stride={stride}: tight start {close:?} alignment={:#x} failed {:?}; allocation={origin:#x} region={:#x}; earlier page {earlier:?} {}",
            info.Alignment, error.code(), region.RegionSize,
            match &second { Some(Ok(_)) => "worked", Some(Err(_)) => "failed", None => "unavailable" }
        ));
    }
    match second {
        Some(result) => result.map(Some),
        None => Err(error),
    }
}

/// The closest 64 KiB boundary at or after `origin` from which `address` is a
/// whole number of elements away.
pub(super) fn earlier_page(address: u64, stride: u32, origin: u64) -> Option<u64> {
    let stride = u64::from(stride.max(1));
    let nearest = address & !0xffff;
    // Page offsets repeat modulo the stride within `stride` pages.
    (0..stride)
        .map_while(|pages| nearest.checked_sub(pages * 65536).filter(|&page| page >= origin))
        .find(|&page| (address - page).is_multiple_of(stride))
}

pub(crate) unsafe fn command_buffer_address(device: *mut c_void, address: u64, bytes: u64) -> windows::core::Result<(Option<ID3D12Resource>, u64)> {
    let device = unsafe { &*device.cast::<Device>() };
    let Some(buffer) = buffer_at(device, address, bytes, 1, false, D3D12_RESOURCE_FLAG_NONE)? else { return Ok((None, address)) };
    let host = unsafe { buffer.native.GetGPUVirtualAddress() } + buffer.offset(address);
    Ok((Some(buffer.native.clone()), host))
}

/// Where a linear CPU upload should land. Textures are resolved before buffers
/// so a placed image is not imported as a buffer over its guest pages.
pub(crate) enum LinearResource {
    Buffer { resource: ID3D12Resource, offset: u64 },
    Texture {
        resource: ID3D12Resource,
        placed: Option<std::sync::Arc<super::super::placement::Texture>>,
        offset: u64,
    },
}

pub(super) fn texture_address_covering(device: &Device, address: u64, bytes: u64) -> Option<LinearResource> {
    let entry = device.state.texture_addresses.preceding(address as usize)?;
    let desc = unsafe { entry.native.GetDesc() };
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER || address < entry.base as u64 {
        return None;
    }
    let offset = address - entry.base as u64;
    let end = offset.checked_add(bytes)?;
    let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(&desc)) };
    let covers = if info.SizeInBytes != 0 && info.SizeInBytes != u64::MAX {
        end <= info.SizeInBytes
    } else {
        offset == 0
    };
    covers.then(|| LinearResource::Texture {
        resource: entry.native.clone(),
        placed: None,
        offset,
    })
}

pub(crate) unsafe fn linear_resource(device_ptr: *mut c_void, address: u64, bytes: u64) -> windows::core::Result<LinearResource> {
    use windows::Win32::Foundation::E_INVALIDARG;
    if address == 0 || bytes == 0 {
        return Err(E_INVALIDARG.into());
    }
    let device = unsafe { &*device_ptr.cast::<Device>() };
    if let Some(texture) = device.state.placements.covering(address, bytes) {
        return Ok(LinearResource::Texture {
            offset: address - texture.address,
            resource: texture.native.clone(),
            placed: Some(texture),
        });
    }
    if let Some(texture) = texture_address_covering(device, address, bytes) {
        return Ok(texture);
    }
    let (buffer, host) = unsafe { command_buffer_address(device_ptr, address, bytes)? };
    let Some(buffer) = buffer else {
        crate::diagnostics::recording_error(format_args!(
            "slot 62 address {address:#x} ({bytes:#x} bytes) is not a buffer or texture"
        ));
        return Err(E_INVALIDARG.into());
    };
    let base = unsafe { buffer.GetGPUVirtualAddress() };
    let Some(offset) = host.checked_sub(base) else {
        return Err(E_INVALIDARG.into());
    };
    Ok(LinearResource::Buffer { resource: buffer, offset })
}

pub(super) fn buffer_stride(view: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> Option<u32> {
    let buffer = unsafe { view.Anonymous.Buffer };
    element_stride(view.Format.0, buffer.StructureByteStride)
}

pub(super) fn element_stride(format: i32, structure_stride: u32) -> Option<u32> {
    if structure_stride != 0 {
        return (structure_stride <= 2048).then_some(structure_stride);
    }
    Some(match format {
        1..=4 => 16, 5..=8 => 12, 9..=22 => 8, 23..=47 | 67 | 87..=93 => 4,
        48..=59 | 85 | 86 | 115 => 2, 60..=65 => 1, _ => return None,
    })
}

pub(super) fn rebase_view(window: (u64, u64), format: i32, structure_stride: u32, first: u64, elements: u32) -> u64 {
    let (prefix, width) = window;
    let stride = element_stride(format, structure_stride).map(u64::from)
        .unwrap_or_else(|| fail_call(format_args!("unsupported placed buffer view stride for format {format}")));
    if !prefix.is_multiple_of(stride) || first.checked_add(u64::from(elements)).is_none_or(|end| end > width / stride) {
        fail_call(format_args!("placed buffer view outside element range: prefix={prefix}, width={width}, stride={stride}, first={first}, count={elements}"));
    }
    first + prefix / stride
}

pub(super) enum PlacedView<'a> {
    Elements(u64, Option<ID3D12Resource>),
    Displaced(&'a AddressViews, u64, u32),
}

pub(super) fn placed_view<'a>(
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
    let (alias, extra) = heap.buffer_alias(&device.native, base - heap_base, width, element, flags)
        .unwrap_or_else(|e| fail_call(format_args!("buffer view alias at {base:#x} width={width} stride={element}: {:?}", e.code())));
    PlacedView::Elements(first + extra, Some(alias))
}

pub(in super::super) unsafe extern "system" fn address_srv(
    device: &Device,
    resource_desc: &D3D12_RESOURCE_DESC,
    desc: &AddressSrv,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    check_destination(handle);
    crate::commands::page_mapping::GPU_APERTURE.wait(desc.address,1)
        .unwrap_or_else(|e| fail_call(format_args!("address SRV pending GPU mapping: {:?}", e.code())));
    let mut view = desc.view;
    let mut byte_view = None;
    let buffer_view = resource_desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
        && view.ViewDimension == D3D12_SRV_DIMENSION_BUFFER;
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
        let entry = match buffer_at(device, desc.address, resource_desc.Width, placement_stride, true, D3D12_RESOURCE_FLAG_NONE) {
            Ok(Some(entry)) => entry,
            Ok(None) => fail_call(format_args!(
                "import SRV at {:#x} produced no buffer ({} bytes)",
                desc.address, resource_desc.Width
            )),
            Err(error) => match srv_mirror(device, desc.address, resource_desc.Width) {
                Ok(created) => created,
                Err(mirror_error) => fail_call(format_args!(
                    "import SRV at {:#x}, {} bytes stride={stride} fmt={} first={} count={} failed {:?} (mirror {:?}; {}; region {:#x?}, placement stride {})",
                    desc.address, resource_desc.Width, view.Format.0, buffer.FirstElement, buffer.NumElements,
                    error.code(), mirror_error.code(), mode.name(), region(), placement_stride
                )),
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
        Some(device.state.placements.texture(&device.native, desc.address, resource_desc,
            D3D12_RESOURCE_STATE_COMMON, None)
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
    let native = existing.as_ref().map(|e| &e.native).or_else(|| placed.as_ref().map(|p| &p.native)).or_else(|| imported.as_ref().map(|e| &e.native));
    crate::diagnostics::display::texture_view(handle.ptr, native, Some(view.Format.0));
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_srv(device, native.expect("buffer import retained"), view, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("byte-offset SRV at {:#x} offset={offset} stride={stride}: {:?}", desc.address, e.code())));
    } else {
        unsafe { device.native.CreateShaderResourceView(native, Some(&view), handle) };
    }
}

/// `D3D12XBOX_UNORDERED_ACCESS_VIEW_DESC`: the standard description followed
/// by the buffer's address, an optional counter address and a memory type.
#[repr(C)]
pub(in super::super) struct AddressUav {
    pub(super) view: D3D12_UNORDERED_ACCESS_VIEW_DESC,
    pub(super) address: u64,
    pub(super) counter_address: u64,
    pub(super) memory_type: u32,
}

/// CreatePlacedRawUnorderedAccessViewX (device slot 71): a buffer UAV at a
/// guest address. The imported buffer must allow unordered access, so an
/// earlier read-only import of the same pages is not reused. Texture UAVs,
/// counters and byte-offset starts are refused rather than guessed.
pub(in super::super) unsafe extern "system" fn address_uav(
    device: &Device,
    resource_desc: &D3D12_RESOURCE_DESC,
    desc: &AddressUav,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    check_destination(handle);
    if desc.address == 0
        || desc.counter_address != 0
        || resource_desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER
        || desc.view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER
    {
        fail_call(format_args!(
            "unsupported address UAV: address={:#x} counter={:#x} resource dimension={} view dimension={}",
            desc.address, desc.counter_address, resource_desc.Dimension.0, desc.view.ViewDimension.0
        ));
    }
    crate::commands::page_mapping::GPU_APERTURE.wait(desc.address, 1)
        .unwrap_or_else(|e| fail_call(format_args!("address UAV pending GPU mapping: {:?}", e.code())));
    let mut view = desc.view;
    let buffer = unsafe { view.Anonymous.Buffer };
    let stride = element_stride(view.Format.0, buffer.StructureByteStride)
        .unwrap_or_else(|| fail_call(format_args!("unsupported address UAV format {}", view.Format.0)));
    rebase_view((0, resource_desc.Width), view.Format.0, buffer.StructureByteStride, buffer.FirstElement, buffer.NumElements);
    let entry = match buffer_at(device, desc.address, resource_desc.Width, stride, true, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
        Ok(Some(entry)) => entry,
        Ok(None) => fail_call(format_args!("import UAV at {:#x} produced no buffer ({} bytes)", desc.address, resource_desc.Width)),
        Err(error) => fail_call(format_args!(
            "import UAV at {:#x}, {} bytes stride={stride} fmt={} first={} count={} failed {:?}",
            desc.address, resource_desc.Width, view.Format.0, buffer.FirstElement, buffer.NumElements, error.code()
        )),
    };
    let offset = entry.offset(desc.address);
    if !offset.is_multiple_of(u64::from(stride)) {
        fail_call(format_args!("address UAV at {:#x} starts {offset} bytes into its buffer, not a whole {stride}-byte element", desc.address));
    }
    view.Anonymous.Buffer.FirstElement = buffer.FirstElement + offset / u64::from(stride);
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    device.state.imported_views.store(handle.ptr, Some(DescriptorBacking::Registered { _address: entry.clone() }));
    device.state.blas.views.store(handle.ptr, None);
    device.state.placements.views.set(handle.ptr, None, true);
    unsafe { device.native.CreateUnorderedAccessView(Some(&entry.native), None, Some(&view), handle) };
}
