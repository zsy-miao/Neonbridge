//! Where each desktop queue has got to, for explaining a stalled memory wait.
//!
//! After every batch and every fence operation the queue signals a private
//! fence. When a wait stalls, the first operation past that fence's value is
//! the one its queue is stopped on.
use std::collections::VecDeque;
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::*;

/// Operations kept per queue.
const KEEP: usize = 256;

struct Op {
    mark: u64,
    text: String,
    /// The fence and value a title wait or signal named.
    fence: Option<(ID3D12Fence, u64)>,
}

struct Track {
    queue: usize,
    kind: i32,
    progress: ID3D12Fence,
    mark: u64,
    ops: VecDeque<Op>,
}

// SAFETY: D3D12 fences are free-threaded.
unsafe impl Send for Track {}

static TRACKS: Mutex<Vec<Track>> = Mutex::new(Vec::new());

/// Note a wait TuxBox itself just put on `queue`.
pub(crate) fn waited(queue: &ID3D12CommandQueue, what: &str, fence: &ID3D12Fence, value: u64) {
    // SAFETY: the queue is live for this call.
    let kind = unsafe { queue.GetDesc() }.Type.0;
    note(queue, kind, what.into(), Some((fence.clone(), value)));
}

/// Note an operation just given to `queue`.
pub(crate) fn note(
    queue: &ID3D12CommandQueue,
    kind: i32,
    text: String,
    fence: Option<(ID3D12Fence, u64)>,
) {
    use windows::core::Interface;
    let key = queue.as_raw().addr();
    let mut tracks = TRACKS.lock().unwrap_or_else(|e| e.into_inner());
    let at = match tracks.iter().position(|track| track.queue == key) {
        Some(at) => at,
        None => {
            let mut device: Option<ID3D12Device> = None;
            // SAFETY: the queue is live for this call.
            if unsafe { queue.GetDevice(&mut device) }.is_err() {
                return;
            }
            let Some(device) = device else { return };
            // SAFETY: a plain fence on the queue's device.
            let Ok(progress) =
                (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) })
            else {
                return;
            };
            tracks.push(Track { queue: key, kind, progress, mark: 0, ops: VecDeque::new() });
            tracks.len() - 1
        }
    };
    let track = &mut tracks[at];
    track.mark += 1;
    // SAFETY: the queue and its private fence are live.
    if unsafe { queue.Signal(&track.progress, track.mark) }.is_err() {
        return;
    }
    if track.ops.len() >= KEEP {
        track.ops.pop_front();
    }
    track.ops.push_back(Op { mark: track.mark, text, fence });
}

/// Log, for each queue that is behind, the operations it has not passed.
pub(crate) fn report() {
    let tracks = TRACKS.lock().unwrap_or_else(|e| e.into_inner());
    for track in tracks.iter() {
        // SAFETY: the private fence is live.
        let done = unsafe { track.progress.GetCompletedValue() };
        let behind = track.mark.saturating_sub(done);
        crate::diagnostics::log(&format!(
            "queue {:#x} type {} passed {done} of {} operations",
            track.queue, track.kind, track.mark
        ));
        if behind == 0 {
            continue;
        }
        for op in track.ops.iter().filter(|op| op.mark > done).take(6) {
            let fence = op.fence.as_ref().map_or_else(String::new, |(fence, value)| {
                use windows::core::Interface;
                // SAFETY: the clone keeps the title's fence alive.
                let now = unsafe { fence.GetCompletedValue() };
                format!(" fence {:#x} value {value} (now {now})", fence.as_raw().addr())
            });
            crate::diagnostics::log(&format!(
                "queue {:#x} pending #{}: {}{fence}",
                track.queue, op.mark, op.text
            ));
        }
    }
}
