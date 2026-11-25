//! Send Xbox BVH builds to the host DXR driver via the guest D3D12 interface.
//! Hardware acceleration structures must be built for the GPU that traverses them.
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{HRESULT, IUnknown, Interface};

type Object = *mut c_void;
type Build = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC;
type Post = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC;
type Inputs = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS;
type Sizes = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO;

#[repr(C)]
struct Builder {
    device: Option<IUnknown>,
    morton64: bool,
    reserved: [u8; 15],
}
#[repr(C)]
struct Entry {
    build: Build,
    extra: [u64; 4],
}
const _: () = assert!(size_of::<Builder>() == 24 && size_of::<Entry>() == 80);

// BuildRaytracingAccelerationStructure and CopyRaytracingAccelerationStructure
// in the Series X|S command-list table, which this library's callers use.
const BUILD_SLOT: usize = 119;
const COPY_SLOT: usize = 121;

// The Xbox COM vtable is distinct from the desktop interface. This provider
// calls the same public slots as an Xbox title, preserving command-list errors.
unsafe fn slot(object: Object, index: usize) -> *const c_void {
    unsafe { *(*object.cast::<*const *const c_void>()).add(index) }
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_construct(
    this: *mut Builder,
    device: Object,
    _driver_only: bool,
) -> *mut Builder {
    let owner = unsafe { IUnknown::from_raw_borrowed(&device) }.cloned();
    unsafe {
        this.write(Builder {
            device: owner,
            morton64: false,
            reserved: [0; 15],
        })
    };
    this
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_destroy(this: *mut Builder) {
    unsafe { this.drop_in_place() };
}

#[unsafe(no_mangle)]
extern "system" fn tuxbox_bvh_morton(this: &mut Builder, enabled: bool) {
    // The native DXR builder owns its sorting strategy.
    this.morton64 = enabled;
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_prebuild(
    this: &Builder,
    inputs: *const Inputs,
    stream: *const u64,
    out: *mut Sizes,
) {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return;
    };
    *out = Sizes::default();
    let Some(device) = &this.device else { return };
    if !stream.is_null() && unsafe { stream.read_unaligned() } != 0 {
        return;
    }
    let device = device.as_raw();
    let query: unsafe extern "system" fn(Object, *const Inputs, *mut Sizes) =
        unsafe { core::mem::transmute(slot(device, 102)) };
    unsafe { query(device, inputs, out) };
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_build(
    _this: &Builder,
    list: Object,
    desc: *const Build,
    count: u32,
    post: *const Post,
) {
    let record: unsafe extern "system" fn(Object, *const Build, u32, *const Post) =
        unsafe { core::mem::transmute(slot(list, BUILD_SLOT)) };
    unsafe { record(list, desc, count, post) };
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_batch(
    this: &Builder,
    list: Object,
    count: u32,
    entries: *const Entry,
) {
    if count == 0 || entries.is_null() {
        return;
    }
    let entries = unsafe { core::slice::from_raw_parts(entries, count as usize) };
    for entry in entries {
        // `extra` holds the postbuild count and array, then the size and
        // address of an Xbox extension stream.
        let [post_count, post, stream_bytes, stream] = entry.extra;
        if stream_bytes != 0 && stream != 0 {
            // Desktop DXR cannot express the stream. A missing description
            // makes the provider skip and report this one build.
            unsafe { tuxbox_bvh_build(this, list, core::ptr::null(), 0, core::ptr::null()) };
            continue;
        }
        let post_count = post_count as u32;
        let post = if post_count == 0 { 0 } else { post };
        unsafe {
            tuxbox_bvh_build(this, list, &entry.build, post_count, post as usize as *const Post)
        };
    }
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_copy(
    _this: &Builder,
    list: Object,
    dst: u64,
    src: u64,
    mode: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE,
) {
    let record: unsafe extern "system" fn(
        Object,
        u64,
        u64,
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE,
    ) = unsafe { core::mem::transmute(slot(list, COPY_SLOT)) };
    unsafe { record(list, dst, src, mode) };
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_rehydrate2(
    _this: &Builder,
    list: Object,
    count: u32,
    builds: *const Build,
    _addresses: *const u64,
) -> HRESULT {
    if count == 0 {
        return HRESULT(0);
    }
    if list.is_null() || builds.is_null() {
        return E_INVALIDARG;
    }
    // Cooked Xbox node records cannot be submitted as native serialized DXR.
    E_NOTIMPL
}

#[unsafe(no_mangle)]
unsafe extern "system" fn tuxbox_bvh_rehydrate(
    this: &Builder,
    list: Object,
    count: u32,
    builds: *const Build,
) -> HRESULT {
    unsafe { tuxbox_bvh_rehydrate2(this, list, count, builds, core::ptr::null()) }
}
