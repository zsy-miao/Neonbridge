use core::ffi::c_void;
use windows::Win32::Foundation::{E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{Result, s, w};

type Free = unsafe extern "system" fn(*mut c_void, u64);

pub(super) struct GuestMemory {
    attributes: u64,
    pub(super) address: usize,
    free: Free,
}
impl Drop for GuestMemory {
    fn drop(&mut self) {
        unsafe { (self.free)(self.address as _, self.attributes) };
    }
}

impl GuestMemory {
    pub(super) fn allocate(bytes: usize, attributes: u64) -> Result<Self> {
        type Alloc = unsafe extern "system" fn(usize, u64) -> *mut c_void;
        unsafe {
            let module = GetModuleHandleW(w!("xmem.dll"))?;
            let alloc: Alloc =
                core::mem::transmute(GetProcAddress(module, s!("XMemAlloc")).ok_or(E_NOTIMPL)?);
            let free: Free =
                core::mem::transmute(GetProcAddress(module, s!("XMemFree")).ok_or(E_NOTIMPL)?);
            let address = alloc(bytes, attributes).addr();
            if address == 0 {
                return Err(E_OUTOFMEMORY.into());
            }
            Ok(Self {
                address,
                free,
                attributes,
            })
        }
    }
}
