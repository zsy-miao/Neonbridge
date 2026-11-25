//! TUXBOX_MEM_DIR (default on, =0 off): a page directory over the 40-bit
//! title address space, so translated compute stages read title memory
//! (BVH nodes, `base<<8` headers, flat loads with no getpc) through the
//! existing GPU copies of it. Ported from MESIA-Unified (GPL-3.0, credit)
//! crates/mesia-gpu/src/memory.rs and draw.rs:948-961; the shader half is
//! xshader's `tb_mem_load` (mesia-shader emit/lower.rs:2632-2695).
//!
//! The binding contract with xshader:
//! - `ByteAddressBuffer tb_mem_dir : register(t0, space32764)`: [`PAGES`]
//!   entries of 8 bytes `{copy, start}`, one per 64 KiB title page
//!   (page = `(lo >> 16) | ((hi & 0xff) << 16)`, clamped to PAGES - 1).
//!   copy 0: the page has no copy yet.
//! - `RWByteAddressBuffer tb_mem_faults : register(u0, space32764)`:
//!   [`FAULTS`] u32; a missing page is stored at `((page % (FAULTS-1)) + 1) * 4`
//!   (entry 0 is a sink). The runtime makes those pages ready at the next
//!   dispatch after its periodic read (the ref reads once a frame).
//! - `cbuffer tb_mem : register(b0, space32764) { uint tb_mem_base; }`:
//!   the heap index of the copy run; copy n is
//!   `ResourceDescriptorHeap[tb_mem_base + n]` as a RWByteAddressBuffer,
//!   read at byte `start + (lo & 0xffff)`. Slot `tb_mem_base` is a null UAV.
//!
//! Copies are the title-address imports (views.rs buffer_at with unordered
//! access), so no refresh or write-back is needed: the import is the title
//! memory. A page is filled only from an import that holds it whole, and an
//! entry only goes from empty to filled (the ref's rule: a list recorded
//! earlier may still read it). At most [`COPIES`] copies; past that pages
//! stay missing (logged once). The run is [`SLOTS`] private slots past the
//! root-table ring of each shader-visible CBV/SRV/UAV heap (root_tables).

use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_TYPELESS, DXGI_SAMPLE_DESC};
use windows::core::Interface;

pub(crate) const SPACE: u32 = tuxbox_dxbc::reserved::MEM_DIR.space;
pub(crate) const PAGES: usize = 1 << 21;
pub(crate) const FAULTS: usize = 4096;
pub(crate) const COPIES: usize = 1023;
/// Heap slots of the copy run: the null slot and one per copy.
pub(crate) const SLOTS: u32 = COPIES as u32 + 1;
const PAGE: u64 = 1 << 16;
/// The span made ready past each named address (the ref's next 4 MiB cell).
const CELL: u64 = 4 << 20;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_MEM_DIR") != Some("0"))
}

/// The stage declares the directory SRV at t0, space32764.
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, _upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && lower == 0
        })
}

struct Copy {
    native: ID3D12Resource,
    /// The import: once the registry drops it (retired on a remap, or
    /// replaced), the copy is retired (sweep).
    import: std::sync::Weak<super::views::TextureAddress>,
    /// The title address at byte 0, and the bytes.
    base: u64,
    bytes: u64,
}

/// A retired copy's resource, kept until lists recorded with its view have
/// finished (RETIRE_GRACE), then released and its number reused.
struct Retired {
    _native: ID3D12Resource,
    number: u32,
    at: std::time::Instant,
}
const RETIRE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

struct State {
    directory: ID3D12Resource,
    /// The directory's mapping: PAGES * 2 u32.
    entries: *mut u32,
    faults: ID3D12Resource,
    /// An all-empty directory for heaps with no run (paged arenas): every
    /// page reads as missing, so no copy slot is indexed.
    empty: ID3D12Resource,
    /// The fault buffer's mapping (CPU write-back memory): FAULTS u32.
    fault_words: *mut u32,
    /// Copy n + 1 is copies[n] (None: retired or free).
    copies: Vec<Option<Copy>>,
    by_resource: HashMap<usize, u32>,
    retired: Vec<Retired>,
    /// Numbers free to reuse (their grace is over).
    free: Vec<u32>,
    /// Per heap: the resource each run slot views (usize::MAX: none yet).
    runs: HashMap<usize, Vec<usize>>,
    last_faults: Option<std::time::Instant>,
    last_sweep: Option<std::time::Instant>,
}
// The mappings stay valid while the resources live in this state.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static FAILED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn buffer(device: &ID3D12Device, bytes: u64, page: D3D12_CPU_PAGE_PROPERTY, flags: D3D12_RESOURCE_FLAGS)
    -> windows::core::Result<(ID3D12Resource, *mut u32)> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_CUSTOM, CPUPageProperty: page,
        MemoryPoolPreference: D3D12_MEMORY_POOL_L0, CreationNodeMask: 1, VisibleNodeMask: 1,
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Alignment: 0, Width: bytes, Height: 1,
        DepthOrArraySize: 1, MipLevels: 1, Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, Flags: flags,
    };
    let mut out: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut out)?;
        let out = out.ok_or(windows::Win32::Foundation::E_POINTER)?;
        let mut mapped = core::ptr::null_mut();
        out.Map(0, None, Some(&mut mapped))?;
        core::ptr::write_bytes(mapped.cast::<u8>(), 0, bytes as usize);
        Ok((out, mapped.cast()))
    }
}

fn with_state<R>(device: &ID3D12Device, f: impl FnOnce(&mut State) -> R) -> Option<R> {
    if !enabled() || FAILED.load(std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if state.is_none() {
        let made = buffer(device, (PAGES * 8) as u64, D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE, D3D12_RESOURCE_FLAG_NONE)
            .and_then(|d| Ok((d, buffer(device, (FAULTS * 4) as u64, D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)?)))
            .and_then(|d| Ok((d, buffer(device, (PAGES * 8) as u64, D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE, D3D12_RESOURCE_FLAG_NONE)?.0)));
        match made {
            Ok((((directory, entries), (faults, fault_words)), empty)) => {
                crate::diagnostics::log_line(format_args!(
                    "memory directory: {} pages, {FAULTS} faults, {COPIES} copies (TUXBOX_MEM_DIR=0 off)", PAGES));
                *state = Some(State {
                    directory, entries, faults, fault_words, empty, copies: Vec::new(),
                    by_resource: HashMap::new(), retired: Vec::new(), free: Vec::new(),
                    runs: HashMap::new(), last_faults: None, last_sweep: None,
                });
            }
            Err(error) => {
                FAILED.store(true, std::sync::atomic::Ordering::Relaxed);
                crate::diagnostics::log_line(format_args!("memory directory: not made ({error}); stages read zeros"));
                return None;
            }
        }
    }
    state.as_mut().map(f)
}

impl State {
    fn filled(&self, page: usize) -> bool {
        unsafe { self.entries.add(page * 2).read_volatile() != 0 }
    }

    /// Numbers the import (once) and fills every empty page it holds whole.
    /// `base`: the title address at byte 0 of `native`.
    fn add(&mut self, native: &ID3D12Resource, import: std::sync::Weak<super::views::TextureAddress>, base: u64) {
        let key = native.as_raw() as usize;
        let bytes = unsafe { native.GetDesc() }.Width;
        let copy = match self.by_resource.get(&key) {
            Some(&n) => n,
            None => {
                let n = match self.free.pop() {
                    Some(n) => n,
                    None if self.copies.len() < COPIES => {
                        self.copies.push(None);
                        self.copies.len() as u32
                    }
                    None => {
                        static FULL: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FULL, || format!(
                            "memory directory: {COPIES} copies live, later pages stay missing until some retire"));
                        return;
                    }
                };
                self.copies[n as usize - 1] = Some(Copy { native: native.clone(), import, base, bytes });
                self.by_resource.insert(key, n);
                n
            }
        };
        let first = base.div_ceil(PAGE);
        let end = (base + bytes) / PAGE;
        for page in first..end.min(PAGES as u64 - 1) {
            let page = page as usize;
            if self.filled(page) {
                continue;
            }
            let start = (page as u64 * PAGE - base) as u32;
            // The start first, then the copy (the ref's order): a reader
            // that sees the copy sees its start.
            unsafe {
                self.entries.add(page * 2 + 1).write_volatile(start);
                self.entries.add(page * 2).write_volatile(copy);
            }
        }
    }

    /// Empties the copy's pages and keeps its resource for the grace.
    fn retire(&mut self, n: u32) {
        let Some(copy) = self.copies[n as usize - 1].take() else { return };
        self.by_resource.remove(&(copy.native.as_raw() as usize));
        let first = copy.base / PAGE;
        let end = (copy.base + copy.bytes).div_ceil(PAGE).min(PAGES as u64);
        for page in first..end {
            let page = page as usize;
            // A list reading the page now sees copy 0 (zeros and a fault,
            // then the new import), never a view of remapped pages.
            unsafe {
                if self.entries.add(page * 2).read_volatile() == n {
                    self.entries.add(page * 2).write_volatile(0);
                }
            }
        }
        self.retired.push(Retired { _native: copy.native, number: n, at: std::time::Instant::now() });
    }

    /// Retires copies whose import the registry dropped, and frees numbers
    /// past their grace (at most every 250 ms).
    fn sweep(&mut self) {
        if self.last_sweep.is_some_and(|t| t.elapsed().as_millis() < 250) {
            return;
        }
        self.last_sweep = Some(std::time::Instant::now());
        let dead: Vec<u32> = self.copies.iter().enumerate()
            .filter(|(_, c)| c.as_ref().is_some_and(|c| c.import.strong_count() == 0))
            .map(|(i, _)| i as u32 + 1).collect();
        for n in dead {
            self.retire(n);
        }
        let (done, kept): (Vec<_>, Vec<_>) = self.retired.drain(..).partition(|r| r.at.elapsed() >= RETIRE_GRACE);
        self.retired = kept;
        self.free.extend(done.into_iter().map(|r| r.number));
    }

    /// Retires every copy over title `[lo, hi)`.
    fn retire_over(&mut self, lo: u64, hi: u64) {
        let over: Vec<u32> = self.copies.iter().enumerate()
            .filter(|(_, c)| c.as_ref().is_some_and(|c| c.base < hi && lo < c.base + c.bytes))
            .map(|(i, _)| i as u32 + 1).collect();
        for n in over {
            self.retire(n);
        }
    }
}

/// A remap of title `[lo, hi)` (heap.rs retire_imports_over): its copies
/// leave the directory.
pub(crate) fn retire_over(lo: u64, hi: u64) {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.retire_over(lo, hi);
    }
}

/// Makes ready the pages at `address` and the next 4 MiB cell, from title
/// imports (views.rs mem_dir_import). Addresses below 4 GiB are no title
/// memory (rebox 18c049c) and are skipped.
pub(crate) fn ready(owner: &windows::core::IUnknown, device: &ID3D12Device, addresses: &[u64]) {
    if addresses.is_empty() {
        return;
    }
    let _ = with_state(device, |_| ());
    for &address in addresses {
        let address = address & ((1 << 40) - 1);
        if address < 1 << 32 {
            continue;
        }
        let page = (address / PAGE) as usize;
        if page >= PAGES - 1 {
            continue;
        }
        let cell = address & !(CELL - 1);
        for at in [address & !(PAGE - 1), cell + CELL] {
            let page = (at / PAGE) as usize;
            if page >= PAGES - 1 || with_state(device, |s| s.filled(page)) != Some(false) {
                continue;
            }
            // The cell holding the page, else the page alone.
            let cell = at & !(CELL - 1);
            let found = unsafe { super::views::mem_dir_import(owner, cell, CELL) }
                .or_else(|| unsafe { super::views::mem_dir_import(owner, at, PAGE) });
            if let Some((native, import, base)) = found {
                with_state(device, |s| s.add(&native, import, base));
            }
        }
    }
}

/// The addresses a dispatch names (the ref's ready(), memory.rs:255-277):
/// root view addresses; root constant pairs `lo >= 0x10000` with the next
/// constant in 1..=0xff as bits 32-39; constants >= 0x0100_0000 as `<< 8`
/// addresses (BVH bases).
pub(crate) fn named(views: impl IntoIterator<Item = u64>, constants: &[u32]) -> Vec<u64> {
    let mut out: Vec<u64> = views.into_iter().filter(|&va| va != 0).collect();
    for (i, &c) in constants.iter().enumerate() {
        if c >= 0x10000 && let Some(&hi) = constants.get(i + 1) && (1..=0xff).contains(&hi) {
            out.push(u64::from(hi) << 32 | u64::from(c));
        }
        if c >= 0x0100_0000 {
            out.push(u64::from(c) << 8);
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Reads and clears the faults at most every 16 ms, making their pages ready.
pub(crate) fn take_faults(owner: &windows::core::IUnknown, device: &ID3D12Device) {
    let pages = with_state(device, |s| {
        s.sweep();
        if s.last_faults.is_some_and(|t| t.elapsed().as_millis() < 16) {
            return Vec::new();
        }
        s.last_faults = Some(std::time::Instant::now());
        let mut pages = Vec::new();
        for i in 1..FAULTS {
            let word = unsafe { s.fault_words.add(i) };
            let page = unsafe { word.read_volatile() };
            if page != 0 {
                unsafe { word.write_volatile(0) };
                pages.push(u64::from(page) * PAGE);
            }
        }
        pages
    }).unwrap_or_default();
    if !pages.is_empty() {
        static SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seen = SEEN.fetch_add(pages.len() as u64, std::sync::atomic::Ordering::Relaxed);
        if seen == 0 || (seen + pages.len() as u64) >> 12 != seen >> 12 {
            crate::diagnostics::log_line(format_args!(
                "memory directory: {} page faults so far (first now {:#x})", seen + pages.len() as u64, pages[0]));
        }
        ready(owner, device, &pages);
    }
}

/// The dispatch binding: (directory SRV address, fault UAV address, heap
/// index of the copy run in `heap`), writing the run's new views first.
/// A heap with no run (None: no heap, or a paged arena's) gets the empty
/// directory. None: no directory.
pub(crate) fn binding(device: &ID3D12Device, heap: Option<&ID3D12DescriptorHeap>) -> Option<(u64, u64, u32)> {
    let Some((heap, (first, cpu, stride))) = heap.and_then(|h| Some((h, super::root_tables::mem_run(h)?))) else {
        static NO_RUN: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&NO_RUN, || "memory directory: a dispatch's heap has no copy run; its stages read zeros".into());
        return with_state(device, |s| unsafe { (s.empty.GetGPUVirtualAddress(), s.faults.GetGPUVirtualAddress(), 0) });
    };
    with_state(device, |s| {
        let key = heap.as_raw() as usize;
        let raw = |n: u64| D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV { FirstElement: 0, NumElements: n as u32, StructureByteStride: 0,
                    CounterOffsetInBytes: 0, Flags: D3D12_BUFFER_UAV_FLAG_RAW },
            },
        };
        let at = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu + i as usize * stride };
        let run = s.runs.entry(key).or_insert_with(|| {
            unsafe { device.CreateUnorderedAccessView(None, None, Some(&raw(0)), at(0)) };
            Vec::new()
        });
        run.resize(s.copies.len(), usize::MAX);
        // Rewrite each slot whose copy changed (new or reused). A retired
        // copy's slot keeps its view until reuse; the resource lives through
        // the grace, so lists in flight stay valid.
        for (i, copy) in s.copies.iter().enumerate() {
            let Some(copy) = copy else { continue };
            let viewed = copy.native.as_raw() as usize;
            if run[i] == viewed {
                continue;
            }
            // Raw views hold at most 2^32 - 1 dwords; a start is a u32 byte offset.
            let words = (copy.bytes / 4).min(u64::from(u32::MAX) / 4);
            unsafe { device.CreateUnorderedAccessView(&copy.native, None, Some(&raw(words)), at(i as u32 + 1)) };
            run[i] = viewed;
        }
        unsafe { (s.directory.GetGPUVirtualAddress(), s.faults.GetGPUVirtualAddress(), first) }
    })
}

/// Forgets a destroyed heap's run.
pub(crate) fn heap_gone(heap: usize) {
    if let Some(s) = STATE.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        s.runs.remove(&heap);
    }
}
