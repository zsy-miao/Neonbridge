#[test]
fn deep_recursion_fits_on_the_helper_stack() {
    fn depth(n: u32) -> u32 {
        let pad = std::hint::black_box([0u8; 1024]);
        if n == 0 { pad[0] as u32 } else { depth(n - 1) + std::hint::black_box(1) }
    }
    // About 8 MiB of frames: more than a 1 MiB thread stack holds.
    assert_eq!(super::run_logged("test", || depth(8000)), 8000);
}

#[test]
fn returns_borrowed_results() {
    let text = String::from("pso");
    assert_eq!(super::run_logged("test", || text.len()), 3);
}

#[test]
fn deadline_text_uses_twenty_seconds_unless_zero_or_invalid() {
    use std::time::Duration;
    assert_eq!(super::parse_deadline(None), Duration::from_secs(20));
    assert_eq!(super::parse_deadline(Some("0")), Duration::ZERO);
    assert_eq!(super::parse_deadline(Some("20")), Duration::from_secs(20));
    assert_eq!(super::parse_deadline(Some("nope")), Duration::from_secs(20));
}

#[test]
fn a_zero_deadline_waits_for_the_compile() {
    let _guard = lock();
    let wait = super::run_for("block", std::time::Duration::ZERO, || {
        std::thread::sleep(std::time::Duration::from_millis(30));
        4
    });
    assert!(matches!(wait, super::PipelineWait::Ready(4)));
    assert_eq!(super::inflight(), 0);
}

#[test]
fn a_finished_compile_releases_its_slot() {
    let _guard = lock();
    let wait = super::run_for("fast", std::time::Duration::from_secs(2), || 5);
    assert!(matches!(wait, super::PipelineWait::Ready(5)));
    assert_eq!(super::inflight(), 0);
}

#[test]
fn a_slow_compile_is_detached_and_still_finishes() {
    let _guard = lock();
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let gate_in_job = std::sync::Arc::clone(&gate);
    let done_in_job = std::sync::Arc::clone(&done);
    let started = std::time::Instant::now();
    let wait = super::run_for("slow", std::time::Duration::from_millis(40), move || {
        let (lock, wake) = &*gate_in_job;
        let mut open = lock.lock().unwrap_or_else(|error| error.into_inner());
        while !*open {
            open = wake.wait(open).unwrap_or_else(|error| error.into_inner());
        }
        done_in_job.store(true, std::sync::atomic::Ordering::SeqCst);
        7
    });
    assert!(matches!(wait, super::PipelineWait::Deferred));
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    assert!(!done.load(std::sync::atomic::Ordering::SeqCst));
    {
        let (lock, wake) = &*gate;
        *lock.lock().unwrap_or_else(|error| error.into_inner()) = true;
        wake.notify_one();
    }
    wait_until(|| done.load(std::sync::atomic::Ordering::SeqCst));
    wait_until(|| super::inflight() == 0);
}

#[test]
fn compiles_past_the_cap_are_queued_and_still_run() {
    let _guard = lock();
    let previous = super::MAX_LIVE.swap(2, std::sync::atomic::Ordering::SeqCst);
    struct Restore(usize);
    impl Drop for Restore {
        fn drop(&mut self) {
            super::MAX_LIVE.store(self.0, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let _restore = Restore(previous);
    let gate = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    for _ in 0..2 {
        let gate_in_job = std::sync::Arc::clone(&gate);
        let wait = super::run_for("hold", std::time::Duration::from_millis(30), move || {
            let (lock, wake) = &*gate_in_job;
            let mut open = lock.lock().unwrap_or_else(|error| error.into_inner());
            while !*open {
                open = wake.wait(open).unwrap_or_else(|error| error.into_inner());
            }
        });
        assert!(matches!(wait, super::PipelineWait::Deferred));
    }
    let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ran_in_job = std::sync::Arc::clone(&ran);
    let started = std::time::Instant::now();
    let wait = super::run_for("ninth", std::time::Duration::from_secs(5), move || {
        ran_in_job.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert!(matches!(wait, super::PipelineWait::Deferred));
    assert!(started.elapsed() < std::time::Duration::from_millis(500));
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst));
    {
        let (lock, wake) = &*gate;
        *lock.lock().unwrap_or_else(|error| error.into_inner()) = true;
        wake.notify_all();
    }
    wait_until(|| ran.load(std::sync::atomic::Ordering::SeqCst));
    wait_until(|| super::inflight() == 0);
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn wait_until(done: impl Fn() -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !done() {
        assert!(std::time::Instant::now() < end, "pipeline helper did not finish");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
