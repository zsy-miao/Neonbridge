//! Addresses of single elements.
use super::*;

impl Plane {
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
