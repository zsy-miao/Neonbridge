//! Multisampled 32-bit render targets with 64 KiB color tiles (swizzle 27).
use super::{Desc, Plane};
use windows_sys::Win32::Foundation::E_NOTIMPL;

impl Plane {
    pub(super) fn multisampled_color(desc: &Desc) -> Result<Self, i32> {
        if !matches!(
            desc.flags,
            0x100001 | 0x100005 | 0x200001 | 0x200005 | 0x300001 | 0x300005
        ) {
            return Err(E_NOTIMPL);
        }
        let (tile_width, tile_height) = tile_shape(desc.samples).ok_or(E_NOTIMPL)?;
        let mut single = *desc;
        single.samples = 1;
        let mut plane = Self::single_64k_plane(&single, 27)?;
        if plane.element_bytes != 4 {
            return Err(E_NOTIMPL);
        }
        let mip = &mut plane.mips[0];
        let pitch = mip.width_texels.next_multiple_of(tile_width);
        let rows = mip.height_texels.next_multiple_of(tile_height);
        let bytes = u64::from(pitch) * u64::from(rows) * 4 * u64::from(desc.samples);
        plane.slice_bytes = bytes;
        plane.size = bytes * u64::from(desc.slices);
        mip.size = bytes;
        mip.slice_bytes = bytes;
        mip.pitch_texels = pitch;
        mip.pitch_bytes = pitch * 4;
        mip.pitch_elements = pitch;
        mip.padded_rows = rows;
        mip.slices = desc.samples;
        Ok(plane)
    }

    pub fn color_samples(&self) -> u32 {
        if self.mips[0].swizzle == 27 {
            self.mips[0].slices
        } else {
            1
        }
    }

    pub fn multisample_tile(&self) -> Option<(u32, u32)> {
        // D3D tiled-resource queries report the standard tile orientation.
        // The hardware swizzle uses the transposed shape for 2x and 8x.
        tile_shape(self.color_samples()).map(|(width, height)| (height, width))
    }

    /// XG accepts coordinates outside the visible rectangle. The tile index
    /// uses the padded pitch, while bank selection depends on higher XY bits.
    pub fn color_sample_offset(&self, layer: u32, x: u64, y: u32, sample: u32) -> Option<u64> {
        let mip = &self.mips[0];
        let count = self.color_samples();
        let (w, h) = tile_shape(count)?;
        if layer >= self.array_size() || sample >= count {
            return None;
        }
        let v4 = mip.reserved[1] == 0xe000;
        let (xb, yb, sb, zb): (&[u16], &[u16], &[u16], &[u16]) = match (v4, count) {
            (false, 2) => (X, Y, &[0x1000], &[0x800, 0x400, 0x200]),
            (false, 4) => (X, Y, &[0x8000, 0x1000], &[0x800, 0x400, 0x200]),
            (false, 8) => (X, Y, &[0x8000, 0x1000, 0x800], &[0x400, 0x200]),
            (true, 2) => (
                &[4, 8, 0x40, 0x800, 0x200, 0x2400, 0x8100, 0x4000],
                &[
                    0x10, 0x20, 0x80, 0x1000, 0x300, 0x400, 0x4100, 0x8000, 0x2000,
                ],
                &[0x8000],
                &[0x400, 0x200],
            ),
            (true, 4) => (
                &[4, 8, 0x40, 0x800, 0x200, 0x2400, 0x4100, 0x8000],
                &[
                    0x10, 0x20, 0x80, 0x1000, 0x300, 0x400, 0x8100, 0x4000, 0x2000,
                ],
                &[0x4000, 0x8000],
                &[0x400, 0x200],
            ),
            (true, 8) => (
                &[4, 8, 0x40, 0x800, 0x200, 0x2400, 0x8100, 0x4000],
                &[
                    0x10, 0x20, 0x80, 0x1000, 0x300, 0x400, 0x4100, 0x8000, 0x2000,
                ],
                &[0x2000, 0x4000, 0x8000],
                &[0x400, 0x200],
            ),
            _ => return None,
        };
        let x = x as u32;
        let tile = u64::from(y / h) * u64::from(mip.pitch_elements / w) + u64::from(x / w);
        let inner = bank(x, xb) ^ bank(y, yb) ^ bank(layer, zb) ^ bank(sample, sb);
        Some(u64::from(layer) * self.slice_bytes + tile * 65536 + u64::from(inner))
    }
}

const X: &[u16] = &[4, 8, 0x40, 0x2000, 0x200, 0x800, 0x8400, 0x100, 0x1000];
const Y: &[u16] = &[
    0x10, 0x20, 0x80, 0x4000, 0x300, 0x400, 0x8800, 0x100, 0x1000,
];

fn tile_shape(samples: u32) -> Option<(u32, u32)> {
    match samples {
        2 => Some((64, 128)),
        4 => Some((64, 64)),
        8 => Some((32, 64)),
        _ => None,
    }
}

fn bank(value: u32, basis: &[u16]) -> u16 {
    basis.iter().enumerate().fold(0, |mask, (i, bit)| {
        if value & (1 << i) == 0 {
            mask
        } else {
            mask ^ bit
        }
    })
}
