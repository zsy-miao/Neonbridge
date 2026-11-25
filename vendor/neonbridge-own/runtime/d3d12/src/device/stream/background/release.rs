//! TUXBOX_PSO_RELEASE_IDLE (default off; =1 on): a lazy pipeline no bind has
//! used for TUXBOX_PSO_IDLE_S (120) seconds gives its native pipeline back to
//! the driver, and its next bind makes it again on the title's thread from the
//! description kept beside it (the driver's pipeline library and our shader
//! cache serve it, so the rebuild is a load). Built pipelines lived for the
//! process (`Upgrade.built` was a OnceLock), and test175's private commit kept
//! growing with driver allocations long after the load. Recorded lists hold
//! their own reference to the native pipeline they set, so a release only
//! drops ours; the proxy itself is freed TUXBOX_PSO_RETIRE_S (10) seconds
//! later, past any bind still holding a borrow of it. Each freed batch logs
//! the private bytes before and after, to measure what a release returns.
use super::{Built, Job, Upgrade, WAITING, Work};
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use windows::core::IUnknown;

pub(crate) fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PSO_RELEASE_IDLE") == Some("1"))
}

fn idle_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| crate::env_toggle!("TUXBOX_PSO_IDLE_S").and_then(|v| v.trim().parse().ok()).unwrap_or(120u64) * 1000)
}

fn retire_ms() -> u64 {
    static MS: OnceLock<u64> = OnceLock::new();
    *MS.get_or_init(|| crate::env_toggle!("TUXBOX_PSO_RETIRE_S").and_then(|v| v.trim().parse().ok()).unwrap_or(10u64).max(2) * 1000)
}

fn now_ms() -> u64 {
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// `Upgrade.built`: set once by a worker as before, but taken back by a release.
#[derive(Default)]
pub(crate) struct Slot(AtomicPtr<Built>);

impl Slot {
    pub(crate) fn get(&self) -> Option<&Built> {
        // SAFETY: a pointer here came from Box::into_raw; a released one is
        // freed only after the retire delay (see the module comment).
        unsafe { self.0.load(Ordering::Acquire).as_ref() }
    }

    pub(crate) fn set(&self, built: Built) -> Result<(), Built> {
        let p = Box::into_raw(Box::new(built));
        match self.0.compare_exchange(core::ptr::null_mut(), p, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => Ok(()),
            // SAFETY: never published.
            Err(_) => Err(*unsafe { Box::from_raw(p) }),
        }
    }

    fn take(&self) -> Option<Box<Built>> {
        let p = self.0.swap(core::ptr::null_mut(), Ordering::AcqRel);
        // SAFETY: as in `get`; the caller retires it.
        (!p.is_null()).then(|| unsafe { Box::from_raw(p) })
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        drop(self.take());
    }
}

/// What `Upgrade` keeps for a release (only while on).
#[derive(Default)]
pub(crate) struct Kept {
    /// The device and description its worker built it from.
    work: Mutex<Option<(IUnknown, Work)>>,
    last_bind: AtomicU64,
    registered: AtomicBool,
    released: AtomicBool,
}

struct Retired(#[allow(dead_code)] Box<Built>, u64);
// SAFETY: D3D12 objects and their proxies are free-threaded.
unsafe impl Send for Retired {}

static SLOTS: Mutex<Vec<Weak<Upgrade>>> = Mutex::new(Vec::new());
static RETIRED: Mutex<Vec<Retired>> = Mutex::new(Vec::new());
static LAST_PASS: AtomicU64 = AtomicU64::new(0);
static RELEASED: AtomicU64 = AtomicU64::new(0);
static REMADE: AtomicU64 = AtomicU64::new(0);

/// A worker made it: keeps its description for a remake.
pub(super) fn keep(slot: &Upgrade, device: IUnknown, work: Work) {
    if on() {
        *slot.kept.work.lock().unwrap_or_else(|e| e.into_inner()) = Some((device, work));
    }
}

/// A lazy pipeline's bind (compute::current): stamps it, and every 10 s
/// releases the idle ones and frees the retired ones.
pub(crate) fn bound(slot: &Arc<Upgrade>) {
    if !on() {
        return;
    }
    let now = now_ms();
    slot.kept.last_bind.store(now, Ordering::Relaxed);
    if !slot.kept.registered.swap(true, Ordering::Relaxed) {
        SLOTS.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(slot));
    }
    let last = LAST_PASS.load(Ordering::Relaxed);
    if now.saturating_sub(last) >= 10_000
        && LAST_PASS.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
    {
        pass(now);
    }
}

fn pass(now: u64) {
    let idle = idle_ms();
    let mut taken = Vec::new();
    SLOTS.lock().unwrap_or_else(|e| e.into_inner()).retain(|w| {
        let Some(slot) = w.upgrade() else { return false };
        if now.saturating_sub(slot.kept.last_bind.load(Ordering::Relaxed)) >= idle
            && slot.kept.work.lock().unwrap_or_else(|e| e.into_inner()).is_some()
            && let Some(built) = slot.built.take()
        {
            slot.kept.released.store(true, Ordering::Release);
            taken.push(Retired(built, now));
        }
        true
    });
    let retire = retire_ms();
    let mut retired = RETIRED.lock().unwrap_or_else(|e| e.into_inner());
    let (due, held): (Vec<_>, Vec<_>) = std::mem::take(&mut *retired).into_iter().partition(|r| now.saturating_sub(r.1) >= retire);
    *retired = held;
    let fresh = taken.len() as u64;
    retired.extend(taken);
    drop(retired);
    let total = RELEASED.fetch_add(fresh, Ordering::Relaxed) + fresh;
    if !due.is_empty() {
        let before = crate::diagnostics::memory::private_bytes();
        let n = due.len();
        drop(due);
        let after = crate::diagnostics::memory::private_bytes();
        crate::diagnostics::log_line(format_args!(
            "pso release: freed {n} idle pipelines, private {} -> {} MiB ({:+} MiB); {total} released, {} remade so far (TUXBOX_PSO_RELEASE_IDLE=1 on)",
            before >> 20, after >> 20, (after as i64 - before as i64) >> 20, REMADE.load(Ordering::Relaxed)));
    }
}

/// A released pipeline's bind: made again on this thread.
pub(super) fn remake(slot: &Arc<Upgrade>) -> Option<&Built> {
    if !on() || !slot.kept.released.load(Ordering::Acquire) {
        return None;
    }
    let (device, mut work) = slot.kept.work.lock().unwrap_or_else(|e| e.into_inner()).take()?;
    slot.kept.released.store(false, Ordering::Release);
    let graphics = match &mut work {
        Work::Stream(stream) => unsafe { super::super::is_graphics(&stream.desc()) },
        Work::Compute(_) => false,
    };
    let started = std::time::Instant::now();
    WAITING.fetch_add(1, Ordering::Relaxed);
    super::build_here(Job { device, work, graphics, vertex: false, slot: slot.clone() }, std::time::Duration::from_secs(3600));
    let n = REMADE.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "pso release: a released pipeline was bound again and remade in {:.1} ms, {} ({n} so far; TUXBOX_PSO_RELEASE_IDLE=1 on)",
            started.elapsed().as_secs_f64() * 1e3, if slot.built.get().is_some() { "made" } else { "failed" }));
    }
    slot.built.get()
}
