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
}

/// The title's pages of video memory for tiled textures (see the module's comment).
struct Pages {
    queue: ID3D12CommandQueue,
    /// signalled after each mapping; title queues wait for it before their next work
    fence: ID3D12Fence,
    value: u64,
    pools: Vec<ID3D12Heap>,
    free: Vec<Tile>,
    /// tiles given back, reusable after `COOL`
    cooling: std::collections::VecDeque<(Tile, std::time::Instant)>,
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
}

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
            free: Vec::new(),
            cooling: std::collections::VecDeque::new(),
            pages: std::collections::HashMap::new(),
            extra_pages: std::collections::HashMap::new(),
            idle: std::collections::VecDeque::new(),
            targets: std::collections::HashMap::new(),
            waited: std::collections::HashMap::new(),
            signaled: 0,
            pool_used: Vec::new(),
            pool_evicted: Vec::new(),
            pool_system: Vec::new(),
        })
    }

    /// Pages no resource used for `IDLE` go back to the pools.
    fn sweep(&mut self, now: std::time::Instant) {
        sweep_idle(&mut self.idle, &mut self.pages, &mut self.extra_pages, &mut self.cooling, now);
    }

    unsafe fn take_tile(&mut self, dev: &ID3D12Device, system_ok: bool) -> Option<Tile> {
        if let Some(t) = self.free.pop() {
            return Some(t);
        }
        if self.cooling.front().is_some_and(|(_, at)| at.elapsed() >= COOL) {
            return self.cooling.pop_front().map(|(t, _)| t);
        }
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
        if system {
            SYSTEM_POOLS.fetch_add(1, Relaxed);
        }
        let h = self.pools.len() as u32 - 1;
        self.free.extend((1..POOL_PAGES).rev().map(|t| (h, t)));
        Some((h, 0))
    }

    /// Map every tile of `res` (`tiles[i]`: overall tile i) – one call per pool heap.
    unsafe fn map(&mut self, res: &ID3D12Resource, tiles: &[Tile]) {
        let mut heaps: Vec<u32> = tiles.iter().map(|t| t.0).collect();
        heaps.sort_unstable();
        heaps.dedup();
        for h in heaps {
            let ranges = heap_ranges(h, tiles);
            let flags: Vec<D3D12_TILE_RANGE_FLAGS> = ranges.iter().map(|r| if r.0 { D3D12_TILE_RANGE_FLAG_NONE } else { D3D12_TILE_RANGE_FLAG_SKIP }).collect();
            let offsets: Vec<u32> = ranges.iter().map(|r| r.1).collect();
            let counts: Vec<u32> = ranges.iter().map(|r| r.2).collect();
            let coord = D3D12_TILED_RESOURCE_COORDINATE::default();
            let region = D3D12_TILE_REGION_SIZE { NumTiles: tiles.len() as u32, UseBox: false.into(), ..Default::default() };
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
                            self.cooling.push_back((p.tile, now));
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
                            self.cooling.push_back((p.tile, now));
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
    unsafe fn bind(&mut self, dev: &ID3D12Device, res: &ID3D12Resource, va: usize, console_bytes: Option<u64>, total: u32, system_ok: bool) -> Option<(Vec<usize>, Vec<(usize, usize)>, Vec<u32>)> {
        let (first, own) = (total > 0).then(|| owned_pages(va, console_bytes, total as usize)).flatten()?;
        let now = std::time::Instant::now();
        self.sweep(now);
        let mut tiles = Vec::with_capacity(total as usize);
        let mut used = Vec::with_capacity(own);
        let mut used_extra = Vec::new();
        for i in 0..total as usize {
            let tile = if i < own {
                let a = first + i * PAGE;
                let tile = match self.pages.get_mut(&a) {
                    Some(p) => {
                        p.users += 1;
                        p.idle_since = None;
                        Some(p.tile)
                    }
                    None => self.take_tile(dev, system_ok).inspect(|&t| {
                        self.pages.insert(a, Page { tile: t, users: 1, idle_since: None });
                    }),
                };
                if tile.is_some() {
                    used.push(a);
                }
                tile
            } else {
                let k = (va, i);
                let tile = match self.extra_pages.get_mut(&k) {
                    Some(p) => {
                        p.users += 1;
                        p.idle_since = None;
                        Some(p.tile)
                    }
                    None => self.take_tile(dev, system_ok).inspect(|&t| {
                        self.extra_pages.insert(k, Page { tile: t, users: 1, idle_since: None });
                    }),
                };
                if tile.is_some() {
                    used_extra.push(k);
                }
                tile
            };
            let Some(t) = tile else { break };
            tiles.push(t);
        }
        if tiles.len() < total as usize {
            self.release_tiles(used, used_extra, now, true);
            return None;
        }
        self.map(res, &tiles);
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
    let Some((va, console_bytes, total, mapped, system_ok)) = pages.targets.get(&key).map(|t| (t.va, t.console_bytes, t.total, t.mapped, t.system_ok)) else {
        return false;
    };
    if mapped {
        return true;
    }
    let Some((used, used_extra, heaps)) = pages.bind(&dev, res, va, console_bytes, total, system_ok) else {
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
        let mut total = 0u32;
        device.GetResourceTiling(&res, Some(&mut total), None, None, None, 0, std::ptr::null_mut());
        let (used, used_extra, heaps) = pages.bind(device, &res, va as usize, Some(console_bytes), total, system_ok)?;
        pages.targets.insert(res.as_raw() as usize, Target { va: va as usize, console_bytes: Some(console_bytes), total, mapped: true, used, used_extra, heaps, system_ok });
        PLACED.fetch_add(1, Relaxed);
        Some((res, u64::from(total) * PAGE as u64, readers))
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
}
unsafe impl Send for Recycled {}

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
    let mut r = RECYCLE.lock().unwrap_or_else(|e| e.into_inner());
    r.push_back(Recycled { device: device.as_raw() as usize, desc, native: native.clone(), readers });
    while r.len() > cap {
        r.pop_front();
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
    if due && super::alias::over_budget(device) {
        evict(pages, &keep, after, None);
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
