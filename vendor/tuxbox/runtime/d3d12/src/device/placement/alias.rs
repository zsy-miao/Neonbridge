// Span aliasing for placed textures.
//
// Copyright (C) omonkey (https://github.com/omonkeyz/xbox-emulator)
// Copyright (C) Neonbridge contributors
//
// Ported from omonkeyz/xbox-emulator crates/compat/d3d12_xs/src/alias.rs
// (commit 27122f7): the span registry (find, insert, retire_overlapping,
// resource_gone, take_idle), local_span, oversize_span and the GPU-local
// path of create_placed_x. Their raw vtable calls are typed windows-rs
// calls here; the title-heap (OpenExistingHeapFromAddress) and shadow
// kinds are left out, since this path places only textures.
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version. This program is distributed in the hope that it will be
// useful, but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General
// Public License for more details: <https://www.gnu.org/licenses/>.

//! Placed textures as real placed resources over shared heaps. test49 ran
//! out of device memory with 23756 live placed textures (7930 MiB), one
//! committed copy each, although the title keeps most of them over the
//! same guest memory. A span is one GPU-local heap standing for a guest
//! VA range [lo, hi); a texture at va is placed at (span heap, va - lo),
//! so every texture over the same guest bytes shares the same heap bytes,
//! as on the console. A new span retires the older spans it overlaps, and
//! a span's heap is released once no texture placed in it is alive.
//!
//! Textures sharing heap bytes take turns: the last one uploaded or
//! written owns them, and another is fully re-uploaded from guest memory,
//! after the owners' GPU readers, before its next use.
//! TUXBOX_PLACED_ALIAS=0 makes one committed copy per placement as before.
use super::Texture;
use core::ffi::c_void;
use core::ops::Range;
use std::collections::{BTreeMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Instant;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Interface;

pub(super) const PLACEMENT_ALIGN: u64 = 0x1_0000;
const LOCAL_SPAN_CAP: u64 = 64 << 20;
const LOCAL_WINDOW: u64 = 4 << 20;
const MEM_FREE: u32 = 0x1_0000;

pub(super) struct Span {
    pub(super) lo: u64,
    pub(super) hi: u64,
    device: usize,
    heap: Option<ID3D12Heap>,
    heap_bytes: u64,
    /// In system memory (a custom L0 heap), made under device-memory pressure.
    system: bool,
    pub(super) retired: bool,
    /// Evicted (ID3D12Device::Evict) after going unused under memory
    /// pressure; made resident again before a submission that uses it.
    evicted: bool,
    /// The last submission or placement that used it.
    used: Instant,
    pub(super) live: HashSet<usize>,
    /// Heap offsets and the texture whose bytes they hold now.
    owners: Vec<(Range<u64>, Weak<Texture>)>,
}

impl Span {
    fn id(&self) -> usize {
        self.heap.as_ref().map_or(0, |h| h.as_raw() as usize)
    }
}

#[derive(Default)]
pub(super) struct Spans {
    by_lo: BTreeMap<u64, Vec<Span>>,
}

impl Spans {
    #[cfg(test)]
    pub(super) fn find(&self, device: usize, va: u64, len: u64, align: u64) -> Option<(u64, usize)> {
        self.find_where(device, va, len, align, |_| true)
    }

    pub(super) fn find_where(&self, device: usize, va: u64, len: u64, align: u64, accept: impl Fn(&Span) -> bool) -> Option<(u64, usize)> {
        let end = va.checked_add(len.max(1))?;
        for (lo, v) in self.by_lo.range(..=va).rev().take(64) {
            for (i, s) in v.iter().enumerate() {
                if exact() {
                    // xbox-emulator alias.rs:76 `find`: the texture sits at
                    // exactly va - lo, so a span serves it only when that
                    // offset is aligned; never below the guest VA.
                    if s.device == device && !s.retired && end <= s.hi && (va - lo) % align.max(1) == 0 && accept(s) {
                        return Some((*lo, i));
                    }
                    continue;
                }
                let offset = (va - lo) / align.max(1) * align.max(1);
                if s.device == device && !s.retired && end - (va - lo - offset) <= s.hi && accept(s) {
                    return Some((*lo, i));
                }
            }
        }
        None
    }

    pub(super) fn get(&self, key: (u64, usize)) -> &Span {
        &self.by_lo[&key.0][key.1]
    }

    pub(super) fn get_mut(&mut self, key: (u64, usize)) -> &mut Span {
        self.by_lo.get_mut(&key.0).unwrap().get_mut(key.1).unwrap()
    }

    fn by_id(&mut self, lo: u64, id: usize) -> Option<&mut Span> {
        self.by_lo.get_mut(&lo)?.iter_mut().find(|s| s.id() == id)
    }

    pub(super) fn insert(&mut self, s: Span) -> (u64, usize) {
        let lo = s.lo;
        let v = self.by_lo.entry(lo).or_default();
        v.push(s);
        (lo, v.len() - 1)
    }

    /// Retires the spans [lo, hi) covers whole: a new span over them
    /// serves every placement they could. Retiring every overlapping span
    /// (test50) made each new span retire its neighbours, so the next
    /// placement in their range made yet another heap, while the title kept
    /// the old textures alive: 2078 span heaps (6691 MiB) in 126 s.
    pub(super) fn retire_covered(&mut self, device: usize, lo: u64, hi: u64) -> usize {
        let mut n = 0;
        for (_, v) in self.by_lo.range_mut(lo..hi) {
            for s in v.iter_mut() {
                if s.device == device && !s.retired && s.hi <= hi {
                    s.retired = true;
                    n += 1;
                }
            }
        }
        n
    }

    /// Retired spans and their heap bytes (still held by live textures).
    fn retired(&self) -> (u64, u64) {
        self.by_lo.values().flatten().filter(|s| s.retired).fold((0, 0), |(n, b), s| (n + 1, b + s.heap_bytes))
    }

    pub(super) fn retire_overlapping(&mut self, device: usize, lo: u64, hi: u64) -> usize {
        let mut n = 0;
        for (_, v) in self.by_lo.range_mut(..hi) {
            for s in v.iter_mut() {
                if s.device == device && !s.retired && s.hi > lo {
                    s.retired = true;
                    n += 1;
                }
            }
        }
        n
    }

    pub(super) fn resource_gone(&mut self, res: usize) {
        for v in self.by_lo.values_mut() {
            for s in v.iter_mut() {
                s.live.remove(&res);
            }
        }
    }

    /// Removes the spans no live texture uses and returns their heaps, for
    /// the caller to release after the registry lock.
    pub(super) fn take_idle(&mut self) -> Vec<Span> {
        let mut out = Vec::new();
        self.by_lo.retain(|_, v| {
            let mut kept = Vec::with_capacity(v.len());
            for s in v.drain(..) {
                if s.live.is_empty() { out.push(s) } else { kept.push(s) }
            }
            *v = kept;
            !v.is_empty()
        });
        out
    }

    pub(super) fn len(&self) -> usize {
        self.by_lo.values().map(|v| v.len()).sum()
    }
}

static SPANS: Mutex<Spans> = Mutex::new(Spans { by_lo: BTreeMap::new() });
static GONE: Mutex<Vec<usize>> = Mutex::new(Vec::new());
static SPAN_COUNT: AtomicU64 = AtomicU64::new(0);
static SPAN_BYTES: AtomicU64 = AtomicU64::new(0);
static PLACED: AtomicU64 = AtomicU64::new(0);
static SHARED: AtomicU64 = AtomicU64::new(0);
/// Committed fallbacks by reason, named in REASONS.
static FALLBACK: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
const REASONS: [&str; 6] = ["MSAA", "tier-1 heaps", "alignment over 64 KiB", "no allocation size", "heap not created", "placement failed"];
const MSAA: usize = 0;
const TIER1: usize = 1;
const ALIGN: usize = 2;
const SIZE: usize = 3;
const HEAP: usize = 4;
const PLACE: usize = 5;
static RETIRED: AtomicU64 = AtomicU64::new(0);
static SYSTEM_COUNT: AtomicU64 = AtomicU64::new(0);
static SYSTEM_BYTES: AtomicU64 = AtomicU64::new(0);
static RETIRED_BYTES: AtomicU64 = AtomicU64::new(0);
static EVICTED_COUNT: AtomicU64 = AtomicU64::new(0);
static EVICTED_BYTES: AtomicU64 = AtomicU64::new(0);
static EVICTIONS: AtomicU64 = AtomicU64::new(0);
static RESIDENCIES: AtomicU64 = AtomicU64::new(0);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(super) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_PLACED_ALIAS") != Some("0")
}

/// Placement as xbox-emulator alias.rs:76/103 does it (GPL-3.0, credit to
/// omonkey): place only at an aligned va - lo, retire every overlapping span.
/// TUXBOX_ALIAS_EXACT=0 places at the aligned offset below the VA and retires
/// only covered spans, as before.
pub(super) fn exact() -> bool {
    crate::env_toggle!("TUXBOX_ALIAS_EXACT") != Some("0")
}

/// Resource heap tier 2: one heap holds render targets and sampled textures.
fn mixed_heaps(device: &ID3D12Device) -> bool {
    static TIER2: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *TIER2.get_or_init(|| {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        let ok = unsafe { device.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS, (&raw mut options).cast(), size_of_val(&options) as u32) }.is_ok()
            && options.ResourceHeapTier.0 >= D3D12_RESOURCE_HEAP_TIER_2.0;
        if !ok {
            crate::diagnostics::log_line(format_args!("placed textures: resource heap tier 1, one committed copy per placement"));
        }
        ok
    })
}

pub(super) fn round_up(v: u64, a: u64) -> u64 {
    v.div_ceil(a) * a
}

/// The guest VA range a new span over va covers: the whole reservation when
/// it is small, else a window from va's 64 KiB page.
pub(super) fn local_span(va: u64, need: u64, reservation: Option<(u64, u64)>) -> (u64, u64) {
    let need = need.max(1);
    let lo = va & !(PLACEMENT_ALIGN - 1);
    let hi_min = round_up(va + need, PLACEMENT_ALIGN);
    let Some((base, end)) = reservation.filter(|(b, e)| *b <= va && hi_min <= *e && b % PLACEMENT_ALIGN == 0) else {
        return (lo, hi_min);
    };
    if end - base <= LOCAL_SPAN_CAP {
        return (base, end);
    }
    (lo, (lo + need.max(LOCAL_WINDOW)).min(end).max(hi_min))
}

/// A texture larger on PC than in its console layout gets its own heap
/// standing only for its console range, so later placements after it never
/// share its PC tail.
pub(super) fn oversize_span(va: u64, pc_size: u64, xbox_size: u64) -> Option<(u64, u64)> {
    let title = round_up(xbox_size, PLACEMENT_ALIGN);
    (xbox_size != 0 && pc_size > title).then(|| (va, va + title))
}

/// rebox's idea (re-derived from 90b6578/a85c353): the walk starts at va's
/// own region instead of the allocation base (every region between them is
/// the same allocation), and stops once local_span can only clip to its window,
/// past LOCAL_SPAN_CAP and a window beyond va + need. Walking from the base
/// of a reservation of thousands of 64 KiB views cost a VirtualQuery per
/// view on every fresh span. TUXBOX_RESERVATION_WALK_ALL=1 walks to its end.
fn reservation(va: u64, need: u64) -> Option<(u64, u64)> {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let query = |a: u64| {
        let mut m = MEMORY_BASIC_INFORMATION::default();
        (unsafe { VirtualQuery(Some(a as *const c_void), &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0).then_some(m)
    };
    let first = query(va)?;
    if first.State.0 == MEM_FREE {
        return None;
    }
    let base = first.AllocationBase as u64;
    let all = crate::env_toggle!("TUXBOX_RESERVATION_WALK_ALL") == Some("1");
    let enough = (va & !(PLACEMENT_ALIGN - 1)).saturating_add(need.max(LOCAL_WINDOW)).max(base.saturating_add(LOCAL_SPAN_CAP + 1));
    let (mut p, mut end) = if all { (base, base) } else { (first.BaseAddress as u64, first.BaseAddress as u64) };
    for _ in 0..65536 {
        match query(p) {
            Some(m) if m.AllocationBase as u64 == base && m.State.0 != MEM_FREE => {
                end = m.BaseAddress as u64 + m.RegionSize as u64;
                p = end;
                if !all && end >= enough {
                    break;
                }
            }
            _ => break,
        }
    }
    (end > base).then_some((base, end))
}

/// Device-local usage over TUXBOX_PLACED_SYSTEM_PCT (default 80) percent of
/// the budget, read at most every 250 ms. test52/53 ran out of device memory
/// with 22-24k live placed textures (7-8 GiB) in span heaps on a ~7 GiB
/// budget; on the console they live in title memory. Over the line, new
/// sampled-texture spans go to system memory (a custom L0 heap the GPU reads
/// over PCIe); render targets and depth stay in device memory. The title's
/// own pages (OpenExistingHeapFromAddress, alias.rs Kind::Title) hold only
/// buffers and row-major textures, so they can't take tiled textures.
/// TUXBOX_PLACED_SYSTEM_PCT=0 keeps every span in device memory.
fn adapter(device: &ID3D12Device) -> Option<&'static windows::Win32::Graphics::Dxgi::IDXGIAdapter3> {
    use std::sync::OnceLock;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter3, IDXGIFactory4};
    static ADAPTER: OnceLock<Option<IDXGIAdapter3>> = OnceLock::new();
    ADAPTER.get_or_init(|| {
        let luid = unsafe { device.GetAdapterLuid() };
        unsafe { CreateDXGIFactory1::<IDXGIFactory4>() }.ok()
            .and_then(|f| unsafe { f.EnumAdapterByLuid::<IDXGIAdapter>(luid) }.ok())
            .and_then(|a| a.cast().ok())
    }).as_ref()
}

/// A memory segment group's usage and budget.
pub(super) fn segment(device: &ID3D12Device, local: bool) -> Option<(u64, u64)> {
    use windows::Win32::Graphics::Dxgi::{DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO};
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    let group = if local { DXGI_MEMORY_SEGMENT_GROUP_LOCAL } else { DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL };
    (unsafe { adapter(device)?.QueryVideoMemoryInfo(0, group, &mut info) }.is_ok() && info.Budget != 0).then_some((info.CurrentUsage, info.Budget))
}

fn under_pressure(device: &ID3D12Device) -> bool {
    let pct: u64 = crate::env_toggle!("TUXBOX_PLACED_SYSTEM_PCT").and_then(|v| v.trim().parse().ok()).unwrap_or(80);
    if pct == 0 {
        return false;
    }
    static CHECKED: Mutex<Option<(std::time::Instant, bool)>> = Mutex::new(None);
    let mut checked = lock(&CHECKED);
    if let Some((at, over)) = *checked && at.elapsed().as_millis() < 250 {
        return over;
    }
    let (usage, budget) = segment(device, true).unwrap_or((0, 0));
    let over = budget != 0 && usage.saturating_mul(100) > budget.saturating_mul(pct);
    let info = windows::Win32::Graphics::Dxgi::DXGI_QUERY_VIDEO_MEMORY_INFO { CurrentUsage: usage, Budget: budget, ..Default::default() };
    if over != checked.is_some_and(|(_, was)| was) {
        crate::diagnostics::log_line(format_args!("placed textures: device local {} of {} MiB is {} {pct}%; new sampled-texture spans go to {} memory (TUXBOX_PLACED_SYSTEM_PCT)",
            info.CurrentUsage >> 20, info.Budget >> 20, if over { "over" } else { "back under" }, if over { "system" } else { "device" }));
    }
    *checked = Some((std::time::Instant::now(), over));
    over
}

fn create_heap(device: &ID3D12Device, size: u64, system: bool) -> Option<ID3D12Heap> {
    let properties = if system {
        D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_CUSTOM, CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_COMBINE,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0, CreationNodeMask: 1, VisibleNodeMask: 1 }
    } else {
        D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() }
    };
    let desc = D3D12_HEAP_DESC {
        SizeInBytes: round_up(size, PLACEMENT_ALIGN),
        Properties: properties,
        Alignment: PLACEMENT_ALIGN,
        Flags: D3D12_HEAP_FLAG_NONE,
    };
    let mut heap: Option<ID3D12Heap> = None;
    if let Err(e) = unsafe { device.CreateHeap(&desc, &mut heap) } {
        static N: AtomicU64 = AtomicU64::new(0);
        if N.fetch_add(1, Relaxed) < 8 {
            crate::diagnostics::log_line(format_args!("placed textures: CreateHeap({:#x}{}) failed {e}", desc.SizeInBytes, if system { ", system memory" } else { "" }));
        }
        return None;
    }
    if let Some(heap) = &heap {
        crate::diagnostics::memory::live_on(heap, crate::diagnostics::memory::Category::PlacedSpanHeap, desc.SizeInBytes);
    }
    SPAN_COUNT.fetch_add(1, Relaxed);
    SPAN_BYTES.fetch_add(desc.SizeInBytes, Relaxed);
    if system {
        SYSTEM_COUNT.fetch_add(1, Relaxed);
        SYSTEM_BYTES.fetch_add(desc.SizeInBytes, Relaxed);
    }
    heap
}

fn housekeeping(sp: &mut Spans) -> Vec<Span> {
    for r in core::mem::take(&mut *lock(&GONE)) {
        sp.resource_gone(r);
    }
    let idle = sp.take_idle();
    for s in &idle {
        SPAN_COUNT.fetch_sub(1, Relaxed);
        SPAN_BYTES.fetch_sub(s.heap_bytes, Relaxed);
        if s.evicted {
            EVICTED_COUNT.fetch_sub(1, Relaxed);
            EVICTED_BYTES.fetch_sub(s.heap_bytes, Relaxed);
        }
        if s.system {
            SYSTEM_COUNT.fetch_sub(1, Relaxed);
            SYSTEM_BYTES.fetch_sub(s.heap_bytes, Relaxed);
        }
    }
    let (n, bytes) = sp.retired();
    RETIRED.store(n, Relaxed);
    RETIRED_BYTES.store(bytes, Relaxed);
    idle
}

/// Where a texture sits: its span (by start and heap) and heap bytes.
pub(super) struct Placed {
    lo: u64,
    heap: usize,
    pub(super) bytes: Range<u64>,
    native: usize,
}

/// Places a texture of this desktop description at guest va in a span
/// heap; None keeps the committed copy (a desc that needs MSAA alignment,
/// tier 1 heaps, a failed heap or placement).
pub(super) fn place(
    device: &ID3D12Device,
    va: u64,
    host: &D3D12_RESOURCE_DESC,
    xbox_size: u64,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Option<(ID3D12Resource, u64, Placed)> {
    if !enabled() || va == 0 {
        return None;
    }
    let made = if host.SampleDesc.Count > 1 {
        Err(MSAA)
    } else if !mixed_heaps(device) {
        Err(TIER1)
    } else {
        place_locked(device, va, host, xbox_size, clear)
    };
    made.map_err(|reason| FALLBACK[reason].fetch_add(1, Relaxed)).ok()
}

fn place_locked(
    device: &ID3D12Device,
    va: u64,
    host: &D3D12_RESOURCE_DESC,
    xbox_size: u64,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Result<(ID3D12Resource, u64, Placed), usize> {
    let mut desc = *host;
    let mut info = D3D12_RESOURCE_ALLOCATION_INFO::default();
    let target = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
    if desc.Flags & target == D3D12_RESOURCE_FLAGS(0) && va.is_multiple_of(0x1000) {
        let small = D3D12_RESOURCE_DESC { Alignment: 0x1000, ..desc };
        let si = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(&small)) };
        if si.Alignment == 0x1000 && si.SizeInBytes != u64::MAX {
            desc = small;
            info = si;
        }
    }
    if info.Alignment == 0 {
        info = unsafe { device.GetResourceAllocationInfo(0, std::slice::from_ref(&desc)) };
    }
    let (size, align) = (info.SizeInBytes, info.Alignment);
    if size == 0 || size == u64::MAX || align == 0 {
        return Err(SIZE);
    }
    if align > PLACEMENT_ALIGN {
        return Err(ALIGN);
    }
    let id = device.as_raw() as usize;
    let mut idle;
    let made = {
        let mut sp = lock(&SPANS);
        idle = housekeeping(&mut sp);
        // Render targets and depth stay in device memory.
        let target = host.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL) != D3D12_RESOURCE_FLAGS(0);
        let system = !target && under_pressure(device);
        let fresh_span = |sp: &mut Spans, lo: u64, hi: u64, bytes: u64| {
            let (heap, system) = match create_heap(device, bytes, system) {
                Some(heap) => (heap, system),
                None if system => (create_heap(device, bytes, false).ok_or(HEAP)?, false),
                None => return Err(HEAP),
            };
            // xbox-emulator alias.rs:103: a new span retires every span it overlaps.
            if exact() { sp.retire_overlapping(id, lo, hi) } else { sp.retire_covered(id, lo, hi) };
            Ok::<_, usize>(sp.insert(Span { lo, hi, device: id, heap: Some(heap), heap_bytes: round_up(bytes, PLACEMENT_ALIGN), system,
                retired: false, evicted: false, used: Instant::now(), live: HashSet::new(), owners: Vec::new() }))
        };
        // With TUXBOX_ALIAS_EXACT=0 a va finer than the alignment is placed at the aligned offset below
        // it: textures over the same guest bytes still share heap bytes, and
        // claim() orders any overlap, exact or not.
        let (key, fresh) = match (oversize_span(va, size, xbox_size), sp.find_where(id, va, size, align, |s| !(target && s.system))) {
            (Some((lo, hi)), _) => (fresh_span(&mut sp, lo, hi, size)?, true),
            (None, Some(k)) => (k, false),
            (None, None) => {
                let (lo, hi) = local_span(va, size.max(xbox_size), reservation(va, size.max(xbox_size)));
                (fresh_span(&mut sp, lo, hi, hi - lo)?, true)
            }
        };
        let span = sp.get_mut(key);
        span.used = Instant::now();
        let offset = if exact() { va - span.lo } else { (va - span.lo) / align * align };
        let mut native: Option<ID3D12Resource> = None;
        let heap = span.heap.as_ref().ok_or(PLACE)?;
        let hr = unsafe { device.CreatePlacedResource(heap, offset, &desc, D3D12_RESOURCE_STATE_COMMON, clear.map(core::ptr::from_ref), &mut native) };
        match (hr, native) {
            (Ok(()), Some(native)) => {
                let raw = native.as_raw() as usize;
                span.live.insert(raw);
                let placed = Placed { lo: span.lo, heap: span.id(), bytes: offset..offset + size, native: raw };
                let spans = sp.len();
                Ok((native, fresh, placed, spans))
            }
            (hr, _) => {
                static N: AtomicU64 = AtomicU64::new(0);
                if N.fetch_add(1, Relaxed) < 16 {
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: CreatePlacedResource(heap +{offset:#x}) for va={va:#x} {}x{} format {} failed {:?}; committed copy instead",
                        desc.Width, desc.Height, desc.Format.0, hr.err()));
                }
                idle.extend(housekeeping(&mut sp));
                Err(PLACE)
            }
        }
    };
    drop(idle);
    let (native, fresh, placed, spans) = made?;
    let total = PLACED.fetch_add(1, Relaxed) + 1;
    let fallbacks: u64 = FALLBACK.iter().map(|f| f.load(Relaxed)).sum();
    if !fresh {
        SHARED.fetch_add(1, Relaxed);
    }
    if total <= 8 || total.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: va={va:#x} -> span [{:#x}, ..) +{:#x}{} | placed {total}, sharing a span {}, committed fallbacks {}, spans live {spans}",
            placed.lo, placed.bytes.start, if fresh { " (new heap)" } else { "" }, SHARED.load(Relaxed), fallbacks));
    }
    Ok((native, size, placed))
}

/// Seconds a span goes unused before it may be evicted under memory
/// pressure: TUXBOX_SPAN_EVICT_S (default 10; 0 evicts none).
pub(super) fn evict_after() -> f64 {
    use std::sync::OnceLock;
    static S: OnceLock<f64> = OnceLock::new();
    *S.get_or_init(|| crate::env_toggle!("TUXBOX_SPAN_EVICT_S").and_then(|v| v.trim().parse().ok()).unwrap_or(10.0))
}

/// Device-local or non-local usage over TUXBOX_SPAN_EVICT_PCT (default 90)
/// percent of its budget.
pub(super) fn over_budget(device: &ID3D12Device) -> bool {
    let pct: u64 = crate::env_toggle!("TUXBOX_SPAN_EVICT_PCT").and_then(|v| v.trim().parse().ok()).unwrap_or(90);
    [true, false].into_iter().filter_map(|local| segment(device, local))
        .any(|(usage, budget)| usage.saturating_mul(100) > budget.saturating_mul(pct))
}

fn pageables(heaps: &[ID3D12Heap]) -> Vec<Option<ID3D12Pageable>> {
    heaps.iter().filter_map(|h| h.cast::<ID3D12Pageable>().ok()).map(Some).collect()
}

/// Evicts the spans unused for `after` seconds that `keep` doesn't name,
/// oldest first, until about `need` bytes are evicted (all when None).
fn evict_idle(device: &ID3D12Device, after: f64, keep: &HashSet<(u64, usize)>, need: Option<u64>) -> u64 {
    let id = device.as_raw() as usize;
    let mut sp = lock(&SPANS);
    let mut idle: Vec<&mut Span> = sp.by_lo.values_mut().flatten()
        .filter(|s| s.device == id && !s.evicted && s.heap.is_some() && !keep.contains(&(s.lo, s.id()))
            && s.used.elapsed().as_secs_f64() >= after)
        .collect();
    idle.sort_by_key(|s| s.used);
    let mut heaps = Vec::new();
    let mut bytes = 0;
    for s in idle {
        if need.is_some_and(|n| bytes >= n) {
            break;
        }
        s.evicted = true;
        bytes += s.heap_bytes;
        heaps.push(s.heap.clone().unwrap());
    }
    if heaps.is_empty() {
        return 0;
    }
    let list = pageables(&heaps);
    // Evict is synchronous with the device and doesn't touch the GPU's queue;
    // the spans' last use is at least `after` seconds old.
    if let Err(e) = unsafe { device.Evict(&list) } {
        crate::diagnostics::log_line(format_args!("placed textures: Evict of {} span heaps failed {e}", heaps.len()));
        for (_, v) in sp.by_lo.iter_mut() {
            for s in v.iter_mut().filter(|s| s.heap.as_ref().is_some_and(|h| heaps.contains(h))) {
                s.evicted = false;
            }
        }
        return 0;
    }
    EVICTED_COUNT.fetch_add(heaps.len() as u64, Relaxed);
    EVICTED_BYTES.fetch_add(bytes, Relaxed);
    let n = EVICTIONS.fetch_add(heaps.len() as u64, Relaxed);
    if n < 8 || (n + heaps.len() as u64).ilog2() != (n + 1).ilog2() {
        crate::diagnostics::log_line(format_args!("placed textures: evicted {} span heaps ({} MiB) unused for {after:.0} s (TUXBOX_SPAN_EVICT_S, TUXBOX_SPAN_EVICT_PCT)",
            heaps.len(), bytes >> 20));
    }
    bytes
}

/// Before a submission: marks the spans its textures sit in as used, makes
/// the evicted ones resident again, and, at most once a second under memory
/// pressure, evicts the spans gone unused for evict_after() seconds.
pub(super) fn touch<'a>(device: &ID3D12Device, placed: impl Iterator<Item = &'a Placed>) -> windows::core::Result<()> {
    let after = evict_after();
    if !enabled() || after <= 0.0 {
        return Ok(());
    }
    let now = Instant::now();
    let mut keep = HashSet::new();
    let mut resident = Vec::new();
    let mut bytes = 0;
    {
        let mut sp = lock(&SPANS);
        for p in placed {
            if !keep.insert((p.lo, p.heap)) {
                continue;
            }
            if let Some(s) = sp.by_id(p.lo, p.heap) {
                s.used = now;
                if s.evicted && let Some(h) = &s.heap {
                    s.evicted = false;
                    bytes += s.heap_bytes;
                    resident.push(h.clone());
                }
            }
        }
    }
    if !resident.is_empty() {
        let list = pageables(&resident);
        let mut made = unsafe { device.MakeResident(&list) };
        if made.is_err() {
            // Make room from spans this submission doesn't use, then retry.
            evict_idle(device, 1.0, &keep, Some(bytes));
            made = unsafe { device.MakeResident(&list) };
        }
        EVICTED_COUNT.fetch_sub(resident.len() as u64, Relaxed);
        EVICTED_BYTES.fetch_sub(bytes, Relaxed);
        RESIDENCIES.fetch_add(resident.len() as u64, Relaxed);
        if let Err(e) = made {
            crate::diagnostics::log_line(format_args!("placed textures: MakeResident of {} span heaps ({} MiB) failed {e}", resident.len(), bytes >> 20));
            return Err(e);
        }
    }
    static PASS: Mutex<Option<Instant>> = Mutex::new(None);
    let due = {
        let mut pass = lock(&PASS);
        let due = pass.is_none_or(|at| now.duration_since(at).as_secs_f64() >= 1.0);
        if due {
            *pass = Some(now);
        }
        due
    };
    if due && over_budget(device) {
        evict_idle(device, after, &keep, None);
    }
    Ok(())
}

/// A span-placed texture is gone: its span is released, after the registry
/// lock, once no other texture uses it.
pub(super) fn gone(placed: &Placed) {
    lock(&GONE).push(placed.native);
    let idle = match SPANS.try_lock() {
        Ok(mut sp) => housekeeping(&mut sp),
        Err(_) => Vec::new(),
    };
    drop(idle);
}

/// Takes the heap bytes of a texture for its next upload or write. Returns
/// None when it still owns them, else the textures that held any of them
/// (possibly none), whose GPU readers the upload waits for; the caller
/// re-uploads every level and drops these after its own locks.
pub(super) fn claim(texture: &Arc<Texture>, placed: &Placed) -> Option<Vec<Arc<Texture>>> {
    let mut sp = lock(&SPANS);
    let span = sp.by_id(placed.lo, placed.heap)?;
    let mine = &placed.bytes;
    if span.owners.iter().any(|(r, t)| r.start <= mine.start && mine.end <= r.end && core::ptr::eq(t.as_ptr(), Arc::as_ptr(texture))) {
        return None;
    }
    let mut previous = Vec::new();
    span.owners.retain(|(r, t)| {
        let overlap = r.start < mine.end && mine.start < r.end;
        if overlap && let Some(t) = t.upgrade() && !Arc::ptr_eq(&t, texture) {
            previous.push(t);
        }
        !overlap && t.strong_count() != 0
    });
    span.owners.push((mine.clone(), Arc::downgrade(texture)));
    drop(sp);
    Some(previous)
}

pub(crate) struct Stats {
    pub(crate) spans: u64,
    pub(crate) span_bytes: u64,
    pub(crate) placed: u64,
    pub(crate) shared: u64,
    /// Committed fallbacks by reason, e.g. "MSAA 3, placement failed 1".
    pub(crate) fallback: String,
    pub(crate) retired: u64,
    pub(crate) retired_bytes: u64,
    pub(crate) system: u64,
    pub(crate) system_bytes: u64,
    pub(crate) evicted: u64,
    pub(crate) evicted_bytes: u64,
    pub(crate) evictions: u64,
    pub(crate) residencies: u64,
}

pub(crate) fn stats() -> Stats {
    Stats {
        spans: SPAN_COUNT.load(Relaxed),
        span_bytes: SPAN_BYTES.load(Relaxed),
        placed: PLACED.load(Relaxed),
        shared: SHARED.load(Relaxed),
        fallback: {
            let parts: Vec<String> = REASONS.iter().zip(&FALLBACK).map(|(r, f)| (r, f.load(Relaxed)))
                .filter(|(_, n)| *n != 0).map(|(r, n)| format!("{r} {n}")).collect();
            if parts.is_empty() { "none".into() } else { parts.join(", ") }
        },
        retired: RETIRED.load(Relaxed),
        retired_bytes: RETIRED_BYTES.load(Relaxed),
        system: SYSTEM_COUNT.load(Relaxed),
        system_bytes: SYSTEM_BYTES.load(Relaxed),
        evicted: EVICTED_COUNT.load(Relaxed),
        evicted_bytes: EVICTED_BYTES.load(Relaxed),
        evictions: EVICTIONS.load(Relaxed),
        residencies: RESIDENCIES.load(Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(lo: u64, hi: u64) -> Span {
        Span { lo, hi, device: 1, heap: None, heap_bytes: 0, system: false, retired: false, evicted: false, used: Instant::now(), live: HashSet::new(), owners: Vec::new() }
    }

    #[test]
    fn same_va_resolves_to_same_heap_and_offset() {
        let mut s = Spans::default();
        let k = s.insert(span(0x10_0000, 0x50_0000));
        assert_eq!(s.find(1, 0x10_0000, 0x1000, 0x10000), Some(k));
        assert_eq!(s.find(1, 0x20_0000, 0x10000, 0x10000), Some(k));
        // A finer va lands at the aligned offset below it.
        assert_eq!(s.find(1, 0x20_1000, 0x10000, 0x10000), Some(k));
        assert_eq!(s.find(1, 0x4f_1000, 0x10000, 0x10000), Some(k));
        assert_eq!(s.find(1, 0x4f_0000, 0x20000, 0x10000), None);
        assert_eq!(s.find(2, 0x10_0000, 0x1000, 0x10000), None);
        s.get_mut(k).live.insert(7);
        s.get_mut(k).live.insert(8);
        s.resource_gone(7);
        assert!(s.take_idle().is_empty());
        s.resource_gone(8);
        assert_eq!(s.take_idle().len(), 1);
        assert_eq!(s.len(), 0);
    }

    #[test]
    fn a_new_span_retires_the_ones_it_overlaps() {
        let mut s = Spans::default();
        let a = s.insert(span(0x10_0000, 0x20_0000));
        let b = s.insert(span(0x30_0000, 0x40_0000));
        assert_eq!(s.retire_overlapping(1, 0x18_0000, 0x28_0000), 1);
        assert!(s.get(a).retired && !s.get(b).retired);
        assert_eq!(s.find(1, 0x10_0000, 0x1000, 0x1000), None);
    }

    #[test]
    fn a_new_span_retires_only_the_ones_it_covers() {
        let mut s = Spans::default();
        let a = s.insert(span(0x10_0000, 0x20_0000));
        let b = s.insert(span(0x18_0000, 0x30_0000));
        assert_eq!(s.retire_covered(1, 0x10_0000, 0x28_0000), 1);
        assert!(s.get(a).retired && !s.get(b).retired);
        assert_eq!(s.retired().0, 1);
    }

    #[test]
    fn local_span_covers_the_reservation() {
        assert_eq!(local_span(0x12_3000, 0x1000, Some((0x10_0000, 0x20_0000))), (0x10_0000, 0x20_0000));
        assert_eq!(local_span(0x12_3000, 0x1000, None), (0x12_0000, 0x13_0000));
        let big = Some((0x1000_0000, 0x2000_0000));
        assert_eq!(local_span(0x1234_5000, 0x1000, big), (0x1234_0000, 0x1274_0000));
        assert_eq!(oversize_span(0x10_0000, 0x3_0000, 0x1_8000), Some((0x10_0000, 0x12_0000)));
        assert_eq!(oversize_span(0x10_0000, 0x2_0000, 0x1_8000), None);
    }
}
