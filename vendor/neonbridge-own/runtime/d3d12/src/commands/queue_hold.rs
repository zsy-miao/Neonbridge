//! TUXBOX_QUEUE_HOLD (default on, =0 off): a title queue holds a submission
//! on the CPU behind a GPU memory wait that is not met yet, as the console
//! does, instead of stalling the native queue on a fence a worker sets.
//!
//! Copied from VIctory-fxceless crates/victory-gpu/src/queue.rs (`Held`,
//! `WAITING`, `hold`, `submit_split`, `release_held`, `value_met`; GPL-3.0,
//! credit to its authors; byte-identical in ztrx), adapted only at the seams:
//! - A `Run` is the tail of one `execute_batch` call (the guest lists from the
//!   first that waits on), replayed whole when it is released, so the work our
//!   submit does around ExecuteCommandLists (shadow refresh, placement,
//!   write-back, its own fence signals) stays in order with it. fxceless
//!   detaches host lists instead; ours are native lists whose fences complete
//!   asynchronously.
//! - Fences are native ID3D12Fences the GPU orders: a title Wait still goes
//!   to the native queue at once (no `Held::Wait`); a title Signal on a queue
//!   that holds anything goes behind it (`Held::Signal`), as in fxceless.
//! - The values a GPU list writes land after the submit returns, so the fence
//!   completion monitor (kept: it mirrors native fence progress to the
//!   title's CPU cells) also calls `release_held` on each tick.
//! - fxceless's shortcut stands: a wait holds its whole list, not only the
//!   commands after it; waits later in a released list are the GPU path's.
use super::*;
use std::sync::Mutex;

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_QUEUE_HOLD") != Some("0"))
}

/// One thing a waiting queue holds back.
enum Held {
    /// A list of the next submission waits for a memory value.
    Value(Arc<memory_wait::Segment>),
    /// A submission tail: the guest lists and the frame of `execute_x`.
    Run(Vec<usize>, Option<(u64, u32)>),
    /// A signal of the queue's, due after what it holds before it.
    Signal(usize, u64),
    /// The queue waits for the native fence to reach the value.
    Wait(ID3D12Fence, u64),
}

// SAFETY: the raw pointers are title objects the title keeps alive while
// their queue holds them (it resets a list only past the fence signalled
// after it, which is held behind it).
unsafe impl Send for Held {}

/// Each waiting queue (its address) and what it holds, in order.
static LINES: Mutex<Vec<(usize, Vec<Held>)>> = Mutex::new(Vec::new());

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

thread_local! {
    /// This thread is running a released entry: the calls it makes run now.
    static RELEASING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn releasing() -> bool {
    RELEASING.with(|r| r.get())
}

/// Runs `f` as a released entry (its submission is not held again).
pub(super) fn run_now(f: impl FnOnce()) {
    let was = RELEASING.with(|r| r.replace(true));
    f();
    RELEASING.with(|r| r.set(was));
}

fn key(queue: &Queue) -> usize {
    queue as *const Queue as usize
}

/// A submission to a queue that honours waits, list by list (fxceless
/// `submit_split`). `waits[i]`: the first GPU memory wait of guest list i.
/// Returns how many lists from the start run now; the rest is held.
pub(super) fn submit_split(queue: &Queue, guest: &[*mut c_void], waits: &[Vec<Arc<memory_wait::Segment>>], frame: Option<(u64, u32)>) -> usize {
    let mut lines = lock(&LINES);
    let held_now = lines.iter().any(|(q, line)| *q == key(queue) && !line.is_empty());
    // Behind what the queue already holds, nothing runs now.
    let cut = if held_now {
        0
    } else {
        (0..guest.len()).find(|&i| waits[i].iter().any(|w| !w.met())).unwrap_or(guest.len())
    };
    if cut == guest.len() {
        return cut;
    }
    static SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEEN.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "lists of a submission are held for a memory value: queue type {}, {} lists, {cut} run at once ({n} so far; TUXBOX_QUEUE_HOLD=0 waits on the GPU)",
            queue.state.kind.0, guest.len()));
    }
    let mut pieces: Vec<Held> = Vec::new();
    let mut from = cut;
    // A list's recording can be replayed once per piece.
    let twice = guest[cut..].iter().enumerate().any(|(i, l)| guest[cut..cut + i].contains(l));
    while from < guest.len() {
        // Up to the next list that waits for anything (met or not: it is
        // looked at again when its turn comes).
        let to = if twice {
            guest.len()
        } else {
            (from + 1..guest.len()).find(|&i| !waits[i].is_empty()).unwrap_or(guest.len())
        };
        // Every wait of the piece's lists, as fxceless pushes by[from..to].
        pieces.extend(waits[from..to].iter().flatten().cloned().map(Held::Value));
        pieces.push(Held::Run(guest[from..to].iter().map(|&l| l as usize).collect(), frame));
        from = to;
    }
    match lines.iter_mut().find(|(q, _)| *q == key(queue)) {
        Some((_, line)) => line.extend(pieces),
        None => lines.push((key(queue), pieces)),
    }
    drop(lines);
    watch();
    cut
}

/// Our lists write their values on the GPU after the submit returns
/// (fxceless's host runs them before it does), so a thread looks at the held
/// values every TUXBOX_QUEUE_HOLD_POLL_US (default 250) while any queue holds.
fn watch() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        let us = crate::env_toggle!("TUXBOX_QUEUE_HOLD_POLL_US").and_then(|v| v.trim().parse().ok()).unwrap_or(250u64).max(50);
        let _ = std::thread::Builder::new().name("TuxBox held queues".into()).spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_micros(us));
            release_held(false);
        });
    });
}

/// A title Wait: held when the fence is not reached or the queue holds
/// anything (fxceless `wait`). Whether it was held.
pub(super) fn hold_wait(queue: &Queue, fence: &ID3D12Fence, value: u64) -> bool {
    if releasing() {
        return false;
    }
    let mut lines = lock(&LINES);
    let holding = lines.iter().any(|(q, line)| *q == key(queue) && !line.is_empty());
    if !holding && unsafe { fence.GetCompletedValue() } >= value {
        return false;
    }
    match lines.iter_mut().find(|(q, _)| *q == key(queue)) {
        Some((_, line)) => line.push(Held::Wait(fence.clone(), value)),
        None => lines.push((key(queue), vec![Held::Wait(fence.clone(), value)])),
    }
    drop(lines);
    watch();
    true
}

/// Adds a signal to what the queue holds, if it holds anything: whether it
/// was added (fxceless `hold` with `Held::Signal`).
pub(super) fn hold_signal(queue: &Queue, fence: *mut c_void, value: u64) -> bool {
    if releasing() {
        return false;
    }
    let mut lines = lock(&LINES);
    let Some((_, line)) = lines.iter_mut().find(|(q, line)| *q == key(queue) && !line.is_empty()) else {
        return false;
    };
    line.push(Held::Signal(fence as usize, value));
    true
}

/// Runs what waiting queues hold as far as their values are there (`all`:
/// all of it), each queue's in the order it was given (fxceless
/// `release_held`). Called where a value may have moved: after a submission,
/// a signal, a CPU fence set, a fence monitor tick; `all` at a present.
pub(crate) fn release_held(all: bool) {
    if !on() || releasing() {
        return;
    }
    // One caller at a time, so a queue's submissions keep their order.
    static ONE: Mutex<()> = Mutex::new(());
    if lock(&LINES).iter().all(|(_, line)| line.is_empty()) {
        return;
    }
    let _one = lock(&ONE);
    loop {
        let mut moved = false;
        let queues: Vec<usize> = lock(&LINES).iter().map(|(q, _)| *q).collect();
        for q in queues {
            loop {
                // Taken off the line only once it may run; the line's lock is
                // let go while it runs (it submits and signals).
                let next = {
                    let mut lines = lock(&LINES);
                    let Some((_, line)) = lines.iter_mut().find(|(k, _)| *k == q) else { break };
                    match line.first() {
                        None => break,
                        Some(Held::Value(wait)) if !all && !wait.met() => break,
                        Some(Held::Wait(fence, value)) if !all && unsafe { fence.GetCompletedValue() } < *value => break,
                        Some(_) => {}
                    }
                    line.remove(0)
                };
                // SAFETY: a live title queue: the title keeps its queues.
                let queue = unsafe { &*(q as *const Queue) };
                let was = RELEASING.with(|r| r.replace(true));
                match next {
                    Held::Value(wait) => {
                        // Passed at a present without being met: what the
                        // queue stood at.
                        if all && !wait.met() {
                            static SEEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                            if SEEN.fetch_add(1, Ordering::Relaxed) < 16 {
                                crate::diagnostics::log_line(format_args!(
                                    "a wait for a memory value was not met at a present: queue {q:#x} runs on"));
                            }
                        }
                    }
                    Held::Run(lists, frame) => {
                        let lists: Vec<*mut c_void> = lists.into_iter().map(|l| l as *mut c_void).collect();
                        unsafe { execute_batch(queue, lists.len() as u32, lists.as_ptr(), frame) };
                    }
                    // Reached (or forced at a present): nothing to send.
                    Held::Wait(..) => {}
                    Held::Signal(fence, value) => {
                        let _ = unsafe { queue_fence::<false>(queue, fence as *mut c_void, value) };
                    }
                }
                RELEASING.with(|r| r.set(was));
                moved = true;
            }
        }
        lock(&LINES).retain(|(_, line)| !line.is_empty());
        if !moved {
            break;
        }
    }
}
