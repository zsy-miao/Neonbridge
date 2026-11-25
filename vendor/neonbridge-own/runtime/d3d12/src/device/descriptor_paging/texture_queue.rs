//! TUXBOX_HEAP_TEXTURE_QUEUE (default on, =0 the whole identity heap counts
//! as sampled at each ztrx draw): textures a ztrx shader picks by index from
//! the identity heap are uploaded once per descriptor write, and looked at
//! again a few frames after (RECHECK_FRAMES), not re-checked at every submit.
//! A slot written with a placed texture's view is queued; each frame the due
//! slots (those looked at before first, then the newest write and the oldest
//! in turns, at most 4096) count as sampled for that frame's submits, whose
//! refresh uploads them within the frame budget. Copied from VIctory-fxceless
//! victory-gpu descriptor/host.rs TextureQueue (written / due / done) and
//! draw.rs heap_textures and RECHECK_FRAMES (GPL-3.0, credit to its authors).
//! Their shortcut stands: a texture the title rewrites with no descriptor
//! write after its last look is missed. Ours: the refresh's write watch
//! decides whether a looked-at texture is read again (fx fingerprints it).
//! test190: counting the whole 999999-slot identity heap as sampled made each
//! submit walk about 1055 textures, at 2.9 fps.
use std::collections::BTreeMap;
use std::sync::Mutex;

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_HEAP_TEXTURE_QUEUE") != Some("0"))
}

/// Frames after an upload a slot is looked at again (fx draw.rs).
const RECHECK_FRAMES: [u64; 5] = [4, 12, 40, 120, 400];

struct Waiting {
    due: u64,
    write: u64,
    looks: u8,
}

struct Queue {
    slots: BTreeMap<usize, Waiting>,
    writes: u64,
    /// The frame `due` was taken for, and the CPU handles due in it.
    frame: u64,
    due: Vec<usize>,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue { slots: BTreeMap::new(), writes: 0, frame: u64::MAX, due: Vec::new() });

fn lock() -> std::sync::MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A placed texture's view was written into identity slot `handle`: due now.
pub(crate) fn written(handle: usize) {
    let mut q = lock();
    q.writes += 1;
    let write = q.writes;
    q.slots.insert(handle, Waiting { due: 0, write, looks: 0 });
}

/// Title memory mapped or unmapped since the queue last took the ranges
/// (fx descriptor/host.rs REMAPPED).
static REMAPPED: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

/// The runtime mapped or unmapped title memory `[lo, hi)`
/// (CopyPageMappingsBatchX): the placed textures there may have arrived,
/// gone, or changed with no descriptor written. Kept until the next ztrx
/// draw's `take_remapped`, whose identity slots over it are queued again.
/// Copied from VIctory-fxceless descriptor/host.rs remapped and
/// texture_slots_remapped (GPL-3.0, credit to its authors), which closes the
/// shortcut above for remapped memory. Locks one mutex and calls nothing.
pub(crate) fn remapped(lo: u64, hi: u64) {
    /// Ranges kept until a draw takes them.
    const MOST: usize = 1 << 15;
    if !enabled() || lo >= hi {
        return;
    }
    let mut ranges = REMAPPED.lock().unwrap_or_else(|e| e.into_inner());
    // Pages are mapped one after the other.
    if let Some(last) = ranges.last_mut() && last.1 == lo {
        last.1 = hi;
        return;
    }
    if ranges.len() >= MOST {
        // No ztrx draw runs: the slots there are looked at when due, as before.
        ranges.clear();
    }
    ranges.push((lo, hi));
}

/// The ranges `remapped` kept since the last call.
pub(crate) fn take_remapped() -> Vec<(u64, u64)> {
    std::mem::take(&mut *REMAPPED.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The identity slot `handle` no longer holds a placed texture.
pub(crate) fn removed(handle: usize) {
    lock().slots.remove(&handle);
}

/// The slots due in `frame` (taken once a frame and kept for its draws):
/// first those looked at before, newest write first; then the others, the
/// newest write and the oldest in turns (fx TextureQueue::due), at most
/// 4096. Each handed slot is due again RECHECK_FRAMES after, then dropped.
pub(crate) fn due(frame: u64) -> Vec<usize> {
    let mut q = lock();
    if q.frame == frame {
        return q.due.clone();
    }
    const MAX: usize = 4096;
    let mut again: Vec<(u64, usize)> = Vec::new();
    let mut fresh: Vec<(u64, usize)> = Vec::new();
    for (&handle, w) in q.slots.iter().filter(|(_, w)| w.due <= frame) {
        if w.looks > 0 { again.push((w.write, handle)) } else { fresh.push((w.write, handle)) }
    }
    again.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    fresh.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let mut order: Vec<usize> = again.iter().map(|&(_, h)| h).take(MAX).collect();
    let (mut new, mut old) = (0, fresh.len());
    while new < old && order.len() < MAX {
        order.push(fresh[new].1);
        new += 1;
        if new < old && order.len() < MAX {
            old -= 1;
            order.push(fresh[old].1);
        }
    }
    for handle in &order {
        let gone = match q.slots.get_mut(handle) {
            Some(w) => match RECHECK_FRAMES.get(usize::from(w.looks)) {
                Some(wait) => {
                    w.looks += 1;
                    w.due = frame + wait;
                    false
                }
                None => true,
            },
            None => false,
        };
        if gone {
            q.slots.remove(handle);
        }
    }
    q.frame = frame;
    q.due = order.clone();
    static LINES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if !order.is_empty() && (n <= 4 || n.is_power_of_two()) {
        crate::diagnostics::log_line(format_args!(
            "heap texture queue: {} identity slots due this frame of {} waiting (TUXBOX_HEAP_TEXTURE_QUEUE=0 off)",
            order.len(), q.slots.len()));
    }
    order
}
