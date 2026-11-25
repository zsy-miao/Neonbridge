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
        DXGI_FORMAT_B5G5R5A1_UNORM if translated && stood_in(format) => DXGI_FORMAT_R8G8B8A8_UNORM,
        DXGI_FORMAT_R9G9B9E5_SHAREDEXP if translated && rgb9e5_targets() => DXGI_FORMAT_R16G16B16A16_FLOAT,
        other => host_format(other),
    }
}

/// R9G9B9E5_SHAREDEXP, which desktop D3D12 never allows as a render target
/// or unordered access, is backed by R16G16B16A16_FLOAT when the title asks
/// for either use: half floats hold every RGB9E5 value with 10 mantissa bits
/// for its 9. In Series S mode the title's FidelityFX upscale output is a
/// 1920x1080 RGB9E5 target with unordered access; made in its own format the
/// texture was refused and its render target view removed the device.
/// VIctory-1984 3a55abd (GPL-3.0, PR #492's rule), with credit. Shortcut: a
/// view of the stand-in reads the alpha written there, where RGB9E5 reads 1.
/// Sampled-only RGB9E5 images (GTA's views of its R32_UINT colour) keep their
/// format. TUXBOX_RGB9E5_TARGETS=0 off.
fn rgb9e5_targets() -> bool {
    crate::env_toggle!("TUXBOX_RGB9E5_TARGETS") != Some("0")
}

/// TUXBOX_TARGET_STAND_INS (default on, =0 off): B5G5R5A1 (86) rendered to
/// or written as unordered access gets an R8G8B8A8_UNORM host image when this
/// host refuses either use, and B4G4R4A4 (115) gets one for unordered access
/// too (it already did for render targets). Copied from VIctory-fxceless
/// victory-gpu resource.rs target_format (9376fd6, GPL-3.0, credit to its
/// authors): NVIDIA refuses both uses of both formats, so the whole texture was
/// refused and every pass that wrote it drew nothing; 8-bit RGBA holds the same
/// channels in the same order, every 4-bit level exact and every 5-bit level
/// within 1/510. Their shortcut is kept: copies between such a target and a
/// texture the title fills in the 16-bit format are not converted.
fn stand_ins() -> bool {
    crate::env_toggle!("TUXBOX_TARGET_STAND_INS") != Some("0")
}

static REFUSED: std::sync::OnceLock<[bool; 2]> = std::sync::OnceLock::new();

/// Asks the device, once, whether it renders to and writes 86 and 115.
pub(crate) fn note_device(device: &ID3D12Device) {
    REFUSED.get_or_init(|| {
        [DXGI_FORMAT_B5G5R5A1_UNORM, DXGI_FORMAT_B4G4R4A4_UNORM].map(|f| {
            let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT { Format: f, ..Default::default() };
            let asked = unsafe {
                device.CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT, (&raw mut support).cast(), size_of_val(&support) as u32)
            };
            let both = D3D12_FORMAT_SUPPORT1_RENDER_TARGET.0 | D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW.0;
            let refused = asked.is_err() || support.Support1.0 & both != both;
            if refused {
                crate::diagnostics::log_line(format_args!(
                    "format {}: this host does not both render to it and write it as unordered access; R8G8B8A8_UNORM stands in for such images (TUXBOX_TARGET_STAND_INS=0 off)", f.0));
            }
            refused
        })
    });
}

/// Whether `format` (86 or 115) gets the RGBA8 stand-in as a target.
pub(super) fn stood_in(format: DXGI_FORMAT) -> bool {
    let i = match format {
        DXGI_FORMAT_B5G5R5A1_UNORM => 0,
        DXGI_FORMAT_B4G4R4A4_UNORM => 1,
        _ => return false,
    };
    stand_ins() && REFUSED.get().is_some_and(|r| r[i])
}

/// The host format of a title texture in `format` that is rendered to or
/// written (fx resource.rs target_format): the RGBA8 stand-in where [`stood_in`].
pub(crate) fn stand_in_format(format: u32) -> u32 {
    if stood_in(DXGI_FORMAT(format as i32)) { DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32 } else { format }
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

/// TUXBOX_SRGB_UAV_TYPELESS (default on, =0 off): the host has no sRGB
/// unordered access, so an sRGB texture with unordered access is made
/// typeless (29->27, 91->90, 93->92); its UAVs name the linear twin, its
/// other views the title's sRGB format. Copied from VIctory-fxceless
/// victory-gpu resource.rs host description ("The host has no sRGB unordered
/// access: such textures are typeless") and SRGB (GPL-3.0, credit to its
/// authors). test186: a typed 2560x1440 R8G8B8A8_UNORM_SRGB texture with
/// unordered access had its UAV refused and the device removed.
pub(crate) fn srgb_uav_typeless(desc: &D3D12_RESOURCE_DESC) -> Option<DXGI_FORMAT> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::env_toggle!("TUXBOX_SRGB_UAV_TYPELESS") != Some("0"))
        || desc.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0
    {
        return None;
    }
    match desc.Format {
        DXGI_FORMAT_R8G8B8A8_UNORM_SRGB => Some(DXGI_FORMAT_R8G8B8A8_TYPELESS),
        DXGI_FORMAT_B8G8R8A8_UNORM_SRGB => Some(DXGI_FORMAT_B8G8R8A8_TYPELESS),
        DXGI_FORMAT_B8G8R8X8_UNORM_SRGB => Some(DXGI_FORMAT_B8G8R8X8_TYPELESS),
        _ => None,
    }
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
        DXGI_FORMAT_B4G4R4A4_UNORM | DXGI_FORMAT_B5G5R5A1_UNORM
            if stood_in(desc.Format)
                && flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0) != 0 =>
        {
            (DXGI_FORMAT_R8G8B8A8_UNORM, flags)
        }
        DXGI_FORMAT_R9G9B9E5_SHAREDEXP
            if rgb9e5_targets()
                && flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0) != 0 =>
        {
            (DXGI_FORMAT_R16G16B16A16_FLOAT, flags)
        }
        // Scarlett depth can also allow unordered access; the desktop image
        // keeps its format and depth usage. A UAV on it is reported at view creation.
        format if flags & D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0 != 0
            && flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0 => {
            (format, flags & !D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0)
        }
        format if let Some(typeless) = srgb_uav_typeless(desc) => {
            let _ = format;
            static ONCE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&ONCE, || format!(
                "sRGB texture (format {}) with unordered access made typeless (TUXBOX_SRGB_UAV_TYPELESS=0 off)", desc.Format.0));
            (typeless, flags)
        }
        _ => return None,
    };
    image.then_some(D3D12_RESOURCE_DESC { Format: format, Flags: D3D12_RESOURCE_FLAGS(flags), ..*desc })
}

/// True when the guest asked for a depth image with unordered access that the
/// host creates without it (everything except the paired float depth plane).
fn unbacked_uav(desc: &D3D12_RESOURCE_DESC) -> bool {
    let dropped = desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER
        && desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)
        && crate::env_toggle!("TUXBOX_DS_UAV_DROP") != Some("0");
    is_depth(desc) && (substitute(desc).is_some() || dropped) && !super::depth::paired(desc)
}

/// Initial state of a resource whose unordered access the host does not back.
pub(super) fn host_initial_state(desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES) -> D3D12_RESOURCE_STATES {
    if unbacked_uav(desc) && state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS {
        D3D12_RESOURCE_STATE_COMMON
    } else if state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS && super::depth::twins() && super::depth::paired(desc) {
        // TUXBOX_DEPTH_TWINS: a depth texture with a twin rests in COMMON,
        // both planes, while the title has it in unordered access (the twin
        // holds the writes; commands/depth.rs handoff).
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
    // test159: a 768x768x6 R16 depth cube placed in UNORDERED_ACCESS was
    // refused E_NOTIMPL (heap.rs place_gpu_target) and the title hit its int3
    // at game+0x14a6390 on the null resource. host_initial_state already rests
    // a twinned depth texture in COMMON while its twin holds the writes; the
    // barrier-layout path now does the same. TUXBOX_TWIN_UAV_LAYOUT=0 refuses.
    if unbacked_uav(desc)
        || (super::depth::twins() && super::depth::paired(desc)
            && crate::env_toggle!("TUXBOX_TWIN_UAV_LAYOUT") != Some("0"))
    { host_layout(layout) } else { layout }
}
