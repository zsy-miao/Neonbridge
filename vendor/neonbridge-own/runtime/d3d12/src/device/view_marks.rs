//! Write marks: translated shaders mark the 64 KiB pages they write through
//! unordered buffer views, so a writable shadow's write-back copies only
//! those pages (VIctory cd94d35 "shaders mark the pages they write through
//! views; copy-backs compare only those" and 940d32f, GPL-3.0, credited;
//! victory-shader emit "Write marks", victory-gpu descriptor/host.rs
//! `set_mark`/`make_marked`, draw.rs binding, shadow.rs `copy_back_marks`).
//!
//! The binding contract with xshader (its shader half):
//! - `ByteAddressBuffer tb_view_marks : register(t4, space32766)`: two
//!   dwords per slot of the bound shader-visible CBV/SRV/UAV heap, indexed
//!   by the host slot (after the index map): a base bit and a phase
//!   (0..65535). Base [`NONE`]: writes through that slot are not marked.
//! - `RWByteAddressBuffer tb_view_writes : register(u0, space32766)`: a
//!   bitmap of [`BITS`] bits. A store or atomic through the slot's view at
//!   byte `x` of the view sets bit `base + ((phase + x) >> 16)` for the
//!   access's first and last byte, clamped to `BITS - 1`.
//!
//! Each writable shadow owns a run of bits, one per 64 KiB of its copy
//! starting at the copy's first byte; a view of the copy gets the bit of its
//! first byte's page as base and that byte's offset in its page as phase
//! ([`entry`]). Entries follow descriptors as the typed view format codes do
//! (`view_formats`): view creation sets or clears them by host CPU handle,
//! descriptor copies carry them, and each shader-visible heap gets a mapped
//! table the root SRV points at.
//!
//! Ours differs from VIctory's in where the copy-back runs. VIctory waits
//! for the submission and compares marked pages on the CPU; our write-back is
//! GPU copies recorded after the title's lists. So the marks never come back
//! to the CPU: a small dispatch turns the run's bits into one predicate qword
//! per page and each page's copy is predicated on it ([`record_write_back`]),
//! then the run's bits are cleared on the GPU. 940d32f (a marked page's
//! copy-back does not save the page for its own copy) is about VIctory's
//! CPU write guard, which ours has no counterpart of.
//!
//! A shader that writes a shadow without marking (an older translation, a
//! pipeline without the binding) would lose its writes to pages it did not
//! mark, so predication is used only while every pipeline made with
//! unordered access views declares the binding ([`unmarked_pipeline`]);
//! otherwise write-backs copy whole entries as before.
//!
//! TUXBOX_VIEW_MARKS=0 leaves the binding out of every root signature and
//! copies every write-back whole.
// The shadow half (assign, entry, record_write_back) is called from
// views/shadow.rs as its dirty-range rewrite lands.
#![allow(dead_code)]
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::core::Result;

/// The view-mark table: `register(t4, space32766)` (t3 is slot_views).
pub(crate) const TABLE_REGISTER: u32 = tuxbox_dxbc::reserved::VIEW_MARKS.register;
/// The written-pages bitmap: `register(u0, space32766)`.
pub(crate) const BITMAP_REGISTER: u32 = tuxbox_dxbc::reserved::VIEW_WRITES.register;
pub(crate) const SPACE: u32 = tuxbox_dxbc::reserved::VIEW_MARKS.space;
/// Bits in the bitmap; marks past it land on the last bit.
pub(crate) const BITS: u32 = 1 << 22;
/// A table entry's base for a slot whose writes are not marked.
pub(crate) const NONE: u32 = u32::MAX;
/// Bytes a bit stands for.
pub(crate) const PAGE: u64 = 1 << 16;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_VIEW_MARKS") != Some("0")
}

/// The translated stage declares the binding (PSV0: the table as an SRV at
/// t4 or the bitmap as a UAV at u0, space32766).
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            space == SPACE
                && ((matches!(resource, 3..=5) && (lower..=upper).contains(&TABLE_REGISTER))
                    || (matches!(resource, 6..=9) && (lower..=upper).contains(&BITMAP_REGISTER)))
        })
}

/// The stage declares an unordered access view of any kind (PSV0).
pub(crate) fn writes(code: &[u8]) -> bool {
    super::null_kinds::bindings(code).iter().any(|&[resource, ..]| matches!(resource, 6..=9))
}

/// Pipelines made with unordered access views but without the binding:
/// while any was made, write-backs copy whole entries.
static UNMARKED: AtomicU64 = AtomicU64::new(0);

/// A pipeline's stages were seen: `writes` it declares unordered access
/// views, `marks` it declares the binding.
pub(crate) fn note_pipeline(writes: bool, marks: bool) {
    if writes && !marks && enabled() && UNMARKED.fetch_add(1, Ordering::Relaxed) == 0 {
        crate::diagnostics::log_line(format_args!(
            "write marks: a pipeline writes views without marking them; shadow write-backs copy whole entries"
        ));
    }
}

/// Whether a pipeline that writes without marking was made.
pub(crate) fn unmarked_pipeline() -> bool {
    UNMARKED.load(Ordering::Relaxed) != 0
}

/// A writable shadow's bits: the first and how many (one per 64 KiB page of
/// its copy).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Run {
    pub(crate) base: u32,
    pub(crate) pages: u32,
}

/// Runs are handed out from the bitmap's start and freed into a list reused
/// first fit; a bitmap too full for a run gives none (the shadow copies
/// whole).
#[derive(Default)]
struct Runs {
    next: u32,
    free: Vec<Run>,
}

static RUNS: Mutex<Runs> = Mutex::new(Runs { next: 0, free: Vec::new() });

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

/// The run for a writable shadow copy of `bytes`; none when marks are off or
/// the bitmap is full.
pub(crate) fn assign(bytes: u64) -> Option<Run> {
    if !enabled() || bytes == 0 {
        return None;
    }
    let pages = u32::try_from(bytes.div_ceil(PAGE)).ok()?;
    let mut runs = lock(&RUNS);
    if let Some(at) = runs.free.iter().position(|r| r.pages >= pages) {
        let r = runs.free[at];
        if r.pages == pages {
            runs.free.swap_remove(at);
        } else {
            runs.free[at] = Run { base: r.base + pages, pages: r.pages - pages };
        }
        return Some(Run { base: r.base, pages });
    }
    // The last bit takes clamped marks: never part of a run.
    let end = runs.next.checked_add(pages)?;
    if end >= BITS {
        static FULL: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FULL, || "write marks: the bitmap is full; later shadows copy whole".to_owned());
        return None;
    }
    let base = runs.next;
    runs.next = end;
    Some(Run { base, pages })
}

/// A shadow with `run` is gone. Its bits may still be set by work in
/// flight: they are cleared when the run is next written back.
pub(crate) fn release(run: Run) {
    lock(&RUNS).free.push(run);
}

/// The table entry of an unordered buffer view whose first byte is `offset`
/// bytes into the copy that owns `run`.
pub(crate) fn entry(run: Run, offset: u64) -> (u32, u32) {
    let page = u32::try_from(offset / PAGE).unwrap_or(u32::MAX).min(run.pages.saturating_sub(1));
    (run.base + page, (offset % PAGE) as u32)
}

// ---------------------------------------------------------------------------
// Entries by descriptor handle, and the shader-visible tables.

struct Table {
    end: usize,
    stride: usize,
    buffer: ID3D12Resource,
    words: *mut u32,
}

// The mapped pointer stays valid while `buffer` lives; writes go through the lock.
unsafe impl Send for Table {}

fn slot(start: usize, end: usize, stride: usize, handle: usize) -> Option<usize> {
    let offset = handle.checked_sub(start)?;
    (handle < end && offset.is_multiple_of(stride)).then_some(offset / stride)
}

#[derive(Default)]
struct State {
    /// Marked entries by host CPU descriptor handle.
    entries: BTreeMap<usize, (u32, u32)>,
    /// Shader-visible heap tables by CPU start.
    tables: BTreeMap<usize, Table>,
}

impl State {
    fn set(&mut self, handle: usize, entry: Option<(u32, u32)>) {
        match entry {
            Some(e) => self.entries.insert(handle, e),
            None => self.entries.remove(&handle),
        };
        let (base, phase) = entry.unwrap_or((NONE, 0));
        if let Some((&start, table)) = self.tables.range(..=handle).next_back()
            && let Some(slot) = slot(start, table.end, table.stride, handle)
        {
            // In bounds: slot < count, and the buffer holds two dwords per slot.
            unsafe {
                table.words.add(2 * slot).write_volatile(base);
                table.words.add(2 * slot + 1).write_volatile(phase);
            }
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State { entries: BTreeMap::new(), tables: BTreeMap::new() });
/// Any entry was ever set: clears and copies before that skip the lock.
static ANY: AtomicBool = AtomicBool::new(false);

/// A view was created at `handle`: `entry` for an unordered buffer view of a
/// marked shadow, none for anything else.
pub(crate) fn write(handle: usize, entry: Option<(u32, u32)>) {
    if !enabled() || (entry.is_none() && !ANY.load(Ordering::Relaxed)) {
        return;
    }
    if entry.is_some() {
        ANY.store(true, Ordering::Relaxed);
    }
    lock(&STATE).set(handle, entry);
}

/// `count` descriptors were copied from `src` to `dst`.
pub(crate) fn copy(dst: usize, src: usize, count: usize, stride: usize) {
    if !enabled() || !ANY.load(Ordering::Relaxed) || count == 0 || dst == src {
        return;
    }
    let mut state = lock(&STATE);
    let entries: Vec<Option<(u32, u32)>> = (0..count).map(|i| state.entries.get(&(src + i * stride)).copied()).collect();
    for (i, entry) in entries.into_iter().enumerate() {
        state.set(dst + i * stride, entry);
    }
}

/// Forgets the entries and table of a released heap's handle ranges.
pub(crate) fn forget(ranges: &[std::ops::Range<usize>]) {
    if !enabled() || !ANY.load(Ordering::Relaxed) {
        return;
    }
    let mut state = lock(&STATE);
    for range in ranges {
        let keys: Vec<usize> = state.entries.range(range.clone()).map(|(&k, _)| k).collect();
        for key in keys {
            state.entries.remove(&key);
        }
        // Recorded lists hold their own reference to the buffer.
        state.tables.remove(&range.start);
    }
}

fn buffer_desc(bytes: u64, flags: D3D12_RESOURCE_FLAGS) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Flags: flags, ..Default::default()
    }
}

fn committed(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, desc: &D3D12_RESOURCE_DESC, state: D3D12_RESOURCE_STATES) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
    let mut buffer = None::<ID3D12Resource>;
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, desc, state, None, &mut buffer)? };
    buffer.ok_or_else(|| E_POINTER.into())
}

/// The GPU address of the table for the shader-visible heap at `start`,
/// created on first use from the entries already set for its slots.
pub(crate) fn table_address(device: &ID3D12Device, start: usize, count: u32, stride: usize) -> Result<(ID3D12Resource, u64)> {
    let mut state = lock(&STATE);
    let end = start + count.max(1) as usize * stride;
    if let Some(table) = state.tables.get(&start).filter(|t| t.end == end && t.stride == stride) {
        return Ok((table.buffer.clone(), unsafe { table.buffer.GetGPUVirtualAddress() }));
    }
    let slots = count.max(1) as usize;
    let buffer = committed(device, D3D12_HEAP_TYPE_UPLOAD, &buffer_desc(slots as u64 * 8, D3D12_RESOURCE_FLAG_NONE), D3D12_RESOURCE_STATE_GENERIC_READ)?;
    let mut mapped = std::ptr::null_mut();
    unsafe { buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped))? };
    let words = mapped.cast::<u32>();
    // Every slot unmarked (base NONE, phase 0) until a view is made there.
    for i in 0..slots {
        unsafe {
            words.add(2 * i).write(NONE);
            words.add(2 * i + 1).write(0);
        }
    }
    for (&handle, &(base, phase)) in state.entries.range(start..end) {
        if let Some(slot) = slot(start, end, stride, handle) {
            unsafe {
                words.add(2 * slot).write(base);
                words.add(2 * slot + 1).write(phase);
            }
        }
    }
    state.tables.insert(start, Table { end, stride, buffer: buffer.clone(), words });
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "write marks: view-mark table bound for a {count}-slot shader-visible heap (t4/u0 space32766; TUXBOX_VIEW_MARKS=0 leaves them out)"));
    let address = unsafe { buffer.GetGPUVirtualAddress() };
    Ok((buffer, address))
}

// ---------------------------------------------------------------------------
// The bitmap and the predicated write-back.

/// The bitmap, the predicate buffer (a qword per bit) and the pass that
/// fills one from the other and clears the bitmap's run.
struct Marks {
    bitmap: ID3D12Resource,
    predicates: ID3D12Resource,
    root: ID3D12RootSignature,
    expand: ID3D12PipelineState,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Marks {}
// SAFETY: as above.
unsafe impl Sync for Marks {}

/// Per run, its bits as predicates (one qword per page, nonzero: marked),
/// and the run's bits zeroed for the next submission.
const EXPAND: &str = r#"
#define RS "RootConstants(num32BitConstants = 2, b0), UAV(u0), UAV(u1)"
cbuffer Run : register(b0) { uint base; uint pages; };
RWByteAddressBuffer bitmap : register(u0);
RWByteAddressBuffer predicates : register(u1);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    uint page = id.x;
    if (page >= pages) return;
    uint bit = base + page;
    uint word = bitmap.Load((bit >> 5) * 4);
    uint set = (word >> (bit & 31)) & 1;
    predicates.Store2(bit * 8, uint2(set, 0));
    uint before;
    bitmap.InterlockedAnd((bit >> 5) * 4, ~(1u << (bit & 31)), before);
}
"#;

fn compile(source: &str) -> Result<Vec<u8>> {
    use core::ffi::c_void;
    use windows::Win32::Foundation::{E_NOTIMPL, FreeLibrary};
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
    use windows::core::{HRESULT, Interface, s, w};
    type Compile = unsafe extern "system" fn(
        *const c_void, usize, *const u8, *const c_void, *const c_void,
        *const u8, *const u8, u32, u32, *mut *mut c_void, *mut *mut c_void,
    ) -> HRESULT;
    let library = unsafe { LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)? };
    let result = (|| unsafe {
        let entry = GetProcAddress(library, s!("D3DCompile")).ok_or(E_NOTIMPL)?;
        let compile: Compile = core::mem::transmute(entry);
        let mut raw = std::ptr::null_mut();
        compile(
            source.as_ptr().cast(), source.len(), s!("marks").as_ptr(), std::ptr::null(), std::ptr::null(),
            s!("main").as_ptr(), s!("cs_5_1").as_ptr(), 0, 0, &mut raw, std::ptr::null_mut(),
        )
        .ok()?;
        let blob = ID3DBlob::from_raw(raw);
        Ok(std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()).to_vec())
    })();
    let _ = unsafe { FreeLibrary(library) };
    result
}

fn make(device: &ID3D12Device) -> Result<Marks> {
    use core::mem::ManuallyDrop;
    let uav = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    let bitmap = committed(device, D3D12_HEAP_TYPE_DEFAULT, &buffer_desc(u64::from(BITS / 8), uav), D3D12_RESOURCE_STATE_UNORDERED_ACCESS)?;
    let predicates = committed(device, D3D12_HEAP_TYPE_DEFAULT, &buffer_desc(u64::from(BITS) * 8, uav), D3D12_RESOURCE_STATE_COMMON)?;
    let code = compile(EXPAND)?;
    let root: ID3D12RootSignature = unsafe { device.CreateRootSignature(0, &code)? };
    let mut desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
        pRootSignature: ManuallyDrop::new(Some(root.clone())),
        CS: D3D12_SHADER_BYTECODE { pShaderBytecode: code.as_ptr().cast(), BytecodeLength: code.len() },
        ..Default::default()
    };
    let expand = unsafe { device.CreateComputePipelineState(&desc) };
    unsafe { ManuallyDrop::drop(&mut desc.pRootSignature) };
    crate::diagnostics::log_line(format_args!(
        "write marks: {} KiB bitmap, {} MiB of page predicates", BITS / 8 / 1024, u64::from(BITS) * 8 >> 20));
    Ok(Marks { bitmap, predicates, root, expand: expand? })
}

fn marks(device: &ID3D12Device) -> Option<&'static Marks> {
    static MARKS: OnceLock<Option<Marks>> = OnceLock::new();
    MARKS
        .get_or_init(|| match make(device) {
            Ok(m) => Some(m),
            Err(error) => {
                crate::diagnostics::log_line(format_args!("write marks: no bitmap ({error}); write-backs copy whole"));
                None
            }
        })
        .as_ref()
}

/// The bitmap's GPU address for the root UAV, made on first use.
pub(crate) fn bitmap_address(device: &ID3D12Device) -> Option<u64> {
    marks(device).map(|m| unsafe { m.bitmap.GetGPUVirtualAddress() })
}

/// [pages copied by predicate, runs written back by predicate, whole
/// write-backs while an unmarked pipeline exists].
pub(crate) static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
static EXPANDS: AtomicU32 = AtomicU32::new(0);

/// Records into `list` the write-back of a marked shadow copy `copy` (its
/// bits `run`) to title memory: `pieces` as the shadow keeps them, (title
/// buffer, offset in it, offset in the copy, bytes). Each piece is split at
/// the copy's 64 KiB pages and each part's copy is predicated on its page's
/// mark. Returns false when it cannot (no bitmap, marks off, or a pipeline
/// writes without marking): the caller copies whole.
pub(crate) fn record_write_back(
    list: &ID3D12GraphicsCommandList,
    run: Run,
    copy: &ID3D12Resource,
    pieces: &[(ID3D12Resource, u64, u64, u64)],
) -> bool {
    if !enabled() {
        return false;
    }
    if unmarked_pipeline() {
        COUNTS[2].fetch_add(1, Ordering::Relaxed);
        return false;
    }
    let mut device = None::<ID3D12Device>;
    if unsafe { list.GetDevice(&mut device) }.is_err() {
        return false;
    }
    let Some(m) = device.as_ref().and_then(marks) else { return false };
    let barrier = |resource: &ID3D12Resource, before, after| D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: unsafe { std::mem::transmute_copy(resource) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
    };
    let uav_barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: std::mem::ManuallyDrop::new(None) }),
        },
    };
    let predicate = D3D12_RESOURCE_STATE_PREDICATION;
    // Buffers promote to unordered access and decay back to COMMON.
    let unordered = D3D12_RESOURCE_STATE_COMMON;
    unsafe {
        // The title's writes to the bitmap are done; the run's bits become
        // predicates and are cleared.
        list.ResourceBarrier(&[uav_barrier.clone()]);
        list.SetComputeRootSignature(&m.root);
        list.SetPipelineState(&m.expand);
        list.SetComputeRoot32BitConstants(0, 2, [run.base, run.pages].as_ptr().cast(), 0);
        list.SetComputeRootUnorderedAccessView(1, m.bitmap.GetGPUVirtualAddress());
        list.SetComputeRootUnorderedAccessView(2, m.predicates.GetGPUVirtualAddress());
        list.Dispatch(run.pages.div_ceil(64), 1, 1);
        list.ResourceBarrier(&[uav_barrier, barrier(&m.predicates, unordered, predicate)]);
        for (title, offset, at, bytes) in pieces {
            let (mut from, end) = (*at, at + bytes);
            while from < end {
                let page = from / PAGE;
                let to = ((page + 1) * PAGE).min(end);
                let bit = run.base + u32::try_from(page).unwrap_or(u32::MAX).min(run.pages.saturating_sub(1));
                list.SetPredication(&m.predicates, u64::from(bit) * 8, D3D12_PREDICATION_OP_EQUAL_ZERO);
                list.CopyBufferRegion(title, offset + (from - at), copy, from, to - from);
                COUNTS[0].fetch_add(1, Ordering::Relaxed);
                from = to;
            }
        }
        list.SetPredication(None, 0, D3D12_PREDICATION_OP_EQUAL_ZERO);
        list.ResourceBarrier(&[barrier(&m.predicates, predicate, unordered)]);
    }
    COUNTS[1].fetch_add(1, Ordering::Relaxed);
    let n = EXPANDS.fetch_add(1, Ordering::Relaxed);
    if n < 4 || (n + 1).is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "write marks: {} runs written back by page ({} page copies recorded), {} whole while a pipeline writes unmarked",
            COUNTS[1].load(Ordering::Relaxed), COUNTS[0].load(Ordering::Relaxed), COUNTS[2].load(Ordering::Relaxed)));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_entry_names_its_first_page_and_phase() {
        let run = Run { base: 100, pages: 4 };
        assert_eq!(entry(run, 0), (100, 0));
        assert_eq!(entry(run, PAGE + 12), (101, 12));
        assert_eq!(entry(run, 9 * PAGE), (103, 0));
    }
}
