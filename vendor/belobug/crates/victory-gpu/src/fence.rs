//! Fences: a 64-bit value and the events waiting for it. Nothing runs on a
//! GPU yet, so only signals (the title's, later our queues') move one.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::object::{self, E_NOTIMPL, FENCE, HRESULT, Iid, Object, S_OK, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;

pub(crate) const FENCE_IIDS: [Iid; 4] = [
    guid(
        0x0a753dcf,
        0xc4d8,
        0x4b91,
        [0xad, 0xf6, 0xbe, 0x5a, 0x60, 0xd9, 0x5a, 0x76],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
struct Fence {
    object: Object,
    /// Titles also read it straight from memory, at the fence's GPU
    /// address.
    value: AtomicU64,
    flags: u32,
    /// Events to set once the value reaches theirs (handles as integers).
    waiters: Mutex<Vec<(u64, usize)>>,
}

fn fence<'a>(this: This) -> &'a Fence {
    // SAFETY: called from fence methods only.
    unsafe { &*this.cast::<Fence>() }
}

pub(crate) extern "system" fn create(
    _: This,
    initial: u64,
    flags: u32,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    tracing::debug!(initial, flags, "fence");
    let fence = Box::new(Fence {
        object: Object::new(&FUNCTIONS),
        value: AtomicU64::new(initial),
        flags,
        waiters: Mutex::new(Vec::new()),
    });
    object::hand_out(FENCE, fence, iid, &FENCE_IIDS, out)
}

fn set_event(event: usize) {
    #[cfg(windows)]
    // SAFETY: the title passed an event handle.
    unsafe {
        windows_sys::Win32::System::Threading::SetEvent(event as *mut c_void)
    };
    #[cfg(not(windows))]
    let _ = event;
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(
        FENCE,
        iid,
        object::is(iid, &FENCE_IIDS).then_some(this),
        out,
    )
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Fence>(this)
}

impl Drop for Fence {
    fn drop(&mut self) {
        let this = &raw const *self as usize;
        waited().retain(|&f| f != this);
    }
}

/// Fences with events waiting for them.
static WAITED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
fn waited() -> std::sync::MutexGuard<'static, Vec<usize>> {
    WAITED.lock().unwrap_or_else(|e| e.into_inner())
}

/// Sets the events of fences whose value was written in memory (by a list's
/// bottom-of-pipe write, say).
pub(crate) fn wake() {
    waited().retain(|&f| {
        // SAFETY: fences leave the list before they are freed.
        let fence = unsafe { &*(f as *const Fence) };
        let mut waiters = fence.waiters.lock().unwrap_or_else(|e| e.into_inner());
        fence.set_reached(&mut waiters);
        !waiters.is_empty()
    });
}

impl Fence {
    fn set_reached(&self, waiters: &mut Vec<(u64, usize)>) {
        let value = self.value.load(Ordering::Acquire);
        waiters.retain(|&(wanted, event)| {
            let reached = wanted <= value;
            if reached {
                set_event(event);
            }
            !reached
        });
    }
}

/// A fence a queue's pending work names.
pub(crate) struct Held(object::Ref);

impl Held {
    /// `this` is a fence, not null.
    pub(crate) fn new(this: This) -> Held {
        Held(object::Ref::new(this))
    }

    pub(crate) fn reached(&self, value: u64) -> bool {
        completed(self.0.get()) >= value
    }

    /// Moves the fence and sets its events; wakes no queue.
    pub(crate) fn signal(&self, value: u64) {
        set(self.0.get(), value);
    }
}
extern "system" fn completed(this: This) -> u64 {
    fence(this).value.load(Ordering::Acquire)
}
extern "system" fn set_event_on_completion(this: This, value: u64, event: *mut c_void) -> HRESULT {
    let fence = fence(this);
    let mut waiters = fence.waiters.lock().unwrap_or_else(|e| e.into_inner());
    if fence.value.load(Ordering::Acquire) >= value {
        if !event.is_null() {
            set_event(event as usize);
        }
        return S_OK;
    }
    if event.is_null() {
        // A null event means "block until reached".
        // shortcut: only work still to run on a queue can reach it later (the
        // title's other threads aside), so fail instead of hanging if there
        // is none.
        drop(waiters);
        while fence.value.load(Ordering::Acquire) < value {
            if !crate::timeline::active() {
                // It may just have run.
                if fence.value.load(Ordering::Acquire) >= value {
                    break;
                }
                tracing::warn!(value, "blocking fence wait not implemented");
                return E_NOTIMPL;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        return S_OK;
    }
    waiters.push((value, event as usize));
    drop(waiters);
    let mut all = waited();
    let at = &raw const *fence as usize;
    if !all.contains(&at) {
        all.push(at);
    }
    drop(all);
    // The value may have been written since (nothing locks memory writes).
    if fence.value.load(Ordering::Acquire) >= value {
        let mut waiters = fence.waiters.lock().unwrap_or_else(|e| e.into_inner());
        fence.set_reached(&mut waiters);
    }
    S_OK
}
fn set(this: This, value: u64) {
    let fence = fence(this);
    let mut waiters = fence.waiters.lock().unwrap_or_else(|e| e.into_inner());
    fence.value.store(value, Ordering::Release);
    fence.set_reached(&mut waiters);
}
/// The CPU's signal: queues may wait for it.
extern "system" fn signal(this: This, value: u64) -> HRESULT {
    set(this, value);
    if crate::timeline::held() {
        crate::timeline::pump();
    }
    S_OK
}

extern "system" fn creation_flags(this: This) -> u32 {
    fence(this).flags
}
/// Where the value lives (GPU addresses are CPU addresses).
extern "system" fn gpu_address(this: This) -> u64 {
    (&raw const fence(this).value) as u64
}

/// IUnknown 3, object 4, device child 1, fence 3, fence 1 (2, a console
/// one among them).
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<FENCE, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    f[8] = completed as *const (); // 0x40
    f[9] = set_event_on_completion as *const (); // 0x48
    f[10] = signal as *const (); // 0x50
    f[11] = creation_flags as *const (); // 0x58
    f[12] = gpu_address as *const (); // 0x60
    Table(f)
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::slot;

    #[cfg(windows)]
    #[test]
    fn signals_set_waiting_events() {
        use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

        let mut f = std::ptr::null();
        assert_eq!(create(std::ptr::null(), 1, 0, &FENCE_IIDS[0], &mut f), S_OK);
        assert_eq!(slot::<extern "system" fn(This) -> u64>(f, 0x40)(f), 1);
        // SAFETY: plain auto-reset event.
        let event = unsafe { CreateEventW(std::ptr::null(), 0, 0, std::ptr::null()) };
        assert!(!event.is_null());
        let wait = slot::<extern "system" fn(This, u64, *mut c_void) -> HRESULT>(f, 0x48);
        let signal = slot::<extern "system" fn(This, u64) -> HRESULT>(f, 0x50);
        assert_eq!(wait(f, 5, event), S_OK);
        // SAFETY: a valid event.
        assert_ne!(unsafe { WaitForSingleObject(event, 0) }, WAIT_OBJECT_0);
        assert_eq!(signal(f, 5), S_OK);
        // SAFETY: as above.
        assert_eq!(unsafe { WaitForSingleObject(event, 0) }, WAIT_OBJECT_0);
        assert_eq!(wait(f, 5, event), S_OK, "already reached");
        // SAFETY: as above.
        assert_eq!(unsafe { WaitForSingleObject(event, 0) }, WAIT_OBJECT_0);
        assert_eq!(wait(f, 9, std::ptr::null_mut()), E_NOTIMPL);
        // Grand Theft Auto VI reads the value at the fence's address.
        let at = slot::<extern "system" fn(This) -> u64>(f, 0x60)(f);
        // SAFETY: the fence's value.
        assert_eq!(unsafe { *(at as *const u64) }, 5);
        // SAFETY: ours.
        unsafe { CloseHandle(event) };
        assert_eq!(slot::<extern "system" fn(This) -> u32>(f, 0x10)(f), 0);
    }
}
