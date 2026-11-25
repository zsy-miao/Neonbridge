//! NV12 chroma addresses are aliases into decoder-owned GPU textures. Associate
//! a placement with a live luma copy on this device before exposing plane 1.
use super::{Device, Out, resource, wrap_pageable};
use crate::proxy::{Proxy, Slots, child_slots, unwrap};
use core::ffi::c_void;
use std::sync::{Arc, Mutex, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_OUTOFMEMORY};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::{HRESULT, IUnknown, Interface, Result};

pub(crate) const DMA_IID: windows::core::GUID =
    windows::core::GUID::from_u128(0x30bc77b9_8379_4b25_95de_b8588c42275d);

#[derive(Default)]
pub(crate) struct Sources(Mutex<Vec<Weak<Plane>>>);

pub(crate) struct Plane {
    pub(crate) source: ID3D12Resource,
    pub(crate) address: u64,
    pub(crate) pitch: u64,
    pub(crate) height: u32,
}

pub(crate) unsafe fn observe(owner: *mut c_void, source: ID3D12Resource) -> Result<Arc<Plane>> {
    let desc = unsafe { source.GetDesc() };
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || desc.Format != DXGI_FORMAT_NV12
        || desc.DepthOrArraySize != 1
        || desc.MipLevels != 1
        || desc.SampleDesc.Count != 1
        || !desc.Width.is_multiple_of(2)
        || !desc.Height.is_multiple_of(2)
    {
        return Err(E_INVALIDARG.into());
    }
    let pitch = desc
        .Width
        .checked_next_multiple_of(256)
        .ok_or(E_INVALIDARG)?;
    let address = pitch
        .checked_mul(u64::from(desc.Height))
        .and_then(|n| n.checked_add(unsafe { source.GetGPUVirtualAddress() }))
        .ok_or(E_INVALIDARG)?;
    let plane = Arc::new(Plane {
        source,
        address,
        pitch,
        height: desc.Height,
    });
    let device = unsafe { &*owner.cast::<Device>() };
    let mut entries = device
        .state
        .media
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    entries.retain(|weak| weak.strong_count() != 0);
    entries.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
    entries.push(Arc::downgrade(&plane));
    Ok(plane)
}

struct Alias {
    desc: D3D12_RESOURCE_DESC,
    address: u64,
}
type AliasObject = Proxy<ID3D12Resource, Alias>;
unsafe extern "system" fn alias_desc(
    alias: &AliasObject,
    out: *mut D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_DESC {
    unsafe { out.write(alias.state.desc) };
    out
}
unsafe extern "system" fn alias_address(alias: &AliasObject) -> u64 {
    alias.state.address
}

static ALIAS: Slots<32> = {
    let mut slots = child_slots::<ID3D12Resource, Alias, 32>();
    slots[10] = crate::slot!(alias_desc);
    slots[11] = crate::slot!(alias_address);
    slots
};

pub(super) unsafe fn place(
    device: &Device,
    address: u64,
    desc: &D3D12_RESOURCE_DESC,
    state: D3D12_RESOURCE_STATES,
    out: Out,
) -> Option<HRESULT> {
    if desc.Format != DXGI_FORMAT_R8G8_UNORM
        || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || desc.DepthOrArraySize != 1
        || desc.MipLevels != 1
        || desc.SampleDesc.Count != 1
        || desc.SampleDesc.Quality != 0
        || desc.Flags != D3D12_RESOURCE_FLAG_NONE
        || state != D3D12_RESOURCE_STATE_COPY_SOURCE
    {
        return None;
    }
    let entries = device
        .state
        .media
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let plane = entries.iter().filter_map(Weak::upgrade).find(|p| {
        p.address == address && p.pitch / 2 == desc.Width && p.height / 2 == desc.Height
    })?;
    // The compact list disambiguates simultaneous decoder images at the same
    // console address using the preceding luma command in its own recording.
    let native = plane.source.clone();
    Some(unsafe {
        wrap_pageable(
            Ok(native),
            &ALIAS,
            device,
            Alias {
                desc: *desc,
                address,
            },
            out,
        )
    })
}

pub(crate) unsafe fn alias(
    object: *mut c_void,
    owner: *mut c_void,
) -> Option<(u64, D3D12_RESOURCE_DESC)> {
    let alias: &AliasObject = unsafe { unwrap(object, owner, &ALIAS) }?;
    Some((alias.state.address, alias.state.desc))
}

pub(crate) unsafe fn native(object: *mut c_void, owner: *mut c_void) -> Result<ID3D12Resource> {
    if let Some(resource) = unsafe { resource(object, owner) } {
        return Ok(resource.clone());
    }
    unsafe { native_child(object, owner) }
}

// Media samples can carry host resources and synchronization fences. A guest
// proxy from a different device returns a guest parent and fails this check.
pub(crate) unsafe fn native_child<I: Interface>(
    object: *mut c_void,
    owner: *mut c_void,
) -> Result<I>
where
    for<'a> &'a I: Into<&'a ID3D12DeviceChild>,
{
    let borrowed = unsafe { IUnknown::from_raw_borrowed(&object) }.ok_or(E_INVALIDARG)?;
    let native: I = borrowed.cast().map_err(|_| E_INVALIDARG)?;
    let child: &ID3D12DeviceChild = (&native).into();
    let mut actual: Option<IUnknown> = None;
    unsafe { child.GetDevice(&mut actual) }.map_err(|_| E_INVALIDARG)?;
    let device = unsafe { &*owner.cast::<Device>() };
    if actual.as_ref() != Some(&device.native.cast()?) {
        return Err(E_INVALIDARG.into());
    }
    Ok(native)
}
