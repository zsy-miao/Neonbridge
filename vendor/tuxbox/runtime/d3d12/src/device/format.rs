//! Scarlett image formats the desktop adapter cannot create as requested.
//! The host image uses a compatible format family; views, clear values and
//! pipeline target formats are translated to match. Copies see host texels.
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};

// Desktop D3D12 has no 16-bit depth with stencil. D24S8 also stores unsigned
// normalized depth, and its extra precision does not change test results.
pub(crate) const D16_UNORM_S8_UINT: DXGI_FORMAT = DXGI_FORMAT(118);
const R16_UNORM_X8_TYPELESS: DXGI_FORMAT = DXGI_FORMAT(119);
const X16_TYPELESS_G8_UINT: DXGI_FORMAT = DXGI_FORMAT(120);

/// Host format for a pipeline target, clear value or a view of any resource.
/// A 4-bit-per-channel target is always backed by RGBA8, which renders and
/// samples the same normalized channels.
pub(crate) fn host_format(format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        D16_UNORM_S8_UINT => DXGI_FORMAT_D24_UNORM_S8_UINT,
        R16_UNORM_X8_TYPELESS => DXGI_FORMAT_R24_UNORM_X8_TYPELESS,
        X16_TYPELESS_G8_UINT => DXGI_FORMAT_X24_TYPELESS_G8_UINT,
        other => other,
    }
}

/// View format for a resource. Formats the host can sample (B4G4R4A4) change
/// only when that resource was created in a substitute family.
pub(super) fn view_format(translated: bool, format: DXGI_FORMAT) -> DXGI_FORMAT {
    match format {
        DXGI_FORMAT_B4G4R4A4_UNORM if translated => DXGI_FORMAT_R8G8B8A8_UNORM,
        other => host_format(other),
    }
}

fn target_format(format: DXGI_FORMAT) -> DXGI_FORMAT {
    view_format(true, format)
}

/// Translated optimized clear value. The pointer stays guest-owned.
pub(super) fn host_clear(clear: *const D3D12_CLEAR_VALUE) -> Option<D3D12_CLEAR_VALUE> {
    unsafe { clear.as_ref() }.map(|c| D3D12_CLEAR_VALUE { Format: target_format(c.Format), ..*c })
}

/// Pipeline render-target and depth formats, translated in place.
pub(super) fn host_targets(formats: &mut [DXGI_FORMAT], depth: &mut DXGI_FORMAT) {
    for format in formats {
        *format = target_format(*format);
    }
    *depth = target_format(*depth);
}

/// Desktop resource flags end at the ray-tracing structure bit (0x100). The
/// console's metadata and compression bits above it have no host meaning.
pub(super) const HOST_FLAGS: i32 = 0x1ff;

pub(crate) fn is_depth(guest: &D3D12_RESOURCE_DESC) -> bool {
    guest.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL)
}

/// Host description for a guest image in a substitute format family.
pub(super) fn substitute(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    let flags = desc.Flags.0 & HOST_FLAGS;
    let image = matches!(desc.Dimension, D3D12_RESOURCE_DIMENSION_TEXTURE1D | D3D12_RESOURCE_DIMENSION_TEXTURE2D)
        && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN;
    let (format, flags) = match desc.Format {
        // Typeless storage lets the depth, stencil and SRV plane views share it.
        // Desktop depth images cannot also allow unordered access.
        D16_UNORM_S8_UINT if flags & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0 => {
            (DXGI_FORMAT_R24G8_TYPELESS, flags & !D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0)
        }
        DXGI_FORMAT_B4G4R4A4_UNORM if flags & D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 != 0 => {
            (DXGI_FORMAT_R8G8B8A8_UNORM, flags)
        }
        // Scarlett depth can also allow unordered access; the desktop image
        // keeps its format and depth usage. A UAV on it is reported at view creation.
        format if flags & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0
            && flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0 => {
            (format, flags & !D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0)
        }
        _ => return None,
    };
    image.then_some(D3D12_RESOURCE_DESC { Format: format, Flags: D3D12_RESOURCE_FLAGS(flags), ..*desc })
}

/// True when the guest asked for a depth image with unordered access that the
/// host creates without it (everything except the paired float depth plane).
fn unbacked_uav(desc: &D3D12_RESOURCE_DESC) -> bool {
    is_depth(desc) && substitute(desc).is_some() && !super::depth::paired(desc)
}

/// Initial state of a resource whose unordered access the host does not back.
pub(super) fn host_initial_state(desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_STATES {
    if unbacked_uav(desc) && state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS {
        D3D12_RESOURCE_STATE_COMMON
    } else {
        state
    }
}

/// Layout equivalent of [`host_initial_state`], also used for barriers.
pub(crate) fn host_layout(layout: D3D12_BARRIER_LAYOUT) -> D3D12_BARRIER_LAYOUT {
    match layout {
        D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS
        | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS
        | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS => D3D12_BARRIER_LAYOUT_COMMON,
        other => other,
    }
}

pub(super) fn host_initial_layout(desc: &D3D12_RESOURCE_DESC, layout: D3D12_BARRIER_LAYOUT) -> D3D12_BARRIER_LAYOUT {
    if unbacked_uav(desc) { host_layout(layout) } else { layout }
}
