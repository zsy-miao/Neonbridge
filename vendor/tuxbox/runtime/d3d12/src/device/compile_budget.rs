//! Driver compiles admitted by the memory they are expected to take. Ported
//! from five-star-recomp afdd699 (GPL-3.0, CompileBudget.cs).
//!
//! The NVIDIA compiler grows a doubling arena per pipeline (1, 2, 4, 8 GB)
//! for minutes; five-star saw six large mesh-shader pipelines started together
//! take 50 GB. Each compile is expected to take 4096 bytes per byte of DXIL,
//! clamped to 2..32 GB. A new compile starts only while free physical memory
//! and commit cover the reserves (4 GB physical, 8 GB commit), its estimate
//! and what the running compiles are still expected to take (their estimates
//! less the process's private growth since compiles began). Compiles of less
//! than 256 KiB of DXIL are counted but never wait. Unlike five-star, a
//! compile goes ahead after TUXBOX_COMPILE_BUDGET_WAIT_MS (30000) so a machine
//! that never frees enough cannot hold a pipeline forever.
//! TUXBOX_COMPILE_BUDGET=0 never waits.
//!
//! The 4096 B per byte figure and the reserves are five-star's NVIDIA mesh
//! measurements, and nothing here had checked them. test132 (RTX 4060, 38 GB
//! commit) never had the 8 GB commit reserve free once the title held 18 GB, so
//! every compile of 256 KiB or more waited the full 30 s. `measure` records
//! what each driver compile actually takes, independent of the switch: the
//! peak process-private growth over the compile, sampled every 20 ms, and what
//! is still held when it returns. It logs one line per compile of 256 KiB or
//! more (and a running maximum per KiB of DXIL), so the estimate and the
//! reserves can be derived from this machine. TUXBOX_COMPILE_MEASURE=0 off.
//!
//! Two ceilings bound the process itself (five-star-recomp 9a5bcc1, GPL-3.0,
//! with credit): at most TUXBOX_MAX_DRIVER_COMPILES (4) waiting-kind compiles
//! of 256 KiB of DXIL or more at once, and none starts while private bytes,
//! the running ones' remaining estimates and its own would pass
//! TUXBOX_COMPILE_CEILING_GB (28); one always may when no large compile runs.
//! test140 had six threads in nvgpucomp64 with 19.8 GB private and 3971 MiB
//! of commit free. Like the reserves, only compiles no title thread waits on
//! are held (and only up to TUXBOX_COMPILE_BUDGET_WAIT_MS).
//!
//! The ceiling also counts the shader worker's private bytes (five-star-recomp
//! fec516c, GPL-3.0, with credit): the process named by the session's
//! `worker-pid` and its children (xshader, dxc), all but this one, sampled at
//! most every 250 ms. The launcher holds its own translation jobs to the same
//! ceiling (src/shaders.rs).
//!
//! A commit floor holds every large compile, awaited or not. test142 lost
//! the device (0x887A0020, a driver internal error, no command list in
//! flight) 124 s in: the machine sat at its commit limit the whole run (free
//! commit 0.5-1.5 GB while the page file grew from 34.6 to 41.8 GB), the
//! compiles a title thread waits on skipped every check above, three ran at
//! once at +1.7-2.2 GB private each, and free commit was 165 MiB just before
//! a driver allocation failed. Now no compile of 256 KiB of DXIL or more
//! starts while another such compile runs and free commit is under
//! TUXBOX_COMPILE_COMMIT_FLOOR_MB (3072; 0 off). Waiting for a running compile
//! is finite, so this hold has no time limit; with none running it never
//! holds, so a compile always gets to start.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const TINY_SHADER_BYTES: u64 = 256 << 10;
const BYTES_PER_SHADER_BYTE: u64 = 4096;
const MINIMUM_ESTIMATE: u64 = 2 << 30;
const MAXIMUM_ESTIMATE: u64 = 32 << 30;
const PHYSICAL_RESERVE: u64 = 4 << 30;
const COMMIT_RESERVE: u64 = 8 << 30;
const PAUSE_MS: u64 = 500;

struct State {
    running: u32,
    /// Running compiles of TINY_SHADER_BYTES or more.
    large: u32,
    expected: u64,
    baseline: u64,
}

static STATE: Mutex<State> = Mutex::new(State { running: 0, large: 0, expected: 0, baseline: 0 });
static WAITING: AtomicU64 = AtomicU64::new(0);
static WAITED: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_COMPILE_BUDGET") != Some("0")
}

fn max_wait() -> std::time::Duration {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    std::time::Duration::from_millis(*MS.get_or_init(|| {
        std::env::var("TUXBOX_COMPILE_BUDGET_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(30000)
    }))
}

/// Driver memory expected for a compile. five-star-recomp 51c338d refit it on
/// 101 measured compiles (256 KB to 1.94 MB of DXIL): 2,816 bytes per byte,
/// 0.5 to 8 GB, where 4,096 and 2 to 32 GB held boots to one or two compiles.
/// TUXBOX_COMPILE_ESTIMATE_FIT=0 keeps the old figures.
fn estimate(shader_bytes: u64) -> u64 {
    static FIT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *FIT.get_or_init(|| crate::env_toggle!("TUXBOX_COMPILE_ESTIMATE_FIT") != Some("0")) {
        return shader_bytes.saturating_mul(2816).clamp(512 << 20, 8 << 30);
    }
    shader_bytes.saturating_mul(BYTES_PER_SHADER_BYTE).clamp(MINIMUM_ESTIMATE, MAXIMUM_ESTIMATE)
}

/// Free physical memory and commit headroom of the machine.
fn available() -> (u64, u64) {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX { dwLength: size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_err() {
        return (u64::MAX, u64::MAX);
    }
    (status.ullAvailPhys, status.ullAvailPageFile)
}

fn wanted_at_once() -> u32 {
    crate::env_toggle!("TUXBOX_WANTED_AT_ONCE").and_then(|v| v.trim().parse().ok()).unwrap_or(6)
}

fn max_large() -> u32 {
    crate::env_toggle!("TUXBOX_MAX_DRIVER_COMPILES").and_then(|v| v.trim().parse().ok()).unwrap_or(4)
}

fn ceiling() -> u64 {
    crate::env_toggle!("TUXBOX_COMPILE_CEILING_GB").and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(28) << 30
}

/// The shader worker's private bytes (see the header), sampled at most
/// every 250 ms.
fn helper_private() -> u64 {
    static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, bytes)) = *last && at.elapsed() < std::time::Duration::from_millis(250) {
        return bytes;
    }
    let bytes = super::shader_cache::worker_pid().map_or(0, crate::diagnostics::memory::private_bytes_with_children);
    *last = Some((std::time::Instant::now(), bytes));
    bytes
}

static WRITE_COMBINED: AtomicU64 = AtomicU64::new(0);

/// The private bytes the ceiling counts: the process's less its committed
/// write-combined memory (the driver's system-memory pools and upload
/// heaps), plus the shader worker's. five-star-recomp 4e95ab2 (GPL-3.0,
/// credit to its authors): those pools grew by 20 GB after the story load
/// and held driver compiles to one at a time. The walk runs every 15 s on
/// its own thread. TUXBOX_COMPILE_CEILING_WC=1 counts them, as before.
fn counted(private: u64) -> u64 {
    static STARTED: std::sync::Once = std::sync::Once::new();
    if crate::env_toggle!("TUXBOX_COMPILE_CEILING_WC") == Some("1") {
        return private.saturating_add(helper_private());
    }
    STARTED.call_once(|| {
        let _ = std::thread::Builder::new().name("TuxBox write-combined walk".into()).spawn(|| loop {
            WRITE_COMBINED.store(write_combined_bytes(), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_secs(15));
        });
    });
    private.saturating_sub(WRITE_COMBINED.load(Ordering::Relaxed)).saturating_add(helper_private())
}

/// Committed private memory with protection 0x404 (read-write,
/// write-combined), by one walk of the address space.
fn write_combined_bytes() -> u64 {
    use windows::Win32::System::Memory::{MEM_COMMIT, MEM_PRIVATE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, PAGE_WRITECOMBINE, VirtualQuery};
    const END: usize = 0x7FFF_FFFF_0000;
    let (mut at, mut bytes) = (0usize, 0u64);
    while at < END {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: a query of this process's address space.
        if unsafe { VirtualQuery(Some(at as _), &mut region, size_of_val(&region)) } == 0 || region.RegionSize == 0 {
            break;
        }
        if region.State == MEM_COMMIT && region.Type == MEM_PRIVATE && region.Protect == (PAGE_READWRITE | PAGE_WRITECOMBINE) {
            bytes += region.RegionSize as u64;
        }
        at = region.BaseAddress as usize + region.RegionSize;
    }
    bytes
}

fn commit_floor() -> u64 {
    static MB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MB.get_or_init(|| crate::env_toggle!("TUXBOX_COMPILE_COMMIT_FLOOR_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(3072)) << 20
}

static FLOOR_HELD: AtomicU64 = AtomicU64::new(0);

/// The floor sized by the program (xcl 28946bb, GPL-2.0-only, credit to its
/// authors): the 3 GB floor was measured on 256 KiB programs, while a
/// 29.8 MB program's driver compile grew past 14 GB of commit and ~1 MB ones
/// took several GB. A compile asks for max(floor, 512 x its DXIL); programs
/// up to 6 MiB wait for what they did before.
fn sized_floor(shader_bytes: u64) -> u64 {
    let floor = commit_floor();
    if floor == 0 { 0 } else { floor.max(shader_bytes.saturating_mul(512)) }
}

/// The commit a gated compile leaves free beyond its estimate and the running
/// compiles' remaining ones. It was a fixed 8 GB (five-star's NVIDIA figure):
/// test132 (38 GB commit, the title at 18 GB) never had it free, so every
/// prefetch of 256 KiB of DXIL or more waited the full 30 s, while
/// VIctory-fxceless builds its early list ungated. Since 2332ba8 the commit
/// floor scales with the program (sized_floor), so the reserve is that floor:
/// 3 GB for programs up to 6 MiB of DXIL instead of 8. The estimate itself
/// stays (measured peaks reach 7.4 KB per DXIL byte alone, test140), and so
/// does the floor's hold: test142 lost the device at the commit limit.
/// TUXBOX_COMPILE_COMMIT_RESERVE=fixed keeps 8 GB; with the floor off
/// (TUXBOX_COMPILE_COMMIT_FLOOR_MB=0) it is 8 GB too.
fn commit_reserve(shader_bytes: u64) -> u64 {
    let floor = sized_floor(shader_bytes);
    if floor == 0 || crate::env_toggle!("TUXBOX_COMPILE_COMMIT_RESERVE") == Some("fixed") { COMMIT_RESERVE } else { floor }
}

fn try_enter(estimate: u64, tiny: bool, forced: bool, wanted: bool, large: bool, shader_bytes: u64) -> bool {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    // test148 #1600 stop: title threads sat in wait_granted / build_here's
    // join while their compile stood here, since this hold ignored that the
    // title waits on it and the 30 s give-up, so it had no time limit at low
    // commit. A wanted or forced compile passes it (xcl 724b9e2: never gate a
    // pipeline a game thread waits for). TUXBOX_FLOOR_HOLDS_WANTED=1 holds them.
    let exempt = (wanted || forced) && crate::env_toggle!("TUXBOX_FLOOR_HOLDS_WANTED") != Some("1");
    if large && state.large != 0 && enabled() && !exempt {
        let floor = sized_floor(shader_bytes);
        let (_, commit) = available();
        if floor != 0 && commit < floor {
            let n = FLOOR_HELD.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "compile budget: a large compile ({} KiB of DXIL) waits for one of the {} running: {} MiB commit free, under its {} MiB floor (max of TUXBOX_COMPILE_COMMIT_FLOOR_MB and 512x its DXIL; {n} holds so far)",
                    shader_bytes >> 10, state.large, commit >> 20, floor >> 20));
            }
            return false;
        }
    }
    let private = crate::diagnostics::memory::private_bytes();
    if state.running == 0 {
        state.baseline = private;
    }
    let remaining = state.expected.saturating_sub(private.saturating_sub(state.baseline));
    if !tiny {
        let need = estimate.saturating_add(remaining);
        let (physical, commit) = available();
        if physical < PHYSICAL_RESERVE.saturating_add(need) || commit < commit_reserve(shader_bytes).saturating_add(need) {
            return false;
        }
    }
    // The cap of large compiles and the ceiling sat under `!tiny`, which a
    // compile the title waits on always is (ungated): those ran past both,
    // and test runs reached far more than 4 large compiles at once. Every
    // large compile counts now, as in five-star-recomp CompileBudget (GPL-3.0,
    // credit to its authors), where only the commit wait is for prefetches;
    // one past TUXBOX_COMPILE_BUDGET_WAIT_MS still starts.
    // TUXBOX_COMPILE_CAP_ALL=0 caps gated compiles only, as before.
    let capped = if crate::env_toggle!("TUXBOX_COMPILE_CAP_ALL") == Some("0") { !tiny } else { large && !forced };
    // xcl 9a272d4 (GPL-2.0-only, credit to its authors): a compile a title
    // thread waits on goes on at once, past the cap and the ceiling, while
    // fewer than TUXBOX_WANTED_AT_ONCE (6; 0 off) large ones run; at a world
    // load the title's threads otherwise stood behind the queue until its
    // hang watchdog fired.
    let capped = capped && !(wanted && state.large < wanted_at_once());
    if capped && state.large != 0 {
        let need = estimate.saturating_add(remaining);
        let (most, ceiling) = (max_large(), ceiling());
        if (most != 0 && state.large >= most)
            || (ceiling != 0 && counted(private).saturating_add(need) > ceiling)
        {
            return false;
        }
    }
    state.running += 1;
    state.large += u32::from(large);
    state.expected += estimate;
    true
}

/// A compile admitted by the budget (its estimate, and whether it is large);
/// leaving it ends the compile.
pub(super) struct Admitted(u64, bool);

impl Drop for Admitted {
    fn drop(&mut self) {
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        state.running = state.running.saturating_sub(1);
        state.large = state.large.saturating_sub(u32::from(self.1));
        state.expected = state.expected.saturating_sub(self.0);
    }
}

/// Whether a compile is waiting for memory now.
pub(super) fn waiting() -> bool {
    WAITING.load(Ordering::Relaxed) != 0
}

/// Waits until a compile of `shader_bytes` of DXIL may start.
pub(super) fn enter(shader_bytes: u64) -> Option<Admitted> {
    if !enabled() {
        return None;
    }
    let estimate = estimate(shader_bytes);
    // test133: with the budget on, title threads and bound pipelines waited
    // the full 30 s behind an 8 GB commit reserve this machine never had.
    // Only a prefetch waits now; a compile a title thread waits on (on its
    // own thread, bound, or stolen) is counted and starts at once
    // (five-star-recomp "Programs on demand", xcl 724b9e2: never gate a
    // pipeline a game thread waits for). TUXBOX_COMPILE_BUDGET=all gates every
    // compile as before.
    let gated = super::stream::background::prefetching() || crate::env_toggle!("TUXBOX_COMPILE_BUDGET") == Some("all");
    let tiny = shader_bytes < TINY_SHADER_BYTES || !gated;
    let mut waited = None;
    let large = shader_bytes >= TINY_SHADER_BYTES;
    loop {
        // A prefetch the title turned to wait on meanwhile starts at once.
        let wanted = !gated || super::stream::background::waited_on();
        let tiny = tiny || (gated && wanted);
        let forced = waited.is_some_and(|s: std::time::Instant| s.elapsed() >= max_wait());
        if try_enter(estimate, tiny || forced, forced, wanted, large, shader_bytes) {
            break;
        }
        if waited.is_none() {
            WAITING.fetch_add(1, Ordering::Relaxed);
            waited = Some(std::time::Instant::now());
        }
        std::thread::sleep(std::time::Duration::from_millis(PAUSE_MS));
    }
    if let Some(start) = waited {
        WAITING.fetch_sub(1, Ordering::Relaxed);
        let n = WAITED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            let (physical, commit) = available();
            crate::diagnostics::log_line(format_args!(
                "compile budget: a compile of {} KiB of DXIL (expected {} MiB) waited {} ms for memory (gives up after TUXBOX_COMPILE_BUDGET_WAIT_MS); {} MiB physical and {} MiB commit free now, {} still waiting ({n} waited so far; TUXBOX_COMPILE_BUDGET=0 off)",
                shader_bytes >> 10, estimate >> 20, start.elapsed().as_millis(), physical >> 20, commit >> 20,
                WAITING.load(Ordering::Relaxed)));
        }
    }
    Some(Admitted(estimate, large))
}

static MEASURED: AtomicU64 = AtomicU64::new(0);
static ACTIVE: AtomicU64 = AtomicU64::new(0);
/// Largest peak growth seen, in bytes per KiB of DXIL.
static WORST_PER_KIB: AtomicU64 = AtomicU64::new(0);

/// One driver compile being measured; dropping it logs the result.
pub(super) struct Measured {
    shader_bytes: u64,
    start: u64,
    started: std::time::Instant,
    peak: std::sync::Arc<AtomicU64>,
    done: std::sync::Arc<std::sync::atomic::AtomicBool>,
    others: u64,
}

pub(super) fn measure(shader_bytes: u64) -> Option<Measured> {
    if shader_bytes < TINY_SHADER_BYTES || crate::env_toggle!("TUXBOX_COMPILE_MEASURE") == Some("0") {
        return None;
    }
    let start = crate::diagnostics::memory::private_bytes();
    let peak = std::sync::Arc::new(AtomicU64::new(start));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (p, d) = (peak.clone(), done.clone());
    let sampler = std::thread::Builder::new().name("TuxBox compile measure".into()).spawn(move || {
        while !d.load(Ordering::Relaxed) {
            p.fetch_max(crate::diagnostics::memory::private_bytes(), Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    });
    if sampler.is_err() {
        return None;
    }
    let others = ACTIVE.fetch_add(1, Ordering::Relaxed);
    Some(Measured { shader_bytes, start, started: std::time::Instant::now(), peak, done, others })
}

impl Drop for Measured {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        ACTIVE.fetch_sub(1, Ordering::Relaxed);
        let end = crate::diagnostics::memory::private_bytes();
        let peak = self.peak.load(Ordering::Relaxed).max(end).saturating_sub(self.start);
        let kept = end.saturating_sub(self.start);
        let kib = (self.shader_bytes >> 10).max(1);
        // Only compiles that ran alone say what one compile takes.
        if self.others == 0 {
            WORST_PER_KIB.fetch_max(peak / kib, Ordering::Relaxed);
        }
        let n = MEASURED.fetch_add(1, Ordering::Relaxed) + 1;
        let (physical, commit) = available();
        crate::diagnostics::log_line(format_args!(
            "compile budget: measured a driver compile of {} KiB of DXIL: {} ms, private peak +{} MiB ({} B per DXIL byte), +{} MiB still held after; {} other compiles running at its start; worst alone so far {} B per DXIL byte (budget assumes {BYTES_PER_SHADER_BYTE}, at least {} MiB); {} MiB physical, {} MiB commit free ({n} measured; TUXBOX_COMPILE_MEASURE=0 off)",
            self.shader_bytes >> 10, self.started.elapsed().as_millis(), peak >> 20, peak / self.shader_bytes.max(1),
            kept >> 20, self.others, WORST_PER_KIB.load(Ordering::Relaxed) / 1024, MINIMUM_ESTIMATE >> 20,
            physical >> 20, commit >> 20));
    }
}
