//! Decode a Scarlett hardware image resource descriptor for address queries.
use super::{Computer, Desc, Plane, allocate_computer, library};
use core::ptr;
use windows_sys::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};

/// Equivalent DXGI storage layouts. Channel selection and numeric interpretation
/// do not affect the address of an element within these formats.
fn storage_format(format: u32) -> Option<u32> {
    Some(match format {
        1..=6 | 128 => 61,           // R8
        7..=19 | 129 => 49,          // R16 or RG8
        20..=61 | 130 | 140 => 28,   // 32-bit color
        62..=71 => 10,               // RG32 or RGBA16
        75..=77 => 2,                // RGBA32
        169..=170 | 175..=176 => 71, // BC1 / BC4
        171..=174 | 177..=182 => 77, // BC2 / BC3 / BC5 / BC6 / BC7
        _ => return None,
    })
}

/// The descriptor consists of eight DWORDs. A null argument leaves the other
/// argument untouched. A rejected, non-null descriptor clears the output.
#[unsafe(export_name = "XGCreateTextureComputerFromDescriptor")]
unsafe extern "system" fn from_descriptor(words: *const [u32; 8], out: *mut *mut Computer) -> i32 {
    let _guard = library::Guard::acquire();
    if words.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let raw = unsafe { words.read_unaligned() };
    let [_, format_width, extent, view, array, levels, metadata, _] = raw;
    unsafe { out.write(ptr::null_mut()) };
    library::initialize();
    let format = (format_width >> 20) & 0x1ff;
    let kind = view >> 28;
    if kind < 8 || format == 0 {
        return E_INVALIDARG;
    }
    let Some(format) = storage_format(format) else {
        return E_NOTIMPL;
    };
    let dimension = match kind {
        8 | 12 => 2,
        9 | 13 | 15 => 3,
        10 => 4,
        _ => return E_NOTIMPL,
    };
    let swizzle = (view >> 20) & 31;
    let last = (view >> 16) & 15;
    if (view >> 12) & 15 != 0
        || (levels >> 4) & 15 != last
        || array >> 16 != 0
        || metadata & (1 << 21) != 0
    {
        return E_NOTIMPL;
    }
    let multisampled = kind == 15;
    if multisampled && !(swizzle == 27 && (1..=3).contains(&last)) {
        return E_NOTIMPL;
    }
    let mut desc = Desc {
        dimension: 3,
        alignment: 0,
        width: u64::from(1 + ((extent & 0x3fff) << 2 | (format_width >> 30))),
        height: 1 + ((extent >> 14) & 0xffff),
        slices: match u16::try_from((array & 0xffff) + 1) {
            Ok(n) => n,
            Err(_) => return E_INVALIDARG,
        },
        mips: if multisampled { 1 } else { last as u16 + 1 },
        format,
        samples: if multisampled { 1 << last } else { 1 },
        quality: 0,
        layout: 256 + swizzle,
        flags: 0,
    };
    let plane = match (dimension, swizzle) {
        (2, 0) if desc.height == 1 && desc.format < 70 => Plane::compute(&desc),
        (2, 5) => return E_INVALIDARG,
        (3, 0 | 5) => Plane::compute(&desc),
        (3, 10 | 17 | 18) => Plane::compute_sparse_64k(&desc),
        (3, 27) => {
            desc.layout = 0;
            desc.flags = 0x100001;
            if desc.mips == 1 {
                Plane::compute_64k_color(&desc)
            } else {
                Plane::compute_64k_mips(&desc)
            }
        }
        (4, 0 | 5) => {
            if swizzle == 0 {
                Plane::compute_linear_volume(&desc)
            } else {
                Plane::compute_4k_volume(&desc)
            }
        }
        _ => return E_NOTIMPL,
    };
    match plane {
        Ok(plane) => unsafe { allocate_computer(plane, dimension, 0, Image::Packed(raw), out) },
        Err(status) => status,
    }
}

pub(super) enum Image {
    Packed([u32; 8]),
    Typed(u32),
}

// Hardware image format and component selection for DXGI formats admitted
// by the texture layout factory.
fn color_view(format: u32) -> Option<(u32, u32)> {
    Some(match format {
        1 | 3 => (75, 0xfac),
        2 => (77, 0xfac),
        4 => (76, 0xfac),
        9 | 12 => (69, 0xfac),
        10 => (71, 0xfac),
        11 => (65, 0xfac),
        13 => (66, 0xfac),
        14 => (70, 0xfac),
        15 | 17 => (62, 0x22c),
        16 => (64, 0x22c),
        18 => (63, 0x22c),
        23 | 25 => (54, 0xfac),
        24 => (50, 0xfac),
        26 => (36, 0x3ac),
        27 | 30 => (60, 0xfac),
        28 => (56, 0xfac),
        29 => (130, 0xfac),
        31 => (57, 0xfac),
        32 => (61, 0xfac),
        33 | 36 => (27, 0x22c),
        34 => (29, 0x22c),
        35 => (23, 0x22c),
        37 => (24, 0x22c),
        38 => (28, 0x22c),
        39 | 42 => (20, 0x204),
        41 => (22, 0x204),
        43 => (21, 0x204),
        48 | 50 => (18, 0x22c),
        49 => (14, 0x22c),
        51 => (15, 0x22c),
        52 => (19, 0x22c),
        53 | 57 => (11, 0x204),
        54 => (13, 0x204),
        56 => (7, 0x204),
        58 => (8, 0x204),
        59 => (12, 0x204),
        60 | 62 => (5, 0x204),
        61 => (1, 0x204),
        63 => (2, 0x204),
        64 => (6, 0x204),
        65 => (1, 0x800),
        70 | 71 => (169, 0xfac),
        72 => (170, 0xfac),
        73 | 74 => (171, 0xfac),
        75 => (172, 0xfac),
        76 | 77 => (173, 0xfac),
        78 => (174, 0xfac),
        79 | 80 => (175, 0x204),
        81 => (176, 0x204),
        82 | 83 => (177, 0x22c),
        84 => (178, 0x22c),
        87 => (56, 0xf2e),
        88 => (56, 0x32e),
        90 => (60, 0xf2e),
        91 => (130, 0xf2e),
        92 => (60, 0x32e),
        93 => (130, 0x32e),
        94 | 95 => (179, 0x3ac),
        96 => (180, 0x3ac),
        97 | 98 => (181, 0xfac),
        99 => (182, 0xfac),
        _ => return None,
    })
}

pub(super) unsafe extern "system" fn get(this: &Computer, plane: u32, out: *mut [u32; 8]) -> i32 {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let words = match this.image {
        Image::Packed(words) if plane == 0 => words,
        Image::Packed(_) => return E_INVALIDARG,
        Image::Typed(format) => {
            let (width, height, mut swizzle) = this.plane.image_extent();
            let last = this.plane.mip_count() - 1;
            let (hw_format, channels) = if let Some(depth) = this.depth_stencil {
                if plane == depth.depth_index() {
                    let hw = match format {
                        53 | 55 => 7,
                        44 | 45 => 141,
                        _ => 22,
                    };
                    (hw, 0x204)
                } else if depth.base(plane).is_some() && plane == depth.depth_index() + 1 {
                    (5, 4)
                } else {
                    return E_INVALIDARG;
                }
            } else if plane == 0 {
                // The single-mip color target descriptor chooses a typed
                // render view for typeless formats and a stored alpha for BGRX.
                let view_format = if last == 0 && swizzle == 27 {
                    match format {
                        1 | 9 | 15 | 23 | 27 | 33 | 53 | 60 => format + 1,
                        39 => 41,
                        48 => 49,
                        88 | 90 | 92 => 87,
                        93 => 91,
                        _ => format,
                    }
                } else {
                    format
                };
                let Some(view) = color_view(view_format) else {
                    return E_NOTIMPL;
                };
                view
            } else if plane == 1 && this.color_metadata == 1 {
                swizzle = 24;
                (5, 4)
            } else {
                return E_INVALIDARG;
            };
            let samples = this
                .depth_stencil
                .map_or_else(|| this.plane.color_samples(), |depth| depth.samples());
            let kind = match this.dimension {
                3 if samples > 1 => 15,
                2 => 12,
                3 => 13,
                4 => 10,
                _ => return E_NOTIMPL,
            };
            let sample_levels = samples.trailing_zeros();
            let width = width - 1;
            let single_render_mip = last == 0 && matches!(swizzle, 24 | 27);
            [
                0,
                (width & 3) << 30 | hw_format << 20,
                0x8000_0000 | (height - 1) << 14 | width >> 2,
                kind << 28 | swizzle << 20 | (last | sample_levels) << 16 | channels,
                this.plane.array_size() - 1,
                if single_render_mip {
                    0x0040_0000 | sample_levels << 4
                } else {
                    0x8040_0000 | last << 4
                },
                0,
                0,
            ]
        }
    };
    unsafe {
        out.write_unaligned(words);
    }
    0
}
