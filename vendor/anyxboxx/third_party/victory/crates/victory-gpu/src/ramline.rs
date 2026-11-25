//! The "ram line": where the process's memory is, in release builds too.
//! (`copystat::log_memory` has the long form, in diagnostics builds only.)

use std::sync::atomic::Ordering::Relaxed;

/// Resident and shared bytes of each of `ranges`: every 16th 4 KiB page of the address space
/// (the pages at multiples of 64 KiB, so many small ranges do not each count one page as sixteen)
/// asked about (`K32QueryWorkingSetEx`, 4096 at a time, at most `CAP` pages a run), scaled up.
fn sample(ranges: &[(usize, usize)]) -> Vec<(u64, u64)> {
    use windows_sys::Win32::System::ProcessStatus::{K32QueryWorkingSetEx, PSAPI_WORKING_SET_EX_INFORMATION};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    const STEP: usize = 16;
    const CAP: usize = 2_000_000;
    let mut out = vec![(0u64, 0u64); ranges.len()];
    let mut batch: Vec<PSAPI_WORKING_SET_EX_INFORMATION> = Vec::with_capacity(4096);
    let mut owner: Vec<usize> = Vec::with_capacity(4096);
    let mut asked = 0usize;
    let flush = |batch: &mut Vec<PSAPI_WORKING_SET_EX_INFORMATION>, owner: &mut Vec<usize>, out: &mut Vec<(u64, u64)>| {
        // SAFETY: our own process and a sized array the call fills in.
        let ok = unsafe { K32QueryWorkingSetEx(GetCurrentProcess(), batch.as_mut_ptr().cast(), (batch.len() * size_of::<PSAPI_WORKING_SET_EX_INFORMATION>()) as u32) };
        if ok != 0 {
            for (e, &i) in batch.iter().zip(owner.iter()) {
                // SAFETY: a union of the flags word, filled in.
                let f = unsafe { e.VirtualAttributes.Flags };
                out[i].0 += ((f & 1) as u64) * (STEP as u64) * 4096;
                out[i].1 += (((f >> 15) & (f & 1)) as u64) * (STEP as u64) * 4096;
            }
        }
        batch.clear();
        owner.clear();
    };
    'all: for (i, &(start, len)) in ranges.iter().enumerate() {
        let mut at = start.next_multiple_of(STEP * 4096);
        while at < start + len {
            if asked >= CAP {
                break 'all;
            }
            // SAFETY: plain data; the address is the input.
            let mut e: PSAPI_WORKING_SET_EX_INFORMATION = unsafe { std::mem::zeroed() };
            e.VirtualAddress = at as *mut _;
            batch.push(e);
            owner.push(i);
            asked += 1;
            if batch.len() == 4096 {
                flush(&mut batch, &mut owner, &mut out);
            }
            at += STEP * 4096;
        }
    }
    if !batch.is_empty() {
        flush(&mut batch, &mut owner, &mut out);
    }
    out
}

/// Estimated (resident, shared) bytes of `ranges` together.
fn resident(ranges: &[(usize, usize)]) -> (u64, u64) {
    sample(ranges).iter().fold((0, 0), |a, r| (a.0 + r.0, a.1 + r.1))
}

/// A committed private or mapped region of the address space.
struct Region {
    base: usize,
    len: usize,
    alloc: usize,
    mapped: bool,
    protect: u32,
}

/// Committed private and mapped regions of the whole address space, by address.
fn committed() -> Vec<Region> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_MAPPED, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut out = Vec::new();
    let mut at = 0usize;
    loop {
        // SAFETY: plain data the call fills in.
        let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: a query of any address is allowed.
        if unsafe { VirtualQuery(at as *const _, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } == 0 {
            break;
        }
        if m.State == MEM_COMMIT && m.Type & (MEM_PRIVATE | MEM_MAPPED) != 0 {
            out.push(Region { base: m.BaseAddress as usize, len: m.RegionSize, alloc: m.AllocationBase as usize, mapped: m.Type & MEM_MAPPED != 0, protect: m.Protect });
        }
        match (m.BaseAddress as usize).checked_add(m.RegionSize) {
            Some(next) if next > at => at = next,
            _ => break,
        }
    }
    out
}

/// The address ranges already given to a group: later groups get only what is left.
#[derive(Default)]
struct Taken(std::collections::BTreeMap<usize, usize>);

impl Taken {
    /// The parts of `[s, e)` nobody has, which are now taken.
    fn take(&mut self, s: usize, e: usize) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut at = s;
        let from = self.0.range(..=s).next_back().filter(|(_, en)| **en > s).map_or(s, |(b, _)| *b);
        for (&b, &en) in self.0.range(from..e) {
            if b > at {
                out.push((at, b));
            }
            at = at.max(en);
            if at >= e {
                break;
            }
        }
        if at < e {
            out.push((at, e));
        }
        for &(a, b) in &out {
            self.0.insert(a, b);
        }
        out
    }
}

/// The parts of `[s, e)` that lie in committed `regions`.
fn clip(regions: &[Region], s: usize, e: usize) -> Vec<(usize, usize)> {
    let first = regions.partition_point(|r| r.base + r.len <= s);
    regions[first..].iter().take_while(|r| r.base < e).map(|r| (r.base.max(s), (r.base + r.len).min(e))).collect()
}

/// Title ranges by kind, as (base, end), with stale reservations dropped: the title frees its
/// `XMemVirtualAlloc` memory itself, so a listed base that is no allocation base is gone.
fn title_ranges() -> [Vec<(usize, usize)>; 5] {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};
    let mut out: [Vec<(usize, usize)>; 5] = Default::default();
    for (base, size, kind) in crate::titlemem::snapshot() {
        if kind == crate::titlemem::VIRTUAL {
            // SAFETY: plain data the call fills in.
            let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
            // SAFETY: a query of any address is allowed.
            let ok = unsafe { VirtualQuery(base as *const _, &mut m, size_of::<MEMORY_BASIC_INFORMATION>()) } != 0;
            if !ok || m.AllocationBase as usize != base {
                crate::titlemem::forget_if_stale(base);
                continue;
            }
        }
        let list = &mut out[(kind as usize).min(4)];
        match list.last_mut() {
            Some(last) if last.1 == base => last.1 = base + size,
            _ => list.push((base, base + size)),
        }
    }
    out
}

/// Every 30 s, on its own thread (started by the first call of [`log`]): how much of title
/// memory, the host copies' CPU sides and the texture staging buffers is in the working set.
/// The title's own memory is what the runtime listed as it allocated it (`crate::titlemem`);
/// the rest of the committed private and mapped regions is split by range, not by totals:
/// the host copies and staging first, then the title's images, page views, virtual allocations
/// and large heap blocks, then whatever is left, private and mapped.
fn sampler() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("resident-line".into()).spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(30));
            let started = std::time::Instant::now();
            let mb = |v: u64| v >> 20;
            let total = |r: &[(usize, usize)]| r.iter().map(|r| r.1 as u64).sum::<u64>();
            let [buffers, windows] = crate::shadow::cpu_ranges();
            let staging = crate::texture::cpu_ranges();
            let regions = committed();
            let titles = title_ranges();
            let (b, w, s) = (resident(&buffers), resident(&windows), resident(&staging));
            let mut taken = Taken::default();
            for g in [&buffers, &windows, &staging] {
                for &(at, len) in g {
                    taken.take(at, at + len);
                }
            }
            // Title kinds, in the order they claim overlapping ranges: (committed, resident) each.
            let names = ["images", "page views", "virtual allocs", "heap blocks"];
            let kinds = [crate::titlemem::IMAGE, crate::titlemem::VIEW, crate::titlemem::VIRTUAL, crate::titlemem::HEAP];
            let by_kind: Vec<(u64, u64)> = kinds
                .iter()
                .map(|&k| {
                    let pieces: Vec<(usize, usize)> = titles[k as usize]
                        .iter()
                        .flat_map(|&(a, e)| taken.take(a, e))
                        .flat_map(|(a, e)| clip(&regions, a, e))
                        .map(|(a, e)| (a, e - a))
                        .collect();
                    (total(&pieces), resident(&pieces).0)
                })
                .collect();
            // What is left of the committed regions, with the region it came from.
            let mut left: Vec<(usize, usize)> = Vec::new();
            let mut from: Vec<&Region> = Vec::new();
            for r in &regions {
                for (a, e) in taken.take(r.base, r.base + r.len) {
                    left.push((a, e - a));
                    from.push(r);
                }
            }
            let res = sample(&left);
            let (mut private, mut mapped) = ((0u64, 0u64), (0u64, 0u64));
            // By allocation base: (resident, committed, mapped, protect of the biggest piece, its size).
            let mut owners: std::collections::HashMap<usize, (u64, u64, bool, u32, usize)> = Default::default();
            for ((piece, r), (rs, _)) in left.iter().zip(&from).zip(&res) {
                let side = if r.mapped { &mut mapped } else { &mut private };
                side.0 += rs;
                side.1 += piece.1 as u64;
                let o = owners.entry(r.alloc).or_insert((0, 0, r.mapped, r.protect, 0));
                o.0 += rs;
                o.1 += piece.1 as u64;
                if piece.1 > o.4 {
                    (o.2, o.3, o.4) = (r.mapped, r.protect, piece.1);
                }
            }
            let mut top: Vec<_> = owners.into_iter().collect();
            top.sort_unstable_by_key(|(_, o)| std::cmp::Reverse(o.0));
            let top: Vec<String> = top
                .iter()
                .take(5)
                .map(|(base, o)| format!("{base:#x} {} protect {:#x} {} MB of {} MB", if o.2 { "mapped" } else { "private" }, o.3, mb(o.0), mb(o.1)))
                .collect();
            let mut title = String::new();
            for (n, (c, r)) in names.iter().zip(&by_kind) {
                title += &format!("title {n} {} MB of {} MB; ", mb(*r), mb(*c));
            }
            let mut c = windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS { cb: size_of::<windows_sys::Win32::System::ProcessStatus::PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
            // SAFETY: our own process and a sized structure.
            unsafe { windows_sys::Win32::System::ProcessStatus::K32GetProcessMemoryInfo(windows_sys::Win32::System::Threading::GetCurrentProcess(), &mut c, c.cb) };
            tracing::info!(
                "resident line: {title}other private {} MB of {} MB; other mapped {} MB of {} MB (largest: {}); \
                 host copies of buffers {} MB of {} MB ({} MB shared); windows {} MB of {} MB ({} MB shared); \
                 texture staging {} MB of {} MB ({} MB shared); process working set {} MB; sampled in {} ms; \
                 rust heap {} MB live (peak {} MB)",
                mb(private.0),
                mb(private.1),
                mb(mapped.0),
                mb(mapped.1),
                top.join(", "),
                mb(b.0),
                mb(total(&buffers)),
                mb(b.1),
                mb(w.0),
                mb(total(&windows)),
                mb(w.1),
                mb(s.0),
                mb(total(&staging)),
                mb(s.1),
                mb(c.WorkingSetSize as u64),
                started.elapsed().as_millis(),
                mb(victory_core::heapcount::RUST_HEAP[0].load(Relaxed)),
                mb(victory_core::heapcount::RUST_HEAP[1].load(Relaxed)),
            );
        });
    });
}

/// Logs one line, called every five seconds from the host's frame stats.
pub(crate) fn log() {
    sampler();
    use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mb = |v: u64| v >> 20;
    let mut c = PROCESS_MEMORY_COUNTERS { cb: size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
    // SAFETY: our own process and a sized structure.
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    unsafe { GlobalMemoryStatusEx(&mut m) };
    let copies = crate::shadow::memory();
    let staged = crate::texture::memory();
    let alive: Vec<u64> = crate::host::ALIVE.iter().map(|a| a.load(Relaxed)).collect();
    let made: Vec<u64> = crate::host::MADE.iter().map(|a| a.load(Relaxed)).collect();
    let kinds: Vec<u64> = crate::resource::KINDS.iter().map(|k| k.load(Relaxed)).collect();
    let htex = [0, 1].map(|i| crate::resource::HOST_TEXTURES[i].load(Relaxed));
    let vk: Vec<u64> = crate::resource::VRAM_KINDS.iter().map(|k| k.load(Relaxed)).collect();
    let ev = [0, 1].map(|i| crate::resource::EVICTED[i].load(Relaxed));
    let sev: Vec<u64> = crate::resource::SLOT_EVICT.iter().map(|k| k.load(Relaxed)).collect();
    let card_copies = crate::shadow::card_copy_bytes();
    let card = crate::host::host().and_then(|h| h.video_memory()).unwrap_or_default();
    let k = crate::pipeline::kept();
    let (idle, idle_bytes) = crate::list::idle_pool();
    let drops = [0, 1, 2].map(|i| crate::pipeline::DXIL_DROPS[i].load(Relaxed));
    tracing::info!(
        "ram line (shaders): containers {} {} MB, translations {} with host code {} MB kept (let go so far {} MB, \
         brought back {}x {} MB; VICTORY_DROP_DXIL), pipeline library file {} MB; idle host lists {} (upload {} MB, \
         destroyed by VICTORY_LIST_POOL_IDLE_S {}); rust heap {} MB; title bytes told written after view writes (VICTORY_DS_NOTIFY) {} MB",
        k[0],
        mb(k[1]),
        k[2],
        mb(k[3]),
        mb(drops[0]),
        drops[1],
        mb(drops[2]),
        mb(k[4]),
        idle,
        mb(idle_bytes),
        crate::list::TRIMMED_LISTS.load(Relaxed),
        mb(victory_core::heapcount::RUST_HEAP[0].load(Relaxed)),
        mb(crate::watch::NOTIFIED.swap(0, Relaxed)),
    );
    tracing::info!(
        "ram line: process working set {} MB, private {} MB; PC free {} MB, free commit {} MB; \
         title memory host copies {} alive {} MB (windows outside buffers {} {} MB; snapshot buffers kept {} reserving {} MB); \
         texture staging {} alive {} MB ({} kept by their texture); \
         host textures {} {} MB; title texture host copies alive {} {} MB, let go {} {} MB; \
         list scratch chunks {} {} MB (spare {} {} MB), other list uploads {} {} MB, host lists {}; \
         made so far: upload buffers {} {} MB, shared {} {} MB, card buffers {} {} MB; \
         card copies {} MB (before twins {} MB); card (VRAM) {} MB in use of {} MB budget, shared {} MB in use; \
         host texture VRAM by kind: filled {} MB, filled placed {} MB, targets {} MB, depth {} MB, uav {} MB, twins {} MB, \
         filled idle but held {} MB, filled duplicates at one address {} MB; evicted under pressure {} {} MB; \
         evicted with slots (VICTORY_SLOT_EVICT) {} textures {} MB, {} slots reset, {} came back ({} within 2 s, away {} ms in all), {} given up{}{}",
        mb(c.WorkingSetSize as u64),
        mb(c.PagefileUsage as u64),
        mb(m.ullAvailPhys),
        mb(m.ullAvailPageFile),
        copies[0],
        mb(copies[1]),
        copies[2],
        mb(copies[3]),
        copies[4],
        mb(copies[5]),
        staged[0],
        mb(staged[1]),
        staged[2],
        htex[0],
        mb(htex[1]),
        kinds[0],
        mb(kinds[1]),
        kinds[2],
        mb(kinds[3]),
        alive[1],
        mb(alive[2]),
        alive[3],
        mb(alive[4]),
        alive[6],
        mb(alive[7]),
        alive[0],
        made[0],
        mb(made[1]),
        made[2],
        mb(made[3]),
        made[4],
        mb(made[5]),
        mb(card_copies[0]),
        mb(card_copies[1]),
        mb(card[1]),
        mb(card[0]),
        mb(card[3]),
        mb(vk[0]),
        mb(vk[1]),
        mb(vk[2]),
        mb(vk[3]),
        mb(vk[4]),
        mb(vk[5]),
        mb(vk[6]),
        mb(vk[7]),
        ev[0],
        mb(ev[1]),
        sev[0],
        mb(sev[1]),
        sev[2],
        sev[3],
        sev[4],
        sev[5] / 10,
        sev[6],
        upload_line(),
        rebar_line(),
    );
}

/// (Neptune) The rebar window part of the ram line (`VICTORY_WINDOW_REBAR=1`), else nothing.
fn rebar_line() -> String {
    if !crate::shadow::window_rebar() {
        return String::new();
    }
    use crate::shadow::REBAR;
    format!(
        "; window rebar: windows {} ({} MB), written since the last line {} MB, refused: budget {} / folding {} / other {} / unordered access {}, shader writes to one (bug) {}",
        REBAR[0].load(Relaxed),
        REBAR[1].load(Relaxed) >> 20,
        REBAR[2].swap(0, Relaxed) >> 20,
        REBAR[3].load(Relaxed),
        REBAR[4].load(Relaxed),
        REBAR[5].load(Relaxed),
        REBAR[7].load(Relaxed),
        REBAR[6].load(Relaxed),
    )
}

/// (Neptune) The window upload part of the ram line (`VICTORY_CARD_WINDOW_UPLOAD=1`), else nothing.
fn upload_line() -> String {
    if !crate::shadow::window_upload() {
        return String::new();
    }
    use crate::shadow::TRANSIENT;
    let mb = |v: u64| v >> 20;
    let [pool, peak, overflow, overflows] = crate::windowup::stats();
    format!(
        "; window upload: transient windows {}, pool {} MB (peak {} MB, overflow slabs alive {} MB, overflow allocations {}), \
         sent from chunks since the last line {} MB, buffers not folded into one {}, shader writes to one (bug) {}",
        TRANSIENT[0].load(Relaxed),
        mb(pool),
        mb(peak),
        mb(overflow),
        overflows,
        mb(crate::windowup::SENT.swap(0, Relaxed)),
        TRANSIENT[1].load(Relaxed),
        TRANSIENT[2].load(Relaxed),
    )
}
