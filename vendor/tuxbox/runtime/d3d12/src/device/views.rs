//! Standard Xbox resource views and samplers, device slots 17 through 22.
mod mirrors;
mod addressed;
mod creation;
pub(super) use mirrors::cbv;
pub(crate) use mirrors::snapshot_cbv_mirrors;
use mirrors::srv_mirror;
pub(super) use addressed::AddressSrv;
use addressed::buffer_at;
#[cfg(test)]
use addressed::earlier_page;
pub(crate) use addressed::command_buffer_address;
pub(crate) use addressed::LinearResource;
pub(crate) use addressed::linear_resource;
use addressed::rebase_view;
use addressed::PlacedView;
use addressed::placed_view;
pub(super) use addressed::address_srv;
pub(super) use addressed::AddressUav;
pub(super) use addressed::address_uav;
pub(super) use creation::srv;
pub(super) use creation::uav;
pub(super) use creation::rtv;
pub(super) use creation::dsv;
pub(super) use creation::sampler;
pub(super) use creation::sampler_x;

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

    /// An imported buffer whose pages were since remapped still shows the old
    /// pages. Lookups skip it so the next use imports the current mapping.
    fn is_stale(&self) -> bool {
        self.buffer_heap.as_ref().is_some_and(|heap| !heap.is_live())
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

fn desktop_cbv_bytes(guest: u32) -> u32 {
    if guest <= D3D12_REQ_CONSTANT_BUFFER_ELEMENT_COUNT * 16 {
        guest.next_multiple_of(D3D12_CONSTANT_BUFFER_DATA_PLACEMENT_ALIGNMENT)
    } else {
        guest
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

const _: () = assert!(std::mem::offset_of!(AddressSrv, address) == 40);

const _: () = assert!(
    std::mem::offset_of!(AddressUav, address) == 40
        && std::mem::offset_of!(AddressUav, counter_address) == 48
        && std::mem::offset_of!(AddressUav, memory_type) == 56
);

#[cfg(test)]
#[path = "../tests/device/views.rs"]
mod tests;
