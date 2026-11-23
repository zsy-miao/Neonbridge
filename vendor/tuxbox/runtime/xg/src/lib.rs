//! The x64 texture address computer used by XGCreateTextureComputer.

mod copies;
mod dcc;
mod descriptor;
mod library;
mod tiling;

use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};
use std::alloc::{Layout as Allocation, alloc};
use tuxbox_xg_layout::{DepthStencil, Desc, Layout, Plane};
use windows_sys::Win32::Foundation::{
    E_FAIL, E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER, S_OK,
};

#[repr(C)]
pub struct Computer {
    methods: &'static Methods,
    refs: AtomicU32,
    version_four: bool,
    plane: Plane,
    dimension: u32,
    color_metadata: u8,
    depth_stencil: Option<DepthStencil>,
    image: descriptor::Image,
}

/// Buffer layout descriptor consumed by the Xbox XG library.
#[repr(C)]
pub struct BufferDesc {
    size: u32,
    reserved_04: u32,
    flags: u32,
    reserved_0c: u32,
    options: u32,
}

#[repr(C)]
pub struct TileShape {
    width: u32,
    height: u32,
    depth: u32,
}

/// Select the Xbox swizzle mode for a resource description.
///
/// The supplied XG build reads an eighth flags argument and does not inspect
/// the dimensions or sample count for this selection.
#[unsafe(export_name = "XGComputeOptimalSwizzleMode")]
extern "system" fn optimal_swizzle_mode(
    dimension: u32,
    format: u32,
    _width: u32,
    _height: u32,
    _depth_or_array: u32,
    _samples: u32,
    bind_flags: u32,
    resource_flags: u32,
) -> u32 {
    if dimension == 1 {
        return 0;
    }
    if bind_flags & 0x40 != 0 {
        let incompatible = has_stencil(format);
        return if resource_flags & 0x0800_8000 != 0 && incompatible {
            u32::MAX
        } else {
            24
        };
    }
    match dimension {
        3 | 4 if resource_flags & 0x0800_0000 != 0 => 10,
        3 | 4 if bind_flags & 0x20 != 0 => 27,
        3 | 4 => 5,
        _ => 0,
    }
}

fn has_stencil(format: u32) -> bool {
    matches!(format, 19 | 20 | 22 | 44 | 45 | 47 | 62 | 118 | 119 | 120)
}

/// Select an explicit Series resource layout. The destination may be the
/// layout field in the input description itself.
#[unsafe(export_name = "XGSuggestSwizzleMode")]
unsafe extern "system" fn suggest_swizzle(
    desc: *const Desc,
    options: u32,
    layout: *mut u32,
) -> i32 {
    if desc.is_null() || layout.is_null() {
        return E_INVALIDARG;
    }
    let resource = unsafe { desc.read_unaligned() };
    if options != 0 {
        return E_NOTIMPL;
    }
    if !(1..=4).contains(&resource.dimension) || resource.width == 0 {
        return E_INVALIDARG;
    }
    let bind_flags = (resource.flags & 3) << 5;
    let swizzle = optimal_swizzle_mode(resource.dimension, resource.format, 0, 0, 0,
        resource.samples, bind_flags, resource.flags);
    if swizzle == u32::MAX {
        return E_INVALIDARG;
    }
    unsafe { layout.write_unaligned(256 + swizzle) };
    S_OK
}

/// A non-null output receives the same selection returned in EAX.
#[unsafe(export_name = "XGComputeOptimalDepthStencilSwizzleMode")]
unsafe extern "system" fn optimal_depth_swizzle(
    format: u32,
    _width: u32,
    _height: u32,
    _depth: u32,
    _mips: u32,
    _samples: u32,
    flag_a: u32,
    flag_b: u32,
    output: *mut u32,
) -> u32 {
    let mode = if (flag_a != 0 || flag_b != 0) && has_stencil(format) {
        u32::MAX
    } else {
        24
    };
    if !output.is_null() {
        unsafe { output.write(mode) };
    }
    mode
}

/// Validate a depth/stencil swizzle mode. This Xbox DLL's supported modes are
/// independent of the supplied format and sample-count parameters.
#[unsafe(export_name = "XGValidateDepthStencilSwizzleMode")]
extern "system" fn validate_depth_stencil_swizzle(
    _format: u32,
    _samples: u32,
    swizzle: u32,
) -> i32 {
    if matches!(swizzle, 4 | 8 | 12 | 16 | 20 | 24 | 28) {
        S_OK
    } else {
        E_FAIL
    }
}

/// Element size and the pixel extent reported by XG for a storage element.
fn tile_format(format: u32) -> Option<(u32, u32, u32)> {
    Some(match format {
        1..=4 => (16, 1, 1),
        // RGB32 uses three 32-bit storage elements per pixel. Native XG
        // applies an integer pixel multiplier, yielding zero pixel width.
        5..=8 => (4, 0, 1),
        9..=22 | 102 => (8, 1, 1),
        23..=47 | 67 | 87..=93 | 100 | 101 | 117 => (4, 1, 1),
        48..=59 | 85 | 86 | 104 | 114 | 115 | 118 | 119 => (2, 1, 1),
        60..=66 | 103 | 105 | 106 | 108..=113 | 116 | 120 => (1, 1, 1),
        68 | 69 | 107 => (4, 2, 1),
        70..=72 | 79..=81 => (8, 4, 4),
        73..=78 | 82..=84 | 94..=99 => (16, 4, 4),
        _ => return None,
    })
}

/// Compute the 64 KiB tile dimensions for supported buffers and 2D formats.
///
/// # Safety
/// Each non-null output must point to a writable `TileShape`.
#[unsafe(export_name = "XGComputeTileShape")]
unsafe extern "system" fn compute_tile_shape(
    dimension: u32,
    format: u32,
    bytes_per_element: u32,
    samples: u32,
    pixels: *mut TileShape,
    elements: *mut TileShape,
) -> i32 {
    if dimension != 3 && samples != 1 {
        return E_INVALIDARG;
    }
    let (element_bytes, pixel_width, pixel_height) = match (dimension, format) {
        (1, 0) if bytes_per_element <= 1 => (1, 0, 0),
        (1, _) => return E_INVALIDARG,
        (2 | 4, _) => return E_NOTIMPL,
        (3, 0) => (bytes_per_element, 0, 0),
        (3, _) => match tile_format(format) {
            Some(format) => format,
            None => return E_NOTIMPL,
        },
        _ => return E_NOTIMPL,
    };
    let (mut width, mut height) = if dimension == 1 {
        (65536, 1)
    } else {
        match element_bytes {
            1 => (256, 256),
            2 => (256, 128),
            4 => (128, 128),
            8 => (128, 64),
            16 => (64, 64),
            _ => return E_INVALIDARG,
        }
    };
    if dimension == 3 {
        match samples {
            1 => (),
            2 => height /= 2,
            4 => {
                width /= 2;
                height /= 2;
            }
            8 => {
                width /= 2;
                height /= 4;
            }
            _ => return E_INVALIDARG,
        }
    }
    // Native writes the element shape first; this also preserves the result
    // when a caller deliberately aliases the two output pointers.
    unsafe {
        if !elements.is_null() {
            elements.write(TileShape {
                width,
                height,
                depth: 1,
            });
        }
        if !pixels.is_null() {
            pixels.write(if format == 0 {
                TileShape {
                    width: 0,
                    height: 0,
                    depth: 0,
                }
            } else {
                TileShape {
                    width: width * pixel_width,
                    height: height * pixel_height,
                    depth: 1,
                }
            });
        }
    }
    S_OK
}

/// Compute a buffer's linear allocation layout.
///
/// # Safety
/// Valid non-null pointers must refer to a readable descriptor and a writable,
/// disjoint `Layout`, respectively.
#[unsafe(export_name = "XGComputeBufferLayout")]
unsafe extern "system" fn compute_buffer_layout(desc: *const BufferDesc, out: *mut Layout) -> i32 {
    if desc.is_null() || out.is_null() {
        return E_INVALIDARG;
    }
    let desc = unsafe { &*desc };
    if desc.size == 0
        || desc.flags & 0xffff_f900 != 0
        || desc.options & 0xf385_6d0a != 0
        || (desc.options & 0x0400_0000 != 0 && desc.size & 0xffff != 0)
    {
        return E_INVALIDARG;
    }
    unsafe {
        out.write_bytes(0, 1);
        (*out).bytes = u64::from(desc.size);
        (*out).alignment = if desc.options & 0x0c00_0000 != 0 {
            65536
        } else {
            4
        };
        (*out).mips = 1;
        (*out).dimension = 1;
    }
    S_OK
}

#[repr(C)]
struct Methods {
    add_ref: unsafe extern "system" fn(*mut Computer) -> u32,
    release: unsafe extern "system" fn(*mut Computer) -> u32,
    layout: unsafe extern "system" fn(&Computer, *mut Layout) -> i32,
    bytes: extern "system" fn(&Computer) -> u64,
    alignment: extern "system" fn(&Computer) -> u64,
    mip_offset: extern "system" fn(&Computer, u32, u32) -> u64,
    offset: unsafe extern "system" fn(&Computer, u32, u32, u64, u32, u32, u32, *mut u32) -> u64,
    coordinate: unsafe extern "system" fn(
        &Computer,
        u64,
        *mut u32,
        *mut u32,
        *mut u64,
        *mut u32,
        *mut u32,
        *mut u32,
    ) -> i32,
    copy_into: unsafe extern "system" fn(&Computer, *mut u8, u32, u32, *const u8, u32, u32) -> i32,
    copy_from:
        unsafe extern "system" fn(&Computer, *mut u8, u32, u32, u32, *const u8, u32, u32) -> i32,
    resource_tiling: unsafe extern "system" fn(
        &Computer,
        *mut u32,
        *mut tiling::PackedMips,
        *mut TileShape,
        *mut u32,
        u32,
        *mut tiling::SubresourceTiling,
    ) -> i32,
    image_descriptor: unsafe extern "system" fn(&Computer, u32, *mut [u32; 8]) -> i32,
    is_sparse: extern "system" fn(&Computer) -> u8,
}

static METHODS: Methods = Methods {
    add_ref,
    release,
    layout: get_layout,
    bytes: resource_bytes,
    alignment: resource_alignment,
    mip_offset,
    offset: element_offset,
    coordinate: coordinates,
    copy_into: copies::into,
    copy_from: copies::from,
    resource_tiling: tiling::resource_tiling,
    image_descriptor: descriptor::get,
    is_sparse: tiling::is_sparse,
};

unsafe extern "system" fn add_ref(this: *mut Computer) -> u32 {
    unsafe { (*this).refs.fetch_add(1, Ordering::Relaxed) + 1 }
}

unsafe extern "system" fn release(this: *mut Computer) -> u32 {
    let left = unsafe { (*this).refs.fetch_sub(1, Ordering::AcqRel) } - 1;
    if left == 0 {
        // The final owner reclaims the global allocation made by the factory.
        drop(unsafe { Box::from_raw(this) });
    }
    left
}

extern "system" fn resource_bytes(this: &Computer) -> u64 {
    this.depth_stencil.map_or_else(
        || this.plane.size + this.plane.color_metadata_size(this.color_metadata),
        DepthStencil::size,
    )
}

extern "system" fn resource_alignment(this: &Computer) -> u64 {
    this.plane.alignment
}

extern "system" fn mip_offset(this: &Computer, plane: u32, level: u32) -> u64 {
    if let Some(depth) = this.depth_stencil {
        return if level == 0 {
            depth.base(plane).unwrap_or(u64::MAX)
        } else {
            u64::MAX
        };
    }
    match plane {
        0 => this.plane.mip_offset(level).unwrap_or(u64::MAX),
        1 if this.color_metadata != 0 && level == 0 => this.plane.size,
        _ => u64::MAX,
    }
}

unsafe extern "system" fn get_layout(this: &Computer, out: *mut Layout) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    // The guest supplies one complete layout. Inactive planes and reserved
    // fields have a defined zero representation, irrespective of prior contents.
    unsafe { write_texture_layout(out, this.plane, this.dimension) };
    if let Some(depth) = this.depth_stencil {
        depth.write_planes(unsafe { &mut *out });
    } else if this.color_metadata != 0 {
        unsafe { (*out).append_color_metadata(this.color_metadata) };
    }
    S_OK
}

/// # Safety
/// `out` points to a writable `Layout` disjoint from `plane`.
unsafe fn write_texture_layout(out: *mut Layout, plane: Plane, dimension: u32) {
    unsafe {
        out.write_bytes(0, 1);
        (*out).bytes = plane.size;
        (*out).alignment = plane.alignment;
        (*out).mips = plane.mip_count();
        (*out).plane_count = 1;
        (*out).planes[0] = plane;
        (*out).dimension = dimension;
    }
}

/// Xbox-specific 2D texture descriptor, distinct from a D3D12 resource desc.
#[repr(C)]
pub struct Texture2DDesc {
    width: u32,
    height: u32,
    mips: u32,
    slices: u32,
    format: u32,
    samples: u32,
    quality: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc_flags: u32,
    swizzle: u32,
    pitch: u32,
}

fn texture_2d_plane(desc: &Texture2DDesc) -> Result<Plane, i32> {
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
unsafe extern "system" fn compute_texture_2d(desc: *const Texture2DDesc, out: *mut Layout) -> i32 {
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
unsafe extern "system" fn create_texture_2d(
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

/// Xbox-specific 1D texture descriptor. It differs from the D3D12 descriptor.
#[repr(C)]
pub struct Texture1DDesc {
    width: u32,
    mips: u32,
    slices: u32,
    format: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc_flags: u32,
    swizzle: u32,
    pitch: u32,
}

fn texture_1d_plane(desc: &Texture1DDesc) -> Result<Plane, i32> {
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
unsafe extern "system" fn compute_texture_1d(desc: *const Texture1DDesc, out: *mut Layout) -> i32 {
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
unsafe extern "system" fn create_texture_1d(
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

/// Xbox-specific 3D texture descriptor.
#[repr(C)]
pub struct Texture3DDesc {
    width: u32,
    height: u32,
    depth: u32,
    mips: u32,
    format: u32,
    usage: u32,
    bind_flags: u32,
    cpu_access: u32,
    misc_flags: u32,
    swizzle: u32,
    pitch: u32,
}

fn texture_3d_plane(desc: &Texture3DDesc) -> Result<Plane, i32> {
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
unsafe extern "system" fn compute_texture_3d(desc: *const Texture3DDesc, out: *mut Layout) -> i32 {
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
unsafe extern "system" fn create_texture_3d(
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

/// # Safety
/// `out` must be a valid writable pointer, disjoint from the new object.
unsafe fn allocate_computer(
    mut plane: Plane,
    dimension: u32,
    color_metadata: u8,
    image: descriptor::Image,
    out: *mut *mut Computer,
) -> i32 {
    let version_four = library::version_four();
    if version_four {
        plane.select_version_four();
    }
    let object = unsafe { alloc(Allocation::new::<Computer>()).cast::<Computer>() };
    if object.is_null() {
        return E_OUTOFMEMORY;
    }
    unsafe {
        object.write(Computer {
            methods: &METHODS,
            refs: AtomicU32::new(1),
            version_four,
            plane,
            dimension,
            color_metadata,
            depth_stencil: None,
            image,
        });
        out.write(object);
    }
    S_OK
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn coordinates(
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

unsafe extern "system" fn element_offset(
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

/// Returns an owned texture computer for a supported texture descriptor.
///
/// # Safety
/// Non-null inputs must describe a readable x64 resource descriptor and a
/// disjoint writable output pointer. Release the returned object through slot 1.
#[unsafe(export_name = "XGCreateTextureComputer")]
unsafe extern "system" fn create(desc: *const Desc, out: *mut *mut Computer) -> i32 {
    let _guard = library::Guard::acquire();
    let result = unsafe { create_inner(desc, out) };
    if result < 0
        && let Some(desc) = unsafe { desc.as_ref() }
    {
        log_rejected_desc(result, desc);
    }
    result
}

fn log_rejected_desc(result: i32, desc: &Desc) {
    use std::ffi::OsStr;
    use std::io::Write;

    static FAILURES_LOGGED: AtomicU32 = AtomicU32::new(0);
    let target = std::env::var_os("TUXBOX_LOG");
    let stderr = target.as_deref() == Some(OsStr::new("stderr"));
    if !(stderr
        || target.as_deref() == Some(OsStr::new("1"))
        || std::env::var_os("TUXBOX_XG_TRACE").is_some())
        || FAILURES_LOGGED.fetch_add(1, Ordering::Relaxed) >= 128
    {
        return;
    }
    let mut message = format!(
        "TuxBox XGCreateTextureComputer: hr={result:#010x} dimension={} alignment={} width={} height={} depth_or_array={} mips={} format={} samples={} quality={} layout={} flags={:#x}\n",
        desc.dimension,
        desc.alignment,
        desc.width,
        desc.height,
        desc.slices,
        desc.mips,
        desc.format,
        desc.samples,
        desc.quality,
        desc.layout,
        desc.flags,
    );
    if stderr {
        let _ = std::io::stderr().lock().write_all(message.as_bytes());
    } else {
        message.push('\0');
        unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(message.as_ptr())
        };
    }
}

unsafe fn create_inner(desc: *const Desc, out: *mut *mut Computer) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(ptr::null_mut()) };
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    library::initialize();
    if DepthStencil::supports_flags(desc.flags) {
        if library::version_four() {
            return E_NOTIMPL;
        }
        let depth = match DepthStencil::compute(desc) {
            Ok(depth) => depth,
            Err(hr) => return hr,
        };
        let result = unsafe {
            allocate_computer(
                depth.data_plane(false),
                3,
                0,
                descriptor::Image::Typed(desc.format),
                out,
            )
        };
        if result == S_OK {
            unsafe { (**out).depth_stencil = Some(depth) };
        }
        return result;
    }
    let (plane, dimension) = match desc.dimension {
        2 if matches!(desc.alignment, 0 | 256 | 4096 | 65536)
            && desc.flags & !0x300004 == 0
            && desc.samples == 1
            && desc.quality == 0
            && desc.height == 1
            && matches!(desc.layout, 0 | 256) =>
        {
            let mut linear = *desc;
            linear.dimension = 3;
            linear.alignment = 0;
            linear.flags = 0;
            linear.layout = 256;
            if (70..=84).contains(&desc.format) || (94..=99).contains(&desc.format) {
                return E_INVALIDARG;
            }
            match Plane::compute(&linear) {
                Ok(plane) => (plane, 2),
                Err(hr) => return hr,
            }
        }
        4 => {
            let allowed_flags = if desc.layout == 261 && !matches!(desc.format, 70..=84 | 94..=99) {
                0x300005
            } else { 0x300004 };
            if !(desc.alignment == 0 || (desc.layout == 261 && desc.alignment == 4096))
                || desc.flags & !allowed_flags != 0 || desc.samples != 1 || desc.quality != 0 {
                return E_NOTIMPL;
            }
            let width = match u32::try_from(desc.width) {
                Ok(width) => width,
                Err(_) => return E_NOTIMPL,
            };
            let swizzle = match desc.layout {
                256 => 0,
                261 => 5,
                _ => return E_NOTIMPL,
            };
            let typed = Texture3DDesc {
                width,
                height: desc.height,
                depth: u32::from(desc.slices),
                mips: u32::from(desc.mips),
                format: desc.format,
                usage: if swizzle == 0 { 3 } else { 0 },
                bind_flags: 8,
                cpu_access: 0,
                misc_flags: 0,
                swizzle,
                pitch: 0,
            };
            match texture_3d_plane(&typed) {
                Ok(plane) => (plane, 4),
                Err(hr) => return hr,
            }
        }
        3 if matches!(desc.layout, 257 | 258) => match Plane::compute_microtile(desc) {
            Ok(plane) => (plane, 3),
            Err(hr) => return hr,
        },
        3 if matches!(desc.layout, 265 | 266 | 273 | 274) => match Plane::compute_sparse_64k(desc) {
            Ok(plane) => (plane, 3),
            Err(hr) => return hr,
        },
        3 if desc.layout == 0
            && desc.mips > 1
            && matches!(
                desc.flags,
                1 | 5 | 0x100001 | 0x100005 | 0x200001 | 0x200005 | 0x300001 | 0x300005
            ) =>
        {
            match Plane::compute_64k_mips(desc) {
                Ok(plane) => (plane, 3),
                Err(hr) => return hr,
            }
        }
        3 if desc.layout == 0
            && matches!(
                desc.flags,
                1 | 5
                    | 0x100001
                    | 0x100005
                    | 0x200001
                    | 0x200005
                    | 0x300001
                    | 0x300005
                    | 0xc00001
                    | 0xc00005
            ) =>
        {
            match Plane::compute_64k_color(desc) {
                Ok(plane) => (plane, 3),
                Err(hr) => return hr,
            }
        }
        3 if matches!(desc.layout, 0 | 261) => {
            // Xbox's default 2D color layout is the supported 4 KiB swizzle.
            let mut tiled = *desc;
            tiled.layout = 261;
            // Neither UAV access nor disabled color metadata alters the 4 KiB
            // shader-resource layout, including block-compressed textures.
            tiled.flags &= !0x0030_0004;
            match Plane::compute(&tiled) {
                Ok(plane) => (plane, 3),
                Err(hr) => return hr,
            }
        }
        _ => match Plane::compute(desc) {
            Ok(plane) => (plane, 3),
            Err(hr) => return hr,
        },
    };
    let color_metadata = if desc.mips > 1 {
        0
    } else if matches!(desc.flags, 0xc00001 | 0xc00005) {
        2
    } else if matches!(desc.flags, 1 | 5) && dimension == 3 && desc.layout == 0 {
        1
    } else {
        0
    };
    unsafe {
        allocate_computer(
            plane,
            dimension,
            color_metadata,
            descriptor::Image::Typed(desc.format),
            out,
        )
    }
}

/// Report the optional BVH computer as unavailable until its GPU kernels are supported.
///
/// # Safety
/// A nonnull output must point to a writable interface pointer.
#[unsafe(export_name = "XGCreateBVHComputer2")]
pub unsafe extern "system" fn create_bvh_computer(
    device: *mut core::ffi::c_void,
    out: *mut *mut core::ffi::c_void,
) -> i32 {
    if out.is_null() {
        return E_POINTER;
    }
    unsafe { out.write(core::ptr::null_mut()) };
    if device.is_null() { E_INVALIDARG } else { E_NOTIMPL }
}
