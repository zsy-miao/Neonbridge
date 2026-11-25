//! Layouts of tiled surfaces.
use super::*;

impl Plane {
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

    pub(super) fn tiled_64k_mips(desc: &Desc, swizzle: u32) -> Result<Self, i32> {
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

    pub(super) fn single_64k_plane(desc: &Desc, swizzle: u32) -> Result<Self, i32> {
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
}
