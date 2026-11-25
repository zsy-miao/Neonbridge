//! Where an element of a surface is stored.
use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "system" fn coordinates(
    this: &Computer,
    offset: u64,
    plane: *mut u32,
    mip: *mut u32,
    x: *mut u64,
    y: *mut u32,
    slice: *mut u32,
    sample: *mut u32,
) -> i32 {
    // HTILE is resolved as a single metadata subresource by the driver.
    let result = if this.depth_stencil.is_some_and(|d| offset < d.htile_bytes()) {
        Some((0, 0, 0, 0))
    } else {
        this.plane.linear_coordinate(offset)
    };
    let Some((level, column, row, layer)) = result else {
        return E_INVALIDARG;
    };
    unsafe {
        if !plane.is_null() {
            plane.write(0);
        }
        if !mip.is_null() {
            mip.write(level);
        }
        if !x.is_null() {
            x.write(column);
        }
        if !y.is_null() {
            y.write(row);
        }
        if !slice.is_null() {
            slice.write(layer);
        }
        if !sample.is_null() {
            sample.write(0);
        }
    }
    S_OK
}

pub(super) unsafe extern "system" fn element_offset(
    this: &Computer,
    plane: u32,
    mip: u32,
    x: u64,
    y: u32,
    slice: u32,
    sample: u32,
    bit: *mut u32,
) -> u64 {
    if let Some(depth) = this.depth_stencil {
        let Some((sample_offset, address)) = depth
            .query_sample(plane, sample)
            .zip(depth.offset(plane, x, y, slice))
        else {
            return u64::MAX;
        };
        if mip != 0 {
            return u64::MAX;
        }
        if !bit.is_null() {
            unsafe { bit.write(0) };
        }
        return address + sample_offset;
    }
    let color_samples = this.plane.color_samples();
    if color_samples > 1 {
        if sample >= color_samples {
            return u64::MAX;
        }
        if !bit.is_null() {
            unsafe { bit.write(0) };
        }
        return if plane == 0 && mip == 0 {
            this.plane.color_sample_offset(slice, x, y, sample).unwrap_or(u64::MAX)
        } else {
            u64::MAX
        };
    }
    if !bit.is_null() {
        // Color elements address whole bytes; metadata may use individual bits.
        unsafe { bit.write(0) };
    }
    if plane == 1
        && this.color_metadata != 0
        && sample == 0
        && mip == 0
        && slice < this.plane.array_size()
    {
        if x >= 16384 || y >= 16384 {
            return u64::MAX;
        }
        let (xb, yb): (&[u64], &[u64]) = if this.version_four {
            match (this.color_metadata, this.plane.element_bytes()) {
                (1, _) => (
                    &[0, 0, 0, 4, 0x1000, 0x2010, 0x820, 0x80, 0x200, 0x4000],
                    &[0, 0, 0, 8, 0x1800, 0x2000, 0x840, 0x100, 0x400, 0],
                ),
                (_, 1) => (
                    &[0, 0, 0, 0, 0x1000, 0x2008, 0x810, 0x40, 0x100, 0x400],
                    &[0, 0, 0, 0, 0x1800, 0x2000, 0x820, 0x80, 0x200, 0x4000],
                ),
                (_, 2) => (
                    &[0, 0, 0, 0, 0x1000, 0x2010, 0x820, 0x80, 0x200, 0x4000],
                    &[0, 0, 0, 8, 0x1800, 0x2000, 0x840, 0x100, 0x400, 0],
                ),
                (_, 4) => (
                    &[0, 0, 0, 8, 0x1000, 0x2020, 0x840, 0x100, 0x400],
                    &[0, 0, 0, 0x10, 0x1800, 0x2000, 0x880, 0x200, 0x4000],
                ),
                (_, 8) => (
                    &[0, 0, 0, 0x10, 0x1000, 0x2040, 0x880, 0x200, 0x4000],
                    &[0, 0, 8, 0x20, 0x1800, 0x2000, 0x900, 0x400, 0],
                ),
                (_, 16) => (
                    &[0, 0, 8, 0x20, 0x1000, 0x2080, 0x900, 0x400],
                    &[0, 0, 0x10, 0x40, 0x1800, 0x2000, 0xa00, 0x4000],
                ),
                _ => return u64::MAX,
            }
        } else if this.color_metadata == 2 {
            match this.plane.element_bytes() {
                16 => (
                    &[0, 0, 8, 0x8000, 0x1000, 0x4000, 0x2040, 0x900, 0x8400, 0],
                    &[0, 0, 0x10, 0x20, 0x1800, 0x2000, 0x4080, 0xa00, 0x8000, 0],
                ),
                8 => (
                    &[0, 0, 0, 0x10, 0x1000, 0x4000, 0xa000, 0x880, 0x8200, 0],
                    &[0, 0, 8, 0x20, 0x1800, 0x2000, 0x4040, 0x900, 0x8400, 0],
                ),
                4 => (
                    &[0, 0, 0, 8, 0x1000, 0x4000, 0xa000, 0x840, 0x8100, 0x400],
                    &[0, 0, 0, 0x10, 0x1800, 0x2000, 0x4020, 0x880, 0x8200, 0],
                ),
                2 => (
                    &[0, 0, 0, 0, 0x1000, 0x4000, 0xa000, 0x820, 0x8080, 0x200],
                    &[0, 0, 0, 8, 0x1800, 0x2000, 0x4010, 0x840, 0x8100, 0x400],
                ),
                1 => (
                    &[0, 0, 0, 0, 0x1000, 0x4000, 0xa000, 0x810, 0x8040, 0x100, 0x400],
                    &[0, 0, 0, 0, 0x1800, 0x2000, 0x4008, 0x820, 0x8080, 0x200, 0],
                ),
                _ => return u64::MAX,
            }
        } else {
            (
                &[0, 0, 0, 4, 0x1000, 0x4000, 0xa000, 0x820, 0x80, 0x200],
                &[0, 0, 0, 8, 0x1800, 0x2000, 0xc010, 0x840, 0x100, 0x400],
            )
        };
        let mut bits = 0u64;
        for i in 0..xb.len() {
            if x & (1 << i) != 0 {
                bits ^= xb[i];
            }
            if y & (1 << i) != 0 {
                bits ^= yb[i];
            }
        }
        let (tw, th, block_bytes) = this.plane.metadata_block(this.color_metadata);
        let (width, height, _) = this.plane.image_extent();
        let stride = u64::from(width.div_ceil(tw));
        let block_bits = block_bytes * 8;
        bits += (u64::from(y / th) * stride + x / u64::from(tw)) * block_bits;
        let wide = this.color_metadata == 2 && !this.version_four;
        if slice & 1 != 0 {
            bits ^= if wide { 0x4000 } else { 0x2000 };
        }
        if slice & 2 != 0 {
            bits ^= if wide { 0x2000 } else { 0x1000 };
        }
        if slice & 4 != 0 && wide {
            bits ^= 0x1000;
        }
        bits += u64::from(slice) * stride * u64::from(height.div_ceil(th)) * block_bits;
        if !bit.is_null() {
            unsafe { bit.write((bits % 8) as u32) };
        }
        return bits / 8;
    }
    if plane != 0 || sample != 0 {
        return u64::MAX;
    }
    let offset = if this.dimension == 4 {
        this.plane.element_offset_in_volume(slice, mip, x, y)
    } else {
        this.plane
            .element_offset_for_hardware(slice, mip, x, y, this.version_four)
    };
    offset.unwrap_or(u64::MAX)
}
