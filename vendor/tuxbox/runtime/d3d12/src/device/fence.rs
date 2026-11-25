//! A Series fence exposes a stable, CPU-readable address for the completed value.
use super::{Proxy, Slots, child_slots, unwrap};
use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, OnceLock, Weak};
use windows::Win32::Foundation::{E_FAIL, E_OUTOFMEMORY, HANDLE, WAIT_FAILED};
use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::{HRESULT, Interface, Result};

pub(super) type State = OnceLock<Arc<Word>>;
type Fence = Proxy<ID3D12Fence, State>;

pub(super) struct Word {
    native: ID3D12Fence,
    value: AtomicU64,
}

struct Watch {
    word: Weak<Word>,
    target: Option<u64>,
}

struct Monitor {
    watches: Vec<Watch>,
    running: bool,
}

static MONITOR: Mutex<Monitor> = Mutex::new(Monitor {
    watches: Vec::new(),
    running: false,
});
// D3D12 can retain registrations past the release of a guest proxy. Keep the
// one shared notification handle valid for the lifetime of this provider.
static EVENT: LazyLock<Result<OwnedHandle>> = LazyLock::new(|| {
    let handle = unsafe { CreateEventW(None, false, false, None)? };
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
});

fn event() -> Result<HANDLE> {
    EVENT
        .as_ref()
        .map(|e| HANDLE(e.as_raw_handle()))
        .map_err(Clone::clone)
}

fn refresh(word: &Word) -> u64 {
    let value = unsafe { word.native.GetCompletedValue() };
    // MONITORX waiters can wake on a write of the existing value as well.
    // The cell only rises (palmetto 77d2f6bb); u64::MAX (device removed)
    // stays at the top. TUXBOX_FENCE_CELL_MAX=0 stores as read.
    if word.value.load(Ordering::Relaxed) != value {
        if crate::env_toggle!("TUXBOX_FENCE_CELL_MAX") == Some("0") {
            word.value.store(value, Ordering::Release);
        } else {
            word.value.fetch_max(value, Ordering::AcqRel);
        }
    }
    value
}

fn monitor() {
    let event = event().expect("completion event created before worker");
    loop {
        {
            let mut state = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
            state.watches.retain_mut(|watch| {
                let Some(word) = watch.word.upgrade() else {
                    return false;
                };
                let value = refresh(&word);
                if let Some(next) = value.checked_add(1)
                    && watch.target != Some(next)
                {
                    // Normal queue progress wakes this thread immediately.
                    watch.target = unsafe { word.native.SetEventOnCompletion(next, event) }
                        .ok()
                        .map(|()| next);
                }
                true
            });
            if state.watches.is_empty() {
                state.running = false;
                return;
            }
        }
        // D3D12 also permits rewinding a fence. Such a write cannot satisfy
        // an upward event threshold, so periodically reconcile it as well.
        if unsafe { WaitForSingleObject(event, 16) } == WAIT_FAILED {
            std::thread::sleep(std::time::Duration::from_millis(16));
        }
    }
}

fn address(this: &Fence) -> Result<*const u64> {
    let event = event()?;
    let mut state = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(word) = this.state.get() {
        refresh(word);
        return Ok(word.value.as_ptr());
    }
    let word = Arc::new(Word {
        native: this.native.clone(),
        value: AtomicU64::new(unsafe { this.native.GetCompletedValue() }),
    });
    if !state.running {
        std::thread::Builder::new()
            .name("TuxBox fence completion".into())
            .spawn(monitor)
            .map_err(|_| E_OUTOFMEMORY)?;
        state.running = true;
    }
    state.watches.push(Watch {
        word: Arc::downgrade(&word),
        target: None,
    });
    let result = word.value.as_ptr();
    this.state.set(word).map_err(|_| E_FAIL)?;
    unsafe { SetEvent(event)? };
    Ok(result)
}

/// For the hang report (test107b: the title's GPU watchdog waited until it
/// exited with code 1): every fence whose completed value a title thread can
/// poll at its CPU address (cell, cell value, host completed value), then
/// the event waits still pending (fence, value, age, registering title RVA).
pub(crate) fn hang_lines() -> Vec<String> {
    let mut lines = Vec::new();
    {
        let state = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
        for word in state.watches.iter().filter_map(|w| w.word.upgrade()) {
            let completed = unsafe { word.native.GetCompletedValue() };
            lines.push(format!(
                "fence {:p}: CPU cell {:p} = {}, host completed {completed}",
                word.native.as_raw(), word.value.as_ptr(), word.value.load(Ordering::Acquire)));
        }
    }
    lines.extend(stalls::pending_lines());
    lines
}

unsafe extern "system" fn cpu_address(this: &Fence) -> *const u64 {
    address(this).unwrap_or_else(|e| {
        crate::proxy::fail_call(format_args!("fence completion address: {:?}", e.code()))
    })
}

unsafe extern "system" fn completed(this: &Fence) -> u64 {
    // The title's cell moves with what it reads here.
    match this.state.get() {
        Some(word) => refresh(word),
        None => unsafe { this.native.GetCompletedValue() },
    }
}

unsafe extern "system" fn on_completion(this: &Fence, value: u64, event: HANDLE) -> HRESULT {
    let host = &this.native;
    stalls::register(host, value);
    unsafe { (host.vtable().SetEventOnCompletion)(host.as_raw(), value, event) }
}

/// Report fence event registrations that stay incomplete 10 s, so a title
/// thread blocked on one can be traced to the fence and value it needs. On in
/// survey mode and by default (TUXBOX_FENCE_STALLS=0 off outside survey).
pub(crate) mod stalls {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
    use windows::core::Interface;

    struct Pending { fence: ID3D12Fence, value: u64, since: Instant, rva: usize, reported: bool, early: bool }
    static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

    /// Last value signalled per host fence and by which queue (0: the CPU).
    static SIGNALS: Mutex<Option<std::collections::HashMap<usize, (u64, usize)>>> = Mutex::new(None);

    pub(crate) fn signalled(fence: &ID3D12Fence, value: u64, queue: usize) {
        let mut signals = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        let map = signals.get_or_insert_with(Default::default);
        if map.len() >= 65536 { map.clear(); }
        map.insert(fence.as_raw() as usize, (value, queue));
    }

    fn last_signal(fence: &ID3D12Fence) -> String {
        let signals = SIGNALS.lock().unwrap_or_else(|e| e.into_inner());
        match signals.as_ref().and_then(|m| m.get(&(fence.as_raw() as usize))) {
            Some((value, 0)) => format!("last signalled {value} by the CPU"),
            Some((value, queue)) => format!("last signalled {value} by queue {queue:#x}"),
            None => "never signalled through us".into(),
        }
    }

    /// A wait held this long is logged (black screen: presents stop at world
    /// load; this shows whether a fence signal never comes). TUXBOX_FENCE_WAIT_LOG_MS
    /// sets it; 0 leaves only the 10 s survey line.
    fn threshold() -> Option<Duration> {
        static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let ms = *MS.get_or_init(|| crate::env_toggle!("TUXBOX_FENCE_WAIT_LOG_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(2000));
        (ms != 0).then(|| Duration::from_millis(ms))
    }

    pub(super) fn register(fence: &ID3D12Fence, value: u64) {
        if !crate::survey::enabled() && crate::env_toggle!("TUXBOX_FENCE_STALLS") == Some("0") { return; }
        static START: std::sync::Once = std::sync::Once::new();
        START.call_once(|| { let _ = std::thread::Builder::new().name("TuxBox fence stalls".into()).spawn(scan); });
        let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if pending.len() < 4096 {
            let rva = crate::survey::title_rva().unwrap_or(0);
            pending.push(Pending { fence: fence.clone(), value, since: Instant::now(), rva, reported: false, early: false });
        }
    }

    pub(super) fn pending_lines() -> Vec<String> {
        let pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        pending.iter().filter(|p| unsafe { p.fence.GetCompletedValue() } < p.value).take(32).map(|p| format!(
            "fence {:p} event wait for value {} pending {:.1} s (completed {}), registered from title RVA {:#x}",
            p.fence.as_raw(), p.value, p.since.elapsed().as_secs_f64(), unsafe { p.fence.GetCompletedValue() }, p.rva)).collect()
    }

    fn scan() {
        loop {
            std::thread::sleep(if threshold().is_some() { Duration::from_millis(500) } else { Duration::from_secs(5) });
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            pending.retain(|p| unsafe { p.fence.GetCompletedValue() } < p.value);
            if let Some(limit) = threshold() {
                static LOGGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                for p in pending.iter_mut().filter(|p| !p.early && p.since.elapsed() > limit) {
                    p.early = true;
                    let n = LOGGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if n <= 32 || n.is_power_of_two() {
                        let completed = unsafe { p.fence.GetCompletedValue() };
                        crate::diagnostics::log_line(format_args!(
                            "fence wait: fence {:p} waited for value {} {:.1} s, completed {completed}, {}, registered from title RVA {:#x} ({n} so far; TUXBOX_FENCE_WAIT_LOG_MS=0 off)",
                            p.fence.as_raw(), p.value, p.since.elapsed().as_secs_f64(), last_signal(&p.fence), p.rva));
                    }
                }
            }
            for p in pending.iter_mut().filter(|p| !p.reported && p.since.elapsed() > Duration::from_secs(10)) {
                p.reported = true;
                let completed = unsafe { p.fence.GetCompletedValue() };
                let line = format!("survey: fence {:p} event for value {} still pending after {:?} (completed {completed}), registered from title RVA {:#x}",
                    p.fence.as_raw(), p.value, p.since.elapsed(), p.rva);
                crate::diagnostics::log_once(&std::sync::Once::new(), || line);
            }
        }
    }
}

unsafe extern "system" fn signal(this: &Fence, value: u64) -> HRESULT {
    let host = &this.native;
    stalls::signalled(host, value, 0);
    let hr = unsafe { (host.vtable().Signal)(host.as_raw(), value) };
    if hr.is_ok()
        && let Some(word) = this.state.get()
    {
        let _guard = MONITOR.lock().unwrap_or_else(|e| e.into_inner());
        refresh(word);
        if let Ok(event) = event() {
            let _ = unsafe { SetEvent(event) };
        }
    }
    hr
}

pub(super) static TABLE: Slots<16> = {
    let mut slots = child_slots::<ID3D12Fence, State, 16>();
    slots[8] = crate::slot!(completed);
    slots[9] = crate::slot!(on_completion);
    slots[10] = crate::slot!(signal);
    slots[12] = crate::slot!(cpu_address);
    slots
};

pub(super) unsafe fn native<'a>(
    object: *mut c_void,
    owner: *mut c_void,
) -> Option<&'a ID3D12Fence> {
    unsafe { unwrap::<ID3D12Fence, State>(object, owner, &TABLE) }
        .map(|proxy| &proxy.native)
}
