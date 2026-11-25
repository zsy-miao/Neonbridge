//! Joining a title's one-record indirect calls into one host call
//! (`VICTORY_INDIRECT_MERGE=1` in victory-gpu).
//!
//! The title issues its mesh draws one record per `ExecuteIndirect`, each
//! reading the next slot of one argument buffer. Consecutive calls that
//! read the same clamped window of records at a constant step, with the
//! same pipeline, signature and bound state, can run as one call whose
//! command signature has that step as its byte stride and whose most
//! commands is the run's length: every record still gets a command of its
//! own, in the same order.
//!
//! Only the decision lives here, without a GPU, so it can be tested.

use std::sync::atomic::{AtomicU64, Ordering};

/// One indirect call as the run detection sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call {
    /// The title pipeline bound.
    pub pipeline: usize,
    /// PC argument kind (0 draw, 1 indexed draw, 10 mesh dispatch).
    pub kind: u32,
    /// The stride the title's signature names.
    pub stride: u32,
    /// The bound state's generation: anything the title sets or records
    /// between two calls changes it, so equal generations mean equal root
    /// bindings, targets, viewports and the rest.
    pub epoch: u64,
    /// The window the record comes from: its host copy's number and the
    /// copy offset of its first record.
    pub copy: u64,
    pub base: u64,
    /// The window's step between records and how many it holds.
    pub step: u32,
    pub records: u32,
    /// The copy offset of this call's record.
    pub offset: u64,
}

impl Call {
    /// The record's place in its window, if it is one of the window's.
    pub fn slot(&self) -> Option<u32> {
        if self.step == 0 || self.offset < self.base {
            return None;
        }
        let from = self.offset - self.base;
        if !from.is_multiple_of(u64::from(self.step)) {
            return None;
        }
        let slot = from / u64::from(self.step);
        (slot < u64::from(self.records)).then_some(slot as u32)
    }

    /// Whether `other` has the same state and window as this call (all but
    /// the record).
    pub fn same_state(&self, other: &Call) -> bool {
        Call { offset: 0, ..*self } == Call { offset: 0, ..*other }
    }
}

/// Whether `step` is a legal byte stride for a one-argument command
/// signature whose argument is `stride` bytes (at least the argument, a
/// multiple of 4).
pub fn legal_stride(step: u32, stride: u32) -> bool {
    step >= stride && step.is_multiple_of(4) && step > 0
}

/// A run of calls not yet recorded: the first, and how many.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    pub first: Call,
    pub count: u32,
}

impl Run {
    /// A run of `first` alone, if it can start one (its record is in its
    /// window and the window's step is a legal stride).
    pub fn start(first: Call) -> Option<Run> {
        (first.slot().is_some() && legal_stride(first.step, first.stride)).then_some(Run { first, count: 1 })
    }

    /// Whether `next` continues the run: the same state and window, and the
    /// record one step after the run's last, still inside the window.
    pub fn extends(&self, next: &Call) -> bool {
        let (Some(first), Some(slot)) = (self.first.slot(), next.slot()) else {
            return false;
        };
        self.first.same_state(next) && u64::from(slot) == u64::from(first) + u64::from(self.count)
    }

    /// Adds `next` if it continues the run.
    pub fn push(&mut self, next: &Call) -> bool {
        let extends = self.extends(next);
        if extends {
            self.count += 1;
        }
        extends
    }

    /// The host call: byte stride, most commands, and the offset of the
    /// first record in the window's scratch buffer.
    pub fn call(&self) -> (u32, u32, u64) {
        let slot = self.first.slot().unwrap_or(0);
        (self.first.step, self.count, u64::from(slot) * u64::from(self.first.step))
    }
}

/// Run lengths counted in buckets: 1, 2, 3-4, 5-8, 9-16, 17-32, 33-64,
/// 65-128, 129 and more.
pub const BUCKETS: usize = 9;

/// The bucket of a run of `n`.
pub fn bucket(n: u32) -> usize {
    if n <= 1 {
        return 0;
    }
    ((32 - (n - 1).leading_zeros()) as usize).min(BUCKETS - 1)
}

/// The bucket labels, for the log.
pub const BUCKET_NAMES: [&str; BUCKETS] = ["1", "2", "3-4", "5-8", "9-16", "17-32", "33-64", "65-128", "129+"];

/// Per frame-rate line: indirect ops in, host calls out, runs by length.
pub struct Stats {
    pub ops: AtomicU64,
    pub calls: AtomicU64,
    pub runs: [AtomicU64; BUCKETS],
}

impl Stats {
    pub const fn new() -> Stats {
        Stats { ops: AtomicU64::new(0), calls: AtomicU64::new(0), runs: [const { AtomicU64::new(0) }; BUCKETS] }
    }

    /// A host call recorded for `n` ops.
    pub fn recorded(&self, n: u32) {
        self.ops.fetch_add(u64::from(n), Ordering::Relaxed);
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.runs[bucket(n)].fetch_add(1, Ordering::Relaxed);
    }

    /// Ops, calls and runs per bucket since the last take.
    pub fn take(&self) -> (u64, u64, [u64; BUCKETS]) {
        (
            self.ops.swap(0, Ordering::Relaxed),
            self.calls.swap(0, Ordering::Relaxed),
            std::array::from_fn(|i| self.runs[i].swap(0, Ordering::Relaxed)),
        )
    }
}

impl Default for Stats {
    fn default() -> Self {
        Stats::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(offset: u64) -> Call {
        Call { pipeline: 7, kind: 10, stride: 12, epoch: 3, copy: 1, base: 0x100, step: 32, records: 16, offset }
    }

    #[test]
    fn a_walk_at_the_step_is_one_run() {
        let mut run = Run::start(call(0x100)).unwrap();
        for i in 1..16 {
            assert!(run.push(&call(0x100 + 32 * i)), "{i}");
        }
        assert_eq!(run.count, 16);
        assert_eq!(run.call(), (32, 16, 0));
        // Past the window's end.
        assert!(!run.push(&call(0x100 + 32 * 16)));
    }

    #[test]
    fn a_run_may_start_inside_the_window() {
        let mut run = Run::start(call(0x100 + 32 * 5)).unwrap();
        assert!(run.push(&call(0x100 + 32 * 6)));
        assert_eq!(run.call(), (32, 2, 160));
    }

    #[test]
    fn a_skip_repeat_or_step_back_ends_the_run() {
        let run = Run::start(call(0x100)).unwrap();
        assert!(!run.extends(&call(0x100)));
        assert!(!run.extends(&call(0x100 + 64)));
        let run = Run::start(call(0x140)).unwrap();
        assert!(!run.extends(&call(0x120)));
        // Off the step.
        assert!(!run.extends(&call(0x144)));
    }

    #[test]
    fn any_state_change_ends_the_run() {
        let run = Run::start(call(0x100)).unwrap();
        let next = call(0x120);
        assert!(run.extends(&next));
        for changed in [
            Call { pipeline: 8, ..next },
            Call { kind: 0, ..next },
            Call { stride: 16, ..next },
            Call { epoch: 4, ..next },
            Call { copy: 2, ..next },
            Call { base: 0x120, ..next },
            Call { step: 16, ..next },
            Call { records: 32, ..next },
        ] {
            assert!(!run.extends(&changed), "{changed:?}");
        }
    }

    #[test]
    fn records_outside_the_window_or_illegal_steps_start_nothing() {
        assert!(Run::start(call(0xf0)).is_none());
        assert!(Run::start(call(0x104)).is_none());
        assert!(Run::start(Call { step: 8, ..call(0x100) }).is_none());
        assert!(Run::start(Call { step: 14, stride: 12, ..call(0x100) }).is_none());
        assert!(Run::start(Call { step: 0, ..call(0x100) }).is_none());
        assert!(Run::start(Call { step: 16, ..call(0x110) }).is_some());
    }

    #[test]
    fn legal_strides() {
        assert!(legal_stride(32, 12));
        assert!(legal_stride(12, 12));
        assert!(legal_stride(16, 16));
        assert!(!legal_stride(12, 16));
        assert!(!legal_stride(18, 12));
    }

    #[test]
    fn buckets_and_stats() {
        let expect = [(0, 0), (1, 0), (2, 1), (3, 2), (4, 2), (5, 3), (8, 3), (9, 4), (16, 4), (17, 5), (64, 6), (65, 7), (128, 7), (129, 8), (4096, 8)];
        for (n, b) in expect {
            assert_eq!(bucket(n), b, "{n}");
        }
        let s = Stats::new();
        s.recorded(1);
        s.recorded(16);
        s.recorded(3);
        let (ops, calls, runs) = s.take();
        assert_eq!((ops, calls), (20, 3));
        assert_eq!(runs, [1, 0, 1, 0, 1, 0, 0, 0, 0]);
        assert_eq!(s.take().0, 0);
    }
}
