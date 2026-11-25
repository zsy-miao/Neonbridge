//! Buffer views whose records cannot start on a host placement boundary.
//!
//! Resources in a heap opened from title memory must allow cross-adapter
//! sharing, which pins their placement to 64 KiB on desktop drivers, and a
//! structured view can only advance by whole records. A record that starts
//! between those boundaries is read from a device-local copy instead. Each
//! queue submission first copies every live shadow from the title pages, so
//! the view observes CPU writes and earlier GPU work. GPU writes recorded
//! earlier in the same submission are not observed.
//!
//! An unordered access view's copy is written back to the title pages after
//! each submission, which replaces any other write to those bytes in it.
//!
//! Constant buffers use the same copies: Xbox places them on 16-byte
//! boundaries, desktop D3D12 needs 256 and removes the device otherwise.
use super::TextureAddress;
use crate::device::Device;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::core::{Interface, Result};

struct Entry {
    /// (title pages, offset in them, offset in the copy, bytes).
    pieces: Vec<(ID3D12Resource, u64, u64, u64)>,
    writable: bool,
    view: Weak<TextureAddress>,
    /// The device copy, kept after its view dies so a view recreated at the
    /// same address (ring constant buffers, every frame) revives it instead of
    /// importing the title pages and creating a copy again.
    copy: ID3D12Resource,
    /// A UAV or copy has targeted the copy: only then is it written back.
    written: bool,
    /// Write-back number of the last UAV, copy or clear that targeted it.
    marked: u64,
    /// Hash of the title bytes at the last refresh (TUXBOX_SHADOW_SKIP=1).
    hash: Option<u64>,
    /// Refresh number at which a descriptor last bound this shadow (or its
    /// title bytes last changed), for aging.
    bound: u64,
    /// Pieces over title pages no heap opens: (mapped upload buffer, title
    /// address, bytes), filled from title memory by the CPU before the
    /// piece's GPU copy (see cpu_piece).
    cpu: Vec<(usize, u64, u64, ID3D12Resource)>,
    /// Writable copies: its bits in the view-marks bitmap (view_marks.rs),
    /// so a write-back copies only the 64 KiB pages a shader marked. None
    /// copies it whole.
    run: Option<super::super::view_marks::Run>,
    /// Write-back number at which a title root UAV last resolved into the
    /// copy (0 never): root UAVs set no marks, so the copy is written back
    /// whole, unpredicated, while one is recent (see mark_root_uav).
    root_uav: u64,
}

impl Drop for Entry {
    fn drop(&mut self) {
        if let Some(run) = self.run.take() {
            super::super::view_marks::release(run);
        }
    }
}

/// A copy or clear recorded into `resource`: if it is a writable shadow's
/// copy, it is written back, and whole from now on (its run is given up),
/// since copies and clears mark no pages and a list may run after any
/// number of write-backs.
pub(crate) fn mark_copied(device: &Device, resource: *mut core::ffi::c_void) {
    mark_written(device, resource);
    let _ = marks_entry(device, resource, None);
}

/// The view-marks entry for a UAV of `resource` starting `offset` bytes into
/// it, when `resource` is a writable shadow's copy with a run. A UAV whose
/// offset is unknown (None) gives the shadow's run up, so it copies whole:
/// a view that cannot mark must not leave its writes unmarked.
pub(crate) fn marks_entry(device: &Device, resource: *mut core::ffi::c_void, offset: Option<u64>) -> Option<(u32, u32)> {
    let shadows = &device.state.shadows;
    let key = lock(&shadows.by_copy).get(&(resource as usize)).copied()?;
    let mut entries = lock(&shadows.entries);
    let entry = entries.get_mut(&key)?;
    let run = entry.run?;
    match offset {
        Some(offset) => Some(super::super::view_marks::entry(run, offset)),
        None => {
            entry.run = None;
            super::super::view_marks::release(run);
            None
        }
    }
}

/// test108/109 bound over 8192 small address SRVs (32 B to 12 KiB, title RVA
/// 0x4ecc821) as null views: their 64 KiB title pages failed to import with
/// E_INVALIDARG and so did the shadow's import. A read-only view of committed,
/// readable title memory now reads it through an upload buffer the CPU fills
/// before each refresh's copy (rebox 55f1d49's idea, re-derived: the CPU
/// address behind the view, else empty). TUXBOX_SHADOW_CPU=0 binds null again.
fn cpu_pieces_on() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_CPU") != Some("0")
}

/// Whether [address, address + bytes) is committed and readable.
fn readable(address: u64, bytes: u64) -> bool {
    use windows::Win32::System::Memory::*;
    // The CPU piece of a shadow is copied both ways; a range only the GPU
    // wrote is still reserved in a lazy title view, so commit it first.
    crate::device::commit_lazy(address as usize, bytes as usize);
    if crate::device::view_state(address as usize, bytes as usize).committed() {
        return true;
    }
    let end = address + bytes;
    let mut at = address;
    while at < end {
        let mut r = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(at as _), &mut r, size_of_val(&r)) } == 0 || r.State != MEM_COMMIT {
            return false;
        }
        let p = r.Protect.0;
        if p & (PAGE_NOACCESS.0 | PAGE_GUARD.0) != 0 || p == 0 {
            return false;
        }
        at = r.BaseAddress as u64 + r.RegionSize as u64;
    }
    true
}

/// An upload buffer standing in for title pages that will not import, mapped
/// for good, and filled now.
fn cpu_piece(device: &Device, address: u64, bytes: u64) -> Result<(ID3D12Resource, usize)> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let mut staging: Option<ID3D12Resource> = None;
    unsafe {
        device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer(bytes.next_multiple_of(256)),
            D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut staging)?;
    }
    let staging = staging.ok_or(E_POINTER)?;
    let mut mapped = core::ptr::null_mut();
    let nothing_read = D3D12_RANGE { Begin: 0, End: 0 };
    unsafe { staging.Map(0, Some(&nothing_read), Some(&mut mapped))? };
    unsafe { fill(mapped as usize, address, bytes) };
    crate::diagnostics::memory::note(crate::diagnostics::memory::Category::ShadowImport, bytes);
    static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "shadow at {address:#x} ({bytes} bytes): title pages not importable, read through a CPU-filled upload buffer ({n} so far; TUXBOX_SHADOW_CPU=0 binds null)"));
    }
    Ok((staging, mapped as usize))
}

/// Copies title bytes into a CPU piece's upload buffer with ReadProcessMemory,
/// which fails instead of faulting. test133 died here (memcpy at d3d12_xs+0x27c5ab
/// under transfer, read at 0xc63080000): a piece's pages were readable when it
/// was made, then the title decommitted them under the live view; the aged hash
/// saw the change, and the refill read reserved memory. A 64 KiB chunk that
/// fails is retried page by page, and pages that still fail read as zeros, as
/// the title no longer has bytes there. No VirtualQuery on this path (each
/// call walked 13,829 title regions in test132). TUXBOX_SHADOW_FILL_CHECKED=0
/// copies directly as before.
unsafe fn fill(mapped: usize, address: u64, bytes: u64) {
    if crate::env_toggle!("TUXBOX_SHADOW_FILL_CHECKED") == Some("0") {
        unsafe { core::ptr::copy_nonoverlapping(address as *const u8, mapped as *mut u8, bytes as usize) };
        return;
    }
    use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
    let read = |at: u64, dst: usize, n: u64| -> bool {
        let mut copied = 0usize;
        // SAFETY: ReadProcessMemory reports unreadable source pages as an error;
        // the destination is the piece's mapped upload buffer, `bytes` long.
        unsafe { ReadProcessMemory(GetCurrentProcess(), at as *const core::ffi::c_void, dst as *mut core::ffi::c_void, n as usize, Some(&mut copied)) }
            .is_ok()
            && copied as u64 == n
    };
    const CHUNK: u64 = 64 << 10;
    const PAGE: u64 = 4 << 10;
    static ZEROED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (mut at, end) = (address, address.saturating_add(bytes));
    while at < end {
        let n = (end - at).min(CHUNK);
        if !read(at, mapped + (at - address) as usize, n) {
            let mut p = at;
            while p < at + n {
                let q = ((p / PAGE + 1) * PAGE).min(at + n);
                let dst = mapped + (p - address) as usize;
                if !read(p, dst, q - p) {
                    // SAFETY: inside the mapped upload buffer.
                    unsafe { core::ptr::write_bytes(dst as *mut u8, 0, (q - p) as usize) };
                    let z = ZEROED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if z < 4 || (z + 1).is_power_of_two() {
                        crate::diagnostics::log_line(format_args!(
                            "shadow fill: title page {p:#x} of {address:#x}+{bytes:#x} is not readable now (decommitted under a live view); it reads as zeros ({} so far; TUXBOX_SHADOW_FILL_CHECKED=0 copies directly)",
                            z + 1));
                    }
                }
                p = q;
            }
        }
        at += n;
    }
}

/// Refreshes so far (one per queue submission).
static REFRESHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A read-only shadow is copied on every refresh while fresh (bound in the
/// last FRESH refreshes); after that its title bytes are hashed every
/// CHECK_EVERY refreshes and copied only when they changed, which makes it
/// fresh again. test36: 7630 live shadows, 249 MB copied per submission,
/// submit time 2.3 -> 69 ms. TUXBOX_SHADOW_AGING=0 copies every live shadow
/// on every submission as before.
const FRESH: u64 = 8;
const CHECK_EVERY: u64 = 16;

fn aging() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_AGING") != Some("0")
}

/// Whether an aged entry is checked on refresh `now` (staggered by address).
fn aged_check_due(now: u64, address: u64) -> bool {
    (now.wrapping_add(address >> 4)).is_multiple_of(CHECK_EVERY)
}

fn fresh(now: u64, bound: u64) -> bool {
    now.saturating_sub(bound) <= FRESH
}

#[cfg(test)]
#[test]
fn aged_shadows_are_checked_once_every_sixteen_refreshes() {
    assert!(fresh(10, 2) && !fresh(11, 2));
    let due = (0..CHECK_EVERY).filter(|&n| aged_check_due(n, 0x1230)).count();
    assert_eq!(due, 1);
}

/// TUXBOX_SHADOW_SKIP=1: a read-only shadow whose title bytes hash the same
/// as at its last refresh is not copied again. The hash is taken on the CPU
/// at submission, so GPU writes still queued ahead of this submission are not
/// seen; off by default until a run shows no stale constants.
fn skip_unchanged() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_SKIP") == Some("1")
}

/// FNV-1a over the committed, readable parts of `bytes` at `address`.
///
/// test116 crashed here: an access violation reading reserved but
/// uncommitted title memory partway through a 0x19800-byte span (the title
/// decommitted pages under a live view). Each region is checked with
/// VirtualQuery first; a skipped region mixes its length into the hash so
/// commits and decommits still change it, and the first skip is logged.
/// test117a: the walk cost submit 374 ms a frame. VirtualQuery waited in
/// the kernel behind the pipeline workers' driver compiles (hot leaf in
/// ntdll under hash). By default each 64 KiB chunk is now copied with
/// ReadProcessMemory, which fails instead of faulting on an uncommitted
/// page and does not query the address space; only a chunk that fails to
/// copy is walked with VirtualQuery.
/// TUXBOX_SHADOW_HASH_COMMITTED=query walks every span as abbe817 did;
/// =0 reads the whole span directly, as before abbe817.
fn hash(address: u64, bytes: u64) -> u64 {
    let seed = 0xcbf2_9ce4_8422_2325u64;
    match crate::env_toggle!("TUXBOX_SHADOW_HASH_COMMITTED") {
        Some("0") => return hash_span(seed, address, bytes),
        Some("query") => return hash_queried(seed, address, bytes),
        _ => {}
    }
    use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
    const CHUNK: u64 = 64 << 10;
    thread_local! {
        static BUFFER: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(vec![0; CHUNK as usize]);
    }
    BUFFER.with(|buffer| {
        let mut buffer = buffer.borrow_mut();
        let mut h = seed;
        let (mut at, end) = (address, address.saturating_add(bytes));
        while at < end {
            let n = (end - at).min(CHUNK);
            let mut copied = 0usize;
            // SAFETY: ReadProcessMemory reports unreadable pages as an error.
            let ok = unsafe {
                ReadProcessMemory(GetCurrentProcess(), at as *const core::ffi::c_void, buffer.as_mut_ptr().cast(), n as usize, Some(&mut copied))
            }.is_ok() && copied as u64 == n;
            h = if ok {
                // SAFETY: the buffer holds `n` copied bytes.
                hash_span(h, buffer.as_ptr() as u64, n)
            } else {
                hash_queried(h, at, n)
            };
            at += n;
        }
        h
    })
}

/// The committed, readable regions of a span hashed in place (found with
/// VirtualQuery), continuing from `h`.
fn hash_queried(mut h: u64, address: u64, bytes: u64) -> u64 {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    static SKIPS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (mut at, end) = (address, address.saturating_add(bytes));
    while at < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery only reads the address space map.
        let known = unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } != 0;
        let region_end = if known { (region.BaseAddress as u64).saturating_add(region.RegionSize as u64) } else { 0 };
        let next = if region_end > at { region_end.min(end) } else { end };
        let readable = known
            && region.State == MEM_COMMIT
            && (region.Protect.0 & (PAGE_GUARD.0 | PAGE_NOACCESS.0)) == 0
            && region.Protect.0 != 0;
        if readable {
            h = hash_span(h, at, next - at);
        } else {
            let n = SKIPS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n < 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "shadow hash: skipped {:#x} bytes at {at:#x} of {address:#x}+{bytes:#x} (state {:#x}, protect {:#x}), skip {}",
                    next - at, region.State.0, region.Protect.0, n + 1
                ));
            }
            h = (h ^ 0x5eed ^ (next - at)).wrapping_mul(0x100_0000_01b3);
        }
        at = next;
    }
    h
}

/// FNV-1a over 8-byte words of a committed span, continuing from `h`.
fn hash_span(mut h: u64, address: u64, bytes: u64) -> u64 {
    // SAFETY: the caller checked the span is committed and readable (or
    // TUXBOX_SHADOW_HASH_COMMITTED=0 trusts the import).
    let data = unsafe { core::slice::from_raw_parts(address as *const u8, bytes as usize) };
    let mut words = data.chunks_exact(8);
    for w in &mut words {
        h = (h ^ u64::from_le_bytes(w.try_into().unwrap())).wrapping_mul(0x100_0000_01b3);
    }
    for &b in words.remainder() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
    }
    h
}

struct Recorder {
    kind: D3D12_COMMAND_LIST_TYPE,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    value: u64,
    // ExecuteCommandLists does not retain resources until completion.
    held: Vec<(ID3D12Resource, ID3D12Resource)>,
}

type EntryMap = Mutex<HashMap<(u64, u64, bool), Entry>>;
type SourceMap = Mutex<HashMap<(u64, u64), ID3D12Resource>>;

/// Every device's shadow maps, for `release_over`.
static REGISTRY: Mutex<Vec<(std::sync::Weak<EntryMap>, std::sync::Weak<SourceMap>)>> = Mutex::new(Vec::new());

/// test81: xmem could not unmap (error 87, even without keeping a
/// placeholder) pages that GPU-imported heaps still held; cached imports of
/// dead shadows held many of them. Before a page mapping over such a range,
/// dead shadows and cached imports over it are released (live shadows stay).
/// TUXBOX_SHADOW_RELEASE_ON_REMAP=0 keeps them. Returns how many went.
pub(super) fn release_over(lo: u64, hi: u64) -> usize {
    if crate::env_toggle!("TUXBOX_SHADOW_RELEASE_ON_REMAP") == Some("0") {
        return 0;
    }
    let maps: Vec<_> = {
        let mut registry = lock(&REGISTRY);
        registry.retain(|(e, s)| e.strong_count() > 0 && s.strong_count() > 0);
        registry.iter().filter_map(|(e, s)| Some((e.upgrade()?, s.upgrade()?))).collect()
    };
    let mut released = 0;
    for (entries, sources) in maps {
        let mut entries = lock(&entries);
        let dead: Vec<_> = entries.iter()
            .filter(|(k, e)| e.view.strong_count() == 0 && k.0 < hi && lo < k.0 + k.1)
            .map(|(k, _)| *k).collect();
        let gone: Vec<Entry> = dead.iter().filter_map(|k| entries.remove(k)).collect();
        let held: std::collections::HashSet<usize> = entries.values()
            .flat_map(|e| e.pieces.iter().map(|p| p.0.as_raw() as usize)).collect();
        drop(entries);
        let mut sources = lock(&sources);
        let unheld: Vec<_> = sources.iter()
            .filter(|(k, s)| k.0 < hi && lo < k.0 + k.1 && !held.contains(&(s.as_raw() as usize)))
            .map(|(k, _)| *k).collect();
        let imports: Vec<ID3D12Resource> = unheld.iter().filter_map(|k| sources.remove(k)).collect();
        drop(sources);
        released += gone.len() + imports.len();
        drop((gone, imports));
    }
    released
}

#[derive(Default)]
pub(crate) struct Shadows {
    entries: Arc<EntryMap>,
    registered: std::sync::Once,
    /// Writable copies by resource pointer, to mark them written.
    by_copy: Mutex<HashMap<usize, (u64, u64, bool)>>,
    recorders: Mutex<Vec<Recorder>>,
    /// Imported title pages by (page, width), shared by every shadow over them.
    sources: Arc<SourceMap>,
}

impl Shadows {
    fn register(&self) {
        self.registered.call_once(|| lock(&REGISTRY).push((Arc::downgrade(&self.entries), Arc::downgrade(&self.sources))));
    }
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

fn buffer(width: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: width, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    }
}

/// Dead entries (their view dropped) are kept for revival up to this many in all.
const KEPT: usize = 8192;

/// Dead copies kept for revival hold device memory: past this many MiB of
/// them (TUXBOX_SHADOW_DEAD_MB, default 256; checked every 64th prune) they
/// are all dropped, like past KEPT entries. Each check also records live and
/// dead bytes for the memory stats line.
fn dead_cap() -> u64 {
    static CAP: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *CAP.get_or_init(|| crate::env_toggle!("TUXBOX_SHADOW_DEAD_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(256) << 20)
}

pub(crate) static LIVE_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(crate) static DEAD_BYTES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// (live, dead) copy bytes.
fn copy_bytes(entries: &HashMap<(u64, u64, bool), Entry>) -> (u64, u64) {
    entries.values().fold((0, 0), |(live, dead), e| {
        let bytes = e.pieces.iter().map(|p| p.3).sum::<u64>();
        if e.view.strong_count() > 0 { (live + bytes, dead) } else { (live, dead + bytes) }
    })
}

/// Shadows and imports taken out under the shadow lock, released after it.
/// test80 froze with title threads waiting on the lock in copy_of while a
/// thread was inside OpenExistingHeapFromAddress1: releasing an import runs
/// its pin's drop, which calls into xmem and the page-mapping registry, and
/// that release ran under the lock (prune, reuse, replaced entries).
/// TUXBOX_SHADOW_DROP_LOCKED=1 releases them under the lock as before.
#[derive(Default)]
struct Garbage(Vec<Entry>, Vec<ID3D12Resource>);

fn drop_locked() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_DROP_LOCKED") == Some("1")
}

fn prune(entries: &mut HashMap<(u64, u64, bool), Entry>, sources: &Mutex<HashMap<(u64, u64), ID3D12Resource>>) -> Garbage {
    let mut garbage = Garbage::default();
    prune_into(entries, sources, &mut garbage);
    if drop_locked() {
        drop(std::mem::take(&mut garbage));
    }
    garbage
}

fn prune_into(entries: &mut HashMap<(u64, u64, bool), Entry>, sources: &Mutex<HashMap<(u64, u64), ID3D12Resource>>, garbage: &mut Garbage) {
    use std::sync::atomic::Ordering::Relaxed;
    static CALLS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let mut over = false;
    if CALLS.fetch_add(1, Relaxed).is_multiple_of(64) {
        let (live, dead) = copy_bytes(entries);
        LIVE_BYTES.store(live, Relaxed);
        DEAD_BYTES.store(dead, Relaxed);
        over = dead > dead_cap();
    }
    // test85: past KEPT live entries every copy_of scanned all entries and
    // cached imports under the shadow lock, and title threads spun on it
    // (62% of a hot title thread). The scan now runs again only once the map
    // has grown KEPT/8 past its size after the last one.
    // TUXBOX_SHADOW_PRUNE_EVERY=1 scans on every call as before.
    static AFTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let every = crate::env_toggle!("TUXBOX_SHADOW_PRUNE_EVERY") == Some("1");
    let due = entries.len() > KEPT && (every || entries.len() > AFTER.load(Relaxed) + KEPT / 8);
    if due || over {
        let dead: Vec<_> = entries.iter().filter(|(_, e)| e.view.strong_count() == 0).map(|(k, _)| *k).collect();
        garbage.0.extend(dead.iter().filter_map(|k| entries.remove(k)));
        AFTER.store(entries.len(), Relaxed);
        // A cached import pins its pages against remaps; keep only those a
        // remaining shadow still holds, so the cache pins nothing extra.
        let held: std::collections::HashSet<usize> = entries.values()
            .flat_map(|e| e.pieces.iter().map(|p| p.0.as_raw() as usize)).collect();
        let mut sources = lock(sources);
        let unheld: Vec<_> = sources.iter().filter(|(_, s)| !held.contains(&(s.as_raw() as usize))).map(|(k, _)| *k).collect();
        garbage.1.extend(unheld.iter().filter_map(|k| sources.remove(k)));
    }
}

/// Drops every dead shadow (its view gone) and every cached import no live
/// shadow holds, so a creation that ran out of memory can retry. Returns how
/// many entries and imports were released.
pub(super) fn release_dead(shadows: &Shadows) -> usize {
    let mut entries = lock(&shadows.entries);
    let dead: Vec<_> = entries.iter().filter(|(_, e)| e.view.strong_count() == 0).map(|(k, _)| *k).collect();
    let gone: Vec<Entry> = dead.iter().filter_map(|k| entries.remove(k)).collect();
    let held: std::collections::HashSet<usize> = entries.values()
        .flat_map(|e| e.pieces.iter().map(|p| p.0.as_raw() as usize)).collect();
    drop(entries);
    let mut sources = lock(&shadows.sources);
    let unheld: Vec<_> = sources.iter().filter(|(_, s)| !held.contains(&(s.as_raw() as usize))).map(|(k, _)| *k).collect();
    let imports: Vec<ID3D12Resource> = unheld.iter().filter_map(|k| sources.remove(k)).collect();
    drop(sources);
    let released = gone.len() + imports.len();
    drop((gone, imports));
    released
}

/// TUXBOX_SHADOW_IMPORT_CACHE=0: every new shadow imports its title pages
/// again. On: a shadow at a new address over pages an earlier shadow imported
/// (ring constant buffers advancing each frame) reuses that import; the
/// busiest thread spent most of each frame in OpenExistingHeapFromAddress1.
fn import_cache() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_IMPORT_CACHE") != Some("0")
}

/// TUXBOX_SHADOW_COPY_REUSE=0: every new read-only shadow creates its device
/// copy. On: it takes the copy of a dead read-only shadow of the same size.
fn copy_reuse() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_COPY_REUSE") != Some("0")
}

static IMPORTS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static IMPORT_HITS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static COPIES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static COPY_REUSES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_cache() {
    use std::sync::atomic::Ordering::Relaxed;
    let total = IMPORTS.load(Relaxed) + IMPORT_HITS.load(Relaxed) + COPIES.load(Relaxed) + COPY_REUSES.load(Relaxed);
    if total.is_power_of_two() && total >= 64 {
        crate::diagnostics::log_line(format_args!(
            "shadow caches: {} imports, {} reused (TUXBOX_SHADOW_IMPORT_CACHE=0 off); {} copies created, {} reused (TUXBOX_SHADOW_COPY_REUSE=0 off)",
            IMPORTS.load(Relaxed), IMPORT_HITS.load(Relaxed), COPIES.load(Relaxed), COPY_REUSES.load(Relaxed)));
    }
}

/// Title pages [page, page + width) imported as one buffer, cached.
fn import_source(device: &Device, page: u64, width: u64) -> Result<ID3D12Resource> {
    use std::sync::atomic::Ordering::Relaxed;
    let shadows = &device.state.shadows;
    let cache = import_cache();
    if cache && let Some(source) = lock(&shadows.sources).get(&(page, width)) {
        IMPORT_HITS.fetch_add(1, Relaxed);
        return Ok(source.clone());
    }
    let source = crate::device::heap::import_buffer(device, page, &buffer(width), D3D12_RESOURCE_STATE_COMMON)?;
    IMPORTS.fetch_add(1, Relaxed);
    crate::diagnostics::memory::note(crate::diagnostics::memory::Category::ShadowImport, width);
    if cache {
        let mut sources = lock(&shadows.sources);
        sources.insert((page, width), source.clone());
    }
    Ok(source)
}

/// A view of `bytes` at `address` whose resource begins exactly at `address`.
pub(super) fn view(device: &Device, address: u64, bytes: u64) -> Result<Arc<TextureAddress>> {
    copy_of(device, address, bytes, false)
}

/// Like [`view`], for unordered access: the copy is written back.
pub(super) fn writable_view(device: &Device, address: u64, bytes: u64) -> Result<Arc<TextureAddress>> {
    copy_of(device, address, bytes, true)
}

fn copy_of(device: &Device, address: u64, bytes: u64, writable: bool) -> Result<Arc<TextureAddress>> {
    let shadows = &device.state.shadows;
    shadows.register();
    let mut entries = lock(&shadows.entries);
    if let Some(entry) = entries.get_mut(&(address, bytes, writable)) {
        if let Some(view) = entry.view.upgrade() {
            entry.bound = REFRESHES.load(std::sync::atomic::Ordering::Relaxed);
            return Ok(view);
        }
    }
    // TUXBOX_SHADOW_REVIVE=0: a dead entry is rebuilt (fresh import) instead,
    // in case the title remapped its pages to other memory meanwhile.
    let revive = crate::env_toggle!("TUXBOX_SHADOW_REVIVE") != Some("0");
    if let Some(entry) = entries.get_mut(&(address, bytes, writable)) && revive {
        // Not registered by address, like a new copy; refreshed before its
        // next submission, as every live shadow is.
        let view = TextureAddress::unlisted(device, &entry.copy, address as usize);
        entry.view = Arc::downgrade(&view);
        entry.hash = None;
        entry.bound = REFRESHES.load(std::sync::atomic::Ordering::Relaxed);
        return Ok(view);
    }
    let size = bytes.next_multiple_of(256);
    let reused = if !writable && copy_reuse() {
        let dead = entries.iter()
            .find(|(k, e)| !k.2 && !e.written && e.view.strong_count() == 0
                && unsafe { e.copy.GetDesc() }.Width == size)
            .map(|(k, _)| *k);
        dead.and_then(|k| entries.remove(&k))
    } else { None };
    let mut garbage = Garbage::default();
    let reused = reused.map(|e| {
        let copy = e.copy.clone();
        garbage.0.push(e);
        copy
    });
    // The import and the copy's creation take milliseconds; with the lock
    // held every recording thread making a view and every submission's
    // refresh waited on them (rebox b077562, re-derived).
    // TUXBOX_VIEW_LOCKFREE=0 holds the lock throughout as before.
    let lockfree = super::view_lockfree();
    let held = if lockfree { drop(entries); None } else { Some(entries) };
    let page = address & !0xffff;
    let width = (address - page + bytes).next_multiple_of(65536);
    let mut cpu: Vec<(usize, u64, u64, ID3D12Resource)> = Vec::new();
    let pieces = match import_source(device, page, width) {
        Ok(source) => vec![(source, address - page, 0, bytes)],
        // The range spans separately mapped title pages that no single heap can
        // open; split it in halves on 64 KiB boundaries until each part opens,
        // so contiguous pages share one import and one copy (page by page
        // gave ~79 copies per shadow on every submission).
        Err(e) if e.code() == windows::Win32::Foundation::E_INVALIDARG => {
            let mut pieces = Vec::new();
            let end = (address + bytes).next_multiple_of(65536);
            let mut cpu_vec = (!writable && cpu_pieces_on()).then(Vec::new);
            split_import(device, address, bytes, page, (page + width / 2).next_multiple_of(65536).min(end), &mut pieces, cpu_vec.as_mut())?;
            split_import(device, address, bytes, (page + width / 2).next_multiple_of(65536).min(end), end, &mut pieces, cpu_vec.as_mut())?;
            cpu = cpu_vec.unwrap_or_default();
            pieces
        }
        Err(e) => return Err(e),
    };
    let copy = match reused {
        Some(copy) => {
            COPY_REUSES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            copy
        }
        None => {
            COPIES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::diagnostics::memory::note(crate::diagnostics::memory::Category::ShadowCopy, size);
            new_copy(device, size, writable)?
        }
    };
    note_cache();
    let mut entries = match held { Some(entries) => entries, None => lock(&shadows.entries) };
    // Another thread made the same shadow meanwhile: use that one.
    if let Some(entry) = entries.get_mut(&(address, bytes, writable))
        && let Some(view) = entry.view.upgrade()
    {
        entry.bound = REFRESHES.load(std::sync::atomic::Ordering::Relaxed);
        return Ok(view);
    }
    // Not registered by address: CBVs, UAVs and command arguments at this VA
    // must keep reaching the title pages, never the copy.
    let view = TextureAddress::unlisted(device, &copy, address as usize);
    let pruned = prune(&mut entries, &shadows.sources);
    let replaced = entries.insert((address, bytes, writable), Entry { pieces, cpu, writable, view: Arc::downgrade(&view), copy: copy.clone(), written: false, marked: 0, run: if writable { super::super::view_marks::assign(size) } else { None }, root_uav: 0, hash: None,
        bound: REFRESHES.load(std::sync::atomic::Ordering::Relaxed) });
    if writable {
        let mut by_copy = lock(&shadows.by_copy);
        by_copy.retain(|_, key| entries.contains_key(key));
        by_copy.insert(copy.as_raw() as usize, (address, bytes, writable));
    }
    drop(entries);
    drop((pruned, replaced, garbage));
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!("shadowed buffer view at {address:#x}, {bytes} bytes"));
    Ok(view)
}

fn new_copy(device: &Device, size: u64, writable: bool) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let mut copy: Option<ID3D12Resource> = None;
    unsafe {
        // Whole 256-byte units, so a constant buffer view of it is valid.
        device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &D3D12_RESOURCE_DESC {
                Flags: if writable { D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS } else { D3D12_RESOURCE_FLAG_NONE },
                ..buffer(size)
            },
            D3D12_RESOURCE_STATE_COMMON, None, &mut copy)?;
    }
    let copy = copy.ok_or(E_POINTER)?;
    Ok(copy)
}

/// Imports title pages [from, to) (64 KiB aligned) as few buffers as will
/// open, appending the pieces of the shadow `address..address + bytes`.
fn split_import(
    device: &Device,
    address: u64,
    bytes: u64,
    from: u64,
    to: u64,
    pieces: &mut Vec<(ID3D12Resource, u64, u64, u64)>,
    cpu: Option<&mut Vec<(usize, u64, u64, ID3D12Resource)>>,
) -> Result<()> {
    if from >= to {
        return Ok(());
    }
    match import_source(device, from, to - from) {
        Ok(source) => {
            let (start, end) = (address.max(from), (address + bytes).min(to));
            if start < end {
                pieces.push((source, start - from, start - address, end - start));
            }
            Ok(())
        }
        Err(e) if e.code() == windows::Win32::Foundation::E_INVALIDARG && to - from > 65536 => {
            let middle = (from + (to - from) / 2).next_multiple_of(65536);
            let mut cpu = cpu;
            split_import(device, address, bytes, from, middle, pieces, cpu.as_deref_mut())?;
            split_import(device, address, bytes, middle, to, pieces, cpu)
        }
        Err(e) if e.code() == windows::Win32::Foundation::E_INVALIDARG && cpu.is_some() => {
            let Some(cpu) = cpu else { return Err(e) };
            let (start, end) = (address.max(from), (address + bytes).min(to));
            if start >= end {
                return Ok(());
            }
            if !readable(start, end - start) {
                return Err(e);
            }
            let (staging, mapped) = cpu_piece(device, start, end - start)?;
            cpu.push((mapped, start, end - start, staging.clone()));
            pieces.push((staging, 0, start - address, end - start));
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// A UAV, copy or clear may write `resource`: if it is a writable shadow's
/// copy, it is written back from now on. Writable shadows nothing targets
/// (most are placed buffers over split pages that are only read) were copied
/// back every submission: ~640 MB each in test15.
pub(crate) fn mark_written(device: &Device, resource: *mut core::ffi::c_void) {
    let shadows = &device.state.shadows;
    let Some(key) = lock(&shadows.by_copy).get(&(resource as usize)).copied() else { return };
    let mut entries = lock(&shadows.entries);
    let Some(entry) = entries.get_mut(&key) else { return };
    entry.marked = WRITE_BACKS.load(std::sync::atomic::Ordering::Relaxed);
    if !entry.written {
        entry.written = true;
        static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            crate::diagnostics::log_line(format_args!("shadow at {:#x} ({} bytes) is written; writing it back", key.0, key.1));
        }
    }
}

/// TUXBOX_ROOT_UAV_MARK=0 leaves root UAV writes to the predicated write-back.
fn root_uav_mark() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_UAV_MARK") != Some("0"))
}

/// A title root UAV (Set*RootUnorderedAccessView) resolved into `resource`.
/// Its shader writes go through no heap UAV, so they set no view mark and a
/// predicated write-back (view_marks::record_write_back) would skip their
/// pages. When `resource` is a writable shadow's copy, it is flagged written
/// and copied back whole for the next write-backs (the list is submitted
/// within a couple of them) while its run is kept for later marked frames.
/// VIctory cd94d35 (root UAV marking, idea).
pub(crate) fn mark_root_uav(device: &Device, resource: *mut core::ffi::c_void) {
    if !root_uav_mark() {
        return;
    }
    mark_written(device, resource);
    let shadows = &device.state.shadows;
    let Some(key) = lock(&shadows.by_copy).get(&(resource as usize)).copied() else { return };
    let mut entries = lock(&shadows.entries);
    let Some(entry) = entries.get_mut(&key) else { return };
    entry.root_uav = WRITE_BACKS.load(std::sync::atomic::Ordering::Relaxed) + 1;
}

/// Write-backs a root UAV mark copies whole: the one after the recording and
/// one more, for a list submitted after another queue's write-back.
const ROOT_UAV_WINDOW: u64 = 2;

fn root_uav_recent(entry: &Entry) -> bool {
    entry.root_uav != 0 && WRITE_BACKS.load(std::sync::atomic::Ordering::Relaxed) < entry.root_uav + ROOT_UAV_WINDOW
}

/// Write-backs so far: a shadow is copied back while a write targeted it in
/// the last RECENT of them, not for the rest of the run, and refreshed only
/// when its title bytes change once none has (as a read-only one ages). The
/// mark was sticky: test137-148 copied the same 191 MB of 56 shadows back and
/// forth every submission (refresh and write-back), ~380 MB a submit over
/// PCIe and ~100 ms of GPU time a frame against test134's 23. A list is
/// submitted within a few write-backs of its recording, so RECENT covers it.
/// Opt-in (test149: shadows were not the 100 ms): TUXBOX_SHADOW_RECENT=N
/// sets the window; unset or 0 keeps the sticky mark.
static WRITE_BACKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn recent_window() -> Option<u64> {
    static N: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let n = *N.get_or_init(|| crate::env_toggle!("TUXBOX_SHADOW_RECENT").and_then(|v| v.trim().parse().ok()).unwrap_or(0));
    (n != 0).then_some(n)
}

/// Whether a writable shadow was targeted in the last write-backs (always,
/// with the window off).
fn recently_written(entry: &Entry) -> bool {
    entry.written && recent_window().is_none_or(|n| WRITE_BACKS.load(std::sync::atomic::Ordering::Relaxed).saturating_sub(entry.marked) <= n)
}

/// TUXBOX_SHADOW_WRITEBACK=all writes back every writable shadow, written or not.
fn write_back_all() -> bool {
    crate::env_toggle!("TUXBOX_SHADOW_WRITEBACK") == Some("all")
}

/// Copy every live shadow from title memory ahead of this queue's work.
pub(crate) unsafe fn refresh(device: &Device, queue: &ID3D12CommandQueue) -> Result<()> {
    unsafe { transfer(device, queue, false) }
}

/// Copy every writable shadow back to title memory after this queue's work.
pub(crate) unsafe fn write_back(device: &Device, queue: &ID3D12CommandQueue) -> Result<()> {
    unsafe { transfer(device, queue, true) }
}

unsafe fn transfer(device: &Device, queue: &ID3D12CommandQueue, back: bool) -> Result<()> {
    let shadows = &device.state.shadows;
    let kind = unsafe { queue.GetDesc() }.Type;
    if kind != D3D12_COMMAND_LIST_TYPE_DIRECT && kind != D3D12_COMMAND_LIST_TYPE_COMPUTE {
        return Ok(());
    }
    let mut copies = Vec::new();
    // Write-backs by view marks: (run, view, pieces).
    let mut marked: Vec<(super::super::view_marks::Run, Arc<TextureAddress>, Vec<(ID3D12Resource, u64, u64, u64)>)> = Vec::new();
    // CPU fills of upload pieces: (mapped, title address, bytes, the upload
    // buffer, held until filled).
    let mut fills: Vec<(usize, u64, u64, ID3D12Resource)> = Vec::new();
    {
        // test79 froze 15 s with four title threads waiting on this lock: it
        // was held while hashing title memory, whose reads can fault into the
        // title's own handlers. Entries to hash are collected under the lock,
        // hashed without it, and settled under it again.
        // TUXBOX_SHADOW_HASH_LOCKED=1 hashes under the lock as before.
        let unlocked = crate::env_toggle!("TUXBOX_SHADOW_HASH_LOCKED") != Some("1");
        let mut entries = lock(&shadows.entries);
        let pruned = prune(&mut entries, &shadows.sources);
        let skip = !back && skip_unchanged();
        let aging = !back && aging();
        let now = if back { 0 } else { REFRESHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1 };
        let (mut live, mut skipped, mut aged) = (0u64, 0u64, 0u64);
        let all = write_back_all();
        // (key, is an aged check, view) to hash outside the lock.
        let mut checks: Vec<((u64, u64, bool), bool, Arc<TextureAddress>)> = Vec::new();
        if back {
            WRITE_BACKS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        for (&key, entry) in entries.iter_mut().filter(|(_, e)| !back || (e.writable && (all || recently_written(e)))) {
            let (address, bytes, _) = key;
            // Dead entries wait for revival; their title pages may be unmapped.
            let Some(view) = entry.view.upgrade() else { continue };
            live += 1;
            // A writable shadow no write targeted lately ages like a read-only one.
            let idle_writable = entry.writable && recent_window().is_some() && !recently_written(entry);
            let aged_check = aging && ((!entry.writable && !fresh(now, entry.bound)) || idle_writable);
            if aged_check && !aged_check_due(now, address) {
                aged += 1;
                continue;
            }
            if aged_check || (skip && !entry.writable) {
                if unlocked {
                    checks.push((key, aged_check, view));
                    continue;
                }
                let h = hash(address, bytes);
                if entry.hash.replace(h) == Some(h) {
                    if aged_check { aged += 1 } else { skipped += 1 }
                    continue;
                }
                if aged_check {
                    // Changed: copy, and treat it as fresh again.
                    entry.bound = now;
                }
            }
            if !back {
                fills.extend(entry.cpu.iter().map(|(m, a, b, r)| (*m, *a, *b, r.clone())));
            } else if !entry.cpu.is_empty() {
                continue;
            } else if let Some(run) = entry.run && !root_uav_recent(entry) {
                marked.push((run, view.clone(), entry.pieces.clone()));
                continue;
            }
            for (source, offset, at, bytes) in &entry.pieces {
                copies.push((source.clone(), *offset, *at, *bytes, view.clone()));
            }
        }
        if !checks.is_empty() {
            drop(entries);
            // The views are held, so their title ranges stay imported.
            let hashed: Vec<u64> = checks.iter().map(|((address, bytes, _), _, _)| hash(*address, *bytes)).collect();
            entries = lock(&shadows.entries);
            for ((key, aged_check, view), h) in checks.into_iter().zip(hashed) {
                let Some(entry) = entries.get_mut(&key) else { continue };
                if entry.hash.replace(h) == Some(h) {
                    if aged_check { aged += 1 } else { skipped += 1 }
                    continue;
                }
                if aged_check {
                    entry.bound = now;
                }
                fills.extend(entry.cpu.iter().map(|(m, a, b, r)| (*m, *a, *b, r.clone())));
                for (source, offset, at, bytes) in &entry.pieces {
                    copies.push((source.clone(), *offset, *at, *bytes, view.clone()));
                }
            }
        }
        drop(entries);
        drop(pruned);
        static LOGGED: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];
        let n = LOGGED[usize::from(back)].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if n < 4 || (n + 1).is_power_of_two() {
            let total: u64 = copies.iter().map(|c| c.3).sum();
            crate::diagnostics::log_line(format_args!(
                "shadow {} #{}: {live} live, {skipped} unchanged skipped, {aged} aged skipped (TUXBOX_SHADOW_AGING=0 copies them), {} copies, {total} bytes",
                if back { "write-back" } else { "refresh" }, n + 1, copies.len()));
        }
    }
    // Outside the lock: title reads can fault into the title's handlers (test79).
    for (mapped, address, bytes, _staging) in &fills {
        unsafe { fill(*mapped, *address, *bytes) };
    }
    drop(fills);
    if copies.is_empty() && marked.is_empty() {
        return Ok(());
    }
    let mut recorders = lock(&shadows.recorders);
    let ready = recorders.iter().position(|r| r.kind == kind && unsafe { r.fence.GetCompletedValue() } >= r.value);
    let mut recorder = match ready {
        Some(index) => {
            let r = recorders.swap_remove(index);
            let mut r = r;
            r.held.clear();
            unsafe { r.allocator.Reset()?; r.list.Reset(&r.allocator, None)?; }
            r
        }
        None => unsafe {
            let allocator: ID3D12CommandAllocator = device.native.CreateCommandAllocator(kind)?;
            let list: ID3D12GraphicsCommandList = device.native.CreateCommandList(0, kind, &allocator, None)?;
            crate::dred::name(&list, "tuxbox shadow copy");
            let fence: ID3D12Fence = device.native.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            Recorder { kind, allocator, list, fence, value: 0, held: Vec::new() }
        },
    };
    for (source, offset, at, bytes, view) in &copies {
        if back {
            unsafe { recorder.list.CopyBufferRegion(source, *offset, &view.native, *at, *bytes) };
        } else {
            unsafe { recorder.list.CopyBufferRegion(&view.native, *at, source, *offset, *bytes) };
        }
        recorder.held.push((source.clone(), view.native.clone()));
    }
    for (run, view, pieces) in &marked {
        if !super::super::view_marks::record_write_back(&recorder.list, *run, &view.native, pieces) {
            for (source, offset, at, bytes) in pieces {
                unsafe { recorder.list.CopyBufferRegion(source, *offset, &view.native, *at, *bytes) };
            }
        }
        for (source, ..) in pieces {
            recorder.held.push((source.clone(), view.native.clone()));
        }
    }
    unsafe {
        recorder.list.Close()?;
        queue.ExecuteCommandLists(&[Some(recorder.list.cast()?)]);
        crate::dred::check(queue, if back { "shadow write-back" } else { "shadow refresh" });
        recorder.value += 1;
        queue.Signal(&recorder.fence, recorder.value)?;
    }
    recorders.push(recorder);
    Ok(())
}
