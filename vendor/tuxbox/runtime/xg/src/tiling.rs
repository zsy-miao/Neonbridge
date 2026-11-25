//! D3D tiled-resource queries on a texture address computer.
use super::{Computer, TileShape};
use windows_sys::Win32::Foundation::{E_FAIL, S_OK};

#[repr(C)]
pub(super) struct PackedMips {
    standard: u8,
    packed: u8,
    reserved: u16,
    tiles: u32,
    start: u32,
}

#[repr(C)]
pub(super) struct SubresourceTiling {
    width: u32,
    height: u16,
    depth: u16,
    start: u32,
}

pub(super) extern "system" fn is_sparse(this: &Computer) -> u8 {
    let (_, _, swizzle) = this.plane.image_extent();
    u8::from(
        matches!(swizzle, 16..=19)
            || (swizzle == 10 && matches!(this.image, super::descriptor::Image::Packed(_))),
    )
}

pub(super) unsafe extern "system" fn resource_tiling(
    this: &Computer,
    total: *mut u32,
    packed: *mut PackedMips,
    shape: *mut TileShape,
    count: *mut u32,
    first: u32,
    tilings: *mut SubresourceTiling,
) -> i32 {
    let plane = &this.plane;
    let (_, _, swizzle) = plane.image_extent();
    if !matches!(swizzle, 10 | 17 | 18 | 24 | 27)
        || this.depth_stencil.is_some_and(|d| d.depth_index() != 0)
    {
        return E_FAIL;
    }
    let (width, height) = match plane.element_bytes() {
        1 => (256, 256),
        2 => (256, 128),
        4 => (128, 128),
        8 => (128, 64),
        16 => (64, 64),
        _ => return E_FAIL,
    };
    let (width, height) = plane.multisample_tile().unwrap_or((width, height));
    let mips = plane.mip_count();
    let standard = plane.unpacked_mip_count();
    let has_tail = standard < mips;
    // The caller specifies capacity in subresources. Packed levels share a
    // tile and have no individual records in the returned tiling array.
    if !count.is_null() {
        let end = first
            .wrapping_add(unsafe { count.read() })
            .min(mips * plane.array_size());
        let mut written = 0;
        if !tilings.is_null() {
            for index in first..end {
                let level = index % mips;
                if level >= standard {
                    continue;
                }
                let slice = index / mips;
                if let (Some((columns, rows)), Some(range)) = (
                    plane.standard_tile_grid(level, width, height),
                    plane.mip_range_in_slice(slice, level),
                ) {
                    unsafe {
                        tilings
                            .add((index - first) as usize)
                            .write(SubresourceTiling {
                                width: columns,
                                height: rows as u16,
                                depth: 1,
                                start: (range.start >> 16) as u32,
                            });
                    }
                    written += 1;
                }
            }
        }
        unsafe {
            count.write(written);
        }
    }
    unsafe {
        if !packed.is_null() {
            packed.write(PackedMips {
                standard: standard as u8,
                packed: (mips - standard) as u8,
                reserved: 0,
                tiles: u32::from(has_tail),
                start: if has_tail {
                    (plane.mip_offset(standard).unwrap_or(0) >> 16) as u32
                } else {
                    0
                },
            });
        }
        if !shape.is_null() {
            let block = plane.texels_per_element_axis();
            shape.write(TileShape {
                width: width * block,
                height: height * block,
                depth: 1,
            });
        }
        if !total.is_null() {
            total.write((plane.size >> 16) as u32);
        }
    }
    S_OK
}
