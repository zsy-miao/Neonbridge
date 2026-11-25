//! Process-wide initialization and the caller-visible recursive XG lock.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::{self, ThreadId};

static INITIALIZED: AtomicBool = AtomicBool::new(false);
static VERSION_FOUR: AtomicBool = AtomicBool::new(false);
static LOCK_ENABLED: AtomicBool = AtomicBool::new(false);
static OWNER: Mutex<Option<(ThreadId, usize)>> = Mutex::new(None);
static RELEASED: Condvar = Condvar::new();

#[unsafe(export_name = "XGEnableExclusiveLocking")]
extern "system" fn enable(enabled: u32) {
    LOCK_ENABLED.store(enabled != 0, Ordering::Release);
}

#[unsafe(export_name = "XGEnterExclusiveLock")]
extern "system" fn enter() -> i32 {
    if !LOCK_ENABLED.load(Ordering::Acquire) {
        return 0;
    }
    let id = thread::current().id();
    let owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    let mut owner = RELEASED
        .wait_while(owner, |held| held.is_some_and(|(who, _)| who != id))
        .unwrap_or_else(|e| e.into_inner());
    owner.get_or_insert((id, 0)).1 += 1;
    1
}

#[unsafe(export_name = "XGLeaveExclusiveLock")]
extern "system" fn leave() {
    let mut owner = OWNER.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((id, depth)) = owner.as_mut()
        && *id == thread::current().id()
    {
        *depth -= 1;
        if *depth == 0 {
            *owner = None;
            RELEASED.notify_one();
        }
    }
}

pub(super) struct Guard(bool);

impl Guard {
    pub(super) fn acquire() -> Self {
        Self(enter() != 0)
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if self.0 {
            leave();
        }
    }
}

#[unsafe(export_name = "XGInitializeLibrary")]
pub(super) extern "system" fn initialize() -> i32 {
    i32::from(INITIALIZED.swap(true, Ordering::AcqRel))
}

#[unsafe(export_name = "XGTerminateLibrary")]
extern "system" fn terminate() -> i32 {
    i32::from(!INITIALIZED.swap(false, Ordering::AcqRel))
}

#[unsafe(export_name = "XGSetHardwareVersion")]
extern "system" fn set_hardware_version(version: u32) -> i32 {
    VERSION_FOUR.store(version == 4, Ordering::Release);
    initialize()
}

pub(super) fn version_four() -> bool {
    VERSION_FOUR.load(Ordering::Acquire)
}
