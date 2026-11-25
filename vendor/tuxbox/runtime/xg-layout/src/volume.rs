//! Layouts of volumes.
use super::*;

impl Plane {
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
}
