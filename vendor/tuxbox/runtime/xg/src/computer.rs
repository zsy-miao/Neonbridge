//! Building a layout computer from a descriptor.
use super::*;

/// # Safety
/// `out` must be a valid writable pointer, disjoint from the new object.
pub(super) unsafe fn allocate_computer(
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

pub(super) fn log_rejected_desc(result: i32, desc: &Desc) {
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

pub(super) unsafe fn create_inner(desc: *const Desc, out: *mut *mut Computer) -> i32 {
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
