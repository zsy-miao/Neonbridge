//! One, two and three dimensional texture computers.
use super::*;

pub(super) fn texture_2d_plane(desc: &Texture2DDesc) -> Result<Plane, i32> {
    // Restrict support to the two usage/swizzle combinations compared against
    // the supplied Xbox library. Other combinations are not equivalent to
    // blindly substituting a D3D12 resource descriptor.
    if !matches!((desc.usage, desc.swizzle), (3, 0) | (0 | 1, 5))
        || desc.bind_flags != 8
        || desc.cpu_access != 0
        || desc.misc_flags != 0
        || desc.pitch != 0
        || desc.samples != 1
        || desc.quality != 0
        || desc.mips == 0
    {
        return Err(E_NOTIMPL);
    }
    let mips = u16::try_from(desc.mips).map_err(|_| E_INVALIDARG)?;
    let slices = u16::try_from(desc.slices).map_err(|_| E_INVALIDARG)?;
    Plane::compute(&Desc {
        dimension: 3,
        alignment: 0,
        width: u64::from(desc.width),
        height: desc.height,
        slices,
        mips,
        format: desc.format,
        samples: 1,
        quality: 0,
        layout: 256 + desc.swizzle,
        flags: 0,
    })
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable layout. Unsupported descriptor combinations return an error.
#[unsafe(export_name = "XGComputeTexture2DLayout")]
pub(super) unsafe extern "system" fn compute_texture_2d(desc: *const Texture2DDesc, out: *mut Layout) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    library::initialize();
    let plane = match texture_2d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { write_texture_layout(out, plane, 3) };
    S_OK
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable output pointer. Release a successful object through slot 1.
#[unsafe(export_name = "XGCreateTexture2DComputer")]
pub(super) unsafe extern "system" fn create_texture_2d(
    desc: *const Texture2DDesc,
    out: *mut *mut Computer,
) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    library::initialize();
    let plane = match texture_2d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { allocate_computer(plane, 3, 0, descriptor::Image::Typed((*desc).format), out) }
}

pub(super) fn texture_1d_plane(desc: &Texture1DDesc) -> Result<Plane, i32> {
    // Native controls establish this linear, shader-resource subset only.
    // Xbox 1D textures reject block-compressed formats; do not reuse the 2D
    // implementation for those or for unmeasured usage and swizzle modes.
    if !matches!(desc.usage, 0 | 1 | 3)
        || desc.bind_flags != 8
        || desc.cpu_access != 0
        || desc.misc_flags != 0
        || desc.swizzle != 0
        || desc.pitch != 0
        || desc.mips == 0
        || (70..=84).contains(&desc.format)
        || (94..=99).contains(&desc.format)
    {
        return Err(E_NOTIMPL);
    }
    let mips = u16::try_from(desc.mips).map_err(|_| E_INVALIDARG)?;
    let slices = u16::try_from(desc.slices).map_err(|_| E_INVALIDARG)?;
    Plane::compute(&Desc {
        dimension: 3,
        alignment: 0,
        width: u64::from(desc.width),
        height: 1,
        slices,
        mips,
        format: desc.format,
        samples: 1,
        quality: 0,
        layout: 256,
        flags: 0,
    })
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable layout. Unsupported descriptor combinations return an error.
#[unsafe(export_name = "XGComputeTexture1DLayout")]
pub(super) unsafe extern "system" fn compute_texture_1d(desc: *const Texture1DDesc, out: *mut Layout) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    library::initialize();
    let plane = match texture_1d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { write_texture_layout(out, plane, 2) };
    S_OK
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable output pointer. Release a successful object through slot 1.
#[unsafe(export_name = "XGCreateTexture1DComputer")]
pub(super) unsafe extern "system" fn create_texture_1d(
    desc: *const Texture1DDesc,
    out: *mut *mut Computer,
) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    library::initialize();
    let plane = match texture_1d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { allocate_computer(plane, 2, 0, descriptor::Image::Typed((*desc).format), out) }
}

pub(super) fn texture_3d_plane(desc: &Texture3DDesc) -> Result<Plane, i32> {
    // Linear color volumes and bounded 4 KiB volume layouts.
    if !matches!((desc.usage, desc.swizzle), (0 | 1 | 3, 0) | (0 | 1, 5))
        || desc.bind_flags != 8
        || desc.cpu_access != 0
        || desc.misc_flags != 0
        || desc.pitch != 0
        || desc.mips == 0
        || (desc.swizzle == 0 && matches!(desc.format, 70..=84 | 94..=99))
    {
        return Err(E_NOTIMPL);
    }
    if desc.width > 2048 || desc.height > 2048 || desc.depth > 2048 {
        return Err(E_NOTIMPL);
    }
    // The existing 2D plane calculator determines levels from width/height;
    // depth-only mip chains need their own validated implementation.
    if desc.width != 0 && desc.height != 0 && desc.mips > 1 + desc.width.max(desc.height).ilog2() {
        return Err(E_NOTIMPL);
    }
    let mips = u16::try_from(desc.mips).map_err(|_| E_INVALIDARG)?;
    let depth = u16::try_from(desc.depth).map_err(|_| E_INVALIDARG)?;
    let base = Desc {
        dimension: 3,
        alignment: 0,
        width: u64::from(desc.width),
        height: desc.height,
        slices: depth,
        mips,
        format: desc.format,
        samples: 1,
        quality: 0,
        layout: 256 + desc.swizzle,
        flags: 0,
    };
    if desc.swizzle == 0 {
        Plane::compute_linear_volume(&base)
    } else {
        Plane::compute_4k_volume(&base)
    }
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable layout. Unsupported descriptor combinations return an error.
#[unsafe(export_name = "XGComputeTexture3DLayout")]
pub(super) unsafe extern "system" fn compute_texture_3d(desc: *const Texture3DDesc, out: *mut Layout) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    library::initialize();
    let plane = match texture_3d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { write_texture_layout(out, plane, 4) };
    S_OK
}

/// # Safety
/// Non-null pointers must reference a readable descriptor and a disjoint,
/// writable output pointer. Release a successful object through slot 1.
#[unsafe(export_name = "XGCreateTexture3DComputer")]
pub(super) unsafe extern "system" fn create_texture_3d(
    desc: *const Texture3DDesc,
    out: *mut *mut Computer,
) -> i32 {
    let _guard = library::Guard::acquire();
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    unsafe { out.write(ptr::null_mut()) };
    library::initialize();
    let plane = match texture_3d_plane(unsafe { &*desc }) {
        Ok(plane) => plane,
        Err(hr) => return hr,
    };
    unsafe { allocate_computer(plane, 4, 0, descriptor::Image::Typed((*desc).format), out) }
}
