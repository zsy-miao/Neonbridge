//! Texture contents the title writes itself (decision 0007, "Texture
//! memory"): streamed pages, layout-library copies, movie frames, glyph
//! atlases. Such a texture lives in console memory in its tiled layout;
//! before a submission samples its host copy, the memory is detiled into a
//! staging buffer when its content changed, and the staging buffer is
//! copied into the host texture.
//!
//! "Changed" means a page of the memory was written since the last detiling
//! ([`crate::watch`]), in write-watched memory. For physical-page views,
//! guard notifications and content hashes detect changes.
//!
//! shortcut: a change detiles the whole texture, not the pages written.
//! Only textures read through descriptor tables are brought up to date
//! (directly indexed views are not known per draw); plane 0 is uploaded
//! (every mip and array slice), depth and stencil planes are not.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8_UNORM;

use crate::host::{HostList, host};
use crate::object::This;

macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

/// Query the primary discrete GPU's dedicated video memory in bytes via DXGI.
fn query_adapter_vram_bytes() -> Option<u64> {
    // SAFETY: plain DXGI adapter queries to inspect video memory.
    unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
        let mut max_vram = 0u64;
        let mut i = 0u32;
        while let Ok(adapter) = factory.EnumAdapters1(i) {
            i += 1;
            if let Ok(desc) = adapter.GetDesc1()
                && (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0)
            {
                max_vram = max_vram.max(desc.DedicatedVideoMemory as u64);
            }
        }
        (max_vram > 0).then_some(max_vram)
    }
}

/// The number of most detailed mips to drop for sampled host textures.
/// 0: keep original resolution.
/// 1: drop 1 mip (halves dimensions, ~75% VRAM savings).
/// 2: drop 2 mips (quarters dimensions, ~93.75% VRAM savings).
/// 3: drop 3 mips (eighths dimensions, ~98.4% VRAM savings).
///
/// When `VICTORY_TEXTURE_MIP_DROP` is not explicitly set in the environment,
/// this automatically defaults to 1 on adapters with <= 8 GB dedicated VRAM
/// to prevent host driver committed out-of-memory crashes on 6 GB and 8 GB GPUs.
pub(crate) fn texture_mip_drop() -> u32 {
    static DROP: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *DROP.get_or_init(|| {
        if let Some(v) = std::env::var("VICTORY_TEXTURE_MIP_DROP")
            .ok()
            .and_then(|val| val.parse::<u32>().ok())
        {
            return v.min(3);
        }
        // Auto-detect based on adapter DedicatedVideoMemory.
        // Threshold: 8.5 GiB comfortably covers 4 GB, 6 GB, and 8 GB GPUs
        // (which report up to ~8192 MiB), while leaving >= 10 GB GPUs at full resolution (0).
        if let Some(vram) = query_adapter_vram_bytes() {
            const EIGHT_GB_THRESHOLD: u64 = 8 * 1024 * 1024 * 1024 + 512 * 1024 * 1024;
            if vram <= EIGHT_GB_THRESHOLD {
                tracing::info!(
                    vram_mb = vram / (1024 * 1024),
                    "auto-enabling VICTORY_TEXTURE_MIP_DROP=1 for <= 8 GB GPU"
                );
                return 1;
            }
        }
        0
    })
}

/// The minimum texture dimension (both width and height) eligible for mip-dropping.
/// Defaults to 2048 (the title's sampled 1920x1080 loading artwork turned flat
/// colour below that). Textures smaller than this on either axis are preserved
/// at full resolution. Configurable via `VICTORY_TEXTURE_MIP_DROP_MIN_SIZE`.
pub(crate) fn texture_mip_drop_min_size() -> u64 {
    static MIN_SIZE: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MIN_SIZE.get_or_init(|| {
        std::env::var("VICTORY_TEXTURE_MIP_DROP_MIN_SIZE")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|&v| v > 0)
            .unwrap_or(2048)
    })
}

/// A texture's upload state: the staging buffer laid out as the host
/// texture's subresources, and what was last detiled.
pub(crate) struct Staging {
    buffer: ID3D12Resource,
    /// Its bytes.
    #[cfg(feature = "diagnostics")]
    total: u64,
    data: *mut u8,
    footprints: Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    rows: Vec<u32>,
    layout: victory_formats::texture::Layout,
    /// Mips per array slice.
    mips: u32,
    /// The first host mip that is uploaded (0: all), and the bytes of title
    /// memory read for the console mips it and the smaller ones are.
    first: u32,
    size: usize,
    /// The staging buffer's bytes (counted in `STAGED` while it lives).
    bytes: u64,
    /// Dropped mips count (offset into console mips for detiling).
    pub(crate) mip_drop: u32,
    memory: *const u8,
    /// Whether unchanged view guards can defer hashing this texture.
    guard_view: bool,
    last: Mutex<Last>,
    /// Whether the staging buffer was detiled since a submitted list last
    /// uploaded it, and whether one has uploaded it at all: a list records
    /// the upload only then. (Every list that sampled a title texture
    /// uploaded all of it again, changed or not: 5,411 subresource copies
    /// of 1,120 textures in one frame of run 18.)
    dirty: std::sync::atomic::AtomicBool,
    uploaded: std::sync::atomic::AtomicBool,
    /// When a list last recorded a use of it (`crate::host::moment`).
    used: std::sync::atomic::AtomicU64,
    /// Highest late-submission number (`crate::host::STARTED`) that may
    /// still be reading this staging buffer on the GPU (`0` if none).
    in_flight: std::sync::atomic::AtomicU64,
    /// Levels on demand (`crate::tiles`): the texture's host copy was made
    /// without memory of its own when the staging buffer was made.
    on_demand: bool,
    /// For such a texture: the levels that were the title's at the last
    /// detiling (a bit a level; the staging buffer holds zeros for the
    /// others), and what that was judged by ([`Staging::judgement`]).
    detiled: std::sync::atomic::AtomicU32,
    judged: [std::sync::atomic::AtomicU64; 2],
    /// For such a texture: when an upload last left a held level without
    /// tiles (`crate::host::moment`, never 0; 0: none did). The upload is
    /// recorded again about a second later if the card has room by then
    /// ([`upload`]).
    waiting: std::sync::atomic::AtomicU64,
    /// The size of the buffer if it came from the pool of spare staging
    /// buffers and goes back to it ([`staging_buffer`]); 0: a buffer of its
    /// own, released when this is dropped.
    pooled: u64,
}

/// A staging buffer nothing holds: the buffer and where it is mapped.
struct Spare(ID3D12Resource, usize);

// SAFETY: a D3D12 buffer and its mapping, touched by nobody while it is
// spare.
unsafe impl Send for Spare {}

/// Spare staging buffers by size (a power of two), and their bytes in all.
static SPARE: Mutex<(std::collections::BTreeMap<u64, Vec<Spare>>, u64)> =
    Mutex::new((std::collections::BTreeMap::new(), 0));
/// The most bytes kept spare, and the largest buffer that is pooled.
const SPARE_MOST: (u64, u64) = (256 << 20, 64 << 20);
/// Staging buffers taken from the spare ones, and made, since the start.
pub(crate) static SPARE_STATS: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// A mapped upload buffer of `total` bytes for a texture's staging, all
/// zeros, and its pool size (0: not pooled).
///
/// Every texture the title streams in got a buffer of its own from the
/// host and gave it back when its staging went: about a quarter of a
/// millisecond to make (run 213's profile: 25 ms a frame on the recording
/// threads for some 100 uploads, most of it the graphics kernel waiting)
/// and more to free on the thread that submits. A streaming burst is 200
/// to 500 uploads in a frame (run 222), and those are the frames that
/// dip. Buffers of up to 64 MiB are now kept by size and used again,
/// zeroed as a new one is (levels not there yet must read as zeros).
/// `VICTORY_STAGING_POOL=0`: made and freed each time, as before.
fn staging_buffer(host: &crate::host::Host, total: u64) -> Option<(ID3D12Resource, *mut u8, u64)> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let size = total.max(64 << 10).next_power_of_two();
    if size > SPARE_MOST.1 || !*ON.get_or_init(|| std::env::var_os("VICTORY_STAGING_POOL").is_none_or(|v| v != "0")) {
        return host.upload_buffer(total).map(|(buffer, data)| (buffer, data, 0));
    }
    let spare = {
        let mut all = SPARE.lock().unwrap_or_else(|e| e.into_inner());
        let taken = all.0.get_mut(&size).and_then(Vec::pop);
        if taken.is_some() {
            all.1 -= size;
        }
        taken
    };
    if let Some(Spare(buffer, data)) = spare {
        SPARE_STATS[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // SAFETY: the buffer's own mapping, `size` bytes, which nothing
        // else holds.
        unsafe { std::ptr::write_bytes(data as *mut u8, 0, total as usize) };
        return Some((buffer, data as *mut u8, size));
    }
    SPARE_STATS[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    host.upload_buffer(size).map(|(buffer, data)| (buffer, data, size))
}

/// What a texture's staging buffer holds: the hash of the memory detiled
/// (memory not watched), or the write-watch asking it was detiled after.
#[derive(Default)]
struct Last {
    hash: Option<blake3::Hash>,
    asked: u64,
    /// The frame a guarded texture was last hashed in.
    inspected: Option<u64>,
    /// Whether that was its first hash: it is hashed again the next frame.
    confirm_first: bool,
    /// The last frame its hash changed with no write reported by the guard.
    alias_change: Option<u64>,
    /// The frame a partly mapped texture was last read in.
    partial: Option<u64>,
    /// The frame a refresh without the lock last found a page of the
    /// texture not there ([`no_wait`]): it is not read again that frame.
    left: Option<u64>,
}

impl Last {
    /// Recheck a new guarded view on its next frame of use. A change that
    /// the guard did not report indicates writes through an alias. While
    /// those continue, check this texture on each frame it is sampled.
    fn check_quiet_view(&self, frame: u64, address: usize) -> bool {
        let Some(previous) = self.inspected else { return true };
        if previous == frame { return false; }
        self.confirm_first
            || self.alias_change.is_some_and(|at| frame.saturating_sub(at) <= 30)
            || frame.saturating_sub(previous) >= 8
            || frame.wrapping_add((address >> 16) as u64).is_multiple_of(8)
    }

    fn changed(&mut self, hash: blake3::Hash, guard: Option<(u64, bool)>) -> bool {
        let changed = self.hash != Some(hash);
        if let Some((frame, unwritten)) = guard {
            if self.inspected != Some(frame) {
                self.confirm_first = self.inspected.is_none();
            }
            self.inspected = Some(frame);
            if changed && unwritten && self.hash.is_some() {
                self.alias_change = Some(frame);
            }
        }
        self.hash = Some(hash);
        changed
    }
}

#[cfg(test)]
#[test]
fn aliased_texture_changes_are_detected_and_stable_views_return_to_periodic_checks() {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
    };
    const BYTES: usize = 65536;
    // Two real virtual views share the same physical pages: one sampled, one written.
    unsafe {
        let section = CreateFileMappingW(INVALID_HANDLE_VALUE, std::ptr::null(), PAGE_READWRITE, 0, BYTES as u32, std::ptr::null());
        assert!(!section.is_null());
        let view = MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, BYTES);
        let writer = MapViewOfFile(section, FILE_MAP_ALL_ACCESS, 0, 0, BYTES);
        CloseHandle(section);
        assert!(!view.Value.is_null() && !writer.Value.is_null() && view.Value != writer.Value);
        let address = view.Value as usize;
        let bytes = || std::slice::from_raw_parts(view.Value.cast::<u8>(), BYTES);
        // Here the guard does report such writes (`watch` makes every view
        // read-only); the test passes "unreported" itself, as for a write
        // it misses (the decoder's, seen in the opening video).
        let mut last = Last::default();
        assert!(last.changed(blake3::hash(bytes()), Some((16, false))));
        *writer.Value.cast::<u8>() = 42;
        assert!(last.check_quiet_view(17, address), "a new texture gets a second observation");
        assert!(last.changed(blake3::hash(bytes()), Some((17, true))));
        for frame in 18..45 {
            assert!(last.check_quiet_view(frame, address));
            *writer.Value.cast::<u8>() = frame as u8;
            assert!(last.changed(blake3::hash(bytes()), Some((frame, true))));
            assert!(!last.check_quiet_view(frame, address), "already inspected this frame");
        }
        let mut stable_checks = 0;
        for frame in 45..165 {
            if last.check_quiet_view(frame, address) {
                assert!(!last.changed(blake3::hash(bytes()), Some((frame, true))));
                if frame >= 85 { stable_checks += 1; }
            }
        }
        assert!((9..=11).contains(&stable_checks), "stable texture should average one scan per eight frames, got {stable_checks}");
        UnmapViewOfFile(writer);
        UnmapViewOfFile(view);
    }
}

// SAFETY: the staging mapping is written only at submission, under
// `last`'s lock; the title memory is only read.
unsafe impl Send for Staging {}
// SAFETY: as above.
unsafe impl Sync for Staging {}

static STAGING: Mutex<Option<HashMap<usize, Arc<Staging>>>> = Mutex::new(None);

/// The buffer a texture the host writes is copied through into the texture
/// of a view the title placed in its memory in another format
/// ([`alias`]): every level of every layer as each of the two formats lays
/// it out for a copy (the same bytes: the texels are as wide), the written
/// texture's layout first.
struct Through {
    buffer: ID3D12Resource,
    parts: Vec<(
        D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
        D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    )>,
}

/// The most levels times layers [`alias`] copies (a cube of nine levels).
const ALIAS_PARTS: u32 = 54;

// SAFETY: a device-local buffer only lists record copies with.
unsafe impl Send for Through {}
// SAFETY: as above.
unsafe impl Sync for Through {}

/// The copy buffers of [`alias`], by the placed view's texture.
static THROUGH: Mutex<Option<HashMap<usize, Arc<Through>>>> = Mutex::new(None);

/// Brings `placed`, the texture of a view the title placed in the memory
/// of `source` in another format (`crate::resource::alias_of`), up to date
/// with what the host wrote to `source`: the texels of every level and
/// layer are copied as they are, through a buffer (the host copies between
/// two formats of one texel width in no other way). Recorded the first time
/// the list reads the view
/// and again whenever the list used `source` in another way since (wrote
/// it, or read it itself). Leaves `placed` in the copy destination state,
/// as [`upload`] does. False if the copy cannot be made: the caller fills
/// the texture from title memory, as before.
pub(crate) fn alias(list: &HostList, placed: This, source: This) -> bool {
    const TARGET_OR_UNORDERED: u32 = 0x4 | 0x8;
    const COPY_DEST: u32 = 0x400;
    const COPY_SOURCE: u32 = 0x800;
    let (Some(to), Some(from)) = (
        crate::resource::host_resource(placed),
        crate::resource::host_resource(source),
    ) else {
        return false;
    };
    let Some(host) = host() else { return false };
    let through = {
        let mut map = THROUGH.lock().unwrap_or_else(|e| e.into_inner());
        let map = map.get_or_insert_with(HashMap::new);
        match map.get(&(placed as usize)) {
            Some(through) => through.clone(),
            None => {
                // SAFETY: live textures.
                let (of_from, of_to) = unsafe { (from.GetDesc(), to.GetDesc()) };
                // Levels times layers (a volume's slices are one part).
                let count = |d: &D3D12_RESOURCE_DESC| {
                    let layers = if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
                        1
                    } else {
                        u32::from(d.DepthOrArraySize)
                    };
                    u32::from(d.MipLevels) * layers
                };
                let parts = count(&of_from);
                if parts == 0 || parts > ALIAS_PARTS || parts != count(&of_to) {
                    return false;
                }
                let mut rows = [(); 2]
                    .map(|()| vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); parts as usize]);
                let mut bytes = [0u64; 2];
                // SAFETY: plain queries; the out pointers are ours and
                // hold `parts` layouts each.
                unsafe {
                    host.device.GetCopyableFootprints(
                        &of_from,
                        0,
                        parts,
                        0,
                        Some(rows[0].as_mut_ptr()),
                        None,
                        None,
                        Some(&mut bytes[0]),
                    );
                    host.device.GetCopyableFootprints(
                        &of_to,
                        0,
                        parts,
                        0,
                        Some(rows[1].as_mut_ptr()),
                        None,
                        None,
                        Some(&mut bytes[1]),
                    );
                }
                let alike = rows[0].iter().zip(&rows[1]).all(|(a, b)| {
                    let (fa, fb) = (a.Footprint, b.Footprint);
                    a.Offset == b.Offset
                        && (fa.Width, fa.Height, fa.Depth, fa.RowPitch)
                            == (fb.Width, fb.Height, fb.Depth, fb.RowPitch)
                });
                if bytes[0] == 0 || bytes[0] != bytes[1] || !alike {
                    return false;
                }
                let Some(buffer) = host.uav_buffer(bytes[0]) else {
                    return false;
                };
                crate::host::name(&buffer, || {
                    format!(
                        "alias copy {}x{} f{} to f{}",
                        of_from.Width, of_from.Height, of_from.Format.0, of_to.Format.0
                    )
                });
                tracing::info!(
                    view = ?placed,
                    texture = ?source,
                    width = of_from.Width,
                    height = of_from.Height,
                    parts,
                    written_as = of_from.Format.0,
                    read_as = of_to.Format.0,
                    "a view placed in the memory of a texture the host writes, in another format: read from that texture by a copy on the host"
                );
                let [of_written, of_read] = rows;
                let through = Arc::new(Through {
                    buffer,
                    parts: of_written.into_iter().zip(of_read).collect(),
                });
                map.insert(placed as usize, through.clone());
                through
            }
        }
    };
    let first = list.first_use(placed as usize);
    // The source's first use in the list records nothing (its prelude
    // brings it to the state); a use in another way since the last copy
    // shows as a change of state.
    let before = list.transition(source, COPY_SOURCE).map(|(texture, before)| {
        let state = crate::host::legal_state(texture, COPY_SOURCE);
        let barrier = crate::host::transition(
            texture,
            D3D12_RESOURCE_STATES(before as i32),
            D3D12_RESOURCE_STATES(state as i32),
        );
        // SAFETY: an open list and a live texture.
        unsafe { list.list.ResourceBarrier(&[barrier]) };
        before
    });
    if !first && before.is_none() {
        return true;
    }
    card_writes(list, placed, to, None);
    crate::list::ensure_state(list, placed, COPY_DEST);
    let part = |texture: &ID3D12Resource, index: usize| D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer the location never releases.
        pResource: unsafe { std::mem::transmute_copy(texture) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: index as u32,
        },
    };
    let rows = |row: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT| D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: as above.
        pResource: unsafe { std::mem::transmute_copy(&through.buffer) },
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: *row,
        },
    };
    let (common, read, written) = (
        D3D12_RESOURCE_STATE_COMMON,
        D3D12_RESOURCE_STATE_COPY_SOURCE,
        D3D12_RESOURCE_STATE_COPY_DEST,
    );
    #[cfg(feature = "diagnostics")]
    list.note_op(|| format!("alias copy of {source:?} into {placed:?}"));
    // SAFETY: an open list and live resources; the buffer is as big as
    // the footprints and rests in the common state between copies.
    unsafe {
        list.list
            .ResourceBarrier(&[crate::host::transition(&through.buffer, common, written)]);
        for (index, (of_written, _)) in through.parts.iter().enumerate() {
            list.list
                .CopyTextureRegion(&rows(of_written), 0, 0, 0, &part(from, index), None);
        }
        list.list
            .ResourceBarrier(&[crate::host::transition(&through.buffer, written, read)]);
        for (index, (_, of_read)) in through.parts.iter().enumerate() {
            list.list
                .CopyTextureRegion(&part(to, index), 0, 0, 0, &rows(of_read), None);
        }
        list.list
            .ResourceBarrier(&[crate::host::transition(&through.buffer, read, common)]);
    }
    list.keep(&through.buffer);
    // A draw that also writes the source (a view of it handled before
    // this one) needs it in that state again; the next read of the alias
    // then copies once more, as it must after a write.
    if let Some(before) = before
        && before & TARGET_OR_UNORDERED != 0
    {
        crate::list::ensure_state(list, source, before);
    }
    true
}

/// Whether a texture refresh that finds the runtime remapping title memory
/// goes on without waiting for it (on by default; `VICTORY_TEXTURE_NO_WAIT=0`
/// waits, as before).
///
/// A texture whose memory is all mapped is read in place, under a lock
/// the runtime takes alone while it changes a mapping
/// (`crate::watch::title_memory_reading`), so that no page goes away under
/// the read. While the world streams in the runtime remaps all the time,
/// and each of the frame's 150-360 refreshes waited for it: runs 99-101
/// have 4, 15 and 16 frame lines with over 300 ms of "textures", up to
/// 3,708 ms, and on every one of them the wait for this lock is the whole
/// of it (1,345 of 1,381 ms; 3,630 of 3,708; the write-watch askings 3-12
/// ms, the texture's own lock 0).
///
/// With the switch the refresh asks for the lock without waiting. If the
/// runtime has it, the texture is read through the kernel instead, which
/// needs no lock: it fails if a page is not there, and then the texture is
/// left as it is and read at its next refresh, a frame later. A read
/// through the kernel costs about 0.6 ms a megabyte against 0.3 in place.
///
/// What changes for the picture: a texture that arrives while the runtime
/// is remapping may show a frame later. It never shows partly read.
///
/// On by default since the opening cutscene's stream-in: the runtime maps
/// pages one at a time (`XMemMapPhysicalPagesScatter`), each under the lock,
/// for seconds on end, and the submitting thread waited 2.1-7.0 s a frame
/// for it (99% of the frame line's "textures"; the reads themselves 16-45
/// ms). A read through the kernel only fails on its own pages, so a refresh
/// now waits for nothing but a remapping of the texture itself.
fn no_wait() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_NO_WAIT").is_none_or(|v| v != "0"))
}

/// Since the last frame-rate line, with [`no_wait`]: refreshes that read
/// through the kernel because title memory was being remapped, and those
/// left for the next refresh because a page was not there.
pub(crate) static UNLOCKED: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Since the last frame-rate line, of texture refreshes on every thread, in
/// microseconds: waiting for the texture's own lock (another thread is
/// refreshing the same texture), asking the write watch and the guards,
/// waiting for title memory to hold still (the lock the runtime takes
/// alone while it remaps, `crate::watch::title_memory_reading`), and the
/// refreshes in all.
///
/// While the world streams in, the frame line's "textures" is seconds a
/// frame in some runs and tenths in others for the same megabytes (2,566
/// ms for 110 MB in run 93, 314 ms for 132 MB in run 91; before these
/// counters, runs 81, 83, 85 and 94 each had lines of 1.5-2.9 s), and the
/// submitting thread is not running while it lasts (run 93: 240-310 ms of
/// its own time a frame beside 1,100-1,800 ms of "textures"): it waits.
/// These say for what.
pub(crate) static REFRESH_WAITS: [std::sync::atomic::AtomicU64; 4] = [const { std::sync::atomic::AtomicU64::new(0) }; 4];

/// Of the frame's texture checks, the microseconds a frame they may take
/// before the rest wait for the next frame (`VICTORY_TEXTURE_BUDGET_MS`,
/// 10 by default; 0: no limit). When a burst of new textures arrives (the
/// opening cutscene, 100-200 MB a frame) it is spread over several frames
/// instead of one frame taking all of it; a texture left for later is drawn
/// with what its host copy has (zeros for one never filled: the staging
/// buffer starts zeroed), never with half-detiled bytes.
fn budget_us() -> u64 {
    static US: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    if cfg!(test) {
        return 0;
    }
    *US.get_or_init(|| {
        let ms = std::env::var("VICTORY_TEXTURE_BUDGET_MS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(10.0);
        (ms.max(0.0) * 1000.0) as u64
    })
}

/// The frame the texture checks' time in [`SPENT`] is of, and that time.
static SPENT: Mutex<(u64, u64)> = Mutex::new((0, 0));

/// Since the last frame-rate line: texture checks left for the next frame
/// by [`budget_us`].
pub(crate) static DEFERRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Adds `us` to `spent`, the time of frame `spent.0`'s checks (begun anew
/// when `frame` is another); false (and nothing added) if that time had
/// reached `budget` (0: no limit).
fn within_budget(spent: &mut (u64, u64), frame: u64, us: u64, budget: u64) -> bool {
    if spent.0 != frame {
        *spent = (frame, 0);
    }
    if budget != 0 && spent.1 >= budget {
        return false;
    }
    spent.1 += us;
    true
}

/// `f`'s result, its time added to [`REFRESH_WAITS`]' place `slot`.
fn waited<T>(slot: usize, f: impl FnOnce() -> T) -> T {
    let started = std::time::Instant::now();
    let out = f();
    let us = started.elapsed().as_micros() as u64;
    REFRESH_WAITS[slot].fetch_add(us, std::sync::atomic::Ordering::Relaxed);
    if slot == 2 {
        crate::framelog::add(crate::framelog::T::RefreshStillUs, us);
    }
    out
}

thread_local! {
    /// Room to read a texture's memory into: one buffer per submitting
    /// thread. Each texture kept its own, a second copy in memory of every
    /// write-watched or partly mapped texture ever sampled, for the run.
    static READ: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Calls `f` with `size` bytes of this thread's read buffer (their
/// contents are whatever the last texture left).
fn with_read<R>(size: usize, f: impl FnOnce(&mut [u8]) -> R) -> R {
    READ.with(|read| {
        let mut read = read.borrow_mut();
        if read.len() < size {
            read.resize(size, 0);
        }
        f(&mut read[..size])
    })
}

/// Whether a texture all of whose memory is mapped is read through the
/// kernel's copy instead of in place (`VICTORY_TEXTURE_COPY=1`; off by
/// default). Round 3 of the memory work read every such texture that way
/// to be safe from pages taken away under the read; it cost 0.6 ms per MB
/// against 0.3 in place (90-105 ms a frame while the world streams in, runs
/// 72 and 73, against 20-40), and both runs with it lost the graphics
/// device, for a reason not found. The read in place is safe now for
/// another reason (`crate::watch::title_memory_reading`); this switch is
/// left to tell whether the copy was what lost the device.
fn through_copy() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_COPY").is_some_and(|v| v == "1"))
}

/// Calls `f` with the `size` bytes of title memory at `memory`, read
/// through the kernel into this thread's read buffer; pages that are not
/// there read as zeros. The bytes are those a read in place would see.
fn with_title<R>(memory: *const u8, size: usize, f: impl FnOnce(&[u8]) -> R) -> R {
    with_read(size, |read| {
        crate::host::copy_mapped(read.as_mut_ptr(), memory, size, false);
        f(read)
    })
}

/// Staging buffers alive (also those only a list still holds) and their
/// bytes.
static STAGED: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

impl Drop for Staging {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        STAGED[0].fetch_sub(1, Relaxed);
        STAGED[1].fetch_sub(self.bytes, Relaxed);
        // Nothing holds the staging any more (lists keep it until they are
        // reset, after their submission has run): its buffer is spare.
        if self.pooled != 0 {
            let mut all = SPARE.lock().unwrap_or_else(|e| e.into_inner());
            if all.1 + self.pooled <= SPARE_MOST.0 {
                all.1 += self.pooled;
                all.0.entry(self.pooled).or_default().push(Spare(self.buffer.clone(), self.data as usize));
            }
        }
    }
}

/// For the memory line: staging buffers alive, their bytes, and how many
/// of them their texture still has (the others wait for a list's reset).
#[cfg(feature = "diagnostics")]
pub(crate) fn memory() -> [u64; 3] {
    use std::sync::atomic::Ordering::Relaxed;
    let kept = STAGING
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .map_or(0, |map| map.len() as u64);
    [STAGED[0].load(Relaxed), STAGED[1].load(Relaxed), kept]
}

/// Staging buffers let go as idle since the start, and their bytes.
static LET_GO: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// For the memory line: staging buffers let go as idle, and their bytes.
#[cfg(feature = "diagnostics")]
pub(crate) fn let_go() -> [u64; 2] {
    use std::sync::atomic::Ordering::Relaxed;
    [LET_GO[0].load(Relaxed), LET_GO[1].load(Relaxed)]
}

/// Lets go of the staging buffers no list recorded a use of for `IDLE_MS`
/// and `IDLE_FRAMES` frames and no list holds: a staging buffer is system memory the size of its
/// texture, kept so that an unchanged texture is not uploaded again, and
/// those of textures no longer sampled were kept for the run (2,246 of
/// them, 2.1 GB, at the end of run 24). The next list that samples such a
/// texture makes a staging buffer anew, detiles and uploads once more.
/// Called before a submission starts, by the thread that submits.
pub(crate) fn let_go_idle() {
    use std::sync::atomic::Ordering::Relaxed;
    const IDLE_FRAMES: u64 = 4;
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = map.as_mut() else {
        return;
    };
    if map.is_empty() {
        return;
    }
    let idle_ms = crate::host::ram_idle_ms(crate::host::available_physical_ram());
    // A list that recorded an upload or a check holds the staging until it
    // is reset: one only the table holds is named by no list, recorded or
    // running.
    let (mut went, mut bytes) = (0u64, 0u64);
    map.retain(|_, s| {
        let keep = Arc::strong_count(s) > 1
            || !crate::host::long_ago(s.used.load(Relaxed), idle_ms, IDLE_FRAMES);
        if !keep {
            LET_GO[0].fetch_add(1, Relaxed);
            LET_GO[1].fetch_add(s.bytes, Relaxed);
            went += 1;
            bytes += s.bytes;
        }
        keep
    });
    if went > 0 {
        crate::host::trail(|| format!("sweep: {went} staging buffers of textures let go ({} KiB), held by no list", bytes >> 10));
    }
}

/// Textures the host GPU wrote by a copy or resolve: their contents are on
/// the host, not in title memory, which stays as it was.
static HOST_WRITTEN: Mutex<Option<std::collections::HashSet<usize>>> = Mutex::new(None);

/// A host copy or resolve writes `resource`.
pub(crate) fn host_written(resource: This) {
    let new = {
        let mut set = HOST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
        set.get_or_insert_with(Default::default).insert(resource as usize)
    };
    // Diagnostics (run 177): where its title memory is, for placed views
    // the title reads that memory through ([`placed_in_host_written`]).
    if new && let Some((at, desc)) = crate::resource::texture_memory(resource) {
        // SAFETY: 56 bytes in the description's layout.
        let words = unsafe { std::ptr::from_ref(&desc).cast::<[u32; 14]>().read_unaligned() };
        let bytes = crate::resource::placed_bytes(&words).unwrap_or(0);
        if !at.is_null() && bytes > 0 {
            HOST_WRITTEN_AT
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(at as u64, (bytes, words[4], words[6], words[7], words[8], resource as usize));
        }
    }
}

/// The title memory of textures the host copied into, by address: bytes
/// and description words 4, 6, 7 and 8 (width, height, layers and levels,
/// format).
static HOST_WRITTEN_AT: Mutex<std::collections::BTreeMap<u64, (u64, u32, u32, u32, u32, usize)>> =
    Mutex::new(std::collections::BTreeMap::new());

/// A texture the title placed at `address` (description words `desc`) was
/// made: says so once if it lies in the memory of a texture the host
/// copied into ([`host_written`]). Its own host copy is filled from title
/// memory, which the copy never reached: the copied texels are on the
/// card in the other texture, where no shader that reads this view finds
/// them.
pub(crate) fn placed_in_host_written(address: u64, desc: &[u32; 14]) {
    let hit = {
        let map = HOST_WRITTEN_AT.lock().unwrap_or_else(|e| e.into_inner());
        map.range(..=address)
            .next_back()
            .filter(|(at, d)| address < **at + d.0)
            .map(|(at, d)| (*at, *d))
    };
    if let Some((at, (bytes, width, height, layers_levels, format, _))) = hit {
        static SAID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 200 {
            tracing::warn!(
                view_address = format_args!("{address:#x}"),
                view_offset = address - at,
                view_width = desc[4],
                view_height = desc[6],
                view_layers_levels = format_args!("{:#x}", desc[7]),
                view_format = desc[8],
                copied_address = format_args!("{at:#x}"),
                copied_bytes = bytes,
                copied_width = width,
                copied_height = height,
                copied_layers_levels = format_args!("{layers_levels:#x}"),
                copied_format = format,
                "HERO LEAD: a placed texture view lies in the memory of a texture the host copied into: it is filled from title memory, which the copy never reached"
            );
        }
    }
}

/// Whether the host GPU wrote `resource` by a copy or resolve
/// ([`host_written`]): its host copy cannot be made again from title
/// memory.
pub(crate) fn host_wrote(resource: usize) -> bool {
    HOST_WRITTEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(&resource))
}

/// The title released a resource.
pub(crate) fn forget(resource: usize) {
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = map.as_mut() {
        map.remove(&resource);
    }
    let mut set = HOST_WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(set) = set.as_mut() {
        set.remove(&resource);
    }
    // Its memory may be given to other textures now (run 177: most of the
    // "HERO LEAD" lines were views in memory a released texture had held).
    HOST_WRITTEN_AT.lock().unwrap_or_else(|e| e.into_inner()).retain(|_, d| d.5 != resource);
    let mut through = THROUGH.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(through) = through.as_mut() {
        through.remove(&resource);
    }
}

/// Whether `resource`'s host copy takes its contents from title memory:
/// a texture the host GPU does not render to or write.
pub(crate) fn title_written(texture: &ID3D12Resource) -> bool {
    const TARGET_DEPTH_UNORDERED: i32 = 0x7;
    // SAFETY: a live resource.
    unsafe { texture.GetDesc() }.Flags.0 & TARGET_DEPTH_UNORDERED == 0
}

thread_local! {
    /// While [`whatever_the_room`] runs its uploads.
    static WHATEVER_THE_ROOM: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f`; the uploads it records ([`upload_from`]) give a texture made
/// level by level the card's memory for every level the title holds,
/// whatever room the card has. For what is read once (`draw.rs`
/// `bake_sources`): a level left waiting for room reads zeros, and nothing
/// reads it a second time. And for what the texture pass found room for
/// (`draw.rs` `heap_textures`): the card is not asked a second time.
pub(crate) fn whatever_the_room<R>(f: impl FnOnce() -> R) -> R {
    WHATEVER_THE_ROOM.set(true);
    let out = f();
    WHATEVER_THE_ROOM.set(false);
    out
}

/// Records, once per list, the copy of `resource`'s staging buffer into
/// its host copy, and the detiling that refreshes the staging buffer at
/// submission if the memory changed. Leaves the texture in the copy
/// destination state.
pub(crate) fn upload(list: &HostList, resource: This, texture: &ID3D12Resource) {
    upload_from(list, resource, texture, 0);
}

/// [`upload`] of the console mips from `first` on, the ones a view shows: of
/// a placed texture the title may have no memory for the larger ones, and
/// what lies there is another texture's. (Ported from myokex's victory-amd.)
pub(crate) fn upload_from(list: &HostList, resource: This, texture: &ID3D12Resource, first: u32) {
    let host_written = HOST_WRITTEN
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .is_some_and(|set| set.contains(&(resource as usize)));
    // shortcut: a texture the host copied into keeps the host's contents
    // even if the title later writes its memory too.
    if host_written || !list.first_use(resource as usize) && staged_covers(resource, first) {
        return;
    }
    let Some(staging) = staging(resource, texture, first) else {
        return;
    };
    use std::sync::atomic::Ordering::Relaxed;
    staging.used.store(crate::host::moment(), Relaxed);
    // Levels on demand: a view of the texture now starts at another
    // level, or another texture now starts inside its span, with no byte
    // of its memory changed. The levels are judged anew: detiled again at
    // this list's submission, and uploaded by this list.
    if staging.on_demand && staging.judgement() != staging.judged() {
        staging.forget();
        staging.dirty.store(true, Relaxed);
    }
    // Levels on demand: the last upload left a held level without tiles
    // (the card had no room, or no heap could be had), and nothing else
    // would record an upload again until the title writes the texture: the
    // level stayed empty, the texture blurred or black. Asked again about
    // once a second, and only when the card has room; the staging buffer
    // still holds the level's bytes. (At once and without room for what is
    // read once: `whatever_the_room`.)
    let whatever = WHATEVER_THE_ROOM.get();
    if staging.on_demand {
        let since = staging.waiting.load(Relaxed);
        if since != 0 && (whatever || crate::host::long_ago(since, 1000, 2)) {
            if whatever || crate::resource::room_lately() {
                staging.waiting.store(0, Relaxed);
                staging.dirty.store(true, Relaxed);
                crate::tiles::STATS[10].fetch_add(1, Relaxed);
            } else {
                staging.waiting.store(crate::host::moment().max(1), Relaxed);
            }
        }
    }
    // The host texture has the staging buffer's contents: only the check
    // at submission is left, and what it finds changed the list's prelude
    // copies in, just before the list runs. (The next list that sampled
    // the texture uploaded it instead: a frame late, and with the boot
    // video's ring of frame textures an older frame shown after a newer
    // one, each texture of the ring a turn behind.)
    if staging.uploaded.load(Relaxed) && !staging.dirty.load(Relaxed) {
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::TexSkipped, 1);
        if let Some(copy) = crate::resource::host_copy(resource) {
            list.defer_upload(staging.clone(), copy);
        }
        // The list records no copy; a prelude upload marks its own read.
        list.defer_texture(staging, false);
        return;
    }
    #[cfg(feature = "diagnostics")]
    crate::copystat::add(crate::copystat::C::TexUploads, 1);
    crate::framelog::add(crate::framelog::T::Uploads, 1);
    const COPY_DEST: u32 = 0x400;
    crate::list::ensure_state(list, resource, COPY_DEST);
    // The staging buffer lives as long as `staging`, which the deferred
    // refresh below holds until the list is reset.
    for (i, footprint) in staging.footprints.iter().enumerate() {
        #[cfg(feature = "diagnostics")]
        {
            let f = footprint.Footprint;
            // SAFETY: live resources.
            let (gpu, d) = unsafe { (staging.buffer.GetGPUVirtualAddress(), texture.GetDesc()) };
            let len = u64::from(f.RowPitch) * u64::from(staging.rows[i]) * u64::from(f.Depth);
            list.note_read(
                || {
                    format!(
                        "texture upload {resource:?} ({}x{}x{} f{} mips {}) sub {i} of {} from staging {gpu:#x}+{:#x}: offset {:#x} {}x{}x{} pitch {}",
                        d.Width,
                        d.Height,
                        d.DepthOrArraySize,
                        d.Format.0,
                        d.MipLevels,
                        staging.footprints.len(),
                        staging.total,
                        footprint.Offset,
                        f.Width,
                        f.Height,
                        f.Depth,
                        f.RowPitch
                    )
                },
                (gpu + footprint.Offset, len),
            );
        }
        #[cfg(not(feature = "diagnostics"))]
        let _ = footprint;
        staging.record_copy_of(&list.list, texture, i);
    }
    // When the list is submitted, its copy takes what the staging buffer
    // holds by then: noted before the check below, which sets `dirty`
    // again if it detiles (the next list then uploads once more).
    {
        let staging = staging.clone();
        list.defer_host(move || {
            staging.uploaded.store(true, Relaxed);
            staging.dirty.store(false, Relaxed);
        });
    }
    // Levels on demand (`crate::tiles`): after the check below, in the
    // order of this list's steps, and before the list is executed, the
    // levels the staging buffer holds as the title's are given the card's
    // memory if they have none. The list copies every level, so each one
    // given here is written whole by it (the rest are copies into no
    // memory, which the card drops).
    let on_demand = if staging.on_demand { crate::resource::host_tiled(resource) } else { None };
    let Some(on_demand) = on_demand else {
        // Once per submission however many of its lists sample the texture.
        list.defer_texture(staging, true);
        return;
    };
    list.defer_texture(staging.clone(), true);
    list.defer_host(move || {
        let waiting = on_demand.uploaded(staging.detiled.load(Relaxed), staging.memory as u64, whatever);
        staging.waiting.store(if waiting { crate::host::moment().max(1) } else { 0 }, Relaxed);
        // A texture picked by index has no staging buffer after this
        // upload: its slots are made due again by the texture pass.
        if waiting {
            crate::descriptor::host::levels_waiting(staging.memory as u64);
        }
    });
}

/// Levels on demand: the levels a list claimed for its zeros
/// (`crate::tiles::Tiled::claim`), held by the list's submission-time
/// step. A list that is reset without having been submitted gives the
/// claim up, or the next list that writes those levels would send no zeros
/// and what its write does not cover would be what the tiles' last owner
/// left.
struct Claimed {
    on_demand: crate::resource::OnDemand,
    fresh: u32,
    ran: std::sync::atomic::AtomicBool,
}

impl Drop for Claimed {
    fn drop(&mut self) {
        if self.fresh != 0
            && !*self.ran.get_mut()
            && let Some(tiled) = self.on_demand.tiled()
        {
            tiled.unclaim(self.fresh);
        }
    }
}

/// Levels on demand (`crate::tiles`): the card itself is about to write
/// title texture `resource` in `list` (a copy or a resolve into level
/// `level`, or into every level). If its host copy `texture` was made
/// without memory, the levels written get theirs when the list is
/// submitted, ahead of it on the queue; the ones that have none yet are
/// first written with zeros, here in the same list, so that what the
/// card's write does not cover is not what the tiles' last owner left.
/// To be called before the list is brought to the state of that write.
/// shortcut: two lists that write the same new level, submitted in the
/// other order than recorded, lose the first one's write under the zeros
/// of the other; so does an upload recorded and not yet submitted.
pub(crate) fn card_writes(list: &HostList, resource: This, texture: &ID3D12Resource, level: Option<u32>) {
    let Some(on_demand) = crate::resource::host_tiled(resource) else {
        return;
    };
    let Some(tiled) = on_demand.tiled() else {
        return;
    };
    let levels = match level {
        Some(level) => tiled.with_level(level),
        None => tiled.every_level(),
    };
    if levels == 0 {
        return;
    }
    let fresh = tiled.claim(levels);
    // Of them, the ones whose zeros could not be recorded.
    let mut unzeroed = if host().is_none() { fresh } else { 0 };
    if fresh != 0
        && let Some(host) = host()
    {
        const COPY_DEST: u32 = 0x400;
        crate::list::ensure_state(list, resource, COPY_DEST);
        // SAFETY: a live resource.
        let d = unsafe { texture.GetDesc() };
        for level in (0..u32::from(d.MipLevels).min(32)).filter(|&level| fresh & (1u32 << level) != 0) {
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut bytes = 0u64;
            // SAFETY: outputs for one subresource.
            unsafe {
                host.device
                    .GetCopyableFootprints(&d, level, 1, 0, Some(&mut footprint), None, None, Some(&mut bytes));
            }
            let Some((buffer, data)) = list.upload(host, bytes) else {
                // (With the levels that share its tiles, if it is one of
                // the small ones.)
                unzeroed |= tiled.with_level(level);
                continue;
            };
            // SAFETY: a mapped buffer of `bytes` bytes, just made.
            unsafe { std::ptr::write_bytes(data, 0, bytes as usize) };
            let to = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: a borrowed pointer the location never releases.
                pResource: unsafe { std::mem::transmute_copy(texture) },
                Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: level },
            };
            let from = D3D12_TEXTURE_COPY_LOCATION {
                // SAFETY: as above; the list holds the buffer until it is reset.
                pResource: unsafe { std::mem::transmute_copy(&buffer) },
                Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
            };
            // SAFETY: an open list; both locations name live resources.
            unsafe { list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
        }
    }
    // A new level without its zeros gets no tiles from this list: the
    // card's write into it is lost (the level reads zeros, as before the
    // write), which is less wrong than a part write over what the tiles'
    // last owner left. The next list that writes it claims it again.
    unzeroed &= fresh;
    if unzeroed != 0 {
        tiled.unclaim(unzeroed);
    }
    let levels = levels & !unzeroed;
    if levels == 0 {
        return;
    }
    let memory = crate::resource::texture_memory(resource).map_or(0, |(memory, _)| memory as u64);
    let step = Claimed {
        on_demand,
        fresh: fresh & !unzeroed,
        ran: std::sync::atomic::AtomicBool::new(false),
    };
    list.defer_host(move || {
        step.ran.store(true, std::sync::atomic::Ordering::Relaxed);
        step.on_demand.card_wrote(levels, memory);
    });
}

impl Staging {
    /// Records the copy of the staging buffer into `texture`, its host
    /// copy, which is in the copy destination state.
    pub(crate) fn record_copy(&self, list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource) {
        for i in 0..self.footprints.len() {
            self.record_copy_of(list, texture, i);
        }
    }

    /// As [`Staging::record_copy`], for subresource `i`.
    fn record_copy_of(&self, list: &ID3D12GraphicsCommandList, texture: &ID3D12Resource, i: usize) {
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer the location never releases.
            pResource: unsafe { std::mem::transmute_copy(texture) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: self.subresource(i),
            },
        };
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: as above.
            pResource: unsafe { std::mem::transmute_copy(&self.buffer) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: self.footprints[i],
            },
        };
        // SAFETY: an open list; both locations name live resources.
        unsafe { list.CopyTextureRegion(&to, 0, 0, 0, &from, None) };
    }

    /// At submission, after the check ([`Staging::refresh`]): takes the
    /// note that the staging buffer was detiled since a list last uploaded
    /// it, for a prelude that uploads it now, in queue order. `false` if
    /// nothing changed since.
    pub(crate) fn take_change(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        self.uploaded.load(Relaxed) && self.dirty.swap(false, Relaxed)
    }

    /// Levels on demand: what says, beside the mapping, which levels of
    /// the texture are the title's now. First the lowest level a live view
    /// of the texture shows (0 without a view that says): the title's own
    /// word, a texture it holds from level 3 on is viewed from level 3.
    /// Then where the next texture placed in this one's span starts, in
    /// bytes from this one's start (`u64::MAX`: none does): for a texture
    /// viewed from level 0 although a neighbour sits where its large
    /// levels would be.
    fn judgement(&self) -> [u64; 2] {
        let start = self.memory as u64;
        let first = crate::descriptor::host::first_viewed(start).map_or(0, u64::from);
        let limit = crate::descriptor::host::next_start(start, start.saturating_add(self.layout.size))
            .map_or(u64::MAX, |at| at - start);
        [first, limit]
    }

    /// What [`Staging::judgement`] said at the last detiling.
    fn judged(&self) -> [u64; 2] {
        use std::sync::atomic::Ordering::Relaxed;
        [self.judged[0].load(Relaxed), self.judged[1].load(Relaxed)]
    }

    /// Levels on demand: the levels that are the title's now, a bit a
    /// level. A level is held if no view starts above it, its bytes end
    /// before the next texture starts, and all of them are mapped. The
    /// smallest levels share the texture's first block and are held
    /// together. (The console layout: that block first, then the other
    /// levels from the smallest up, level 0 last:
    /// `victory_formats::texture::Layout`.) A texture this cannot tell
    /// the levels of holds them all.
    fn held_levels(&self, [first, limit]: [u64; 2]) -> u32 {
        let mips = &self.layout.mips;
        if self.layout.slices != 1 || mips.is_empty() || mips.len() > 32 {
            return u32::MAX;
        }
        let tail = (self.layout.first_tail_mip as usize).min(mips.len());
        let there = |from: u64, to: u64| {
            to > from && to <= limit && committed(self.memory.wrapping_add(from as usize), (to - from) as usize)
        };
        let mut held = 0u32;
        for (level, m) in mips.iter().enumerate().take(tail) {
            let end = if level == 0 { self.layout.slice_size } else { mips[level - 1].offset };
            if level as u64 >= first && there(m.offset, end) {
                held |= 1 << level;
            }
        }
        if tail < mips.len() && there(0, self.layout.alignment.min(self.layout.slice_size)) {
            for level in tail..mips.len() {
                if level as u64 >= first {
                    held |= 1 << level;
                }
            }
        }
        held
    }

    /// Forgets what the staging buffer was last filled from: the next
    /// refresh detiles anew, whether the memory changed or not.
    fn forget(&self) {
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Last::default();
    }

    /// The title memory the texture is read from: address and bytes.
    pub(crate) fn memory(&self) -> (usize, usize) {
        (self.memory as usize, self.size)
    }

    /// The host mip and the array slice of the `i`th part of the staging
    /// buffer.
    fn part(&self, i: usize) -> (u32, u32) {
        let mips = self.mips - self.first;
        (self.first + i as u32 % mips, i as u32 / mips)
    }

    /// The host texture's subresource the `i`th part of the staging buffer is.
    fn subresource(&self, i: usize) -> u32 {
        let (mip, slice) = self.part(i);
        slice * self.mips + mip
    }

    /// Notes that late submission `submission` may still be reading this
    /// staging buffer on the GPU.
    pub(crate) fn mark_in_flight(&self, submission: u64) {
        self.in_flight
            .fetch_max(submission, std::sync::atomic::Ordering::Release);
    }

    /// The highest late submission number that may still be reading this
    /// staging buffer on the GPU (`0` if none).
    pub(crate) fn in_flight(&self) -> u64 {
        self.in_flight.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Detiles the texture memory into the staging buffer if it changed
    /// since the last time (and is mapped at all).
    pub(crate) fn refresh(&self) {
        use std::sync::atomic::Ordering::Relaxed;
        let frame = crate::frame::current();
        if !within_budget(&mut SPENT.lock().unwrap_or_else(|e| e.into_inner()), frame, 0, budget_us()) {
            // Left as it is (nothing learnt about writes), so checked again
            // next frame; its host copy shows what it showed.
            DEFERRED.fetch_add(1, Relaxed);
            return;
        }
        let started = std::time::Instant::now();
        let read = self.refresh_now();
        let us = started.elapsed().as_micros() as u64;
        within_budget(&mut SPENT.lock().unwrap_or_else(|e| e.into_inner()), frame, us, u64::MAX);
        let preparing = &crate::host::PREPARING;
        preparing[2].fetch_add(us, Relaxed);
        preparing[3].fetch_add(read as u64, Relaxed);
    }

    /// Returns the bytes of title memory read.
    fn refresh_now(&self) -> usize {
        let whole = std::time::Instant::now();
        let read = self.refresh_timed();
        let us = whole.elapsed().as_micros() as u64;
        REFRESH_WAITS[3].fetch_add(us, std::sync::atomic::Ordering::Relaxed);
        crate::framelog::add(crate::framelog::T::RefreshUs, us);
        read
    }

    fn refresh_timed(&self) -> usize {
        let size = self.size;
        let start = self.memory as usize;
        let mut last = waited(0, || self.last.lock().unwrap_or_else(|e| e.into_inner()));
        let last = &mut *last;
        if let Some((now, written)) = waited(1, || crate::watch::writes(start, start + size, false)) {
            if last.asked != 0 && written.iter().all(|&(_, w)| w <= last.asked) {
                return 0;
            }
            last.asked = now;
            // Read through the copy, which stands unmapped pages: memory of
            // big pools is too slow to ask page by page.
            with_read(size, |read| {
                crate::host::copy_mapped(read.as_mut_ptr(), self.memory, size, false);
                self.detile(read);
            });
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::TexWatched, size as u64);
            return size;
        }
        // View memory has no write-watch, and a texture there was hashed
        // whole at every check: 229 MB and 51 ms a frame in a gameplay view
        // where two textures change (run 58). It is guarded now, as buffers
        // in view memory are: a write of the title's to it faults once and
        // is noted, a mapping is announced by the runtime. While neither
        // happened the texture is as it was. A write through another view
        // of the same pages is a fault of that view too (the guard makes
        // every view of them read-only, `crate::watch`), and a write the
        // runtime makes itself into physical pages is told to the guard
        // there; the hash is the net under both. Stable views take turns
        // about every eighth frame, by the texture's address; a view whose
        // hash changed with nothing reported is checked every frame while
        // it keeps changing ([`Last::check_quiet_view`]).
        // `VICTORY_TEXTURE_GUARD=0` hashes at every check, as before.
        #[cfg(feature = "diagnostics")]
        let mut unreported = false;
        static GUARD: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let mut guard = None;
        if self.guard_view
            && *GUARD.get_or_init(|| std::env::var_os("VICTORY_TEXTURE_GUARD").is_none_or(|v| v != "0"))
            && let Some((now, written)) = waited(1, || crate::watch::writes(start, start + size, true))
        {
            let unwritten = last.asked != 0 && written.iter().all(|&(_, w)| w <= last.asked);
            last.asked = now;
            let frame = crate::frame::current();
            // Once a frame at most, not at each of its ~32 submissions that
            // sample the texture (`Last::check_quiet_view`).
            if unwritten && !last.check_quiet_view(frame, start) {
                return 0;
            }
            guard = Some((frame, unwritten));
            #[cfg(feature = "diagnostics")]
            {
                unreported = unwritten;
            }
        }
        // From here on the texture's memory may be read in place. Held until
        // that read is done: the runtime changes what title memory shows
        // only with this taken alone, so what `committed` finds mapped
        // stays mapped (see `crate::watch::title_memory_reading`; without
        // it a page went between the asking and the read in runs 71 and
        // 74).
        let _mapped = if no_wait() {
            // Not waited for: while the runtime remaps title memory the
            // texture is read through the kernel instead, whole or not at
            // all (`no_wait`).
            let Some(mapped) = crate::watch::title_memory_reading_now(start, size) else {
                return self.refresh_unlocked(last, size, guard);
            };
            mapped
        } else {
            waited(2, || crate::watch::title_memory_reading(start, size))
        };
        if !committed(self.memory, size) {
            // Partly mapped: a streamed texture with only some mips' pages
            // there, the usual state while it streams in. Read through the
            // copy, which stands unmapped pages (zeros), and upload what
            // is there; nothing at all mapped changes nothing.
            // shortcut: read again at most every 30 frames (reading around
            // unmapped pages is slow, and nothing tells us when pages
            // arrive): a mip that streams in shows up to 30 frames late.
            let frame = crate::frame::current();
            if last.partial.is_some_and(|at| frame < at + 30) {
                return 0;
            }
            last.partial = Some(frame);
            #[cfg(feature = "diagnostics")]
            crate::copystat::add(crate::copystat::C::TexHashed, size as u64);
            let changed = with_read(size, |read| {
                crate::host::copy_mapped(read.as_mut_ptr(), self.memory, size, false);
                let hash = blake3::hash(read);
                if !last.changed(hash, guard) {
                    return false;
                }
                if read.iter().all(|&b| b == 0) {
                    return false;
                }
                self.detile(read);
                #[cfg(feature = "diagnostics")]
                if unreported {
                    self.unreported_change();
                }
                true
            });
            #[cfg(feature = "diagnostics")]
            if changed {
                crate::copystat::add(crate::copystat::C::TexChanged, size as u64);
            }
            let _ = changed;
            return size;
        }
        if through_copy() {
            return with_title(self.memory, size, |tiled| {
                let hash = blake3::hash(tiled);
                #[cfg(feature = "diagnostics")]
                crate::copystat::add(crate::copystat::C::TexHashed, size as u64);
                if !last.changed(hash, guard) {
                    return size;
                }
                self.detile(tiled);
                #[cfg(feature = "diagnostics")]
                if unreported {
                    self.unreported_change();
                }
                #[cfg(feature = "diagnostics")]
                crate::copystat::add(crate::copystat::C::TexChanged, size as u64);
                size
            });
        }
        // SAFETY: committed memory of the texture's layout size, and it
        // stays committed while `_mapped` is held.
        let tiled = unsafe { std::slice::from_raw_parts(self.memory, size) };
        let hash = blake3::hash(tiled);
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::TexHashed, size as u64);
        if !last.changed(hash, guard) {
            return size;
        }
        self.detile(tiled);
        #[cfg(feature = "diagnostics")]
        if unreported {
            self.unreported_change();
        }
        #[cfg(feature = "diagnostics")]
        crate::copystat::add(crate::copystat::C::TexChanged, size as u64);
        size
    }

    /// Diagnostics: the first time a texture's hash found a change that the
    /// guard over its memory did not report (a write through another view
    /// of its pages, which the guard now watches too).
    #[cfg(feature = "diagnostics")]
    fn unreported_change(&self) {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(
                address = format_args!("{:#x}", self.memory as usize),
                size = self.layout.size,
                desc = ?self.layout.desc,
                "a texture's hash found a change its guard did not report: a write through another view of its memory"
            );
        });
    }

    /// A refresh while the runtime is remapping title memory ([`no_wait`]):
    /// the texture is read through the kernel, which fails if a page of it
    /// is not there at this moment. Then nothing is taken and the texture
    /// is looked at again at its next refresh (what was learnt about writes
    /// to it is forgotten, so that it is not passed over as unwritten).
    /// Returns the bytes read.
    fn refresh_unlocked(&self, last: &mut Last, size: usize, guard: Option<(u64, bool)>) -> usize {
        use std::sync::atomic::Ordering::Relaxed;
        // A texture streaming in fails here at every submission that
        // samples it (32 a frame), each read running up to its first page
        // not there: once a frame is enough.
        let frame = crate::frame::current();
        if last.left == Some(frame) {
            last.asked = 0;
            return 0;
        }
        let read = with_read(size, |read| {
            if !crate::host::read_into(self.memory, read) {
                return false;
            }
            let hash = blake3::hash(read);
            if last.changed(hash, guard) {
                self.detile(read);
            }
            true
        });
        if read {
            UNLOCKED[0].fetch_add(1, Relaxed);
            size
        } else {
            UNLOCKED[1].fetch_add(1, Relaxed);
            last.asked = 0;
            last.left = Some(frame);
            0
        }
    }

    /// Detiles `tiled` (the texture's memory) into the staging buffer.
    fn detile(&self, tiled: &[u8]) {
        self.dirty.store(true, std::sync::atomic::Ordering::Relaxed);
        // Levels on demand: which levels are the title's is judged here,
        // once, for what the staging buffer holds from now on. The step
        // that gives levels their memory goes by this answer.
        let held = if self.on_demand {
            use std::sync::atomic::Ordering::Relaxed;
            let judged = self.judgement();
            self.judged[0].store(judged[0], Relaxed);
            self.judged[1].store(judged[1], Relaxed);
            let held = self.held_levels(judged);
            self.detiled.store(held, Relaxed);
            held
        } else {
            u32::MAX
        };
        #[cfg(feature = "diagnostics")]
        if tiled.iter().all(|&b| b == 0) {
            tracing::debug!(
                memory = ?self.memory,
                desc = ?self.layout.desc,
                "a sampled texture's memory is all zero (not streamed yet, or a failed BCPack read)"
            );
        }
        for (i, (footprint, &rows)) in self.footprints.iter().zip(&self.rows).enumerate() {
            let (host_mip, slice) = self.part(i);
            let console_mip = host_mip + self.mip_drop;
            let f = footprint.Footprint;
            let pitch = f.RowPitch as usize;
            let len = pitch * rows as usize * f.Depth as usize;
            // SAFETY: the subresource's part of the mapped staging buffer.
            let linear = unsafe {
                std::slice::from_raw_parts_mut(self.data.add(footprint.Offset as usize), len)
            };
            if console_mip < 32 && held & (1 << console_mip) == 0 {
                // Not the title's: zeros, not what lies in that memory.
                linear.fill(0);
                continue;
            }
            if let Err(e) = detile_wide(&self.layout, tiled, console_mip, slice, linear, pitch, rows) {
                tracing::warn!(?e, console_mip, slice, "texture not detiled");
                return;
            }
        }
    }
}

/// A subresource of at least this many bytes is detiled in bands on the
/// kept threads ([`crate::shadow::wide_run`]): one thread detiles some
/// 4.5 GB/s, and a 33-67 MB texture went 1.7-2 times as fast on four
/// (`victory-formats`' `detile_threads` benchmark).
const BAND_BYTES: usize = 4 << 20;

/// [`Layout::detile`](victory_formats::texture::Layout::detile) of one
/// subresource into `linear` (`rows` rows `pitch` bytes apart, depth slices
/// `pitch * rows` apart); a big one of one slice deep is cut into bands of
/// rows detiled on several threads.
fn detile_wide(
    layout: &victory_formats::texture::Layout,
    tiled: &[u8],
    mip: u32,
    slice: u32,
    linear: &mut [u8],
    pitch: usize,
    rows: u32,
) -> Result<(), victory_formats::texture::Error> {
    let height = layout.mips.get(mip as usize).map_or(0, |m| m.height);
    let deep = layout.subresource_depth(mip) != 1;
    if deep || linear.len() < 2 * BAND_BYTES || pitch == 0 || height < 2 || height > rows {
        return layout.detile(tiled, mip, slice, linear, pitch, pitch * rows as usize);
    }
    let band = (BAND_BYTES / pitch).clamp(1, height as usize) as u32;
    let failed = std::sync::Arc::new(Mutex::new(None));
    let (tiled_at, tiled_len) = (tiled.as_ptr() as usize, tiled.len());
    let layout_at = std::ptr::from_ref(layout) as usize;
    let jobs: Vec<Box<dyn FnOnce() + Send>> = linear[..height as usize * pitch]
        .chunks_mut(band as usize * pitch)
        .enumerate()
        .map(|(k, part)| {
            let y = k as u32 * band;
            let rows = y..(y + band).min(height);
            let (part_at, part_len) = (part.as_mut_ptr() as usize, part.len());
            let failed = failed.clone();
            Box::new(move || {
                // SAFETY: `wide_run` returns only when every job has run,
                // while the caller's borrows of the layout and both buffers
                // last; the bands are disjoint parts of `linear`.
                let (layout, tiled, part) = unsafe {
                    (
                        &*(layout_at as *const victory_formats::texture::Layout),
                        std::slice::from_raw_parts(tiled_at as *const u8, tiled_len),
                        std::slice::from_raw_parts_mut(part_at as *mut u8, part_len),
                    )
                };
                if let Err(e) = layout.detile_rows(tiled, mip, slice, part, pitch, rows) {
                    *failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                }
            }) as Box<dyn FnOnce() + Send>
        })
        .collect();
    crate::shadow::wide_run(jobs);
    match failed.lock().unwrap_or_else(|e| e.into_inner()).take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Lets go of a title texture's staging buffer (lists that recorded an
/// upload from it hold it until they have run); the next upload makes one
/// anew.
pub(crate) fn unstage(resource: This) {
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = map.as_mut() {
        map.remove(&(resource as usize));
    }
}

/// Where a staged title texture's memory is: address and bytes.
pub(crate) fn staged_memory(resource: This) -> Option<(u64, u64)> {
    let map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let (at, len) = map.as_ref()?.get(&(resource as usize))?.memory();
    Some((at as u64, len as u64))
}

/// A fingerprint of the title memory `[at, at + len)`, never 0: of all of
/// it up to 48 KiB, else of 16 KiB at its start, its middle and its end
/// (a texture streams in whole mips; the small ones lie first). Read
/// through the kernel, so pages not there count as zeros.
pub(crate) fn fingerprint(at: u64, len: u64) -> u64 {
    const WINDOW: usize = 16 << 10;
    thread_local! {
        static READ: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    let len = len as usize;
    let mut hasher = blake3::Hasher::new();
    READ.with(|read| {
        let mut read = read.borrow_mut();
        read.resize(3 * WINDOW, 0);
        let mut take = |from: usize, bytes: usize| {
            read[..bytes].fill(0);
            crate::host::copy_mapped(read.as_mut_ptr(), (at as usize + from) as *const u8, bytes, false);
            hasher.update(&read[..bytes]);
        };
        if len <= 3 * WINDOW {
            take(0, len);
        } else {
            take(0, WINDOW);
            take((len / 2) & !0xfff, WINDOW);
            take(len - WINDOW, WINDOW);
        }
    });
    let hash = hasher.finalize();
    let mut first = [0u8; 8];
    first.copy_from_slice(&hash.as_bytes()[..8]);
    u64::from_le_bytes(first) | 1
}

/// Whether a title texture has a staging buffer: a list recorded its
/// upload, and the lists that sample it check it at submission.
/// For the frame listing: whether a list has uploaded `resource`'s staging
/// buffer, and whether it was detiled since. None: it has none.
#[cfg(feature = "diagnostics")]
pub(crate) fn staged_state(resource: This) -> Option<(bool, bool)> {
    use std::sync::atomic::Ordering::Relaxed;
    let map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let staging = map.as_ref()?.get(&(resource as usize))?;
    Some((staging.uploaded.load(Relaxed), staging.dirty.load(Relaxed)))
}

pub(crate) fn is_staged(resource: This) -> bool {
    let map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    map.as_ref().is_some_and(|map| map.contains_key(&(resource as usize)))
}

/// The bytes of a title texture's memory, and whether all of it is mapped,
/// if the texture has been staged for upload.
pub(crate) fn staged(resource: This) -> Option<(u64, bool)> {
    let s = {
        let map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
        map.as_ref()?.get(&(resource as usize))?.clone()
    };
    Some((s.size as u64, committed(s.memory, s.size)))
}

/// Whether a title texture's staging buffer holds the console mips from
/// `first` on (false without one).
fn staged_covers(resource: This, first: u32) -> bool {
    let map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    map.as_ref()
        .and_then(|map| map.get(&(resource as usize)))
        .is_some_and(|s| s.covers(first))
}

impl Staging {
    /// Whether it holds the console mips from `first` on.
    fn covers(&self, first: u32) -> bool {
        self.first == 0 || self.first + self.mip_drop <= first
    }
}

/// Whether `[at, at + len)` is committed memory (streamed textures may be
/// unmapped until their pages arrive).
pub(crate) fn committed(at: *const u8, len: usize) -> bool {
    if by_query() {
        return committed_by_query(at, len);
    }
    if len == 0 {
        return true;
    }
    let (there, of) = readable(at as usize, len, true);
    there == of
}

/// Whether the two questions above and below are put to `VirtualQuery`, as
/// they were (`VICTORY_COMMITTED_QUERY=1`). `VirtualQuery` answers with the
/// length of the run of pages in the same state, which it finds by walking
/// the page tables to the run's end: 8.8 ms for one question in the title's
/// 1.1 GB block (round 6). The pass over the heap's views asks three or
/// more of them for every texture slot that is due, 3,000 slots a frame
/// when the card has no room (run 75): that pass then ran 208-264 ms, and
/// while it ran the submitting thread's write-watch askings waited behind
/// the walks (run 76: 8-22 askings a frame of 6-21 ms each, 46-172 ms).
fn by_query() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_COMMITTED_QUERY").is_some_and(|v| v == "1"))
}

/// Of the kernel's pages that hold `[at, at + len)` (`len > 0`): how many
/// can be read, and how many there are; with `all`, it stops at the first
/// that cannot. One question to the kernel for up to 256 pages at once
/// (`QueryWorkingSetEx`: the state of each page asked about, no walk); a
/// page not in the working set may still be there (never touched, or
/// trimmed), and a one-byte read through the kernel says.
/// A page that is committed and may not be read counts as not there, which
/// is what the callers ask for: they read or write the memory next.
fn readable(at: usize, len: usize, all: bool) -> (usize, usize) {
    use windows_sys::Win32::System::ProcessStatus::{K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    const KERNEL_PAGE: usize = 4096;
    let (first, last) = (at / KERNEL_PAGE, at.saturating_add(len - 1) / KERNEL_PAGE);
    let of = last - first + 1;
    let mut there = 0;
    // SAFETY: plain data.
    let mut info: [PSAPI_WORKING_SET_EX_INFORMATION; 256] = unsafe { std::mem::zeroed() };
    let mut page = first;
    while page <= last {
        let n = (last - page + 1).min(info.len());
        for (i, entry) in info[..n].iter_mut().enumerate() {
            entry.VirtualAddress = ((page + i) * KERNEL_PAGE) as *mut _;
        }
        // SAFETY: `n` entries of the size given; our own process.
        let asked = unsafe {
            K32QueryWorkingSetEx(
                GetCurrentProcess(),
                info.as_mut_ptr().cast(),
                (n * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32,
            )
        } != 0;
        for (i, entry) in info[..n].iter().enumerate() {
            // Bit 0 of the answer: the page is in the working set.
            // SAFETY: plain data the query filled in.
            let valid = asked && unsafe { entry.VirtualAttributes.Flags } & 1 != 0;
            let mut byte = [0u8];
            if valid || crate::host::read_into(((page + i) * KERNEL_PAGE) as *const u8, &mut byte) {
                there += 1;
            } else if all {
                return (there, of);
            }
        }
        page += n;
    }
    (there, of)
}

fn committed_by_query(at: *const u8, len: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut p = at as usize;
    let end = p + len;
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

/// How many bytes of `[at, at + len)` are committed memory: all of a
/// texture that is there, some of one streaming in (its smaller mips lie
/// at the start of its memory, and their pages are mapped first), none of
/// one not there. With neither the first nor the last page committed the
/// answer is none without a look at the pages between (two queries for the
/// slots whose texture is away, which are asked again and again).
pub(crate) fn mapped_bytes(at: *const u8, len: usize) -> u64 {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, VirtualQuery};
    if len == 0 {
        return 0;
    }
    let start = at as usize;
    let end = start.saturating_add(len);
    if !committed(at, 1) && !committed((end - 1) as *const u8, 1) {
        return 0;
    }
    if !by_query() {
        static BY_PAGE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if *BY_PAGE.get_or_init(|| std::env::var_os("VICTORY_MAPPED_BY_PAGE").is_some_and(|v| v == "1")) {
            // By page, without the walk: whole pages, cut to the range.
            let (there, _) = readable(start, len, false);
            return (there as u64 * 4096).min(len as u64);
        }
        // By 64 KiB block, one page asked of each: the title's memory is
        // mapped in blocks of that size (the console's page), and the
        // answer is a measure of how much of a texture has arrived, not a
        // promise that every byte can be read (the upload reads through the
        // kernel). By page, a texture half there cost a failed kernel read
        // for each of its missing pages, every frame it stayed due: 1,663
        // textures "uploaded in part" kept the pass over the heap's views at
        // 90-160 ms a frame (run 81). `VICTORY_MAPPED_BY_PAGE=1`: by page.
        const BLOCK: usize = 64 << 10;
        let mut there = 0u64;
        let mut block = start & !(BLOCK - 1);
        while block < end {
            let (lo, hi) = (block.max(start), block.saturating_add(BLOCK).min(end));
            if readable(lo, 1, true).0 == 1 {
                there += (hi - lo) as u64;
            }
            block = block.saturating_add(BLOCK);
        }
        return there;
    }
    let (mut p, mut there) = (start, 0u64);
    while p < end {
        // SAFETY: plain data filled in by the query.
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a valid out pointer of the right size.
        let n = unsafe { VirtualQuery(p as *const _, &mut info, size_of_val(&info)) };
        let next = (info.BaseAddress as usize).saturating_add(info.RegionSize).min(end);
        if n == 0 || next <= p {
            break;
        }
        if info.State == MEM_COMMIT {
            there += (next - p) as u64;
        }
        p = next;
    }
    there
}

/// The staging state of a title texture from console mip `first` on, made
/// on first use (and made anew for larger mips than the one there holds).
fn staging(resource: This, texture: &ID3D12Resource, first: u32) -> Option<Arc<Staging>> {
    // Asked before the table is locked: the sweep lets go of a staging
    // buffer with its texture's host side locked, in that order.
    let on_demand = crate::resource::host_tiled(resource).is_some();
    let mut map = STAGING.lock().unwrap_or_else(|e| e.into_inner());
    let map = map.get_or_insert_with(HashMap::new);
    if let Some(s) = map.get(&(resource as usize))
        && s.covers(first)
    {
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
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let mip_drop = u32::from(desc.mip_levels).saturating_sub(u32::from(d.MipLevels));
    // Software-decoded movie planes are linear R8 textures. The decoder can
    // write through another view of the same pages, bypassing the guards on
    // the sampled view. Check their content on every submission so cycling
    // through the movie's textures does not replay old frames.
    let guard_view = !(d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && d.Format == DXGI_FORMAT_R8_UNORM
        && d.MipLevels == 1
        && d.DepthOrArraySize == 1
        && layout.desc.swizzle == victory_formats::texture::Swizzle::LINEAR);
    let (first, size) = staged_mips(&layout, first, mip_drop, u32::from(d.MipLevels));
    let count = if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D {
        u32::from(d.MipLevels)
    } else {
        u32::from(d.MipLevels) * u32::from(d.DepthOrArraySize) - first
    };
    let mut footprints = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
    let mut rows = vec![0u32; count as usize];
    let mut total = 0;
    // SAFETY: outputs for `count` subresources.
    unsafe {
        host.device.GetCopyableFootprints(
            &d,
            first,
            count,
            0,
            Some(footprints.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            None,
            Some(&mut total),
        );
    }
    let (buffer, data, pooled) = staging_buffer(host, total)?;
    crate::host::name(&buffer, || {
        format!(
            "staging of {resource:?} {}x{}x{} f{}",
            d.Width, d.Height, d.DepthOrArraySize, d.Format.0
        )
    });
    tracing::debug!(?desc, total, mip_drop, "texture contents uploaded from title memory");
    STAGED[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    STAGED[1].fetch_add(total, std::sync::atomic::Ordering::Relaxed);
    let s = Arc::new(Staging {
        buffer,
        #[cfg(feature = "diagnostics")]
        total,
        data,
        footprints,
        rows,
        layout,
        mips: u32::from(d.MipLevels),
        first,
        size,
        bytes: total,
        mip_drop,
        memory,
        guard_view,
        last: Mutex::default(),
        dirty: std::sync::atomic::AtomicBool::new(true),
        uploaded: std::sync::atomic::AtomicBool::new(false),
        used: std::sync::atomic::AtomicU64::new(crate::host::moment()),
        in_flight: std::sync::atomic::AtomicU64::new(0),
        on_demand,
        detiled: std::sync::atomic::AtomicU32::new(0),
        judged: [const { std::sync::atomic::AtomicU64::new(0) }; 2],
        waiting: std::sync::atomic::AtomicU64::new(0),
        pooled,
    });
    map.insert(resource as usize, s.clone());
    Some(s)
}

/// The first host mip a staging buffer uploads (0: all) and the bytes of
/// title memory it reads, for a view from console mip `first` on of a
/// texture whose host copy lacks its `mip_drop` largest mips and has
/// `host_mips`: the console mips from `first` (or the first the host copy
/// has) on are read, and host mips are left out only where the title's
/// layout can end behind the smaller mips
/// ([`victory_formats::texture::Layout::bytes_from`]).
fn staged_mips(layout: &victory_formats::texture::Layout, first: u32, mip_drop: u32, host_mips: u32) -> (u32, usize) {
    let console_first = first.max(mip_drop);
    let size = layout.bytes_from(console_first) as usize;
    if console_first < mip_drop + host_mips && size < layout.size as usize && console_first > mip_drop {
        (console_first - mip_drop, size)
    } else {
        (0, layout.bytes_from(mip_drop) as usize)
    }
}

#[cfg(test)]
mod staged_mips_tests {
    use super::staged_mips;
    use victory_formats::texture::{Desc, Dimension, Layout, Swizzle};

    /// A view of the smaller mips reads only their memory, and uploads
    /// from the host mip it is; with the largest mips dropped on the host,
    /// host mips count from the first the host copy has.
    #[test]
    fn a_view_of_the_smaller_mips_stages_only_them() {
        let desc = Desc {
            dimension: Dimension::Tex2d,
            width: 1024,
            height: 1024,
            depth: 1,
            mips: 9,
            element_bytes: 8,
            block: 4,
            samples: 1,
            swizzle: Swizzle::SW_4KB_S,
        };
        let l = Layout::new(&desc).unwrap();
        let size = l.size as usize;
        assert_eq!(staged_mips(&l, 0, 0, 9), (0, size));
        assert_eq!(staged_mips(&l, 3, 0, 9), (3, l.bytes_from(3) as usize));
        // Largest mip dropped: all host mips, read from console mip 1 on.
        assert_eq!(staged_mips(&l, 0, 1, 8), (0, l.bytes_from(1) as usize));
        assert_eq!(staged_mips(&l, 1, 1, 8), (0, l.bytes_from(1) as usize));
        assert_eq!(staged_mips(&l, 3, 1, 8), (2, l.bytes_from(3) as usize));
        // A first mip past the chain: the whole texture.
        assert_eq!(staged_mips(&l, 12, 0, 9), (0, size));
        // Slices each hold their chain: the whole texture.
        let l = Layout::new(&Desc { depth: 2, ..desc }).unwrap();
        assert_eq!(staged_mips(&l, 3, 0, 9), (0, l.size as usize));
    }
}

#[cfg(test)]
mod title_memory_tests {
    use super::with_title;

    /// Title memory whose tail is reserved and not committed reads through
    /// the copy as its bytes and then zeros, without a fault; so does
    /// memory a page of which is taken away between two reads, and the
    /// zeros are zeros whatever the read buffer held before.
    #[test]
    fn an_uncommitted_tail_reads_as_zeros() {
        use windows_sys::Win32::System::Memory::*;
        const HEAD: usize = 0xb_0000;
        const SIZE: usize = 0x20_0000;
        // SAFETY: a private reservation of our own; only committed pages
        // are written in place, and nothing else reads them in place.
        unsafe {
            // The read buffer holds something else first.
            let other = vec![0x77u8; SIZE];
            with_title(other.as_ptr(), SIZE, |bytes| assert!(bytes.iter().all(|&b| b == 0x77)));
            let base = VirtualAlloc(std::ptr::null(), SIZE, MEM_RESERVE, PAGE_NOACCESS).cast::<u8>();
            assert!(!base.is_null());
            assert_eq!(VirtualAlloc(base.cast(), HEAD, MEM_COMMIT, PAGE_READWRITE), base.cast());
            base.write_bytes(0x5a, HEAD);
            with_title(base, SIZE, |bytes| {
                assert!(bytes[..HEAD].iter().all(|&b| b == 0x5a), "what is mapped");
                assert!(bytes[HEAD..].iter().all(|&b| b == 0), "what is not: zeros");
            });
            // The title takes a page in the middle away: zeros from then
            // on, the rest as before.
            assert_ne!(VirtualFree(base.add(0x4_0000).cast(), 0x1_0000, MEM_DECOMMIT), 0);
            with_title(base, SIZE, |bytes| {
                assert!(bytes[..0x4_0000].iter().all(|&b| b == 0x5a));
                assert!(bytes[0x4_0000..0x5_0000].iter().all(|&b| b == 0));
                assert!(bytes[0x5_0000..HEAD].iter().all(|&b| b == 0x5a));
                assert!(bytes[HEAD..].iter().all(|&b| b == 0));
            });
            assert_ne!(VirtualFree(base.cast(), 0, MEM_RELEASE), 0);
        }
    }

    /// The texture both device-loss runs named last (16 texels wide, 4 or 8
    /// high, format 10, layout 0x100, console flags 0x300000; its memory
    /// began 0x300 into a page in run 73 and 0x700 in run 72, and here also
    /// 0xf00, so that a page line runs through it): what the detiling is
    /// given and what it writes are the same read in place and read through
    /// the copy, and the rows it writes end where the staging buffer the
    /// host sizes for it ends (896 bytes for 4 rows of 128 bytes 256 apart,
    /// 1,920 for 8).
    #[test]
    fn the_small_texture_of_runs_72_and_73_detiles_the_same_both_ways() {
        use windows_sys::Win32::System::Memory::*;
        for (height, within) in [(4u32, 0x300usize), (8, 0x700), (4, 0xf00), (8, 0xf00)] {
            // The description as the log printed it, word for word.
            let words: [u32; 14] = [3, 0, 0, 0, 16, 0, height, 65537, 10, 1, 0, 256, 3_145_728, 0];
            // SAFETY: fourteen words are a resource description (56 bytes,
            // plain numbers).
            let desc: crate::resource::ResourceDesc = unsafe { std::mem::transmute(words) };
            let layout = crate::resource::texture_layout(&desc)
                .expect("a console layout")
                .planes[0]
                .layout
                .clone();
            let size = layout.size as usize;
            let (row, pitch) = (16 * 8, 256usize);
            assert!(size >= row * height as usize, "the layout holds every texel");
            // Committed pages and a reserved one after them; the texture
            // begins `within` bytes into the first, as the title placed it.
            // SAFETY: our own reservation; the committed part is written
            // and read in place, by this thread alone.
            unsafe {
                let pages = size.div_ceil(4096) + 1;
                let base = VirtualAlloc(std::ptr::null(), (pages + 1) * 4096, MEM_RESERVE, PAGE_NOACCESS).cast::<u8>();
                assert!(!base.is_null());
                assert_eq!(VirtualAlloc(base.cast(), pages * 4096, MEM_COMMIT, PAGE_READWRITE), base.cast());
                for i in 0..pages * 4096 {
                    base.add(i).write((i * 7 + i / 251) as u8);
                }
                let memory = base.add(within);
                let in_place = std::slice::from_raw_parts(memory, size);
                // The bytes: the same in place and through the copy.
                with_title(memory, size, |copied| assert_eq!(copied, in_place));
                // What the detiling writes: the same, and inside the bytes
                // the host's staging buffer for this texture has.
                let staged = (height as usize - 1) * pitch + row;
                assert_eq!(staged, if height == 4 { 896 } else { 1920 });
                let detile = |tiled: &[u8]| {
                    // The slice `Staging::detile` hands over: whole rows.
                    let mut linear = vec![0xeeu8; pitch * height as usize];
                    layout
                        .detile(tiled, 0, 0, &mut linear, pitch, pitch * height as usize)
                        .expect("detiled");
                    linear
                };
                let direct = detile(in_place);
                let through = with_title(memory, size, |copied| detile(copied));
                assert_eq!(direct, through);
                for y in 0..height as usize {
                    assert!(direct[y * pitch..][..row].iter().any(|&b| b != 0xee), "row {y} written");
                    assert!(direct[y * pitch + row..(y + 1) * pitch].iter().all(|&b| b == 0xee), "nothing past row {y}");
                }
                // A buffer of exactly the staged bytes is enough.
                let mut exact = vec![0u8; staged];
                layout
                    .detile(in_place, 0, 0, &mut exact, pitch, pitch * height as usize)
                    .expect("the staging buffer's bytes are enough");
                assert_ne!(VirtualFree(base.cast(), 0, MEM_RELEASE), 0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_vram_query_does_not_panic() {
        if let Some(bytes) = query_adapter_vram_bytes() {
            assert!(bytes > 0);
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::within_budget;

    /// A frame's checks go on until their time reaches the budget, then
    /// wait for the next frame; without a budget they all go on.
    #[test]
    fn checks_past_the_budget_wait_for_the_next_frame() {
        let mut spent = (0, 0);
        assert!(within_budget(&mut spent, 7, 6_000, 10_000));
        assert!(within_budget(&mut spent, 7, 6_000, 10_000));
        assert!(!within_budget(&mut spent, 7, 0, 10_000), "12 ms spent of 10");
        assert!(within_budget(&mut spent, 8, 0, 10_000), "a new frame");
        assert_eq!(spent, (8, 0));
        assert!(within_budget(&mut spent, 8, 50_000, 0));
        assert!(within_budget(&mut spent, 8, 0, 0), "no limit");
    }
}

#[cfg(test)]
mod detile_tests {
    use super::{BAND_BYTES, detile_wide};
    use victory_formats::texture::{Desc, Dimension, Layout, Swizzle};

    /// Big subresources detiled in bands on several threads, and small ones
    /// whole, give the rows of one whole detile, also with padded rows and
    /// in the mips of a big texture.
    #[test]
    fn a_texture_detiled_in_bands_is_the_one_detiled_whole() {
        let desc = |w, h, mips, bpe, block, swizzle| Desc {
            dimension: Dimension::Tex2d,
            width: w,
            height: h,
            depth: 1,
            mips,
            element_bytes: bpe,
            block,
            samples: 1,
            swizzle,
        };
        for d in [
            desc(2048, 1500, 3, 4, 1, Swizzle::SW_64KB_S),
            desc(4096, 4096, 2, 16, 4, Swizzle::SW_64KB_D),
            desc(1030, 2051, 1, 8, 1, Swizzle::SW_4KB_S),
            desc(256, 256, 1, 4, 1, Swizzle::SW_4KB_S),
        ] {
            let l = Layout::new(&d).unwrap();
            let tiled: Vec<u8> = (0..l.size as usize)
                .map(|i| (i * 7 + i / 4093) as u8)
                .collect();
            for mip in 0..d.mips {
                let m = &l.mips[mip as usize];
                let pitch = (m.width as usize * d.element_bytes as usize).next_multiple_of(256);
                let rows = m.height;
                let size = pitch * rows as usize;
                let mut whole = vec![0u8; size];
                l.detile(&tiled, mip, 0, &mut whole, pitch, size).unwrap();
                let mut banded = vec![0xA5u8; size];
                detile_wide(&l, &tiled, mip, 0, &mut banded, pitch, rows).unwrap();
                // Row padding is not written by either: compare the rows.
                let bytes = m.width as usize * d.element_bytes as usize;
                for y in 0..rows as usize {
                    assert_eq!(
                        banded[y * pitch..][..bytes],
                        whole[y * pitch..][..bytes],
                        "{d:?} mip {mip} row {y} ({} bands)",
                        size.div_ceil(BAND_BYTES)
                    );
                }
            }
        }
    }

    /// The detile of a submission's big texture whole on one thread and in
    /// bands on the kept threads (a benchmark: `cargo test --release -p
    /// victory-gpu detile_in_bands -- --ignored --nocapture`).
    #[test]
    #[ignore = "benchmark"]
    fn detile_in_bands() {
        for (w, h, bpe, swizzle) in [
            (3840u32, 2160u32, 4u32, Swizzle::SW_4KB_S),
            (4096, 4096, 4, Swizzle::SW_64KB_S),
            (2048, 2048, 16, Swizzle::SW_64KB_D),
            (1024, 1024, 4, Swizzle::SW_64KB_S),
        ] {
            let d = Desc {
                dimension: Dimension::Tex2d,
                width: w,
                height: h,
                depth: 1,
                mips: 1,
                element_bytes: bpe,
                block: 1,
                samples: 1,
                swizzle,
            };
            let l = Layout::new(&d).unwrap();
            let tiled: Vec<u8> = (0..l.size as usize).map(|i| i as u8).collect();
            let pitch = (w * bpe) as usize;
            let size = pitch * h as usize;
            let mut linear = vec![0u8; size];
            let mut fastest = |f: &mut dyn FnMut(&mut [u8])| {
                (0..7)
                    .map(|_| {
                        let started = std::time::Instant::now();
                        f(&mut linear);
                        started.elapsed().as_secs_f64()
                    })
                    .fold(f64::INFINITY, f64::min)
            };
            let one = fastest(&mut |out| l.detile(&tiled, 0, 0, out, pitch, size).unwrap());
            let bands = fastest(&mut |out| detile_wide(&l, &tiled, 0, 0, out, pitch, h).unwrap());
            println!(
                "{w}x{h} {bpe} B {swizzle:?}, {:.1} MB: whole {:.2} ms, in bands {:.2} ms ({:.2}x)",
                size as f64 / 1e6,
                one * 1e3,
                bands * 1e3,
                one / bands
            );
        }
    }
}
