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
#[track_caller]
pub(crate) fn commit_lazy(base: usize, size: usize) -> usize {
    let caller = core::panic::Location::caller();
    let pages = commit_lazy_at(base, size);
    if pages > 0 {
        note_commit(caller, pages);
    }
    pages
}

/// test175: title-mapped commit grew ~3 GB and xmem's counter cannot say
/// who committed. Pages committed per d3d12 caller, logged at most every
/// 10 s while any commit happens. TUXBOX_COMMIT_BY_CALLER=0 off.
fn note_commit(caller: &'static core::panic::Location<'static>, pages: usize) {
    use std::sync::Mutex;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::env_toggle!("TUXBOX_COMMIT_BY_CALLER") != Some("0")) {
        return;
    }
    static BY: Mutex<(Vec<(&'static str, u32, u64)>, Option<std::time::Instant>)> = Mutex::new((Vec::new(), None));
    let mut by = BY.lock().unwrap_or_else(|e| e.into_inner());
    let (file, line) = (caller.file(), caller.line());
    match by.0.iter_mut().find(|e| e.0 == file && e.1 == line) {
        Some(e) => e.2 += pages as u64,
        None => by.0.push((file, line, pages as u64)),
    }
    if by.1.is_some_and(|t| t.elapsed().as_secs() < 10) {
        return;
    }
    by.1 = Some(std::time::Instant::now());
    let list: Vec<String> = by.0.iter().map(|(f, l, n)| format!("{}:{l} {} MiB", f.rsplit(['/', '\\']).next().unwrap_or(f), n * 64 / 1024)).collect();
    crate::diagnostics::log_line(format_args!(
        "lazy commits by caller since start: {} (TUXBOX_COMMIT_BY_CALLER=0 off)", list.join(", ")));
}

fn commit_lazy_at(base: usize, size: usize) -> usize {
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

/// Retires the imports over pages xmem saw remapped since the last call
/// (xmem TuxBoxTakeImportRemaps, five-star-recomp 61c095b): called before an
/// import or a view lookup by address, outside every provider lock.
pub(crate) fn follow_remaps() {
    type Take = unsafe extern "system" fn(*mut [usize; 2], usize) -> usize;
    static TAKE: std::sync::OnceLock<Option<Take>> = std::sync::OnceLock::new();
    let Some(take) = *TAKE.get_or_init(|| unsafe {
        let module = GetModuleHandleW(w!("xmem.dll")).ok()?;
        GetProcAddress(module, s!("TuxBoxTakeImportRemaps")).map(|f| core::mem::transmute::<_, Take>(f))
    }) else { return };
    let mut ranges = [[0usize; 2]; 256];
    loop {
        let n = unsafe { take(ranges.as_mut_ptr(), ranges.len()) };
        // Adjacent pages are retired as one range.
        let mut run: Option<(usize, usize)> = None;
        for &[lo, hi] in &ranges[..n] {
            run = match run {
                Some((a, b)) if lo == b => Some((a, hi)),
                Some((a, b)) => { super::retire_imports_over(a as u64, b as u64); Some((lo, hi)) }
                None => Some((lo, hi)),
            };
        }
        if let Some((a, b)) = run {
            super::retire_imports_over(a as u64, b as u64);
        }
        if n < ranges.len() {
            break;
        }
    }
}
