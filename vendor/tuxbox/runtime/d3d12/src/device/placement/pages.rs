// Superseded-texture unmapping on a pool of 64 KiB video-memory pages.
//
// Copyright (C) ivall and the XCL contributors (https://github.com/ivall/xcl)
// Copyright (C) Neonbridge contributors
//
// Copied from ivall/xcl crates/xcl/src/hle/d3d12x/vram.rs (commit 7695539,
// from ad79932/38b05b7): the page pool (Pages, take_tile, map, unmap,
// release_tiles, bind, heap_ranges, owned_pages, sweep_idle), unmap_superseded,
// remap_if_unmapped, forget_pages, before_execute and page_bytes, with their
// tests. XCL is licensed GPL-2.0-only; this file keeps that origin and is
// credited in docs/DETAILS.md. Adapted: logging goes to the d3d12 log, the
// heap is made directly (no own_work), and place/claim/stats are added here to
// fit Neonbridge's placed textures.
//
// This program is free software; you can redistribute it and/or modify it
// under the terms of the GNU General Public License version 2 as published by
// the Free Software Foundation. This program is distributed in the hope that it
// will be useful, but WITHOUT ANY WARRANTY; without even the implied warranty
// of MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General
// Public License for more details: <https://www.gnu.org/licenses/>.

//! Placed textures as reserved (tiled) resources on the title's 64 KiB pages:
//! a page of video memory for each 64 KiB page of guest address a texture
//! covers, so textures at one address share pages as on the console. test55-57
//! ran out of system commit with ~10k live placed textures (2.7 GiB) superseded
//! by newer placements; spans kept their heaps alive. A texture superseded by
//! a placement over a different range is unmapped (its tiles read zero) and
//! its unshared pages go back to the pool; when the newer one is released the
//! newest covered older one is mapped again and re-uploaded. Title queues wait
//! on the mapping fence before their next work. TUXBOX_PLACED_PAGES=0 keeps
//! every placed texture in spans (alias.rs).
#![allow(unsafe_op_in_unsafe_fn, dead_code)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, Weak};

use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

use super::Texture;

/// bytes of a page – of the title's addresses and of a tile
const PAGE: usize = 0x1_0000;
/// pages of a video-memory pool heap (64 MiB)
const POOL_PAGES: u32 = 1024;
/// a page no target uses any more is kept this long for the next target at its address (with
/// what it holds – a title may make a target again over memory it rendered to)
const IDLE: std::time::Duration = std::time::Duration::from_secs(2);
/// a pool tile given back is not reused before this (the GPU may still be at work on what the
/// title released)
const COOL: std::time::Duration = std::time::Duration::from_secs(2);

/// A pool tile: (pool heap, tile in it).
type Tile = (u32, u32);

struct Page {
    tile: Tile,
    /// targets using the page
    users: u32,
    /// when the last of them went
    idle_since: Option<std::time::Instant>,
}

/// A page of `Pages::pages` (its title address) or of `Pages::extra_pages` (its key there).
#[derive(Clone, Copy)]
enum PageKey {
    Own(usize),
    Extra(usize, usize),
}

/// The pages of `idle` – those whose last user went, oldest first, each with when – that no
/// resource has used for `IDLE` go back to the pools: out of their map, their tile to `cooling`.
/// An entry whose page was used again since (in use now, or idle again from a later moment, with
/// a later entry) is dropped. Only the queue is looked at: every page of both maps, looked at at
/// every placement, grew with the square of what a title streams.
fn sweep_idle(
    idle: &mut std::collections::VecDeque<(PageKey, std::time::Instant)>,
    pages: &mut std::collections::HashMap<usize, Page>,
    extra_pages: &mut std::collections::HashMap<(usize, usize), Page>,
    cooling: &mut std::collections::VecDeque<(Tile, std::time::Instant)>,
    now: std::time::Instant,
) {
    while idle.front().is_some_and(|&(_, since)| now.duration_since(since) >= IDLE) {
        let Some((key, since)) = idle.pop_front() else { break };
        let still_idle = |p: &Page| p.idle_since == Some(since);
        let page = match key {
            PageKey::Own(a) if pages.get(&a).is_some_and(still_idle) => pages.remove(&a),
            PageKey::Extra(va, i) if extra_pages.get(&(va, i)).is_some_and(still_idle) => extra_pages.remove(&(va, i)),
            _ => None,
        };
        if let Some(p) = page {
            cooling.push_back((p.tile, now));
        }
    }
}

struct Target {
    va: usize,
    console_bytes: Option<u64>,
    total: u32,
    mapped: bool,
    used: Vec<usize>,
    used_extra: Vec<(usize, usize)>,
    /// the pool heaps its tiles are in (residency, `touch`)
    heaps: Vec<u32>,
    /// a sampled texture, whose new pools may go to system memory
    system_ok: bool,
    /// leading tiles (the top `levels` mips) left unmapped (see `trim_tiles`)
    skip: u32,
    /// the first mapped mip
    levels: u32,
}

/// Per-heap free-tile allocator for `Pages`. Copied from ivall/xcl 3bc7143
/// (raylitebull, GPL-2.0-only), "keep vram::Pages tile mappings contiguous per
/// heap": free tiles are kept per pool heap (sorted descending so `.pop()` yields
/// ascending contiguous tiles), and a texture takes its tiles from one heap when
/// one fits it, so streaming churn across many 64 MiB pools no longer scatters a
/// texture over dozens of heaps (dozens of UpdateTileMappings calls each).
/// Adapted: heaps evicted while empty (`trim_empty`) are taken from last, and
/// TUXBOX_PAGES_CONTIGUOUS=0 takes tile by tile from the fullest heap instead,
/// as the packing order before did.
#[derive(Default)]
struct TilePool {
    /// Free tile indices for each pool heap (descending when `!dirty_sort[h]`).
    free_by_heap: Vec<Vec<u32>>,
    /// Whether `free_by_heap[h]` must be sorted descending before popping.
    dirty_sort: Vec<bool>,
    /// Total free tiles currently available across all heaps.
    free_total: usize,
    /// Tiles given back, reusable after `COOL`.
    cooling: std::collections::VecDeque<(Tile, std::time::Instant)>,
}

impl TilePool {
    fn add_heap(&mut self, pool_pages: u32) -> u32 {
        let h = self.free_by_heap.len() as u32;
        self.free_by_heap.push((0..pool_pages).rev().collect());
        self.dirty_sort.push(false);
        self.free_total += pool_pages as usize;
        h
    }

    fn push_cooling(&mut self, tile: Tile, now: std::time::Instant) {
        self.cooling.push_back((tile, now));
    }

    /// Move every cooled tile (`>= COOL`) into its pool heap's free list so best-fit allocation
    /// sees all ready tiles at once.
    fn drain_cooling(&mut self, now: std::time::Instant) {
        while self.cooling.front().is_some_and(|&(_, at)| now.duration_since(at) >= COOL) {
            let Some((tile, _)) = self.cooling.pop_front() else { break };
            self.return_tiles(&[tile]);
        }
    }

    fn return_tiles(&mut self, tiles: &[Tile]) {
        for &(h, t) in tiles {
            let idx = h as usize;
            if idx < self.free_by_heap.len() {
                self.free_by_heap[idx].push(t);
                self.dirty_sort[idx] = true;
                self.free_total += 1;
            }
        }
    }

    /// Pick the pool heap to take up to `rem` tiles from, warm heaps first (`cold[h]`: evicted
    /// while empty, used only when no warm heap has a free tile):
    /// 1. `preferred` if it fits all `rem` tiles (keeps partial-overlap placements on 1 heap),
    /// 2. otherwise best-fit heap with `len >= rem` (fits the whole texture in 1 heap without
    ///    splitting fuller pools),
    /// 3. otherwise the heap with the most free tiles (minimizing distinct heaps touched).
    /// `contiguous == false`: the heap with the fewest free tiles (the old packing order).
    fn pick_heap(&self, rem: usize, preferred: Option<u32>, cold: &[bool], contiguous: bool) -> Option<usize> {
        if self.free_total == 0 {
            return None;
        }
        let is_cold = |h: usize| cold.get(h).copied().unwrap_or(false);
        for want_cold in [false, true] {
            let candidates = || self.free_by_heap.iter().enumerate().filter(move |&(h, t)| !t.is_empty() && is_cold(h) == want_cold);
            if !contiguous {
                if let Some((h, _)) = candidates().min_by_key(|&(h, t)| (t.len(), h)) {
                    return Some(h);
                }
                continue;
            }
            if let Some(p) = preferred.map(|h| h as usize).filter(|&h| h < self.free_by_heap.len() && is_cold(h) == want_cold)
                && self.free_by_heap[p].len() >= rem
            {
                return Some(p);
            }
            let mut best_fit: Option<(usize, usize)> = None;
            let mut largest: Option<(usize, usize)> = None;
            // TUXBOX_PAGES_DENSE_FIRST (default on, =0 the heap with the most free
            // tiles): a texture no heap fits whole fills the densest heap first,
            // so tiles never spread over the sparsest pools. test173 ended with
            // 1202 MiB of tiles across 71 pools of 64 MiB, only 5 ever empty.
            let dense = dense_first();
            for (h, tiles) in candidates() {
                let len = tiles.len();
                if len >= rem {
                    if best_fit.is_none_or(|(_, blen)| len < blen) {
                        best_fit = Some((h, len));
                    }
                } else if largest.is_none_or(|(lh, llen)| if dense { len < llen } else { len > llen } || (len == llen && preferred == Some(h as u32) && preferred != Some(lh as u32))) {
                    largest = Some((h, len));
                }
            }
            if let Some((h, _)) = best_fit.or(largest) {
                return Some(h);
            }
        }
        None
    }

    /// Allocate `count` tiles, preferring a single heap and ascending contiguous offsets.
    /// `make_heap` allocates a new backing pool heap of `pool_pages` tiles when no heap has a
    /// free tile. On failure, any tiles already taken are returned untouched.
    fn take_batch(
        &mut self,
        count: usize,
        preferred: Option<u32>,
        cold: &[bool],
        now: std::time::Instant,
        pool_pages: u32,
        contiguous: bool,
        mut make_heap: impl FnMut() -> Option<u32>,
    ) -> Option<Vec<Tile>> {
        if count == 0 {
            return Some(Vec::new());
        }
        self.drain_cooling(now);
        let mut out = Vec::with_capacity(count);
        while out.len() < count {
            let rem = count - out.len();
            let h = match self.pick_heap(rem, preferred, cold, contiguous) {
                Some(h) => h,
                None => {
                    let Some(new_h) = make_heap() else {
                        self.return_tiles(&out);
                        return None;
                    };
                    let added = self.add_heap(pool_pages);
                    debug_assert_eq!(added, new_h);
                    added as usize
                }
            };
            if self.dirty_sort[h] {
                self.free_by_heap[h].sort_unstable_by(|a, b| b.cmp(a));
                self.dirty_sort[h] = false;
            }
            let free = &mut self.free_by_heap[h];
            let take = if contiguous { rem.min(free.len()) } else { 1 };
            for _ in 0..take {
                let t = free.pop().unwrap();
                out.push((h as u32, t));
            }
            self.free_total -= take;
        }
        Some(out)
    }
}

fn dense_first() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PAGES_DENSE_FIRST") != Some("0"))
}

/// The title's pages of video memory for tiled textures (see the module's comment).
struct Pages {
    queue: ID3D12CommandQueue,
    /// signalled after each mapping; title queues wait for it before their next work
    fence: ID3D12Fence,
    value: u64,
    pools: Vec<ID3D12Heap>,
    /// free and cooling tiles, per pool heap (xcl 3bc7143)
    pool: TilePool,
    /// title page address → its video memory
    pages: std::collections::HashMap<usize, Page>,
    /// extra / sub-64 KiB tiles keyed by `(va, tile_index)` so repeated placements at the same
    /// address share them without aliasing unrelated addresses
    extra_pages: std::collections::HashMap<(usize, usize), Page>,
    /// pages of both maps whose last user went, oldest first, with when (`sweep_idle`)
    idle: std::collections::VecDeque<(PageKey, std::time::Instant)>,
    /// resource → its mapping metadata and currently held pages / extra keys
    targets: std::collections::HashMap<usize, Target>,
    /// queue → the fence value it last waited for
    waited: std::collections::HashMap<usize, u64>,
    /// the fence value last signalled: mappings signal once, before the next
    /// title work that waits for them (test58: a Signal per placed texture)
    signaled: u64,
    /// per pool heap: last submission using it, evicted, in system memory
    pool_used: Vec<std::time::Instant>,
    pool_evicted: Vec<bool>,
    pool_system: Vec<bool>,
    /// evicted because none of its tiles is in use (`trim_empty`)
    pool_empty: Vec<bool>,
    /// compaction copies the GPU may still run: (fence value, what they hold)
    compacted: Vec<(u64, Vec<ID3D12Resource>, ID3D12CommandAllocator, ID3D12GraphicsCommandList)>,
}

static COMPACTED_POOLS: AtomicU64 = AtomicU64::new(0);

/// TUXBOX_PAGES_COMPACT (default on, =0 off): over budget, the pages of the
/// sparsest idle pool move to free tiles of other pools, so it drains and
/// `trim_empty` evicts it. test173 ended with 71 pools (4544 MiB) for 1202 MiB
/// of tiles in use at device local 7551 of 7188 MiB: the pools grew to the
/// load's 4233 MiB peak, and long-lived textures then held a few tiles in
/// nearly every pool, so only 5 ever emptied; demoting the other 60 (lower
/// residency priority) left them charged. Only pools whose textures are all
/// sampled ones (no render target, depth or UAV) and that no submission used
/// for TUXBOX_SPAN_EVICT_S are moved, so nothing writes a page while it is
/// copied. The copy goes through two reserved buffers mapped onto the old and
/// the new tiles (tile bytes shared by resources at one address, as every
/// page here already is), on the pages queue before the targets are mapped
/// again, and title work waits for that fence as for any mapping.
fn compact_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PAGES_COMPACT") != Some("0"))
}

static EMPTY_EVICTED: AtomicU64 = AtomicU64::new(0);
static EMPTY_RESIDENT: AtomicU64 = AtomicU64::new(0);

// D3D12 objects are free-threaded
unsafe impl Send for Pages {}

/// device raw pointer → its pages (None inside: tiled tier < 2 on this device)
static PAGES: Mutex<std::collections::BTreeMap<usize, Option<Pages>>> = Mutex::new(std::collections::BTreeMap::new());

impl Pages {
    unsafe fn new(dev: &ID3D12Device) -> Option<Pages> {
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, &mut o as *mut _ as *mut c_void, std::mem::size_of_val(&o) as u32).ok()?;
        // tier 2: unmapped tiles read as zero, writes to them go nowhere
        if o.TiledResourcesTier.0 < D3D12_TILED_RESOURCES_TIER_2.0 {
            crate::diagnostics::log_line(format_args!("D3D12.X: tiled resources tier {}: GPU-owned textures in arenas", o.TiledResourcesTier.0));
            return None;
        }
        let queue: ID3D12CommandQueue = dev.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC::default()).ok()?;
        let fence: ID3D12Fence = dev.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
        crate::diagnostics::log_line(format_args!("D3D12.X: textures in title memory: tiled resources on 64 KiB pages of video memory, one per page of the title's"));
        Some(Pages {
            queue,
            fence,
            value: 0,
            pools: Vec::new(),
            pool: TilePool::default(),
            pages: std::collections::HashMap::new(),
            extra_pages: std::collections::HashMap::new(),
            idle: std::collections::VecDeque::new(),
            targets: std::collections::HashMap::new(),
            waited: std::collections::HashMap::new(),
            signaled: 0,
            pool_used: Vec::new(),
            pool_evicted: Vec::new(),
            pool_system: Vec::new(),
            pool_empty: Vec::new(),
            compacted: Vec::new(),
        })
    }

    /// Over budget, page pools none of whose tiles is in use are evicted,
    /// keeping one such pool resident; their tiles go to the bottom of the
    /// free list so resident pools are used first, and a pool is made resident
    /// again when its tile is taken (`take_tile`). test91b held 3712 MiB of
    /// pools for 2607 MiB of pages in use, at device local 7748 of 7188 MiB.
    /// Nothing maps an empty pool, so unlike a demoted one it cannot be
    /// sampled while evicted. TUXBOX_PAGES_TRIM_EMPTY=0 off.
    ///
    /// Every pass (over budget or not) also packs: test93 held 3776 MiB of
    /// pools for 3136 MiB of pages and never had an empty pool, since the
    /// free list was LIFO and tiles given back came out of every pool. The
    /// free list is ordered so new tiles come from the fullest pools first
    /// and the sparsest pools are taken from last, so they drain to empty.
    /// TUXBOX_PAGES_PACK=0 keeps the old order.
    /// One pass of `compact_on`: moves the pages of the sparsest idle pool
    /// (at most a quarter full) that is not in `busy`.
    unsafe fn compact(&mut self, dev: &ID3D12Device, busy: &[bool], idle_s: f64) {
        let done = self.fence.GetCompletedValue();
        self.compacted.retain(|c| c.0 > done);
        let now = std::time::Instant::now();
        self.pool.drain_cooling(now);
        let n = self.pools.len();
        let mut used = vec![0usize; n];
        for p in self.pages.values().chain(self.extra_pages.values()) {
            used[p.tile.0 as usize] += 1;
        }
        let mut written = vec![false; n];
        for t in self.targets.values().filter(|t| t.mapped && !t.system_ok) {
            for &h in &t.heaps {
                written[h as usize] = true;
            }
        }
        let Some(h) = (0..n)
            .filter(|&h| used[h] != 0 && used[h] <= POOL_PAGES as usize / 4 && !busy.get(h).copied().unwrap_or(false)
                && !written[h] && !self.pool_empty[h] && now.duration_since(self.pool_used[h]).as_secs_f64() >= idle_s)
            .min_by_key(|&h| used[h])
        else { return };
        let count = used[h];
        let free_elsewhere = self.pool.free_total - self.pool.free_by_heap[h].len();
        if free_elsewhere < count {
            return;
        }
        // The new tiles come from other pools: this one's free list is set aside.
        let own_free = std::mem::take(&mut self.pool.free_by_heap[h]);
        self.pool.free_total -= own_free.len();
        let fresh = self.take_batch(dev, count, None, false, now);
        self.pool.free_total += own_free.len();
        self.pool.free_by_heap[h] = own_free;
        let Some(fresh) = fresh else { return };
        if fresh.iter().any(|t| t.0 as usize == h) || fresh.len() != count {
            self.pool.return_tiles(&fresh);
            return;
        }
        let keys: Vec<PageKey> = self.pages.iter().filter(|(_, p)| p.tile.0 as usize == h).map(|(&a, _)| PageKey::Own(a))
            .chain(self.extra_pages.iter().filter(|(_, p)| p.tile.0 as usize == h).map(|(&(va, i), _)| PageKey::Extra(va, i)))
            .collect();
        let tile_of = |s: &Self, k: PageKey| match k {
            PageKey::Own(a) => s.pages[&a].tile,
            PageKey::Extra(va, i) => s.extra_pages[&(va, i)].tile,
        };
        let old: Vec<Tile> = keys.iter().map(|&k| tile_of(self, k)).collect();
        let reserved = |tiles: usize| -> Option<ID3D12Resource> {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: (tiles * PAGE) as u64, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
            };
            let mut r: Option<ID3D12Resource> = None;
            dev.CreateReservedResource(&desc, D3D12_RESOURCE_STATE_COMMON, None, &mut r).ok()?;
            r
        };
        let fail = |s: &mut Self, why: &str| {
            s.pool.return_tiles(&fresh);
            crate::diagnostics::log_line(format_args!("placed textures: compacting a page pool failed: {why} (TUXBOX_PAGES_COMPACT=0 off)"));
        };
        let (Some(from), Some(to)) = (reserved(count), reserved(count)) else { return fail(self, "reserved buffer") };
        let Ok(alloc) = dev.CreateCommandAllocator::<ID3D12CommandAllocator>(D3D12_COMMAND_LIST_TYPE_DIRECT) else { return fail(self, "allocator") };
        let Ok(list) = dev.CreateCommandList::<_, _, ID3D12GraphicsCommandList>(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &alloc, None) else { return fail(self, "command list") };
        self.map(&from, &old, 0);
        self.map(&to, &fresh, 0);
        list.CopyBufferRegion(&to, 0, &from, 0, (count * PAGE) as u64);
        if list.Close().is_err() {
            return fail(self, "close");
        }
        self.queue.ExecuteCommandLists(&[Some(list.cast().expect("command list"))]);
        for (&k, &t) in keys.iter().zip(&fresh) {
            match k {
                PageKey::Own(a) => self.pages.get_mut(&a).expect("page").tile = t,
                PageKey::Extra(va, i) => self.extra_pages.get_mut(&(va, i)).expect("page").tile = t,
            }
        }
        // Every target on the pool is mapped again from its first mapped tile.
        let moved: Vec<usize> = self.targets.iter().filter(|(_, t)| t.mapped && t.heaps.contains(&(h as u32))).map(|(&k, _)| k).collect();
        for key in &moved {
            let t = &self.targets[key];
            let tiles: Vec<Tile> = t.used.iter().map(|a| self.pages[a].tile).chain(t.used_extra.iter().map(|k| self.extra_pages[k].tile)).collect();
            let levels = t.levels;
            let raw = *key as *mut c_void;
            let Some(res) = ID3D12Resource::from_raw_borrowed(&raw) else { continue };
            let res = res.clone();
            self.map(&res, &tiles, levels);
            let mut heaps: Vec<u32> = tiles.iter().map(|t| t.0).collect();
            heaps.sort_unstable();
            heaps.dedup();
            if let Some(t) = self.targets.get_mut(key) {
                t.heaps = heaps;
            }
        }
        self.value += 1;
        let _ = self.queue.Signal(&self.fence, self.value);
        self.signaled = self.value;
        for &t in &old {
            self.pool.push_cooling(t, now);
        }
        self.compacted.push((self.value, vec![from, to], alloc, list));
        let total = COMPACTED_POOLS.fetch_add(1, Relaxed) + 1;
        if total <= 8 || total.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "placed textures: compacted page pool {h}: {count} pages ({} MiB) of {} textures moved to other pools ({total} pools so far; TUXBOX_PAGES_COMPACT=0 off)",
                count * PAGE >> 20, moved.len()));
        }
    }

    unsafe fn trim_empty(&mut self, dev: &ID3D12Device, over_budget: bool) {
        let pack = crate::env_toggle!("TUXBOX_PAGES_PACK") != Some("0");
        let trim = over_budget && crate::env_toggle!("TUXBOX_PAGES_TRIM_EMPTY") != Some("0");
        if !pack && !trim {
            return;
        }
        self.pool.drain_cooling(std::time::Instant::now());
        let free: Vec<u32> = (0..self.pools.len()).map(|h| self.pool.free_by_heap.get(h).map_or(0, |f| f.len() as u32)).collect();
        let empty: Vec<usize> = if trim { (0..self.pools.len()).filter(|&h| free[h] >= POOL_PAGES).collect() } else { Vec::new() };
        let resident: Vec<usize> = empty.iter().copied().filter(|&h| !self.pool_empty[h]).collect();
        let kept = usize::from(empty.len() == resident.len());
        let cold: Vec<usize> = resident.into_iter().skip(kept).collect();
        if !cold.is_empty() {
            let list: Vec<Option<ID3D12Pageable>> = cold.iter().map(|&h| self.pools[h].cast().ok()).collect();
            match dev.Evict(&list) {
                Ok(()) => {
                    for &h in &cold {
                        self.pool_empty[h] = true;
                    }
                    let n = EMPTY_EVICTED.fetch_add(cold.len() as u64, Relaxed) + cold.len() as u64;
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: evicted {} empty page pools ({} MiB; {n} so far, {} made resident again; TUXBOX_PAGES_TRIM_EMPTY=0 off)",
                        cold.len(), cold.len() * POOL_PAGES as usize * PAGE >> 20, EMPTY_RESIDENT.load(Relaxed)));
                }
                Err(e) => crate::diagnostics::log_line(format_args!("placed textures: Evict of {} empty page pools failed {e}", cold.len())),
            }
        }
        // Evicted pools' tiles are taken last, and a texture's tiles come from
        // the best-fitting heap (TilePool::pick_heap, xcl 3bc7143).
        if pack {
            static PACKS: AtomicU64 = AtomicU64::new(0);
            let n = PACKS.fetch_add(1, Relaxed) + 1;
            if n.is_power_of_two() && n >= 16 {
                let all_free = free.iter().filter(|&&f| f >= POOL_PAGES).count();
                let half_free = free.iter().filter(|&&f| f >= POOL_PAGES / 2).count();
                crate::diagnostics::log_line(format_args!(
                    "placed textures: packing pass {n}: {} pools, {all_free} with no tile in use, {half_free} at least half free, {} tiles free (TUXBOX_PAGES_PACK=0 off)",
                    self.pools.len(), self.pool.free_total));
            }
        }
    }

    /// Pages no resource used for `IDLE` go back to the pools.
    fn sweep(&mut self, now: std::time::Instant) {
        sweep_idle(&mut self.idle, &mut self.pages, &mut self.extra_pages, &mut self.pool.cooling, now);
    }

    /// `count` tiles for one texture, from one heap when one fits (xcl 3bc7143). A pool
    /// evicted while empty is made resident before its tiles are used again.
    unsafe fn take_batch(&mut self, dev: &ID3D12Device, count: usize, preferred: Option<u32>, system_ok: bool, now: std::time::Instant) -> Option<Vec<Tile>> {
        let contiguous = crate::env_toggle!("TUXBOX_PAGES_CONTIGUOUS") != Some("0");
        let cold = self.pool_empty.clone();
        let mut pool = std::mem::take(&mut self.pool);
        let tiles = pool.take_batch(count, preferred, &cold, now, POOL_PAGES, contiguous, || unsafe { self.new_pool(dev, system_ok) });
        self.pool = pool;
        let tiles = tiles?;
        let mut heaps: Vec<u32> = tiles.iter().map(|t| t.0).filter(|&h| self.pool_empty[h as usize]).collect();
        heaps.sort_unstable();
        heaps.dedup();
        for h in heaps {
            let list = [self.pools[h as usize].cast::<ID3D12Pageable>().ok()];
            if let Err(e) = dev.MakeResident(&list) {
                crate::diagnostics::log_line(format_args!("placed textures: MakeResident of an empty page pool failed {e}"));
                self.pool.return_tiles(&tiles);
                return None;
            }
            self.pool_empty[h as usize] = false;
            EMPTY_RESIDENT.fetch_add(1, Relaxed);
        }
        Some(tiles)
    }

    /// A new 64 MiB pool heap (its index); the caller adds its tiles to the pool.
    unsafe fn new_pool(&mut self, dev: &ID3D12Device, system_ok: bool) -> Option<u32> {
        let system = system_ok && system_pools(dev);
        let desc = D3D12_HEAP_DESC {
            SizeInBytes: POOL_PAGES as u64 * PAGE as u64,
            Properties: if system {
                // GPU-only system memory: the GPU reads it over PCIe.
                D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_CUSTOM, CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_NOT_AVAILABLE,
                    MemoryPoolPreference: D3D12_MEMORY_POOL_L0, CreationNodeMask: 1, VisibleNodeMask: 1 }
            } else {
                D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() }
            },
            Alignment: 0,
            // every kind of texture (resource heap tier 2: the only flags for both)
            Flags: D3D12_HEAP_FLAG_NONE,
        };
        let mut heap: Option<ID3D12Heap> = None;
        if let Err(e) = dev.CreateHeap(&desc, &mut heap) {
            crate::diagnostics::log_line(format_args!("D3D12.X: no video memory for more pages of title textures ({e})"));
            return None;
        }
        self.pools.push(heap?);
        self.pool_used.push(std::time::Instant::now());
        self.pool_evicted.push(false);
        self.pool_system.push(system);
        self.pool_empty.push(false);
        if system {
            SYSTEM_POOLS.fetch_add(1, Relaxed);
        }
        Some(self.pools.len() as u32 - 1)
    }

    /// Map the tiles of `res` from subresource `first` on (`tiles[i]`: the
    /// i-th tile from there in the resource's linear tile order) – one call
    /// per pool heap.
    unsafe fn map(&mut self, res: &ID3D12Resource, tiles: &[Tile], first: u32) {
        // `first` is the first mapped mip of a one-slice texture.
        let mut heaps: Vec<u32> = tiles.iter().map(|t| t.0).collect();
        heaps.sort_unstable();
        heaps.dedup();
        for h in heaps {
            // xcl 3bc7143: a trailing skipped run is left out, so the call walks
            // only up to the heap's last tile.
            let ranges = heap_ranges_trimmed(h, tiles);
            if ranges.is_empty() {
                continue;
            }
            let num_tiles: u32 = ranges.iter().map(|r| r.2).sum();
            let flags: Vec<D3D12_TILE_RANGE_FLAGS> = ranges.iter().map(|r| if r.0 { D3D12_TILE_RANGE_FLAG_NONE } else { D3D12_TILE_RANGE_FLAG_SKIP }).collect();
            let offsets: Vec<u32> = ranges.iter().map(|r| r.1).collect();
            let counts: Vec<u32> = ranges.iter().map(|r| r.2).collect();
            let coord = D3D12_TILED_RESOURCE_COORDINATE { Subresource: first, ..Default::default() };
            let region = D3D12_TILE_REGION_SIZE { NumTiles: num_tiles, UseBox: false.into(), ..Default::default() };
            self.queue.UpdateTileMappings(
                res,
                1,
                Some(&coord),
                Some(&region),
                &self.pools[h as usize],
                ranges.len() as u32,
                Some(flags.as_ptr()),
                Some(offsets.as_ptr()),
                Some(counts.as_ptr()),
                D3D12_TILE_MAPPING_FLAG_NONE,
            );
        }
        self.mapped();
    }

    /// A mapping was queued: title work after it waits for it.
    unsafe fn mapped(&mut self) {
        self.value += 1;
        if signal_each() {
            let _ = self.queue.Signal(&self.fence, self.value);
            self.signaled = self.value;
        }
    }

    /// Unmap every tile of `res` (`D3D12_TILE_RANGE_FLAG_NULL`) so its unshared tiles can return
    /// to the pool even while the title keeps the COM object alive.
    unsafe fn unmap(&mut self, res: &ID3D12Resource, total: u32) {
        let coord = D3D12_TILED_RESOURCE_COORDINATE::default();
        let region = D3D12_TILE_REGION_SIZE { NumTiles: total, UseBox: false.into(), ..Default::default() };
        let flag = D3D12_TILE_RANGE_FLAG_NULL;
        self.queue.UpdateTileMappings(
            res,
            1,
            Some(&coord),
            Some(&region),
            None,
            1,
            Some(&flag),
            None,
            Some(&total),
            D3D12_TILE_MAPPING_FLAG_NONE,
        );
        self.mapped();
    }

    fn release_tiles(&mut self, used: Vec<usize>, used_extra: Vec<(usize, usize)>, now: std::time::Instant, immediate_cool: bool) {
        for a in used {
            if let Some(p) = self.pages.get_mut(&a) {
                p.users = p.users.saturating_sub(1);
                if p.users == 0 {
                    if immediate_cool {
                        if let Some(p) = self.pages.remove(&a) {
                            self.pool.push_cooling(p.tile, now);
                        }
                    } else {
                        p.idle_since = Some(now);
                        self.idle.push_back((PageKey::Own(a), now));
                    }
                }
            }
        }
        for k in used_extra {
            if let Some(p) = self.extra_pages.get_mut(&k) {
                p.users = p.users.saturating_sub(1);
                if p.users == 0 {
                    if immediate_cool {
                        if let Some(p) = self.extra_pages.remove(&k) {
                            self.pool.push_cooling(p.tile, now);
                        }
                    } else {
                        p.idle_since = Some(now);
                        self.idle.push_back((PageKey::Extra(k.0, k.1), now));
                    }
                }
            }
        }
    }

    /// Acquire/share all `total` tiles for `res` at `va` and map them on the `Pages` queue.
    unsafe fn bind(&mut self, dev: &ID3D12Device, res: &ID3D12Resource, va: usize, console_bytes: Option<u64>, total: u32, system_ok: bool, (skip, levels): (u32, u32)) -> Option<(Vec<usize>, Vec<(usize, usize)>, Vec<u32>)> {
        let (first, own) = (total > 0).then(|| owned_pages(va, console_bytes, total as usize)).flatten()?;
        let now = std::time::Instant::now();
        self.sweep(now);
        // xcl 3bc7143: count the missing tiles first and take them as one batch,
        // from the heap of a page already shared when it fits them.
        let mut missing = 0usize;
        let mut preferred: Option<u32> = None;
        for i in skip as usize..total as usize {
            let existing = if i < own { self.pages.get(&(first + i * PAGE)).map(|p| p.tile.0) } else { self.extra_pages.get(&(va, i)).map(|p| p.tile.0) };
            match existing {
                Some(h) if preferred.is_none() => preferred = Some(h),
                Some(_) => {}
                None => missing += 1,
            }
        }
        let mut fresh = self.take_batch(dev, missing, preferred, system_ok, now)?.into_iter();
        let mut tiles = Vec::with_capacity(total as usize);
        let mut used = Vec::with_capacity(own);
        let mut used_extra = Vec::new();
        for i in skip as usize..total as usize {
            let tile = if i < own {
                let a = first + i * PAGE;
                let t = match self.pages.get_mut(&a) {
                    Some(p) => {
                        p.users += 1;
                        p.idle_since = None;
                        p.tile
                    }
                    None => {
                        let t = fresh.next().expect("one fresh tile per missing page");
                        self.pages.insert(a, Page { tile: t, users: 1, idle_since: None });
                        t
                    }
                };
                used.push(a);
                t
            } else {
                let k = (va, i);
                let t = match self.extra_pages.get_mut(&k) {
                    Some(p) => {
                        p.users += 1;
                        p.idle_since = None;
                        p.tile
                    }
                    None => {
                        let t = fresh.next().expect("one fresh tile per missing page");
                        self.extra_pages.insert(k, Page { tile: t, users: 1, idle_since: None });
                        t
                    }
                };
                used_extra.push(k);
                t
            };
            tiles.push(tile);
        }
        self.map(res, &tiles, levels);
        let mut heaps: Vec<u32> = tiles.iter().map(|t| t.0).collect();
        heaps.sort_unstable();
        heaps.dedup();
        Some((used, used_extra, heaps))
    }
}

/// The ranges of one UpdateTileMappings call for pool heap `heap` over a resource whose overall
/// tile i goes to `tiles[i]`: (mapped, tile offset in the heap, count) – runs of consecutive
/// tiles of the heap, and skipped runs for tiles of other heaps.
fn heap_ranges(heap: u32, tiles: &[Tile]) -> Vec<(bool, u32, u32)> {
    let mut out: Vec<(bool, u32, u32)> = Vec::new();
    for &(h, t) in tiles {
        let mine = h == heap;
        match out.last_mut() {
            Some(r) if mine && r.0 && r.1 + r.2 == t => r.2 += 1,
            Some(r) if !mine && !r.0 => r.2 += 1,
            _ => out.push((mine, if mine { t } else { 0 }, 1)),
        }
    }
    out
}

/// Like `heap_ranges`, with any trailing `SKIP` range trimmed so `UpdateTileMappings` only walks
/// up to the last tile belonging to `heap` (xcl 3bc7143).
fn heap_ranges_trimmed(heap: u32, tiles: &[Tile]) -> Vec<(bool, u32, u32)> {
    let mut out = heap_ranges(heap, tiles);
    while out.last().is_some_and(|r| !r.0) {
        out.pop();
    }
    out
}

/// 64 KiB page boundaries `P` with `va <= P < va + bytes`, capped at `total` tiles:
/// `(first_page, count)`. Because every `P` lies strictly inside `[va, va + bytes)`, two
/// non-overlapping ranges never share a page here even when `va` is only 4 KiB aligned.
fn owned_pages(va: usize, console_bytes: Option<u64>, total: usize) -> Option<(usize, usize)> {
    match console_bytes {
        Some(b) => {
            let bytes = usize::try_from(b).ok()?;
            let first = va.checked_next_multiple_of(PAGE)?;
            let end = va.saturating_add(bytes);
            let n = if first < end { (end - first).div_ceil(PAGE) } else { 0 };
            Some((first, n.min(total)))
        }
        None => (va % PAGE == 0).then_some((va, total)),
    }
}

/// Unmap a placed texture superseded by a placement with a different address range, returning its
/// unshared tiles (`extra_pages` and unshared tail pages) to the cooling pool immediately.
pub(super) unsafe fn unmap_superseded(res: &ID3D12Resource) {
    let key = res.as_raw() as usize;
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    for pages in g.values_mut().flatten() {
        let Some(t) = pages.targets.get_mut(&key) else { continue };
        if !t.mapped {
            return;
        }
        let total = t.total;
        let used = std::mem::take(&mut t.used);
        let used_extra = std::mem::take(&mut t.used_extra);
        t.mapped = false;
        pages.unmap(res, total);
        pages.release_tiles(used, used_extra, now, true);
        return;
    }
}

/// Re-map a previously unmapped placed texture when the newer placement shadowing it is released.
pub(super) unsafe fn remap_if_unmapped(res: &ID3D12Resource) -> bool {
    let mut dev: Option<ID3D12Device> = None;
    if res.GetDevice(&mut dev).is_err() {
        return false;
    }
    let Some(dev) = dev else { return false };
    let key = res.as_raw() as usize;
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(pages)) = g.get_mut(&(dev.as_raw() as usize)) else { return false };
    let Some((va, console_bytes, total, mapped, system_ok, skip)) = pages.targets.get(&key).map(|t| (t.va, t.console_bytes, t.total, t.mapped, t.system_ok, (t.skip, t.levels))) else {
        return false;
    };
    if mapped {
        return true;
    }
    let Some((used, used_extra, heaps)) = pages.bind(&dev, res, va, console_bytes, total, system_ok, skip) else {
        return false;
    };
    if let Some(t) = pages.targets.get_mut(&key) {
        t.used = used;
        t.used_extra = used_extra;
        t.heaps = heaps;
        t.mapped = true;
    }
    true
}

/// A resource released: its pages are no longer its.
fn forget_pages(resource: usize) {
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let now = std::time::Instant::now();
    for pages in g.values_mut().flatten() {
        let Some(t) = pages.targets.remove(&resource) else { continue };
        if t.skip != 0 {
            trimmed(resource, 0);
        }
        if t.mapped {
            pages.release_tiles(t.used, t.used_extra, now, false);
        }
        return;
    }
}

/// Before GPU work on `queue`: it waits for the pages mapped since its last work on that device.
pub(super) unsafe fn before_execute(queue: &ID3D12CommandQueue) {
    let mut dev: Option<ID3D12Device> = None;
    if queue.GetDevice(&mut dev).is_err() {
        return;
    }
    let Some(dev) = dev else { return };
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(pages)) = g.get_mut(&(dev.as_raw() as usize)) else { return };
    let key = queue.as_raw() as usize;
    if pages.signaled < pages.value {
        let _ = pages.queue.Signal(&pages.fence, pages.value);
        pages.signaled = pages.value;
    }
    if pages.waited.get(&key).copied().unwrap_or(0) < pages.value {
        let _ = queue.Wait(&pages.fence, pages.value);
        crate::diagnostics::queue_waits::note(windows::core::Interface::as_raw(queue) as usize, "tile mapping", &pages.fence, pages.value);
        pages.waited.insert(key, pages.value);
    }
}

/// (bytes of pages in use by tiled textures, bytes of pool heaps) – `XCL_ACTIVITY`
pub fn page_bytes() -> (usize, usize) {
    let g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    g.values().flatten().fold((0, 0), |(used, pools), p| {
        (used + (p.pages.len() + p.extra_pages.len()) * PAGE, pools + p.pools.len() * POOL_PAGES as usize * PAGE)
    })
}

/// Page pools by state, for the residency line: tiles mapped, free, cooling
/// and idle pages (last user gone, not yet swept).
pub fn pool_summary() -> String {
    let g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let mib = |tiles: usize| tiles * PAGE >> 20;
    let (mut pools, mut evicted, mut empty, mut system, mut free, mut cooling, mut used, mut idle) = (0, 0, 0, 0, 0, 0, 0, 0);
    for p in g.values().flatten() {
        pools += p.pools.len();
        evicted += p.pool_evicted.iter().filter(|&&e| e).count();
        empty += p.pool_empty.iter().filter(|&&e| e).count();
        system += p.pool_system.iter().filter(|&&e| e).count();
        free += p.pool.free_total;
        cooling += p.pool.cooling.len();
        used += p.pages.len() + p.extra_pages.len();
        idle += p.idle.len();
    }
    format!("page pools {pools} ({} MiB; evicted idle {evicted}, evicted empty {empty}, in system memory {system}): tiles in use {} MiB, free {} MiB, cooling {} MiB, idle pages {} MiB",
        mib(pools * POOL_PAGES as usize), mib(used), mib(free), mib(cooling), mib(idle))
}

static UNMAPPED: AtomicU64 = AtomicU64::new(0);
static UNMAPPED_BYTES: AtomicU64 = AtomicU64::new(0);
static RETURNED_BYTES: AtomicU64 = AtomicU64::new(0);
static UNMAPS: AtomicU64 = AtomicU64::new(0);
static REMAPS: AtomicU64 = AtomicU64::new(0);
static PLACED: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);

pub(super) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_PAGES") != Some("0")
}

/// A texture of this desktop description at guest va (console size
/// `console_bytes`) as a reserved resource on the title's pages; None: it
/// can't be (tier < 2, a desc the driver won't make reserved, no pages).
pub(super) fn place(device: &ID3D12Device, va: u64, host: &D3D12_RESOURCE_DESC, console_bytes: u64, clear: Option<&D3D12_CLEAR_VALUE>) -> Option<(ID3D12Resource, u64, Readers)> {
    if !enabled() || !super::alias::enabled() || host.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE1D || host.SampleDesc.Count > 1 {
        return None;
    }
    unsafe {
        let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
        let pages = g.entry(device.as_raw() as usize).or_insert_with(|| Pages::new(device)).as_mut()?;
        let tiled = D3D12_RESOURCE_DESC { Layout: D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE, Alignment: 0, ..*host };
        if !reservable(device, &tiled) {
            return None;
        }
        let system_ok = (host.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)).0 == 0;
        let (mut res, readers) = match recycled(device, &tiled) {
            Some((res, readers)) => (Some(res), readers),
            None => (None, Vec::new()),
        };
        if res.is_some() {
            RECYCLED.fetch_add(1, Relaxed);
        } else if let Err(e) = device.CreateReservedResource(&tiled, D3D12_RESOURCE_STATE_COMMON, clear.map(core::ptr::from_ref), &mut res) {
            if REFUSED.fetch_add(1, Relaxed) < 4 {
                crate::diagnostics::log_line(format_args!("placed textures: {}x{} format {} flags {:#x} not reserved ({e}): in a span",
                    host.Width, host.Height, host.Format.0, host.Flags.0));
            }
            return None;
        }
        let res = res?;
        crate::device::mark_reserved(&res);
        let mut total = 0u32;
        device.GetResourceTiling(&res, Some(&mut total), None, None, None, 0, std::ptr::null_mut());
        let (skip, levels) = if system_ok { trim_tiles(device, &res, host) } else { (0, 0) };
        let (used, used_extra, heaps) = pages.bind(device, &res, va as usize, Some(console_bytes), total, system_ok, (skip, levels))?;
        trimmed(res.as_raw() as usize, levels);
        if skip != 0 && TRIMS.fetch_add(1, Relaxed) % 1024 == 0 {
            let saved = TRIMMED_BYTES.fetch_add(u64::from(skip) * PAGE as u64, Relaxed) + u64::from(skip) * PAGE as u64;
            crate::diagnostics::log_line(format_args!("placed textures: {} trimmed to mip 1 or 2 while device local is over budget ({} MiB of top mips not mapped; this one from mip {levels})",
                TRIMS.load(Relaxed), saved >> 20));
        } else if skip != 0 {
            TRIMMED_BYTES.fetch_add(u64::from(skip) * PAGE as u64, Relaxed);
        }
        pages.targets.insert(res.as_raw() as usize, Target { va: va as usize, console_bytes: Some(console_bytes), total, mapped: true, used, used_extra, heaps, system_ok, skip, levels });
        PLACED.fetch_add(1, Relaxed);
        Some((res, u64::from(total) * PAGE as u64, readers))
    }
}

/// Sampled textures placed while device-local memory is over
/// TUXBOX_PAGES_TRIM_PCT (default 95) percent of its budget leave mip 0
/// unmapped, and their views start at mip 1 (ResourceMinLODClamp): a quarter
/// of the memory. test62 ran device local at 11711 of 7188 MiB, 7055 MiB of
/// it textures on pages, and hung with system commit exhausted. Only 2D
/// textures of one slice, at least 512x512, whose mip 1 is not packed;
/// unmapped tiles read zero and drop writes (tiled resources tier 2).
/// TUXBOX_PAGES_TRIM_PCT=0 maps every mip. test63 still ran at 8442 of 7188
/// MiB (render targets and images alone 6.6 GB), so above
/// TUXBOX_PAGES_TRIM2_PCT (110) mips 0 and 1 are both left unmapped (1/16).
/// Returns (tiles skipped, first mapped mip).
unsafe fn trim_tiles(device: &ID3D12Device, res: &ID3D12Resource, host: &D3D12_RESOURCE_DESC) -> (u32, u32) {
    let pct: u64 = crate::env_toggle!("TUXBOX_PAGES_TRIM_PCT").and_then(|v| v.trim().parse().ok()).unwrap_or(95);
    if pct == 0 || host.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D || host.DepthOrArraySize != 1
        || host.MipLevels < 2 || host.Width < 512 || host.Height < 512
    {
        return (0, 0);
    }
    let Some((usage, budget)) = super::alias::segment(device, true) else { return (0, 0) };
    if usage.saturating_mul(100) <= budget.saturating_mul(pct) {
        return (0, 0);
    }
    let deep: u64 = crate::env_toggle!("TUXBOX_PAGES_TRIM2_PCT").and_then(|v| v.trim().parse().ok()).unwrap_or(110);
    let mut count = 3u32;
    let mut tilings = [D3D12_SUBRESOURCE_TILING::default(); 3];
    let mut packed = D3D12_PACKED_MIP_INFO::default();
    unsafe { device.GetResourceTiling(res, None, Some(&mut packed), None, Some(&mut count), 0, tilings.as_mut_ptr()) };
    if count < 2 || tilings[0].StartTileIndexInOverallResource != 0 {
        return (0, 0);
    }
    // A mip whose tiles are packed (u32::MAX) can't start the mapping.
    let start = |level: usize| (level < count as usize && u32::from(packed.NumStandardMips) > level as u32)
        .then(|| tilings[level].StartTileIndexInOverallResource)
        .filter(|&s| s != 0 && s != u32::MAX);
    if deep != 0 && host.MipLevels >= 3 && usage.saturating_mul(100) > budget.saturating_mul(deep)
        && let Some(s) = start(2)
    {
        return (s, 2);
    }
    start(1).map_or((0, 0), |s| (s, 1))
}

static TRIMS: AtomicU64 = AtomicU64::new(0);
static TRIMMED_BYTES: AtomicU64 = AtomicU64::new(0);
static TRIMMED: Mutex<Option<HashMap<usize, u32>>> = Mutex::new(None);

/// Record the first mapped mip of `resource` (0: all mapped).
fn trimmed(resource: usize, levels: u32) {
    let mut g = TRIMMED.lock().unwrap_or_else(|e| e.into_inner());
    if levels != 0 {
        g.get_or_insert_with(Default::default).insert(resource, levels);
    } else if let Some(map) = g.as_mut() {
        map.remove(&resource);
    }
}

/// A Texture2D view of a native resource whose mip 0 was left unmapped
/// starts at mip 1.
pub(crate) fn clamp_view(resource: Option<&ID3D12Resource>, view: &mut D3D12_SHADER_RESOURCE_VIEW_DESC) {
    let Some(resource) = resource else { return };
    if view.ViewDimension != D3D12_SRV_DIMENSION_TEXTURE2D {
        return;
    }
    let key = resource.as_raw() as usize;
    let Some(levels) = TRIMMED.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key).copied()) else {
        return;
    };
    let t = unsafe { &mut view.Anonymous.Texture2D };
    // A view whose mips all lie in the unmapped ones is left as it is.
    if t.MostDetailedMip < levels && (t.MipLevels == u32::MAX || t.MostDetailedMip + t.MipLevels > levels) {
        t.ResourceMinLODClamp = t.ResourceMinLODClamp.max(levels as f32);
    }
}

/// The texture last uploaded or written over each guest page (by its first
/// page), for the ownership turns of textures sharing pages.
static OWNERS: Mutex<Option<HashMap<u64, Weak<Texture>>>> = Mutex::new(None);

/// As alias::claim for a texture on pages: None when it still owns every
/// page it covers, else the textures that held any (whose readers its full
/// re-upload waits for).
pub(super) fn claim(texture: &Arc<Texture>) -> Option<Vec<Arc<Texture>>> {
    let range = texture.range();
    let mut g = OWNERS.lock().unwrap_or_else(|e| e.into_inner());
    let owners = g.get_or_insert_with(HashMap::new);
    let mut lost = false;
    let mut previous = Vec::new();
    let mut page = range.start & !(PAGE as u64 - 1);
    while page < range.end.max(range.start + 1) {
        let mine = owners.get(&page).is_some_and(|t| core::ptr::eq(t.as_ptr(), Arc::as_ptr(texture)));
        if !mine {
            lost = true;
            if let Some(t) = owners.get(&page).and_then(Weak::upgrade) {
                previous.push(t);
            }
            owners.insert(page, Arc::downgrade(texture));
        }
        page += PAGE as u64;
    }
    if owners.len() > 1 << 20 {
        owners.retain(|_, t| t.strong_count() != 0);
    }
    lost.then_some(previous)
}

/// Unmaps a superseded texture on pages; true when it was mapped.
pub(super) fn unmap(texture: &Texture) -> bool {
    let key = texture.native.as_raw() as usize;
    let before = page_bytes().0;
    let was = PAGES.lock().unwrap_or_else(|e| e.into_inner()).values().flatten().any(|p| p.targets.get(&key).is_some_and(|t| t.mapped));
    if !was {
        return false;
    }
    unsafe { unmap_superseded(&texture.native) };
    UNMAPPED.fetch_add(1, Relaxed);
    UNMAPPED_BYTES.fetch_add(texture.bytes, Relaxed);
    UNMAPS.fetch_add(1, Relaxed);
    RETURNED_BYTES.fetch_add(before.saturating_sub(page_bytes().0) as u64, Relaxed);
    true
}

/// Maps an unmapped texture again; true when it was unmapped and now is mapped.
pub(super) fn remap(texture: &Texture) -> bool {
    let key = texture.native.as_raw() as usize;
    let was = PAGES.lock().unwrap_or_else(|e| e.into_inner()).values().flatten().any(|p| p.targets.get(&key).is_some_and(|t| !t.mapped));
    if !was || !unsafe { remap_if_unmapped(&texture.native) } {
        return false;
    }
    UNMAPPED.fetch_sub(1, Relaxed);
    UNMAPPED_BYTES.fetch_sub(texture.bytes, Relaxed);
    REMAPS.fetch_add(1, Relaxed);
    true
}

pub(super) fn is_mapped(texture: &Texture) -> bool {
    let key = texture.native.as_raw() as usize;
    PAGES.lock().unwrap_or_else(|e| e.into_inner()).values().flatten().any(|p| p.targets.get(&key).is_some_and(|t| t.mapped))
}

/// A texture that never came to be gives its pages back.
pub(super) fn forget(resource: usize) {
    forget_pages(resource);
}

/// A texture on pages is gone.
pub(super) fn gone(texture: &Texture, readers: Readers) {
    let key = texture.native.as_raw() as usize;
    if !is_mapped(texture) && PAGES.lock().unwrap_or_else(|e| e.into_inner()).values().flatten().any(|p| p.targets.contains_key(&key)) {
        UNMAPPED.fetch_sub(1, Relaxed);
        UNMAPPED_BYTES.fetch_sub(texture.bytes, Relaxed);
    }
    forget_pages(key);
    recycle(&texture.native, readers);
}

type Readers = Vec<(ID3D12Fence, u64)>;

// Copied from ivall/xcl crates/xcl/src/hle/d3d12x/vram.rs at 24d8b92 (with
// 38ea451), GPL-2.0-only as the rest of this file; the answer is cached per
// device, format and sample count, since place() runs hundreds of times a
// second.

/// A reserved texture needs TILED in the format's support (`support2`) and, multisampled, a
/// tiled quality level at its sample count.
fn can_reserve(support2: i32, samples: u32, tiled_msaa_levels: u32) -> bool {
    support2 & D3D12_FORMAT_SUPPORT2_TILED.0 != 0 && (samples <= 1 || tiled_msaa_levels > 0)
}

/// Whether the device can make `desc` a reserved texture, asked before CreateReservedResource:
/// refused, it is an error in the debug layer (#940: R32G8X24_TYPELESS depth, 1 and 4 samples).
/// A query that fails says yes, and creation decides.
unsafe fn reservable_uncached(dev: &ID3D12Device, desc: &D3D12_RESOURCE_DESC) -> bool {
    let mut f = D3D12_FEATURE_DATA_FORMAT_SUPPORT { Format: desc.Format, ..Default::default() };
    if dev.CheckFeatureSupport(D3D12_FEATURE_FORMAT_SUPPORT, &mut f as *mut _ as *mut c_void, std::mem::size_of_val(&f) as u32).is_err() {
        return true;
    }
    // a format without TILED is not reservable whatever the sample count's query says
    if !can_reserve(f.Support2.0, 1, 0) {
        return false;
    }
    let samples = desc.SampleDesc.Count;
    let mut m = D3D12_FEATURE_DATA_MULTISAMPLE_QUALITY_LEVELS { Format: desc.Format, SampleCount: samples, Flags: D3D12_MULTISAMPLE_QUALITY_LEVELS_FLAG_TILED_RESOURCE, NumQualityLevels: 0 };
    if samples > 1 && dev.CheckFeatureSupport(D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS, &mut m as *mut _ as *mut c_void, std::mem::size_of_val(&m) as u32).is_err() {
        return true;
    }
    can_reserve(f.Support2.0, samples, m.NumQualityLevels)
}

/// reservable_uncached, once per (device, format, samples). TUXBOX_PAGES_TILED_CHECK=0 skips it.
fn reservable(dev: &ID3D12Device, desc: &D3D12_RESOURCE_DESC) -> bool {
    if crate::env_toggle!("TUXBOX_PAGES_TILED_CHECK") == Some("0") {
        return true;
    }
    static KNOWN: Mutex<Option<HashMap<(usize, i32, u32), bool>>> = Mutex::new(None);
    let key = (dev.as_raw() as usize, desc.Format.0, desc.SampleDesc.Count);
    if let Some(&known) = KNOWN.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(&key)) {
        return known;
    }
    let ok = unsafe { reservable_uncached(dev, desc) };
    if !ok {
        crate::diagnostics::log_line(format_args!("placed textures: format {} ({} samples) has no tiled support: in a span", desc.Format.0, desc.SampleDesc.Count));
    }
    KNOWN.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(key, ok);
    ok
}

/// Released reserved resources by device and description, oldest first, for
/// the next placement of that description: test58 spent 16% of the render
/// thread making ~400 reserved resources a second in the kernel. A recycled
/// one has every tile mapped again before use, and its last readers are
/// waited on before its first upload, as pool.rs does for committed copies.
/// TUXBOX_PAGES_RECYCLE caps how many are kept (default 4096; 0 keeps none).
struct Recycled {
    device: usize,
    desc: D3D12_RESOURCE_DESC,
    native: ID3D12Resource,
    readers: Readers,
    bytes: u64,
    when: std::time::Instant,
}
unsafe impl Send for Recycled {}

/// test152: host-private commit grew 1.8 -> 9.6 GiB (4.6 GiB read-write)
/// with re-placements, while this queue, capped only by count, never filled
/// (2239 < 4096) and so freed nothing, keeping every varied description's
/// reserved resource and its driver bookkeeping. It is also capped by the
/// resources' allocation size, TUXBOX_PAGES_RECYCLE_MB (256; 0 count only),
/// and drops entries unused for TUXBOX_PAGES_RECYCLE_SECS (10; 0 never).
fn recycle_limits() -> (u64, u64) {
    static L: std::sync::OnceLock<(u64, u64)> = std::sync::OnceLock::new();
    *L.get_or_init(|| {
        let n = |v: Option<&str>, d: u64| v.and_then(|v| v.trim().parse().ok()).unwrap_or(d);
        (n(crate::env_toggle!("TUXBOX_PAGES_RECYCLE_MB"), 256) << 20, n(crate::env_toggle!("TUXBOX_PAGES_RECYCLE_SECS"), 10))
    })
}

static RECYCLE: Mutex<std::collections::VecDeque<Recycled>> = Mutex::new(std::collections::VecDeque::new());
static RECYCLED: AtomicU64 = AtomicU64::new(0);
static SYSTEM_POOLS: AtomicU64 = AtomicU64::new(0);
static POOL_EVICTIONS: AtomicU64 = AtomicU64::new(0);
static POOL_RESIDENCIES: AtomicU64 = AtomicU64::new(0);

fn recycle_cap() -> usize {
    crate::env_toggle!("TUXBOX_PAGES_RECYCLE").and_then(|v| v.trim().parse().ok()).unwrap_or(4096)
}

fn recycle(native: &ID3D12Resource, readers: Readers) {
    let cap = recycle_cap();
    if cap == 0 {
        return;
    }
    let mut device: Option<ID3D12Device> = None;
    if unsafe { native.GetDevice(&mut device) }.is_err() {
        return;
    }
    let Some(device) = device else { return };
    // Keyed as place() asks: the runtime may report the 64 KiB alignment.
    let desc = D3D12_RESOURCE_DESC { Alignment: 0, ..unsafe { native.GetDesc() } };
    let (max_bytes, secs) = recycle_limits();
    let bytes = if max_bytes != 0 { unsafe { device.GetResourceAllocationInfo(0, &[desc]) }.SizeInBytes } else { 0 };
    let now = std::time::Instant::now();
    let mut r = RECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    r.push_back(Recycled { device: device.as_raw() as usize, desc, native: native.clone(), readers, bytes, when: now });
    while r.len() > cap {
        r.pop_front();
    }
    if secs != 0 {
        while r.front().is_some_and(|e| now.duration_since(e.when).as_secs() >= secs) {
            r.pop_front();
        }
    }
    if max_bytes != 0 {
        let mut total: u64 = r.iter().map(|e| e.bytes).sum();
        while total > max_bytes && let Some(e) = r.pop_front() {
            total -= e.bytes;
        }
    }
}

fn recycled(device: &ID3D12Device, desc: &D3D12_RESOURCE_DESC) -> Option<(ID3D12Resource, Readers)> {
    let id = device.as_raw() as usize;
    let mut r = RECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    let at = r.iter().rposition(|e| e.device == id && e.desc == *desc)?;
    r.remove(at).map(|e| (e.native, e.readers))
}

/// Every mapping signals the fence on its own (TUXBOX_PAGES_SIGNAL_EACH=1),
/// instead of once before the next title work.
fn signal_each() -> bool {
    crate::env_toggle!("TUXBOX_PAGES_SIGNAL_EACH") == Some("1")
}

/// With TUXBOX_PAGES_SYSTEM_PCT=N, new pools for sampled textures go to
/// system memory while device-local usage is over N percent of its budget
/// (test58: device local 11403 of 7177 MiB, 7154 MiB of it pages; non-local
/// 3734 of 7437). Render targets, depth and UAV textures keep device pools.
/// Off by default (0): in test59 76 such pools (about 4.8 GiB) were charged
/// to system commit, which was down to 1 GiB free when the device was
/// removed.
fn system_pools(device: &ID3D12Device) -> bool {
    let pct: u64 = crate::env_toggle!("TUXBOX_PAGES_SYSTEM_PCT").and_then(|v| v.trim().parse().ok()).unwrap_or(0);
    if pct == 0 {
        return false;
    }
    let over = super::alias::segment(device, true).is_some_and(|(usage, budget)| usage.saturating_mul(100) > budget.saturating_mul(pct));
    static WAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if WAS.swap(over, Relaxed) != over {
        crate::diagnostics::log_line(format_args!("placed textures: device local {} {pct}% of its budget; new page pools for sampled textures go to {} memory (TUXBOX_PAGES_SYSTEM_PCT)",
            if over { "over" } else { "back under" }, if over { "system" } else { "device" }));
    }
    over
}

/// Before a submission: marks the pool heaps its textures on pages use,
/// makes the evicted ones resident again, and, at most once a second over
/// budget (TUXBOX_SPAN_EVICT_PCT), evicts the pools no submission used for
/// TUXBOX_SPAN_EVICT_S seconds (default 10; 0 evicts none), as alias.rs does
/// for spans. A pool's tiles handed to a new texture are made resident by
/// that texture's first submission.
///
/// A submission names only the textures it tracks, not those a shader
/// reaches through the descriptor heap, and a pool holds tiles of many
/// textures, so an evicted pool could still be sampled (a GPU page fault).
/// So idle pools are demoted instead: their residency priority drops to the
/// minimum, video memory moves them out first when over budget and pages
/// them back in when used, and a touched pool gets normal priority again.
/// Without it test60 ran device local at 11290 of 7188 MiB. TUXBOX_PAGES_EVICT=1
/// evicts instead (test59 lost its device after 104 evictions);
/// TUXBOX_PAGES_DEMOTE=0 leaves every pool alone.
pub(super) fn touch<'a>(device: &ID3D12Device, textures: impl Iterator<Item = &'a Texture>) -> windows::core::Result<()> {
    let after = super::alias::evict_after();
    let evicting = crate::env_toggle!("TUXBOX_PAGES_EVICT") == Some("1");
    if !enabled() || after <= 0.0 || (!evicting && crate::env_toggle!("TUXBOX_PAGES_DEMOTE") == Some("0")) {
        return Ok(());
    }
    let priority = |pools: &[usize], pages: &Pages, value: D3D12_RESIDENCY_PRIORITY| -> windows::core::Result<()> {
        let device1 = device.cast::<ID3D12Device1>()?;
        let list: Vec<Option<ID3D12Pageable>> = pools.iter().filter_map(|&h| pages.pools[h].cast().ok()).map(Some).collect();
        let values = vec![value; list.len()];
        unsafe { device1.SetResidencyPriority(list.len() as u32, list.as_ptr(), values.as_ptr()) }
    };
    let now = std::time::Instant::now();
    let mut g = PAGES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(Some(pages)) = g.get_mut(&(device.as_raw() as usize)) else { return Ok(()) };
    let mut keep = vec![false; pages.pools.len()];
    let mut resident = Vec::new();
    for t in textures {
        let Some(target) = pages.targets.get(&(t.native.as_raw() as usize)) else { continue };
        for &h in &target.heaps {
            let h = h as usize;
            if keep[h] {
                continue;
            }
            keep[h] = true;
            pages.pool_used[h] = now;
            if pages.pool_evicted[h] {
                pages.pool_evicted[h] = false;
                resident.push(h);
            }
        }
    }
    let evict = |pages: &mut Pages, keep: &[bool], idle: f64, need: Option<usize>| -> usize {
        let mut cold: Vec<usize> = (0..pages.pools.len())
            .filter(|&h| !keep[h] && !pages.pool_evicted[h] && now.duration_since(pages.pool_used[h]).as_secs_f64() >= idle)
            .collect();
        cold.sort_by_key(|&h| pages.pool_used[h]);
        if let Some(n) = need {
            cold.truncate(n);
        }
        if cold.is_empty() {
            return 0;
        }
        let list: Vec<Option<ID3D12Pageable>> = cold.iter().filter_map(|&h| pages.pools[h].cast().ok()).map(Some).collect();
        let done = if evicting { unsafe { device.Evict(&list) } } else { priority(&cold, pages, D3D12_RESIDENCY_PRIORITY_MINIMUM) };
        if let Err(e) = done {
            crate::diagnostics::log_line(format_args!("placed textures: {} of {} page pools failed {e}", if evicting { "Evict" } else { "demoting" }, cold.len()));
            return 0;
        }
        for &h in &cold {
            pages.pool_evicted[h] = true;
        }
        let n = POOL_EVICTIONS.fetch_add(cold.len() as u64, Relaxed);
        if n < 8 || (n + cold.len() as u64).ilog2() != (n + 1).ilog2() {
            crate::diagnostics::log_line(format_args!("placed textures: {} {} page pools ({} MiB) unused for {idle:.0} s (TUXBOX_SPAN_EVICT_S, TUXBOX_PAGES_DEMOTE)",
                if evicting { "evicted" } else { "demoted" }, cold.len(), cold.len() * POOL_PAGES as usize * PAGE >> 20));
        }
        cold.len()
    };
    if !resident.is_empty() {
        let list: Vec<Option<ID3D12Pageable>> = resident.iter().filter_map(|&h| pages.pools[h].cast().ok()).map(Some).collect();
        if !evicting {
            if let Err(e) = priority(&resident, pages, D3D12_RESIDENCY_PRIORITY_NORMAL) {
                crate::diagnostics::log_line(format_args!("placed textures: restoring the priority of {} page pools failed {e}", resident.len()));
            }
        }
        let mut made = if evicting { unsafe { device.MakeResident(&list) } } else { Ok(()) };
        if made.is_err() {
            evict(pages, &keep, 1.0, Some(resident.len()));
            made = unsafe { device.MakeResident(&list) };
        }
        POOL_RESIDENCIES.fetch_add(resident.len() as u64, Relaxed);
        if let Err(e) = made {
            crate::diagnostics::log_line(format_args!("placed textures: MakeResident of {} page pools failed {e}", resident.len()));
            return Err(e);
        }
    }
    static PASS: Mutex<Option<std::time::Instant>> = Mutex::new(None);
    let due = {
        let mut pass = PASS.lock().unwrap_or_else(|e| e.into_inner());
        let due = pass.is_none_or(|at| now.duration_since(at).as_secs_f64() >= 1.0);
        if due {
            *pass = Some(now);
        }
        due
    };
    if due {
        let over = super::alias::over_budget(device);
        if over && compact_on() {
            unsafe { pages.compact(device, &keep, after) };
        }
        unsafe { pages.trim_empty(device, over) };
        if over {
            evict(pages, &keep, after, None);
        }
    }
    Ok(())
}

pub(crate) struct Stats {
    pub(crate) recycled: u64,
    pub(crate) system_pools: u64,
    pub(crate) evicted_pools: u64,
    pub(crate) pool_evictions: u64,
    pub(crate) pool_residencies: u64,
    pub(crate) placed: u64,
    pub(crate) in_use: u64,
    pub(crate) pools: u64,
    pub(crate) unmapped: u64,
    pub(crate) unmapped_bytes: u64,
    pub(crate) returned_bytes: u64,
    pub(crate) unmaps: u64,
    pub(crate) remaps: u64,
}

pub(crate) fn stats() -> Stats {
    let (in_use, pools) = page_bytes();
    let evicted_pools = PAGES.lock().unwrap_or_else(|e| e.into_inner()).values().flatten()
        .map(|p| p.pool_evicted.iter().filter(|&&e| e).count() as u64).sum();
    Stats {
        recycled: RECYCLED.load(Relaxed),
        system_pools: SYSTEM_POOLS.load(Relaxed),
        evicted_pools,
        pool_evictions: POOL_EVICTIONS.load(Relaxed),
        pool_residencies: POOL_RESIDENCIES.load(Relaxed),
        placed: PLACED.load(Relaxed),
        in_use: in_use as u64,
        pools: pools as u64,
        unmapped: UNMAPPED.load(Relaxed),
        unmapped_bytes: UNMAPPED_BYTES.load(Relaxed),
        returned_bytes: RETURNED_BYTES.load(Relaxed),
        unmaps: UNMAPS.load(Relaxed),
        remaps: REMAPS.load(Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const MIB: usize = 1 << 20;

    /// One call per heap: its tiles in runs, the other heaps' skipped.
    #[test]
    fn tile_ranges_per_heap() {
        let tiles = [(0, 5), (0, 6), (1, 0), (1, 1), (0, 9), (0, 10), (0, 11)];
        assert_eq!(heap_ranges(0, &tiles), [(true, 5, 2), (false, 0, 2), (true, 9, 3)]);
        assert_eq!(heap_ranges(1, &tiles), [(false, 0, 2), (true, 0, 2), (false, 0, 3)]);
        // shared pages out of order: runs break where the tiles do
        assert_eq!(heap_ranges(0, &[(0, 3), (0, 1), (0, 2)]), [(true, 3, 1), (true, 1, 2)]);
        assert_eq!(heap_ranges_trimmed(1, &tiles), [(false, 0, 2), (true, 0, 2)]);
    }

    /// xcl 3bc7143's test: streaming churn across 100 pool heaps. Cooling tiles returned in
    /// interleaved cross-heap order are regrouped per heap and sorted into contiguous ascending
    /// runs, so new textures stay inside 1 pool heap (1 UpdateTileMappings call, 1 range).
    /// Added: a cold (evicted-empty) heap is used only when no warm heap has a tile.
    #[test]
    fn tile_pool_keeps_churned_allocations_in_single_heap_and_contiguous() {
        let t0 = Instant::now();
        let mut pool = TilePool::default();
        let heaps_made = std::cell::Cell::new(0u32);
        let make = || {
            let h = heaps_made.get();
            heaps_made.set(h + 1);
            Some(h)
        };
        const P: u32 = 64;
        let mut live: Vec<Vec<Tile>> = Vec::new();
        for _ in 0..100 {
            live.push(pool.take_batch(P as usize, None, &[], t0, P, true, make).unwrap());
        }
        assert_eq!(heaps_made.get(), 100);
        assert_eq!(pool.free_total, 0);
        for tile_idx in 0..32usize {
            for h in 0..100usize {
                pool.push_cooling(live[h][tile_idx], t0);
            }
        }
        let t1 = t0 + COOL + Duration::from_millis(1);
        for _ in 0..100 {
            let alloc = pool.take_batch(32, None, &[], t1, P, true, make).unwrap();
            let h0 = alloc[0].0;
            assert!(alloc.iter().all(|&(h, _)| h == h0), "texture scattered across multiple heaps");
            assert_eq!(heap_ranges_trimmed(h0, &alloc), [(true, 0, 32)]);
        }
        assert_eq!(heaps_made.get(), 100, "must reuse cooled tiles without making new heaps");
        for t in 10..18u32 {
            pool.push_cooling((7, t), t1);
            pool.push_cooling((3, t), t1);
        }
        let t2 = t1 + COOL + Duration::from_millis(1);
        let pref = pool.take_batch(8, Some(7), &[], t2, P, true, make).unwrap();
        assert!(pref.iter().all(|&(h, _)| h == 7), "preferred heap should be chosen");
        assert_eq!(heap_ranges_trimmed(7, &pref), [(true, 10, 8)]);
        // heap 3 is cold: with a warm heap free it is skipped
        pool.push_cooling((5, 40), t2);
        let mut cold = vec![false; 100];
        cold[3] = true;
        let t3 = t2 + COOL + Duration::from_millis(1);
        assert_eq!(pool.take_batch(1, None, &cold, t3, P, true, make).unwrap(), [(5, 40)]);
        // rollback when no heap can be made
        assert_eq!(pool.free_total, 8);
        assert!(pool.take_batch(16, None, &cold, t3, P, true, || None).is_none());
        assert_eq!(pool.free_total, 8, "OOM rollback must return all popped tiles");
    }

    /// Idle pages go back after `IDLE`, and a sweep looks at the queue of idle pages alone: one
    /// used again stays, one idle again later goes when that is `IDLE` ago, and the sweeps of
    /// 20 000 placements beside 200 000 pages in use take no time to speak of (a look at every
    /// page at each of them: 4 000 000 000 looks).
    #[test]
    fn idle_pages_are_swept_from_their_queue() {
        use std::collections::{HashMap, VecDeque};
        let t0 = Instant::now();
        let at = |ms: u64| t0 + Duration::from_millis(ms);
        let mut idle: VecDeque<(PageKey, Instant)> = VecDeque::new();
        let mut pages: HashMap<usize, Page> = (0..200_000usize).map(|i| (i * PAGE, Page { tile: (0, i as u32), users: 1, idle_since: None })).collect();
        let mut extra: HashMap<(usize, usize), Page> = HashMap::from([((7 * PAGE, 3), Page { tile: (1, 0), users: 1, idle_since: None })]);
        let mut cooling: VecDeque<(Tile, Instant)> = VecDeque::new();
        // what `release_tiles` and `bind` do to a page
        fn release(p: &mut Page, idle: &mut VecDeque<(PageKey, Instant)>, key: PageKey, when: Instant) {
            (p.users, p.idle_since) = (0, Some(when));
            idle.push_back((key, when));
        }
        fn use_again(p: &mut Page) {
            (p.users, p.idle_since) = (1, None);
        }
        // pages 1, 2, 3 idle at 0 ms and an extra page at 100 ms; page 2 is used again at
        // 500 ms; page 3 too, and is idle again at 1500 ms
        for a in [PAGE, 2 * PAGE, 3 * PAGE] {
            release(pages.get_mut(&a).unwrap(), &mut idle, PageKey::Own(a), at(0));
        }
        release(extra.get_mut(&(7 * PAGE, 3)).unwrap(), &mut idle, PageKey::Extra(7 * PAGE, 3), at(100));
        use_again(pages.get_mut(&(2 * PAGE)).unwrap());
        use_again(pages.get_mut(&(3 * PAGE)).unwrap());
        release(pages.get_mut(&(3 * PAGE)).unwrap(), &mut idle, PageKey::Own(3 * PAGE), at(1500));

        sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(1900));
        assert!(cooling.is_empty() && pages.len() == 200_000 && idle.len() == 5, "nothing is idle for IDLE yet");
        sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(2050));
        assert_eq!(cooling.iter().map(|c| c.0).collect::<Vec<_>>(), [(0, 1)], "page 1 alone: 2 is in use, 3 idle since 1500 ms");
        assert!(!pages.contains_key(&PAGE) && pages.contains_key(&(2 * PAGE)) && pages.contains_key(&(3 * PAGE)) && extra.len() == 1);
        sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(2150));
        assert_eq!(cooling.iter().map(|c| c.0).collect::<Vec<_>>(), [(0, 1), (1, 0)], "the extra page, idle since 100 ms");
        assert!(extra.is_empty() && pages.len() == 199_999);
        sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(3499));
        assert!(pages.contains_key(&(3 * PAGE)));
        sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(3500));
        assert_eq!(cooling.iter().map(|c| c.0).collect::<Vec<_>>(), [(0, 1), (1, 0), (0, 3)]);
        assert!(idle.is_empty() && pages.len() == 199_998 && pages[&(2 * PAGE)].users == 1);

        // every placement sweeps: with nothing idle that is nothing, whatever is in use
        let t = Instant::now();
        for k in 0..20_000u64 {
            sweep_idle(&mut idle, &mut pages, &mut extra, &mut cooling, at(4000 + k));
        }
        assert!(t.elapsed() < Duration::from_secs(2), "{:?} for 20 000 sweeps", t.elapsed());
        assert_eq!(pages.len(), 199_998);
    }

    /// Any 64 KiB page boundary inside `[va, va + bytes)` belongs to that range alone, so
    /// non-overlapping ranges never share an owned page even when only 4 KiB aligned.
    #[test]
    fn owned_pages_never_alias_disjoint_ranges() {
        assert_eq!(owned_pages(0x1_0000, Some(0x1_0000), 4), Some((0x1_0000, 1)));
        assert_eq!(owned_pages(0x1_0000, Some(0x1_4000), 4), Some((0x1_0000, 2)));
        assert_eq!(owned_pages(0x1_4000, Some(0x4000), 4), Some((0x2_0000, 0)));
        assert_eq!(owned_pages(0x1_4000, Some(0x1_0000), 4), Some((0x2_0000, 1)));
        assert_eq!(owned_pages(0x1_4000, Some(0x4_0000), 4), Some((0x2_0000, 4)));
        assert_eq!(owned_pages(0x1_4000, None, 4), None);
        // Exhaustively check 4 KiB-aligned disjoint pairs over 256 KiB.
        const K4: usize = 0x1000;
        for a in (0..16).map(|i| 0x10_0000 + i * K4) {
            for sa in [K4, 4 * K4, 16 * K4, 20 * K4] {
                let (fa, na) = owned_pages(a, Some(sa as u64), 8).unwrap();
                let pa: Vec<usize> = (0..na).map(|i| fa + i * PAGE).collect();
                for b in (0..16).map(|i| 0x10_0000 + i * K4) {
                    for sb in [K4, 4 * K4, 16 * K4, 20 * K4] {
                        if a + sa <= b || b + sb <= a {
                            let (fb, nb) = owned_pages(b, Some(sb as u64), 8).unwrap();
                            for j in 0..nb {
                                assert!(!pa.contains(&(fb + j * PAGE)), "disjoint [{a:#x}..{:#x}) and [{b:#x}..{:#x}) shared page", a + sa, b + sb);
                            }
                        }
                    }
                }
            }
        }
    }

}
