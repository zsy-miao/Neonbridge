//! TUXBOX_VIEW_FITS (default on, =0 passes every texture view on): a texture
//! SRV or UAV the host would refuse (and remove the device for) is written
//! null, and one of a typed texture in another format of its family takes the
//! texture's format. The console allows any view; the host refuses views
//! without the resource's permission, of typed depth, of a typeless colour
//! texture with no format, and in a format of another family. Copied from
//! VIctory-fxceless victory-gpu descriptor/host.rs fits and resource.rs
//! family / SRGB / DEPTH_FORMATS (GPL-3.0, credit to its authors); its
//! stand-in target branch (resource.rs target_format) uses format.rs stand_in_format.
//! shortcut (theirs): such a view (sRGB of UNORM, say) reads without its
//! conversion.

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_VIEW_FITS") != Some("0"))
}

/// The typeless format of a format's family (itself if it has none).
fn family(format: u32) -> u32 {
    const FAMILIES: [(u32, u32); 22] = [
        (1, 4),
        (5, 8),
        (9, 14),
        (15, 18),
        (19, 22),
        (23, 25),
        (27, 32),
        (33, 38),
        (39, 43),
        (44, 47),
        (48, 52),
        (53, 59),
        (60, 65),
        (70, 72),
        (73, 75),
        (76, 78),
        (79, 81),
        (82, 84),
        (90, 91),
        (92, 93),
        (94, 96),
        (97, 99),
    ];
    match format {
        87 => 90,
        88 => 92,
        f => FAMILIES
            .iter()
            .find(|&&(first, last)| (first..=last).contains(&f))
            .map_or(f, |&(first, _)| first),
    }
}

/// sRGB formats and their linear twins.
const SRGB: [(u32, u32); 3] = [(29, 28), (91, 87), (93, 88)];

/// Typed depth formats and their typeless families.
const DEPTH_FORMATS: [(u32, u32); 4] = [(20, 19), (40, 39), (45, 44), (55, 53)];

/// Whether the host can make a shader (or unordered access) view of
/// `texture` in `format` (none: the texture's own); a view of a typed
/// texture in another format of its family takes the texture's format.
fn fits(texture: &ID3D12Resource, mut format: Option<&mut DXGI_FORMAT>, unordered: bool) -> bool {
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let allowed = if unordered {
        d.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 != 0
    } else {
        d.Flags.0 & D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE.0 == 0
    };
    let own = d.Format.0 as u32;
    if !allowed || DEPTH_FORMATS.iter().any(|&(typed, _)| typed == own) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&ONCE, || format!(
            "view the host does not allow (unordered {unordered}, format {own}, flags {:#x}): written null", d.Flags.0));
        return false;
    }
    // Every family has more than one format, its typeless one first.
    let typeless = family(own) == own && family(own + 1) == own;
    if let Some(format) = format.as_deref_mut()
        && unordered
        && let Some(&(_, linear)) = SRGB.iter().find(|&&(s, _)| s == format.0 as u32)
    {
        format.0 = linear as i32;
    }
    let view = format.as_ref().map_or(0, |f| f.0 as u32);
    if view == 0 {
        return !typeless;
    }
    // A view of a target whose format the host refuses, in the title's
    // format: in the stand-in the host copy has (fx fits, TUXBOX_TARGET_STAND_INS).
    if view != own && crate::device::format::stand_in_format(view) == own {
        if let Some(format) = format {
            *format = d.Format;
        }
        return true;
    }
    if family(view) != family(own) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&ONCE, || format!("view format {view} of another family than {own}: written null"));
        return false;
    }
    if let Some(format) = format
        && !typeless
    {
        *format = d.Format;
    }
    true
}

/// A texture of `target` (not a buffer), or None.
fn texture(target: Option<&ID3D12Resource>) -> Option<(&ID3D12Resource, D3D12_RESOURCE_DESC)> {
    let target = target?;
    let d = unsafe { target.GetDesc() };
    (d.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER && d.Dimension != D3D12_RESOURCE_DIMENSION_UNKNOWN).then_some((target, d))
}

/// What to write for an SRV of `target`: None as asked; else (null, view).
pub(super) fn srv(target: Option<&ID3D12Resource>, view: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>)
    -> Option<(bool, D3D12_SHADER_RESOURCE_VIEW_DESC)> {
    if !enabled() {
        return None;
    }
    let (texture, d) = texture(target)?;
    if view.is_some_and(|v| matches!(v.ViewDimension, D3D12_SRV_DIMENSION_BUFFER | D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE)) {
        return None;
    }
    let mut format = view.map(|v| v.Format).filter(|f| f.0 != 0);
    let asked = format;
    if fits(texture, format.as_mut(), false) {
        return match (view, format) {
            (Some(v), Some(f)) if Some(f) != asked => Some((false, D3D12_SHADER_RESOURCE_VIEW_DESC { Format: f, ..*v })),
            _ => None,
        };
    }
    let mut null = view.copied().unwrap_or_else(|| whole_srv(&d));
    null.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    match null.ViewDimension {
        D3D12_SRV_DIMENSION_TEXTURE2D => null.Anonymous.Texture2D.PlaneSlice = 0,
        D3D12_SRV_DIMENSION_TEXTURE2DARRAY => null.Anonymous.Texture2DArray.PlaneSlice = 0,
        _ => {}
    }
    Some((true, null))
}

/// What to write for a UAV of `target`: None as asked; else (null, view).
pub(super) fn uav(target: Option<&ID3D12Resource>, view: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>)
    -> Option<(bool, D3D12_UNORDERED_ACCESS_VIEW_DESC)> {
    if !enabled() {
        return None;
    }
    let (texture, d) = texture(target)?;
    if view.is_some_and(|v| v.ViewDimension == D3D12_UAV_DIMENSION_BUFFER) {
        return None;
    }
    let mut format = view.map(|v| v.Format).filter(|f| f.0 != 0);
    let asked = format;
    if fits(texture, format.as_mut(), true) {
        return match (view, format) {
            (Some(v), Some(f)) if Some(f) != asked => Some((false, D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: f, ..*v })),
            _ => None,
        };
    }
    let mut null = view.copied().unwrap_or_else(|| whole_uav(&d));
    null.Format = DXGI_FORMAT_R32_UINT;
    match null.ViewDimension {
        D3D12_UAV_DIMENSION_TEXTURE2D => null.Anonymous.Texture2D.PlaneSlice = 0,
        D3D12_UAV_DIMENSION_TEXTURE2DARRAY => null.Anonymous.Texture2DArray.PlaneSlice = 0,
        _ => {}
    }
    Some((true, null))
}

pub(super) fn whole_srv(d: &D3D12_RESOURCE_DESC) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    let mut v = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        ..Default::default()
    };
    let arrayed = d.DepthOrArraySize > 1;
    match d.Dimension {
        D3D12_RESOURCE_DIMENSION_TEXTURE1D => {
            v.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE1D;
            v.Anonymous.Texture1D = D3D12_TEX1D_SRV { MipLevels: u32::MAX, ..Default::default() };
        }
        D3D12_RESOURCE_DIMENSION_TEXTURE3D => {
            v.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE3D;
            v.Anonymous.Texture3D = D3D12_TEX3D_SRV { MipLevels: u32::MAX, ..Default::default() };
        }
        _ if arrayed => {
            v.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2DARRAY;
            v.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_SRV {
                MipLevels: u32::MAX, ArraySize: u32::from(d.DepthOrArraySize), ..Default::default()
            };
        }
        _ => {
            v.ViewDimension = D3D12_SRV_DIMENSION_TEXTURE2D;
            v.Anonymous.Texture2D = D3D12_TEX2D_SRV { MipLevels: u32::MAX, ..Default::default() };
        }
    }
    v
}

pub(super) fn whole_uav(d: &D3D12_RESOURCE_DESC) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut v = D3D12_UNORDERED_ACCESS_VIEW_DESC::default();
    match d.Dimension {
        D3D12_RESOURCE_DIMENSION_TEXTURE1D => v.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1D,
        D3D12_RESOURCE_DIMENSION_TEXTURE3D => {
            v.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE3D;
            v.Anonymous.Texture3D = D3D12_TEX3D_UAV { WSize: u32::MAX, ..Default::default() };
        }
        _ if d.DepthOrArraySize > 1 => {
            v.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
            v.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV { ArraySize: u32::from(d.DepthOrArraySize), ..Default::default() };
        }
        _ => v.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D,
    }
    v
}
