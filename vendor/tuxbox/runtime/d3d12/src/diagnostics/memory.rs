//! Memory stats every TUXBOX_MEM_STATS_MS (default 10000; 0 off): device
//! memory budget and usage (DXGI QueryVideoMemoryInfo, local and
//! non-local), this process's private commit, the system's free commit,
//! and what our own fallbacks created per category since the last line and
//! in all. test44 ran out of memory at 104 s with only 4455 xmem views, so
//! the growth is device or commit memory; a category that grows every
//! interval points at it.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use windows::Win32::Foundation::LUID;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3, IDXGIFactory4};
use windows::core::Interface;

#[derive(Clone, Copy)]
pub(crate) enum Category {
    ShadowCopy,
    ShadowImport,
    DeviceBuffer,
    ImageInDeviceMemory,
    CommittedResource,
    PlacedLayout,
    RenderHeap,
    HeapImport,
    PlacedTexture,
    DescTable,
    PlacedUpload,
    PlacedReadback,
    PlacedSpanHeap,
}

const N: usize = 13;
const NAMES: [&str; N] = ["shadow copies", "shadow imports", "device buffers", "images in device memory", "committed resources", "slot 133 placements", "render heaps", "heap imports", "placed textures (committed)", "descriptor table buffers", "placed texture upload buffers", "placed texture readback buffers", "placed texture span heaps"];
static COUNTS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static BYTES: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

pub(crate) fn note(category: Category, bytes: u64) {
    COUNTS[category as usize].fetch_add(1, Relaxed);
    BYTES[category as usize].fetch_add(bytes, Relaxed);
}

fn interval_ms() -> u64 {
    crate::env_toggle!("TUXBOX_MEM_STATS_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(10_000)
}

#[repr(C)]
#[derive(Default)]
struct ProcessMemoryCounters {
    cb: u32,
    page_faults: u32,
    peak_working_set: usize,
    working_set: usize,
    quota_peak_paged: usize,
    quota_paged: usize,
    quota_peak_nonpaged: usize,
    quota_nonpaged: usize,
    pagefile: usize,
    peak_pagefile: usize,
    private: usize,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCounters, cb: u32) -> i32;
    fn GetCurrentProcess() -> isize;
}

fn mib(bytes: u64) -> u64 {
    bytes >> 20
}

/// One stats line from the given readings (pure, for the test).
fn line(local: (u64, u64), non_local: (u64, u64), private: u64, free_commit: u64, total_commit: u64, deltas: &[(u64, u64, u64, u64)]) -> String {
    let categories: Vec<String> = NAMES.iter().zip(deltas)
        .filter(|(_, d)| d.2 != 0)
        .map(|(name, &(dc, db, tc, tb))| format!("{name} +{dc} (+{} MiB) of {tc} ({} MiB)", mib(db), mib(tb)))
        .collect();
    let (live, dead) = (crate::device::shadow_bytes().0, crate::device::shadow_bytes().1);
    let placed = crate::device::placement::pool::stats();
    let spans = crate::device::placement::alias::stats();
    let paged = crate::device::placement::pages::stats();
    format!("memory: device local {}/{} MiB, non-local {}/{} MiB (usage/budget); process private {} MiB; system free commit {} of {} MiB; shadow copies live {} MiB, dead kept {} MiB (TUXBOX_SHADOW_DEAD_MB caps); placed textures live {} ({} MiB), pooled {} ({} MiB, TUXBOX_PLACED_TEXTURE_POOL_MB caps), reused {}, of the live superseded by a newer placement {} ({} MiB); span heaps live {} ({} MiB, TUXBOX_PLACED_ALIAS=0 off), of those in system memory {} ({} MiB, TUXBOX_PLACED_SYSTEM_PCT), evicted {} ({} MiB; evictions {}, made resident again {}, TUXBOX_SPAN_EVICT_S), retired but held by live textures {} ({} MiB), placed in spans {}, on 64 KiB pages {} (pages in use {} MiB of pools {} MiB, TUXBOX_PLACED_PAGES=0 off), superseded and unmapped {} ({} MiB; unmaps {}, remaps {}, {} MiB of pages returned), of those sharing a span {}, committed fallbacks: {}; created: {}",
        mib(local.1), mib(local.0), mib(non_local.1), mib(non_local.0), mib(private), mib(free_commit), mib(total_commit), mib(live), mib(dead),
        placed.live, mib(placed.live_bytes), placed.pooled, mib(placed.pooled_bytes), placed.reused, placed.superseded, mib(placed.superseded_bytes),
        spans.spans, mib(spans.span_bytes), spans.system, mib(spans.system_bytes), spans.evicted, mib(spans.evicted_bytes), spans.evictions, spans.residencies, spans.retired, mib(spans.retired_bytes), spans.placed, paged.placed, mib(paged.in_use), mib(paged.pools), paged.unmapped, mib(paged.unmapped_bytes), paged.unmaps, paged.remaps, mib(paged.returned_bytes), spans.shared, spans.fallback,
        if categories.is_empty() { "nothing".into() } else { categories.join(", ") })
}

/// Starts the stats thread once, for the adapter with this LUID.
pub(crate) fn start(luid: LUID) {
    static ONCE: std::sync::Once = std::sync::Once::new();
    let every = interval_ms();
    if every == 0 { return; }
    ONCE.call_once(|| {
        let (low, high) = (luid.LowPart, luid.HighPart);
        let _ = std::thread::Builder::new().name("TuxBox memory stats".into()).spawn(move || {
            let luid = LUID { LowPart: low, HighPart: high };
            let adapter: Option<IDXGIAdapter3> = unsafe { CreateDXGIFactory1::<IDXGIFactory4>() }.ok()
                .and_then(|f| unsafe { f.EnumAdapterByLuid::<windows::Win32::Graphics::Dxgi::IDXGIAdapter>(luid) }.ok())
                .and_then(|a| a.cast().ok());
            let mut last = [(0u64, 0u64); N];
            loop {
                std::thread::sleep(std::time::Duration::from_millis(every));
                let query = |group| {
                    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    adapter.as_ref().and_then(|a| unsafe { a.QueryVideoMemoryInfo(0, group, &mut info) }.ok())
                        .map_or((0, 0), |()| (info.Budget, info.CurrentUsage))
                };
                let (local, non_local) = (query(DXGI_MEMORY_SEGMENT_GROUP_LOCAL), query(DXGI_MEMORY_SEGMENT_GROUP_NON_LOCAL));
                let mut counters = ProcessMemoryCounters { cb: size_of::<ProcessMemoryCounters>() as u32, ..Default::default() };
                unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) };
                let mut status = windows::Win32::System::SystemInformation::MEMORYSTATUSEX {
                    dwLength: size_of::<windows::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32, ..Default::default()
                };
                let _ = unsafe { windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut status) };
                let mut deltas = [(0u64, 0u64, 0u64, 0u64); N];
                for i in 0..N {
                    let (count, bytes) = (COUNTS[i].load(Relaxed), BYTES[i].load(Relaxed));
                    deltas[i] = (count - last[i].0, bytes - last[i].1, count, bytes);
                    last[i] = (count, bytes);
                }
                crate::diagnostics::log_line(format_args!("{}",
                    line(local, non_local, counters.private as u64, status.ullAvailPageFile, status.ullTotalPageFile, &deltas)));
                if crate::env_toggle!("TUXBOX_COMMIT_BREAKDOWN") != Some("0") {
                    crate::diagnostics::log_line(format_args!("{}", commit_breakdown()));
                }
            }
        });
    });
}

/// The process's committed address space by kind, and its largest
/// allocations: test57 ran out of system commit with 20 GB process private
/// and no breakdown. TUXBOX_COMMIT_BREAKDOWN=0 skips it.
fn commit_breakdown() -> String {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    const PRIVATE: u32 = 0x2_0000;
    const MAPPED: u32 = 0x4_0000;
    const IMAGE: u32 = 0x100_0000;
    let mut by_kind = [0u64; 3];
    let mut by_base: std::collections::HashMap<(u64, u32), u64> = std::collections::HashMap::new();
    let mut address = 0u64;
    let mut regions = 0u32;
    // Bounded: a walk of millions of regions under memory pressure must not
    // hold up the stats thread (test58 logged no commit line at all).
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(2);
    while address < 0x7fff_ffff_0000 && regions < 4_000_000 {
        if regions % 4096 == 0 && started.elapsed() > budget {
            break;
        }
        let mut m = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(address as *const core::ffi::c_void), &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        regions += 1;
        let size = m.RegionSize as u64;
        if m.State == MEM_COMMIT {
            let kind = match m.Type.0 { PRIVATE => 0, MAPPED => 1, IMAGE => 2, _ => 0 };
            by_kind[kind] += size;
            if kind != 2 {
                *by_base.entry((m.AllocationBase as u64, m.Type.0)).or_default() += size;
            }
        }
        address = (m.BaseAddress as u64).saturating_add(size.max(0x1000));
    }
    let mut largest: Vec<_> = by_base.into_iter().collect();
    largest.sort_unstable_by_key(|(_, b)| std::cmp::Reverse(*b));
    let top: Vec<String> = largest.iter().take(8).map(|((base, kind), bytes)| {
        format!("{base:#x} {} {} MiB", if *kind == MAPPED { "mapped" } else { "private" }, mib(*bytes))
    }).collect();
    let small = largest.iter().skip(8).fold((0u64, 0u64), |(n, b), (_, s)| (n + 1, b + s));
    let partial = if address < 0x7fff_ffff_0000 { format!(" (partial: stopped at {address:#x})") } else { String::new() };
    format!("commit: private {} MiB, mapped {} MiB, image {} MiB in {regions} regions, walked in {} ms{partial}; largest allocations: {}; the other {} allocations {} MiB (TUXBOX_COMMIT_BREAKDOWN=0 off)",
        mib(by_kind[0]), mib(by_kind[1]), mib(by_kind[2]), started.elapsed().as_millis(), top.join(", "), small.0, mib(small.1))
}

#[cfg(test)]
#[test]
fn the_line_names_only_categories_that_ever_grew() {
    let mut deltas = [(0, 0, 0, 0); N];
    deltas[0] = (3, 3 << 20, 10, 40 << 20);
    let text = line((8 << 30, 6 << 30), (16 << 30, 1 << 30), 9 << 30, 2 << 30, 32 << 30, &deltas);
    assert!(text.contains("device local 6144/8192 MiB"));
    assert!(text.contains("shadow copies +3 (+3 MiB) of 10 (40 MiB)"));
    assert!(!text.contains("device buffers"));
}
