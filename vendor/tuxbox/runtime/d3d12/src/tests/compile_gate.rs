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
