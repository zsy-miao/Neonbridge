//! Scarlett Texture2D layout and array addressing for single-sample color textures.
mod depth;
mod msaa_color;
#[cfg(test)]
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

    pub fn metadata_block(&self, kind: u8) -> (u32, u32, u64) {
        let version_four = self.mips[0].reserved[1] == 0xe000;
        if kind == 1 {
            return if version_four {
                (1024, 512, 4096)
            } else {
                (1024, 1024, 8192)
            };
        }
        if version_four {
            let (w, h) = match self.element_bytes {
                1 => (1024, 1024),
                2 => (1024, 512),
                4 => (512, 512),
                8 => (512, 256),
                _ => (256, 256),
            };
            (w, h, 4096)
        } else {
            let (w, h) = self.metadata_tile_shape();
            (w, h, 8192)
        }
    }

    /// Single-sample 2D render-target mip chains, including their shared 64 KiB tail.
    pub fn compute_64k_mips(desc: &Desc) -> Result<Self, i32> {
        if desc.dimension != 3
            || desc.layout != 0
            || desc.alignment != 0
            || desc.samples != 1
            || desc.quality != 0
            || !(1..=8).contains(&desc.slices)
            || !(2..=12).contains(&desc.mips)
            || !(32..=16384).contains(&desc.width)
            || !(32..=16384).contains(&desc.height)
            || !matches!(
                desc.flags,
                1 | 5 | 0x100001 | 0x100005 | 0x200001 | 0x200005 | 0x300001 | 0x300005
            )
            || !matches!(desc.format, 1..=4 | 9..=18 | 23..=39 | 41..=43 | 48..=54 | 56..=65 | 87 | 88 | 90..=93)
        {
            return Err(E_NOTIMPL);
        }
        Self::tiled_64k_mips(desc, 27)
    }

    fn tiled_64k_mips(desc: &Desc, swizzle: u32) -> Result<Self, i32> {
        let mut base = *desc;
        base.layout = LINEAR;
        base.flags = 0;
        let mut plane = Self::compute(&base)?;
        let (tw, th) = match plane.element_bytes {
            1 => (256u32, 256u32),
            2 => (256, 128),
            4 => (128, 128),
            8 => (128, 64),
            16 => (64, 64),
            _ => return Err(E_NOTIMPL),
        };
        let levels = usize::from(desc.mips);
        let block = if matches!(desc.format, 70..=84 | 94..=99) {
            4
        } else {
            1
        };
        let width = (desc.width as u32).div_ceil(block);
        let height = desc.height.div_ceil(block);
        let first_tail = (0..levels)
            .find(|i| {
                width.div_ceil(1 << i) <= tw / 2 && height.div_ceil(1 << i) <= th
            })
            .unwrap_or(levels);
        // A BC mip chain continues down to one texel inside its last 4×4 block.
        let max_tail = ((tw / 2).max(th) * block).ilog2() as usize + 1;
        if levels - first_tail > max_tail {
            return Err(E_NOTIMPL);
        }
        plane.unpacked_mips = first_tail as u32;
        plane.tail_mips = (levels - first_tail) as u32;
        plane.alignment = 65536;
        plane.size = if first_tail < levels { 65536 } else { 0 };
        for (i, mip) in plane.mips[..levels].iter_mut().enumerate() {
            let tail = i >= first_tail;
            let idx = i.saturating_sub(first_tail);
            let pitch = if tail {
                (tw >> (idx + 1)).max(tw / 16)
            } else {
                width.div_ceil(1 << i).next_multiple_of(tw)
            };
            let rows = if tail {
                (th >> idx).max(th / 16)
            } else {
                height.div_ceil(1 << i).next_multiple_of(th)
            };
            mip.size = u64::from(pitch) * u64::from(rows) * u64::from(plane.element_bytes);
            mip.slice_bytes = mip.size;
            mip.pitch_texels = pitch * block;
            mip.pitch_bytes = pitch * plane.element_bytes;
            mip.pitch_elements = pitch;
            mip.padded_rows = rows;
            mip.alignment = if tail { 256 } else { 65536 };
            let tail_swizzle = if swizzle == 10 { 2 } else { 3 };
            mip.swizzle = if tail { tail_swizzle } else { swizzle };
            mip.packed_tail = u32::from(tail);
            if !tail && swizzle == 27 {
                mip.reserved[1] = 0x8000;
                mip.reserved[3] = 0x8000;
            }
            if !tail {
                plane.size += mip.size;
            }
        }
        plane.slice_bytes = plane.size;
        let mut end = plane.slice_bytes;
        plane.size *= u64::from(desc.slices);
        for (i, mip) in plane.mips[..levels].iter_mut().enumerate() {
            if i < first_tail {
                end -= mip.size;
                mip.offset = end;
            } else {
                let idx = i - first_tail;
                mip.offset = if idx < 5 {
                    32768 >> idx
                } else {
                    1536 - ((idx - 5) as u64) * 256
                };
            }
        }
        Ok(plane)
    }

    /// 256-byte standard and display tiles on Scarlett.
    pub fn compute_microtile(desc: &Desc) -> Result<Self, i32> {
        if !matches!(desc.layout, 257 | 258) || desc.flags & !0x0030_0004 != 0
            || !matches!(desc.alignment, 0 | 256 | 4096) {
            return Err(E_NOTIMPL);
        }
        if matches!(desc.format, 70..=84 | 94..=99) { return Err(E_NOTIMPL); }
        let base = Desc { layout: LINEAR, flags: 0, alignment: 0, ..*desc };
        let mut plane = Self::compute(&base)?;
        let mode = desc.layout - 256;
        let (xb, yb) = microtile_bits(plane.element_bytes, mode).ok_or(E_NOTIMPL)?;
        let mut offset = 0;
        for (level, mip) in plane.mips[..usize::from(desc.mips)].iter_mut().enumerate().rev() {
            let scale = 1 << level;
            let pitch = (desc.width as u32).div_ceil(scale).next_multiple_of(1 << xb.len());
            let rows = desc.height.div_ceil(scale).next_multiple_of(1 << yb.len());
            mip.size = u64::from(pitch) * u64::from(rows) * u64::from(plane.element_bytes);
            mip.offset = offset;
            mip.slice_bytes = mip.size;
            mip.pitch_texels = pitch;
            mip.pitch_elements = pitch;
            mip.pitch_bytes = pitch * plane.element_bytes;
            mip.padded_rows = rows;
            mip.swizzle = mode;
            offset += mip.size;
        }
        plane.slice_bytes = offset;
        plane.size = offset * u64::from(desc.slices);
        Ok(plane)
    }

    /// Bounded single-mip render-target layout with 64 KiB swizzle 27.
    pub fn compute_64k_color(desc: &Desc) -> Result<Self, i32> {
        let four_byte = matches!(desc.format, 1..=4 | 9..=18 | 23..=39 | 41..=43 | 48..=54 | 56..=65 | 87 | 88 | 90..=93)
            && matches!(
                desc.flags,
                1 | 5 | 0x100001 | 0x100005 | 0x200001 | 0x200005 | 0x300001 | 0x300005
            );
        let color_family = matches!(desc.format, 1..=4 | 9..=18 | 23..=39 | 41..=43 | 48..=54 | 56..=65 | 87 | 88 | 90..=93)
            && matches!(desc.flags, 0xc00001 | 0xc00005);
        if desc.dimension != 3
            || !(four_byte || color_family)
            || desc.layout != 0
            || desc.alignment != 0
            || desc.quality != 0
            || desc.mips != 1
            || !(1..=8).contains(&desc.slices)
            || desc.width == 0
            || desc.width > 16384
            || desc.height == 0
            || desc.height > 16384
        {
            return Err(E_NOTIMPL);
        }
        if desc.samples == 1 {
            Self::single_64k_plane(desc, 27)
        } else {
            Self::multisampled_color(desc)
        }
    }

    /// 64 KiB display swizzle. The packed image factory can mark this layout sparse.
    pub fn compute_sparse_64k(desc: &Desc) -> Result<Self, i32> {
        if desc.dimension != 3
            || !matches!(desc.layout, 265 | 266 | 273 | 274)
            || desc.alignment != 0
            || desc.samples != 1
            || desc.quality != 0
            || (desc.mips != 1 && desc.layout != 266)
            || !(1..=2048).contains(&desc.slices)
            || !(1..=16384).contains(&desc.width)
            || !(1..=16384).contains(&desc.height)
            || !(desc.flags & !0x0030_0004 == 0 || matches!(desc.flags, 0x0800_0000 | 0x0800_0004))
        {
            return Err(E_NOTIMPL);
        }
        if desc.mips == 1 {
            Self::single_64k_plane(desc, desc.layout - 256)
        } else {
            Self::tiled_64k_mips(desc, 10)
        }
    }

    fn single_64k_plane(desc: &Desc, swizzle: u32) -> Result<Self, i32> {
        let mut base = *desc;
        base.layout = TILE_4K;
        base.flags = 0;
        let mut plane = Self::compute(&base)?;
        let (tw, th) = match plane.element_bytes {
            1 => (256, 256),
            2 => (256, 128),
            4 => (128, 128),
            8 => (128, 64),
            16 => (64, 64),
            _ => return Err(E_NOTIMPL),
        };
        let compressed = matches!(desc.format, 70..=84 | 94..=99);
        let block = if compressed { 4 } else { 1 };
        let pitch = plane.mips[0].width_elements.next_multiple_of(tw);
        let rows = plane.mips[0].height_elements.next_multiple_of(th);
        let bytes = u64::from(pitch) * u64::from(rows) * u64::from(plane.element_bytes);
        plane.size = bytes * u64::from(desc.slices);
        plane.slice_bytes = bytes;
        plane.alignment = 65536;
        let mip = &mut plane.mips[0];
        mip.size = bytes;
        mip.slice_bytes = bytes;
        mip.pitch_texels = pitch * block;
        mip.pitch_bytes = pitch * plane.element_bytes;
        mip.pitch_elements = pitch;
        mip.padded_rows = rows;
        mip.alignment = 65536;
        mip.swizzle = swizzle;
        if swizzle == 27 {
            mip.reserved[1] = 0x8000;
            mip.reserved[3] = 0x8000;
        }
        Ok(plane)
    }

    /// Compute a single-plane, linear 3D texture layout. `desc.slices` is the
    /// original volume depth. Mip storage is padded to that original depth,
    /// while each mip's visible depth shrinks with the level.
    pub fn compute_linear_volume(desc: &Desc) -> Result<Self, i32> {
        if desc.layout != LINEAR || desc.slices > 2048 {
            return Err(E_NOTIMPL);
        }
        let depth = u32::from(desc.slices);
        let mut plane = Self::compute(desc)?;
        let mip_count = plane.mip_count() as usize;
        for (level, mip) in plane.mips[..mip_count].iter_mut().enumerate() {
            mip.size = mip.size.checked_mul(u64::from(depth)).ok_or(E_INVALIDARG)?;
            mip.depth = (depth >> level).max(1);
        }
        Ok(plane)
    }

    /// Compute a 4 KiB-swizzled 3D plane in texels or BC blocks.
    pub fn compute_4k_volume(desc: &Desc) -> Result<Self, i32> {
        if desc.layout != TILE_4K {
            return Err(E_NOTIMPL);
        }
        let block = if matches!(desc.format, 70..=84 | 94..=99) { 4 } else { 1 };
        let mut linear = *desc;
        linear.layout = LINEAR;
        let mut plane = Self::compute(&linear)?;
        let (tw, th, td, _, _, _) = volume_tile(plane.element_bytes).ok_or(E_NOTIMPL)?;
        let levels = usize::from(desc.mips);
        // The packed region starts when a mip fits across a tile and in its
        // lower half vertically. Depth groups repeat this entire mip chain.
        let tail = (levels.saturating_sub(5)..levels).find(|&level| {
            let scale = 1u32 << level;
            levels > 1
                && (desc.width as u32).div_ceil(block * scale) <= tw
                && desc.height.div_ceil(block * scale) <= th / 2
        });
        if let Some(first) = tail {
            plane.unpacked_mips = first as u32;
            plane.tail_mips = (levels - first) as u32;
        }
        let mut chain_bytes = 0u64;
        for (level, mip) in plane.mips[..levels].iter_mut().enumerate() {
            let scale = 1u32 << level;
            let w = (desc.width as u32).div_ceil(block * scale);
            let h = desc.height.div_ceil(block * scale);
            let d = u32::from(desc.slices).div_ceil(scale);
            let in_tail = tail.is_some_and(|first| level >= first);
            let tail_index = level.saturating_sub(tail.unwrap_or(level));
            let pitch = if in_tail {
                if tail_index == 0 { tw } else { tw / 2 }
            } else {
                w.next_multiple_of(tw)
            };
            let rows = if in_tail {
                if tail_index == 0 { th / 2 } else { th / 4 }
            } else {
                h.next_multiple_of(th)
            };
            let depth = d.next_multiple_of(if in_tail { td / 2 } else { td });
            let slice_bytes = if in_tail {
                4096 / u64::from(td)
            } else {
                u64::from(pitch) * u64::from(rows) * u64::from(plane.element_bytes)
            };
            mip.size = slice_bytes * u64::from(depth);
            mip.slice_bytes = slice_bytes;
            mip.pitch_texels = pitch * block;
            mip.pitch_bytes = pitch * plane.element_bytes;
            mip.alignment = if in_tail { 256 } else { 4096 };
            mip.pitch_elements = pitch;
            mip.padded_rows = rows;
            mip.padded_depth = depth;
            mip.depth = (u32::from(desc.slices) >> level).max(1);
            mip.swizzle = if in_tail { 1 } else { 5 };
            mip.samples = td;
            mip.packed_tail = u32::from(in_tail);
            if in_tail {
                mip.padding[0] = match tail_index {
                    0 => (th / 2) << 11,
                    1 => ((th / 4) << 11) | (tw / 2),
                    2 => tw / 2,
                    3 => (th / 4) << 11,
                    _ => 0,
                };
            }
            if !in_tail {
                chain_bytes += slice_bytes * u64::from(td);
            }
        }
        if tail.is_some() {
            chain_bytes += 4096;
        }
        let mut end = chain_bytes;
        for mip in &mut plane.mips[..levels] {
            if mip.packed_tail == 0 {
                end -= mip.slice_bytes * u64::from(td);
                mip.offset = end;
            } else {
                mip.offset = 0;
            }
        }
        plane.slice_bytes = chain_bytes;
        plane.size = chain_bytes * u64::from(u32::from(desc.slices).div_ceil(td));
        plane.alignment = 4096;
        Ok(plane)
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

    /// Native linear address-to-coordinate queries scan physical mip ranges
    /// first, then decode the slice in increasing level order.
    pub fn linear_coordinate(&self, offset: u64) -> Option<(u32, u64, u32, u32)> {
        let levels = &self.mips[..self.mip_count() as usize];
        if self.mips[0].swizzle != 0
            || self.slice_bytes == 0
            || !levels
                .iter()
                .any(|m| (m.offset..m.offset + m.size).contains(&offset))
        {
            return None;
        }
        let pitch = u64::from(self.mips[0].pitch_bytes);
        let slice = u32::try_from(offset / self.slice_bytes).ok()?;
        let mut position = offset % self.slice_bytes;
        for (index, mip) in levels.iter().enumerate() {
            let bytes = u64::from(mip.padded_rows) * pitch;
            if position < bytes {
                let x = position % pitch / u64::from(self.element_bytes);
                let y = u32::try_from(position / pitch).ok()?;
                return (x < u64::from(mip.width_elements)
                    && y < mip.height_elements
                    && slice < mip.depth)
                    .then_some((index as u32, x, y, slice));
            }
            position -= bytes;
        }
        None
    }

    pub fn mip_offset(&self, level: u32) -> Option<u64> {
        self.mips
            .get(level as usize)
            .filter(|_| level < self.mip_count())
            .map(|m| m.offset)
    }

    pub fn standard_tile_grid(
        &self,
        level: u32,
        tile_width: u32,
        tile_height: u32,
    ) -> Option<(u32, u32)> {
        let mip = self.mips.get(level as usize)?;
        (level < self.unpacked_mips).then_some((
            mip.pitch_elements / tile_width,
            mip.padded_rows / tile_height,
        ))
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

    /// Element address in a supported linear or 4 KiB-swizzled 3D plane.
    pub fn element_offset_in_volume(&self, z: u32, level: u32, x: u64, y: u32) -> Option<u64> {
        let mip = self.mips.get(level as usize)?;
        if z >= mip.depth || x >= u64::from(mip.width_elements) || y >= mip.height_elements {
            return None;
        }
        if mip.swizzle == 0 {
            return self.element_offset_in_slice(z, level, x, y);
        }
        let (tw, th, td, xb, yb, zb) = volume_tile(self.element_bytes)?;
        let x = u32::try_from(x).ok()?;
        let tile = (y / th) * (mip.pitch_elements / tw) + x / tw;
        Some(
            mip.offset
                + u64::from(z / td) * self.slice_bytes
                + u64::from(tile) * 4096
                + u64::from(scatter(x + (mip.padding[0] & 0x7ff), xb)
                    | scatter(y + (mip.padding[0] >> 11), yb)
                    | scatter(z, zb)),
        )
    }

    pub fn element_offset_in_slice(&self, slice: u32, level: u32, x: u64, y: u32) -> Option<u64> {
        self.element_offset_for_hardware(slice, level, x, y, false)
    }

    pub fn element_offset_for_hardware(
        &self,
        slice: u32,
        level: u32,
        x: u64,
        y: u32,
        version_four: bool,
    ) -> Option<u64> {
        if slice >= self.array_size() {
            return None;
        }
        let mip = self.mips.get(level as usize)?;
        let origin = u64::from(slice) * self.slice_bytes + mip.offset;
        if x >= u64::from(mip.width_elements) || y >= mip.height_elements {
            return None;
        }
        if self.color_samples() > 1 {
            return self.color_sample_offset(slice, x, y, 0);
        }
        let x = x as u32;
        if mip.swizzle == 0 {
            return Some(origin + u64::from(y * mip.pitch_bytes + x * self.element_bytes));
        }
        if self.alignment == 256 && matches!(mip.swizzle, 1 | 2) {
            let (xb, yb) = microtile_bits(self.element_bytes, mip.swizzle)?;
            let tile = (y >> yb.len()) * (mip.pitch_elements >> xb.len()) + (x >> xb.len());
            return Some(origin + u64::from(tile) * 256 + u64::from(scatter(x, xb) | scatter(y, yb)));
        }
        if matches!(mip.swizzle, 9 | 10 | 17 | 18) || (mip.swizzle == 2 && self.alignment == 65536) {
            // Swizzle 9 (64 KiB standard) shares the S_X bit table without the bank XOR.
            let table = if mip.swizzle == 9 { 17 } else { mip.swizzle };
            let (xb, yb): (&[u8], &[u8]) = match (self.element_bytes, table) {
                (1, 17) => (&[0, 1, 2, 3, 9, 11, 13, 15], &[4, 5, 6, 7, 8, 10, 12, 14]),
                (1, _) => (&[0, 1, 2, 6, 9, 11, 13, 15], &[4, 3, 5, 7, 8, 10, 12, 14]),
                (2, _) => (&[1, 2, 3, 7, 9, 11, 13, 15], &[4, 5, 6, 8, 10, 12, 14]),
                (4, 17) => (&[2, 3, 7, 9, 11, 13, 15], &[4, 5, 6, 8, 10, 12, 14]),
                (4, _) => (&[2, 3, 6, 9, 11, 13, 15], &[4, 5, 7, 8, 10, 12, 14]),
                (8, 17) => (&[3, 6, 7, 9, 11, 13, 15], &[4, 5, 8, 10, 12, 14]),
                (8, _) => (&[3, 5, 6, 9, 11, 13, 15], &[4, 7, 8, 10, 12, 14]),
                (16, 17) => (&[6, 7, 9, 11, 13, 15], &[4, 5, 8, 10, 12, 14]),
                (16, _) => (&[4, 6, 9, 11, 13, 15], &[5, 7, 8, 10, 12, 14]),
                _ => return None,
            };
            let tile = (y >> yb.len()) * (mip.pitch_elements >> xb.len()) + (x >> xb.len());
            let mut inner = scatter(x, xb) | scatter(y, yb);
            if matches!(mip.swizzle, 17 | 18) {
                // T-swizzles fold three upper address bits into bank selection.
                inner ^= if version_four {
                    ((inner & 0x0800) >> 1) | ((inner & 0x1000) >> 3) | ((inner & 0x2000) >> 5)
                } else {
                    ((inner & 0x2000) >> 1) | ((inner & 0x4000) >> 3) | ((inner & 0x8000) >> 5)
                };
            }
            return Some(origin + u64::from(tile) * 65536 + u64::from(inner));
        }
        if mip.swizzle == 27 || mip.swizzle == 3 {
            let (x_basis, y_basis, tw, th): (&[u64], &[u64], u32, u32) = match self.element_bytes {
                16 => (
                    &[
                        0x10, 0x40, 0x2000, 0x1000, 0x200, 0x800, 0x8400, 0x100, 0x1000, 0,
                    ],
                    &[
                        0x20, 0x80, 0x4000, 0x8000, 0x300, 0x400, 0x800, 0x100, 0x1000, 0,
                    ],
                    64,
                    64,
                ),
                8 => (
                    &[
                        8, 0x20, 0x40, 0x4000, 0x200, 0x800, 0x1400, 0x8100, 0x1000, 0,
                    ],
                    &[
                        0x10, 0x80, 0x2000, 0x8000, 0x300, 0x400, 0x800, 0x100, 0x1000, 0,
                    ],
                    128,
                    64,
                ),
                4 => (
                    &[4, 8, 0x40, 0x2000, 0x200, 0x800, 0x1400, 0x8100, 0x1000, 0],
                    &[
                        0x10, 0x20, 0x80, 0x4000, 0x300, 0x400, 0x8800, 0x100, 0x1000, 0,
                    ],
                    128,
                    128,
                ),
                2 => (
                    &[2, 4, 8, 0x80, 0x200, 0x800, 0x1400, 0x8100, 0x1000, 0],
                    &[
                        0x10, 0x20, 0x40, 0x2000, 0x300, 0x400, 0x4800, 0x8100, 0x1000, 0,
                    ],
                    256,
                    128,
                ),
                1 => (
                    &[1, 2, 4, 8, 0x200, 0x800, 0x1400, 0x4100, 0x9000, 0],
                    &[
                        0x10, 0x20, 0x40, 0x80, 0x300, 0x400, 0x2800, 0x8100, 0x1000, 0,
                    ],
                    256,
                    256,
                ),
                _ => return None,
            };
            let version_four = version_four || mip.reserved[1] == 0xe000;
            let (x_basis, y_basis): (&[u64], &[u64]) = if version_four {
                match self.element_bytes {
                    1 => (
                        &[1, 2, 4, 8, 0x200, 0xc00, 0x1100, 0x4000, 0x8000, 0x2000],
                        &[
                            0x10, 0x20, 0x40, 0x80, 0x300, 0x400, 0x2100, 0x8000, 0x4000, 0,
                        ],
                    ),
                    2 => (
                        &[2, 4, 8, 0x80, 0x200, 0x1400, 0x2100, 0x8000, 0x4000, 0],
                        &[
                            0x10, 0x20, 0x40, 0x800, 0x300, 0x400, 0x4100, 0x8000, 0x2000, 0,
                        ],
                    ),
                    4 => (
                        &[4, 8, 0x40, 0x800, 0x200, 0x2400, 0x4100, 0x8000, 0, 0],
                        &[
                            0x10, 0x20, 0x80, 0x1000, 0x300, 0x400, 0x8100, 0x4000, 0x2000, 0,
                        ],
                    ),
                    8 => (
                        &[8, 0x20, 0x40, 0x1000, 0x200, 0x4400, 0x8100, 0x2000, 0, 0],
                        &[
                            0x10, 0x80, 0x800, 0x2000, 0x300, 0x400, 0x8100, 0x4000, 0, 0,
                        ],
                    ),
                    16 => (
                        &[0x10, 0x40, 0x800, 0x2000, 0x200, 0x8400, 0x4100, 0, 0, 0],
                        &[
                            0x20, 0x80, 0x1000, 0x4000, 0x300, 0x400, 0x8100, 0x2000, 0, 0,
                        ],
                    ),
                    _ => return None,
                }
            } else {
                (x_basis, y_basis)
            };
            let mut inner = 0u64;
            for i in 0..x_basis.len() {
                if x & (1 << i) != 0 {
                    inner ^= x_basis[i];
                }
                if y & (1 << i) != 0 {
                    inner ^= y_basis[i];
                }
            }
            if slice & 1 != 0 {
                inner ^= if version_four { 0x400 } else { 0x800 };
            }
            if slice & 2 != 0 {
                inner ^= if version_four { 0x200 } else { 0x400 };
            }
            if slice & 4 != 0 && !version_four {
                inner ^= 0x200;
            }
            if mip.swizzle == 3 {
                let origins: &[u64] = match (self.element_bytes, version_four) {
                    (1, true) => &[
                        0x4000, 0x8000, 0x1100, 0x2100, 0xc00, 0x600, 0x700, 0x400, 0x100,
                    ],
                    (2, true) => &[0x8000, 0x4100, 0x2100, 0x400, 0x1400, 0x100, 0xb00, 0x300],
                    (4, true) => &[0x4100, 0x8100, 0x2400, 0x400, 0x200, 0xb00, 0x1300, 0x300],
                    (8, true) => &[0x8100, 0x400, 0x4400, 0x300, 0x200, 0x3000, 0x2800],
                    (16, true) => &[0x8400, 0x400, 0x200, 0x300, 0x2000, 0x4800, 0x5000],
                    (16, false) => &[0x800, 0x400, 0x200, 0x300, 0x1000, 0xa000, 0xc000],
                    (8, false) => &[0x1400, 0x400, 0x800, 0x300, 0x200, 0xc000, 0xa000],
                    (4, false) => &[0x1400, 0x8800, 0x800, 0x400, 0x200, 0x2300, 0x4300, 0x300],
                    (2, false) => &[0x8100, 0x4800, 0x1400, 0x400, 0x800, 0x100, 0x2300, 0x300],
                    (1, false) => &[
                        0x4100, 0x8100, 0x1400, 0x2800, 0x800, 0x600, 0x700, 0x400, 0x100,
                    ],
                    _ => return None,
                };
                return Some(
                    u64::from(slice) * self.slice_bytes
                        + (origins.get((level - self.unpacked_mips) as usize)? ^ inner),
                );
            }
            let tile = (y / th) * (mip.pitch_elements / tw) + x / tw;
            return Some(origin + u64::from(tile) * 65536 + inner);
        }
        // These are byte-address bit positions for the X and Y element coordinates
        // within one 4 KiB tile. Compressed formats address 4x4 blocks.
        let (tile_width, tile_height, x_bits, y_bits): (u32, u32, &[u8], &[u8]) =
            match self.element_bytes {
                1 => (64, 64, &[0, 1, 2, 3, 9, 11], &[4, 5, 6, 7, 8, 10]),
                2 => (64, 32, &[1, 2, 3, 7, 9, 11], &[4, 5, 6, 8, 10]),
                4 => (32, 32, &[2, 3, 7, 9, 11], &[4, 5, 6, 8, 10]),
                8 => (32, 16, &[3, 6, 7, 9, 11], &[4, 5, 8, 10]),
                16 => (16, 16, &[6, 7, 9, 11], &[4, 5, 8, 10]),
                _ => return None,
            };
        let tile = (y / tile_height) * (mip.pitch_elements / tile_width) + x / tile_width;
        Some(origin + u64::from(tile * 4096 + scatter(x, x_bits) + scatter(y, y_bits)))
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
