//! Keep CPU-backed tiled Xbox textures coherent with their desktop images.
pub(crate) mod alias;
pub(crate) mod pages;
mod descriptors;
mod packed_copy;
pub(crate) mod pool;
mod publication;
mod upload;
pub(crate) mod watch;
pub(crate) mod writeback;
pub(crate) use descriptors::{DescriptorMap, DescriptorRange, HeapViews, PlacedView, Views};
pub(crate) use publication::QueueAccess;

use super::Device;
use core::{ffi::c_void, ops::Range};
use std::collections::{BTreeMap, btree_map::Entry};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use tuxbox_xg_layout::{Desc, Plane};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
use windows::core::{HRESULT, Result};

pub(crate) fn supported_state(state: D3D12_RESOURCE_STATES, flags: D3D12_RESOURCE_FLAGS) -> bool {
    let allowed = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
        | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
        | D3D12_RESOURCE_STATE_COPY_SOURCE;
    state == D3D12_RESOURCE_STATE_COPY_DEST
        || state == D3D12_RESOURCE_STATE_RENDER_TARGET && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET)
        || state.0 & !allowed.0 == 0
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

fn committed(
    device: &ID3D12Device,
    kind: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: kind,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native = None;
    // SAFETY: D3D12 only borrows the input structures during creation.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            desc,
            initial,
            clear.map(core::ptr::from_ref),
            &mut native,
        )?;
    }
    let bytes = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(desc)) }.SizeInBytes;
    use crate::diagnostics::memory::Category;
    let category = match kind {
        D3D12_HEAP_TYPE_UPLOAD => Category::PlacedUpload,
        D3D12_HEAP_TYPE_READBACK => Category::PlacedReadback,
        _ => Category::PlacedTexture,
    };
    let native: ID3D12Resource = native.ok_or(E_POINTER)?;
    crate::diagnostics::memory::live_on(&native, category, if bytes == u64::MAX { 0 } else { bytes });
    Ok(native)
}

struct Mip {
    slice: u32,
    level: u32,
    source: Range<u64>,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    elements: u32,
    element_bytes: u32,
    rows: u32,
    bytes: u64,
}

#[derive(Default)]
struct Contents {
    // A 64-bit hash of each level's last guest bytes (None: unknown) makes
    // writes to shared pages and all-zero rewrites observable. test68 kept an
    // exact copy of every level of its 33000 live textures: host private
    // commit grew from 3 to 11 GB in blocks of a few MiB.
    previous: Vec<Option<u64>>,
    uploaded: u64,
    /// The upload fence `uploaded` counts on (None: never uploaded).
    uploaded_on: Option<ID3D12Fence>,
    readers: Vec<(ID3D12Fence, u64)>,
    /// When each large level was last read back from guest memory (see
    /// `big_snapshot`).
    big_at: Vec<Option<std::time::Instant>>,
    /// A video plane: Bink's completed-decode count when it was last read.
    plane_done: Option<u64>,
}

/// test155: the 4K intro's nine format-61 planes (a three-frame ring of Y and
/// CbCr) were each read back and hashed in full every refresh, 21-27 ms of a
/// 76 ms frame on the submit thread, though only Bink writes them and only
/// when it decodes. A plane already read since Bink's last completed decode
/// (xmem TuxBoxBinkState) is unchanged and is skipped.
/// TUXBOX_PLANE_DECODE_SKIP=0 reads every plane every refresh.
fn plane_decode_skip() -> bool {
    crate::env_toggle!("TUXBOX_PLANE_DECODE_SKIP") != Some("0")
}

/// test66 froze presents for 15 s, then the title's watchdog killed it.
/// The submitting thread sat in `snapshot_mip` under `pages::claim`
/// throughout: a level is read back through ReadProcessMemory and compared
/// in full whenever the texture lost its pages to another one, and the title
/// had a 5808x5808 texture, hundreds of MiB a level, with a 1 GiB host
/// allocation appearing as private commit grew 6 GB. A level larger than
/// TUXBOX_SNAPSHOT_BIG_MB (16) is read again only when the write watch saw a
/// CPU write in it, or none was ever read, and at most every
/// TUXBOX_SNAPSHOT_BIG_MS (500); 0 MB reads every level every time. test67
/// froze the same way with no level over 64 MiB, and its write watch skipped
/// no level in any refresh, so each refresh also reads back at most
/// TUXBOX_REFRESH_READ_MB (32) of levels; the rest wait for a later one.
/// test111: the Bink video's 1920x1080 format-61 planes were refreshed as
/// "1 of 1 mips skipped, read budget spent", so the YUV->RGB draw sampled one
/// plane from the previous frame against fresh others (the datamosh). A
/// single-level R8/R8G8 plane of 256+ texels is now read every refresh,
/// past the byte and time budgets. TUXBOX_PLANE_UNBUDGETED=0 defers it again.
pub(super) fn video_plane(desc: &D3D12_RESOURCE_DESC, mips: usize) -> bool {
    mips == 1
        && matches!(desc.Format.0, 49 | 61)
        && desc.Width >= 256
        && desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && crate::env_toggle!("TUXBOX_PLANE_UNBUDGETED") != Some("0")
}

type BinkState = unsafe extern "system" fn(*mut u64) -> u32;

/// xmem's Bink hooks: (decodes in flight, decodes completed).
pub(super) fn bink_state() -> Option<(u32, u64)> {
    static F: std::sync::OnceLock<Option<BinkState>> = std::sync::OnceLock::new();
    let f = (*F.get_or_init(|| unsafe {
        use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let module = GetModuleHandleW(windows::core::w!("xmem.dll")).ok()?;
        GetProcAddress(module, windows::core::s!("TuxBoxBinkState")).map(|f| core::mem::transmute::<_, BinkState>(f))
    }))?;
    let mut done = 0;
    let busy = unsafe { f(&mut done) };
    Some((busy, done))
}

/// For the plane trace: Bink's decode state now.
pub(super) fn bink_note() -> String {
    bink_state().map_or(String::new(), |(busy, done)| format!(", Bink {} ({done} decodes done)", if busy != 0 { "decoding" } else { "idle" }))
}

/// test112: Bink macroblock residue on the intro. A video plane is not read
/// while Bink decodes (xmem TuxBoxBinkState), so a half-written frame is
/// never uploaded. The plane stays owed and is read at the next refresh;
/// a decode in flight over 250 ms no longer holds it back.
/// TUXBOX_BINK_PLANE_GATE=0 reads regardless.
fn bink_gate() -> bool {
    if crate::env_toggle!("TUXBOX_BINK_PLANE_GATE") == Some("0") {
        return false;
    }
    let Some((busy, _)) = bink_state() else { return false };
    static SINCE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let mut since = SINCE.lock().unwrap_or_else(|e| e.into_inner());
    if busy == 0 {
        *since = None;
        return false;
    }
    since.get_or_insert_with(std::time::Instant::now).elapsed() < std::time::Duration::from_millis(250)
}

/// Gap 3: each plane was gated on its own, so a decode finishing between the
/// Y and CbCr reads of one refresh uploaded planes of two frames. A refresh
/// now fixes Bink's completed-decode count at its start (begin_plane_pass);
/// once a plane is gated, or the count moves, every later plane of that
/// refresh is gated too and reads next refresh together.
/// TUXBOX_BINK_FRAME_GATE=0 gates each plane alone.
static PLANE_PASS: std::sync::Mutex<Option<(u64, bool)>> = std::sync::Mutex::new(None);

pub(super) fn begin_plane_pass() {
    let mut pass = PLANE_PASS.lock().unwrap_or_else(|e| e.into_inner());
    *pass = (crate::env_toggle!("TUXBOX_BINK_FRAME_GATE") != Some("0"))
        .then(bink_state).flatten().map(|(_, done)| (done, false));
}

fn plane_gated() -> bool {
    let alone = bink_gate();
    let mut pass = PLANE_PASS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((done, gated)) = pass.as_mut() else { return alone };
    if alone || bink_state().is_some_and(|(_, now)| now != *done) {
        *gated = true;
    }
    *gated
}

/// This refresh's whole byte budget (refresh_read_budget, scaled for a busy
/// refresh): a level is never deferred while the budget is untouched.
pub(super) static REFRESH_FULL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) fn refresh_read_budget() -> u64 {
    static MB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MB.get_or_init(|| crate::env_toggle!("TUXBOX_REFRESH_READ_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(32)) << 20
}

/// Textures whose pages another texture had claimed since their last
/// refresh (each re-reads every level): the refresh log line reports it.
/// The datamosh guard, for the periodic refresh line: video planes read
/// while xmem reported a Bink decode in flight (expected 0: a half-written
/// frame uploaded), and plane reads the Bink gate put off to a later refresh.
pub(super) static PLANE_READ_DURING_DECODE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub(super) static PLANE_GATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(super) static LOST_CLAIMS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Adds inherited readers keeping one entry per fence (the highest value) and
/// none already passed. test70 froze inside `changed` growing a 4 GB readers
/// list: two textures taking the same pages from each other each copied the
/// other's whole list every refresh, doubling both, and a kept claim never
/// uploads to clear them. TUXBOX_READERS_MERGE=0 appends as before.
fn merge_readers(readers: &mut Vec<(ID3D12Fence, u64)>, inherited: Vec<(ID3D12Fence, u64)>) {
    if crate::env_toggle!("TUXBOX_READERS_MERGE") == Some("0") {
        readers.extend(inherited);
        return;
    }
    for (fence, value) in inherited {
        match readers.iter_mut().find(|(f, _)| *f == fence) {
            Some((_, v)) => *v = (*v).max(value),
            None => readers.push((fence, value)),
        }
    }
    readers.retain(|(f, v)| unsafe { f.GetCompletedValue() } < *v);
}

/// test69 read one level of 4847 MiB back from guest memory into a scratch
/// buffer and an upload copy (two 4847 MiB host allocations) on the submit
/// thread until the title's watchdog fired. A level over
/// TUXBOX_SNAPSHOT_MAX_MB (256) is never read back: guest CPU writes to it
/// are not uploaded. 0 lifts the cap.
fn too_big(size: u64, desc: &D3D12_RESOURCE_DESC, level: usize) -> bool {
    static MB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let mb = *MB.get_or_init(|| crate::env_toggle!("TUXBOX_SNAPSHOT_MAX_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(256));
    if mb == 0 || size <= mb << 20 {
        return false;
    }
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: level {level} of a {}x{}x{} format {} dimension {} texture spans {} MiB of guest memory: never read back ({n} so far; TUXBOX_SNAPSHOT_MAX_MB=0 reads it)",
            desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Dimension.0, size >> 20));
    }
    true
}

/// A fast 64-bit hash of a level's bytes (four multiply-rotate lanes).
pub(super) fn level_hash(bytes: &[u8]) -> u64 {
    const K: [u64; 4] = [0x9e37_79b9_7f4a_7c15, 0xc2b2_ae3d_27d4_eb4f, 0x1656_67b1_9e37_79f9, 0xff51_afd7_ed55_8ccd];
    let mut lanes = [K[0] ^ bytes.len() as u64, K[1], K[2], K[3]];
    let mut chunks = bytes.chunks_exact(32);
    for chunk in &mut chunks {
        for (i, lane) in lanes.iter_mut().enumerate() {
            let w = u64::from_le_bytes(chunk[i * 8..i * 8 + 8].try_into().unwrap());
            *lane = (*lane ^ w).wrapping_mul(K[3]).rotate_left(31);
        }
    }
    let mut tail = [0u8; 32];
    let rest = chunks.remainder();
    tail[..rest.len()].copy_from_slice(rest);
    for (i, lane) in lanes.iter_mut().enumerate() {
        let w = u64::from_le_bytes(tail[i * 8..i * 8 + 8].try_into().unwrap());
        *lane = (*lane ^ w ^ rest.len() as u64).wrapping_mul(K[3]).rotate_left(31);
    }
    let mut h = lanes[0] ^ lanes[1].rotate_left(17) ^ lanes[2].rotate_left(34) ^ lanes[3].rotate_left(51);
    h ^= h >> 33;
    h = h.wrapping_mul(K[3]);
    h ^ (h >> 29)
}

fn big_snapshot(size: u64, last: Option<std::time::Instant>, unchanged: Option<bool>) -> bool {
    use std::sync::OnceLock;
    static LIMITS: OnceLock<(u64, u64)> = OnceLock::new();
    let (mb, ms) = *LIMITS.get_or_init(|| (
        crate::env_toggle!("TUXBOX_SNAPSHOT_BIG_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(16),
        crate::env_toggle!("TUXBOX_SNAPSHOT_BIG_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(500),
    ));
    if mb == 0 || size <= mb << 20 {
        return false;
    }
    let Some(last) = last else { return false };
    let skip = unchanged != Some(false) || last.elapsed() < std::time::Duration::from_millis(ms);
    if skip {
        static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "placed textures: a {} MiB level not read back again ({n} so far; TUXBOX_SNAPSHOT_BIG_MB=0 reads every level)", size >> 20));
        }
    }
    skip
}

struct Alias {
    desc: D3D12_RESOURCE_DESC,
    texture: Weak<Texture>,
}

/// Aliases by start address, and each 64 KiB page's textures (rebox
/// 7287e01, re-derived): a placement over pages older textures cover marks
/// them superseded, counted on the memory line, since on the console the
/// newer one now owns that memory. TUXBOX_PLACED_PAGE_INDEX=0 keeps no
/// page index.
#[derive(Default)]
struct Images(Mutex<BTreeMap<u64, Vec<Alias>>>, Mutex<std::collections::HashMap<u64, Vec<Weak<Texture>>>>);

fn page_index() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_PAGE_INDEX") != Some("0")
}

fn pages(range: &Range<u64>) -> core::ops::RangeInclusive<u64> {
    range.start >> 16..=(range.end.max(range.start + 1) - 1) >> 16
}

#[cfg(test)]
#[test]
fn a_range_covers_the_pages_it_touches() {
    assert_eq!(pages(&(0x10000..0x20000)), 1..=1);
    assert_eq!(pages(&(0xffff..0x10001)), 0..=1);
    assert_eq!(pages(&(0x30000..0x30000)), 3..=3);
}

/// The unmapped textures on pages that a texture going away covered and that
/// no other live mapped texture newer than them covers, newest first (xcl
/// title_textures.rs remove()).
/// test98 hung with the title's submit thread blocked in a Texture's Drop
/// inside another's: a reference upgraded here under the page index lock
/// was the last one by the time it was dropped, and its Drop takes that
/// lock again. Every reference upgraded under the lock is now kept in
/// `held` and dropped after it is released.
fn uncovered(images: &Images, gone: &Texture) -> Vec<Arc<Texture>> {
    let range = gone.range();
    let mut held: Vec<Arc<Texture>> = Vec::new();
    let index = lock(&images.1);
    let near = |r: &Range<u64>, held: &mut Vec<Arc<Texture>>| {
        let mut out: Vec<Arc<Texture>> = Vec::new();
        for page in pages(r) {
            for t in index.get(&page).into_iter().flatten().filter_map(Weak::upgrade) {
                if !core::ptr::eq(Arc::as_ptr(&t), gone) && overlaps(&t.range(), r) && !out.iter().any(|o| Arc::ptr_eq(o, &t)) {
                    out.push(t);
                } else {
                    held.push(t);
                }
            }
        }
        out
    };
    let (mut candidates, rest): (Vec<Arc<Texture>>, Vec<Arc<Texture>>) =
        near(&range, &mut held).into_iter().partition(|t| t.on_pages && !pages::is_mapped(t));
    held.extend(rest);
    candidates.sort_unstable_by_key(|t| std::cmp::Reverse(t.seq));
    let mut chosen: Vec<Arc<Texture>> = Vec::new();
    for c in candidates {
        let around = near(&c.range(), &mut held);
        let covered = around.iter().any(|t| t.seq > c.seq
            && (chosen.iter().any(|x| Arc::ptr_eq(x, t)) || (!t.on_pages || pages::is_mapped(t))));
        held.extend(around);
        if covered {
            held.push(c);
        } else {
            chosen.push(c);
        }
    }
    drop(index);
    drop(held);
    chosen
}

/// A 3D texture whose console mip chain ends in a packed tail: XG and PC
/// drivers pack volume tails differently, so it stays off pages and takes the
/// committed detile path (xcl 971c0e5 `console_has_packed_3d_mips`, GPL-2.0,
/// credit to ivall). TUXBOX_PACKED_3D_OFF_PAGES=0 places it on pages as before.
fn packed_3d_tail(desc: &D3D12_RESOURCE_DESC) -> bool {
    desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.MipLevels > 1
        && crate::env_toggle!("TUXBOX_PACKED_3D_OFF_PAGES") != Some("0")
        && color_plane(&console_layout(desc)).is_ok_and(|p| p.unpacked_mip_count() < u32::from(desc.MipLevels))
}

/// Whether `small`'s size is one of `big`'s mip levels (rebox fe7f1d4
/// `view_is_part`, GPL-2.0, credit to its authors).
fn is_part(big: &D3D12_RESOURCE_DESC, small: &D3D12_RESOURCE_DESC) -> bool {
    (0..16).any(|m| (big.Width >> m).max(1) == small.Width && (big.Height >> m).max(1) == small.Height)
}

/// A view of some of a texture's mips or slices, or the texture of part of a
/// new one: neither retires the other (rebox fe7f1d4 `view_stays`; the
/// loading screen's art was retired by its own mip-0 view and drew empty).
/// TUXBOX_PART_VIEWS_STAY=0 supersedes them as before.
fn part_view(old: &Texture, new: &Texture) -> bool {
    if crate::env_toggle!("TUXBOX_PART_VIEWS_STAY") == Some("0") {
        return false;
    }
    let (o, n) = (old.range(), new.range());
    (n.start >= o.start && n.end <= o.end && is_part(&old.desc, &new.desc))
        || (o.start >= n.start && o.end <= n.end && is_part(&new.desc, &old.desc))
}

/// Indexes a new texture by page and marks the live older textures it
/// overlaps superseded. Returns those textures: the caller drops them
/// after releasing the image locks, since a last reference runs Drop,
/// which takes both.
fn index_pages(images: &Images, texture: &Arc<Texture>) -> Vec<Arc<Texture>> {
    let range = texture.range();
    let mut older = Vec::new();
    let mut index = lock(&images.1);
    for page in pages(&range) {
        let list = index.entry(page).or_default();
        list.retain(|t| t.strong_count() != 0);
        for old in list.iter().filter_map(Weak::upgrade) {
            if overlaps(&old.range(), &range) && !part_view(&old, texture) && !old.superseded.swap(true, Ordering::Relaxed) {
                pool::superseded(old.bytes, true);
            }
            older.push(old);
        }
        list.push(Arc::downgrade(texture));
    }
    older
}

pub(crate) struct Texture {
    pub(super) native: ID3D12Resource,
    pub(super) desc: D3D12_RESOURCE_DESC,
    pub(super) address: u64,
    plane: Option<Plane>,
    size: u64,
    mips: Vec<Mip>,
    contents: Mutex<Contents>,
    images: Weak<Images>,
    /// The native copy's allocation size, for the live count and the pool.
    bytes: u64,
    /// A newer placement overlaps this one's memory.
    superseded: std::sync::atomic::AtomicBool,
    /// Placed in a span heap shared with the textures over the same guest
    /// bytes (alias.rs), rather than a committed copy of its own.
    span: Option<alias::Placed>,
    /// A reserved resource on the title's 64 KiB pages (pages.rs).
    on_pages: bool,
    /// Creation order, for remapping the newest covered texture (pages.rs).
    seq: u64,
    /// Milliseconds since `epoch()` when a submission last used it.
    last_used: AtomicU64,
    /// Unmapped by `unmap_idle`; mapped again at its next use.
    idle_unmapped: std::sync::atomic::AtomicBool,
    /// `now_ms` when it was last mapped again after an idle unmap (0: never).
    remapped_at: AtomicU64,
}

use std::sync::atomic::AtomicU64;

fn epoch() -> std::time::Instant {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    *EPOCH.get_or_init(std::time::Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// test74 ran 8356 MiB of device-local memory against a 7188 MiB budget,
/// 3852 MiB of it in page pools held by mapped textures. While over budget,
/// at most once a second, textures on pages that no submission used for
/// TUXBOX_PAGES_IDLE_S (5) are unmapped, oldest first, until the overage is
/// covered (at most 512 a sweep); their pages go back to the pools, and a
/// pool left empty is evicted as before. Such a texture is mapped again and
/// re-uploaded whole at its next use. 0 never unmaps.
fn unmap_idle(images: &Images) {
    static IDLE_S: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let idle = *IDLE_S.get_or_init(|| crate::env_toggle!("TUXBOX_PAGES_IDLE_S").and_then(|v| v.trim().parse().ok()).unwrap_or(5));
    if idle == 0 || !page_index() {
        return;
    }
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = now_ms();
    let last = LAST.load(Ordering::Relaxed);
    if now < last + 1000 || LAST.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_err() {
        return;
    }
    let over = crate::diagnostics::memory::local_over();
    if over == 0 {
        return;
    }
    let cutoff = now.saturating_sub(idle * 1000);
    static MIN_KB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    let min_bytes = *MIN_KB.get_or_init(|| crate::env_toggle!("TUXBOX_PAGES_IDLE_MIN_KB").and_then(|v| v.trim().parse().ok()).unwrap_or(1024)) << 10;
    // Upgrade under the lock, filter after it: a filtered-out texture whose
    // last reference this was runs Drop, which takes the image locks.
    let all: Vec<Arc<Texture>> = lock(&images.0).values().flatten().filter_map(|alias| alias.texture.upgrade()).collect();
    let mut cold: Vec<Arc<Texture>> = all.into_iter()
        .filter(|t| t.on_pages && !t.idle_unmapped.load(Ordering::Relaxed) && !t.superseded.load(Ordering::Relaxed)
            && t.last_used.load(Ordering::Relaxed) <= cutoff
            // test75 unmapped mostly 17x17 and 16x9 textures, mapped 2048+
            // of them again and freed nothing: only textures of 1 MiB or more
            // (TUXBOX_PAGES_IDLE_MIN_KB, 1024) count, the largest first, and
            // one mapped again in the last 60 s is left alone.
            && t.bytes >= min_bytes
            && (t.remapped_at.load(Ordering::Relaxed) == 0 || t.remapped_at.load(Ordering::Relaxed) <= now.saturating_sub(60_000)))
        .collect();
    cold.sort_unstable_by_key(|t| std::cmp::Reverse(t.bytes));
    let (mut freed, mut count) = (0u64, 0u64);
    for texture in cold.iter().take(512) {
        if freed >= over {
            break;
        }
        if pages::unmap(texture) {
            texture.idle_unmapped.store(true, Ordering::Relaxed);
            lock(&texture.contents).previous.iter_mut().for_each(|p| *p = None);
            freed += texture.bytes;
            count += 1;
        }
    }
    drop(cold);
    crate::diagnostics::memory::local_freed(freed);
    static TOTAL: AtomicU64 = AtomicU64::new(0);
    let n = TOTAL.fetch_add(count, Ordering::Relaxed) + count;
    if count != 0 {
        static LINES: AtomicU64 = AtomicU64::new(0);
        let line = LINES.fetch_add(1, Ordering::Relaxed) + 1;
        if line <= 8 || line.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "placed textures: device-local over budget by {} MiB: unmapped {count} textures idle {idle} s or more (about {} MiB; {n} so far; TUXBOX_PAGES_IDLE_S=0 off)",
                over >> 20, freed >> 20));
        }
    }
}

/// Maps idle-unmapped textures of a submission again before it runs.
fn remap_used(textures: &[Arc<Texture>]) {
    let now = now_ms();
    static REMAPPED: AtomicU64 = AtomicU64::new(0);
    for texture in textures.iter().filter(|t| t.on_pages) {
        texture.last_used.store(now, Ordering::Relaxed);
        if texture.idle_unmapped.swap(false, Ordering::Relaxed) && pages::remap(texture) {
            texture.remapped_at.store(now, Ordering::Relaxed);
            lock(&texture.contents).previous.iter_mut().for_each(|p| *p = None);
            let n = REMAPPED.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "placed textures: an idle-unmapped {}x{} texture mapped again for a submission ({n} so far)", texture.desc.Width, texture.desc.Height));
            }
        }
    }
}

static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Drop for Texture {
    fn drop(&mut self) {
        crate::exports::profiling::mapping_released();
        let readers = std::mem::take(&mut self.contents.get_mut().unwrap_or_else(|e| e.into_inner()).readers);
        match &self.span {
            Some(placed) => {
                pool::forget(self.bytes);
                alias::gone(placed);
            }
            None if self.on_pages => {
                pool::forget(self.bytes);
                pages::gone(self, readers);
            }
            None => pool::release(&self.native, host_desc(&self.desc), self.bytes, readers),
        }
        if *self.superseded.get_mut() {
            pool::superseded(self.bytes, false);
        }
        let Some(images) = self.images.upgrade() else {
            return;
        };
        if let Entry::Occupied(mut entry) = lock(&images.0).entry(self.address) {
            entry
                .get_mut()
                .retain(|alias| alias.texture.strong_count() != 0);
            if entry.get().is_empty() {
                entry.remove();
            }
        }
        let uncovered = if self.on_pages && page_index() { uncovered(&images, self) } else { Vec::new() };
        for older in &uncovered {
            // xcl remove(): the newest covered older texture is mapped again
            // and re-uploaded whole.
            if pages::remap(older) {
                lock(&older.contents).previous.iter_mut().for_each(|p| *p = None);
                if older.superseded.swap(false, Ordering::Relaxed) {
                    pool::superseded(older.bytes, false);
                }
            }
        }
        drop(uncovered);
        if page_index() {
            let mut index = lock(&images.1);
            for page in pages(&self.range()) {
                if let std::collections::hash_map::Entry::Occupied(mut list) = index.entry(page) {
                    list.get_mut().retain(|t| t.strong_count() != 0);
                    if list.get().is_empty() {
                        list.remove();
                    }
                }
            }
        }
    }
}

// XG's linear NV12 surface contains full-height luma followed by half-height
// interleaved chroma. Both use the same 256-byte-aligned row pitch.
pub(super) fn nv12_pitch(desc: &D3D12_RESOURCE_DESC) -> Option<u64> {
    (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12
        && desc.Layout.0 == 0x100 && desc.Flags.0 == 0x300000
        && desc.Width > 0 && desc.Width <= 16384 && desc.Width.is_multiple_of(2)
        && desc.Height > 0 && desc.Height <= 16384 && desc.Height.is_multiple_of(2)
        && desc.DepthOrArraySize == 1 && desc.MipLevels == 1
        && desc.SampleDesc.Count == 1 && desc.SampleDesc.Quality == 0)
        .then(|| desc.Width.next_multiple_of(256))
}

/// The console layout of a texture description: the default (unknown)
/// layout is XG's 4 KiB pick and row major is linear, so a placed texture
/// reads title memory in the layout its console size assumed
/// (TUXBOX_CONSOLE_SIZES=0 keeps the description as given).
pub(super) fn console_layout(desc: &D3D12_RESOURCE_DESC) -> D3D12_RESOURCE_DESC {
    if crate::env_toggle!("TUXBOX_CONSOLE_SIZES") == Some("0") || desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        return *desc;
    }
    match desc.Layout.0 {
        // Layout 0 is the layout library's pick by kind: 1D linear, depth
        // 64KB_Z_X, render target 64KB_R_X, else 4KB_S (VIctory
        // victory-gpu resource.rs:150-157 `texture_layout`, GPL-3.0, credit
        // to its authors). TUXBOX_LAYOUT0_BY_KIND=0 maps it to 4KB_S always.
        0 if crate::env_toggle!("TUXBOX_LAYOUT0_BY_KIND") == Some("0") => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x105), ..*desc },
        0 if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE1D => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x100), ..*desc },
        0 if desc.Flags.0 & 2 != 0 => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x118), ..*desc },
        0 if desc.Flags.0 & 1 != 0 => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x11b), ..*desc },
        0 => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x105), ..*desc },
        1 => D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT(0x100), ..*desc },
        _ => *desc,
    }
}

pub(super) fn color_plane(desc: &D3D12_RESOURCE_DESC) -> Result<Plane> {
    let flags = if (desc.Layout.0 == 0x105 && desc.Flags.0 & !0x300004 == 0)
        || (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x105
            && desc.Flags.0 & !0x300005 == 0 && !matches!(desc.Format.0, 70..=84 | 94..=99))
    {
        0
    } else {
        desc.Flags.0 as u32
    };
    let layout = Desc {
            dimension: desc.Dimension.0 as _,
            alignment: desc.Alignment,
            width: desc.Width,
            height: desc.Height,
            slices: desc.DepthOrArraySize,
            mips: desc.MipLevels,
            format: desc.Format.0 as _,
            samples: desc.SampleDesc.Count,
            quality: desc.SampleDesc.Quality,
            layout: desc.Layout.0 as _,
            flags,
        };
    if matches!(layout.layout, 257 | 258) {
        return Plane::compute_microtile(&layout).map_err(|status| HRESULT(status).into());
    }
    if matches!(layout.layout, 265 | 266) {
        // 64 KiB standard (0x109) or display (0x10a, SW_64KB_D, mips allowed)
        // swizzle, as selected by XGCreateTextureComputer.
        return Plane::compute_sparse_64k(&layout).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE1D && desc.Layout.0 == 0x100
        && desc.Height == 1 && matches!(desc.Alignment, 0 | 256 | 4096 | 65536) {
        let linear = Desc { dimension: 3, alignment: 0, ..layout };
        return Plane::compute(&linear).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x100
        && desc.Alignment == 0 && desc.Flags.0 & !0x300004 == 0 && !matches!(desc.Format.0, 70..=84 | 94..=99)
        && desc.Width <= 2048 && desc.Height <= 2048 {
        // Same selection as XGCreateTextureComputer for a linear volume.
        let volume = Desc { dimension: 3, alignment: 0, flags: 0, ..layout };
        return Plane::compute_linear_volume(&volume).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x105
        && matches!(desc.Alignment, 0 | 4096) && desc.Width <= 2048 && desc.Height <= 2048 {
        // The XG volume tile calculator takes a 2D base plane and its physical depth.
        let volume = Desc { dimension: 3, alignment: 0, ..layout };
        return Plane::compute_4k_volume(&volume).map_err(|status| HRESULT(status).into());
    }
    Plane::compute(&layout).or_else(|_| Plane::compute_64k_color(&layout))
        .or_else(|status| mode0_64k(desc, &layout).ok_or(status))
        .map_err(|status| HRESULT(status).into())
}

/// A 2D layout-0x100 texture the linear model refuses is sized as the
/// console's 64 KiB chain (standard 64 KiB swizzle, shared mip-tail tile):
/// the game's desc builder writes 0x100 for its tile-mode byte 0, which its
/// texture metadata reads as 64 KB tiles (ScarlettCompat b92f060, GPL-3.0,
/// credit: a 2048x2048 A8 0x100 texture refused, slot 25 returned UINT64_MAX
/// and the title asserted). Only after every other model refuses, so the
/// linear sizes XGCreateTextureComputer gives stay. TUXBOX_MODE0_64K=0 refuses.
fn mode0_64k(desc: &D3D12_RESOURCE_DESC, layout: &Desc) -> Option<Plane> {
    if desc.Layout.0 != 0x100 || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || crate::env_toggle!("TUXBOX_MODE0_64K") == Some("0") {
        return None;
    }
    let chain = Desc { layout: 265, alignment: 0, flags: layout.flags & 0x0030_0004, ..*layout };
    let plane = Plane::compute_sparse_64k(&chain).ok()?;
    static LOGGED: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&LOGGED, || format!(
        "layout 0x100 {}x{} mips={} format={} flags={:#x}: sized as the 64 KiB chain, {} bytes (ScarlettCompat b92f060; TUXBOX_MODE0_64K=0 refuses)",
        desc.Width, desc.Height, desc.MipLevels, desc.Format.0, desc.Flags.0, plane.size));
    Some(plane)
}

/// The desktop description of a placed texture's committed copy. A
/// simultaneous-access texture implicitly promotes on first use and decays
/// to COMMON after ExecuteCommandLists, including copy-queue uploads.
fn host_desc(desc: &D3D12_RESOURCE_DESC) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Alignment: 0,
        Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS
            | (desc.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)),
        ..*desc
    }
}

impl Texture {
    /// Width and height of the top level (display-sized draws).
    pub(crate) fn extent(&self) -> (u64, u32) {
        (self.desc.Width, self.desc.Height)
    }

    /// Guest address of a subresource's texels. On the console Map returns this
    /// memory directly; the uploader publishes CPU writes before GPU reads.
    pub(crate) fn guest_pointer(&self, subresource: u32) -> Option<u64> {
        let mip = self.mips.get(subresource as usize)?;
        self.address.checked_add(mip.source.start)
    }

    pub(crate) fn flags(&self) -> D3D12_RESOURCE_FLAGS {
        self.desc.Flags
    }

    fn new(
        device: &ID3D12Device,
        images: &Arc<Images>,
        address: u64,
        desc: &D3D12_RESOURCE_DESC,
        clear: Option<&D3D12_CLEAR_VALUE>,
    ) -> Result<Arc<Self>> {
        let pitch = nv12_pitch(desc);
        let plane = if pitch.is_some() { None } else { Some(color_plane(&console_layout(desc))?) };
        let luma_bytes = pitch.unwrap_or_default() * u64::from(desc.Height);
        let volume = desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D;
        let (size, alignment, count) = plane.as_ref().map_or(
            (luma_bytes + luma_bytes / 2, 256, 2),
            |p| (p.size, p.alignment, if volume { p.mip_count() } else { p.mip_count() * p.array_size() }));
        if address == 0
            || !address.is_multiple_of(alignment)
            || !crate::exports::placement_fits(address, size)
        {
            static RANGE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&RANGE, || format!(
                "placed texture range at {address:#x}: size={size:#x} alignment={alignment:#x} fits={} {}x{} format={} layout={:#x}",
                crate::exports::placement_fits(address, size), desc.Width, desc.Height, desc.Format.0, desc.Layout.0));
            return Err(E_INVALIDARG.into());
        }
        crate::diagnostics::watch("placed texture", address, size, || format!(
            "{}x{}x{} format={} layout={:#x} flags={:#x} mips={}", desc.Width, desc.Height,
            desc.DepthOrArraySize, desc.Format.0, desc.Layout.0, desc.Flags.0, desc.MipLevels));
        let host = host_desc(desc);
        // A copy a dropped placement left with this description is reused
        // (test48: 38k committed copies in two minutes); its old readers are
        // waited on before the first upload overwrites it.
        // Span heaps first (alias.rs): the textures over the same guest bytes
        // share heap bytes, as on the console.
        let paged = if packed_3d_tail(desc) { None } else { pages::place(device, address, &host, size, clear) };
        let on_pages = paged.is_some();
        // A texture that fails below gives its pages back.
        struct Unplace(Option<usize>);
        impl Drop for Unplace {
            fn drop(&mut self) {
                if let Some(raw) = self.0 {
                    pages::forget(raw);
                }
            }
        }
        let mut unplace = Unplace(paged.as_ref().map(|(n, _, _)| windows::core::Interface::as_raw(n) as usize));
        let (native, bytes, readers, span) = match paged.map(|(n, b, r)| (n, b, r, None)).ok_or(()).or_else(|()| alias::place(device, address, &host, size, clear).map(|(n, b, p)| (n, b, Vec::new(), Some(p))).ok_or(())) {
            Ok(found) => found,
            Err(()) => match pool::take(device, &host) {
            Some((native, bytes, readers)) => (native, bytes, readers, None),
            None => {
                let native = committed(device, D3D12_HEAP_TYPE_DEFAULT, &host, D3D12_RESOURCE_STATE_COMMON, clear)?;
                let bytes = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(&host)) }.SizeInBytes;
                (native, if bytes == u64::MAX { 0 } else { bytes }, Vec::new(), None)
            }
            },
        };
        pool::created(bytes);
        let mut mips = Vec::with_capacity(count as usize);
        for index in 0..count {
            let (slice, level, element_bytes, source) = if let Some(p) = &plane {
                let slice = index / p.mip_count();
                let level = index % p.mip_count();
                (slice, level, p.element_bytes(), if volume { 0..p.size } else { p.mip_range_in_slice(slice, level).ok_or(E_INVALIDARG)? })
            } else if index == 0 {
                (0, 0, 1, 0..luma_bytes)
            } else {
                (0, 0, 2, luma_bytes..size)
            };
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let (mut rows, mut row_bytes, mut bytes) = (0, 0, 0);
            unsafe {
                device.GetCopyableFootprints(
                    &host,
                    index,
                    1,
                    0,
                    Some(&mut footprint),
                    Some(&mut rows),
                    Some(&mut row_bytes),
                    Some(&mut bytes),
                );
            }
            let elements = u32::try_from(row_bytes / u64::from(element_bytes))
                .map_err(|_| E_INVALIDARG)?;
            if rows == 0
                || elements == 0
                || !row_bytes.is_multiple_of(u64::from(element_bytes))
                || bytes == u64::MAX
                || !volume && plane.as_ref().is_some_and(|p| p.element_offset(level, u64::from(elements - 1), rows - 1).is_none())
                || pitch.is_some_and(|p| row_bytes > p || p * u64::from(rows) != source.end - source.start)
            {
                return Err(E_NOTIMPL.into());
            }
            mips.push(Mip {
                slice,
                level,
                source,
                element_bytes,
                footprint,
                elements,
                rows,
                bytes,
            });
        }
        let contents = Mutex::new(Contents {
            previous: vec![None; mips.len()],
            readers,
            ..Default::default()
        });
        Ok(Arc::new(Self {
            native,
            desc: *desc,
            address,
            plane,
            size,
            mips,
            contents,
            images: Arc::downgrade(images),
            bytes,
            superseded: Default::default(),
            span,
            on_pages,
            seq: SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            last_used: AtomicU64::new(now_ms()),
            idle_unmapped: Default::default(),
            remapped_at: AtomicU64::new(0),
        }))
        .inspect(|_| unplace.0 = None)
    }

    fn snapshot_mip(&self, mip: &Mip, output: &mut Vec<u8>) -> Result<bool> {
        let size = (mip.source.end - mip.source.start) as usize;
        output.clear();
        output.try_reserve_exact(size).map_err(|_| E_OUTOFMEMORY)?;
        let mut copied = 0;
        // SAFETY: the OS checks the guest mapping. A partial transfer never
        // exposes uninitialized bytes, including when pages are decommitted.
        let result = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                (self.address + mip.source.start) as *const _,
                output.as_mut_ptr().cast(),
                size,
                Some(&mut copied),
            )
        };
        if result.is_ok() && copied == size {
            unsafe { output.set_len(size) };
            return Ok(true);
        }
        // A mip spanning a decommitted page would otherwise never upload and
        // stay black. Read it page by page and zero-fill what is unmapped
        // (xcl's page-split copy). Skip it only if no page is readable.
        output.resize(size, 0);
        const PAGE: u64 = 0x1000;
        let base = self.address + mip.source.start;
        let mut readable = false;
        let mut offset = 0usize;
        while offset < size {
            let address = base + offset as u64;
            let chunk = ((PAGE - address % PAGE) as usize).min(size - offset);
            let mut got = 0;
            // SAFETY: as above; the destination slice is initialized and in bounds.
            let ok = unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    address as *const _,
                    output[offset..].as_mut_ptr().cast(),
                    chunk,
                    Some(&mut got),
                )
            };
            if ok.is_ok() && got == chunk {
                readable = true;
            } else {
                output[offset..offset + chunk].fill(0);
            }
            offset += chunk;
        }
        if !readable {
            output.clear();
        }
        Ok(readable)
    }

    fn changed(
        self: &Arc<Self>,
        changes: &mut Vec<upload::Change>,
        scratch: &mut Vec<u8>,
        watch: Option<&watch::Watch>,
        budget: &mut u64,
        spare: &mut Vec<Vec<u8>>,
    ) -> Result<usize> {
        // Another texture over the same span bytes wrote them since this one
        // did: wait for its readers and upload every level again.
        let previous = if self.on_pages { pages::claim(self) } else { self.span.as_ref().and_then(|placed| alias::claim(self, placed)) };
        let mut inherited = Vec::new();
        for other in previous.iter().flatten() {
            inherited.extend(lock(&other.contents).readers.iter().cloned());
        }
        // A texture of the same description at the same address (the title
        // recreating it) lays the shared bytes out identically: nothing to
        // upload again. test68 re-read thousands of levels a refresh as such
        // pairs took the pages from each other. TUXBOX_CLAIM_SAME_KEEP=0 always
        // re-reads.
        let same = |other: &Arc<Texture>| {
            let (a, b) = (&self.desc, &other.desc);
            other.address == self.address && a.Dimension == b.Dimension && a.Width == b.Width && a.Height == b.Height
                && a.DepthOrArraySize == b.DepthOrArraySize && a.MipLevels == b.MipLevels && a.Format == b.Format
                && a.Layout == b.Layout && a.SampleDesc.Count == b.SampleDesc.Count
        };
        let keep = crate::env_toggle!("TUXBOX_CLAIM_SAME_KEEP") != Some("0");
        let lost = previous.as_ref().is_some_and(|others| !keep || others.is_empty() || !others.iter().all(same));
        drop(previous);
        let mut contents = lock(&self.contents);
        if lost {
            LOST_CLAIMS.fetch_add(1, Ordering::Relaxed);
            contents.previous.iter_mut().for_each(|p| *p = None);
        }
        // A later upload over the shared pages still waits for their readers.
        merge_readers(&mut contents.readers, inherited);
        let mut skipped = 0;
        if contents.big_at.len() < self.mips.len() {
            contents.big_at.resize(self.mips.len(), None);
        }
        for (level, mip) in self.mips.iter().enumerate() {
            let range = self.address + mip.source.start..self.address + mip.source.end;
            // No CPU write since the last compare: the snapshot would match.
            if contents.previous[level].is_some() && watch.is_some_and(|w| w.unchanged(range.clone())) {
                skipped += 1;
                continue;
            }
            let plane = video_plane(&self.desc, self.mips.len());
            // test162: xmem's count stayed 0 through the whole intro (its
            // decodes were not counted), so the planes read before the first
            // frame were kept and the intro was black. Only a count that has
            // moved is trusted; at 0 every refresh reads, as before.
            let decodes = if plane && plane_decode_skip() { bink_state().map(|(_, done)| done).filter(|&d| d != 0) } else { None };
            if decodes.is_some() && contents.previous[level].is_some() && contents.plane_done == decodes {
                skipped += 1;
                continue;
            }
            if plane && plane_gated() {
                let n = PLANE_GATED.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 8 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: video plane at {:#x} not read while Bink decodes, read next refresh ({n} so far; TUXBOX_BINK_PLANE_GATE=0 reads it)",
                        self.address));
                }
                contents.previous[level] = None;
                skipped += 1;
                continue;
            }
            let size = mip.source.end - mip.source.start;
            if too_big(size, &self.desc, level) {
                skipped += 1;
                continue;
            }
            if big_snapshot(size, contents.big_at[level], watch.map(|w| w.unchanged(range))) {
                skipped += 1;
                continue;
            }
            // The first level of a refresh always reads (progress); later ones
            // stop at the budget (0: unlimited) and are read next time.
            let limit = refresh_read_budget();
            if limit != 0 && *budget != REFRESH_FULL.load(Ordering::Relaxed).max(limit) && size > *budget && !video_plane(&self.desc, self.mips.len()) {
                static DEFERRED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = DEFERRED.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: a {} KiB level deferred to a later refresh, over its read-back budget ({n} so far; TUXBOX_REFRESH_READ_MB=0 reads all)", size >> 10));
                }
                // Spent: the rest of this refresh defers, and the next one
                // starts at this texture (upload.rs refresh).
                *budget = 0;
                // The write watch was reset this refresh: without a snapshot
                // the level is read next time even if no page is written again
                // (it stayed stale until its next write; TUXBOX_DEFER_OWED=0).
                if crate::env_toggle!("TUXBOX_DEFER_OWED") != Some("0") {
                    contents.previous[level] = None;
                }
                skipped += 1;
                continue;
            }
            if plane && bink_state().is_some_and(|(busy, _)| busy != 0) {
                let n = PLANE_READ_DURING_DECODE.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: video plane at {:#x} read while Bink decodes ({n} so far; expected 0)", self.address));
                }
            }
            *budget = budget.saturating_sub(size);
            if size > 1 << 20 {
                contents.big_at[level] = Some(std::time::Instant::now());
            }
            if !self.snapshot_mip(mip, scratch)? {
                contents.previous[level] = None;
                continue;
            }
            if decodes.is_some() {
                contents.plane_done = decodes;
            }
            let hash = level_hash(scratch);
            if contents.previous[level] != Some(hash) {
                changes.push(upload::Change {
                    texture: self.clone(),
                    level,
                    bytes: std::mem::replace(scratch, spare.pop().unwrap_or_default()),
                    hash,
                });
            }
        }
        Ok(skipped)
    }

    fn watched_range(&self) -> Range<u64> {
        let start = self.mips.iter().map(|m| m.source.start).min().unwrap_or(0);
        let end = self.mips.iter().map(|m| m.source.end).max().unwrap_or(0);
        self.address + start..self.address + end
    }
}

#[derive(Default)]
pub(super) struct Placements {
    pub(super) views: Arc<DescriptorMap<PlacedView>>,
    pub(super) targets: Arc<Views<Texture>>,
    /// SRV / UAV handle -> render heap of its image (heap::residency)
    pub(super) render_views: Arc<Views<super::heap::RenderHeap>>,
    /// RTV / DSV handle -> render heap of its image
    pub(super) render_targets: Arc<Views<super::heap::RenderHeap>>,
    memory: Arc<PlacementMemory>,
}

/// The render heaps a recording's RTVs and DSVs live in, touched again at
/// each submit (retouch): a list recorded once, or long before its submit,
/// stamped them only while recording, so they looked idle while still in
/// use. That is what the hold by list age guarded, and the hold kept every
/// idle heap resident (test154/155: 572 render heaps idle 30 s+, none
/// evicted, device-local 7739/6969 MiB). With the retouch the hold is
/// dropped; TUXBOX_TARGET_RETOUCH=0 keeps the hold by list age. Held weakly:
/// a list recorded once and never reset would otherwise keep each heap it
/// named alive (and its driver commit) after the title released the images.
#[derive(Default)]
pub(crate) struct TargetHeaps(Vec<(std::sync::Weak<super::heap::RenderHeap>, bool)>);

impl TargetHeaps {
    /// The recording ends (Reset): each heap it bound a target on counts as
    /// cleared there or not (RenderHeap::cleared, the residency line).
    pub(crate) fn clear(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        for (heap, cleared) in &self.0 {
            if let Some(heap) = heap.upgrade() {
                if *cleared { heap.cleared.fetch_add(1, Relaxed); } else { heap.uncleared.fetch_add(1, Relaxed); }
            }
        }
        self.0.clear();
    }

    fn note(&mut self, heap: Arc<super::heap::RenderHeap>, cleared: bool) {
        heap.touch();
        let weak = Arc::downgrade(&heap);
        if let Some(entry) = self.0.iter_mut().find(|(h, _)| h.ptr_eq(&weak)) {
            entry.1 |= cleared;
        } else if self.0.try_reserve(1).is_ok() {
            self.0.push((weak, cleared));
        }
    }

    /// The submit of a list that recorded these targets: each heap is used now.
    pub(crate) fn retouch(&self) {
        for heap in self.0.iter().filter_map(|(h, _)| h.upgrade()) {
            heap.touch();
        }
    }
}

pub(crate) fn target_retouch() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_TARGET_RETOUCH") != Some("0"))
}

fn note_target(device: &Device, handle: usize, heaps: &mut TargetHeaps) {
    if super::heap::residency::tracking() && let Some(heap) = device.state.placements.render_targets.get(handle) {
        heaps.note(heap, false);
    }
}

/// A clear of the RTV or DSV at `handle` was recorded.
pub(crate) unsafe fn note_clear(device: *mut c_void, handle: usize, heaps: &mut TargetHeaps) {
    let device = unsafe { &*device.cast::<Device>() };
    if super::heap::residency::tracking() && let Some(heap) = device.state.placements.render_targets.get(handle) {
        heaps.note(heap, true);
    }
}

pub(crate) unsafe fn render_target(device: *mut c_void, handle: usize, heaps: &mut TargetHeaps) -> Option<Arc<Texture>> {
    let device = unsafe { &*device.cast::<Device>() };
    note_target(device, handle, heaps);
    device.state.placements.targets.get(handle)
}

/// A bound or cleared DSV (or RTV) handle: its render heap is used.
pub(crate) unsafe fn touch_target(device: *mut c_void, handle: usize, heaps: &mut TargetHeaps) {
    let device = unsafe { &*device.cast::<Device>() };
    note_target(device, handle, heaps);
}

pub(super) fn evict_after() -> f64 {
    alias::evict_after()
}

/// Descriptor ranges submitted since the last walk of their render views.
static PENDING_VIEWS: Mutex<Vec<Range<usize>>> = Mutex::new(Vec::new());

/// A submission's descriptor ranges use their render views' heaps. While
/// no heap is evicted they are queued and stamped once a second
/// (`flush_render_views`): the per-submit walk under the views mutex took
/// test97 from 35-40 fps to 15. The queue push comes first, so an eviction
/// either sees it in its flush or is seen here. TUXBOX_RENDER_HEAP_TOUCH=submit
/// walks at every submit, as before.
fn touch_render_views(placements: &Placements, heaps: &[Range<usize>]) {
    let every = crate::env_toggle!("TUXBOX_RENDER_HEAP_TOUCH") == Some("submit");
    if !every {
        let mut pending = PENDING_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
        for range in heaps {
            if !pending.contains(range) {
                if pending.len() >= 4096 {
                    drop(pending);
                    flush_render_views(placements, false);
                    pending = PENDING_VIEWS.lock().unwrap_or_else(|e| e.into_inner());
                }
                pending.push(range.clone());
            }
        }
    }
    if every || super::heap::residency::any_evicted() {
        placements.render_views.each_in(heaps, |heap| heap.touch());
    }
}

/// Stamps the render heaps of every queued range. `stamp_only` inside the
/// evictor (which holds the residency lock): a heap it just flagged is
/// only stamped, and its recheck keeps it resident.
fn flush_render_views(placements: &Placements, stamp_only: bool) {
    let ranges = std::mem::take(&mut *PENDING_VIEWS.lock().unwrap_or_else(|e| e.into_inner()));
    if !ranges.is_empty() {
        placements.render_views.each_in(&ranges, |heap| if stamp_only { heap.stamp() } else { heap.touch() });
    }
}

pub(crate) fn list_submitted(age: std::time::Duration) {
    super::heap::residency::list_submitted(age);
}

pub(crate) unsafe fn render_targets(
    device: *mut c_void, count: u32, handles: *const D3D12_CPU_DESCRIPTOR_HANDLE, contiguous: bool,
    output: &mut Vec<Arc<Texture>>, heaps: &mut TargetHeaps,
) -> Result<()> {
    let device = unsafe { &*device.cast::<Device>() };
    output.clear();
    let stride = if contiguous && count > 1 {
        (unsafe { device.native.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) }) as usize
    } else { 0 };
    for i in 0..count as usize {
        let handle = if contiguous {
            unsafe { (*handles).ptr }.checked_add(i * stride).ok_or(E_INVALIDARG)?
        } else {
            unsafe { (*handles.add(i)).ptr }
        };
        note_target(device, handle, heaps);
        if let Some(texture) = device.state.placements.targets.get(handle) {
            output.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
            output.push(texture);
        }
    }
    Ok(())
}

#[derive(Default)]
struct PlacementMemory {
    device: OnceLock<ID3D12Device>,
    // Equal descriptors at the same VA name the same image, including writes
    // made before the title creates a second resource proxy for it.
    images: Arc<Images>,
    submissions: Mutex<Option<Submissions>>,
    failed: Arc<AtomicI32>,
}

// The Xbox export is process-wide. Keep only weak references so that a cache
// flush can visit each live device without extending its lifetime.
static FLUSH_DEVICES: Mutex<Vec<Weak<PlacementMemory>>> = Mutex::new(Vec::new());

/// Pending GPU writes each submit scans: logged each time the most seen doubles.
fn pending_writes_note(n: usize) {
    static MOST: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(32);
    if n >= MOST.load(Ordering::Relaxed) * 2 {
        MOST.store(n, Ordering::Relaxed);
        crate::diagnostics::log_line(format_args!("placed textures: {n} unpublished GPU writes scanned per submit"));
    }
}

static FLUSHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static FLUSH_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// D3DFlushEntireCpuCache refreshes every live placed texture under the
/// submit lock: its calls and average cost are logged at 1, 10, 100, ...
pub(crate) fn flush_cpu_writes() {
    let started = std::time::Instant::now();
    let _site = crate::diagnostics::cost::site();
    flush_cpu_writes_inner();
    let spent = started.elapsed().as_nanos() as u64;
    let n = FLUSHES.fetch_add(1, Ordering::Relaxed) + 1;
    let ns = FLUSH_NS.fetch_add(spent, Ordering::Relaxed) + spent;
    if matches!(n, 1 | 10 | 100 | 1000 | 10000 | 100000) {
        crate::diagnostics::log_line(format_args!(
            "D3DFlushEntireCpuCache: {n} calls, {:.1} ms on average (refreshes every live placed texture under the submit lock)",
            ns as f64 / n as f64 / 1e6));
    }
}

fn flush_cpu_writes_inner() {
    unsafe { windows::Win32::System::Threading::FlushProcessWriteBuffers() };
    let live: Vec<_> = {
        let mut registry = lock(&FLUSH_DEVICES);
        registry.retain(|device| device.strong_count() != 0);
        registry.iter().filter_map(Weak::upgrade).collect()
    };
    for memory in live {
        if let Err(error) = memory.flush() {
            crate::diagnostics::recording_error(format_args!(
                "placed texture cache flush: {error}"
            ));
        }
    }
}

impl PlacementMemory {
    fn flush(&self) -> Result<()> {
        let textures: Vec<_> = lock(&self.images.0)
            .values()
            .flatten()
            .filter_map(|alias| alias.texture.upgrade())
            .collect();
        self.flush_textures(textures)
    }

    fn flush_textures(&self, mut textures: Vec<Arc<Texture>>) -> Result<()> {
        let Some(device) = self.device.get() else {
            return Ok(());
        };
        let mut slot = lock(&self.submissions);
        HRESULT(self.failed.load(Ordering::Acquire)).ok()?;
        let submissions = match slot.as_mut() {
            Some(submissions) => submissions,
            None if textures.is_empty() => return Ok(()),
            None => slot.insert(Submissions {
                transfer: upload::Transfer::new(device)?,
                per_queue: Default::default(),
                writes: Vec::new(),
            }),
        };
        // The CPU shadow of any pending GPU write is stale until publication.
        // This includes other layouts that alias the same guest pages.
        textures.retain(|texture| {
            !submissions.writes.iter().any(|write| {
                overlaps(&texture.range(), &write.range)
                    && unsafe { write.published.GetCompletedValue() } < write.tick
            })
        });
        submissions.transfer.refresh(device, &textures, None)
    }
}

/// VIctory 13bbd6f (Marvisak, GPL-3.0) gives one host texture to each texture
/// memory: title textures at one address with the same shape and element size
/// share it, each viewing it in its own format. Here each format gets its own
/// native copy, kept in step only through title memory (GPU write-back, then a
/// re-read). This counts how often that happens before porting the sharing:
/// a live texture at the same address with the same dimension, size, array,
/// mips, samples and layout but another format or flags. The first 16 are
/// logged with both formats. TUXBOX_SAME_MEMORY_WATCH=0 off.
fn same_memory_other_format(images: &BTreeMap<u64, Vec<Alias>>, address: u64, desc: &D3D12_RESOURCE_DESC) {
    static FOUND: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    if crate::env_toggle!("TUXBOX_SAME_MEMORY_WATCH") == Some("0") {
        return;
    }
    let Some(alias) = images.get(&address).and_then(|list| list.iter().find(|a| {
        let d = &a.desc;
        d.Dimension == desc.Dimension && d.Width == desc.Width && d.Height == desc.Height
            && d.DepthOrArraySize == desc.DepthOrArraySize && d.MipLevels == desc.MipLevels
            && d.SampleDesc.Count == desc.SampleDesc.Count && d.Layout == desc.Layout
            && a.texture.strong_count() != 0
    })) else { return };
    let n = FOUND.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() {
        let live = alias.texture.upgrade();
        crate::diagnostics::log_line(format_args!(
            "placed textures: {address:#x} {}x{}x{} mips {} placed again as format {} flags {:#x} over a live one of format {} flags {:#x} ({} MiB, superseded already: {}); each gets its own native copy ({n} so far; VIctory 13bbd6f shares one; TUXBOX_SAME_MEMORY_WATCH=0 off)",
            desc.Width, desc.Height, desc.DepthOrArraySize, desc.MipLevels, desc.Format.0, desc.Flags.0,
            alias.desc.Format.0, alias.desc.Flags.0, live.as_ref().map_or(0, |t| t.bytes >> 20),
            live.as_ref().is_some_and(|t| t.superseded.load(Ordering::Relaxed))));
    }
}

/// Gap 6: the title packs light as R9G9B9E5 into R32_UINT render targets
/// (compute cannot write R9G9B9E5) and samples them through R9G9B9E5 views
/// placed at the same address: the sky, the light probes, bounced light. Each
/// format got its own native texture, and the R9G9B9E5 one, filled from title
/// memory nothing writes, read black. An R9G9B9E5 texture placed over a live
/// R32_UINT target of the same shape is now that target, which is created
/// castable to R9G9B9E5 (allocation.rs packed_uav_formats). VIctory-fxceless
/// resource.rs:1187-1237 alias_of (GPL-3.0, credit to its authors).
/// TUXBOX_PACKED_ALIAS=0 gives it its own texture again.
fn packed_target(images: &BTreeMap<u64, Vec<Alias>>, address: u64, desc: &D3D12_RESOURCE_DESC) -> Option<Arc<Texture>> {
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R9G9B9E5_SHAREDEXP};
    if desc.Format != DXGI_FORMAT_R9G9B9E5_SHAREDEXP || desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
        || desc.SampleDesc.Count > 1 || crate::env_toggle!("TUXBOX_PACKED_ALIAS") == Some("0") {
        return None;
    }
    let found = images.get(&address)?.iter().filter(|a| {
        let d = &a.desc;
        d.Format == DXGI_FORMAT_R32_UINT && d.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET)
            && d.Dimension == desc.Dimension && d.Width == desc.Width && d.Height == desc.Height
            && d.DepthOrArraySize == desc.DepthOrArraySize && d.MipLevels == desc.MipLevels
            && d.SampleDesc.Count <= 1 && d.Layout == desc.Layout
    }).find_map(|a| a.texture.upgrade())?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: R9G9B9E5 {}x{} at {address:#x} reads the R32_UINT target there ({n} so far; TUXBOX_PACKED_ALIAS=0 makes its own)",
            desc.Width, desc.Height));
    }
    Some(found)
}

impl Placements {
    pub(super) fn finish_cpu_write(&self, texture: &Arc<Texture>) -> Result<()> {
        self.memory.flush_textures(vec![texture.clone()])
    }

    fn find_texture(images: &BTreeMap<u64, Vec<Alias>>, address: u64, desc: &D3D12_RESOURCE_DESC) -> Option<Arc<Texture>> {
        images.get(&address)?.iter().filter(|alias| alias.desc == *desc)
            .find_map(|alias| alias.texture.upgrade())
    }

    pub(super) fn texture(
        &self,
        device: &ID3D12Device,
        address: u64,
        desc: &D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES,
        clear: Option<&D3D12_CLEAR_VALUE>,
    ) -> Result<Arc<Texture>> {
        if !supported_state(initial, desc.Flags) {
            return Err(E_NOTIMPL.into());
        }
        if let Some(value) = clear {
            if !desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET) {
                return Err(E_INVALIDARG.into());
            }
            if value.Format != desc.Format {
                return Err(E_NOTIMPL.into());
            }
        }
        let mut images = lock(&self.memory.images.0);
        // An optimized clear value is a creation hint. Reopening a placed
        // texture with another hint must preserve its GPU and CPU contents.
        if let Some(texture) = Self::find_texture(&images, address, desc) {
            return Ok(texture);
        }
        if let Some(texture) = packed_target(&images, address, desc) {
            return Ok(texture);
        }
        same_memory_other_format(&images, address, desc);
        let packed_owner = packed_copy::owner_of(&images, address, desc);
        let texture = Texture::new(device, &self.memory.images, address, desc, clear)?;
        if let Some(owner) = &packed_owner {
            packed_copy::register(device, owner, &texture);
        }
        self.memory.device.get_or_init(|| {
            let mut registry = lock(&FLUSH_DEVICES);
            registry.retain(|device| device.strong_count() != 0);
            registry.push(Arc::downgrade(&self.memory));
            device.clone()
        });
        images.entry(address).or_default().push(Alias {
            desc: *desc,
            texture: Arc::downgrade(&texture),
        });
        let older = if page_index() { index_pages(&self.memory.images, &texture) } else { Vec::new() };
        drop(images);
        // xcl insert(): a texture on pages superseded by a placement over a
        // different range is unmapped and its unshared pages go back to the pool.
        for old in &older {
            if old.on_pages && old.range() != texture.range() && old.superseded.load(Ordering::Relaxed) {
                pages::unmap(old);
            }
        }
        drop(older);
        Ok(texture)
    }
}

/// Whether a queue waits on placed-texture fences of other queues (on unless
/// TUXBOX_PLACED_CROSS_WAITS=0 off; default on, batch rule).
fn cross_waits() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PLACED_CROSS_WAITS") != Some("0"))
}

struct PendingWrite {
    // The readback job owns the image only through native completion. An idle
    // queue need not keep its last frame's GPU allocations alive indefinitely.
    texture: Weak<Texture>,
    range: Range<u64>,
    published: ID3D12Fence,
    tick: u64,
}

struct Submissions {
    transfer: upload::Transfer,
    /// test82 froze with queues held 2 s behind the one upload copy queue:
    /// an upload waited there on its submitting queue's reader, that queue
    /// sat in a GPU memory wait, and every other queue's later upload queued
    /// behind it, including the one writing the awaited word. Each guest
    /// queue now uploads on its own copy queue, keyed by the guest queue;
    /// TUXBOX_UPLOAD_PER_QUEUE=0 shares one as before.
    per_queue: std::collections::HashMap<usize, upload::Transfer>,
    writes: Vec<PendingWrite>,
}

fn upload_per_queue() -> bool {
    crate::env_toggle!("TUXBOX_UPLOAD_PER_QUEUE") != Some("0")
}

impl Texture {
    fn range(&self) -> Range<u64> {
        // The entire range was checked during construction.
        self.address..self.address + self.size
    }
}

fn overlap_note(what: &str) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!("{what}: batch runs anyway ({n} so far; TUXBOX_OVERLAP_STRICT=1 drops it)"));
    }
}

/// Whether a written texture's range overlaps another texture of the batch.
/// test108: submit time grew from 0.5 to 18.8 ms a frame as the heaps' views
/// (gathered into every batch) grew to tens of thousands of textures; this
/// was a writes x textures scan, now a sort and sweep by range start.
fn written_overlap(textures: &[Arc<Texture>], writes: &[Arc<Texture>]) -> bool {
    let written: std::collections::HashSet<*const Texture> = writes.iter().map(Arc::as_ptr).collect();
    let mut spans: Vec<(u64, u64, bool)> = textures.iter()
        .map(|t| { let r = t.range(); (r.start, r.end, written.contains(&Arc::as_ptr(t))) })
        .filter(|s| s.0 < s.1)
        .collect();
    spans.sort_unstable_by_key(|s| s.0);
    // Furthest end so far of any span, and of a written one.
    let (mut end_any, mut end_written) = (0u64, 0u64);
    for (start, end, write) in spans {
        if start < end_written || (write && start < end_any) {
            return true;
        }
        end_any = end_any.max(end);
        if write {
            end_written = end_written.max(end);
        }
    }
    false
}

/// Logs the largest batch so far each time it doubles.
fn batch_size_note(textures: usize, writes: usize) {
    static MAX: AtomicU64 = AtomicU64::new(0);
    let n = textures as u64;
    let max = MAX.load(Ordering::Relaxed);
    if n >= 256 && n >= max.saturating_mul(2) && MAX.compare_exchange(max, n, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: a batch now covers {textures} textures ({writes} written), most gathered from its descriptor heaps' views"));
    }
}

fn overlaps(a: &Range<u64>, b: &Range<u64>) -> bool {
    a.start < b.end && b.start < a.end
}

fn unique(textures: &mut Vec<Arc<Texture>>) {
    textures.sort_unstable_by_key(Arc::as_ptr);
    textures.dedup_by(|a, b| Arc::ptr_eq(a, b));
}

// Descriptor contents may change after command-list Close. Resolve SRVs in the
// shader-visible heaps that actually participated in a draw or dispatch.
/// test81 dropped a batch on E_OUTOFMEMORY in placed texture submission.
/// A step that runs out of memory is retried once after dead shadows and
/// their cached imports are released; TUXBOX_PLACED_OOM_RETRY=0 fails at once.
fn oom_retry<T>(device: &Device, what: &str, mut f: impl FnMut() -> Result<T>) -> Result<T> {
    match f() {
        Err(e) if e.code() == E_OUTOFMEMORY && crate::env_toggle!("TUXBOX_PLACED_OOM_RETRY") != Some("0") => {
            let released = super::views::release_dead_shadows(device);
            let result = f();
            oom_log(format_args!("placed texture {what} ran out of memory: released {released} dead shadows and imports, retry {}",
                match &result { Ok(_) => "succeeded".to_string(), Err(e) => format!("failed {:#010x}", e.code().0) }));
            result
        }
        result => result,
    }
}

fn oom_log(line: std::fmt::Arguments) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 16 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!("{line} ({n} so far)"));
    }
}

/// After a failed retry, a refresh or write-back that cannot get memory is
/// skipped (stale bytes) instead of dropping the batch;
/// TUXBOX_PLACED_OOM_STRICT=1 drops it.
fn oom_skip(error: &windows::core::Error) -> bool {
    error.code() == E_OUTOFMEMORY && crate::env_toggle!("TUXBOX_PLACED_OOM_STRICT") != Some("1")
}

pub(crate) unsafe fn submit(
    device: *mut c_void,
    queue: &ID3D12CommandQueue,
    access: &QueueAccess,
    textures: &mut Vec<Arc<Texture>>,
    writes: &mut Vec<Arc<Texture>>,
    heaps: &[Range<usize>],
    execute: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let device = unsafe { &*device.cast::<Device>() };
    let placements = &device.state.placements;
    if super::heap::residency::tracking() {
        touch_render_views(placements, heaps);
        static PASS: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
        let now = std::time::Instant::now();
        let due = {
            let mut pass = PASS.lock().unwrap_or_else(|e| e.into_inner());
            let due = pass.is_none_or(|at| now.duration_since(at).as_secs_f64() >= 1.0);
            if due { *pass = Some(now); }
            due
        };
        if due {
            flush_render_views(placements, false);
        }
        if due && alias::over_budget(&device.native) {
            super::heap::residency::evict_idle(&device.native, || flush_render_views(placements, true));
        }
    }
    {
        let _site = crate::diagnostics::cost::site();
        placements.views.gather(heaps, textures, writes);
    }
    unique(writes);
    textures
        .try_reserve(writes.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    textures.extend(writes.iter().cloned());
    if textures.is_empty() {
        return execute();
    }
    unique(textures);
    batch_size_note(textures.len(), writes.len());
    remap_used(textures);
    unmap_idle(&placements.memory.images);
    unsafe { pages::before_execute(queue) };
    {
        let _site = crate::diagnostics::cost::site();
        oom_retry(device, "residency (spans)", || alias::touch(&device.native, textures.iter().filter_map(|t| t.span.as_ref())))?;
        oom_retry(device, "residency (pages)", || pages::touch(&device.native, textures.iter().filter(|t| t.on_pages).map(|t| &**t)))?;
    }
    // test63: refusing a batch whose written texture overlaps another of its
    // textures dropped five batches, with the lists in them that write GPU
    // memory wait words, and every queue then stalled on those words 2 s at
    // a time (0.1 fps). The batch now runs: overlapping layouts see each
    // other's bytes as the console's would. TUXBOX_OVERLAP_STRICT=1 refuses.
    let strict = crate::env_toggle!("TUXBOX_OVERLAP_STRICT") == Some("1");
    if !writes.is_empty() && written_overlap(textures, writes) {
        if strict {
            crate::diagnostics::recording_error(format_args!(
                "overlapping placed texture layouts in one GPU write batch"
            ));
            return Err(E_NOTIMPL.into());
        }
        overlap_note("a written placed texture overlaps another texture of its batch");
    }
    let _refresh_site = crate::diagnostics::cost::site();
    // test109: this block cost 12.7 ms/frame for refreshes of about 7 mips.
    // Sub-sites split it: the lock wait (a D3DFlushEntireCpuCache flush holds
    // it over every live texture), the pending-write scan, and the refresh.
    let lock_site = crate::diagnostics::cost::site();
    let mut slot = lock(&placements.memory.submissions);
    drop(lock_site);
    HRESULT(placements.memory.failed.load(Ordering::Acquire)).ok()?;
    let submission = match slot.as_mut() {
        Some(s) => s,
        None => slot.insert(Submissions {
            transfer: upload::Transfer::new(&device.native)?,
            per_queue: Default::default(),
            writes: Vec::new(),
        }),
    };
    let reader = access.reader(&device.native)?;
    let scan_site = crate::diagnostics::cost::site();
    submission
        .writes
        .retain(|w| unsafe { w.published.GetCompletedValue() } < w.tick);
    pending_writes_note(submission.writes.len());
    let copy_kind = unsafe { queue.GetDesc() }.Type;
    let (alias_copy, alias_skip) = packed_copy::before_batch(&device.native, copy_kind, textures, writes);
    let mut refresh = Vec::new();
    refresh
        .try_reserve(textures.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    for texture in textures.iter() {
        let mut pending = false;
        for write in submission
            .writes
            .iter()
            .filter(|w| overlaps(&texture.range(), &w.range))
        {
            if Arc::as_ptr(texture) != write.texture.as_ptr() {
                if strict {
                    crate::diagnostics::recording_error(format_args!(
                        "placed texture layout alias before GPU write publication"
                    ));
                    return Err(E_NOTIMPL.into());
                }
                // Ordered after the write; the alias keeps its native bytes.
                overlap_note("a placed texture aliases one whose GPU write is not yet published");
            }
            // CPU bytes still contain the old version. The native image contains
            // the ordered GPU write and can be read directly by the next batch.
            // test76: this wait held the title's writer queue 2 s at 0 of 4761
            // while the writing queue waited on the title. The writing queue
            // already waits for its own publication, and the title orders its
            // queues itself, so no queue waits here unless
            // TUXBOX_PLACED_CROSS_WAITS (default on; =0 skips it).
            if cross_waits() {
                unsafe { queue.Wait(&write.published, write.tick)? };
                crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(queue) as usize, "placed texture write-back publication", &write.published, write.tick);
            }
            pending = true;
        }
        packed_read_note(texture, pending);
        if !pending && !alias_skip.contains(&Arc::as_ptr(texture)) {
            refresh.push(texture.clone());
        }
    }
    drop(scan_site);

    if upload::drain_any() {
        submission.transfer.drain();
        for other in submission.per_queue.values_mut() { other.drain(); }
    }
    let transfer = if upload_per_queue() {
        match submission.per_queue.entry(windows::core::Interface::as_raw(queue) as usize) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(upload::Transfer::new(&device.native)?),
        }
    } else {
        &mut submission.transfer
    };
    let transfer_site = crate::diagnostics::cost::site();
    let refreshed = oom_retry(device, "refresh", || transfer.refresh(&device.native, &refresh, Some(reader)));
    drop(transfer_site);
    if let Err(error) = refreshed {
        if !oom_skip(&error) {
            return Err(error);
        }
        oom_log(format_args!("placed texture refresh of {} textures skipped on E_OUTOFMEMORY: the batch runs with their previous bytes (TUXBOX_PLACED_OOM_STRICT=1 drops it)", refresh.len()));
    }
    drop(_refresh_site);
    let _wait_site = crate::diagnostics::cost::site();
    // Each upload fence this batch's textures were last uploaded on, with
    // the highest tick needed from it.
    let mut required: Vec<(ID3D12Fence, u64)> = Vec::new();
    for t in textures.iter() {
        let contents = lock(&t.contents);
        let Some(fence) = contents.uploaded_on.as_ref() else { continue };
        match required.iter_mut().find(|(f, _)| f == fence) {
            Some((_, v)) => *v = (*v).max(contents.uploaded),
            None => required.push((fence.clone(), contents.uploaded)),
        }
    }
    for (fence, value) in &required {
        if unsafe { fence.GetCompletedValue() } < *value {
            unsafe { queue.Wait(fence, *value)? };
            crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(queue) as usize, "placed texture upload", fence, *value);
        }
    }
    for texture in writes.iter() {
        for (fence, value) in &lock(&texture.contents).readers {
            // Another queue's read: the title orders it (test76 held a queue
            // 1.8 s on one), so it is waited on only with TUXBOX_PLACED_CROSS_WAITS (default on; =0 skips it).
            if cross_waits() && fence != reader && unsafe { fence.GetCompletedValue() } < *value {
                unsafe { queue.Wait(fence, *value)? };
                crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(queue) as usize, "placed texture reader", fence, *value);
            }
        }
    }
    drop(_wait_site);
    let tick = transfer.tick()?;
    let publication = if writes.is_empty() {
        None
    } else {
        submission
            .writes
            .try_reserve(writes.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        let worker = access.worker(&device.native, &placements.memory.failed)?;
        let kind = unsafe { queue.GetDesc() }.Type;
        match oom_retry(device, "write-back", || writeback::Readback::record(&device.native, kind, writes)) {
            Ok(readback) => Some((worker, readback)),
            Err(error) if oom_skip(&error) => {
                oom_log(format_args!("placed texture write-back of {} textures skipped on E_OUTOFMEMORY: their CPU bytes stay stale (TUXBOX_PLACED_OOM_STRICT=1 drops the batch)", writes.len()));
                None
            }
            Err(error) => return Err(error),
        }
    };
    if let Some(copy) = alias_copy {
        unsafe { queue.ExecuteCommandLists(&[Some(windows::core::Interface::cast(&copy.1)?)]) };
        crate::dred::check(queue, "packed alias copy");
        packed_copy::retain(copy, reader, tick);
    }
    execute()?;
    if let Some((_, readback)) = &publication {
        // Separate ExecuteCommandLists calls allow simultaneous-access textures
        // to decay to COMMON between the guest batch and this copy.
        unsafe { queue.ExecuteCommandLists(&[Some(readback.list.clone())]) };
        crate::dred::check(queue, "placement readback");
    }
    if let Err(error) = unsafe { queue.Signal(reader, tick) } {
        transfer.failed = Some(error.code());
        // A failed signal supplies no proof that the GPU released the recording.
        // Retry once to retire it, or retain it for the lifetime of a live driver.
        if let Some((worker, readback)) = publication {
            if unsafe { queue.Signal(reader, tick) }.is_ok() {
                worker.enqueue(readback, reader, tick)?;
            } else if unsafe { device.native.GetDeviceRemovedReason() }.is_ok() {
                writeback::park(readback, reader, tick);
            }
        }
        return Err(error);
    }
    if let Some((worker, readback)) = publication {
        if let Err(error) = worker.enqueue(readback, reader, tick) {
            transfer.failed = Some(error.code());
            return Err(error);
        }
        // A guest fence signaled after this operation must include the CPU
        // publication. The submitting CPU thread remains free to signal fences.
        crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(queue) as usize, "placed texture write-back", &worker.published, tick);
        if let Err(error) = unsafe { queue.Wait(&worker.published, tick) } {
            transfer.failed = Some(error.code());
            return Err(error);
        }
        for texture in writes.iter() {
            submission.writes.push(PendingWrite {
                texture: Arc::downgrade(texture),
                range: texture.range(),
                published: worker.published.clone(),
                tick,
            });
        }
    }
    let _site = crate::diagnostics::cost::site();
    for texture in textures {
        let mut state = lock(&texture.contents);
        state
            .readers
            .retain(|(f, v)| f != reader && unsafe { f.GetCompletedValue() } < *v);
        state.readers.push((reader.clone(), tick));
    }
    Ok(())
}

/// Gap 6 remainder (fxceless gaps item 2): test152 placed 12 R9G9B9E5
/// textures over live UAV-only R32_UINT ones, which 5e3a1ee packed_target
/// does not serve. Their bytes come only from title memory, filled by the
/// owner's write-back. Each batch reading one logs (rate-limited) whether
/// a GPU write is still unpublished and whether its first 4 KiB of title
/// memory are all zero, to tell a missing write-back from a stale re-read
/// before porting fxceless's GPU copy. TUXBOX_PACKED_READ_NOTE=0 off.
fn packed_read_note(texture: &Texture, pending: bool) {
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R9G9B9E5_SHAREDEXP;
    if texture.desc.Format != DXGI_FORMAT_R9G9B9E5_SHAREDEXP || crate::env_toggle!("TUXBOX_PACKED_READ_NOTE") == Some("0") {
        return;
    }
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if !(n <= 8 || n.is_power_of_two()) {
        return;
    }
    let mut sample = [0u8; 4096];
    let mut read = 0usize;
    let ok = unsafe {
        ReadProcessMemory(GetCurrentProcess(), texture.address as *const core::ffi::c_void, sample.as_mut_ptr().cast(),
            sample.len().min(texture.size as usize), Some(&mut read))
    }.is_ok();
    let zero = sample[..read].iter().all(|&b| b == 0);
    crate::diagnostics::log_line(format_args!(
        "placed textures: a batch reads R9G9B9E5 {}x{} at {:#x}: GPU write unpublished {pending}, title memory read {} ({read} bytes), all zero {zero} ({n} so far; TUXBOX_PACKED_READ_NOTE=0 off)",
        texture.desc.Width, texture.desc.Height, texture.address, if ok { "ok" } else { "failed" }));
}
