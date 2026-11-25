//! Bound how many large translated compute shaders the driver compiles at once.
//!
//! The title creates pipelines on many threads. Translated compute programs of
//! several hundred KiB take the driver minutes and on the order of a gigabyte
//! each. With sixteen in flight ProjectScarlett filled its memory limit and
//! swap, and none of the larger ones finished in nine minutes. Smaller shaders
//! are not limited.

use std::sync::{Condvar, LazyLock, Mutex};

/// Translated DXIL of at least this size takes a slot. ProjectScarlett's
/// compiles that ran for more than 30 seconds were all 700 KiB or larger, and
/// 95% of its compute translations are below 450 KiB.
const LARGE_BYTES: usize = 512 << 10;
const DEFAULT_SLOTS: usize = 4;

/// `TUXBOX_LARGE_COMPILES`: how many large compiles may run at once. `0`
/// removes the limit.
static LARGE: LazyLock<Gate> = LazyLock::new(|| {
    Gate::new(parse_slots(std::env::var("TUXBOX_LARGE_COMPILES").ok().as_deref()))
});

fn parse_slots(value: Option<&str>) -> usize {
    value.and_then(|text| text.trim().parse().ok()).unwrap_or(DEFAULT_SLOTS)
}

pub(crate) struct Gate {
    /// Zero means unlimited.
    slots: usize,
    running: Mutex<usize>,
    freed: Condvar,
}

/// One running large compile. Dropping it frees the slot.
pub(crate) struct Slot<'a>(&'a Gate);

impl Gate {
    fn new(slots: usize) -> Self {
        Self { slots, running: Mutex::new(0), freed: Condvar::new() }
    }

    /// Block until a slot is free. `waiting` is called once, with the slot
    /// count, if the caller has to wait.
    fn acquire(&self, waiting: impl FnOnce(usize)) -> Slot<'_> {
        let mut running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        if self.slots != 0 && *running >= self.slots {
            waiting(self.slots);
            while *running >= self.slots {
                running = self.freed.wait(running).unwrap_or_else(|e| e.into_inner());
            }
        }
        *running += 1;
        Slot(self)
    }
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        *self.0.running.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
        self.0.freed.notify_one();
    }
}

pub(crate) fn is_large(bytes: usize) -> bool {
    bytes >= LARGE_BYTES
}

/// Hold the returned value across the pipeline creation call. Small shaders get
/// `None` and never wait.
pub(crate) fn large_compile(bytes: usize, waiting: impl FnOnce(usize)) -> Option<Slot<'static>> {
    is_large(bytes).then(|| LARGE.acquire(waiting))
}

#[cfg(test)]
#[path = "tests/compile_gate.rs"]
mod tests;
