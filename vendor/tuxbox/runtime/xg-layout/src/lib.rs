//! Scarlett Texture2D layout and array addressing for single-sample color textures.
mod tiled;
mod volume;
mod element;

mod depth;
mod msaa_color;
#[cfg(test)]
#[path = "tests/lib.rs"]
mod tests;
pub use depth::DepthStencil;

use windows_sys::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};

const MAX_MIPS: usize = 15;
const LINEAR: u32 = 256;
const TILE_4K: u32 = 261;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Desc {
    pub dimension: u32,
    pub alignment: u64,
    pub width: u64,
    pub height: u32,
    pub slices: u16,
    pub mips: u16,
    pub format: u32,
    pub samples: u32,
    pub quality: u32,
    pub layout: u32,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Mip {
    size: u64,
    offset: u64,
    slice_bytes: u64,
    pitch_texels: u32,
    pitch_bytes: u32,
    alignment: u32,
    pitch_elements: u32,
    padded_rows: u32,
    padded_depth: u32,
    width_elements: u32,
    height_elements: u32,
    width_texels: u32,
    height_texels: u32,
    depth: u32,
    slices: u32,
    swizzle: u32,
    reserved: [u32; 5],
    samples: u32,
    packed_tail: u32,
    padding: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Plane {
    color_plane: u32,
    padding: u32,
    pub size: u64,
    slice_bytes: u64,
    offset: u64,
    pub alignment: u64,
    element_bytes: u32,
    unpacked_mips: u32,
    tail_mips: u32,
    reserved: u32,
    mips: [Mip; MAX_MIPS],
}

#[repr(C)]
pub struct Layout {
    pub bytes: u64,
    pub alignment: u64,
    pub mips: u32,
    pub plane_count: u32,
    pub planes: [Plane; 4],
    pub dimension: u32,
    reserved: u32,
}

const _: () = {
    assert!(size_of::<Desc>() == 56 && core::mem::offset_of!(Desc, layout) == 44);
    assert!(size_of::<Mip>() == 0x70 && size_of::<Plane>() == 0x6c8);
    assert!(core::mem::offset_of!(Plane, mips) == 0x38);
    assert!(size_of::<Layout>() == 0x1b40 && core::mem::offset_of!(Layout, planes) == 0x18);
};

impl Layout {
    /// Append the bounded color metadata plane used by the 64 KiB profile.
    pub fn append_color_metadata(&mut self, kind: u8) {
        let size = self.planes[0].color_metadata_size(kind);
        let alignment = if kind == 2 {
            65536
        } else {
            self.planes[0].metadata_block(kind).2
        };
        let mut metadata = Plane {
            color_plane: if kind == 2 { 9 } else { 2 },
            size,
            offset: self.bytes,
            alignment,
            element_bytes: if kind == 2 { 1 } else { 0 },
            ..Plane::default()
        };
        metadata.mips[0].size = size;
        metadata.mips[0].alignment = alignment as u32;
        self.planes[1] = metadata;
        self.plane_count = 2;
        self.bytes += size;
    }
}

impl Plane {
    pub fn color_metadata_size(&self, kind: u8) -> u64 {
        if kind == 0 {
            return 0;
        }
        let (tw, th, bytes) = self.metadata_block(kind);
        u64::from(self.mips[0].width_texels.div_ceil(tw))
            * u64::from(self.mips[0].height_texels.div_ceil(th))
            * bytes
            * u64::from(self.array_size())
    }

    pub fn compute(desc: &Desc) -> Result<Self, i32> {
        let (block, element_bytes, tile_width, tile_height): (u32, u32, u32, u32) =
            match desc.format {
                1..=4 => (1, 16, 16, 16),            // RGBA32.
                9..=18 => (1, 8, 32, 16),            // RGBA16 and RG32.
                48..=54 | 56..=59 => (1, 2, 64, 32), // RG8 and color R16.
                60..=65 => (1, 1, 64, 64),           // R8 and A8.
                // Packed RGB, RGBA8, RG16, R32 (color), and BGR(A/X)8.
                23..=39 | 41..=43 | 67 | 87 | 88 | 90..=93 => (1, 4, 32, 32),
                // BC1 and BC4 each store a 4x4 block in eight bytes.
                70..=72 | 79..=81 => (4, 8, 32, 16),
                // BC2/3/5/6H/7 each store a 4x4 block in 16 bytes.
                73..=78 | 82..=84 | 94..=99 => (4, 16, 16, 16),
                _ => return Err(E_NOTIMPL),
            };
        let linear = desc.layout == LINEAR;
        // A linear uncompressed render target with disabled color metadata
        // uses the same pitch, mip offsets and array stride as a shader resource.
        let placement_supported = if linear {
            let disabled_metadata_target = block == 1
                && desc.flags & 0x0030_0000 != 0
                && desc.flags & !0x0030_0005 == 0;
            (desc.flags & !0x0030_0004 == 0 || disabled_metadata_target)
                && matches!(desc.alignment, 0 | 256 | 4096 | 65536)
        } else {
            desc.layout == TILE_4K && desc.flags == 0 && matches!(desc.alignment, 0 | 4096)
        };
        if desc.dimension != 3
            || desc.samples != 1
            || desc.quality != 0
            || !placement_supported
        {
            return Err(E_NOTIMPL);
        }
        if !(1..=16384).contains(&desc.width)
            || !(1..=16384).contains(&desc.height)
            || !(1..=2048).contains(&desc.slices)
        {
            return Err(E_INVALIDARG);
        }
        let width = desc.width as u32;
        let levels = u32::from(desc.mips);
        if levels == 0 || levels > 1 + width.max(desc.height).ilog2() {
            return Err(E_INVALIDARG);
        }
        let mut plane = Self {
            color_plane: 1,
            element_bytes,
            unpacked_mips: levels,
            alignment: if linear { 256 } else { 4096 },
            ..Self::default()
        };
        // Padded levels round the original extent up. Visible mip dimensions round down.
        for level in 0..levels {
            let scale = 1 << level;
            let w = width.div_ceil(block * scale);
            let h = desc.height.div_ceil(block * scale);
            if !linear && levels > 1 && w <= tile_width / 2 && h <= tile_height {
                plane.unpacked_mips = plane.unpacked_mips.min(level);
            }
            let in_tail = level >= plane.unpacked_mips;
            let (pitch, rows, alignment, swizzle) = if linear {
                (w.next_multiple_of(256 / element_bytes), h, 256, 0)
            } else if in_tail {
                // Small mips share one 4 KiB tile, down to a 256-byte microtile.
                let index = level - plane.unpacked_mips;
                (
                    (tile_width >> (index + 1)).max(tile_width / 4),
                    (tile_height >> index).max(tile_height / 4),
                    256,
                    1,
                )
            } else {
                (
                    w.next_multiple_of(tile_width),
                    h.next_multiple_of(tile_height),
                    4096,
                    5,
                )
            };
            let size = u64::from(pitch * element_bytes) * u64::from(rows);
            let width_texels = (width >> level).max(1);
            let height_texels = (desc.height >> level).max(1);
            plane.mips[level as usize] = Mip {
                size,
                slice_bytes: size,
                pitch_texels: pitch * block,
                pitch_bytes: pitch * element_bytes,
                alignment,
                pitch_elements: pitch,
                padded_rows: rows,
                padded_depth: u32::from(desc.slices),
                width_elements: width_texels.div_ceil(block),
                height_elements: height_texels.div_ceil(block),
                width_texels,
                height_texels,
                depth: u32::from(desc.slices),
                slices: 1,
                swizzle,
                samples: 1,
                packed_tail: u32::from(in_tail),
                ..Mip::default()
            };
            if !in_tail {
                plane.size += size;
            }
        }
        plane.tail_mips = levels - plane.unpacked_mips;
        if plane.tail_mips > 0 {
            plane.size += 4096;
        }
        plane.slice_bytes = plane.size;
        // Smaller mips occupy the lower addresses in both layouts.
        let mut end = plane.size;
        for mip in &mut plane.mips[..levels as usize] {
            end -= mip.size;
            mip.offset = end;
        }
        plane.size *= u64::from(desc.slices);
        Ok(plane)
    }

    /// Set the bank mask and bank stride for the version-4 Scarlett profile.
    /// The layout ABI exposes both fields and address queries use them to
    /// select the matching XOR equation.
    pub fn select_version_four(&mut self) {
        for mip in &mut self.mips {
            if mip.swizzle == 27 {
                mip.reserved[1] = 0xe000;
                mip.reserved[3] = 0x2000;
            }
        }
    }

    /// Visible dimensions in storage elements (blocks for BC formats).
    pub fn mip_extent(&self, level: u32) -> Option<(u32, u32, u32, bool)> {
        let mip = self.mips.get(level as usize)?;
        (level < self.mip_count()).then_some((
            mip.width_elements,
            mip.height_elements,
            mip.depth,
            mip.swizzle == 0,
        ))
    }

    /// Logical texel dimensions and primary hardware image swizzle.
    pub fn image_extent(&self) -> (u32, u32, u32) {
        let mip = &self.mips[0];
        let swizzle = match mip.swizzle {
            1 if self.alignment != 256 => 5,
            2 if self.alignment == 65536 => 10,
            3 => 27,
            other => other,
        };
        (mip.width_texels, mip.height_texels, swizzle)
    }

    pub fn mip_offset(&self, level: u32) -> Option<u64> {
        self.mips
            .get(level as usize)
            .filter(|_| level < self.mip_count())
            .map(|m| m.offset)
    }

    pub fn unpacked_mip_count(&self) -> u32 {
        self.unpacked_mips
    }

    pub fn mip_count(&self) -> u32 {
        self.unpacked_mips + self.tail_mips
    }

    pub fn array_size(&self) -> u32 {
        self.mips[0].depth
    }

    pub fn element_bytes(&self) -> u32 {
        self.element_bytes
    }

    /// BC formats address 4×4 texel blocks. Other single-plane formats address texels.
    pub fn texels_per_element_axis(&self) -> u32 {
        self.mips[0].pitch_texels / self.mips[0].pitch_elements
    }

    pub fn metadata_tile_shape(&self) -> (u32, u32) {
        match self.element_bytes {
            16 => (512, 256),
            8 => (512, 512),
            4 => (1024, 512),
            2 => (1024, 1024),
            _ => (2048, 1024),
        }
    }

    pub fn metadata_tile_stride(&self) -> u32 {
        self.mips[0]
            .width_texels
            .div_ceil(self.metadata_tile_shape().0)
    }

    pub fn metadata_tile_rows(&self) -> u32 {
        self.mips[0]
            .height_texels
            .div_ceil(self.metadata_tile_shape().1)
    }

    /// Range within the Xbox allocation occupied by this mip, including padding.
    pub fn mip_range_in_slice(&self, slice: u32, level: u32) -> Option<core::ops::Range<u64>> {
        if slice >= self.array_size() || level >= self.mip_count() {
            return None;
        }
        let mip = &self.mips[level as usize];
        let base = u64::from(slice) * self.slice_bytes;
        Some(base + mip.offset..base + mip.offset + mip.size)
    }

    pub fn element_offset(&self, level: u32, x: u64, y: u32) -> Option<u64> {
        self.element_offset_in_slice(0, level, x, y)
    }

    pub fn element_offset_in_slice(&self, slice: u32, level: u32, x: u64, y: u32) -> Option<u64> {
        self.element_offset_for_hardware(slice, level, x, y, false)
    }

}

type VolumeTile = (u32, u32, u32, &'static [u8], &'static [u8], &'static [u8]);

fn volume_tile(bytes: u32) -> Option<VolumeTile> {
    Some(match bytes {
        1 => (16, 16, 16, &[0, 1, 6, 9], &[3, 5, 8, 11], &[2, 4, 7, 10]),
        2 => (8, 16, 16, &[1, 6, 9], &[3, 5, 8, 11], &[2, 4, 7, 10]),
        4 => (8, 16, 8, &[2, 6, 9], &[3, 5, 8, 11], &[4, 7, 10]),
        8 => (8, 8, 8, &[3, 6, 9], &[5, 8, 11], &[4, 7, 10]),
        16 => (4, 8, 8, &[6, 9], &[5, 8, 11], &[4, 7, 10]),
        _ => return None,
    })
}

fn microtile_bits(bytes: u32, mode: u32) -> Option<(&'static [u8], &'static [u8])> {
    Some(match (bytes, mode) {
        (1, 1) => (&[0, 1, 2, 3], &[4, 5, 6, 7]),
        (1, 2) => (&[0, 1, 2, 6], &[4, 3, 5, 7]),
        (2, 1 | 2) => (&[1, 2, 3, 7], &[4, 5, 6]),
        (4, 1) => (&[2, 3, 7], &[4, 5, 6]),
        (4, 2) => (&[2, 3, 6], &[4, 5, 7]),
        (8, 1) => (&[3, 6, 7], &[4, 5]),
        (8, 2) => (&[3, 5, 6], &[4, 7]),
        (16, 1) => (&[6, 7], &[4, 5]),
        (16, 2) => (&[4, 6], &[5, 7]),
        _ => return None,
    })
}

fn scatter(value: u32, positions: &[u8]) -> u32 {
    positions
        .iter()
        .enumerate()
        .fold(0, |bits, (i, bit)| bits | (((value >> i) & 1) << bit))
}
