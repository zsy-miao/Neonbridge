//! A Scarlett depth texture can expose its depth plane as a writable UAV. On
//! desktop D3D12 the depth/stencil image and writable R32 image must be distinct.
use super::{Backing, Device, Out, RESOURCE, wrap_pageable};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::{HRESULT, Result};

/// The float depth plane needs an R32 companion image for unordered access.
pub(super) fn paired(guest: &D3D12_RESOURCE_DESC) -> bool {
    pair_primary(guest).is_some()
}

/// Host description for a guest image the desktop cannot create as described.
pub(super) fn primary(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    pair_primary(desc).or_else(|| super::format::substitute(desc))
}

fn pair_primary(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    let writable = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0;
    (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && (desc.Format == DXGI_FORMAT_R32G8X24_TYPELESS
            && matches!(desc.Flags.0 as u32, 0x0100_0006 | 0x8030_0006)
            || desc.Format == super::format::D16_UNORM_S8_UINT && desc.Flags.0 & super::format::HOST_FLAGS == writable)
        && desc.MipLevels == 1
        && desc.DepthOrArraySize == 1
        && desc.SampleDesc.Count == 1
        && desc.SampleDesc.Quality == 0
        && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN)
        .then_some(D3D12_RESOURCE_DESC {
            Format: DXGI_FORMAT_R32G8X24_TYPELESS,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            ..*desc
        })
}

pub(crate) struct Pair {
    pub(crate) guest: D3D12_RESOURCE_DESC,
    pub(crate) image: ID3D12Resource,
    pub(crate) transfer: ID3D12Resource,
    pub(crate) depth_plane: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    pub(crate) image_plane: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
}

fn footprint(
    device: &ID3D12Device,
    desc: &D3D12_RESOURCE_DESC,
    format: DXGI_FORMAT,
) -> Result<D3D12_PLACED_SUBRESOURCE_FOOTPRINT> {
    let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let (mut rows, mut row_bytes, mut bytes) = (0, 0, 0);
    unsafe {
        device.GetCopyableFootprints(
            desc,
            0,
            1,
            0,
            Some(&mut layout),
            Some(&mut rows),
            Some(&mut row_bytes),
            Some(&mut bytes),
        )
    };
    if bytes == u64::MAX
        || layout.Footprint.Format != format
        || layout.Footprint.Depth != 1
        || u64::from(layout.Footprint.Width) != desc.Width
        || rows != desc.Height
        || layout.Footprint.Height != rows
        || row_bytes != desc.Width * 4
    {
        return Err(E_INVALIDARG.into());
    }
    Ok(layout)
}

impl Pair {
    pub(super) fn new(
        device: &ID3D12Device,
        heap: &D3D12_HEAP_PROPERTIES,
        guest: &D3D12_RESOURCE_DESC,
        primary: &D3D12_RESOURCE_DESC,
    ) -> Result<Self> {
        let companion = D3D12_RESOURCE_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..*primary
        };
        let mut depth_plane = footprint(device, primary, DXGI_FORMAT_R32_TYPELESS)?;
        let mut image_plane = footprint(device, &companion, DXGI_FORMAT_R32_FLOAT)?;
        let pitch = depth_plane
            .Footprint
            .RowPitch
            .max(image_plane.Footprint.RowPitch);
        depth_plane.Footprint.RowPitch = pitch;
        image_plane.Footprint.RowPitch = pitch;
        let staging = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: u64::from(pitch) * u64::from(guest.Height),
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let allocate = |desc: &D3D12_RESOURCE_DESC, state| {
            let mut result = None;
            unsafe {
                device.CreateCommittedResource(
                    heap,
                    D3D12_HEAP_FLAG_NONE,
                    desc,
                    state,
                    None,
                    &mut result,
                )?
            };
            result.ok_or_else(|| windows::core::Error::from(E_POINTER))
        };
        Ok(Self {
            guest: *guest,
            image: allocate(&companion, D3D12_RESOURCE_STATE_UNORDERED_ACCESS)?,
            transfer: allocate(&staging, D3D12_RESOURCE_STATE_COPY_DEST)?,
            depth_plane,
            image_plane,
        })
    }
}

pub(super) unsafe fn wrap<const CONSOLE_LAYOUT: bool>(
    device: &Device,
    heap: &D3D12_HEAP_PROPERTIES,
    guest: &D3D12_RESOURCE_DESC,
    primary: Option<&D3D12_RESOURCE_DESC>,
    initial: D3D12_RESOURCE_STATES,
    native: Option<ID3D12Resource>,
    out: Out,
) -> HRESULT {
    let Some(native) = native else {
        return E_POINTER;
    };
    let backing = match primary {
        None if CONSOLE_LAYOUT => Backing::ConsoleLayout(guest.Layout, guest.Flags),
        None => Backing::Native,
        Some(_) if !paired(guest) => Backing::Translated(*guest),
        Some(host_desc) => match Pair::new(&device.native, heap, guest, host_desc) {
            Ok(pair) => Backing::Depth(Box::new(pair)),
            Err(error) => return error.code(),
        },
    };
    let mut state = super::ResourceState::from(backing);
    state.blas_buffer = device
        .state
        .blas
        .register(&native, heap.Type, guest, initial);
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device, state, out) }
}
