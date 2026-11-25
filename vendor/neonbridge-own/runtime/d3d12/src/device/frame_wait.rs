//! One process-wide budget per presented frame for every title-thread wait on
//! a pipeline still being made (compute and graphics binds, the dispatch
//! recheck, slot 57 and quad-mask variants: all go through
//! `stream::background::wait_for`). Copied from ztrx's VIctory fork,
//! crates/victory-gpu/src/pipeline/make.rs:98-109 (`wait_budget`), 149-210
//! (`shared_budget`, `wait`; ztrx 4e17478, identical in VIctory-fxceless,
//! GPL-3.0, credit to its authors), split at the seam into `take` (the
//! reservation before their condvar wait) and `give_back` (the settling after
//! it): a waiting thread takes all that is left of the frame's budget and
//! gives back what it did not use, so five recording threads cannot each wait
//! the whole budget. The frame is the count of presents (their
//! `frame::current()`). Once spent, a wait returns at once and its draw or
//! dispatch is skipped as the existing over-budget paths skip.
//! TUXBOX_FRAME_PIPELINE_WAIT_MS (500; 0 off, today's behaviour);
//! TUXBOX_FRAME_PIPELINE_WAIT_SHARED=0 counts per wait as their
//! VICTORY_PIPELINE_WAIT_SHARED=0 does.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Presents so far (ztrx: `crate::frame::current()`).
static FRAME: AtomicU64 = AtomicU64::new(0);
static SKIPPED: AtomicU64 = AtomicU64::new(0);

/// Waiting allowed per title frame, over all threads (ztrx make.rs:98-109).
fn wait_budget() -> Duration {
    static BUDGET: OnceLock<Duration> = OnceLock::new();
    *BUDGET.get_or_init(|| {
        let millis = crate::env_toggle!("TUXBOX_FRAME_PIPELINE_WAIT_MS")
            .and_then(|value| value.trim().parse::<u64>().ok())
            .unwrap_or(500);
        crate::diagnostics::log_line(format_args!(
            "frame pipeline wait budget: {millis} ms per frame shared by all threads (TUXBOX_FRAME_PIPELINE_WAIT_MS, 0 off)"));
        Duration::from_millis(millis)
    })
}

/// ztrx make.rs:149-160: on by default; =0 counts each wait as it ends.
fn shared_budget() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_FRAME_PIPELINE_WAIT_SHARED") != Some("0"))
}

/// The frame the budget belongs to, and what was waited of it (or is taken
/// by a thread waiting now). ztrx make.rs:168-170.
static SPENT: Mutex<(u64, Duration)> = Mutex::new((u64::MAX, Duration::ZERO));

/// Whether waits draw on a per-frame budget (TUXBOX_FRAME_PIPELINE_WAIT_MS not 0).
pub(crate) fn active() -> bool {
    !wait_budget().is_zero()
}

/// A frame was presented: the next frame has its own budget.
pub(crate) fn refill() {
    let _ = wait_budget();
    FRAME.fetch_add(1, Ordering::Relaxed);
}

/// What a waiting thread took of a frame's budget.
pub(crate) struct Grant {
    frame: u64,
    left: Duration,
    /// How long the thread may wait.
    pub(crate) wait: Duration,
}

/// ztrx make.rs:163-185 (the reservation): takes what is left of this
/// frame's budget; None when it is spent (the caller skips its wait). Off,
/// `want` is granted unchanged and nothing is counted.
pub(crate) fn take(want: Duration) -> Option<Grant> {
    let budget = wait_budget();
    let frame = FRAME.load(Ordering::Relaxed);
    if budget.is_zero() {
        return Some(Grant { frame: u64::MAX, left: Duration::ZERO, wait: want });
    }
    let shared = shared_budget();
    let left = {
        let mut spent = SPENT.lock().unwrap_or_else(|e| e.into_inner());
        if spent.0 != frame {
            *spent = (frame, Duration::ZERO);
        }
        let left = budget.saturating_sub(spent.1);
        if shared {
            spent.1 += left;
        }
        left
    };
    if left.is_zero() {
        let n = SKIPPED.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 16 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "frame pipeline wait budget spent: a wait for a pipeline still being made returned at once, its draw/dispatch skipped ({n} so far; TUXBOX_FRAME_PIPELINE_WAIT_MS=0 off)"));
        }
        return None;
    }
    Some(Grant { frame, left, wait: left.min(want) })
}

/// ztrx make.rs:197-210 (the settling): gives back what the wait did not use.
pub(crate) fn give_back(grant: Grant, used: Duration) {
    if grant.frame == u64::MAX {
        return;
    }
    let (frame, left) = (grant.frame, grant.left);
    let waited = used.min(left);
    let shared = shared_budget();
    let mut spent = SPENT.lock().unwrap_or_else(|e| e.into_inner());
    // A frame that began meanwhile has its own budget.
    if spent.0 == frame {
        if shared {
            spent.1 = spent.1.saturating_sub(left - waited);
        } else {
            spent.1 += waited;
        }
    }
}
