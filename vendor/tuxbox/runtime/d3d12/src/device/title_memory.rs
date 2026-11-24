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

/// What xmem knows of a title range without a VirtualQuery (xmem
/// lazy_commit.rs view_state; test137 measured 16 ms per GiB of view per
/// VirtualQuery, and the GPU page pool is one 5.4 GiB view).
#[derive(Clone, Copy, Default)]
pub(crate) struct ViewState {
    flags: u32,
    /// The single lazy view holding the range: its OS allocation.
    pub(crate) base: usize,
    pub(crate) size: usize,
}

impl ViewState {
    /// Lazy read-write views cover the range and every page is known committed.
    pub(crate) fn committed(&self) -> bool {
        self.flags & 3 == 3
    }
    /// Lazy read-write views cover the range (pages may still be reserved).
    pub(crate) fn covered(&self) -> bool {
        self.flags & 1 != 0
    }
    /// One view holds the whole range (`base`, `size`).
    pub(crate) fn one_view(&self) -> bool {
        self.flags & 4 != 0
    }
}

pub(crate) fn view_state(address: usize, bytes: usize) -> ViewState {
    type State = unsafe extern "system" fn(usize, usize, *mut usize, *mut usize) -> u32;
    static STATE: std::sync::OnceLock<Option<State>> = std::sync::OnceLock::new();
    let state = STATE.get_or_init(|| unsafe {
        let module = GetModuleHandleW(w!("xmem.dll")).ok()?;
        GetProcAddress(module, s!("TuxBoxViewState")).map(|f| core::mem::transmute::<_, State>(f))
    });
    let mut out = ViewState::default();
    if let Some(state) = state && bytes > 0 {
        out.flags = unsafe { state(address, bytes, &mut out.base, &mut out.size) };
    }
    out
}
