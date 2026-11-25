//! `xmem.dll`: console memory, served from host memory. The GPU side works
//! from CPU addresses (unified memory), so memory types and bandwidth
//! classes change nothing here; what constrains addresses (alignment,
//! 2 MiB pages, address-bit limits) is honoured.
#![allow(non_snake_case)]
// Title code calls these through its import table, passing pointers the real
// API would trust.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::alloc::{Layout, alloc, dealloc};
use std::collections::{BTreeMap, HashMap};
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_COMMITMENT_LIMIT, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, MEM_ADDRESS_REQUIREMENTS, MEM_COMMIT, MEM_EXTENDED_PARAMETER,
    MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER, MEM_RESERVE,
    MEM_RESERVE_PLACEHOLDER, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile3,
    MemExtendedParameterAddressRequirements, PAGE_NOACCESS, PAGE_READWRITE, SEC_COMMIT,
    SEC_RESERVE, UnmapViewOfFile2, VirtualAlloc, VirtualAlloc2, VirtualFree, VirtualQueryEx,
};
use windows_sys::Win32::System::ProcessStatus::{
    K32GetPerformanceInfo, K32GetProcessMemoryInfo, PERFORMANCE_INFORMATION,
    PROCESS_MEMORY_COUNTERS,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;
use windows_sys::core::BOOL;

use crate::{HleModule, hle_export};

const MIB: usize = 1 << 20;

/// Whether we run under Wine (its ntdll exports `wine_get_version`).
pub(crate) fn on_wine() -> bool {
    static WINE: LazyLock<bool> = LazyLock::new(|| {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        // SAFETY: plain lookups in ntdll, always loaded.
        unsafe {
            let ntdll = GetModuleHandleA(c"ntdll.dll".as_ptr().cast());
            !ntdll.is_null() && GetProcAddress(ntdll, c"wine_get_version".as_ptr().cast()).is_some()
        }
    });
    *WINE
}

/// Standard memory the title has handed to the system.
static TRANSFERRED: AtomicUsize = AtomicUsize::new(0);

// Console additions to the host's flags.
/// 2 MiB pages; the host reads this bit as large pages, which need a
/// privilege.
const PAGES_2M: u32 = 0x2000_0000;
/// With `PAGES_2M`: 64 KiB pages; the host reads it as AWE memory.
const PAGES_64K_BIT: u32 = 0x40_0000;
const PAGES_64K: u32 = PAGES_2M | PAGES_64K_BIT;
/// The host's protection constants and `PAGE_GUARD`. Above them the console
/// adds GPU access bits (0x800-0x40000). Write-combining and uncached are
/// dropped too: our GPU side reads this memory through the CPU.
const HOST_PROTECTION: u32 = 0x1ff;
/// XMem flags of memory both processors reach: all of ours.
const GRAPHICS: u64 = 3;
/// XMem flag bit of 32-bit graphics memory.
const ADDRESS_32BIT: u64 = 4;
/// The host's write watching (not in windows-sys' memory module).
const MEM_WRITE_WATCH: u32 = 0x20_0000;

/// The addresses XMem `flags` allow, first and last: the low 4 GiB for
/// 32-bit memory; else from 4 GiB (kept for 32-bit requests, which the
/// title's big pools would otherwise crowd out) up to 46 down to 40 bits
/// from the field at bit 12, or 40 bits without one: the console keeps
/// title memory there, and its ray-tracing structures must be (
/// the title's BVH builder refuses one above; the title asks for its GPU heap
/// with no field). Decision 0001.
fn address_range(flags: u64) -> (usize, usize) {
    let bits = match (flags & ADDRESS_32BIT, flags >> 12 & 7) {
        (0, 0) => 40,
        (0, field) => 47 - field,
        _ => return (0, u32::MAX as usize),
    };
    (1 << 32, (1 << bits) - 1)
}

pub extern "system" fn XMemVirtualAlloc(
    base: *mut c_void,
    size: usize,
    kind: u32,
    flags: u64,
    protect: u32,
) -> *mut c_void {
    let (lowest, highest) = address_range(flags);
    let mut requirements = MEM_ADDRESS_REQUIREMENTS {
        LowestStartingAddress: lowest as *mut c_void,
        HighestEndingAddress: highest as *mut c_void,
        Alignment: if kind & (PAGES_2M | PAGES_64K_BIT) == PAGES_2M {
            2 * MIB
        } else {
            0
        },
    };
    let console_protect = protect;
    let mut protect = match protect & HOST_PROTECTION {
        0 => PAGE_NOACCESS,
        p => p,
    };
    // A reservation of 64 KiB pages takes physical pages later
    // (`XMemMapPhysicalPages`); on the host only a placeholder can.
    // shortcut: committing part of such a reservation later would fail
    // (the title's 23 calls never do); replace that part of the
    // placeholder then.
    let placeholder = kind & PAGES_64K == PAGES_64K && kind & MEM_COMMIT == 0;
    if placeholder {
        if kind & MEM_REPLACE_PLACEHOLDER != 0 {
            // Part of a placeholder the title split: it stays one.
            return base;
        }
        protect = PAGE_NOACCESS;
    }
    let mut kind = kind & !(PAGES_2M | PAGES_64K_BIT);
    if placeholder {
        kind |= MEM_RESERVE | MEM_RESERVE_PLACEHOLDER;
    }
    let mut parameter = MEM_EXTENDED_PARAMETER::default();
    parameter.Anonymous1._bitfield = MemExtendedParameterAddressRequirements as u64;
    parameter.Anonymous2.Pointer = (&raw mut requirements).cast();
    // Address requirements apply to reservations at an address of our choice.
    let constrained = base.is_null();
    if constrained {
        kind |= MEM_RESERVE;
        // The GPU side copies only the pages the title wrote since its last
        // copy (`victory_gpu`, decision 0007); placeholders cannot be
        // watched (their physical-page views are copied whole).
        if kind & MEM_RESERVE_PLACEHOLDER == 0 {
            kind |= MEM_WRITE_WATCH;
        }
    }
    // SAFETY: the title's own arguments, cleaned of console bits; the
    // parameter points at `requirements`, which outlives the call.
    let at = unsafe {
        VirtualAlloc2(
            GetCurrentProcess(),
            base,
            size,
            kind,
            protect,
            if constrained {
                &mut parameter
            } else {
                std::ptr::null_mut()
            },
            constrained.into(),
        )
    };
    // The GPU side's record of what is mapped (`victory_gpu` regions): a
    // placeholder is holes until pages are mapped into it; any other new
    // reservation is no placeholder, whatever was there before.
    if !at.is_null() && kind & MEM_RESERVE != 0 {
        if placeholder {
            victory_gpu::title_memory_placeholder(at as usize, size);
        } else {
            victory_gpu::title_memory_not_placeholder(at as usize, size);
        }
    }
    // The title's GPU-only reservations (no CPU access, GPU bit: 0x801):
    // counted where page mappings land in them (MEMORY.md §3.3 4).
    if !at.is_null() && base.is_null() && console_protect & HOST_PROTECTION == PAGE_NOACCESS && console_protect & 0x800 != 0
    {
        GPU_ONLY.write().unwrap_or_else(|e| e.into_inner()).push((at as usize, at as usize + size));
        victory_gpu::title_memory_gpu_only(at as usize, size);
        tracing::info!(
            at = format_args!("{at:p}"),
            size,
            protect = format_args!("{console_protect:#x}"),
            "XMemVirtualAlloc: a GPU-only reservation"
        );
    }
    if !at.is_null() && kind & MEM_COMMIT != 0 && !placeholder {
        VIRTUAL_BYTES.fetch_add(size, Ordering::Relaxed);
    }
    if base.is_null() {
        tracing::debug!(
            size,
            kind = format_args!("{kind:#x}"),
            flags = format_args!("{flags:#x}"),
            at = format_args!("{at:p}"),
            "XMemVirtualAlloc"
        );
    }
    at
}

/// `MEMORY_BASIC_INFORMATION` (the console's first 0x30 bytes are the
/// same), then the XMem flags and tag.
#[repr(C)]
pub struct BasicInformation {
    host: MEMORY_BASIC_INFORMATION,
    flags: u64,
    tag_code: u32,
    tag: [u8; 4],
}

// shortcut: every committed page reports graphics memory and no tag, since
// all of it is GPU-reachable here (the title's BVH builder, the
// only caller, wants graphics memory); remember the flags of
// `XMemVirtualAlloc` regions if a title tells CPU-only memory apart.
pub extern "system" fn XMemVirtualQuery(
    process: HANDLE,
    address: *const c_void,
    out: *mut BasicInformation,
) -> BOOL {
    let size = size_of::<MEMORY_BASIC_INFORMATION>();
    // SAFETY: the title passes room for the whole structure; the host
    // fails on a null one.
    unsafe {
        if VirtualQueryEx(process, address, out.cast(), size) == 0 {
            return 0;
        }
        let info = &mut *out;
        info.flags = if info.host.State == MEM_COMMIT {
            GRAPHICS
        } else {
            0
        };
        info.tag_code = 0;
        info.tag = [0; 4];
    }
    1
}

// `XMemAlloc` attributes: alignment as log2 (5 bits at 24, 4 bytes and up
// from 2), then the memory type, then bit 33: a 32-bit address.
const ALIGNMENT_SHIFT: u64 = 24;
const ALLOC_32BIT: u64 = 1 << 33;
/// Our block header: the block's size and alignment, just below the
/// pointer handed out.
const HEADER: usize = 2 * size_of::<usize>();

fn layout(size: usize, attributes: u64) -> Option<Layout> {
    // An alignment of at least 16 leaves room for the header.
    let align = 1usize << (attributes >> ALIGNMENT_SHIFT & 0x1f).max(4);
    Layout::from_size_align(size.checked_add(align)?, align).ok()
}

type AllocRoutine = extern "system" fn(usize, u64) -> *mut c_void;
type FreeRoutine = extern "system" fn(*mut c_void, u64);

/// The allocation and free routines the title's exe exports as
/// `__XMemAllocationHooks__` (the title does): `XMemAlloc` and
/// `XMemFree` call them, for the title's own calls and its middleware's;
/// the `…Default` exports do not.
fn hooks() -> [usize; 2] {
    static AT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let at = match AT.get() {
        Some(&at) => at,
        // Looked up once the title is mapped, which is before it runs.
        None => match crate::runtime::main_image() {
            Some(base) => *AT.get_or_init(|| {
                use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
                // SAFETY: a mapped image and a NUL-terminated name.
                let at = unsafe {
                    GetProcAddress(base as _, c"__XMemAllocationHooks__".as_ptr().cast())
                }
                .map_or(0, |f| f as usize);
                tracing::info!(found = at != 0, "xmem allocation hooks");
                at
            }),
            None => 0,
        },
    };
    if at == 0 {
        return [0; 2];
    }
    // SAFETY: the export is two routine pointers.
    unsafe { (at as *const [usize; 2]).read() }
}

/// The title's general allocator, for middleware too: its hook if it has
/// one, else [`XMemAllocDefault`].
pub extern "system" fn XMemAlloc(size: usize, attributes: u64) -> *mut c_void {
    match hooks()[0] {
        0 => XMemAllocDefault(size, attributes),
        hook => {
            // SAFETY: the title's own routine of this type.
            let alloc: AllocRoutine = unsafe { std::mem::transmute(hook) };
            alloc(size, attributes)
        }
    }
}

/// See [`XMemAlloc`].
pub extern "system" fn XMemFree(address: *mut c_void, attributes: u64) {
    match hooks()[1] {
        0 => XMemFreeDefault(address, attributes),
        hook => {
            // SAFETY: the title's own routine of this type.
            let free: FreeRoutine = unsafe { std::mem::transmute(hook) };
            free(address, attributes)
        }
    }
}

/// The system allocator behind [`XMemAlloc`].
pub extern "system" fn XMemAllocDefault(size: usize, attributes: u64) -> *mut c_void {
    if attributes & ALLOC_32BIT != 0 {
        return XMemVirtualAlloc(
            std::ptr::null_mut(),
            size,
            MEM_RESERVE | MEM_COMMIT,
            GRAPHICS | ADDRESS_32BIT,
            PAGE_READWRITE,
        );
    }
    let Some(layout) = layout(size, attributes) else {
        return std::ptr::null_mut();
    };
    // shortcut: the host heap's addresses lie above 2^40, unlike the
    // console's; nothing seen builds ray-tracing structures in this memory
    // (the title uses its GPU heap). Sub-allocate from a
    // reservation below 2^40 if one does.
    // SAFETY: the layout has a non-zero size.
    let block = unsafe { alloc(layout) };
    if block.is_null() {
        return block.cast();
    }
    // SAFETY: the block holds the alignment (at least the header) and then
    // `size` bytes.
    unsafe {
        let user = block.add(layout.align());
        user.sub(HEADER)
            .cast::<[usize; 2]>()
            .write([layout.size(), layout.align()]);
        user.cast()
    }
}

/// Frees what [`XMemAllocDefault`] returned, given the same attributes.
pub extern "system" fn XMemFreeDefault(address: *mut c_void, attributes: u64) {
    if address.is_null() {
        return;
    }
    // SAFETY: the address came from `XMemAlloc` with these attributes: a
    // whole reservation, or a header-prefixed block.
    unsafe {
        if attributes & ALLOC_32BIT != 0 {
            VirtualFree(address, 0, MEM_RELEASE);
            return;
        }
        let user = address.cast::<u8>();
        let [size, align] = user.sub(HEADER).cast::<[usize; 2]>().read();
        dealloc(
            user.sub(align),
            Layout::from_size_align_unchecked(size, align),
        );
    }
}

/// A retail console has no tools memory.
pub extern "system" fn XMemIsToolsMemAvailable() -> BOOL {
    0
}

// Physical pages: on the console, 64 KiB frames a title maps into its own
// reservations, at more than one address if it likes. Here each allocation
// is a pagefile-backed section, its pages are 64 KiB slices of it, and a
// mapping is a view of consecutive slices in place of a host placeholder.

const PAGE: usize = 64 << 10;

/// A section: its handle, its bytes, and a view of all of it of our own,
/// made the first time a page of it is shown through the GPU side's
/// translation table (`VICTORY_MEM2_GPU_ONLY_NOVIEW`; 0: none yet, or it
/// failed).
struct Section(HANDLE, usize, std::sync::OnceLock<usize>);

// SAFETY: a section handle may be used from any thread.
unsafe impl Send for Section {}
// SAFETY: as above.
unsafe impl Sync for Section {}

impl Drop for Section {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::UnmapViewOfFile;
        if let Some(&at) = self.2.get()
            && at != 0
        {
            // SAFETY: our own view of the section; no table entry points
            // into it any more (each holds the section).
            unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: at as *mut c_void }) };
        }
        // SAFETY: our handle; views keep the section alive on their own.
        unsafe { CloseHandle(self.0) };
    }
}

impl Section {
    /// The view of all of the section, made once; 0 if it cannot be.
    fn whole(&self) -> usize {
        use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile};
        *self.2.get_or_init(|| {
            // SAFETY: a section we own; the view is ours until the section
            // goes.
            let at = unsafe { MapViewOfFile(self.0, FILE_MAP_WRITE, 0, 0, self.1) };
            at.Value as usize
        })
    }
}

/// Where a page lives, or what a view starts with: a section and a page
/// index in it.
#[derive(Clone)]
struct Slice {
    section: Arc<Section>,
    index: usize,
}

#[derive(Default)]
struct Physical {
    next_page: usize,
    pages: HashMap<usize, Slice>,
    /// Mapped views by address: their length and what they show.
    views: BTreeMap<usize, (usize, Slice)>,
    /// Where each page is shown, by section and index in it.
    shown: HashMap<(usize, usize), Vec<usize>>,
    /// Addresses of 64 KiB placeholders known to stand alone (a view of
    /// one page was unmapped there): mapping a page there needs no split
    /// first (`VICTORY_PAGEMAP_RUNS`).
    lone: std::collections::HashSet<usize>,
    /// `VICTORY_MEM2_GPU_ONLY_NOVIEW`: GPU-only destination pages shown
    /// through the GPU side's translation table, not a view, and the page
    /// each shows ([`translate`]).
    translated: BTreeMap<usize, Slice>,
}

impl Slice {
    /// The page `k` pages on, as [`Physical::shown`] names it.
    fn key(&self, k: usize) -> (usize, usize) {
        (Arc::as_ptr(&self.section) as usize, self.index + k)
    }
}

static PHYSICAL: LazyLock<Mutex<Physical>> = LazyLock::new(|| {
    Mutex::new(Physical {
        // Page numbers are opaque to titles; 0 stays unused.
        next_page: 0x10_0000,
        ..Default::default()
    })
});

fn physical() -> MutexGuard<'static, Physical> {
    PHYSICAL.lock().unwrap_or_else(|e| e.into_inner())
}

/// `count` is in and out (pages wanted, pages given); all or nothing here.
pub extern "system" fn XMemAllocatePhysicalPages(
    flags: u64,
    count: *mut usize,
    pages: *mut usize,
) -> BOOL {
    if count.is_null() || pages.is_null() {
        return 0;
    }
    // SAFETY: checked non-null.
    let n = unsafe { *count };
    let Some(size) = n.checked_mul(PAGE).filter(|&s| s > 0) else {
        return 0;
    };
    // A pagefile-backed section of `size` bytes, its pages committed when
    // first mapped or written (`commit`): the title asks for 6.1
    // GiB at once, which a busy host often cannot commit (error 1450).
    // SAFETY: plain section creation.
    let handle = unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            std::ptr::null(),
            // Under Wine a committed section is a sparse file, free until
            // touched, and committing a reserved one page by page costs a
            // wineserver request each that scans every range committed so
            // far: commit it whole there.
            // `VICTORY_PHYS_COMMIT=1`: here too, so a map is no commit.
            PAGE_READWRITE | if on_wine() || phys_commit() { SEC_COMMIT } else { SEC_RESERVE },
            (size as u64 >> 32) as u32,
            size as u32,
            std::ptr::null(),
        )
    };
    if handle.is_null() {
        let error = std::io::Error::last_os_error();
        tracing::warn!(n, %error, "XMemAllocatePhysicalPages failed");
        // SAFETY: checked non-null.
        unsafe { count.write(0) };
        return 0;
    }
    tracing::debug!(
        n,
        flags = format_args!("{flags:#x}"),
        "XMemAllocatePhysicalPages"
    );
    // For the GPU side's in-place census: only a committed section can be
    // opened as a host heap.
    victory_gpu::title_memory_section(handle as usize, size as u64, on_wine() || phys_commit());
    PHYSICAL_BYTES.fetch_add(size, Ordering::Relaxed);
    let section = Arc::new(Section(handle, size, std::sync::OnceLock::new()));
    let mut physical = physical();
    let first = physical.next_page;
    physical.next_page += n * FRAMES;
    for index in 0..n {
        let slice = Slice {
            section: section.clone(),
            index,
        };
        let number = first + index * FRAMES;
        physical.pages.insert(number, slice);
        // SAFETY: the title passes room for `n` page numbers.
        unsafe { pages.add(index).write(number) };
    }
    1
}

/// Page numbers count 4 KiB frames, so a 64 KiB page's is a multiple of
/// 16: the title keeps them as 21-bit `number >> 4` and rebuilds
/// them with `<< 4` (128 GiB of pages at most).
const FRAMES: usize = PAGE >> 12;

/// Pages still mapped stay readable through their views until unmapped.
pub extern "system" fn XMemFreePhysicalPages(n: usize, pages: *const usize) -> BOOL {
    if pages.is_null() {
        return 0;
    }
    let mut physical = physical();
    let mut freed = 0;
    for i in 0..n {
        // SAFETY: the title passes `n` page numbers.
        freed += usize::from(physical.pages.remove(&unsafe { *pages.add(i) }).is_some());
    }
    let _ = PHYSICAL_BYTES.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |b| Some(b.saturating_sub(freed * PAGE)));
    1
}

/// A view of our own on consecutive slices of one section, committed and
/// writable, unmapped when dropped.
struct View {
    at: MEMORY_MAPPED_VIEW_ADDRESS,
    len: usize,
}

impl View {
    /// Maps `n` slices from `slice` on.
    fn map(slice: &Slice, n: usize) -> Option<View> {
        use windows_sys::Win32::System::Memory::{FILE_MAP_WRITE, MapViewOfFile};
        let (offset, len) = ((slice.index * PAGE) as u64, n * PAGE);
        // SAFETY: slices of a section we own; the view is ours until dropped.
        let at = unsafe {
            MapViewOfFile(
                slice.section.0,
                FILE_MAP_WRITE,
                (offset >> 32) as u32,
                offset as u32,
                len,
            )
        };
        if at.Value.is_null() {
            return None;
        }
        let view = View { at, len };
        commit(at.Value as usize, len).then_some(view)
    }

    fn bytes(&mut self) -> &mut [u8] {
        // SAFETY: the view holds `len` committed read-write bytes, ours
        // alone while `self` lives (other views of the slices only alias
        // them, as the console's do).
        unsafe { std::slice::from_raw_parts_mut(self.at.Value.cast(), self.len) }
    }
}

impl Drop for View {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Memory::UnmapViewOfFile;
        // SAFETY: our view.
        unsafe { UnmapViewOfFile(self.at) };
    }
}

/// The pages behind bytes `[offset, offset + len)` of `pages` (64 KiB
/// each), as runs of consecutive slices of one section: each run's first
/// slice and its length in pages. One look at the page table, which the
/// title's own mapping calls share with every DirectStorage worker. `None`
/// if a page is unknown or `pages` ends early.
fn runs(pages: &[usize], offset: usize, len: usize) -> Option<Vec<(Slice, usize)>> {
    let end = offset.checked_add(len)?;
    let pages = pages.get(offset / PAGE..end.div_ceil(PAGE))?;
    let physical = physical();
    let mut runs: Vec<(Slice, usize)> = Vec::new();
    for page in pages {
        let slice = physical.pages.get(page)?;
        match runs.last_mut() {
            Some((first, n))
                if Arc::ptr_eq(&first.section, &slice.section)
                    && slice.index == first.index + *n =>
            {
                *n += 1;
            }
            _ => runs.push((slice.clone(), 1)),
        }
    }
    Some(runs)
}

/// Copies `data` into physical pages, from `offset` bytes into the first,
/// whether or not they are mapped anywhere (DirectStorage destinations).
/// One view per run of consecutive pages, not one per page: each view
/// costs a map, a commit and an unmap (which flushes every core's TLB).
/// False if a page is unknown or the host fails.
pub(crate) fn write_physical(pages: &[usize], offset: usize, data: &[u8]) -> bool {
    let Some(runs) = runs(pages, offset, data.len()) else {
        return false;
    };
    let (mut within, mut done) = (offset % PAGE, 0);
    for (slice, n) in &runs {
        let Some(mut view) = View::map(slice, *n) else {
            return false;
        };
        let k = (n * PAGE - within).min(data.len() - done);
        view.bytes()[within..within + k].copy_from_slice(&data[done..done + k]);
        (within, done) = (0, done + k);
        written(slice, *n);
    }
    true
}

/// After writes into `n` pages from `slice` on through a view of our own:
/// where the title sees those pages, the GPU side has to copy them again (it
/// does not see such writes).
fn written(slice: &Slice, n: usize) {
    let mut shown: Vec<usize> = {
        let physical = physical();
        (0..n)
            .filter_map(|k| physical.shown.get(&slice.key(k)))
            .flatten()
            .copied()
            .collect()
    };
    shown.sort_unstable();
    // After the fact, and without making pages shown through the GPU
    // side's translation table views (`VICTORY_MEM2_GPU_ONLY_NOVIEW`).
    for run in shown.chunk_by(|a, b| a + PAGE == *b) {
        victory_gpu::physical_written(run[0], run.len() * PAGE);
    }
}

/// Bytes `[offset, offset + len)` of physical pages, writable in place
/// until dropped: DirectStorage decodes into it instead of a buffer it
/// would then copy.
pub(crate) struct PageWindow {
    view: View,
    within: usize,
    len: usize,
    slice: Slice,
}

impl Drop for PageWindow {
    fn drop(&mut self) {
        written(&self.slice, self.view.len / PAGE);
    }
}

impl PageWindow {
    pub(crate) fn bytes(&mut self) -> &mut [u8] {
        let (within, len) = (self.within, self.len);
        &mut self.view.bytes()[within..within + len]
    }
}

/// A window on bytes `[offset, offset + len)` of `pages` when they are
/// consecutive pages of one allocation (how titles usually hand them out);
/// `None` otherwise (pages of several allocations, out of order, or
/// unknown, or a host failure), which `write_physical` then handles or
/// reports.
pub(crate) fn page_window(pages: &[usize], offset: usize, len: usize) -> Option<PageWindow> {
    let runs = runs(pages, offset, len)?;
    let [(slice, n)] = runs.as_slice() else {
        return None;
    };
    Some(PageWindow {
        view: View::map(slice, *n)?,
        within: offset % PAGE,
        len,
        slice: slice.clone(),
    })
}

/// Maps `n` pages at `address`, in place of what is mapped there; null
/// `pages` unmaps. Returns `address`, or null if a page or the host fails.
pub extern "system" fn XMemMapPhysicalPages(
    address: *mut c_void,
    n: usize,
    pages: *const usize,
) -> *mut c_void {
    victory_gpu::count_remap(3, 1);
    victory_gpu::count_remap(4, n as u64);
    map_physical(address, n, pages)
}

/// [`XMemMapPhysicalPages`], not counted as a call of its own.
fn map_physical(address: *mut c_void, n: usize, pages: *const usize) -> *mut c_void {
    let started = std::time::Instant::now();
    let base = address as usize;
    // SAFETY: the title passes `n` page numbers unless null.
    let pages = (!pages.is_null()).then(|| unsafe { std::slice::from_raw_parts(pages, n) });
    let mut physical = physical();
    let mut held = None;
    let batched = pagemap_runs();
    if batched {
        begin_notes();
    }
    let ok = map_pages(&mut physical, base, n, pages, &mut held, false);
    if batched {
        end_notes(&physical);
    }
    victory_gpu::count_remap(13, started.elapsed().as_micros() as u64);
    drop(held);
    drop(physical);
    if ok { address } else { std::ptr::null_mut() }
}

/// Maps `n` pages at `base` (`None`: unmaps), under the caller's lock.
/// Holds the bytes whose mapping changes in `held` (taken, or extended
/// when the caller holds a change already, `VICTORY_PAGEMAP_RUNS`) until
/// the caller drops it. False if a page or the host fails. With
/// `page_views` each page gets a view of its own, as when the title maps
/// them one at a time (`XMemMapPhysicalPagesScatter`).
fn map_pages(
    physical: &mut Physical,
    base: usize,
    n: usize,
    pages: Option<&[usize]>,
    held: &mut Option<victory_gpu::Remapping>,
    page_views: bool,
) -> bool {
    // Until the pages are mapped again: `unmap` takes whole views away and
    // maps what stays of them again, and a mapped view is a reservation
    // until it is committed. A read of title memory in place must not fall
    // into that gap (`victory_gpu::title_memory_remapping`); reads of other
    // bytes go on.
    let (from, to) = reach(physical, base, n * PAGE);
    hold(held, from, to);
    unmap(physical, base, n * PAGE);
    let Some(pages) = pages else {
        return true;
    };
    let mut i = 0;
    while i < n {
        let Some(first) = physical.pages.get(&pages[i]).cloned() else {
            tracing::warn!(page = pages[i], "XMemMapPhysicalPages: unknown page");
            return false;
        };
        // One view per run of consecutive slices of one section; one per
        // page for a Scatter batch: the title remaps a page of it alone
        // later, and unmapping a page of a view takes the whole view away
        // and maps the rest again, so its neighbours (which the title goes
        // on reading) would be "reserved, never committed" meanwhile.
        let run = if page_views {
            1
        } else {
            1 + pages[i + 1..]
                .iter()
                .zip(1..)
                .take_while(|&(page, k)| {
                    physical.pages.get(page).is_some_and(|s| {
                        Arc::ptr_eq(&s.section, &first.section) && s.index == first.index + k
                    })
                })
                .count()
        };
        if !map(physical, base + i * PAGE, run * PAGE, first) {
            return false;
        }
        i += run;
    }
    true
}

/// Holds `[from, to)` against reads in place: takes the change, or adds
/// the bytes to the one held.
fn hold(held: &mut Option<victory_gpu::Remapping>, from: usize, to: usize) {
    match held {
        Some(h) => h.extend(from, to - from),
        None => *held = Some(victory_gpu::title_memory_remapping(from, to - from)),
    }
}

/// One page at each address; null `pages`, or a page number 0, unmaps
/// (the title passes 0 for addresses to clear).
pub extern "system" fn XMemMapPhysicalPagesScatter(
    addresses: *const *mut c_void,
    n: usize,
    pages: *const usize,
) -> BOOL {
    if addresses.is_null() {
        return 0;
    }
    if pagemap_runs() {
        // SAFETY: the title passes `n` addresses and, unless null, `n` pages.
        let (addresses, pages) = unsafe {
            (
                std::slice::from_raw_parts(addresses.cast::<usize>(), n),
                (!pages.is_null()).then(|| std::slice::from_raw_parts(pages, n)),
            )
        };
        return scatter_batched(addresses, pages).into();
    }
    victory_gpu::count_remap(0, 1);
    victory_gpu::count_remap(1, n as u64);
    for i in 0..n {
        // SAFETY: the title passes `n` addresses and, unless null, `n` pages.
        let (address, page) = unsafe {
            (
                *addresses.add(i),
                if pages.is_null() || *pages.add(i) == 0 {
                    std::ptr::null()
                } else {
                    pages.add(i)
                },
            )
        };
        if map_physical(address, 1, page).is_null() {
            return 0;
        }
    }
    1
}

/// `VICTORY_PAGEMAP_RUNS=1` (off by default): the title's batches of page
/// mappings (`XMemMapPhysicalPagesScatter`, the queue's
/// `CopyPageMappingsBatchX`) are done as batches: the page lock and the
/// hold on the bytes changed taken once per batch, consecutive addresses
/// mapped as one range (one unmap pass, one view per run of consecutive
/// pages), the placeholder split left out where a lone 64 KiB placeholder
/// stands already, and the GPU side told once per joined range at the end
/// of the batch instead of twice per page. It maps exactly what the
/// page-by-page path maps (the entries in the title's order); counted on
/// the frame line ("page mapping per frame").
pub(crate) fn pagemap_runs() -> bool {
    #[cfg(test)]
    if tests::PAGEMAP_RUNS.load(Ordering::Relaxed) {
        return true;
    }
    static ON: LazyLock<bool> = LazyLock::new(|| {
        let on = std::env::var("VICTORY_PAGEMAP_RUNS").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::warn!("VICTORY_PAGEMAP_RUNS: page mappings are done batch by batch");
        }
        on
    });
    *ON
}

/// `VICTORY_PHYS_COMMIT=1` (off by default): physical pages are sections
/// committed whole when allocated (`SEC_COMMIT`, as under Wine), so a map
/// is no commit. The commit charge is the whole pool at once (3.9 GiB on
/// Series S, 6.1 on X; RAM only as pages are touched).
fn phys_commit() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| {
        let on = std::env::var("VICTORY_PHYS_COMMIT").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::warn!("VICTORY_PHYS_COMMIT: physical page sections are committed whole");
        }
        on
    });
    *ON
}

/// [`XMemMapPhysicalPagesScatter`] with [`pagemap_runs`].
fn scatter_batched(addresses: &[usize], pages: Option<&[usize]>) -> bool {
    let started = std::time::Instant::now();
    let unmaps = |i: usize| pages.is_none_or(|p| p[i] == 0);
    let groups = victory_core::pagemap::scatter_groups(addresses, unmaps);
    let mut physical = physical();
    let mut held = None;
    begin_notes();
    let mut ok = true;
    for &(first, len) in &groups {
        let group = match pages {
            Some(p) if !unmaps(first) => Some(&p[first..first + len]),
            _ => None,
        };
        if !map_pages(&mut physical, addresses[first], len, group, &mut held, true) {
            ok = false;
            break;
        }
    }
    end_notes(&physical);
    drop(held);
    drop(physical);
    victory_gpu::count_remap(0, 1);
    victory_gpu::count_remap(1, addresses.len() as u64);
    victory_gpu::count_remap(2, groups.len() as u64);
    victory_gpu::count_remap(13, started.elapsed().as_micros() as u64);
    ok
}

thread_local! {
    /// While a batch is done ([`pagemap_runs`]): what it changed, told to
    /// the GPU side at its end ([`end_notes`]).
    static NOTES: std::cell::RefCell<Option<victory_core::pagemap::Notes>> =
        const { std::cell::RefCell::new(None) };
}

fn begin_notes() {
    NOTES.with(|n| *n.borrow_mut() = Some(victory_core::pagemap::Notes::new()));
}

/// Tells the GPU side what the batch changed: each joined range once as
/// remapped, then the views made that are still there, so it ends where
/// telling it page by page would have. Under the page lock and the hold
/// on the bytes, as before.
fn end_notes(physical: &Physical) {
    let Some(notes) = NOTES.with(|n| n.borrow_mut().take()) else {
        return;
    };
    let joined = notes.changed_joined();
    for &(from, to) in &joined {
        victory_gpu::title_memory_remapped(from, to - from);
    }
    let views = notes.views_alive(|at, len| physical.views.get(&at).is_some_and(|v| v.0 == len));
    for &(at, len, section, offset) in &views {
        victory_gpu::title_memory_view(at, len, section, offset);
    }
    let sent = joined.len() + views.len();
    victory_gpu::count_remap(11, sent as u64);
    victory_gpu::count_remap(15, (notes.changes() + notes.views()).saturating_sub(sent) as u64);
}

/// The GPU side is told `[at, at + len)` shows something new: now, or at
/// the end of the batch.
fn note_remapped(at: usize, len: usize) {
    let noted = NOTES.with(|n| n.borrow_mut().as_mut().map(|n| n.changed(at, len)).is_some());
    if !noted {
        victory_gpu::title_memory_remapped(at, len);
    }
}

/// The GPU side is told of a view: now, or at the end of the batch.
fn note_view(at: usize, len: usize, section: usize, offset: u64) {
    let noted = NOTES.with(|n| n.borrow_mut().as_mut().map(|n| n.view(at, len, section, offset)).is_some());
    if !noted {
        victory_gpu::title_memory_view(at, len, section, offset);
    }
}

/// The title's GPU-only reservations (protection with the GPU bit and no
/// CPU access, 0x801), `[start, end)`: the CPU cannot touch them on the
/// console (MEMORY.md §2.3).
static GPU_ONLY: std::sync::RwLock<Vec<(usize, usize)>> = std::sync::RwLock::new(Vec::new());

/// Destination pages of `[at, at + len)` in a GPU-only reservation.
fn gpu_only_pages(at: usize, len: usize) -> u64 {
    let end = at.saturating_add(len);
    let r = GPU_ONLY.read().unwrap_or_else(|e| e.into_inner());
    r.iter()
        .map(|&(a, b)| {
            let (from, to) = (a.max(at), b.min(end));
            if from < to { ((to - from) / PAGE) as u64 } else { 0 }
        })
        .sum()
}

/// `VICTORY_MEM2_GPU_ONLY_NOSNAP`: which physical pages GPU-only addresses
/// show, and whether an address the CPU can reach shows them too
/// (`victory_core::mem2::Aliases`); kept only with the switch on. Locked
/// inside the page lock, never around it.
static ALIASES: LazyLock<std::sync::RwLock<victory_core::mem2::Aliases>> =
    LazyLock::new(|| std::sync::RwLock::new(victory_core::mem2::Aliases::new()));

/// `[at, at + len)` (pages) shows `slice` and the slices after it now: for
/// [`ALIASES`]. A page that GPU-only addresses show and that the CPU can now
/// reach here is told to the GPU side before the title gets this address.
fn shows(at: usize, slice: &Slice) {
    shows_n(at, PAGE, slice);
}

fn shows_n(at: usize, len: usize, slice: &Slice) {
    if !victory_gpu::mem2_nosnap() {
        return;
    }
    let mut exposed = Vec::new();
    {
        let mut a = ALIASES.write().unwrap_or_else(|e| e.into_inner());
        for k in 0..len / PAGE {
            let page = at + k * PAGE;
            let gpu_only = gpu_only_pages(page, PAGE) != 0;
            exposed.extend(a.show(page, slice.key(k), gpu_only));
        }
    }
    for at in exposed {
        victory_gpu::gpu_only_exposed(at);
    }
}

/// `[at, at + len)` (pages) shows nothing now: for [`ALIASES`].
fn hides(at: usize, len: usize) {
    if !victory_gpu::mem2_nosnap() {
        return;
    }
    let mut a = ALIASES.write().unwrap_or_else(|e| e.into_inner());
    for k in 0..len / PAGE {
        a.hide(at + k * PAGE);
    }
}

/// For the GPU side (`VICTORY_MEM2_GPU_ONLY_NOSNAP`): whether no page shown
/// in `[at, at + len)` is shown where the CPU can reach it, and all of it
/// lies in GPU-only reservations.
pub fn cpu_free(at: usize, len: usize) -> bool {
    victory_gpu::mem2_nosnap()
        && gpu_only_pages(at / PAGE * PAGE, (at % PAGE + len).div_ceil(PAGE) * PAGE) as usize
            == (at % PAGE + len).div_ceil(PAGE)
        && ALIASES.read().unwrap_or_else(|e| e.into_inner()).cpu_free(at, len)
}

/// Bytes of physical pages allocated and not freed (sections made),
/// for the title's own memory use ([`XMemGetWorkingSetStatistics`]).
static PHYSICAL_BYTES: AtomicUsize = AtomicUsize::new(0);
/// Bytes committed by `XMemVirtualAlloc` (not placeholders).
static VIRTUAL_BYTES: AtomicUsize = AtomicUsize::new(0);

/// `CopyPageMappingsBatchX` with [`pagemap_runs`]: the batch's operations
/// joined (`victory_core::pagemap::merge_alias`), done under one page lock
/// and one hold on the bytes, the GPU side told once at the end. The
/// queue's page-mapping copies (`victory_gpu::resource::use_page_batch_mapper`).
pub fn map_alias_batch(ops: &[victory_core::pagemap::AliasOp]) -> bool {
    let started = std::time::Instant::now();
    let merged = victory_core::pagemap::merge_alias(ops);
    let pages: u64 = ops.iter().map(|o| o.pages()).sum();
    let repeated: u64 = ops.iter().filter(|o| o.repeat).map(|o| o.pages()).sum();
    let gpu_only: u64 = ops.iter().map(|o| gpu_only_pages(o.to as usize, o.len as usize)).sum();
    let mut physical = physical();
    let mut held = None;
    begin_notes();
    let mut ok = true;
    for op in &merged {
        ok &= alias_locked(&mut physical, op, &mut held);
    }
    end_notes(&physical);
    drop(held);
    drop(physical);
    victory_gpu::count_remap(5, 1);
    victory_gpu::count_remap(6, pages);
    victory_gpu::count_remap(7, merged.len() as u64);
    victory_gpu::count_remap(8, gpu_only);
    victory_gpu::count_remap(14, repeated);
    victory_gpu::count_remap(13, started.elapsed().as_micros() as u64);
    ok
}

/// One operation of [`map_alias_batch`], under the page lock: as
/// [`map_alias`] for a range, the one source page for every destination
/// page when repeated.
fn alias_locked(
    physical: &mut Physical,
    op: &victory_core::pagemap::AliasOp,
    held: &mut Option<victory_gpu::Remapping>,
) -> bool {
    let (to, len) = (op.to as usize, op.len as usize);
    let (start, end) = reach(physical, to, len);
    hold(held, start, end);
    unmap(physical, to, len);
    let Some(from) = op.from.map(|f| f as usize) else {
        return true;
    };
    for k in (0..len).step_by(PAGE) {
        let at = if op.repeat { from } else { from + k };
        let Some(slice) = shown_at(physical, at) else {
            return false;
        };
        if translate(physical, to + k, &slice) {
            continue;
        }
        if !map(physical, to + k, PAGE, slice) {
            return false;
        }
    }
    true
}

/// `VICTORY_MEM2_GPU_ONLY_NOVIEW=1` (off by default): a page-mapping
/// destination page `at` in a GPU-only reservation (which the console's CPU
/// cannot touch) shows `slice` through the GPU side's translation table
/// (`victory_gpu` translate), not a view: no placeholder split, map, commit
/// or unmap for it. Whatever reads or writes it on the CPU side is
/// translated or has it made a view then. False (map it as before) with the
/// switch off, elsewhere, or if the section's own view cannot be made.
/// Under the page lock, after `unmap` took away what `at` showed.
fn translate(physical: &mut Physical, at: usize, slice: &Slice) -> bool {
    if !at.is_multiple_of(PAGE) || gpu_only_pages(at, PAGE) == 0 || !victory_gpu::mem2_gpu_only_noview() {
        return false;
    }
    let whole = slice.section.whole();
    if whole == 0 {
        return false;
    }
    let offset = slice.index * PAGE;
    victory_gpu::title_memory_translated(at, whole + offset, slice.section.0 as usize, offset as u64);
    // Its bytes are new to the GPU side (what it showed went in `unmap`).
    note_remapped(at, PAGE);
    physical.shown.entry(slice.key(0)).or_default().push(at);
    physical.translated.insert(at, slice.clone());
    shows(at, slice);
    true
}

/// The page a view shows at `at`, if one does.
fn shown_at(physical: &Physical, at: usize) -> Option<Slice> {
    physical
        .views
        .range(..=at)
        .next_back()
        .filter(|(base, (size, _))| at < *base + size)
        .map(|(base, (_, s))| Slice {
            index: s.index + (at - base) / PAGE,
            section: s.section.clone(),
        })
        // Or shown through the GPU side's translation table.
        .or_else(|| physical.translated.get(&(at / PAGE * PAGE)).cloned())
}

/// Maps the physical pages shown at `[from, from + len)` at `to` as well,
/// in place of what `to` shows; `from` 0 unmaps. The GPU's page-mapping
/// copies (`victory_gpu`, queue `CopyPageMappingsBatchX`):
/// the title maps pages into a pool view, then has the GPU map them into its tiled
/// heap. Page by page; false if a source page is not mapped physical memory.
pub fn map_alias(to: u64, from: u64, len: u64) -> bool {
    let (to, from, len) = (to as usize, from as usize, len as usize);
    victory_gpu::count_remap(6, (len / PAGE) as u64);
    victory_gpu::count_remap(7, 1);
    victory_gpu::count_remap(8, gpu_only_pages(to, len));
    let mut physical = physical();
    // As in `XMemMapPhysicalPages`: no read in place meets the gap.
    let (start, end) = reach(&physical, to, len);
    let _changing = victory_gpu::title_memory_remapping(start, end - start);
    unmap(&mut physical, to, len);
    for k in (0..len).step_by(PAGE) {
        if from == 0 {
            break;
        }
        let at = from + k;
        let Some(slice) = shown_at(&physical, at) else {
            return false;
        };
        if translate(&mut physical, to + k, &slice) {
            continue;
        }
        if !map(&mut physical, to + k, PAGE, slice) {
            return false;
        }
    }
    true
}

/// Maps `slice` and the slices after it over `[base, base + len)`, which
/// must lie in one placeholder.
fn map(physical: &mut Physical, base: usize, len: usize, slice: Slice) -> bool {
    // A lone 64 KiB placeholder is the whole range already: no split
    // (`VICTORY_PAGEMAP_RUNS`; if that was wrong the map fails and is
    // made again after the split).
    let lone = len == PAGE && pagemap_runs() && physical.lone.remove(&base);
    if lone {
        victory_gpu::count_remap(12, 1);
        // SAFETY: maps the section in place of the placeholder.
        let view = unsafe {
            MapViewOfFile3(
                slice.section.0,
                GetCurrentProcess(),
                base as *const c_void,
                (slice.index * PAGE) as u64,
                len,
                MEM_REPLACE_PLACEHOLDER,
                PAGE_READWRITE,
                std::ptr::null_mut(),
                0,
            )
        };
        if view.Value as usize == base {
            return mapped(physical, base, len, slice, view);
        }
    }
    // SAFETY: splits the placeholder around the range (failing harmlessly
    // when the range is the whole placeholder), then maps the section
    // there; both act only on placeholders.
    let view = unsafe {
        VirtualFree(
            base as *mut c_void,
            len,
            MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER,
        );
        MapViewOfFile3(
            slice.section.0,
            GetCurrentProcess(),
            base as *const c_void,
            (slice.index * PAGE) as u64,
            len,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_READWRITE,
            std::ptr::null_mut(),
            0,
        )
    };
    if view.Value as usize != base && len > PAGE {
        // The range spans placeholders of their own (what an unmap of
        // several views leaves, or a split): one view per page there.
        return (0..len / PAGE).all(|k| {
            let page = Slice {
                index: slice.index + k,
                section: slice.section.clone(),
            };
            map(physical, base + k * PAGE, PAGE, page)
        });
    }
    if view.Value as usize != base {
        tracing::warn!(
            address = format_args!("{base:#x}"),
            len,
            error = %std::io::Error::last_os_error(),
            "mapping physical pages failed (not a placeholder?)"
        );
        return false;
    }
    mapped(physical, base, len, slice, view)
}

/// After `view` of `slice` was mapped at `[base, base + len)`: commits it
/// and records it.
fn mapped(
    physical: &mut Physical,
    base: usize,
    len: usize,
    slice: Slice,
    view: MEMORY_MAPPED_VIEW_ADDRESS,
) -> bool {
    if !commit(base, len) {
        // SAFETY: the view just mapped, back to a placeholder.
        unsafe { UnmapViewOfFile2(GetCurrentProcess(), view, MEM_PRESERVE_PLACEHOLDER) };
        if len == PAGE && pagemap_runs() {
            physical.lone.insert(base);
        }
        return false;
    }
    victory_gpu::count_remap(9, 1);
    if len > PAGE && !physical.lone.is_empty() {
        for k in 0..len / PAGE {
            physical.lone.remove(&(base + k * PAGE));
        }
    }
    // New contents, read-write: the GPU side's copies no longer guard it.
    note_remapped(base, len);
    // What the range shows now, for the GPU side's in-place census.
    note_view(base, len, slice.section.0 as usize, (slice.index * PAGE) as u64);
    for k in 0..len / PAGE {
        let shown = physical.shown.entry(slice.key(k)).or_default();
        shown.push(base + k * PAGE);
    }
    shows_n(base, len, &slice);
    physical.views.insert(base, (len, slice));
    true
}

/// Commits the section pages a view shows (once per page, whichever view).
fn commit(base: usize, len: usize) -> bool {
    if on_wine() || phys_commit() {
        return true; // sections are committed whole there
    }
    // SAFETY: `[base, base + len)` is a view of a reserved section.
    if !unsafe { VirtualAlloc(base as *const c_void, len, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
        return true;
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(ERROR_COMMITMENT_LIMIT as i32) {
        let mut info = PERFORMANCE_INFORMATION {
            cb: size_of::<PERFORMANCE_INFORMATION>() as u32,
            ..Default::default()
        };
        // SAFETY: the buffer and size describe a writable PERFORMANCE_INFORMATION.
        let queried = unsafe { K32GetPerformanceInfo(&mut info, info.cb) } != 0;
        victory_loader::log_crash(&commit_limit_message(len, &error, queried.then_some(&info)));
    } else {
        tracing::warn!(len, %error, "committing physical pages failed");
    }
    false
}

fn commit_limit_message(
    len: usize,
    error: &std::io::Error,
    info: Option<&PERFORMANCE_INFORMATION>,
) -> String {
    let mut message = format!(
        "VIctory: Windows commit limit reached while committing {len} bytes of physical pages: {error}.\n"
    );
    if let Some(info) = info {
        use std::fmt::Write;

        let committed = info.CommitTotal * info.PageSize / MIB;
        let limit = info.CommitLimit * info.PageSize / MIB;
        let _ = writeln!(
            message,
            "System commit: {committed} MiB of {limit} MiB in use."
        );
    }
    message.push_str(
        "Close other applications or increase the Windows paging file size, then retry.\n",
    );
    message
}

/// Start and end of the bytes whose mapping `unmap(physical, base, len)`
/// and mapping `[base, base + len)` again change: the range, and the whole
/// of the views reaching out of it (`unmap` takes them away and maps what
/// stays of them again).
fn reach(physical: &Physical, base: usize, len: usize) -> (usize, usize) {
    let end = base.saturating_add(len);
    if len == 0 {
        return (base, end); // `unmap` changes nothing
    }
    let mut reach = (base, end);
    // As `unmap` finds them: the view holding `base`, the last starting
    // inside (views do not overlap).
    if let Some((&at, &(size, _))) = physical.views.range(..=base).next_back()
        && at + size > base
    {
        reach = (at, end.max(at + size));
    }
    if let Some((&at, &(size, _))) = physical.views.range(base + 1..end).next_back() {
        reach.1 = reach.1.max(at + size);
    }
    reach
}

/// Returns `[base, base + len)` to placeholders; views reaching past it
/// are mapped again outside it.
fn unmap(physical: &mut Physical, base: usize, len: usize) {
    // No bytes: no view to return. The search below would still find the
    // view holding `base` and cut it in two there, and its range of views
    // starting inside would run backwards (which `BTreeMap` panics on).
    if len == 0 {
        return;
    }
    let end = base + len;
    // Pages shown through the GPU side's translation table
    // (`VICTORY_MEM2_GPU_ONLY_NOVIEW`): the table's entries go first (a page
    // made a view there becomes a placeholder again), then the sections.
    if !physical.translated.is_empty() {
        let pages: Vec<usize> = physical.translated.range(base / PAGE * PAGE..end).map(|(&at, _)| at).collect();
        if !pages.is_empty() {
            victory_gpu::title_memory_untranslated(base, len);
            for at in pages {
                let Some(slice) = physical.translated.remove(&at) else {
                    continue;
                };
                if let std::collections::hash_map::Entry::Occupied(mut o) = physical.shown.entry(slice.key(0)) {
                    o.get_mut().retain(|&shown| shown != at);
                    if o.get().is_empty() {
                        o.remove();
                    }
                }
                hides(at, PAGE);
                note_remapped(at, PAGE);
            }
        }
    }
    // Views do not overlap: the one holding `base`, then those starting
    // inside (the GPU's page mappings make one view per page).
    let first = physical
        .views
        .range(..=base)
        .next_back()
        .filter(|(at, (size, _))| *at + size > base)
        .map(|(at, _)| *at);
    let hit: Vec<usize> = first
        .into_iter()
        .chain(physical.views.range(base + 1..end).map(|(at, _)| *at))
        .collect();
    for at in hit {
        let Some((size, slice)) = physical.views.remove(&at) else {
            continue;
        };
        for k in 0..size / PAGE {
            if let std::collections::hash_map::Entry::Occupied(mut o) =
                physical.shown.entry(slice.key(k))
            {
                o.get_mut().retain(|&shown| shown != at + k * PAGE);
                if o.get().is_empty() {
                    o.remove();
                }
            }
        }
        hides(at, size);
        note_remapped(at, size);
        victory_gpu::count_remap(10, 1);
        // SAFETY: a view we mapped; it becomes a placeholder again.
        unsafe {
            UnmapViewOfFile2(
                GetCurrentProcess(),
                MEMORY_MAPPED_VIEW_ADDRESS {
                    Value: at as *mut c_void,
                },
                MEM_PRESERVE_PLACEHOLDER,
            )
        };
        // A view of one page leaves a placeholder of one page.
        if size == PAGE && pagemap_runs() {
            physical.lone.insert(at);
        }
        if at < base {
            map(physical, at, base - at, slice.clone());
        }
        if at + size > end {
            let index = slice.index + (end - at) / PAGE;
            map(physical, end, at + size - end, Slice { index, ..slice });
        }
    }
}

/// The oldest layout we know; newer titles pass other flag values (Minecraft
/// 0x20000001) and read the same leading fields.
#[repr(C)]
#[derive(Debug, Default)]
pub struct WorkingSet {
    pub title_limit: usize,
    pub title_used: usize,
    pub tools_limit: usize,
    pub tools_used: usize,
    pub stack_used: u32,
    pub physical_pages: u32,
    pub gpu_optimal_limit: usize,
    pub gpu_optimal_used: usize,
    pub tools_gpu_optimal_limit: usize,
    pub tools_gpu_optimal_used: usize,
    pub kernel_pool_used: u32,
    pub kernel_pool_limit: u32,
}

/// The title's budget and our process's commit as what it uses. No tools
/// memory: a retail console.
// shortcut: all fields are filled whatever the flags ask for, and use is
// not split by bandwidth class (GPU-optimal use reads 0); track
// allocations per class if a title budgets from that field.
pub extern "system" fn XMemGetWorkingSetStatistics(_which: u32, out: *mut WorkingSet) -> BOOL {
    if out.is_null() {
        return 0;
    }
    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        ..Default::default()
    };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
    let (gpu_optimal, standard) = victory_gpu::console::current().title_memory();
    let title_limit = gpu_optimal + standard - TRANSFERRED.load(Ordering::Relaxed);
    let stats = WorkingSet {
        title_limit,
        title_used: if own_memory_stats() {
            // `VICTORY_TITLE_MEM_STATS=1`: the title's own memory (what it
            // committed and its physical pages), never above the limit:
            // the title subtracts used from limit unsigned (0x2474500).
            (VIRTUAL_BYTES.load(Ordering::Relaxed) + PHYSICAL_BYTES.load(Ordering::Relaxed)).min(title_limit)
        } else {
            counters.PagefileUsage
        },
        gpu_optimal_limit: gpu_optimal,
        ..Default::default()
    };
    // What the title is told, logged at the first asking and at every
    // 1,024th: "used" is all the process has committed, the layer's own
    // memory and the card's included (27 GB in run 60's gameplay against a
    // limit of 13.5 GB). A title that budgets its streaming from these two
    // figures believes it is far over its limit.
    // shortcut: the figure is not yet the title's own memory alone (its
    // heaps and the pages it mapped); count those if this line shows the
    // title asking.
    static ASKED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let asked = ASKED.fetch_add(1, Ordering::Relaxed);
    if asked.is_multiple_of(1024) {
        tracing::info!(
            asked = asked + 1,
            which = _which,
            limit_mb = stats.title_limit >> 20,
            used_mb = stats.title_used >> 20,
            "the title asks how much memory it uses"
        );
    }
    // SAFETY: checked non-null; the title passes room for the structure.
    unsafe { out.write(stats) };
    1
}

/// `VICTORY_TITLE_MEM_STATS=1` (off by default): `XMemGetWorkingSetStatistics`
/// tells the title its own memory use (its committed allocations and its
/// physical pages), not the whole process's commit (the runtime's copies
/// and the card's memory included, far above the title's limit, so the
/// title's `limit - used` wrapped around).
fn own_memory_stats() -> bool {
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("VICTORY_TITLE_MEM_STATS").is_ok_and(|v| v.trim() == "1"));
    *ON
}

/// The title's own memory, for the frame line's memory line: committed by
/// `XMemVirtualAlloc`, physical pages allocated.
pub fn title_own_bytes() -> (usize, usize) {
    (VIRTUAL_BYTES.load(Ordering::Relaxed), PHYSICAL_BYTES.load(Ordering::Relaxed))
}

/// Hands title memory (standard, in 2 MiB pages) to the system. The other
/// transfers involve tools memory, which a retail console has none of.
pub extern "system" fn XMemTransferMemory(kind: u32, size: usize) -> BOOL {
    const TITLE_TO_SYSTEM: u32 = 0;
    tracing::info!(kind, size, "XMemTransferMemory");
    if size == 0 {
        return 1;
    }
    let Some(size) = size.checked_next_multiple_of(2 * MIB) else {
        return 0;
    };
    let standard = victory_gpu::console::current().title_memory().1;
    let moved = kind == TITLE_TO_SYSTEM
        && TRANSFERRED
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |t| {
                t.checked_add(size).filter(|&t| t <= standard)
            })
            .is_ok();
    moved.into()
}

pub(crate) fn module() -> HleModule {
    HleModule {
        name: "xmem",
        dlls: &["xmem.dll"],
        exports: vec![
            hle_export!(XMemAlloc),
            hle_export!(XMemFree),
            hle_export!(XMemAllocDefault),
            hle_export!(XMemFreeDefault),
            hle_export!(XMemIsToolsMemAvailable),
            hle_export!(XMemAllocatePhysicalPages),
            hle_export!(XMemFreePhysicalPages),
            hle_export!(XMemMapPhysicalPages),
            hle_export!(XMemMapPhysicalPagesScatter),
            hle_export!(XMemVirtualAlloc),
            hle_export!(XMemVirtualQuery),
            hle_export!(XMemGetWorkingSetStatistics),
            hle_export!(XMemTransferMemory),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(address: *const c_void) -> BasicInformation {
        // SAFETY: plain data; zero is a valid value.
        let mut info: BasicInformation = unsafe { std::mem::zeroed() };
        // SAFETY: a pseudo handle needs no closing.
        let process = unsafe { GetCurrentProcess() };
        assert_eq!(XMemVirtualQuery(process, address, &mut info), 1);
        info
    }

    #[test]
    fn alloc_honours_alignment_and_frees() {
        for (field, align) in [(0, 16), (4, 16), (0xc, 4096), (0x10, 64 * 1024)] {
            let attributes = field << ALIGNMENT_SHIFT | 1 << 29; // a graphics type
            let p = XMemAlloc(100, attributes);
            assert!(!p.is_null());
            assert_eq!(p as usize % align, 0, "field {field:#x}");
            // SAFETY: 100 bytes were allocated.
            unsafe { p.cast::<u8>().write_bytes(0xab, 100) };
            XMemFree(p, attributes);
        }
        let p = XMemAlloc(4096, ALLOC_32BIT);
        assert!(!p.is_null() && (p as usize) < 1 << 32);
        XMemFree(p, ALLOC_32BIT);
    }

    #[test]
    fn virtual_alloc_meets_console_constraints() {
        const GPU_READ_WRITE: u32 = 0x2000;
        let p = XMemVirtualAlloc(
            std::ptr::null_mut(),
            4 * MIB,
            PAGES_2M | MEM_RESERVE | MEM_COMMIT,
            GRAPHICS | 7 << 12, // 40-bit addresses
            PAGE_READWRITE | GPU_READ_WRITE,
        );
        assert!(!p.is_null());
        assert_eq!(p as usize % (2 * MIB), 0);
        assert!((p as usize) < 1 << 40);
        let info = query(p);
        assert_eq!(info.host.State, MEM_COMMIT);
        assert_eq!(info.host.Protect, PAGE_READWRITE);
        assert_eq!(info.flags & GRAPHICS, GRAPHICS);
        // Write-watched, for the GPU side's copies.
        let (mut found, mut count, mut granularity) = ([0usize; 4], 4, 0);
        // SAFETY: room for `count` addresses.
        let watched = unsafe {
            windows_sys::Win32::System::Memory::GetWriteWatch(
                0,
                p,
                4 * MIB,
                found.as_mut_ptr().cast(),
                &mut count,
                &mut granularity,
            )
        };
        assert_eq!(watched, 0);
        // SAFETY: our own reservation.
        assert_ne!(unsafe { VirtualFree(p, 0, MEM_RELEASE) }, 0);
        assert_eq!(query(p).flags, 0);
        // No address field: above the 32-bit range and below 2^40, where
        // the console keeps it.
        let p = XMemVirtualAlloc(
            std::ptr::null_mut(),
            16 << 30,
            MEM_RESERVE,
            GRAPHICS,
            PAGE_NOACCESS,
        );
        assert!(!p.is_null() && (p as usize) >= 1 << 32, "{p:p}");
        assert!((p as usize) + (16 << 30) <= 1 << 40, "{p:p}");
        // SAFETY: our own reservation.
        assert_ne!(unsafe { VirtualFree(p, 0, MEM_RELEASE) }, 0);
    }

    #[test]
    fn transfers_shrink_the_title_budget() {
        let mut before = WorkingSet::default();
        assert_eq!(XMemGetWorkingSetStatistics(0x2000_0001, &mut before), 1);
        assert!(before.title_used > 0);
        assert_eq!(XMemTransferMemory(0, 128 * MIB - 1), 1);
        assert_eq!(XMemTransferMemory(2, MIB), 0, "no tools memory");
        let standard = victory_gpu::console::current().title_memory().1;
        assert_eq!(XMemTransferMemory(0, standard), 0, "over budget");
        let mut after = WorkingSet::default();
        XMemGetWorkingSetStatistics(0x2000_0001, &mut after);
        assert_eq!(before.title_limit - after.title_limit, 128 * MIB);
    }

    #[test]
    fn statistics_have_the_console_layout() {
        assert_eq!(size_of::<WorkingSet>(), 0x50);
        assert_eq!(size_of::<BasicInformation>(), 0x40);
        assert_eq!(std::mem::offset_of!(WorkingSet, gpu_optimal_limit), 0x28);
    }

    #[test]
    fn commit_limit_report_includes_request_and_system_commit() {
        let error = std::io::Error::from_raw_os_error(ERROR_COMMITMENT_LIMIT as i32);
        let info = PERFORMANCE_INFORMATION {
            CommitTotal: 512,
            CommitLimit: 768,
            PageSize: 4096,
            ..Default::default()
        };
        let report = commit_limit_message(4 * MIB, &error, Some(&info));
        assert!(report.contains("4194304 bytes"));
        assert!(report.contains("2 MiB of 3 MiB in use"));
        let fallback = commit_limit_message(4 * MIB, &error, None);
        for report in [&report, &fallback] {
            assert!(report.contains("os error 1455"));
            assert!(report.contains("Windows paging file size"));
        }
        assert!(!fallback.contains("System commit:"));
    }

    #[test]
    fn physical_pages_map_alias_and_unmap() {
        let mut pages = [0usize; 3];
        let mut n = 3;
        assert_eq!(XMemAllocatePhysicalPages(0, &mut n, pages.as_mut_ptr()), 1);
        assert_eq!(n, 3);
        // Packed into 21 bits as `number >> 4`, as the title keeps them.
        for p in pages {
            assert_eq!((p >> 4 & 0x1f_ffff) << 4, p);
        }
        // Reservations of 64 KiB pages, as titles make them for mapping.
        let reserve = || {
            XMemVirtualAlloc(
                std::ptr::null_mut(),
                3 * PAGE,
                MEM_RESERVE | PAGES_64K,
                0,
                PAGE_READWRITE,
            )
        };
        let (a, b) = (reserve(), reserve());
        assert!(!a.is_null() && !b.is_null());
        let at = |p: *mut c_void, page: usize| p.wrapping_byte_add(page * PAGE);
        assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);

        // The same page at a second address shows the same bytes.
        assert_eq!(
            XMemMapPhysicalPages(at(b, 1), 1, pages[2..].as_ptr()),
            at(b, 1)
        );
        // SAFETY: both pages are mapped read-write.
        unsafe {
            at(a, 2).cast::<u8>().write(7);
            assert_eq!(at(b, 1).cast::<u8>().read(), 7);
            a.cast::<u8>().write(1);
        }
        // Writes into pages (DirectStorage's) show through their views.
        assert!(write_physical(&pages[2..], 1, &[9]));
        // SAFETY: mapped read-write.
        assert_eq!(unsafe { at(b, 1).cast::<u8>().add(1).read() }, 9);

        // Unmapping the middle page keeps its neighbours.
        assert_eq!(
            XMemMapPhysicalPages(at(a, 1), 1, std::ptr::null()),
            at(a, 1)
        );
        assert_eq!(query(at(a, 1)).host.State, MEM_RESERVE);
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(query(at(a, 2)).host.State, MEM_COMMIT);
        // SAFETY: still mapped.
        assert_eq!(unsafe { at(a, 2).cast::<u8>().read() }, 7);

        // A scattered page lands in the hole.
        let addresses = [at(a, 1)];
        assert_eq!(
            XMemMapPhysicalPagesScatter(addresses.as_ptr(), 1, pages.as_ptr()),
            1
        );
        // SAFETY: page 0 is now mapped at both `a` and `a + PAGE`.
        assert_eq!(unsafe { at(a, 1).cast::<u8>().read() }, 1);
        // Page number 0 unmaps.
        assert_eq!(
            XMemMapPhysicalPagesScatter(addresses.as_ptr(), 1, [0usize].as_ptr()),
            1
        );
        assert_eq!(query(at(a, 1)).host.State, MEM_RESERVE);
        // The GPU's page-mapping copy: what `a + 2 pages` shows, at `b`.
        assert!(map_alias(b as u64, at(a, 2) as u64, PAGE as u64));
        // SAFETY: mapped read-write.
        assert_eq!(unsafe { b.cast::<u8>().read() }, 7);
        assert!(map_alias(b as u64, 0, PAGE as u64));
        assert_eq!(query(b).host.State, MEM_RESERVE);
        assert!(!map_alias(b as u64, at(a, 1) as u64, PAGE as u64));
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
        assert_eq!(XMemIsToolsMemAvailable(), 0);
    }

    /// A thread that reads a mapped page in place, as the GPU side reads a
    /// texture, never meets the moment in which the runtime has taken the
    /// page's view away to map it again: mapping the same pages at the same
    /// address unmaps the view and maps a new one, and without the lock the
    /// reader faults on a page "reserved, never committed" (the crashes of
    /// runs 71 and 74).
    #[test]
    fn a_read_in_place_never_meets_the_gap_of_a_remapping() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let mut pages = [0usize; 3];
        let mut n = 3;
        assert_eq!(XMemAllocatePhysicalPages(0, &mut n, pages.as_mut_ptr()), 1);
        let a = XMemVirtualAlloc(
            std::ptr::null_mut(),
            3 * PAGE,
            MEM_RESERVE | PAGES_64K,
            0,
            PAGE_READWRITE,
        );
        assert!(!a.is_null());
        assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);
        // SAFETY: mapped read-write.
        unsafe { a.cast::<u8>().write(1) };
        let first = a as usize;
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    let _reading = victory_gpu::title_memory_reading(first, 1);
                    // SAFETY: the page is mapped whenever no change of
                    // mapping is under way, and none is while `_reading`
                    // is held.
                    assert_eq!(unsafe { (first as *const u8).read_volatile() }, 1);
                    reads += 1;
                }
                reads
            })
        };
        for _ in 0..2000 {
            // The same pages at the same address: the view goes and comes
            // back, with the same bytes.
            assert_eq!(XMemMapPhysicalPages(a, 3, pages.as_ptr()), a);
        }
        stop.store(true, Ordering::Relaxed);
        assert!(reader.join().expect("the reader never faulted") > 0);
        assert_eq!(XMemMapPhysicalPages(a, 3, std::ptr::null()), a);
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    }

    #[test]
    fn mapping_no_pages_changes_nothing() {
        let pages = allocate(2);
        let a = XMemVirtualAlloc(
            std::ptr::null_mut(),
            2 * PAGE,
            MEM_RESERVE | PAGES_64K,
            0,
            PAGE_READWRITE,
        );
        assert!(!a.is_null());
        assert_eq!(XMemMapPhysicalPages(a, 2, pages.as_ptr()), a);
        // SAFETY: both pages are mapped read-write.
        unsafe { a.cast::<u8>().write(5) };
        let second = a.wrapping_byte_add(PAGE);

        // A count of 0 at the start of a view, inside it and past it: with
        // pages, without (an unmap), and as the GPU's page-mapping copy.
        for at in [a, second, a.wrapping_byte_add(2 * PAGE)] {
            assert_eq!(XMemMapPhysicalPages(at, 0, pages.as_ptr()), at);
            assert_eq!(XMemMapPhysicalPages(at, 0, std::ptr::null()), at);
            assert!(map_alias(at as u64, a as u64, 0));
            assert!(map_alias(at as u64, 0, 0));
        }

        // The view is still the one view it was.
        let view = physical().views.get(&(a as usize)).map(|v| v.0);
        assert_eq!(view, Some(2 * PAGE));
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(query(second).host.State, MEM_COMMIT);
        // SAFETY: still mapped.
        assert_eq!(unsafe { a.cast::<u8>().read() }, 5);

        // A count of 1 still unmaps that page alone.
        assert_eq!(XMemMapPhysicalPages(second, 1, std::ptr::null()), second);
        assert_eq!(query(second).host.State, MEM_RESERVE);
        assert_eq!(query(a).host.State, MEM_COMMIT);
        assert_eq!(XMemMapPhysicalPages(a, 1, std::ptr::null()), a);
        assert_eq!(XMemFreePhysicalPages(2, pages.as_ptr()), 1);
    }

    /// `n` pages of one new allocation.
    fn allocate(n: usize) -> Vec<usize> {
        let (mut pages, mut given) = (vec![0; n], n);
        assert_eq!(
            XMemAllocatePhysicalPages(0, &mut given, pages.as_mut_ptr()),
            1
        );
        assert_eq!(given, n);
        pages
    }

    /// What each page of `n` at `base` shows: its first byte, or `None`
    /// for a hole.
    fn shown(base: *mut c_void, n: usize) -> Vec<Option<u8>> {
        (0..n)
            .map(|k| {
                let p = base.wrapping_byte_add(k * PAGE);
                // SAFETY: read only where committed.
                (query(p).host.State == MEM_COMMIT).then(|| unsafe { p.cast::<u8>().read() })
            })
            .collect()
    }

    fn placeholder(n: usize) -> *mut c_void {
        let p = XMemVirtualAlloc(std::ptr::null_mut(), n * PAGE, MEM_RESERVE | PAGES_64K, 0, PAGE_READWRITE);
        assert!(!p.is_null());
        p
    }

    /// `VICTORY_PAGEMAP_RUNS`: a Scatter batch done as groups maps what the
    /// page-by-page path maps, in the title's order: runs of consecutive
    /// pages, pages out of order, unmaps in between, an address twice, and
    /// a page already mapped there before.
    #[test]
    fn a_batched_scatter_maps_what_page_by_page_maps() {
        let pages = allocate(8);
        for (k, &p) in pages.iter().enumerate() {
            assert!(write_physical(&[p], 0, &[10 + k as u8]));
        }
        let (a, b) = (placeholder(12), placeholder(12));
        // Both start with pages 0..5 mapped as one view at slots 2..7: a
        // batch that cuts it must keep what stays of it.
        assert_eq!(XMemMapPhysicalPages(a.wrapping_byte_add(2 * PAGE), 5, pages.as_ptr()), a.wrapping_byte_add(2 * PAGE));
        assert_eq!(XMemMapPhysicalPages(b.wrapping_byte_add(2 * PAGE), 5, pages.as_ptr()), b.wrapping_byte_add(2 * PAGE));
        // (page slot in the reservation, page index or 8 = unmap)
        let entries: [(usize, usize); 11] =
            [(0, 5), (1, 6), (2, 7), (3, 8), (4, 1), (5, 0), (7, 3), (8, 4), (9, 8), (10, 8), (7, 2)];
        let numbers: Vec<usize> = entries.iter().map(|&(_, i)| if i == 8 { 0 } else { pages[i] }).collect();
        let at = |base: *mut c_void| -> Vec<usize> { entries.iter().map(|&(slot, _)| base as usize + slot * PAGE).collect() };
        let (to_a, to_b) = (at(a), at(b));
        // Page by page (the switch is off in tests).
        assert_eq!(XMemMapPhysicalPagesScatter(to_a.as_ptr().cast(), entries.len(), numbers.as_ptr()), 1);
        assert!(scatter_batched(&to_b, Some(&numbers)));
        assert_eq!(shown(a, 12), shown(b, 12));
        assert_eq!(shown(b, 12)[7], Some(12)); // the last entry for slot 7 wins
        assert_eq!(shown(b, 12)[6], Some(14)); // what stays of the cut view
        assert_eq!(shown(b, 12)[3], None);
        // A whole-batch unmap (no pages) joins into one group.
        let all: Vec<usize> = (0..12).map(|k| b as usize + k * PAGE).collect();
        assert!(scatter_batched(&all, None));
        assert!(shown(b, 12).iter().all(Option::is_none));
        // Writes still reach every place a page is shown.
        assert!(scatter_batched(&[b as usize, b as usize + PAGE], Some(&[pages[3], pages[3]])));
        assert!(write_physical(&pages[3..4], 0, &[99]));
        assert_eq!(shown(b, 2), vec![Some(99), Some(99)]);
        assert!(scatter_batched(&all, None));
        let all_a: Vec<usize> = (0..12).map(|k| a as usize + k * PAGE).collect();
        assert_eq!(XMemMapPhysicalPagesScatter(all_a.as_ptr().cast(), 12, std::ptr::null()), 1);
        // An unknown page fails the batch, as it fails the call.
        assert!(!scatter_batched(&[b as usize], Some(&[0x7fff_fff0])));
        assert_eq!(XMemFreePhysicalPages(8, pages.as_ptr()), 1);
    }

    /// `VICTORY_PAGEMAP_RUNS`: a Scatter batch gives each page a view of its
    /// own, as page by page does, so remapping one page of it later never
    /// takes its neighbours away. With one view over the run, unmapping a
    /// page took the whole view and mapped the rest again, and the title,
    /// reading a neighbour meanwhile, faulted on memory "reserved, never
    /// committed" (fast-check runs R3-crash, R4-crash, R3b, 120-160 s in).
    #[test]
    fn a_batched_scatter_makes_a_view_per_page() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let pages = allocate(3);
        for (k, &p) in pages.iter().enumerate() {
            assert!(write_physical(&[p], 0, &[40 + k as u8]));
        }
        let b = placeholder(3);
        let at: Vec<usize> = (0..3).map(|k| b as usize + k * PAGE).collect();
        assert!(scatter_batched(&at, Some(&pages)));
        {
            let physical = physical();
            for &a in &at {
                assert_eq!(physical.views.get(&a).map(|v| v.0), Some(PAGE), "a view of its own at {a:#x}");
            }
        }
        assert_eq!(shown(b, 3), vec![Some(40), Some(41), Some(42)]);
        // The middle page remapped again and again while a thread reads its
        // neighbours in place, as the title does, without any lock.
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let reader = {
            let stop = stop.clone();
            let (first, last) = (at[0], at[2]);
            std::thread::spawn(move || {
                let mut reads = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    // SAFETY: mapped throughout; only the middle page is remapped.
                    let (x, y) = unsafe { ((first as *const u8).read_volatile(), (last as *const u8).read_volatile()) };
                    assert_eq!((x, y), (40, 42));
                    reads += 1;
                }
                reads
            })
        };
        for k in 0..500 {
            let page = if k % 2 == 0 { 0 } else { pages[1] };
            assert!(scatter_batched(&at[1..2], Some(&[page])));
        }
        stop.store(true, Ordering::Relaxed);
        assert!(reader.join().expect("a neighbour stayed mapped") > 0);
        assert!(scatter_batched(&at, None));
        assert_eq!(XMemFreePhysicalPages(3, pages.as_ptr()), 1);
    }

    /// `VICTORY_MEM2_GPU_ONLY_NOVIEW`: page mappings into a GPU-only
    /// reservation make no views; a touch in place makes the page a view
    /// after all (the fallback), with the bytes the page holds; physical
    /// writes still reach it; unmapping takes table entries and views away.
    /// Run alone (`mem2_`), as the switch is read once per process.
    #[test]
    fn mem2_gpu_only_mappings_are_translated_and_made_views_on_touch() {
        use victory_core::pagemap::AliasOp;
        // SAFETY: set before any thread of this process reads it (the
        // switch is read once; other tests do not map into GPU-only
        // reservations, which is where it is first read).
        unsafe {
            std::env::set_var("VICTORY_MEM2_GPU_ONLY_NOVIEW", "1");
            std::env::set_var("VICTORY_MEM2_GPU_ONLY_NOSNAP", "1");
        }
        if !victory_gpu::mem2_gpu_only_noview() {
            eprintln!("switch read off earlier in this process: run this test alone");
            return;
        }
        let pages = allocate(4);
        for (k, &p) in pages.iter().enumerate() {
            assert!(write_physical(&[p], 0, &[70 + k as u8]));
        }
        let pool = placeholder(4);
        assert_eq!(XMemMapPhysicalPages(pool, 4, pages.as_ptr()), pool);
        // A GPU-only reservation as the title makes it (no CPU access, GPU bit).
        let g = XMemVirtualAlloc(std::ptr::null_mut(), 4 * PAGE, MEM_RESERVE | PAGES_64K, 0, 0x801);
        assert!(!g.is_null());
        let p = PAGE as u64;
        let at = |k: usize| g.wrapping_byte_add(k * PAGE);
        assert!(map_alias_batch(&[AliasOp { to: g as u64, from: Some(pool as u64 + p), len: 3 * p, repeat: false }]));
        // No views: placeholders still, the pages in the table.
        for k in 0..4 {
            assert_ne!(query(at(k)).host.State, MEM_COMMIT, "page {k} has no view");
        }
        assert_eq!(physical().translated.len(), 3);
        // The CPU-alias record: the pages are shown in the pool, a CPU view.
        let nosnap = victory_gpu::mem2_nosnap();
        assert!(!cpu_free(g as usize, PAGE));
        // A touch in place makes the page a view, showing the page's bytes.
        // SAFETY: a page shown through the table; the fault makes it a view.
        assert_eq!(unsafe { at(1).cast::<u8>().read_volatile() }, 72);
        assert_eq!(query(at(1)).host.State, MEM_COMMIT);
        assert_ne!(query(at(0)).host.State, MEM_COMMIT, "only the page touched");
        // A physical write reaches the view and the translated page alike.
        assert!(write_physical(&pages[2..3], 0, &[9]));
        // SAFETY: a view now.
        assert_eq!(unsafe { at(1).cast::<u8>().read_volatile() }, 9);
        // SAFETY: as above, made a view by this read.
        assert_eq!(unsafe { at(0).cast::<u8>().read_volatile() }, 71);
        // An alias from a translated page resolves through the table.
        assert!(map_alias_batch(&[AliasOp { to: g as u64 + 3 * p, from: Some(g as u64 + 2 * p), len: p, repeat: false }]));
        assert_eq!(physical().translated.len(), 4);
        // Unmapping takes the table's entries and the views made away.
        assert!(map_alias_batch(&[AliasOp { to: g as u64, from: None, len: 4 * p, repeat: false }]));
        assert!(physical().translated.range(g as usize..g as usize + 4 * PAGE).next().is_none());
        for k in 0..4 {
            assert_ne!(query(at(k)).host.State, MEM_COMMIT, "page {k} unmapped");
        }
        // Translated again where a view was made before.
        assert!(map_alias_batch(&[AliasOp { to: g as u64, from: Some(pool as u64), len: p, repeat: false }]));
        assert_ne!(query(at(0)).host.State, MEM_COMMIT);
        // The pool view goes: the page is shown at the GPU-only address
        // alone, which the CPU cannot write.
        if nosnap {
            assert!(!cpu_free(g as usize, PAGE));
            assert_eq!(XMemMapPhysicalPages(pool, 1, std::ptr::null()), pool);
            assert!(cpu_free(g as usize, PAGE));
            assert!(!cpu_free(pool as usize, PAGE), "not a GPU-only reservation");
        }
        // SAFETY: shown through the table; a touch makes it a view.
        assert_eq!(unsafe { at(0).cast::<u8>().read_volatile() }, 70);
        assert!(map_alias_batch(&[AliasOp { to: g as u64, from: None, len: p, repeat: false }]));
        assert_ne!(query(at(0)).host.State, MEM_COMMIT);
        assert_eq!(XMemMapPhysicalPages(pool, 4, std::ptr::null()), pool);
        assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
    }

    /// Turns [`pagemap_runs`] on for the tests that want it (it only ever
    /// maps the same as off, so other tests running meanwhile are fine).
    pub(super) static PAGEMAP_RUNS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    /// With the switch on: single pages unmapped leave lone placeholders,
    /// mapped again without a split, batches notified at their end, and a
    /// batch that holds several ranges of bytes (its groups) as one change.
    #[test]
    fn lone_placeholders_map_without_a_split_and_batches_hold_their_ranges() {
        PAGEMAP_RUNS.store(true, std::sync::atomic::Ordering::Relaxed);
        let pages = allocate(4);
        for (k, &p) in pages.iter().enumerate() {
            assert!(write_physical(&[p], 0, &[60 + k as u8]));
        }
        let b = placeholder(8);
        let slot = |k: usize| b as usize + k * PAGE;
        // Pages one at a time at slots 0, 2, 4, 6 (groups of one: the
        // batch holds four ranges of bytes).
        assert!(scatter_batched(&[slot(0), slot(2), slot(4), slot(6)], Some(&pages)));
        assert_eq!(shown(b, 8), vec![Some(60), None, Some(61), None, Some(62), None, Some(63), None]);
        // Unmapped one by one: lone placeholders now.
        assert!(scatter_batched(&[slot(0), slot(2), slot(4), slot(6)], None));
        assert!(physical().lone.contains(&slot(2)));
        // Mapped again there, out of order: no split needed.
        assert!(scatter_batched(&[slot(4), slot(2)], Some(&[pages[0], pages[3]])));
        assert!(!physical().lone.contains(&slot(2)));
        assert_eq!(shown(b, 8), vec![None, None, Some(63), None, Some(60), None, None, None]);
        // A run over lone placeholders and plain ones (slots 2..6: views
        // and placeholders of their own) still maps.
        assert!(scatter_batched(&[slot(2), slot(3), slot(4), slot(5)], Some(&pages)));
        assert_eq!(shown(b, 8), vec![None, None, Some(60), Some(61), Some(62), Some(63), None, None]);
        // A read in place elsewhere goes on while a batch is held.
        {
            let _reading = victory_gpu::title_memory_reading(slot(7), 1);
            assert!(scatter_batched(&[slot(0)], Some(&pages[..1])));
        }
        assert!(scatter_batched(&(0..8).map(slot).collect::<Vec<_>>(), None));
        assert!(shown(b, 8).iter().all(Option::is_none));
        assert_eq!(XMemFreePhysicalPages(4, pages.as_ptr()), 1);
    }

    /// `VICTORY_PAGEMAP_RUNS`: a page-mapping batch, joined, maps what the
    /// queue's range-by-range calls map, a page repeated included.
    #[test]
    fn a_batched_page_mapping_maps_what_range_by_range_maps() {
        use victory_core::pagemap::AliasOp;
        let pages = allocate(6);
        for (k, &p) in pages.iter().enumerate() {
            assert!(write_physical(&[p], 0, &[40 + k as u8]));
        }
        // The pool: one view of all its pages, as the title keeps it.
        let pool = placeholder(6);
        assert_eq!(XMemMapPhysicalPages(pool, 6, pages.as_ptr()), pool);
        let (a, b) = (placeholder(10), placeholder(10));
        let p = PAGE as u64;
        let ops = |to: *mut c_void| {
            let (d, s) = (to as u64, pool as u64);
            vec![
                AliasOp { to: d, from: Some(s), len: 2 * p, repeat: false },
                AliasOp { to: d + 2 * p, from: Some(s + 2 * p), len: p, repeat: false },
                AliasOp { to: d + 3 * p, from: Some(s + 5 * p), len: 3 * p, repeat: true },
                AliasOp { to: d + 6 * p, from: None, len: p, repeat: false },
                AliasOp { to: d + 7 * p, from: Some(s + 4 * p), len: 2 * p, repeat: false },
                AliasOp { to: d + 9 * p, from: Some(s), len: p, repeat: false },
                AliasOp { to: d + 6 * p, from: Some(s + p), len: p, repeat: false },
            ]
        };
        for op in ops(a) {
            let ok = match (op.from, op.repeat) {
                (None, _) => map_alias(op.to, 0, op.len),
                (Some(f), false) => map_alias(op.to, f, op.len),
                (Some(f), true) => (0..op.pages()).all(|k| map_alias(op.to + k * p, f, p)),
            };
            assert!(ok);
        }
        assert!(map_alias_batch(&ops(b)));
        assert_eq!(shown(a, 10), shown(b, 10));
        assert_eq!(shown(b, 10), vec![Some(40), Some(41), Some(42), Some(45), Some(45), Some(45), Some(41), Some(44), Some(45), Some(40)]);
        // Through the alias, the pool's page is the same memory.
        assert!(write_physical(&pages[5..6], 0, &[7]));
        assert_eq!(shown(b, 10)[4], Some(7));
        // Unmapping it all.
        assert!(map_alias_batch(&[AliasOp { to: b as u64, from: None, len: 10 * p, repeat: false }]));
        assert!(shown(b, 10).iter().all(Option::is_none));
        // A source that shows nothing fails.
        assert!(!map_alias_batch(&[AliasOp { to: b as u64, from: Some(a as u64 + 6 * p + 0x10_0000_0000), len: p, repeat: false }]));
        assert_eq!(XMemFreePhysicalPages(6, pages.as_ptr()), 1);
    }

    /// The bytes of one page, through a view of our own.
    fn page_bytes(page: usize) -> Vec<u8> {
        let slice = physical().pages.get(&page).cloned().expect("known page");
        View::map(&slice, 1).expect("mapped").bytes().to_vec()
    }

    #[test]
    fn runs_group_consecutive_pages_of_one_allocation() {
        let (a, b) = (allocate(3), allocate(1));
        let lengths = |pages: &[usize], offset, len| {
            runs(pages, offset, len).map(|r| r.iter().map(|&(_, n)| n).collect::<Vec<_>>())
        };
        assert_eq!(lengths(&a, 0, 3 * PAGE), Some(vec![3]));
        // Only the pages the bytes reach.
        assert_eq!(lengths(&a, PAGE + 5, 10), Some(vec![1]));
        assert_eq!(lengths(&a, PAGE - 1, 2), Some(vec![2]));
        assert_eq!(lengths(&a, 0, 0), Some(vec![]));
        // Out of order, or across allocations: one run each.
        assert_eq!(
            lengths(&[a[0], a[2], a[1]], 0, 3 * PAGE),
            Some(vec![1, 1, 1])
        );
        assert_eq!(lengths(&[a[1], a[2], b[0]], 0, 3 * PAGE), Some(vec![2, 1]));
        // A slice that follows on in another section starts a run.
        assert_eq!(lengths(&[b[0], a[1]], 0, 2 * PAGE), Some(vec![1, 1]));
        // Too few pages for the bytes, or an unknown one.
        assert_eq!(lengths(&a, 0, 3 * PAGE + 1), None);
        assert_eq!(lengths(&[a[0], 0x10], 0, 2 * PAGE), None);
        assert_eq!(lengths(&a, usize::MAX, 2), None);
        assert_eq!(XMemFreePhysicalPages(3, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }

    #[test]
    fn writes_cross_runs_and_allocations() {
        let (a, b) = (allocate(2), allocate(1));
        let pages = [a[0], a[1], b[0]];
        let data: Vec<u8> = (0..2 * PAGE + 64).map(|i| (i * 7 % 251) as u8).collect();
        let offset = PAGE - 32;
        // Past the third page: the second run is too short.
        assert!(!write_physical(&pages, offset, &data));
        assert!(write_physical(&pages, offset, &data[..2 * PAGE]));
        let seen: Vec<u8> = pages.iter().flat_map(|&p| page_bytes(p)).collect();
        assert!(seen[..offset].iter().all(|&b| b == 0), "before the offset");
        assert_eq!(seen[offset..offset + 2 * PAGE], data[..2 * PAGE]);
        assert!(seen[offset + 2 * PAGE..].iter().all(|&b| b == 0), "after");
        assert!(
            !write_physical(&[a[0], 0x10], 0, &data[..PAGE + 1]),
            "unknown page"
        );
        assert_eq!(XMemFreePhysicalPages(2, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }

    /// Pages know where they are shown, as views come, are cut and go.
    #[test]
    fn pages_know_where_they_are_shown() {
        let (mut count, mut pages) = (4usize, [0usize; 4]);
        assert_eq!(XMemAllocatePhysicalPages(0, &mut count, pages.as_mut_ptr()), 1);
        let kind = MEM_RESERVE | PAGES_64K;
        let base = XMemVirtualAlloc(std::ptr::null_mut(), 8 * PAGE, kind, 3, PAGE_READWRITE);
        assert!(!base.is_null());
        let at = base as usize;
        let shown = |page: usize| {
            let physical = physical();
            let key = physical.pages[&page].key(0);
            let mut shown = physical.shown.get(&key).cloned().unwrap_or_default();
            shown.sort_unstable();
            shown
        };
        assert_eq!(XMemMapPhysicalPages(base, 4, pages.as_ptr()), base);
        assert_eq!(shown(pages[2]), [at + 2 * PAGE]);
        // Shown twice; the first view cut in two.
        let second = (at + 5 * PAGE) as *mut c_void;
        assert_eq!(XMemMapPhysicalPages(second, 1, &pages[2]), second);
        assert_eq!(shown(pages[2]), [at + 2 * PAGE, at + 5 * PAGE]);
        let gone = (at + 2 * PAGE) as *mut c_void;
        XMemMapPhysicalPages(gone, 1, std::ptr::null());
        assert_eq!(shown(pages[2]), [at + 5 * PAGE]);
        assert_eq!(shown(pages[1]), [at + PAGE]);
        assert_eq!(shown(pages[3]), [at + 3 * PAGE]);
        XMemMapPhysicalPages(base, 8, std::ptr::null());
        assert!(pages.iter().all(|&p| shown(p).is_empty()));
    }

    #[test]
    fn page_windows_cover_one_run_in_place() {
        let (a, b) = (allocate(3), allocate(1));
        let mut window = page_window(&a, PAGE + 3, PAGE).expect("one run");
        assert_eq!(window.bytes().len(), PAGE);
        window.bytes().fill(0x5a);
        drop(window);
        let (second, third) = (page_bytes(a[1]), page_bytes(a[2]));
        assert!(second[..3].iter().all(|&b| b == 0));
        assert!(second[3..].iter().all(|&b| b == 0x5a));
        assert!(third[..3].iter().all(|&b| b == 0x5a));
        assert!(third[3..].iter().all(|&b| b == 0));
        // Several runs, or an unknown page: no window.
        assert!(page_window(&[a[0], b[0]], 0, PAGE + 1).is_none());
        assert!(page_window(&[a[1], a[0]], 0, PAGE + 1).is_none());
        assert!(page_window(&[0x10], 0, 1).is_none());
        // One page of several is still one run.
        assert!(page_window(&[a[0], b[0]], 0, PAGE).is_some());
        assert_eq!(XMemFreePhysicalPages(3, a.as_ptr()), 1);
        assert_eq!(XMemFreePhysicalPages(1, b.as_ptr()), 1);
    }
}
