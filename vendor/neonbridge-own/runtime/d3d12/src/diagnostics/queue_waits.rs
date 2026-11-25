//! What each queue was last told to wait for on the GPU: test75's GPU memory
//! waits stalled 2 s while their writer, on one queue, ran 400-533 ms late and
//! never passed its progress fence, with nothing saying what held that
//! queue. Every queue Wait (the title's and ours) is noted here with its
//! kind; a stalled wait names what is still unsignalled on its writer's
//! queue. TUXBOX_QUEUE_WAITS=0 keeps no record.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::Graphics::Direct3D12::ID3D12Fence;
use windows::core::Interface;

fn on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_QUEUE_WAITS") != Some("0"))
}

struct Entry {
    kind: &'static str,
    fence: ID3D12Fence,
    value: u64,
    at: Instant,
}

static WAITS: Mutex<Option<HashMap<usize, VecDeque<Entry>>>> = Mutex::new(None);

/// Records that `queue` was told to wait for `fence` to reach `value`.
pub(crate) fn note(queue: usize, kind: &'static str, fence: &ID3D12Fence, value: u64) {
    if !on() {
        return;
    }
    let mut waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    let list = waits.get_or_insert_with(HashMap::new).entry(queue).or_default();
    // Completed waits are dropped from the front as new ones arrive.
    while list.front().is_some_and(|e| unsafe { e.fence.GetCompletedValue() } >= e.value) {
        list.pop_front();
    }
    if list.len() >= 64 {
        list.pop_front();
    }
    list.push_back(Entry { kind, fence: fence.clone(), value, at: Instant::now() });
}

/// Fence -> the queue that signals it, for fences of our own queues (the
/// placed-texture upload copy queues): describe follows a held fence to its
/// signalling queue's own waits one level, so a cycle through an upload's
/// waits shows in the stall line (TUXBOX_QUEUE_WAITS_CHAIN, default on, =0
/// one level as before). test182's hang was inferred: the line named upload
/// fence 0x1d08fa17b50 at 11508 of 11510 but not what its copy queue awaited.
static SIGNALLERS: Mutex<Option<HashMap<usize, usize>>> = Mutex::new(None);

pub(crate) fn signaller(fence: &ID3D12Fence, queue: usize) {
    if !on() {
        return;
    }
    SIGNALLERS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(fence.as_raw() as usize, queue);
}

fn chain() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_QUEUE_WAITS_CHAIN") != Some("0"))
}

/// The waits on `queue` whose fence has not reached its value, oldest first;
/// a held fence that one of our queues signals is followed one level.
pub(crate) fn describe(queue: usize) -> String {
    if !on() {
        return String::new();
    }
    let mut text = describe_one(queue);
    if chain() {
        let behind: Vec<usize> = {
            let waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
            let signallers = SIGNALLERS.lock().unwrap_or_else(|e| e.into_inner());
            waits.as_ref().and_then(|w| w.get(&queue)).into_iter().flatten()
                .filter(|e| unsafe { e.fence.GetCompletedValue() } < e.value)
                .filter_map(|e| signallers.as_ref()?.get(&(e.fence.as_raw() as usize)).copied())
                .filter(|&q| q != queue)
                .collect()
        };
        let mut seen = Vec::new();
        for q in behind {
            if seen.contains(&q) { continue; }
            seen.push(q);
            text.push_str(&format!(" [its signalling queue{}]", describe_one(q)));
        }
    }
    text
}

fn describe_one(queue: usize) -> String {
    let waits = WAITS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(list) = waits.as_ref().and_then(|w| w.get(&queue)) else {
        return format!("; queue {queue:#x} has no recorded waits");
    };
    let open: Vec<String> = list.iter()
        .filter_map(|e| {
            let now = unsafe { e.fence.GetCompletedValue() };
            (now < e.value).then(|| format!("{} fence {:p} at {now} of {} for {} ms", e.kind, e.fence.as_raw(), e.value, e.at.elapsed().as_millis()))
        })
        .take(4)
        .collect();
    if open.is_empty() {
        format!("; queue {queue:#x} waits on no unsignalled fence")
    } else {
        format!("; queue {queue:#x} is held by: {}", open.join(", "))
    }
}
