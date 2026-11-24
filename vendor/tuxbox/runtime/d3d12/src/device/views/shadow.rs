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
    /// Hash of the title bytes at the last refresh (TUXBOX_SHADOW_SKIP=1).
    hash: Option<u64>,
    /// Refresh number at which a descriptor last bound this shadow (or its
    /// title bytes last changed), for aging.
    bound: u64,
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

/// FNV-1a over 8-byte words of `bytes` at `address` (title memory, readable
/// while the view lives).
fn hash(address: u64, bytes: u64) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    // SAFETY: the shadow's title range was imported, so it is committed.
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
    if entries.len() > KEPT || over {
        let dead: Vec<_> = entries.iter().filter(|(_, e)| e.view.strong_count() == 0).map(|(k, _)| *k).collect();
        garbage.0.extend(dead.iter().filter_map(|k| entries.remove(k)));
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
    let pieces = match import_source(device, page, width) {
        Ok(source) => vec![(source, address - page, 0, bytes)],
        // The range spans separately mapped title pages that no single heap can
        // open; split it in halves on 64 KiB boundaries until each part opens,
        // so contiguous pages share one import and one copy (page by page
        // gave ~79 copies per shadow on every submission).
        Err(e) if e.code() == windows::Win32::Foundation::E_INVALIDARG => {
            let mut pieces = Vec::new();
            let end = (address + bytes).next_multiple_of(65536);
            split_import(device, address, bytes, page, (page + width / 2).next_multiple_of(65536).min(end), &mut pieces)?;
            split_import(device, address, bytes, (page + width / 2).next_multiple_of(65536).min(end), end, &mut pieces)?;
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
    let replaced = entries.insert((address, bytes, writable), Entry { pieces, writable, view: Arc::downgrade(&view), copy: copy.clone(), written: false, hash: None,
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
            split_import(device, address, bytes, from, middle, pieces)?;
            split_import(device, address, bytes, middle, to, pieces)
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
    if let Some(entry) = lock(&shadows.entries).get_mut(&key) && !entry.written {
        entry.written = true;
        static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 8 {
            crate::diagnostics::log_line(format_args!("shadow at {:#x} ({} bytes) is written; writing it back", key.0, key.1));
        }
    }
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
        for (&key, entry) in entries.iter_mut().filter(|(_, e)| !back || (e.writable && (e.written || all))) {
            let (address, bytes, _) = key;
            // Dead entries wait for revival; their title pages may be unmapped.
            let Some(view) = entry.view.upgrade() else { continue };
            live += 1;
            let aged_check = aging && !entry.writable && !fresh(now, entry.bound);
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
    if copies.is_empty() {
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
