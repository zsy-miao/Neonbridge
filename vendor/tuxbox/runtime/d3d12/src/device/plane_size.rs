//! Console allocation sizes for depth with stencil and multisampled colour
//! (Xbox2Windows b316ad12 + 992116ec, console_plane_size.h, MIT). Desktop
//! answers are larger: R32G8X24 takes 8 bytes a texel against the console's
//! 4-byte depth plane plus 1-byte stencil plane, and multisampled textures
//! align to 4 MiB against 64 KiB. The title carves these from a fixed pool
//! sized by the answer. Every plane is padded to whole 64 KiB tiles.
//! TUXBOX_CONSOLE_PLANE_SIZES=0 leaves these families to the other rules.
use windows::Win32::Graphics::Direct3D12::*;

pub(super) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_CONSOLE_PLANE_SIZES") != Some("0")
}

/// One plane of width x height texels and `bytes` bytes of samples a texel
/// (a power of two up to 64 KiB), padded to 64 KiB tiles.
fn plane_bytes(width: u64, height: u64, bytes: u64, slices: u64) -> u64 {
    let texels_log2 = 16 - bytes.trailing_zeros();
    let (tile_width, tile_height) = (1u64 << texels_log2.div_ceil(2), 1u64 << (texels_log2 / 2));
    (width.next_multiple_of(tile_width) * height.next_multiple_of(tile_height) * bytes * slices).next_multiple_of(0x10000)
}

/// Texel bytes of the colour formats answered when multisampled.
fn colour_texel_bytes(format: i32) -> u64 {
    match format {
        9..=14 => 8,
        23..=32 | 87..=93 => 4,
        _ => 0,
    }
}

/// A 2D texture of one mip within the D3D12 limits, 1/2/4/8 samples at
/// quality 0, the default layout and the default or 64 KiB alignment.
fn shape(desc: &D3D12_RESOURCE_DESC) -> bool {
    desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D && desc.MipLevels == 1
        && (1..=u64::from(D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION)).contains(&desc.Width)
        && (1..=D3D12_REQ_TEXTURE2D_U_OR_V_DIMENSION).contains(&desc.Height)
        && (1..=D3D12_REQ_TEXTURE2D_ARRAY_AXIS_DIMENSION as u16).contains(&desc.DepthOrArraySize)
        && matches!(desc.SampleDesc.Count, 1 | 2 | 4 | 8) && desc.SampleDesc.Quality == 0
        && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN && matches!(desc.Alignment, 0 | 0x10000)
}

/// The console size of a description in these families, else None. Bounded
/// by the shape: below 2^45, no overflow.
pub(super) fn console_size(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    if !shape(desc) {
        return None;
    }
    let (w, h) = (desc.Width, u64::from(desc.Height));
    let (samples, slices) = (u64::from(desc.SampleDesc.Count), u64::from(desc.DepthOrArraySize));
    let size = match desc.Format.0 {
        19..=22 => plane_bytes(w, h, 4 * samples, slices) + plane_bytes(w, h, samples, slices),
        118..=120 => plane_bytes(w, h, 2 * samples, slices) + plane_bytes(w, h, samples, slices),
        f if samples > 1 && colour_texel_bytes(f) != 0 => plane_bytes(w, h, colour_texel_bytes(f) * samples, slices),
        _ => return None,
    };
    Some(D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: size, Alignment: 0x10000 })
}

/// Formats 118-120 have no desktop form, so their shape is answered over a
/// desktop failure; the other formats keep a desktop failure.
pub(super) fn answers_over_failure(desc: &D3D12_RESOURCE_DESC) -> bool {
    matches!(desc.Format.0, 118..=120)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};

    fn tex(w: u64, h: u32, format: i32, samples: u32) -> D3D12_RESOURCE_DESC {
        D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: w, Height: h, DepthOrArraySize: 1, MipLevels: 1,
            Format: DXGI_FORMAT(format), SampleDesc: DXGI_SAMPLE_DESC { Count: samples, Quality: 0 }, ..Default::default()
        }
    }

    #[test]
    fn tiles_follow_texel_bytes() {
        // 128x128 at 4 bytes, 256x256 at 1 byte, 64x64 at 16 bytes: one tile each.
        assert_eq!(plane_bytes(128, 128, 4, 1), 0x10000);
        assert_eq!(plane_bytes(256, 256, 1, 1), 0x10000);
        assert_eq!(plane_bytes(64, 64, 16, 1), 0x10000);
        assert_eq!(plane_bytes(129, 1, 4, 1), 0x20000);
    }

    #[test]
    fn depth_stencil_is_depth_plus_stencil_planes() {
        let size = console_size(&tex(1920, 1080, 20, 1)).unwrap();
        assert_eq!(size.SizeInBytes, plane_bytes(1920, 1080, 4, 1) + plane_bytes(1920, 1080, 1, 1));
        assert_eq!(size.Alignment, 0x10000);
        assert!(console_size(&tex(1920, 1080, 119, 4)).is_some());
    }

    #[test]
    fn msaa_colour_only_when_multisampled() {
        assert!(console_size(&tex(1920, 1080, 28, 1)).is_none());
        assert_eq!(console_size(&tex(1920, 1080, 28, 4)).unwrap().SizeInBytes, plane_bytes(1920, 1080, 16, 1));
    }

    #[test]
    fn shape_is_validated() {
        let mut d = tex(1920, 1080, 20, 1);
        d.MipLevels = 2;
        assert!(console_size(&d).is_none());
        let mut d = tex(u64::MAX - 10, 1080, 20, 1);
        assert!(console_size(&d).is_none());
        d = tex(64, 64, 28, 4);
        d.SampleDesc.Quality = 1;
        assert!(console_size(&d).is_none());
        d = tex(64, 64, 28, 3);
        assert!(console_size(&d).is_none());
    }
}
