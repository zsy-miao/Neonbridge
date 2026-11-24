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

/// Commits the reserved pages of lazy title views in `[base, base + size)`
/// (xmem lazy_commit.rs, from xcl 38b05b7: views of physical pages commit on
/// first CPU use) and returns how many 64 KiB pages it committed. Memory the
/// CPU never touched stays reserved, so a kernel call (an import, a
/// NtWriteVirtualMemory) fails on it and a commit-state check refuses it:
/// every such use commits first. Pages outside a lazy view are left alone.
pub(crate) fn commit_lazy(base: usize, size: usize) -> usize {
    type Commit = unsafe extern "system" fn(usize, usize) -> usize;
    static COMMIT: std::sync::OnceLock<Option<Commit>> = std::sync::OnceLock::new();
    let commit = COMMIT.get_or_init(|| unsafe {
        let module = GetModuleHandleW(w!("xmem.dll")).ok()?;
        GetProcAddress(module, s!("TuxBoxCommitRange")).map(|f| core::mem::transmute::<_, Commit>(f))
    });
    match commit {
        Some(commit) if size > 0 => unsafe { commit(base, size) },
        _ => 0,
    }
}
