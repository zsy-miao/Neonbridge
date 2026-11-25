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

static LIVE_COUNTS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static LIVE_BYTES: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

/// Device memory counted as created (`note`) and as live until dropped: the
/// memory line's "live" part. test72's line named only creation totals, so
/// which kind held its 8.2 GB of device-local memory was unknown.
pub(crate) struct Live(Category, u64);

pub(crate) fn live(category: Category, bytes: u64) -> Live {
    note(category, bytes);
    LIVE_COUNTS[category as usize].fetch_add(1, Relaxed);
    LIVE_BYTES[category as usize].fetch_add(bytes, Relaxed);
    Live(category, bytes)
}

impl Drop for Live {
    fn drop(&mut self) {
        LIVE_COUNTS[self.0 as usize].fetch_sub(1, Relaxed);
        LIVE_BYTES[self.0 as usize].fetch_sub(self.1, Relaxed);
    }
}

const LIVE_MARKER: windows::core::GUID = windows::core::GUID::from_u128(0x5b0e8a3c_7d41_4f2a_9c6e_1a2b3c4d5e6f);

/// `live` for a native resource or heap with no owner of ours to hold the
/// count: a private-data marker on it holds it, so it is counted until the
/// object's last release. test137 read 8041 MiB of device-local use with page
/// pools (3968) and render heaps (3134) the only live categories; committed
/// placed-texture copies, their upload and readback buffers, device buffers
/// and descriptor tables were counted only as created. Falls back to `note`.
pub(crate) fn live_on(object: &impl Interface, category: Category, bytes: u64) {
    use windows::Win32::Graphics::Direct3D12::{ID3D12Device, ID3D12DeviceChild};
    static MARKER: crate::proxy::Slots<8> = crate::proxy::child_slots::<ID3D12Device, Live, 8>();
    let Ok(child) = object.cast::<ID3D12DeviceChild>() else { return note(category, bytes) };
    let mut device: Option<ID3D12Device> = None;
    let Some(device) = unsafe { child.GetDevice(&mut device) }.ok().and(device) else { return note(category, bytes) };
    let mut out = core::ptr::null_mut();
    // A failed allocation drops the Live at once: counted as created only.
    if unsafe { crate::proxy::allocate(&MARKER, &[], device, None, live(category, bytes), &mut out) }.is_err() || out.is_null() {
        return;
    }
    let marker = unsafe { windows::core::IUnknown::from_raw(out) };
    let _ = unsafe { child.SetPrivateDataInterface(&LIVE_MARKER, &marker) };
}

/// Device-local (usage, budget) at the last stats reading, and bytes freed
/// since by idle unmaps (placement.rs `unmap_idle`), which read this.
static LOCAL: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];

/// Device-local bytes over budget at the last reading, less what was freed since.
pub(crate) fn local_over() -> u64 {
    let (usage, budget, freed) = (LOCAL[0].load(Relaxed), LOCAL[1].load(Relaxed), LOCAL[2].load(Relaxed));
    usage.saturating_sub(budget).saturating_sub(freed)
}

pub(crate) fn local_freed(bytes: u64) {
    LOCAL[2].fetch_add(bytes, Relaxed);
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

/// The process's private commit.
pub(crate) fn private_bytes() -> u64 {
    let mut counters = ProcessMemoryCounters { cb: size_of::<ProcessMemoryCounters>() as u32, ..Default::default() };
    if unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut counters, counters.cb) } == 0 {
        return 0;
    }
    counters.private as u64
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
    let watched = crate::device::placement::watch::scan_stats();
    let held: Vec<String> = NAMES.iter().enumerate()
        .filter(|(i, _)| LIVE_COUNTS[*i].load(Relaxed) != 0)
        .map(|(i, name)| format!("{name} {} ({} MiB)", LIVE_COUNTS[i].load(Relaxed), mib(LIVE_BYTES[i].load(Relaxed))))
        .collect();
    format!("memory: device local {}/{} MiB, non-local {}/{} MiB (usage/budget); live now: {}; process private {} MiB; system free commit {} of {} MiB; shadow copies live {} MiB, dead kept {} MiB (TUXBOX_SHADOW_DEAD_MB caps); placed textures live {} ({} MiB), pooled {} ({} MiB, TUXBOX_PLACED_TEXTURE_POOL_MB caps), reused {}, of the live superseded by a newer placement {} ({} MiB); span heaps live {} ({} MiB, TUXBOX_PLACED_ALIAS=0 off), of those in system memory {} ({} MiB, TUXBOX_PLACED_SYSTEM_PCT), evicted {} ({} MiB; evictions {}, made resident again {}, TUXBOX_SPAN_EVICT_S), retired but held by live textures {} ({} MiB), placed in spans {}, on 64 KiB pages {} (recycled {}; pages in use {} MiB of pools {} MiB, {} pools in system memory, {} evicted, evictions {}, made resident again {}; TUXBOX_PLACED_PAGES=0 off), superseded and unmapped {} ({} MiB; unmaps {}, remaps {}, {} MiB of pages returned), of those sharing a span {}, committed fallbacks: {}; write-watch merged scans {} (calls saved {}, TUXBOX_WATCH_GAP_MB); created: {}",
        mib(local.1), mib(local.0), mib(non_local.1), mib(non_local.0), if held.is_empty() { "none tracked".to_string() } else { held.join(", ") }, mib(private), mib(free_commit), mib(total_commit), mib(live), mib(dead),
        placed.live, mib(placed.live_bytes), placed.pooled, mib(placed.pooled_bytes), placed.reused, placed.superseded, mib(placed.superseded_bytes),
        spans.spans, mib(spans.span_bytes), spans.system, mib(spans.system_bytes), spans.evicted, mib(spans.evicted_bytes), spans.evictions, spans.residencies, spans.retired, mib(spans.retired_bytes), spans.placed, paged.placed, paged.recycled, mib(paged.in_use), mib(paged.pools), paged.system_pools, paged.evicted_pools, paged.pool_evictions, paged.pool_residencies, paged.unmapped, mib(paged.unmapped_bytes), paged.unmaps, paged.remaps, mib(paged.returned_bytes), spans.shared, spans.fallback, watched.0, watched.1,
        if categories.is_empty() { "nothing".into() } else { categories.join(", ") })
}

/// Starts the stats thread once, for the adapter with this LUID.
pub(crate) fn start(luid: LUID) {
    commit_guard();
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
                LOCAL[0].store(local.1, Relaxed);
                LOCAL[1].store(local.0, Relaxed);
                LOCAL[2].store(0, Relaxed);
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

/// OpenScarlett-OCE 3da6373 (the idea; AGPL, nothing copied): at the system
/// commit limit every allocation of every process fails, so other programs
/// die and remote sessions cannot start a command. test137 ended with 1062
/// MiB of system commit free (18452 MiB process private). Every 250 ms, once
/// less than TUXBOX_MIN_COMMIT_MB (2048; 0 off) can still be committed, the
/// memory line is logged and the process ends itself with
/// STATUS_COMMITMENT_LIMIT before the system runs out.
fn commit_guard() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let floor = crate::env_toggle!("TUXBOX_MIN_COMMIT_MB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(2048) << 20;
        if floor == 0 {
            return;
        }
        let _ = std::thread::Builder::new().name("TuxBox commit guard".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let mut status = windows::Win32::System::SystemInformation::MEMORYSTATUSEX {
                dwLength: size_of::<windows::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32, ..Default::default()
            };
            if unsafe { windows::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut status) }.is_err()
                || status.ullAvailPageFile >= floor
            {
                continue;
            }
            crate::diagnostics::log_line(format_args!(
                "commit guard: the system can commit only {} MiB more (limit {} MiB; this process holds {} MiB private): ending the process before allocations fail everywhere (TUXBOX_MIN_COMMIT_MB={}; 0 off)",
                mib(status.ullAvailPageFile), mib(status.ullTotalPageFile), mib(private_bytes()), floor >> 20));
            crate::diagnostics::log_line(format_args!("{}", line((0, 0), (0, 0), private_bytes(), status.ullAvailPageFile, status.ullTotalPageFile, &[(0, 0, 0, 0); N])));
            // Its lines reach the log before the process ends (733f135 left
            // them to the 100 ms wait alone).
            let _ = std::io::Write::flush(&mut std::io::stderr());
            std::thread::sleep(std::time::Duration::from_millis(100));
            const STATUS_COMMITMENT_LIMIT: u32 = 0xC000_012D;
            let _ = unsafe { windows::Win32::System::Threading::TerminateProcess(windows::Win32::System::Threading::GetCurrentProcess(), STATUS_COMMITMENT_LIMIT) };
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
    // Host private bytes by protection class: write-combined or uncached
    // (driver-mapped GPU memory), plain read-write (CPU heaps), other.
    let mut host_protect = [0u64; 3];
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
            if kind == 0 && m.AllocationBase as u64 >= 0x10_0000_0000 {
                let p = m.Protect.0;
                host_protect[if p & 0x600 != 0 { 0 } else if p & 0xff == 0x04 { 1 } else { 2 }] += size;
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
    // test66 grew 6 GB of private commit in its last 30 s across thousands of
    // small allocations: split them by whose address range they are in (the
    // title's below 64 GiB, the host's above) and kind.
    let mut split = [(0u64, 0u64); 4];
    for ((base, kind), bytes) in largest.iter().skip(8) {
        let i = usize::from(*base >= 0x10_0000_0000) * 2 + usize::from(*kind == MAPPED);
        split[i].0 += 1;
        split[i].1 += bytes;
    }
    let names = ["title private", "title mapped", "host private", "host mapped"];
    let split: Vec<String> = split.iter().zip(names).map(|((n, b), name)| format!("{name} {n} ({} MiB)", mib(*b))).collect();
    // test149: host private grew 1.8 to 11.9 GiB with no owner named. Its
    // protection split and largest allocations tell driver-mapped GPU memory
    // from CPU heaps (storage audit).
    let mut host: Vec<u64> = largest.iter().filter(|((b, k), _)| *b >= 0x10_0000_0000 && *k != MAPPED).map(|(_, s)| *s).collect();
    host.sort_unstable_by(|a, b| b.cmp(a));
    let sizes: Vec<String> = host.iter().take(4).map(|s| format!("{} MiB", mib(*s))).collect();
    let buckets = host.iter().fold([0u64; 3], |mut c, s| { c[if *s >= 64 << 20 { 0 } else if *s >= 1 << 20 { 1 } else { 2 }] += 1; c });
    let split = format!("{}; host private by protection: write-combined/uncached {} MiB, read-write {} MiB, other {} MiB; host private allocations >=64 MiB {}, 1-64 MiB {}, <1 MiB {}, largest {}",
        split.join(", "), mib(host_protect[0]), mib(host_protect[1]), mib(host_protect[2]), buckets[0], buckets[1], buckets[2], sizes.join("/"));
    let partial = if address < 0x7fff_ffff_0000 { format!(" (partial: stopped at {address:#x})") } else { String::new() };
    format!("commit: private {} MiB, mapped {} MiB, image {} MiB in {regions} regions, walked in {} ms{partial}; largest allocations: {}; the other {} allocations {} MiB: {} (TUXBOX_COMMIT_BREAKDOWN=0 off)",
        mib(by_kind[0]), mib(by_kind[1]), mib(by_kind[2]), started.elapsed().as_millis(), top.join(", "), small.0, mib(small.1), split)
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

/// The private commit of process `pid` plus that of its children (all but
/// this process): the shader worker and the xshader and dxc it runs, which
/// five-star-recomp fec516c (GPL-3.0) counts toward the compile ceiling.
pub(crate) fn private_bytes_with_children(pid: u32) -> u64 {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    let me = std::process::id();
    let mut pids = Vec::new();
    if pid != me {
        pids.push(pid);
    }
    if let Ok(snapshot) = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) } {
        let mut entry = PROCESSENTRY32W { dwSize: size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while more {
            if entry.th32ParentProcessID == pid && entry.th32ProcessID != me {
                pids.push(entry.th32ProcessID);
            }
            more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        let _ = unsafe { CloseHandle(snapshot) };
    }
    pids.into_iter().map(|p| {
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, p) }) else { return 0 };
        let mut counters = ProcessMemoryCounters { cb: size_of::<ProcessMemoryCounters>() as u32, ..Default::default() };
        let ok = unsafe { K32GetProcessMemoryInfo(process.0 as isize, &mut counters, counters.cb) } != 0;
        let _ = unsafe { CloseHandle(process) };
        if ok { counters.private as u64 } else { 0 }
    }).sum()
}
