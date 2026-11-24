//! Standard Xbox resource views and samplers, device slots 17 through 22.
mod byte_offset;
pub(super) use byte_offset::AddressViews;

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
    check_destination(handle);
    device.state.placements.views.set(handle.ptr, None, false);
    device.state.blas.views.store(handle.ptr, None);
    // Xbox uses the actual constant data length. A desktop CBV's size is in
    // 256-byte units. Keep the address at the start chosen by the guest.
    let mut aligned = unsafe { desc.as_ref() }.copied();
    let mut owner = None;
    if let Some(value) = &mut aligned {
        if value.SizeInBytes <= D3D12_REQ_CONSTANT_BUFFER_ELEMENT_COUNT * 16 {
            value.SizeInBytes = value.SizeInBytes.next_multiple_of(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT);
        }
        let address = value.BufferLocation;
        if address != 0 && value.SizeInBytes != 0 {
            owner = buffer_at(device, address, u64::from(value.SizeInBytes), 1, false)
                .unwrap_or_else(|e| fail_call(format_args!("CBV buffer {address:#x}, {} bytes: {:?}", value.SizeInBytes, e.code())));
            if let Some(buffer) = &owner {
                value.BufferLocation = unsafe { buffer.native.GetGPUVirtualAddress() } + buffer.offset(address);
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

// Desktop textures have no buffer GPU VA. Reserve address space on demand
// for the Xbox descriptor API, which identifies the original texture by VA.
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
    fn offset(&self, address: u64) -> u64 {
        self.buffer_offset + (address - self.base as u64)
    }

    pub(super) fn register(device: &Device, native: &ID3D12Resource, desc: &D3D12_RESOURCE_DESC) -> std::sync::Arc<Self> {
        use windows::Win32::System::Memory::{VirtualAlloc, MEM_RESERVE, PAGE_NOACCESS};
        let size = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(desc)) }.SizeInBytes;
        let base = unsafe { VirtualAlloc(None, size as usize, MEM_RESERVE, PAGE_NOACCESS) }.addr();
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
            if self.owns_address { let _ = unsafe { VirtualFree(self.base as _, 0, MEM_RELEASE) }; }
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

fn buffer_at(device: &Device, address: u64, bytes: u64, stride: u32, required: bool)
    -> windows::core::Result<Option<std::sync::Arc<TextureAddress>>> {
    use windows::Win32::Foundation::E_INVALIDARG;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    crate::commands::page_mapping::GPU_APERTURE.wait(address,bytes)?;
    if let Some(entry) = device.state.texture_addresses.preceding(address as usize) {
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
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQuery(Some(address as _), &mut region, size_of_val(&region)) } == 0 || region.State != MEM_COMMIT {
        return if required { Err(E_INVALIDARG.into()) } else { Ok(None) };
    }
    let mut host = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1,
        // D3D12_RESOURCE_FLAG_USE_TIGHT_ALIGNMENT (Agility SDK 617).
        Flags: if tight { D3D12_RESOURCE_FLAGS(0x400) } else { D3D12_RESOURCE_FLAG_NONE },
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let page = if tight {
        let probe = D3D12_RESOURCE_DESC { Flags: host.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER, ..host };
        let info = unsafe { device.native.GetResourceAllocationInfo(0, std::slice::from_ref(&probe)) };
        if info.Alignment == 0 || info.SizeInBytes == u64::MAX { return Err(E_INVALIDARG.into()); }
        let origin = region.AllocationBase.addr() as u64;
        // Search no further than a single period of the alignment congruence.
        (0..info.Alignment.min((address - origin) / u64::from(stride) + 1))
            .map(|n| address - n * u64::from(stride))
            .find(|&base| (base - origin).is_multiple_of(info.Alignment)).ok_or(E_INVALIDARG)?
    } else { nearest };
    host.Width = bytes.checked_add(address - page).ok_or(E_INVALIDARG)?;
    if !tight { host.Width = host.Width.checked_next_multiple_of(65536).ok_or(E_INVALIDARG)?; }
    let native = super::heap::import_buffer(device, page, &host, D3D12_RESOURCE_STATE_COMMON)?;
    Ok(Some(TextureAddress::at(device, &native, page as usize, false, None)))
}

pub(crate) unsafe fn command_buffer_address(device: *mut c_void, address: u64, bytes: u64) -> windows::core::Result<(Option<ID3D12Resource>, u64)> {
    let device = unsafe { &*device.cast::<Device>() };
    let Some(buffer) = buffer_at(device, address, bytes, 1, false)? else { return Ok((None, address)) };
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

enum PlacedView<'a> {
    Elements(u64, Option<ID3D12Resource>),
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
    let (alias, extra) = heap.buffer_alias(&device.native, base - heap_base, width, element, flags)
        .unwrap_or_else(|e| fail_call(format_args!("buffer view alias at {base:#x} width={width} stride={element}: {:?}", e.code())));
    PlacedView::Elements(first + extra, Some(alias))
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
        let entry = buffer_at(device, desc.address, resource_desc.Width, placement_stride, true)
            .unwrap_or_else(|e| fail_call(format_args!("import SRV at {:#x}, {} bytes stride={stride} fmt={} first={} count={} failed {:?} ({}; region {:#x?}, placement stride {})",
                desc.address, resource_desc.Width, view.Format.0, buffer.FirstElement, buffer.NumElements, e.code(), mode.name(), region(), placement_stride)))
            .expect("required buffer import");
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
            PlacedView::Displaced(mode, offset, stride) => byte_view = Some((mode, offset, stride)),
        }
        &adjusted
    } else { desc };
    let translated;
    let substitute = proxy.and_then(|p| p.state.translated());
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::format::view_format(substitute.is_some(), view.Format) != view.Format => {
            translated = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: super::format::view_format(substitute.is_some(), view.Format), ..*view
            };
            &raw const translated
        }
        None if substitute.is_some_and(super::format::is_depth) => {
            fail_call(format_args!("Scarlett depth SRV needs an explicit plane format"))
        }
        _ => desc,
    };
    crate::diagnostics::display::texture_view(handle.ptr, resource, unsafe { desc.as_ref() }.map(|view| view.Format.0));
    let texture = proxy.and_then(|resource| resource.state.placed());
    let scene = device.state.blas.scene_view(proxy, unsafe { desc.as_ref() });
    device.state.blas.views.store(handle.ptr, scene);
    device.state.placements.views.set(handle.ptr, texture, false);
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_srv(device, resource.expect("placed buffer"), unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed SRV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        unsafe { device.native.CreateShaderResourceView(alias.as_ref().map(|(native, _)| native).or(resource), Some(desc), handle) };
    }
    device.state.imported_views.store(handle.ptr, alias.map(|(native, original)| DescriptorBacking::BufferAlias { native, original }));
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
    let owner = identity(device).as_raw();
    let proxy = unsafe { super::resource_proxy(object, owner) };
    let placed = proxy.and_then(|p| p.state.placed());
    let window = proxy.and_then(|p| p.state.buffer_window());
    let counter_window = unsafe { super::resource_proxy(counter, owner) }.and_then(|p| p.state.buffer_window());
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
    device.state.blas.views.store(handle.ptr, None);
    device.state.imported_views.store(handle.ptr, uav_alias.as_ref().map(|(alias, original)|
        DescriptorBacking::BufferAlias { native: alias.clone(), original: original.clone() }));
    let buffer = unsafe { view_resource(device, object, desc, handle) };
    let buffer = uav_alias.as_ref().map(|(alias, _)| alias).or(buffer);
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
        unsafe { device.native.CreateUnorderedAccessView(None, None, Some(view), handle) };
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
        unsafe {
            device
                .native
                .CreateUnorderedAccessView(&pair.image, None, Some(&host), handle)
        };
        return;
    }
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_uav(device, buffer.expect("placed buffer"), counter, unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed UAV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        unsafe { device.native.CreateUnorderedAccessView(buffer, counter, Some(desc), handle) };
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
    let substitute = proxy.is_some_and(|p| p.state.translated().is_some());
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::format::view_format(substitute, view.Format) != view.Format => {
            translated = D3D12_RENDER_TARGET_VIEW_DESC { Format: super::format::view_format(substitute, view.Format), ..*view };
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

pub(super) unsafe extern "system" fn dsv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let resource = unsafe { view_resource(device, object, desc, handle) };
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
    unsafe { device.native.CreateSampler(desc, handle) };
}

// Xbox extended sampler state separates the minification, magnification and
// mip filters. Border color, LOD range and comparison are ordinary D3D values.
pub(super) unsafe extern "system" fn sampler_x(
    device: &Device, words: *const u32, handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    if words.is_null() { fail_call(format_args!("null extended sampler")); }
    let w = unsafe { core::slice::from_raw_parts(words, 21) };
    let comparison = w[16] != D3D12_COMPARISON_FUNC_NEVER.0 as u32 && w[16] != 0;
    let reduction = if comparison { 1 } else { match w[5] { 0 => 0, 1 => 2, 2 => 3, other => fail_call(format_args!("unsupported sampler reduction {other}")) } };
    let linear = |filter: u32| match filter { 0 | 1 => 0, 2 => 1, other => fail_call(format_args!("unsupported sampler filter {other}")) };
    let bits = if w[1] == 3 && w[2] == 3 { 0x55 } else {
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
    unsafe { sampler(device, &host, handle) };
}
