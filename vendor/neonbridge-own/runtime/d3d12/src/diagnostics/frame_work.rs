//! What the title asked to draw and what became of it, per 10 s heartbeat:
//! test58 reached the first mission with a black world and nothing in the log
//! said whether draws were dropped (pipeline still being made, untranslatable,
//! descriptor table not fitting, a refused call) or recorded and lost with
//! their batch. Counts are of recorded calls, not of GPU work.
//! TUXBOX_FRAME_WORK=0 turns the line off.
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

#[derive(Clone, Copy)]
pub(crate) enum Work {
    DrawCalls,
    DrawsRecorded,
    DispatchCalls,
    DispatchesRecorded,
    DroppedPending,
    DroppedTable,
    BindsUntranslatable,
    BindsRefused,
    ClearsRtv,
    ClearsDepth,
    ClearsUav,
    RefusedCalls,
    GraphicsOnOtherLists,
    BatchesDropped,
    PipelinesRefused,
    PipelinesUntranslatable,
    NullPipelines,
    /// A ztrx draw/dispatch with no identity heap, or one whose root view
    /// did not resolve (test187: 0 recorded, every drop counter 0).
    DroppedZtrxHeap,
    DroppedRoot,
    Presents,
}

const N: usize = Work::Presents as usize + 1;
static COUNTS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];

fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_FRAME_WORK") != Some("0")
}

#[inline]
pub(crate) fn add(work: Work) {
    COUNTS[work as usize].fetch_add(1, Relaxed);
}

/// The counts since the previous call, or None when off.
pub(crate) fn report() -> Option<String> {
    if !enabled() {
        return None;
    }
    let c: Vec<u64> = COUNTS.iter().map(|c| c.swap(0, Relaxed)).collect();
    let w = |k: Work| c[k as usize];
    use Work::*;
    Some(format!(
        "frame work in 10 s ({} presents): draws {} called, {} recorded; dispatches {} called, {} recorded; \
         dropped draws/dispatches: pipeline still being made {}, descriptor table not fitting {}, ztrx with no identity heap {}, root view not resolved {}; \
         pipeline binds: untranslatable {}, refused {}; clears: rtv {}, depth {}, uav {}; \
         refused recording calls dropped {}, graphics calls on compute/copy lists {}, batches dropped {}; \
         pipelines refused at creation {}, untranslatable in the background {}, null substitutes {} (TUXBOX_FRAME_WORK=0 off)",
        w(Presents), w(DrawCalls), w(DrawsRecorded), w(DispatchCalls), w(DispatchesRecorded),
        w(DroppedPending), w(DroppedTable), w(DroppedZtrxHeap), w(DroppedRoot), w(BindsUntranslatable), w(BindsRefused),
        w(ClearsRtv), w(ClearsDepth), w(ClearsUav),
        w(RefusedCalls), w(GraphicsOnOtherLists), w(BatchesDropped),
        w(PipelinesRefused), w(PipelinesUntranslatable), w(NullPipelines),
    ))
}
