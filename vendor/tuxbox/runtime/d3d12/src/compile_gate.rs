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
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
    use std::time::Duration;

    /// The most holders seen at once while `jobs` threads pass through `gate`.
    fn peak(gate: &Gate, jobs: usize) -> usize {
        let (live, most) = (AtomicUsize::new(0), AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..jobs {
                scope.spawn(|| {
                    let _slot = gate.acquire(|_| {});
                    most.fetch_max(live.fetch_add(1, SeqCst) + 1, SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    live.fetch_sub(1, SeqCst);
                });
            }
        });
        most.load(SeqCst)
    }

    #[test]
    fn no_more_than_the_slot_count_run_together() {
        assert_eq!(peak(&Gate::new(2), 8), 2);
        assert_eq!(peak(&Gate::new(1), 4), 1);
    }

    #[test]
    fn zero_slots_means_unlimited() {
        assert!(peak(&Gate::new(0), 6) > 2);
    }

    #[test]
    fn only_a_caller_that_must_wait_is_told() {
        let gate = Gate::new(1);
        let told = AtomicUsize::new(0);
        let first = gate.acquire(|_| panic!("a free slot needs no wait"));
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _second = gate.acquire(|slots| {
                    told.store(slots, SeqCst);
                });
            });
            while told.load(SeqCst) == 0 {
                std::thread::yield_now();
            }
            drop(first);
        });
        assert_eq!(told.load(SeqCst), 1);
    }

    #[test]
    fn a_panicking_compile_frees_its_slot() {
        let gate = Gate::new(1);
        let crashed = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let _slot = gate.acquire(|_| {});
                    panic!("driver compile failed");
                })
                .join()
        });
        assert!(crashed.is_err());
        let _again = gate.acquire(|_| panic!("the slot should be free"));
    }

    #[test]
    fn small_shaders_never_take_a_slot() {
        assert!(large_compile(LARGE_BYTES - 1, |_| panic!("no wait")).is_none());
        assert!(large_compile(LARGE_BYTES, |_| {}).is_some());
    }

    #[test]
    fn slot_count_comes_from_the_environment_value() {
        assert_eq!(parse_slots(None), DEFAULT_SLOTS);
        assert_eq!(parse_slots(Some("2")), 2);
        assert_eq!(parse_slots(Some(" 0 ")), 0);
        assert_eq!(parse_slots(Some("many")), DEFAULT_SLOTS);
        assert_eq!(parse_slots(Some("-1")), DEFAULT_SLOTS);
    }
}
