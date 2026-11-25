//! Texture contents the title writes itself (decision 0007, "Texture
//! memory"): streamed pages, layout-library copies, movie frames, glyph
//! atlases. Such a texture lives in console memory in its tiled layout;
//! before a submission samples its host copy, the memory is detiled into a
//! staging buffer when its content changed, and the staging buffer is
//! copied into the host texture.
//!
//! "Changed" means a page of the memory was written since the last detiling
//! ([`crate::watch`]), in write-watched memory; in physical-page views,
//! which cannot be watched, a content hash of the whole memory at every
//! submission that samples it.
//!
//! shortcut: a change detiles the whole texture, not the pages written.
//! Only textures read through descriptor tables are brought up to date
//! (directly indexed views are not known per draw); plane 0 is uploaded
//! (every mip and array slice), depth and stencil planes are not.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::*;

use crate::host::{HostList, host};
use crate::object::This;

macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

/// A texture's upload state: the staging buffer laid out as the host
/// texture's subresources, and what was last detiled.
pub(crate) struct Staging {
    buffer: ID3D12Resource,
    /// Its part of the buffer, if that is shared.
    _slot: Option<crate::texpool::Slot>,
    /// Its bytes.
    size: u64,
    /// The [`USES`] count at its last use: the least recently used go
    /// first when staging buffers pass [`budget`].
    used: AtomicU64,
    data: *mut u8,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    layout: victory_formats::texture::Layout,
    /// Mips per array slice, and the title's larger ones left out.
    mips: u32,
    skip: u32,
    memory: *const u8,
    last: Mutex<Last>,
    /// The runtime wrote the memory since it was last looked at.
    rewritten: std::sync::atomic::AtomicBool,
    /// Set once `GetWriteWatch` reports this texture is not in write-watched
    /// memory (physical-page views), skipping the syscall on every frame.
    not_watched: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    gpu_uploads: std::sync::atomic::AtomicU64,
}

/// What a texture's staging buffer holds: the hash of the memory detiled
/// (memory not watched), or the write-watch asking it was detiled after.
#[derive(Default)]
struct Last {
    hash: Option<blake3::Hash>,
    asked: u64,
    /// How often in a row it had not changed, and the frame to look at it
    /// next in.
    still: u32,
    next: u64,
}

// SAFETY: the staging mapping is written only at submission, under
// `last`'s lock; the title memory is only read.
unsafe impl Send for Staging {}
// SAFETY: as above.
unsafe impl Sync for Staging {}

impl Drop for Staging {
    fn drop(&mut self) {
        // Retire the upload buffer and its texpool slot so an in-flight GPU
        // submission or async copy cannot fault on freed staging memory.
        crate::host::retire((self.buffer.clone(), self._slot.take()));
    }
}

/// Textures found changed since they were uploaded: by chance (looked at
/// again when their turn came), and because the runtime wrote them.
pub(crate) static CHANGED: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];

static STAGING: Mutex<Option<HashMap<usize, Arc<Staging>>>> = Mutex::new(None);

/// Bytes of the staging buffers in [`STAGING`]. They live in upload heaps,
/// that is in host memory counted against the commit limit, one the size
/// of each texture ever sampled (all mips and slices): without a bound,
/// loading the world commits gigabytes of them.
static STAGED: AtomicU64 = AtomicU64::new(0);

/// Bytes of the texture staging buffers kept.
pub(crate) fn staged_bytes() -> u64 {
    STAGED.load(Ordering::Relaxed)
}

/// Texture uses so far: the clock of [`Staging::used`].
static USES: AtomicU64 = AtomicU64::new(0);

/// The staging bytes kept before the least recently used buffers no list
/// holds are released (made again, and detiled whole, on their next use):
/// `VICTORY_STAGING_BUDGET_MB`, 384 MiB by default.
fn budget() -> u64 {
    static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("VICTORY_STAGING_BUDGET_MB")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(384)
            << 20
    })
}

/// Releases the least recently used staging buffers no list holds until
/// those kept fit [`budget`] with `more` bytes on top.
fn evict(map: &mut HashMap<usize, Arc<Staging>>, more: u64) {
    let budget = budget();
    if STAGED.load(Ordering::Relaxed) + more <= budget {
        return;
    }
    // Some more than needed: looking at all of them takes a while.
    let budget = budget / 4 * 3;
    // Held only by the map: no list will copy from it, and no submission
    // still running reads it (lists hold theirs until reset).
    let mut idle: Vec<(u64, usize, u64)> = map
        .iter()
        .filter(|(_, s)| Arc::strong_count(s) == 1)
        .map(|(&k, s)| (s.used.load(Ordering::Relaxed), k, s.size))
        .collect();
    idle.sort_unstable();
    let mut freed = 0u64;
    let mut count = 0usize;
    for (_, key, size) in idle {
        if STAGED.load(Ordering::Relaxed) + more <= budget {
            break;
        }
        map.remove(&key);
        STAGED.fetch_sub(size, Ordering::Relaxed);
        freed += size;
        count += 1;
    }
    if count > 0 {
        tracing::debug!(
            count,
            freed_mib = freed >> 20,
            kept_mib = STAGED.load(Ordering::Relaxed) >> 20,
            "texture staging buffers released"
        );
    }
}

thread_local! {
    /// Texture memory read for hashing and detiling: one per submitting
    /// thread, as large as the largest texture it read.
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// Textures the host GPU wrote by a copy or resolve: their contents are on
/// the host, not in title memory, which stays as it was.
static HOST_WRITTEN: Mutex<Option<std::collections::HashSet<usize>>> = Mutex::new(None);

/// A host copy or resolve writes `resource`.
pub(crate) fn host_written(resource: This) {
    let mut set = HOST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    set.get_or_insert_with(Default::default)
        .insert(resource as usize);
}

/// The runtime wrote new contents into the texture `resource`'s memory.
pub(crate) fn rewritten(resource: usize) {
    if let Some(map) = STAGING.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
        && let Some(s) = map.get(&resource)
    {
        s.rewritten.store(true, Ordering::Release);
    }
    let mut set = HOST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = set.as_mut() {
        set.remove(&resource);
    }
}

/// The title released a resource.
pub(crate) fn forget(resource: usize) {
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = map.as_mut()
        && let Some(s) = map.remove(&resource)
    {
        STAGED.fetch_sub(s.size, Ordering::Relaxed);
    }
    let mut set = HOST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = set.as_mut() {
        set.remove(&resource);
    }
}

/// Whether `resource`'s host copy takes its contents from title memory:
/// a texture the host GPU does not render to or write.
pub(crate) fn title_written(texture: &ID3D12Resource) -> bool {
    const TARGET_DEPTH_UNORDERED: i32 = 0x7;
    // SAFETY: a live resource.
    unsafe { texture.GetDesc() }.Flags.0 & TARGET_DEPTH_UNORDERED == 0
}

/// Textures a bindless slot began to view, to upload before anything of the
/// next submission runs: every list reads the slot, not only the one whose
/// recording made it.
static FIRST: Mutex<Vec<(Arc<Staging>, Arc<crate::resource::HostTexture>)>> = Mutex::new(Vec::new());

/// A bindless slot views `resource` from now on.
pub(crate) fn upload_first(resource: This, texture: &ID3D12Resource) {
    let host_written = HOST_WRITTEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(&(resource as usize)));
    if host_written {
        return;
    }
    if let Some(copy) = crate::resource::host_copy(resource)
        && let Some(staging) = staging(resource, texture)
    {
        FIRST.lock().unwrap_or_else(|e| e.into_inner()).push((staging, copy));
    }
}

/// The textures of [`upload_first`] since the last call.
pub(crate) fn take_first() -> Vec<(Arc<Staging>, Arc<crate::resource::HostTexture>)> {
    std::mem::take(&mut *FIRST.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Registers a title-written texture with `list` (once per list) so that at
/// submission time its staging buffer is checked for CPU modifications and,
/// only when its contents changed (including first upload), copied into its
/// host texture on the list's prelude command list.
pub(crate) fn upload(list: &HostList, resource: This, texture: &ID3D12Resource) {
    let host_written = HOST_WRITTEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(&(resource as usize)));
    // shortcut: a texture the host copied into keeps the host's contents
    // even if the title later writes its memory too.
    if host_written || !list.first_use(resource as usize) {
        return;
    }
    let Some(copy) = crate::resource::host_copy(resource) else {
        return;
    };
    let Some(staging) = staging(resource, texture) else {
        return;
    };
    // Once per submission however many of its lists sample the texture;
    // `copy` keeps the host texture alive even if the title releases it
    // before submission, and `CopyTextureRegion` is only recorded onto the
    // prelude when `staging.refresh()` reports dirty contents.
    list.defer_texture(staging, copy);
}

impl Staging {
    /// Detiles the texture memory into the staging buffer if it changed
    /// since the last time (and is mapped at all). Returns `true` when the
    /// staging buffer received new contents that must be copied to the host
    /// texture (including initial upload).
    pub(crate) fn refresh(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let started = std::time::Instant::now();
        let (read, dirty) = self.refresh_now();
        let preparing = &crate::host::PREPARING;
        preparing[2].fetch_add(started.elapsed().as_micros() as u64, Relaxed);
        preparing[3].fetch_add(read as u64, Relaxed);
        dirty
    }

    /// Returns `(bytes_read, dirty)`.
    fn refresh_now(&self) -> (usize, bool) {
        // The larger mips' memory may be another texture's.
        let size = self.layout.reach(self.skip) as usize;
        let start = self.memory as usize;
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let last = &mut *last;
        if !self.not_watched.load(Ordering::Relaxed) {
            if let Some((now, written)) = crate::watch::writes(start, start + size, false) {
                if last.asked != 0 && written.iter().all(|&(_, w)| w <= last.asked) {
                    return (0, false);
                }
                last.asked = now;
                // Read through the copy, which stands unmapped pages: memory of
                // big pools is too slow to ask page by page.
                with_tiled_copy(self.memory, size, |tiled| self.detile(tiled));
                #[cfg(feature = "diagnostics")]
                crate::copystat::add(crate::copystat::C::TexWatched, size as u64);
                return (size, true);
            }
            self.not_watched.store(true, Ordering::Relaxed);
        }
        // Check backoff BEFORE probing `committed` (which touches every page):
        // static textures skip both page probing and BLAKE3 hashing in O(1),
        // while `title_memory_written` still flips `rewritten` immediately.
        let frame = crate::queue::presented() + 1;
        let mode = crate::resource::textures();
        let rewritten = self.rewritten.swap(false, Ordering::AcqRel)
            || mode == crate::resource::Textures::Always;
        if rewritten {
            last.still = 0;
        }
        if !cfg!(test)
            && last.hash.is_some()
            && (frame < last.next && !rewritten || mode == crate::resource::Textures::Keep)
        {
            return (0, false);
        }
        if !committed(self.memory, size) {
            #[cfg(feature = "diagnostics")]
            tracing::debug!(
                memory = ?self.memory,
                size,
                desc = ?self.layout.desc,
                "a sampled texture's memory is not committed"
            );
            return (0, false);
        }
        // Read through our safe copy: the title may unmap the pages
        // (evicting a texture a big table lists) while we read them.
        SCRATCH.with_borrow_mut(|tiled| {
            tiled.resize(size, 0);
            crate::host::copy_mapped(tiled.as_mut_ptr(), self.memory, size, false);
            let read = self.changed(last, tiled, rewritten);
            last.still = if read.1 { 0 } else { (last.still + 1).min(6) };
            // Not all in the same frame; static textures back off up to 32 frames
            // (`title_memory_written` resets `still` immediately on streamed writes).
            last.next = match last.still {
                0 | 1 => 0,
                k => frame + (1 << (k - 1).min(5)) - (self.memory as u64 >> 16) % 2,
            };
            // As in `with_tiled_copy`: a one-off huge read is not kept.
            if tiled.capacity() > 64 << 20 {
                *tiled = Vec::new();
            }
            read
        })
    }

    /// After reading the texture's memory into `tiled`: detiles it unless
    /// its hash is the last one's. Returns the bytes read and whether the
    /// staging buffer got new contents.
    fn changed(&self, last: &mut Last, tiled: &[u8], told: bool) -> (usize, bool) {
        let size = tiled.len();
        let hash = blake3::hash(tiled);
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::TexHashed, size as u64);
        if last.hash == Some(hash) {
            return (size, false);
        }
        #[cfg(feature = "diagnostics")]
        flips::note(self, &hash, tiled, told, last.hash.is_some());
        if last.hash.replace(hash).is_some() {
            // A texture in use changed: how it was found.
            CHANGED[usize::from(told)].fetch_add(1, Ordering::Relaxed);
        }
        self.detile(tiled);
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::TexChanged, size as u64);
        (size, true)
    }

    /// Records `CopyTextureRegion` for every subresource from this staging
    /// buffer into `texture` (which must already be in `COPY_DEST`).
    pub(crate) fn record_copies(&self, list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource) {
        for (i, footprint) in self.footprints.iter().enumerate() {
            let to = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: a borrowed pointer the location never releases.
                pResource: unsafe { std::mem::transmute_copy(texture) },
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    SubresourceIndex: i as u32,
                },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: as above.
                pResource: unsafe { std::mem::transmute_copy(&self.buffer) },
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                    PlacedFootprint: *footprint,
                },
            };
            // SAFETY: an open list; both locations name live resources.
            unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
            #[cfg(test)]
            self.gpu_uploads
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    fn gpu_uploads(&self) -> u64 {
        self.gpu_uploads.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Detiles `tiled` (the texture's memory) into the staging buffer.
    fn detile(&self, tiled: &[u8]) {
        #[cfg(feature = "diagnostics")]
        if tiled.iter().all(|&b| b == 0) {
            tracing::debug!(
                memory = ?self.memory,
                desc = ?self.layout.desc,
                "a sampled texture's memory is all zero (not streamed yet, or a failed BCPack read)"
            );
        }
        for (i, (footprint, &rows)) in self.footprints.iter().zip(&self.rows).enumerate() {
            let (mip, slice) = (i as u32 % self.mips + self.skip, i as u32 / self.mips);
            let f = footprint.Footprint;
            let pitch = f.RowPitch as usize;
            let len = pitch * rows as usize * f.Depth as usize;
            // SAFETY: the subresource's part of the mapped staging buffer.
            let linear = unsafe {
                std::slice::from_raw_parts_mut(self.data.add(footprint.Offset as usize), len)
            };
            if let Err(e) =
                self.layout
                    .detile(tiled, mip, slice, linear, pitch, pitch * rows as usize)
            {
                tracing::warn!(?e, mip, slice, "texture not detiled");
                return;
            }
        }
    }
}

/// Diagnostics: which textures' contents change, how often, and whether they
/// come back to contents they had.
#[cfg(feature = "diagnostics")]
pub(crate) mod flips {
    use super::*;

    #[derive(Default)]
    struct Seen {
        hashes: Vec<[u8; 8]>,
        changes: u32,
        back: u32,
        zero: u32,
        told: u32,
        /// Changes to the part of the memory with the largest mip.
        desc: String,
        first: u64,
        last: u64,
    }
    static SEEN: Mutex<Option<HashMap<(usize, [u32; 4]), Seen>>> = Mutex::new(None);
    /// The views made of placed textures, by address.
    pub(crate) static VIEWS: Mutex<std::collections::BTreeMap<u64, Vec<([u32; 14], [u32; 16])>>> =
        Mutex::new(std::collections::BTreeMap::new());

    /// A view of a placed texture.
    pub(crate) fn view(address: u64, resource: &[u32; 14], desc: [u32; 16]) {
        let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        let list = views.entry(address).or_default();
        if !list.contains(&(*resource, desc)) && list.len() < 16 {
            list.push((*resource, desc));
        }
    }

    pub(super) fn note(s: &Staging, hash: &blake3::Hash, tiled: &[u8], told: bool, again: bool) {
        let frame = crate::queue::presented();
        let d = &s.layout.desc;
        let key = (
            s.memory as usize,
            [s.layout.size as u32, s.mips, s.footprints.len() as u32, s.skip],
        );
        let mut h = [0u8; 8];
        h.copy_from_slice(&hash.as_bytes()[..8]);
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let e = seen.get_or_insert_with(HashMap::new).entry(key).or_default();
        if e.hashes.is_empty() {
            e.desc = format!("{d:?}");
            e.first = frame;
        }
        e.last = frame;
        if again || !e.hashes.is_empty() {
            e.changes += 1;
            e.told += u32::from(told);
        }
        if e.hashes.iter().rev().skip(1).any(|x| *x == h) {
            e.back += 1;
        }
        if tiled.iter().step_by(61).all(|&b| b == 0) {
            e.zero += 1;
        }
        if e.hashes.len() > 64 {
            e.hashes.remove(0);
        }
        e.hashes.push(h);
    }

    /// Logs the textures that changed most.
    pub(crate) fn log() {
        let seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        let Some(seen) = seen.as_ref() else { return };
        let (mut changed, mut back, mut zero) = (0, 0, 0);
        for s in seen.values() {
            changed += s.changes;
            back += s.back;
            zero += s.zero;
        }
        let mut most: Vec<_> = seen.iter().collect();
        most.sort_by_key(|(_, s)| std::cmp::Reverse((s.back, s.changes)));
        tracing::info!(textures = seen.len(), changed, back, zero, "texture contents");
        for ((at, k), s) in most.into_iter().take(12) {
            tracing::info!(
                "  texture {at:#x} {k:?} changed {} (told {}), back {}, zero {}, frames {}-{}: {}",
                s.changes, s.told, s.back, s.zero, s.first, s.last, s.desc
            );
            let views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
            for (a, list) in views.range(*at as u64..*at as u64 + u64::from(k[0])).take(6) {
                for (r, d) in list.iter().take(3) {
                    tracing::info!("    view at {a:#x}: resource {r:x?} srv {d:x?}");
                }
            }
        }
    }
}

/// Reads a texture for the duration of its detile. Keep the initialized
/// length so alternating large and small textures do not re-zero the tail.
fn with_tiled_copy(memory: *const u8, size: usize, detile: impl FnOnce(&[u8])) {
    SCRATCH.with_borrow_mut(|read| {
        if size > read.len() {
            read.reserve_exact(size - read.len());
            read.resize(size, 0);
        }
        let tiled = &mut read[..size];
        crate::host::copy_mapped(tiled.as_mut_ptr(), memory, size, false);
        detile(tiled);
        // Keep the room for the next texture unless it is far bigger than
        // usual (a one-off, like a whole pool): it is commit charge.
        if read.capacity() > 64 << 20 {
            *read = Vec::new();
        }
    });
}

/// Whether `[at, at + len)` is committed memory (streamed textures may be
/// unmapped until their pages arrive).
pub(crate) fn committed(at: *const u8, len: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut p = at as usize;
    let end = p + len;
    if crate::shadow::in_place(p as u64, end as u64) {
        return true;
    }
    if !cfg!(test) {
        // A byte of each 64 KiB read: querying a page of a big region costs
        // as much as its pages are many.
        let mut byte = [0u8; 1];
        while p < end {
            if !crate::host::read_into(p as *const u8, &mut byte) {
                return false;
            }
            p = (p + 1).next_multiple_of(1 << 16);
        }
        return end <= at as usize
            || crate::host::read_into((end - 1) as *const u8, &mut byte);
    }
    while p < end {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        let n = unsafe { VirtualQuery(p as *const _, &mut info, size_of_val(&info)) };
        if n == 0 || info.State != MEM_COMMIT {
            return false;
        }
        p = info.BaseAddress as usize + info.RegionSize;
    }
    true
}

/// The staging state of a title texture, made on first use.
fn staging(resource: This, texture: &ID3D12Resource) -> Option<Arc<Staging>> {
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    let now = USES.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(s) = map.get(&(resource as usize)) {
        s.used.store(now, Ordering::Relaxed);
        return Some(s.clone());
    }
    let (memory, desc) = crate::resource::texture_memory(resource)?;
    let Some(layout) = crate::resource::texture_layout(&desc).map(|t| t.planes[0].layout.clone())
    else {
        once!(
            ?desc,
            "texture contents without a console layout not uploaded"
        );
        return None;
    };
    if layout.desc.swizzle.xor() {
        once!(?desc, "texture contents in an XOR mode not uploaded");
        return None;
    }
    let host = host()?;
    let s = create_staging(host, Some(map), now, resource, texture, layout, memory)?;
    tracing::debug!(?desc, "texture contents queued for staging upload");
    map.insert(resource as usize, s.clone());
    Some(s)
}

/// A staging buffer for `texture`; with `map` (the kept ones), those no
/// list holds are released first to stay within [`budget`].
fn create_staging(
    host: &crate::host::Host,
    map: Option<&mut HashMap<usize, Arc<Staging>>>,
    now: u64,
    resource: This,
    texture: &ID3D12Resource,
    layout: victory_formats::texture::Layout,
    memory: *const u8,
) -> Option<Arc<Staging>> {
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let count = if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
        u32::from(d.MipLevels)
    } else {
        u32::from(d.MipLevels) * u32::from(d.DepthOrArraySize)
    };
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
    let mut rows = vec![0u32; count as usize];
    let mut total = 0;
    // SAFETY: outputs for `count` subresources.
    unsafe {
        host.device.GetCopyableFootprints(
            &d,
            0,
            count,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            None,
            Some(&mut total),
        );
    }
    if let Some(map) = map {
        evict(map, total);
    }
    let (buffer, data, slot) = match crate::texpool::staging(host, total) {
        Some((buffer, data, offset, slot)) => {
            for f in &mut footprints {
                f.Offset += offset;
            }
            (buffer, data, Some(slot))
        }
        None => {
            let (buffer, data) = host.upload_buffer(total)?;
            (buffer, data, None)
        }
    };
    STAGED.fetch_add(total, Ordering::Relaxed);
    crate::host::name(&buffer, || {
        format!(
            "staging of {resource:?} {}x{}x{} f{}",
            d.Width, d.Height, d.DepthOrArraySize, d.Format.0
        )
    });
    Some(Arc::new(Staging {
        buffer,
        _slot: slot,
        size: total,
        used: AtomicU64::new(now),
        data,
        footprints,
        rows,
        layout,
        mips: u32::from(d.MipLevels),
        skip: crate::resource::skipped_mips(resource),
        memory,
        last: Mutex::default(),
        rewritten: Default::default(),
        not_watched: Default::default(),
        #[cfg(test)]
        gpu_uploads: std::sync::atomic::AtomicU64::new(0),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Host;
    use crate::resource::HostTexture;
    use std::sync::atomic::{AtomicU32, Ordering};
    use victory_formats::texture::{Dimension, Swizzle, Texture, TextureDesc};
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};

    const SHADER_RESOURCE: u32 = (D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0
        | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0) as u32;

    fn make_rgba_texture(host: &Host, width: u32, height: u32) -> (Arc<HostTexture>, Texture) {
        let layout_tex = Texture::new(&TextureDesc {
            dimension: Dimension::Tex2d,
            format: DXGI_FORMAT_R8G8B8A8_UNORM.0 as u32,
            width,
            height,
            depth_or_array: 1,
            mips: 1,
            samples: 1,
            swizzle: Swizzle::LINEAR,
        })
        .expect("linear RGBA layout");
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(width),
            Height: height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            ..Default::default()
        };
        let mut resource: Option<ID3D12Resource> = None;
        // SAFETY: plain default-heap 2D texture creation on our test device.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut resource,
                )
                .expect("create host texture");
        }
        let copy = Arc::new(HostTexture {
            slot: None,
            skip: 0,
            written: Default::default(),
            twin: Default::default(),
            texture: resource.unwrap(),
            state: AtomicU32::new(D3D12_RESOURCE_STATE_COMMON.0 as u32),
            bytes: 0,
        });
        (copy, layout_tex)
    }

    /// Reads a 2D RGBA8 texture back from `host` into a tightly packed row buffer.
    fn read_back_rgba(host: &Host, copy: &Arc<HostTexture>, width: u32, height: u32) -> Vec<u8> {
        let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let mut rows = 0u32;
        let mut total = 0u64;
        // SAFETY: querying footprint of a live texture.
        let desc = unsafe { copy.texture.GetDesc() };
        // SAFETY: single-subresource footprint query.
        unsafe {
            host.device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                Some(&mut rows),
                None,
                Some(&mut total),
            );
        }
        let readback_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            ..Default::default()
        };
        let readback_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: total,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut readback: Option<ID3D12Resource> = None;
        // SAFETY: plain readback buffer creation.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &readback_heap,
                    D3D12_HEAP_FLAG_NONE,
                    &readback_desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut readback,
                )
                .expect("create readback buffer");
        }
        let readback = readback.unwrap();
        let list = host.new_list().expect("readback list");
        let before = copy.state.load(Ordering::Relaxed);
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: borrowed COM pointer for the copy call.
            pResource: unsafe { std::mem::transmute_copy(&readback) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: footprint,
            },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: borrowed COM pointer for the copy call.
            pResource: unsafe { std::mem::transmute_copy(&copy.texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        // SAFETY: recording a valid readback copy on an open list.
        unsafe {
            if before != D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32 {
                list.list.ResourceBarrier(&[crate::host::transition(
                    &copy.texture,
                    D3D12_RESOURCE_STATES(before as i32),
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                )]);
            }
            list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
            if before != D3D12_RESOURCE_STATE_COPY_SOURCE.0 as u32 {
                list.list.ResourceBarrier(&[crate::host::transition(
                    &copy.texture,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                    D3D12_RESOURCE_STATES(before as i32),
                )]);
            }
        }
        list.close();
        host.execute(&[&list]);

        let mut ptr = std::ptr::null_mut();
        // SAFETY: GPU finished writing `readback`; map for CPU read.
        unsafe {
            readback.Map(0, None, Some(&mut ptr)).expect("map readback");
            let pitch = footprint.Footprint.RowPitch as usize;
            let row_bytes = width as usize * 4;
            let src = std::slice::from_raw_parts(ptr.cast::<u8>(), total as usize);
            let mut out = Vec::with_capacity(row_bytes * height as usize);
            for y in 0..height as usize {
                let start = footprint.Offset as usize + y * pitch;
                out.extend_from_slice(&src[start..start + row_bytes]);
            }
            readback.Unmap(0, None);
            out
        }
    }

    fn fill_linear_rgba(layout: &victory_formats::texture::Layout, mem: &mut [u8], pixel: [u8; 4]) {
        let mut linear = vec![0u8; layout.desc.width as usize * layout.desc.height as usize * 4];
        for chunk in linear.as_chunks_mut::<4>().0 {
            *chunk = pixel;
        }
        let pitch = layout.desc.width as usize * 4;
        layout
            .tile(mem, 0, 0, &linear, pitch, linear.len())
            .expect("tile pixels");
    }

    fn all_pixels_eq(bytes: &[u8], expected: [u8; 4]) -> bool {
        let (chunks, rem) = bytes.as_chunks::<4>();
        rem.is_empty() && !chunks.is_empty() && chunks.iter().all(|&px| px == expected)
    }

    /// Exercises the real D3D12 WARP prelude upload path end to end:
    /// (a) initial use uploads title memory to GPU,
    /// (b) subsequent frames with unchanged title memory record zero GPU copies,
    /// (c) multi-list submissions after a modification upload only once,
    /// (d) modifying title memory triggers a fresh GPU upload with new pixels.
    #[test]
    fn warp_skips_clean_texture_uploads_and_reuploads_on_change() {
        let Some(host) = Host::new_warp() else {
            return; // WARP software adapter unavailable here
        };
        let (width, height) = (4, 4);
        let (copy, tex) = make_rgba_texture(&host, width, height);
        let layout = tex.planes[0].layout.clone();
        let mut title_mem = vec![0u8; layout.size as usize];
        let v1 = [0x11, 0x22, 0x33, 0xff];
        fill_linear_rgba(&layout, &mut title_mem, v1);

        let staging = create_staging(
            &host,
            None,
            0,
            std::ptr::null(),
            &copy.texture,
            layout.clone(),
            title_mem.as_ptr(),
        )
        .expect("create staging");
        assert_eq!(staging.gpu_uploads(), 0);

        let make_sampling_list = || {
            let list = host.new_list().expect("new list");
            list.defer_texture(staging.clone(), copy.clone());
            list.expect_texture_state(&copy, SHADER_RESOURCE);
            list.close();
            list
        };

        // (a) Submission 1: first use must upload onto the prelude and land on GPU.
        let list1 = make_sampling_list();
        host.execute(&[&list1]);
        assert_eq!(staging.gpu_uploads(), 1, "initial use must record 1 copy");
        assert_eq!(copy.state.load(Ordering::Relaxed), SHADER_RESOURCE);
        assert!(all_pixels_eq(
            &read_back_rgba(&host, &copy, width, height),
            v1
        ));

        // (b) Submission 2: unchanged title memory across two lists records ZERO copies.
        let list2a = make_sampling_list();
        let list2b = make_sampling_list();
        host.execute(&[&list2a, &list2b]);
        assert_eq!(
            staging.gpu_uploads(),
            1,
            "unchanged texture must record zero CopyTextureRegion uploads"
        );
        // Re-submitting the same closed list with unchanged memory also records zero copies.
        host.execute(&[&list2a]);
        assert_eq!(staging.gpu_uploads(), 1);
        assert!(all_pixels_eq(
            &read_back_rgba(&host, &copy, width, height),
            v1
        ));

        // (c) Submission 3: CPU modifies title memory; two lists sample it -> exactly 1 upload.
        let v2 = [0xaa, 0xbb, 0xcc, 0xff];
        fill_linear_rgba(&layout, &mut title_mem, v2);
        let list3a = make_sampling_list();
        let list3b = make_sampling_list();
        host.execute(&[&list3a, &list3b]);
        assert_eq!(
            staging.gpu_uploads(),
            2,
            "modified texture across two lists must upload exactly once"
        );
        assert_eq!(copy.state.load(Ordering::Relaxed), SHADER_RESOURCE);
        assert!(all_pixels_eq(
            &read_back_rgba(&host, &copy, width, height),
            v2
        ));

        // (d) Submission 4: unchanged again -> zero additional uploads.
        let list4 = make_sampling_list();
        host.execute(&[&list4]);
        assert_eq!(staging.gpu_uploads(), 2);
    }

    /// Verifies the write-watched (`MEM_WRITE_WATCH`) dirty-tracking path on
    /// real D3D12 WARP: first use uploads, clean frames skip, and a CPU page
    /// write triggers a single fresh upload.
    #[test]
    fn warp_write_watched_texture_uploads_only_when_written() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        };
        const MEM_WRITE_WATCH: u32 = 0x20_0000;

        let Some(host) = Host::new_warp() else {
            return;
        };
        let (width, height) = (4, 4);
        let (copy, tex) = make_rgba_texture(&host, width, height);
        let layout = tex.planes[0].layout.clone();
        let alloc_size = (layout.size as usize).max(crate::watch::PAGE);
        // SAFETY: allocating a private write-watched page buffer freed before returning.
        let raw = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                alloc_size,
                MEM_RESERVE | MEM_COMMIT | MEM_WRITE_WATCH,
                PAGE_READWRITE,
            )
        } as *mut u8;
        assert!(!raw.is_null());

        let v1 = [0x33, 0x44, 0x55, 0xff];
        // SAFETY: `raw` is committed read-write memory of `alloc_size` bytes.
        unsafe {
            let mem = std::slice::from_raw_parts_mut(raw, layout.size as usize);
            fill_linear_rgba(&layout, mem, v1);
        }

        let staging = create_staging(&host, None, 0, std::ptr::null(), &copy.texture, layout.clone(), raw)
            .expect("create staging");

        let run_once = || {
            let list = host.new_list().expect("new list");
            list.defer_texture(staging.clone(), copy.clone());
            list.expect_texture_state(&copy, SHADER_RESOURCE);
            list.close();
            host.execute(&[&list]);
        };

        // Initial submission uploads.
        run_once();
        assert_eq!(staging.gpu_uploads(), 1);
        assert!(all_pixels_eq(
            &read_back_rgba(&host, &copy, width, height),
            v1
        ));

        // Unchanged write-watched memory skips GPU upload.
        run_once();
        assert_eq!(staging.gpu_uploads(), 1);

        // CPU write dirties the watched page -> next submission uploads.
        let v2 = [0xde, 0xad, 0xbe, 0xff];
        // SAFETY: `raw` is still committed read-write memory.
        unsafe {
            let mem = std::slice::from_raw_parts_mut(raw, layout.size as usize);
            fill_linear_rgba(&layout, mem, v2);
        }
        run_once();
        assert_eq!(staging.gpu_uploads(), 2);
        assert!(all_pixels_eq(
            &read_back_rgba(&host, &copy, width, height),
            v2
        ));

        // Clean again -> zero upload.
        run_once();
        assert_eq!(staging.gpu_uploads(), 2);

        // SAFETY: freeing our own allocation.
        unsafe { VirtualFree(raw.cast(), 0, MEM_RELEASE) };
    }

    #[test]
    fn host_written_texture_never_queues_cpu_upload() {
        let Some(host) = Host::new_warp() else {
            return;
        };
        let (copy, _) = make_rgba_texture(&host, 4, 4);
        let fake_resource = 0x1234_5678usize as This;
        host_written(fake_resource);
        let list = host.new_list().expect("new list");
        upload(&list, fake_resource, &copy.texture);
        assert!(
            !list.has_deferred_work(),
            "host-written textures must not queue CPU-to-GPU uploads"
        );
        forget(fake_resource as usize);
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn watched_texture_reads_reuse_space_and_expose_only_current_bytes() {
        #[cfg(feature = "diagnostics")]
        if crate::copystat::mode() == 2 {
            return; // this mode skips all copies
        }
        let large = [0x67; 8192];
        let small = [0x21; 128];
        let next = [0x43; 8192];
        let mut allocation = None;
        for source in [&large[..], &small[..], &next[..]] {
            with_tiled_copy(source.as_ptr(), source.len(), |tiled| {
                assert_eq!(tiled, source);
                if let Some(first) = allocation {
                    assert_eq!(tiled.as_ptr(), first);
                } else {
                    allocation = Some(tiled.as_ptr());
                }
            });
        }
    }

    #[test]
    fn unmapped_texture_pages_do_not_keep_previous_pixels() {
        use windows_sys::Win32::System::Memory::{
            MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_NOACCESS, PAGE_READWRITE, VirtualAlloc,
            VirtualFree,
        };

        #[cfg(feature = "diagnostics")]
        if crate::copystat::mode() != 0 {
            return; // diagnostic copy overrides do not handle unmapped pages
        }
        const PAGE: usize = 4096;
        let previous = [0x65; 2 * PAGE];
        with_tiled_copy(previous.as_ptr(), previous.len(), |tiled| {
            assert_eq!(tiled, previous);
        });
        // SAFETY: reserve two pages at an address chosen by Windows.
        let base = unsafe { VirtualAlloc(std::ptr::null(), 2 * PAGE, MEM_RESERVE, PAGE_NOACCESS) };
        assert!(!base.is_null());
        // SAFETY: commit the first page of that reservation.
        let mapped = unsafe { VirtualAlloc(base, PAGE, MEM_COMMIT, PAGE_READWRITE) };
        assert_eq!(mapped, base);
        // SAFETY: the first page is writable.
        unsafe { base.cast::<u8>().write_bytes(0x32, PAGE) };
        with_tiled_copy(base.cast(), 2 * PAGE, |tiled| {
            assert!(tiled[..PAGE].iter().all(|&b| b == 0x32));
            assert!(tiled[PAGE..].iter().all(|&b| b == 0));
        });
        // SAFETY: this reservation is no longer used.
        assert_ne!(unsafe { VirtualFree(base, 0, MEM_RELEASE) }, 0);
    }
}
