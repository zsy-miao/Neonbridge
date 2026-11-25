//! The x64 texture address computer used by XGCreateTextureComputer.

mod swizzle;
mod textures;
mod addressing;
mod computer;
use textures::texture_3d_plane;
use addressing::coordinates;
use addressing::element_offset;
use computer::allocate_computer;
use computer::log_rejected_desc;
use computer::create_inner;

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

fn has_stencil(format: u32) -> bool {
    matches!(format, 19 | 20 | 22 | 44 | 45 | 47 | 62 | 118 | 119 | 120)
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
