//! Choosing tiling modes.
use super::*;

/// Select the Xbox swizzle mode for a resource description.
///
/// The supplied XG build reads an eighth flags argument and does not inspect
/// the dimensions or sample count for this selection.
#[unsafe(export_name = "XGComputeOptimalSwizzleMode")]
pub(super) extern "system" fn optimal_swizzle_mode(
    dimension: u32,
    format: u32,
    _width: u32,
    _height: u32,
    _depth_or_array: u32,
    _samples: u32,
    bind_flags: u32,
    resource_flags: u32,
) -> u32 {
    if dimension == 1 {
        return 0;
    }
    if bind_flags & 0x40 != 0 {
        let incompatible = has_stencil(format);
        return if resource_flags & 0x0800_8000 != 0 && incompatible {
            u32::MAX
        } else {
            24
        };
    }
    match dimension {
        3 | 4 if resource_flags & 0x0800_0000 != 0 => 10,
        3 | 4 if bind_flags & 0x20 != 0 => 27,
        3 | 4 => 5,
        _ => 0,
    }
}

/// Select an explicit Series resource layout. The destination may be the
/// layout field in the input description itself.
#[unsafe(export_name = "XGSuggestSwizzleMode")]
pub(super) unsafe extern "system" fn suggest_swizzle(
    desc: *const Desc,
    options: u32,
    layout: *mut u32,
) -> i32 {
    if desc.is_null() || layout.is_null() {
        return E_INVALIDARG;
    }
    let resource = unsafe { desc.read_unaligned() };
    if options != 0 {
        return E_NOTIMPL;
    }
    if !(1..=4).contains(&resource.dimension) || resource.width == 0 {
        return E_INVALIDARG;
    }
    let bind_flags = (resource.flags & 3) << 5;
    let swizzle = optimal_swizzle_mode(resource.dimension, resource.format, 0, 0, 0,
        resource.samples, bind_flags, resource.flags);
    if swizzle == u32::MAX {
        return E_INVALIDARG;
    }
    unsafe { layout.write_unaligned(256 + swizzle) };
    S_OK
}

/// A non-null output receives the same selection returned in EAX.
#[unsafe(export_name = "XGComputeOptimalDepthStencilSwizzleMode")]
pub(super) unsafe extern "system" fn optimal_depth_swizzle(
    format: u32,
    _width: u32,
    _height: u32,
    _depth: u32,
    _mips: u32,
    _samples: u32,
    flag_a: u32,
    flag_b: u32,
    output: *mut u32,
) -> u32 {
    let mode = if (flag_a != 0 || flag_b != 0) && has_stencil(format) {
        u32::MAX
    } else {
        24
    };
    if !output.is_null() {
        unsafe { output.write(mode) };
    }
    mode
}

/// Validate a depth/stencil swizzle mode. This Xbox DLL's supported modes are
/// independent of the supplied format and sample-count parameters.
#[unsafe(export_name = "XGValidateDepthStencilSwizzleMode")]
pub(super) extern "system" fn validate_depth_stencil_swizzle(
    _format: u32,
    _samples: u32,
    swizzle: u32,
) -> i32 {
    if matches!(swizzle, 4 | 8 | 12 | 16 | 20 | 24 | 28) {
        S_OK
    } else {
        E_FAIL
    }
}

/// Element size and the pixel extent reported by XG for a storage element.
pub(super) fn tile_format(format: u32) -> Option<(u32, u32, u32)> {
    Some(match format {
        1..=4 => (16, 1, 1),
        // RGB32 uses three 32-bit storage elements per pixel. Native XG
        // applies an integer pixel multiplier, yielding zero pixel width.
        5..=8 => (4, 0, 1),
        9..=22 | 102 => (8, 1, 1),
        23..=47 | 67 | 87..=93 | 100 | 101 | 117 => (4, 1, 1),
        48..=59 | 85 | 86 | 104 | 114 | 115 | 118 | 119 => (2, 1, 1),
        60..=66 | 103 | 105 | 106 | 108..=113 | 116 | 120 => (1, 1, 1),
        68 | 69 | 107 => (4, 2, 1),
        70..=72 | 79..=81 => (8, 4, 4),
        73..=78 | 82..=84 | 94..=99 => (16, 4, 4),
        _ => return None,
    })
}

/// Compute the 64 KiB tile dimensions for supported buffers and 2D formats.
///
/// # Safety
/// Each non-null output must point to a writable `TileShape`.
#[unsafe(export_name = "XGComputeTileShape")]
pub(super) unsafe extern "system" fn compute_tile_shape(
    dimension: u32,
    format: u32,
    bytes_per_element: u32,
    samples: u32,
    pixels: *mut TileShape,
    elements: *mut TileShape,
) -> i32 {
    if dimension != 3 && samples != 1 {
        return E_INVALIDARG;
    }
    let (element_bytes, pixel_width, pixel_height) = match (dimension, format) {
        (1, 0) if bytes_per_element <= 1 => (1, 0, 0),
        (1, _) => return E_INVALIDARG,
        (2 | 4, _) => return E_NOTIMPL,
        (3, 0) => (bytes_per_element, 0, 0),
        (3, _) => match tile_format(format) {
            Some(format) => format,
            None => return E_NOTIMPL,
        },
        _ => return E_NOTIMPL,
    };
    let (mut width, mut height) = if dimension == 1 {
        (65536, 1)
    } else {
        match element_bytes {
            1 => (256, 256),
            2 => (256, 128),
            4 => (128, 128),
            8 => (128, 64),
            16 => (64, 64),
            _ => return E_INVALIDARG,
        }
    };
    if dimension == 3 {
        match samples {
            1 => (),
            2 => height /= 2,
            4 => {
                width /= 2;
                height /= 2;
            }
            8 => {
                width /= 2;
                height /= 4;
            }
            _ => return E_INVALIDARG,
        }
    }
    // Native writes the element shape first; this also preserves the result
    // when a caller deliberately aliases the two output pointers.
    unsafe {
        if !elements.is_null() {
            elements.write(TileShape {
                width,
                height,
                depth: 1,
            });
        }
        if !pixels.is_null() {
            pixels.write(if format == 0 {
                TileShape {
                    width: 0,
                    height: 0,
                    depth: 0,
                }
            } else {
                TileShape {
                    width: width * pixel_width,
                    height: height * pixel_height,
                    depth: 1,
                }
            });
        }
    }
    S_OK
}
