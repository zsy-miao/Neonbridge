//! GameInput v0 device status notifications and initial enumeration.
use super::{DEVICES, Object, Value, gi_time, kind, pool};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;
use windows_sys::Win32::Foundation::{E_INVALIDARG, E_OUTOFMEMORY, S_OK};

type Callback = unsafe extern "system" fn(u64, *mut c_void, *const Object, u64, u32, u32);
type Snapshot = [(Arc<Object>, u32); DEVICES];

static REGISTRATIONS: LazyLock<Mutex<HashMap<u64, Arc<Registration>>>> =
    LazyLock::new(Mutex::default);
static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn snapshot() -> Snapshot {
    let mut pool = lock(pool());
    pool.update_devices(gi_time(), false);
    std::array::from_fn(|i| {
        let device = &pool.devices[i];
        let Value::Device { status, .. } = &device.value else {
            unreachable!()
        };
        (device.clone(), status.load(Ordering::Relaxed))
    })
}

#[derive(Default)]
struct State {
    seen: [u32; DEVICES],
    stopped: bool,
    running: Option<thread::ThreadId>,
    armed: bool,
}

struct Registration {
    token: u64,
    slot: Option<usize>,
    kinds: u32,
    status_filter: u32,
    // Only the caller's callback interprets this opaque pointer.
    context: usize,
    callback: Callback,
    state: Mutex<State>,
    changed: Condvar,
}

impl Registration {
    fn dispatch(&self, sample: &Snapshot) {
        for (slot, (device, current)) in sample.iter().enumerate() {
            if self.slot.is_some_and(|i| i != slot)
                || self.kinds != 0 && self.kinds & kind(slot) == 0
            {
                continue;
            }
            let mut state = lock(&self.state);
            if state.stopped {
                return;
            }
            let previous = std::mem::replace(&mut state.seen[slot], *current);
            if (previous ^ current) & self.status_filter == 0 {
                continue;
            }
            state.running = Some(thread::current().id());
            drop(state);
            // The sample owns the device. Guest code runs outside all locks and
            // may read the manager, retain the device, or stop this registration.
            unsafe {
                (self.callback)(
                    self.token,
                    self.context as *mut c_void,
                    Arc::as_ptr(device),
                    gi_time(),
                    *current,
                    previous,
                )
            };
            lock(&self.state).running = None;
            self.changed.notify_all();
        }
    }

    fn poll(&self, initial: Option<Snapshot>) {
        let state = self
            .changed
            .wait_while(lock(&self.state), |s| !s.armed && !s.stopped)
            .unwrap_or_else(|e| e.into_inner());
        if state.stopped {
            return;
        }
        drop(state);
        if let Some(initial) = initial {
            self.dispatch(&initial);
        }
        loop {
            let (state, _) = self
                .changed
                .wait_timeout_while(lock(&self.state), Duration::from_millis(100), |s| {
                    !s.stopped
                })
                .unwrap_or_else(|e| e.into_inner());
            if state.stopped {
                return;
            }
            drop(state);
            self.dispatch(&snapshot());
        }
    }
}

#[unsafe(no_mangle)]
unsafe extern "system" fn gi_register_device(
    _: &Object,
    device: *const Object,
    kinds: u32,
    status_filter: u32,
    enumeration: u32,
    context: *mut c_void,
    callback: Option<Callback>,
    out: *mut u64,
) -> i32 {
    if !out.is_null() {
        unsafe { out.write(0) };
    }
    let Some(callback) = callback else {
        return E_INVALIDARG;
    };
    if enumeration > 2 {
        return E_INVALIDARG;
    }
    let initial = snapshot();
    let slot = initial.iter().position(|(d, _)| Arc::as_ptr(d) == device);
    if !device.is_null() && slot.is_none() {
        return E_INVALIDARG;
    }
    let Ok(token) =
        NEXT_TOKEN.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
    else {
        return E_OUTOFMEMORY;
    };
    let reg = Arc::new(Registration {
        token,
        slot,
        kinds,
        status_filter,
        context: context as usize,
        callback,
        state: Mutex::new(State {
            seen: if enumeration == 0 {
                std::array::from_fn(|i| initial[i].1)
            } else {
                [0; DEVICES]
            },
            ..State::default()
        }),
        changed: Condvar::new(),
    });
    let worker = reg.clone();
    let async_initial = (enumeration == 1).then(|| initial.clone());
    if thread::Builder::new()
        .name("GameInput devices".into())
        .spawn(move || worker.poll(async_initial))
        .is_err()
    {
        return E_OUTOFMEMORY;
    }
    lock(&REGISTRATIONS).insert(token, reg.clone());
    if !out.is_null() {
        unsafe { out.write(token) };
    }
    if enumeration == 2 {
        reg.dispatch(&initial);
    }
    lock(&reg.state).armed = true;
    reg.changed.notify_all();
    S_OK
}

#[unsafe(no_mangle)]
extern "system" fn gi_stop(_: &Object, token: u64) {
    if let Some(reg) = lock(&REGISTRATIONS).get(&token) {
        lock(&reg.state).stopped = true;
        reg.changed.notify_all();
    }
}

#[unsafe(no_mangle)]
extern "system" fn gi_unregister(_: &Object, token: u64, timeout: u64) -> bool {
    let Some(reg) = lock(&REGISTRATIONS).get(&token).cloned() else {
        return false;
    };
    let mut state = lock(&reg.state);
    state.stopped = true;
    reg.changed.notify_all();
    if state.running == Some(thread::current().id()) {
        return false;
    }
    if state.running.is_some() {
        state = if timeout == u64::MAX {
            reg.changed
                .wait_while(state, |s| s.running.is_some())
                .unwrap_or_else(|e| e.into_inner())
        } else {
            reg.changed
                .wait_timeout_while(state, Duration::from_micros(timeout), |s| {
                    s.running.is_some()
                })
                .unwrap_or_else(|e| e.into_inner())
                .0
        };
    }
    if state.running.is_some() {
        return false;
    }
    drop(state);
    lock(&REGISTRATIONS).remove(&token).is_some()
}
