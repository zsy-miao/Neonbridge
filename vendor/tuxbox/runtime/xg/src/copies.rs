//! CPU transfers between linear client data and one XG texture subresource.
use super::Computer;
use core::ptr;
use windows_sys::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, S_OK};

#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "system" fn into(
    this: &Computer,
    texture: *mut u8,
    plane: u32,
    subresource: u32,
    source: *const u8,
    row: u32,
    slice: u32,
) -> i32 {
    unsafe {
        transfer(
            this,
            plane,
            subresource,
            texture,
            source.cast_mut(),
            row,
            slice,
            0,
            true,
        )
    }
}

// The fifth argument is the sample index. In particular, the source address
// and both pitches occupy different slots from the write method.
#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "system" fn from(
    this: &Computer,
    dest: *mut u8,
    plane: u32,
    subresource: u32,
    sample: u32,
    source: *const u8,
    row: u32,
    slice: u32,
) -> i32 {
    if sample
        >= this
            .depth_stencil
            .map_or_else(|| this.plane.color_samples(), |depth| depth.samples())
    {
        return E_INVALIDARG;
    }
    unsafe {
        transfer(
            this,
            plane,
            subresource,
            source.cast_mut(),
            dest,
            row,
            slice,
            sample,
            false,
        )
    }
}

#[allow(clippy::too_many_arguments)]
unsafe fn transfer(
    this: &Computer,
    plane: u32,
    subresource: u32,
    texture: *mut u8,
    linear: *mut u8,
    row_pitch: u32,
    slice_pitch: u32,
    sample: u32,
    upload: bool,
) -> i32 {
    let mut base = 0;
    let stencil;
    let surface = if let Some(depth) = this.depth_stencil {
        let Some(plane_start) = depth.base(plane) else {
            return E_INVALIDARG;
        };
        // XG uploads into the full allocation; download callers pass the
        // start of the selected plane as the source.
        if upload {
            base = plane_start;
        }
        if plane == depth.depth_index() {
            &this.plane
        } else if plane == depth.depth_index() + 1 {
            stencil = depth.data_plane(true);
            &stencil
        } else {
            return E_NOTIMPL;
        }
    } else if plane != 0 {
        return if plane == 1 && this.color_metadata != 0 {
            E_NOTIMPL
        } else {
            E_INVALIDARG
        };
    } else {
        &this.plane
    };
    if texture.is_null() || linear.is_null() {
        return E_INVALIDARG;
    }
    let mip = subresource % surface.mip_count();
    let layer = subresource / surface.mip_count();
    if (this.dimension == 4 && layer != 0) || layer >= surface.array_size() {
        return E_INVALIDARG;
    }
    let Some((width, height, depth, row_linear)) = surface.mip_extent(mip) else {
        return E_INVALIDARG;
    };
    let depth = if this.dimension == 4 { depth } else { 1 };
    let bpe = surface.element_bytes() as usize;
    let row_bytes = width as usize * bpe;
    let row_pitch = row_pitch as usize;
    let slice_pitch = slice_pitch as usize;
    let row_end = u64::from(height - 1) * row_pitch as u64 + row_bytes as u64;
    if (height > 1 && row_pitch < row_bytes) || (depth > 1 && (slice_pitch as u64) < row_end) {
        return E_INVALIDARG;
    }
    let copy_width = if row_linear { width } else { 1 };
    for z in 0..depth {
        for y in 0..height {
            let row_start = z as usize * slice_pitch + y as usize * row_pitch;
            for x in (0..width).step_by(copy_width as usize) {
                let offset = if let Some(depth) = this.depth_stencil {
                    depth
                        .offset(plane, x.into(), y, layer)
                        .map(|offset| offset + u64::from(sample) * bpe as u64)
                } else if surface.color_samples() > 1 {
                    surface.color_sample_offset(layer, x.into(), y, sample)
                } else if this.dimension == 4 {
                    surface.element_offset_in_volume(z, mip, x.into(), y)
                } else {
                    surface.element_offset_for_hardware(layer, mip, x.into(), y, this.version_four)
                };
                let Some(offset) = offset else {
                    return E_INVALIDARG;
                };
                let count = copy_width as usize * bpe;
                if offset + count as u64 > surface.size {
                    return E_INVALIDARG;
                }
                // The caller supplies disjoint backing buffers covering the layout
                // and pitched subresource. The plane equation bounds each tiled span.
                unsafe {
                    let tiled = texture.add((base + offset) as usize);
                    let cpu = linear.add(row_start + x as usize * bpe);
                    if upload {
                        ptr::copy_nonoverlapping(cpu, tiled, count);
                    } else {
                        ptr::copy_nonoverlapping(tiled, cpu, count);
                    }
                }
            }
        }
    }
    S_OK
}
