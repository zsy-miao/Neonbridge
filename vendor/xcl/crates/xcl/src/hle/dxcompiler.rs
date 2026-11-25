//! `dxcompiler_xs.dll`: the console's DXC. Its factory is the public one, so titles get the PC
//! compiler XCL uses for its own translations (DXIL out; pipelines from PC shaders pass through).

#![allow(non_snake_case)]

use std::ffi::c_void;

use crate::util::HRESULT;

const E_FAIL: HRESULT = 0x8000_4005u32 as i32;

type CreateInstance = unsafe extern "system" fn(*const c_void, *const c_void, *mut *mut c_void) -> HRESULT;

pub fn exports() -> Vec<(&'static str, usize)> {
    crate::hle_exports![DxcCreateInstance, DxcCreateInstance2]
}

pub unsafe extern "system" fn DxcCreateInstance(clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    match crate::gpu::recomp::dxc::create_instance() {
        Some(f) => std::mem::transmute::<usize, CreateInstance>(f)(clsid, iid, out),
        None => {
            if !out.is_null() {
                *out = std::ptr::null_mut();
            }
            crate::xlog_warn!("dxcompiler_xs: DxcCreateInstance – no DXC on this PC (XCL_DXC_DIR)");
            E_FAIL
        }
    }
}

/// `DxcCreateInstance2(IMalloc*, clsid, iid, out)`: the allocator is the PC compiler's own
pub unsafe extern "system" fn DxcCreateInstance2(_malloc: *mut c_void, clsid: *const c_void, iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    DxcCreateInstance(clsid, iid, out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_console_compiler_is_the_pc_one() {
        use windows::core::Interface;
        use windows::Win32::Graphics::Direct3D::Dxc::{IDxcCompiler3, CLSID_DxcCompiler};
        if crate::gpu::recomp::dxc::location().is_err() {
            return;
        }
        let mut p = std::ptr::null_mut();
        let hr = unsafe { super::DxcCreateInstance(&CLSID_DxcCompiler as *const _ as _, &IDxcCompiler3::IID as *const _ as _, &mut p) };
        assert!(hr >= 0 && !p.is_null(), "{hr:#x}");
        drop(unsafe { IDxcCompiler3::from_raw(p) });
        let (m, _) = crate::hle::registry().find_any("DxcCreateInstance").unwrap();
        assert_eq!(m.name, crate::hle::DXCOMPILER);
    }
}
