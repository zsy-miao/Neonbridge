//! A Scarlett depth texture can expose its float plane as a writable UAV. On
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

/// TUXBOX_DEPTH_TWINS=1: depth textures written as unordered access get a
/// colour twin in every family fxceless gives one (VIctory-fxceless
/// victory-gpu resource.rs:322-570 Twin, GPL-3.0, credited): 32-bit float
/// depth with or without stencil and 16-bit depth, every mip and slice, the
/// twin 16 bits a texel for 16-bit depth and a 32-bit float otherwise. Ours
/// stays barrier-driven (a handoff copy at each transition into or out of
/// unordered access) where fxceless tracks which side is behind. Not ported:
/// fxceless's R8 twin of a stencil plane (stencil views stay null) and its
/// pass that draws a 24-bit depth plane from a twin (24-bit depth is not
/// paired). Off: only one-mip, one-slice R32G8X24 textures with the two
/// console flag words seen first, as before.
pub(crate) fn twins() -> bool {
    crate::env_toggle!("TUXBOX_DEPTH_TWINS") == Some("1")
}

fn pair_primary(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    let common = desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.SampleDesc.Count == 1
        && desc.SampleDesc.Quality == 0
        && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN;
    let paired = if twins() {
        // Both the depth-stencil and the unordered-access bit (0x2 | 0x4).
        common
            && matches!(desc.Format, DXGI_FORMAT_R32G8X24_TYPELESS | DXGI_FORMAT_R32_TYPELESS | DXGI_FORMAT_R16_TYPELESS)
            && desc.Flags.0 as u32 & 0x6 == 0x6
            && desc.MipLevels >= 1
            && desc.DepthOrArraySize >= 1
    } else {
        common
            && desc.Format == DXGI_FORMAT_R32G8X24_TYPELESS
            && matches!(desc.Flags.0 as u32, 0x0100_0006 | 0x8030_0006)
            && desc.MipLevels == 1
            && desc.DepthOrArraySize == 1
    };
    paired.then_some(D3D12_RESOURCE_DESC {
        Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ..*desc
    })
}

pub(crate) struct Pair {
    pub(crate) guest: D3D12_RESOURCE_DESC,
    pub(crate) image: ID3D12Resource,
    pub(crate) transfer: ID3D12Resource,
    /// Per subresource of the depth plane (mip + slice * mips): its layout in
    /// the transfer buffer as the depth plane's copy and as the twin's.
    pub(crate) planes: Vec<(D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)>,
    /// The depth texture has a stencil plane, its subresources after the
    /// depth plane's.
    pub(crate) stencil: bool,
    /// The twin's texels are 16 bits (16-bit depth).
    pub(crate) narrow: bool,
}

impl Pair {
    /// Subresources of the depth plane.
    pub(crate) fn count(&self) -> u32 {
        self.planes.len() as u32
    }
}

fn footprints(
    device: &ID3D12Device,
    desc: &D3D12_RESOURCE_DESC,
    count: u32,
    texel: u64,
) -> Result<Vec<(D3D12_PLACED_SUBRESOURCE_FOOTPRINT, u32)>> {
    let n = count as usize;
    let mut layouts = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); n];
    let mut rows = vec![0u32; n];
    let mut row_bytes = vec![0u64; n];
    let mut bytes = 0;
    unsafe {
        device.GetCopyableFootprints(
            desc,
            0,
            count,
            0,
            Some(layouts.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            Some(row_bytes.as_mut_ptr()),
            Some(&mut bytes),
        )
    };
    if bytes == u64::MAX {
        return Err(E_INVALIDARG.into());
    }
    let mips = u32::from(desc.MipLevels.max(1));
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let mip = i as u32 % mips;
        let width = (desc.Width >> mip).max(1);
        let height = (desc.Height >> mip).max(1);
        let l = layouts[i];
        if l.Footprint.Depth != 1
            || u64::from(l.Footprint.Width) != width
            || rows[i] != height
            || l.Footprint.Height != rows[i]
            || row_bytes[i] != width * texel
        {
            return Err(E_INVALIDARG.into());
        }
        out.push((l, rows[i]));
    }
    Ok(out)
}

impl Pair {
    pub(super) fn new(
        device: &ID3D12Device,
        heap: &D3D12_HEAP_PROPERTIES,
        guest: &D3D12_RESOURCE_DESC,
        primary: &D3D12_RESOURCE_DESC,
    ) -> Result<Self> {
        let narrow = primary.Format == DXGI_FORMAT_R16_TYPELESS;
        let stencil = primary.Format == DXGI_FORMAT_R32G8X24_TYPELESS;
        let companion = D3D12_RESOURCE_DESC {
            Format: if narrow { DXGI_FORMAT_R16_TYPELESS } else { DXGI_FORMAT_R32_FLOAT },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..*primary
        };
        let texel = if narrow { 2 } else { 4 };
        let count = u32::from(primary.MipLevels.max(1)) * u32::from(primary.DepthOrArraySize.max(1));
        let depth = footprints(device, primary, count, texel)?;
        let image = footprints(device, &companion, count, texel)?;
        // One pitch per subresource for both sides, packed at 512 bytes.
        let mut planes = Vec::with_capacity(depth.len());
        let mut offset = 0u64;
        for ((mut d, rows), (mut c, _)) in depth.into_iter().zip(image) {
            let pitch = d.Footprint.RowPitch.max(c.Footprint.RowPitch);
            offset = offset.next_multiple_of(u64::from(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT));
            d.Footprint.RowPitch = pitch;
            c.Footprint.RowPitch = pitch;
            d.Offset = offset;
            c.Offset = offset;
            offset += u64::from(pitch) * u64::from(rows);
            planes.push((d, c));
        }
        let staging = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: offset,
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
            planes,
            stencil,
            narrow,
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
