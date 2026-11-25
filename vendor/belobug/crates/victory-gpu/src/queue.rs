//! Command queues. Submitted lists run on the host GPU and are waited for,
//! so fence signals complete at once, unless a wait holds the queue back
//! (`timeline`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

use crate::fence::Held;
use crate::object::{self, E_INVALIDARG, HRESULT, Iid, Object, QUEUE, S_OK, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;
use crate::timeline::{Op, Timeline};

const QUEUE_IIDS: [Iid; 4] = [
    guid(
        0x0ec870a6,
        0x5d7e,
        0x4c22,
        [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueueDesc {
    kind: u32,
    priority: i32,
    flags: u32,
    nodes: u32,
}

/// The title reads the list type right after the header.
#[repr(C)]
struct Queue {
    object: Object,
    kind: u32,
    desc: QueueDesc,
    timeline: Arc<Timeline>,
}
const _: () = assert!(std::mem::offset_of!(Queue, kind) == 0x10);

fn queue<'a>(this: This) -> &'a Queue {
    // SAFETY: called from queue methods only.
    unsafe { &*this.cast::<Queue>() }
}

pub(crate) extern "system" fn create(
    _: This,
    desc: *const QueueDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    tracing::debug!(?desc, "command queue");
    let queue = Box::new(Queue {
        object: if crate::device::newer() {
            Object::new(&NEWER_FUNCTIONS)
        } else {
            Object::new(&FUNCTIONS)
        },
        kind: desc.kind,
        desc,
        timeline: Timeline::new(),
    });
    object::hand_out(QUEUE, queue, iid, &QUEUE_IIDS, out)
}

/// The console's queue description: an engine or pipe and a queue on it
/// in place of PC's priority and node mask.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueueDescX {
    kind: u32,
    flags: u32,
    engine_or_pipe: u32,
    queue: u32,
}

/// Device method `CreateCommandQueueX`. The host picks engines itself, so
/// the queue is created as on PC.
pub(crate) extern "system" fn create_x(
    this: This,
    desc: *const QueueDescX,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&x) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    tracing::debug!(desc = ?x, "console command queue");
    let desc = QueueDesc {
        kind: x.kind,
        priority: 0,
        flags: x.flags,
        nodes: 1,
    };
    create(this, &desc, iid, out)
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(
        QUEUE,
        iid,
        object::is(iid, &QUEUE_IIDS).then_some(this),
        out,
    )
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Queue>(this)
}

/// Lists submitted together, run and waited for (decision 0007).
#[cfg(windows)]
struct Submission {
    host: &'static crate::host::Host,
    runs: VecDeque<crate::list::Submitted>,
}

#[cfg(windows)]
impl crate::timeline::Lists for Submission {
    fn ready(&self) -> usize {
        self.runs.iter().take_while(|l| l.unmet().is_empty()).count()
    }
    fn len(&self) -> usize {
        self.runs.len()
    }
    fn early(&mut self) -> bool {
        self.runs.front().is_some_and(|l| l.write_early())
    }
    fn take(&mut self, n: usize) -> Box<dyn FnOnce()> {
        let (host, runs) = (self.host, self.runs.drain(..n).collect::<Vec<_>>());
        Box::new(move || host.execute_title(&runs.iter().map(|l| l.shared()).collect::<Vec<_>>()))
    }
    fn waits(&self) -> String {
        self.runs.iter().map(|l| l.script()).collect::<Vec<_>>().join(" || ")
    }
}

/// Runs the lists once what they wait for is there.
extern "system" fn execute(this: This, count: u32, lists: *const This) {
    if lists.is_null() {
        return;
    }
    #[cfg(windows)]
    if let Some(host) = crate::host::host() {
        // SAFETY: the title passes `count` lists.
        let lists = unsafe { std::slice::from_raw_parts(lists, count as usize) };
        let runs: VecDeque<_> = lists.iter().flat_map(|&l| crate::list::submitted(l)).collect();
        queue(this)
            .timeline
            .submit(Op::Lists(Box::new(Submission { host, runs })));
        return;
    }
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !ONCE.swap(true, Ordering::Relaxed) {
        tracing::warn!(count, "command lists submitted; no host GPU runs them");
    }
}

/// The start of a present plane: frame token, then the plane's resources.
#[repr(C)]
pub(crate) struct Plane {
    token: u64,
    count: u32,
    resources: *const This,
}
/// Submission tagged with the frame it belongs to `(count, lists, frame
/// token, flags)`; frames are paced elsewhere, so it runs as a plain one.
extern "system" fn execute_x(this: This, count: u32, lists: *const This, _token: u64, _flags: u32) {
    execute(this, count, lists);
}
/// Signal with sync settings for the top and bottom of the pipe; our
/// submissions are complete when they return, so it signals at once.
extern "system" fn signal2_x(
    this: This,
    fence: This,
    value: u64,
    _bottom: *const u8,
    _top: *const u8,
) -> HRESULT {
    signal(this, fence, value)
}
/// Markers and events only matter to profilers.
extern "system" fn marker(_: This) {}
/// Signals once the queue's work before it has run.
extern "system" fn signal(this: This, fence: This, value: u64) -> HRESULT {
    if fence.is_null() {
        return E_INVALIDARG;
    }
    queue(this)
        .timeline
        .submit(Op::Signal(Held::new(fence), value));
    S_OK
}
/// Holds back the queue's later work until the fence reaches the value.
extern "system" fn wait(this: This, fence: This, value: u64) -> HRESULT {
    if fence.is_null() {
        return E_INVALIDARG;
    }
    queue(this)
        .timeline
        .submit(Op::Wait(Held::new(fence), value));
    S_OK
}

extern "system" fn timestamp_frequency(_: This, out: *mut u64) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: checked non-null.
    unsafe { out.write(TIMESTAMP_FREQUENCY) };
    S_OK
}
pub(crate) const TIMESTAMP_FREQUENCY: u64 = 100_000_000;

/// The GPU clock: [`TIMESTAMP_FREQUENCY`] ticks per second since the
/// process first asked.
pub(crate) fn gpu_clock() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(std::time::Instant::now).elapsed();
    (elapsed.as_nanos() / (1_000_000_000 / u128::from(TIMESTAMP_FREQUENCY))) as u64
}

/// What a list's bottom-of-pipe write stores when the list has run.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Written {
    Value(u64),
    /// The GPU clock: [`TIMESTAMP_FREQUENCY`] ticks per second since the
    /// process started (our GPU work is done on the CPU's timeline).
    GpuTimestamp,
    /// The CPU's performance counter.
    SystemTimestamp,
    /// A host GPU timestamp read back at this address, put on the GPU clock
    /// by the submission that resolved it (the GPU clock now elsewhere).
    HostTimestamp(usize),
}

impl Written {
    /// Stores the low `bytes` (4 or 8) of the value at `at`.
    pub(crate) fn write(self, at: *mut u8, bytes: usize) {
        let value = match self {
            Written::Value(v) => v,
            Written::GpuTimestamp | Written::HostTimestamp(_) => gpu_clock(),
            Written::SystemTimestamp => system_timestamp(),
        };
        // SAFETY: the title names writable GPU memory (unified with ours).
        // Use atomic Release stores when aligned so concurrent readers (such
        // as a fence's `AtomicU64` value) observe them without a data race.
        unsafe {
            if bytes == 4 {
                if (at as usize).is_multiple_of(align_of::<AtomicU32>()) {
                    (*at.cast::<AtomicU32>()).store(value as u32, Ordering::Release);
                } else {
                    at.cast::<u32>().write_unaligned(value as u32);
                    std::sync::atomic::fence(Ordering::Release);
                }
            } else if (at as usize).is_multiple_of(align_of::<AtomicU64>()) {
                (*at.cast::<AtomicU64>()).store(value, Ordering::Release);
            } else {
                at.cast::<u64>().write_unaligned(value);
                std::sync::atomic::fence(Ordering::Release);
            }
        }
    }
}

fn system_timestamp() -> u64 {
    #[cfg(windows)]
    {
        let mut count = 0;
        // SAFETY: a valid out pointer.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut count) };
        count as u64
    }
    #[cfg(not(windows))]
    0
}

/// Frames presented so far.
static FRAMES: AtomicU64 = AtomicU64::new(0);

pub(crate) fn presented() -> u64 {
    FRAMES.load(Ordering::Relaxed)
}

/// A frame passes (tests that check per-frame work).
#[cfg(test)]
pub(crate) fn pass_frame() {
    FRAMES.fetch_add(1, Ordering::Relaxed);
}

/// The second of several planes: they follow each other with the same frame
/// token. (Where a plane ends is not known; its start is.)
#[cfg(windows)]
fn second_plane(first: *const Plane) -> Option<This> {
    // SAFETY: the title passes plane descriptions.
    let token = unsafe { (*first).token };
    (24..=256).step_by(8).find_map(|offset| {
        let mut words = [0u64; 3];
        // SAFETY: plain words.
        let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), 24) };
        if !crate::host::read_into(first.cast::<u8>().wrapping_add(offset), bytes)
            || words[0] != token
            || !(1..=4).contains(&(words[1] as u32))
            || words[2] < 0x10000
            || words[2] % 8 != 0
        {
            return None;
        }
        let mut resource = [0u8; 8];
        crate::host::read_into(words[2] as *const u8, &mut resource)
            .then(|| u64::from_le_bytes(resource) as This)
            .filter(|r| !r.is_null() && crate::resource::is_texture(*r))
    })
}

/// Shows the planes' first resources in our window, the second over the
/// first.
///
/// shortcut: two planes at most, each over the whole screen, the sources
/// expected in the common state.
extern "system" fn present(
    this: This,
    planes: u32,
    plane_parameters: *const Plane,
    _parameters: *const u8,
) -> HRESULT {
    let mut shown = None;
    #[allow(unused_mut)]
    let mut over: Option<object::Ref> = None;
    #[cfg(windows)]
    if planes > 1 && !plane_parameters.is_null() {
        over = second_plane(plane_parameters).map(object::Ref::new);
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::info!(planes, found = over.is_some(), "several planes presented"));
    }
    if planes > 0
        // SAFETY: the title passes `planes` plane descriptions.
        && let Some(plane) = unsafe { plane_parameters.as_ref() }
        && plane.count > 0
        && !plane.resources.is_null()
    {
        // SAFETY: the plane lists `count` resources.
        let resource = unsafe { *plane.resources };
        shown = (!resource.is_null()).then(|| object::Ref::new(resource));
    }
    // After the work the queue holds back.
    queue(this)
        .timeline
        .submit(Op::Work(Box::new(move || show(shown, over))));
    S_OK
}

/// Shows a frame.
fn show(shown: Option<object::Ref>, over: Option<object::Ref>) {
    let frame = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    #[cfg(windows)]
    crate::shadow::sweep(frame);
    if frame.is_power_of_two() {
        let (draws, dispatches) = crate::list::take_counts();
        #[cfg(windows)]
        let (drawn, dispatched) = {
            static DRAWN: AtomicU64 = AtomicU64::new(0);
            static DISPATCHED: AtomicU64 = AtomicU64::new(0);
            let since = |last: &AtomicU64, now: u64| now - last.swap(now, Ordering::Relaxed);
            (
                since(&DRAWN, crate::draw::drawn()),
                since(&DISPATCHED, crate::draw::dispatched()),
            )
        };
        #[cfg(not(windows))]
        let (drawn, dispatched) = (0, 0);
        tracing::info!(
            frame,
            draws,
            drawn,
            dispatches,
            dispatched,
            "frame presented (since the last line: draws and dispatches, and those run on the host)"
        );
        #[cfg(windows)]
        if frame >= 256 {
            let (hits, misses) = crate::shader_cache::counts();
            tracing::info!(hits, misses, "shaders from the cache and translated so far");
        }
    }
    #[cfg(all(windows, feature = "diagnostics"))]
    if crate::capture::listed(frame) {
        crate::capture::dump_targets(frame);
    }
    #[cfg(all(windows, feature = "diagnostics"))]
    if let Some(target) = crate::capture::present_override(frame)
        && let Some(host) = crate::host::host()
    {
        let state = crate::host::legal_state(&target.texture, target.state.load(Ordering::Relaxed));
        match plane(over.as_ref()) {
            Some((texture, before)) => {
                host.present_over(Some((&target.texture, state)), (texture, before), frame);
            }
            None => {
                host.present(&target.texture, state, frame);
            }
        }
        return;
    }
    #[cfg(windows)]
    if let Some(over) = plane(over.as_ref())
        && let Some(host) = crate::host::host()
    {
        host.present_over(plane(shown.as_ref()), over, frame);
        return;
    }
    #[cfg(windows)]
    if let Some(resource) = shown.as_ref().map(object::Ref::get)
        && let Some(texture) = crate::resource::host_resource(resource)
        && let Some(host) = crate::host::host()
    {
        const COPY_SOURCE: u32 = 0x800;
        let state = crate::resource::host_transition(resource, COPY_SOURCE)
            .map_or(COPY_SOURCE, |(_, before)| before);
        let shown = host.present(texture, state, frame);
        crate::resource::host_transition(resource, state);
        #[cfg(feature = "diagnostics")]
        blinks(frame, resource, state, shown);
        #[cfg(not(feature = "diagnostics"))]
        let _ = shown;
    }
    #[cfg(not(windows))]
    let _ = shown;
}
/// A presented resource's host texture and the host state it is in.
#[cfg(windows)]
fn plane(
    resource: Option<&object::Ref>,
) -> Option<(&'static windows::Win32::Graphics::Direct3D12::ID3D12Resource, u32)> {
    let resource = resource?.get();
    let texture = crate::resource::host_resource(resource)?;
    const COPY_SOURCE: u32 = 0x800;
    let state = crate::resource::host_transition(resource, COPY_SOURCE)
        .map_or(COPY_SOURCE, |(_, before)| before);
    crate::resource::host_transition(resource, state);
    Some((texture, state))
}

/// Diagnostics: logs a presented frame that is black after one that was
/// not, with what was presented and how many draws ran since.
#[cfg(all(windows, feature = "diagnostics"))]
fn blinks(frame: u64, resource: This, state: u32, shown: Option<bool>) {
    static LAST: std::sync::Mutex<(bool, u64, u64)> = std::sync::Mutex::new((false, 0, 0));
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let drawn = crate::draw::drawn();
    let black = shown.unwrap_or(false);
    let draws = crate::draw::take_frame();
    if black && !last.0 {
        last.2 += 1;
        tracing::warn!(
            frame,
            resource = ?resource,
            state = format_args!("{state:#x}"),
            draws = drawn - last.1,
            copied = shown.is_some(),
            black_frames = last.2,
            levels = ?crate::capture::recent_levels(),
            "a black frame after a frame that was not"
        );
        for d in draws {
            tracing::warn!("black frame {frame}: {d}");
        }
        AFTER.store(frame + 6, Ordering::Relaxed);
    }
    // Six frames on: the levels around it (a fade or a single drop).
    static AFTER: AtomicU64 = AtomicU64::new(0);
    if AFTER.load(Ordering::Relaxed) == frame {
        tracing::warn!(frame, levels = ?crate::capture::recent_levels(), "levels after the black frame");
    }
    *last = (black, drawn, last.2);
}

/// A run of destination pages: how they are mapped (0 pool pages from
/// `start` on, 1 pool page `start` each, 2 unmapped, 3 left as they are)
/// and how many.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct PageRange {
    kind: u32,
    start: u32,
    count: u32,
}

/// The ranges mapped from a destination address on.
#[repr(C)]
pub(crate) struct PageBatch {
    destination: u64,
    count: u32,
    ranges: *const PageRange,
}

/// Bytes per mapped page.
const PAGE: u64 = 64 << 10;

/// One page-mapping operation captured from a batch at submission time.
#[derive(Clone, Copy)]
enum PageOp {
    /// Map `len` consecutive pool bytes from `source` at `at`.
    Copy { at: u64, source: u64, len: u64 },
    /// Map the single pool page at `source` repeatedly `count` times from `at`.
    Fill { at: u64, source: u64, count: u64 },
    /// Unmap `len` bytes at `at`.
    Unmap { at: u64, len: u64 },
    /// Out-of-bounds pool index, overflow, or unknown range kind.
    Invalid,
}

/// `CopyPageMappingsBatchX(count, batches, pool, pool pages, flags)`: maps
/// pages of a pool (memory the title mapped physical pages into) at the
/// batches' addresses. Grand Theft Auto VI streams textures into pool pages
/// and maps them into its tiled heap this way (title+0xf2c470, ranges of
/// one constant page or of unmapped pages). Done after every earlier
/// submission: at once if none is held back.
extern "system" fn copy_page_mappings(
    this: This,
    count: u32,
    batches: *const PageBatch,
    pool: u64,
    pool_pages: u32,
    _flags: u32,
) -> HRESULT {
    let Some(map) = crate::resource::page_mapper() else {
        return E_INVALIDARG;
    };
    if batches.is_null() && count > 0 {
        return E_INVALIDARG;
    }
    // Read now: the batch and range arrays belong to the title (often on its
    // stack) and may be reused before a held-back queue runs them.
    let batches = (count > 0).then(|| {
        // SAFETY: the title passes `count` batches.
        unsafe { std::slice::from_raw_parts(batches, count as usize) }
    });
    let mut ops = Vec::new();
    for b in batches.into_iter().flatten() {
        if b.ranges.is_null() || b.count == 0 {
            continue;
        }
        // SAFETY: each batch holds `count` ranges.
        let ranges = unsafe { std::slice::from_raw_parts(b.ranges, b.count as usize) };
        let mut at = b.destination;
        for r in ranges {
            let len = u64::from(r.count) * PAGE;
            let Some(next) = at.checked_add(len) else {
                ops.push(PageOp::Invalid);
                break;
            };
            let inside = |pages: u32| r.start.checked_add(pages).is_some_and(|e| e <= pool_pages);
            let source = || pool.checked_add(u64::from(r.start) * PAGE);
            match r.kind {
                0 if inside(r.count) => match source() {
                    Some(source) => ops.push(PageOp::Copy { at, source, len }),
                    None => ops.push(PageOp::Invalid),
                },
                1 if inside(1) => match source() {
                    Some(source) => ops.push(PageOp::Fill {
                        at,
                        source,
                        count: u64::from(r.count),
                    }),
                    None => ops.push(PageOp::Invalid),
                },
                2 => ops.push(PageOp::Unmap { at, len }),
                3 => {}
                _ => ops.push(PageOp::Invalid),
            }
            at = next;
        }
    }
    queue(this).timeline.submit(Op::Work(Box::new(move || {
        apply_page_mappings(map, pool, &ops)
    })));
    S_OK
}

fn apply_page_mappings(map: crate::resource::PageMapper, pool: u64, ops: &[PageOp]) {
    let mut ok = true;
    for &op in ops {
        ok &= match op {
            PageOp::Copy { at, source, len } => map(at, source, len),
            PageOp::Fill { at, source, count } => {
                (0..count).all(|k| map(at + k * PAGE, source, PAGE))
            }
            PageOp::Unmap { at, len } => map(at, 0, len),
            PageOp::Invalid => false,
        };
    }
    if !ok {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(pool = format_args!("{pool:#x}"), "a page mapping failed")
        });
    }
}

extern "system" fn desc(this: This, out: *mut QueueDesc) {
    if !out.is_null() {
        // SAFETY: checked non-null; the title passes room for a description.
        unsafe { out.write(queue(this).desc) };
    }
}

/// The queue methods by their slot in older titles' queues (the 2020-era
/// layout): IUnknown 3, object 4, device child 1, queue 26 (12 as on PC,
/// then console ones).
const METHODS: [(usize, *const ()); 15] = [
    (0, query as *const ()),
    (2, release as *const ()),
    (7, object::parent_device as *const ()), // 0x38
    (10, execute as *const ()),              // 0x50
    (11, marker as *const ()),               // 0x58 set marker
    (12, marker as *const ()),               // 0x60 begin event
    (13, marker as *const ()),               // 0x68 end event
    (14, signal as *const ()),               // 0x70
    (15, wait as *const ()),                 // 0x78
    (16, timestamp_frequency as *const ()),  // 0x80
    (26, desc as *const ()),                 // 0xd0
    (29, copy_page_mappings as *const ()),   // 0xe8 page mapping batch
    (30, present as *const ()),              // 0xf0
    (33, execute_x as *const ()),            // 0x108
    (32, signal2_x as *const ()),            // 0x100
];

/// Newer titles' queues have two more methods before Signal, which moved
/// from 0x70 to 0x80: every slot from 0x70 on is two later (taken here as
/// starting exactly at 0x70, the methods before it kept in place).
/// Inferred from Grand Theft Auto VI's calls: Signal 0x80 `(fence,
/// value)`, timestamp frequency 0x90 (title+0x24841a6), suspend and resume
/// 0xb8/0xc0, PresentX 0x100, the frame-token submission 0x118 `(count,
/// lists, token, flags)` (title+0x24821dd).
const fn newer_slot(slot: usize) -> usize {
    if slot >= 14 { slot + 2 } else { slot }
}

const fn queue_table(newer: bool) -> [*const (); 36] {
    let mut f = object::d3d12_object(object::missing_table::<QUEUE, 36>());
    let mut i = 0;
    while i < METHODS.len() {
        let (slot, method) = METHODS[i];
        f[if newer { newer_slot(slot) } else { slot }] = method;
        i += 1;
    }
    f
}

static FUNCTIONS: Table<36> = Table(queue_table(false));
static NEWER_FUNCTIONS: Table<36> = Table(queue_table(true));

#[cfg(test)]
pub(crate) fn test_queue() -> (This, usize) {
    let desc = QueueDesc {
        kind: 0,
        priority: 0,
        flags: 0,
        nodes: 0,
    };
    let mut q = std::ptr::null();
    assert_eq!(
        create(std::ptr::null(), &desc, &QUEUE_IIDS[0], &mut q),
        S_OK
    );
    // SAFETY: one of our queues; its first field is the table.
    let table = unsafe { *q.cast::<*const *const ()>() };
    let signal_slot = if table == NEWER_FUNCTIONS.0.as_ptr() {
        0x80
    } else {
        0x70
    };
    (q, signal_slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::slot;

    /// Where a queue's signal is: newer layouts have it two slots later.
    /// Another test may switch layouts while this one runs, so the queue's
    /// own table decides.
    fn signal_slot(q: This) -> usize {
        // SAFETY: one of our queues; its first field is the table.
        let table = unsafe { *q.cast::<*const *const ()>() };
        if table == NEWER_FUNCTIONS.0.as_ptr() {
            0x80
        } else {
            0x70
        }
    }

    #[test]
    fn queue_signals_complete_at_once() {
        let null = std::ptr::null();
        let desc = QueueDesc {
            kind: 0,
            priority: 0,
            flags: 0,
            nodes: 0,
        };
        let mut q = null;
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut q), S_OK);
        // SAFETY: the title reads the type at +0x10.
        assert_eq!(unsafe { q.cast::<u8>().add(0x10).cast::<u32>().read() }, 0);
        let mut fence = null;
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        assert_eq!(
            crate::fence::create(null, 0, 0, fence_iid, &mut fence),
            S_OK
        );
        let signal = slot::<extern "system" fn(This, This, u64) -> HRESULT>(q, signal_slot(q));
        assert_eq!(signal(q, fence, 7), S_OK);
        assert_eq!(
            slot::<extern "system" fn(This) -> u64>(fence, 0x40)(fence),
            7
        );
        assert_eq!(slot::<extern "system" fn(This) -> u32>(q, 0x10)(q), 0);
    }

    /// Two queues: the first waits for a value the second signals later.
    /// The first queue's signal after its wait must not complete before.
    #[test]
    fn work_after_a_wait_runs_once_the_other_queue_signals() {
        type Call = extern "system" fn(This, This, u64) -> HRESULT;
        let null = std::ptr::null();
        let desc = QueueDesc {
            kind: 0,
            priority: 0,
            flags: 0,
            nodes: 0,
        };
        let (mut waits, mut signals) = (null, null);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut waits), S_OK);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut signals), S_OK);
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        let (mut ready, mut done) = (null, null);
        assert_eq!(
            crate::fence::create(null, 0, 0, fence_iid, &mut ready),
            S_OK
        );
        assert_eq!(crate::fence::create(null, 0, 0, fence_iid, &mut done), S_OK);
        let completed = |f: This| slot::<extern "system" fn(This) -> u64>(f, 0x40)(f);

        let signal = |q: This| slot::<Call>(q, signal_slot(q));
        let wait = |q: This| slot::<Call>(q, signal_slot(q) + 8);
        assert_eq!(wait(waits)(waits, ready, 1), S_OK);
        assert_eq!(signal(waits)(waits, done, 1), S_OK);
        assert_eq!(completed(done), 0, "ran before the value was reached");

        assert_eq!(signal(signals)(signals, ready, 1), S_OK);
        assert_eq!(completed(done), 1);
        // Not held any more: work runs when submitted again.
        assert_eq!(signal(waits)(waits, done, 2), S_OK);
        assert_eq!(completed(done), 2);

        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        for object in [waits, signals, ready, done] {
            assert_eq!(release(object), 0);
        }
    }

    /// A present submitted after a wait must not run until the wait is
    /// reached, must read its stack plane parameters at submission time and
    /// must keep its resource alive until it runs.
    #[test]
    fn present_runs_after_a_queues_wait() {
        type Sync = extern "system" fn(This, This, u64) -> HRESULT;
        type Present = extern "system" fn(This, u32, *const Plane, *const u8) -> HRESULT;
        let null = std::ptr::null();
        let desc = QueueDesc {
            kind: 0,
            priority: 0,
            flags: 0,
            nodes: 0,
        };
        let (mut waits, mut signals) = (null, null);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut waits), S_OK);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut signals), S_OK);
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        let (mut ready, mut done) = (null, null);
        assert_eq!(
            crate::fence::create(null, 0, 0, fence_iid, &mut ready),
            S_OK
        );
        assert_eq!(crate::fence::create(null, 0, 0, fence_iid, &mut done), S_OK);
        let resource = crate::resource::test_buffer(256);

        let signal = |q: This| slot::<Sync>(q, signal_slot(q));
        let wait = |q: This| slot::<Sync>(q, signal_slot(q) + 8);
        let present = |q: This| slot::<Present>(q, signal_slot(q) + 0x80);
        let completed = |f: This| slot::<extern "system" fn(This) -> u64>(f, 0x40)(f);
        let add_ref = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x08)(o);
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);

        let before = FRAMES.load(Ordering::Relaxed);
        assert_eq!(wait(waits)(waits, ready, 1), S_OK);
        {
            // Caller's plane parameters may live on its stack and be gone
            // before the queue wakes.
            let resources = Box::new([resource]);
            let plane = Box::new(Plane {
                token: 1,
                count: 1,
                resources: resources.as_ptr(),
            });
            assert_eq!(present(waits)(waits, 1, &*plane, std::ptr::null()), S_OK);
        }
        assert_eq!(signal(waits)(waits, done, 1), S_OK);

        assert_eq!(
            FRAMES.load(Ordering::Relaxed),
            before,
            "presented before the wait was reached"
        );
        assert_eq!(completed(done), 0);
        // The queued present holds a reference of its own.
        assert_eq!(add_ref(resource), 3);
        assert_eq!(release(resource), 2);

        assert_eq!(signal(signals)(signals, ready, 1), S_OK);
        assert_eq!(FRAMES.load(Ordering::Relaxed), before + 1);
        assert_eq!(completed(done), 1);
        assert_eq!(release(resource), 0, "present gave back its reference");

        // Not held any more: present runs at once.
        assert_eq!(
            present(waits)(waits, 0, std::ptr::null(), std::ptr::null()),
            S_OK
        );
        assert_eq!(FRAMES.load(Ordering::Relaxed), before + 2);

        for object in [waits, signals, ready, done] {
            assert_eq!(release(object), 0);
        }
    }

    /// Page mappings submitted after a wait must not run until the wait is
    /// reached, and must capture caller-owned batch/range arrays at call time.
    #[test]
    fn page_mappings_run_after_a_queues_wait() {
        use std::sync::Mutex;

        static CALLS: Mutex<Vec<(u64, u64, u64)>> = Mutex::new(Vec::new());
        fn record(to: u64, from: u64, len: u64) -> bool {
            CALLS
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push((to, from, len));
            true
        }
        crate::resource::use_page_mapper(record);

        type Sync = extern "system" fn(This, This, u64) -> HRESULT;
        type CopyPages = extern "system" fn(This, u32, *const PageBatch, u64, u32, u32) -> HRESULT;
        let null = std::ptr::null();
        let desc = QueueDesc {
            kind: 0,
            priority: 0,
            flags: 0,
            nodes: 0,
        };
        let (mut waits, mut signals) = (null, null);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut waits), S_OK);
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut signals), S_OK);
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        let (mut ready, mut done) = (null, null);
        assert_eq!(
            crate::fence::create(null, 0, 0, fence_iid, &mut ready),
            S_OK
        );
        assert_eq!(crate::fence::create(null, 0, 0, fence_iid, &mut done), S_OK);

        let signal = |q: This| slot::<Sync>(q, signal_slot(q));
        let wait = |q: This| slot::<Sync>(q, signal_slot(q) + 8);
        let copy_pages = |q: This| slot::<CopyPages>(q, signal_slot(q) + 0x78);
        let completed = |f: This| slot::<extern "system" fn(This) -> u64>(f, 0x40)(f);
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);

        let pool = 0x1000_0000_u64;
        let dst = 0x2000_0000_u64;
        CALLS.lock().unwrap().clear();
        assert_eq!(wait(waits)(waits, ready, 1), S_OK);
        {
            // Heap-allocated so they are freed before the queue wakes.
            let ranges = Box::new([
                PageRange {
                    kind: 0,
                    start: 1,
                    count: 2,
                },
                PageRange {
                    kind: 1,
                    start: 3,
                    count: 2,
                },
                PageRange {
                    kind: 2,
                    start: 0,
                    count: 1,
                },
                PageRange {
                    kind: 3,
                    start: 0,
                    count: 1,
                },
            ]);
            let batch = Box::new(PageBatch {
                destination: dst,
                count: ranges.len() as u32,
                ranges: ranges.as_ptr(),
            });
            assert_eq!(copy_pages(waits)(waits, 1, &*batch, pool, 4, 0), S_OK);
        }
        assert_eq!(signal(waits)(waits, done, 1), S_OK);

        assert!(
            CALLS.lock().unwrap().is_empty(),
            "page mappings ran before the wait was reached"
        );
        assert_eq!(completed(done), 0);

        assert_eq!(signal(signals)(signals, ready, 1), S_OK);
        assert_eq!(completed(done), 1);
        assert_eq!(
            *CALLS.lock().unwrap(),
            vec![
                (dst, pool + PAGE, 2 * PAGE),
                (dst + 2 * PAGE, pool + 3 * PAGE, PAGE),
                (dst + 3 * PAGE, pool + 3 * PAGE, PAGE),
                (dst + 4 * PAGE, 0, PAGE),
            ]
        );

        // Not held any more: runs at once.
        CALLS.lock().unwrap().clear();
        let range = PageRange {
            kind: 2,
            start: 0,
            count: 1,
        };
        let batch = PageBatch {
            destination: dst,
            count: 1,
            ranges: &range,
        };
        assert_eq!(copy_pages(waits)(waits, 1, &batch, pool, 4, 0), S_OK);
        assert_eq!(*CALLS.lock().unwrap(), vec![(dst, 0, PAGE)]);

        for object in [waits, signals, ready, done] {
            assert_eq!(release(object), 0);
        }
    }

    /// A queue held back behind a fence whose memory is written directly
    /// (bypassing `Signal` and bottom-of-pipe writes) wakes without needing
    /// another submission on that queue.
    #[test]
    fn direct_memory_write_wakes_held_queue() {
        type Sync = extern "system" fn(This, This, u64) -> HRESULT;
        let null = std::ptr::null();
        let desc = QueueDesc {
            kind: 0,
            priority: 0,
            flags: 0,
            nodes: 0,
        };
        let mut waits = null;
        assert_eq!(create(null, &desc, &QUEUE_IIDS[0], &mut waits), S_OK);
        let fence_iid = &crate::fence::FENCE_IIDS[0];
        let (mut ready, mut done) = (null, null);
        assert_eq!(
            crate::fence::create(null, 0, 0, fence_iid, &mut ready),
            S_OK
        );
        assert_eq!(crate::fence::create(null, 0, 0, fence_iid, &mut done), S_OK);

        let signal = |q: This| slot::<Sync>(q, signal_slot(q));
        let wait = |q: This| slot::<Sync>(q, signal_slot(q) + 8);
        let completed = |f: This| slot::<extern "system" fn(This) -> u64>(f, 0x40)(f);
        let address = |f: This| slot::<extern "system" fn(This) -> u64>(f, 0x60)(f) as usize;
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);

        assert_eq!(wait(waits)(waits, ready, 4), S_OK);
        assert_eq!(signal(waits)(waits, done, 1), S_OK);
        assert_eq!(completed(done), 0);

        // SAFETY: `address(ready)` is the fence's `AtomicU64` value.
        unsafe { (*(address(ready) as *const AtomicU64)).store(4, Ordering::Release) };

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while completed(done) < 1 && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            completed(done),
            1,
            "held queue did not wake on direct write"
        );

        for object in [waits, ready, done] {
            assert_eq!(release(object), 0);
        }
    }

    /// Grand Theft Auto VI's compute queue: `{COMPUTE, 0, pipe 1, queue 0}`.
    #[test]
    fn console_queues_are_created_as_on_pc() {
        let null = std::ptr::null();
        let desc = QueueDescX {
            kind: 2,
            flags: 0,
            engine_or_pipe: 1,
            queue: 0,
        };
        let mut q = null;
        assert_eq!(create_x(null, &desc, &QUEUE_IIDS[0], &mut q), S_OK);
        assert_eq!(queue(q).desc.kind, 2);
        let none = std::ptr::null();
        assert_eq!(create_x(null, none, &QUEUE_IIDS[0], &mut q), E_INVALIDARG);
    }

    #[test]
    fn newer_queues_have_their_methods_two_slots_later() {
        let (old, new) = (queue_table(false), queue_table(true));
        assert_eq!(old[10], new[10]); // submission stays at 0x50
        assert_eq!(old[14], new[16]); // signal 0x70 → 0x80
        assert_eq!(old[30], new[32]); // present 0xf0 → 0x100
        assert_eq!(new[35], execute_x as *const ()); // 0x118
    }

    #[test]
    fn timestamps_and_values_are_written() {
        let mut at = [0u64; 2];
        Written::Value(0x1_0000_0002).write(at.as_mut_ptr().cast(), 4);
        assert_eq!(at[0], 2);
        Written::GpuTimestamp.write(at[1..].as_mut_ptr().cast(), 8);
        let first = at[1];
        std::thread::sleep(std::time::Duration::from_millis(2));
        Written::GpuTimestamp.write(at[1..].as_mut_ptr().cast(), 8);
        assert!(at[1] > first);
    }
}
