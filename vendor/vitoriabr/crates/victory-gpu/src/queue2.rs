//! More queues on the card, for copies only (`VICTORY_QUEUE2`; off by
//! default): work of ours that does not depend on the order of the title's
//! draws goes to a copy queue of its own and no longer stands in the one
//! direct queue behind them. A copy queue runs on the card's copy engine,
//! beside the engine that draws.
//!
//! Two lanes, each a copy queue with a fence of its own, so that one never
//! stands behind the other:
//!
//! - **down** (bit 1): the fetch of the pieces shaders wrote in copies in
//!   the card's memory (`Host::relay_card_back`, `Host::card_back`). With
//!   the relay that fetch stood behind every flight sent since: 114-157 ms
//!   a frame of the landing thread's 176-229 (run 118).
//! - **up** (bit 2): the title bytes that changed on the CPU's side of such
//!   copies cross to the card's memory (`Host::card_before`), into a
//!   landing buffer of ours there; the direct queue waits for them and
//!   moves them on into the copies, inside the card's memory.
//!
//! What is synchronised, and with what:
//!
//! - A job of a lane may wait for fences of the direct queue
//!   ([`Lane::send`]'s `after`: `ID3D12CommandQueue::Wait` on the lane's
//!   queue). A fetch names the fence value behind the last flight it lands,
//!   so it runs after that flight's lists and its compare.
//! - The direct queue waits for a job of the up lane ([`Second::hold`]:
//!   `Wait` on the direct queue for the lane's fence), so nothing sent to
//!   the direct queue afterwards runs before the bytes have landed.
//! - No wait can stand for ever: a job only ever waits for a value already
//!   signalled on the direct queue, and the direct queue only for a job
//!   already sent, so each wait's partner lies ahead of it in its queue.
//! - Only buffers go through the lanes. A buffer needs no state change for
//!   a copy queue (it is promoted from the common state and falls back to
//!   it), and it may be READ on one queue while another queue's work on it
//!   is in the air; a texture may not, which is why no texture goes this
//!   way.
//! - No lane WRITES a buffer that the direct queue's work in the air
//!   names: the host's debug layer calls that an error ("not safe to start
//!   write GPU operations now on this Command Queue", seen on the software
//!   adapter with a first draft that copied title bytes straight into the
//!   copies). Hence the landing buffers ([`Second::landing`]): each is
//!   written by one job of the up lane and read by the direct queue behind
//!   its wait for that job, and is given out again only when the card has
//!   passed that read. The copies themselves change only on the direct
//!   queue, in its order, as before.
//! - With the relay the sends of title bytes are numbered
//!   (`crate::host::QUEUE_ORDER`). The direct queue signals each number
//!   behind its send ([`Second::sent_up`]), and a fetch on the down lane
//!   takes for "the sends ahead of me" those the card has finished
//!   ([`Second::order_done`]). A later send that the card happens to run
//!   before the fetch is left out by the merge as one that came after
//!   (`Shadow::card_merge_relay`): the title's bytes stand, as they would.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

/// `VICTORY_QUEUE2`, a sum of bits; unset or 0: off, everything goes
/// through the one direct queue as before.
///
/// - 1: fetches of what shaders wrote to copies in the card's memory go to
///   a copy queue (the gain is with `VICTORY_RELAY=1`/`2`; without the
///   relay nothing else is queued when they run, so it only shows that
///   the path works);
/// - 2: the title bytes sent up to copies in the card's memory cross from
///   the PC's memory on a copy queue, into landing buffers in the card's
///   memory ([`landing_most`] says how much of it), and the direct queue
///   moves them on from there.
///
/// Read once, when the first submission asks.
pub(crate) fn bits() -> u32 {
    static BITS: OnceLock<u32> = OnceLock::new();
    *BITS.get_or_init(|| std::env::var("VICTORY_QUEUE2").ok().and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0) & 3)
}

/// The most bytes of landing buffers in the card's memory, for title
/// bytes on their way to copies there (`VICTORY_QUEUE2_LANDING_MB`, 96;
/// a frame sent up 45-76 MB in runs 116-120, and a buffer is free again as
/// soon as the card has run the submission it was for). A send that finds
/// no room goes straight from the PC's memory on the direct queue, as
/// without the switch; the frame line counts those.
fn landing_most() -> u64 {
    static MOST: OnceLock<u64> = OnceLock::new();
    *MOST.get_or_init(|| {
        std::env::var("VICTORY_QUEUE2_LANDING_MB").ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(96) << 20
    })
}

/// A buffer in the card's memory that title bytes land in.
struct Landing {
    buffer: ID3D12Resource,
    size: u64,
    /// The value of the host's fence that the card passes after the direct
    /// queue's last read of the buffer: free once it is reached.
    until: u64,
}

/// Which landing buffer (its size, and the fence value it is held until)
/// takes `bytes` when the host's fence has reached `completed`: the
/// smallest free one that holds them.
fn pick_landing(landings: &[(u64, u64)], bytes: u64, completed: u64) -> Option<usize> {
    landings
        .iter()
        .enumerate()
        .filter(|&(_, &(size, until))| until <= completed && size >= bytes)
        .min_by_key(|&(_, &(size, _))| size)
        .map(|(at, _)| at)
}

/// An allocator and a list of the copy type.
pub(crate) type CopyList = (ID3D12CommandAllocator, ID3D12GraphicsCommandList);

/// Since the last frame-rate line, of one lane: jobs, their bytes,
/// microseconds the lane had a job (sent and not finished: its time on the
/// card and its waits for the direct queue's fences), microseconds a thread
/// of ours stood waiting for its jobs.
#[derive(Default)]
pub(crate) struct LaneStats {
    pub(crate) jobs: AtomicU64,
    pub(crate) bytes: AtomicU64,
    pub(crate) busy_us: AtomicU64,
    pub(crate) waited_us: AtomicU64,
}

struct LaneState {
    /// Jobs sent: the last value signalled on the lane's fence.
    sent: u64,
    /// Lists not in use, each with the job it last ran in: free for the
    /// next job once the card has run that one.
    spare: Vec<(u64, CopyList)>,
    /// To the lane's clock: a job and when it was sent.
    clock: Option<Sender<(u64, Instant)>>,
}

/// A copy queue with a fence of its own; its jobs run in the order sent.
pub(crate) struct Lane {
    device: ID3D12Device,
    queue: ID3D12CommandQueue,
    /// Signalled on `queue` behind each job with the job's number.
    fence: ID3D12Fence,
    state: Mutex<LaneState>,
    pub(crate) stats: LaneStats,
}

// SAFETY: D3D12 devices, queues, fences, allocators and lists are
// free-threaded objects; the lists are recorded by the thread that took
// them and otherwise touched under `state`.
unsafe impl Send for Lane {}
// SAFETY: as above.
unsafe impl Sync for Lane {}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// A lane's busy time grows by the stretch from `sent` (or from when the
/// job before it finished, `before`, if that was later: the lane was busy
/// with that one until then) to `done`. Returns the microseconds.
fn busy_stretch(before: Option<Instant>, sent: Instant, done: Instant) -> u64 {
    let from = before.map_or(sent, |b| b.max(sent));
    done.saturating_duration_since(from).as_micros() as u64
}

impl Lane {
    fn new(device: &ID3D12Device, name: &str) -> windows::core::Result<Lane> {
        // SAFETY: plain object creation with valid descriptions.
        unsafe {
            let queue: ID3D12CommandQueue = device.CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_COPY,
                ..Default::default()
            })?;
            let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
            let wide: Vec<u16> = format!("victory copy queue ({name})").encode_utf16().chain([0]).collect();
            let _ = queue.SetName(windows::core::PCWSTR(wide.as_ptr()));
            Ok(Lane {
                device: device.clone(),
                queue,
                fence,
                state: Mutex::new(LaneState {
                    sent: 0,
                    spare: Vec::new(),
                    clock: None,
                }),
                stats: LaneStats::default(),
            })
        }
    }

    /// Starts the lane's clock: a thread that waits for each job in turn
    /// and adds up how long the lane had one ([`LaneStats::busy_us`]).
    fn clocked(&'static self, name: &str) {
        let (tx, rx) = channel();
        let started = std::thread::Builder::new().name(format!("victory-queue2-{name}")).spawn(move || self.watch(&rx));
        match started {
            Ok(_) => lock(&self.state).clock = Some(tx),
            Err(e) => tracing::warn!(error = %e, "no thread for a copy queue's clock: its milliseconds stay 0"),
        }
    }

    fn watch(&self, jobs: &Receiver<(u64, Instant)>) {
        // SAFETY: a plain event of our own.
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return;
        };
        let mut before = None;
        while let Ok((job, sent)) = jobs.recv() {
            if self.wait(job, event).is_err() {
                return;
            }
            let done = Instant::now();
            self.stats.busy_us.fetch_add(busy_stretch(before, sent, done), Ordering::Relaxed);
            before = Some(done);
        }
    }

    /// An open list of the copy type: one whose last job the card has run,
    /// or a new one.
    pub(crate) fn list(&self) -> windows::core::Result<CopyList> {
        // SAFETY: a plain query; a list is reset only when the job it ran
        // in is done; plain object creation otherwise.
        unsafe {
            let done = self.fence.GetCompletedValue();
            let free = {
                let mut state = lock(&self.state);
                state.spare.iter().position(|(job, _)| *job <= done).map(|at| state.spare.swap_remove(at).1)
            };
            if let Some((allocator, list)) = free {
                allocator.Reset()?;
                list.Reset(&allocator, None)?;
                return Ok((allocator, list));
            }
            let allocator: ID3D12CommandAllocator = self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COPY)?;
            let list: ID3D12GraphicsCommandList =
                self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_COPY, &allocator, None)?;
            Ok((allocator, list))
        }
    }

    /// Closes `list` (from [`Lane::list`], with copies recorded) and sends
    /// it as the lane's next job, of `bytes` bytes. The job runs behind
    /// the lane's earlier jobs and not before each fence of `after` (fences
    /// of other queues) has reached its value: every value must have been
    /// signalled on its queue already. Returns the job's number, for
    /// [`Lane::wait`], [`Lane::done`] and [`Second::hold`].
    pub(crate) fn send(&self, list: CopyList, after: &[(&ID3D12Fence, u64)], bytes: u64) -> windows::core::Result<u64> {
        // SAFETY: an open list of ours; the lane's queue and fence; live
        // fences of the same device.
        unsafe {
            list.1.Close()?;
            let lists = [Some(list.1.cast::<ID3D12CommandList>()?)];
            // One job at a time from its waits to its signal: the jobs'
            // numbers are the order they run in.
            let mut state = lock(&self.state);
            for &(fence, value) in after {
                // (A fence is at 0 before anything is signalled on it:
                // nothing to wait for.)
                if value != 0 {
                    self.queue.Wait(fence, value)?;
                }
            }
            self.queue.ExecuteCommandLists(&lists);
            state.sent += 1;
            let job = state.sent;
            self.queue.Signal(&self.fence, job)?;
            state.spare.push((job, list));
            if let Some(clock) = &state.clock {
                let _ = clock.send((job, Instant::now()));
            }
            self.stats.jobs.fetch_add(1, Ordering::Relaxed);
            self.stats.bytes.fetch_add(bytes, Ordering::Relaxed);
            Ok(job)
        }
    }

    /// Whether the card has run `job`.
    pub(crate) fn done(&self, job: u64) -> bool {
        // SAFETY: a plain query.
        unsafe { self.fence.GetCompletedValue() >= job }
    }

    /// Waits on this thread, with its `event`, until the card has run
    /// `job`.
    pub(crate) fn wait(&self, job: u64, event: HANDLE) -> windows::core::Result<()> {
        // SAFETY: the lane's fence and the caller's event.
        unsafe {
            if self.fence.GetCompletedValue() < job {
                self.fence.SetEventOnCompletion(job, event)?;
                WaitForSingleObject(event, INFINITE);
            }
        }
        Ok(())
    }
}

/// The copy queues and what ties them to the direct queue.
pub(crate) struct Second {
    bits: u32,
    device: ID3D12Device,
    /// The landing buffers made so far ([`Second::landing`]).
    landings: Mutex<Vec<Landing>>,
    /// The landing buffers' bytes alive; since the last frame-rate line,
    /// the sends that found no room in them.
    pub(crate) landing: [AtomicU64; 2],
    /// Fetches (bit 1).
    pub(crate) down: Lane,
    /// Sends of title bytes to copies in the card's memory (bit 2).
    pub(crate) up: Lane,
    /// Signalled on the DIRECT queue behind each numbered send of title
    /// bytes, with its number ([`Second::sent_up`]).
    order: ID3D12Fence,
    /// Signalled on the DIRECT queue just ahead of each wait for the up
    /// lane ([`Second::hold`]), for the clock: when the direct queue got
    /// there. Behind the lock: the holds made so far, and the way to the
    /// thread that times them.
    reached: ID3D12Fence,
    holds: Mutex<(u64, Option<Sender<(u64, u64)>>)>,
    /// Since the last frame-rate line: waits of the direct queue for the up
    /// lane, those of them at which it stood, and for how many
    /// microseconds.
    pub(crate) held: [AtomicU64; 3],
}

// SAFETY: fences (free-threaded) and lanes.
unsafe impl Send for Second {}
// SAFETY: as above.
unsafe impl Sync for Second {}

impl Second {
    /// The lanes on `device`. `bits` as [`bits`].
    pub(crate) fn new(device: &ID3D12Device, bits: u32) -> windows::core::Result<Second> {
        // SAFETY: plain fences on the device.
        unsafe {
            Ok(Second {
                bits,
                device: device.clone(),
                landings: Mutex::new(Vec::new()),
                landing: [const { AtomicU64::new(0) }; 2],
                down: Lane::new(device, "down")?,
                up: Lane::new(device, "up")?,
                order: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                reached: device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?,
                holds: Mutex::new((0, None)),
                held: [const { AtomicU64::new(0) }; 3],
            })
        }
    }

    /// Starts the clocks: the lanes' and the one that times the direct
    /// queue's waits ([`Second::held`]).
    pub(crate) fn clocked(&'static self) {
        self.down.clocked("down");
        self.up.clocked("up");
        let (tx, rx) = channel();
        match std::thread::Builder::new().name("victory-queue2-holds".into()).spawn(move || self.watch_holds(&rx)) {
            Ok(_) => lock(&self.holds).1 = Some(tx),
            Err(e) => tracing::warn!(error = %e, "no thread to time the direct queue's waits for the copy queue: they stay 0"),
        }
    }

    /// Whether fetches go to the down lane.
    pub(crate) fn fetches(&self) -> bool {
        self.bits & 1 != 0
    }

    /// Whether title bytes go up through the up lane.
    pub(crate) fn uploads(&self) -> bool {
        self.bits & 2 != 0
    }

    /// A buffer in the card's memory for `bytes` title bytes on their way
    /// up, for a job of the up lane to write and the direct queue to read
    /// behind its wait for that job ([`Second::hold`]). `completed`: the
    /// value the host's fence has reached; `until`: the value it reaches
    /// when the card has run that read (the next one signalled behind
    /// it). The buffer is not given out again before then, so no job
    /// writes it while anything in the air names it. `None`: no room under
    /// [`landing_most`], or no memory; the caller sends the bytes the old
    /// way.
    pub(crate) fn landing(&self, bytes: u64, completed: u64, until: u64) -> Option<ID3D12Resource> {
        let mut landings = lock(&self.landings);
        let sizes: Vec<(u64, u64)> = landings.iter().map(|l| (l.size, l.until)).collect();
        if let Some(at) = pick_landing(&sizes, bytes, completed) {
            landings[at].until = until;
            return Some(landings[at].buffer.clone());
        }
        // A new one; free ones that are too small make room for it (the
        // card has passed their last use: nothing names them).
        let size = bytes.next_multiple_of(1 << 20);
        let alive = |landings: &[Landing]| landings.iter().map(|l| l.size).sum::<u64>();
        if alive(&landings) + size > landing_most() {
            landings.retain(|l| l.until > completed);
        }
        let made = if alive(&landings) + size > landing_most() {
            None
        } else {
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: size,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                ..Default::default()
            };
            let mut buffer: Option<ID3D12Resource> = None;
            // SAFETY: plain creation of a buffer in the card's memory.
            let made = unsafe {
                self.device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer)
            };
            match (made, buffer) {
                (Ok(()), Some(buffer)) => {
                    let wide: Vec<u16> = "victory landing buffer (title bytes on their way up)".encode_utf16().chain([0]).collect();
                    // SAFETY: a live buffer and a terminated string.
                    let _ = unsafe { buffer.SetName(windows::core::PCWSTR(wide.as_ptr())) };
                    landings.push(Landing {
                        buffer: buffer.clone(),
                        size,
                        until,
                    });
                    Some(buffer)
                }
                _ => None,
            }
        };
        self.landing[0].store(alive(&landings), Ordering::Relaxed);
        if made.is_none() {
            self.landing[1].fetch_add(1, Ordering::Relaxed);
        }
        made
    }

    /// The numbered send `number` of title bytes has just gone to `direct`
    /// (or there was nothing to send under that number): the number is
    /// signalled behind it.
    pub(crate) fn sent_up(&self, direct: &ID3D12CommandQueue, number: u64) {
        // SAFETY: a live queue and our fence.
        let _ = unsafe { direct.Signal(&self.order, number) };
    }

    /// The last numbered send of title bytes the card has finished: every
    /// send up to it is in what a fetch sent from now on brings down.
    pub(crate) fn order_done(&self) -> u64 {
        // SAFETY: a plain query.
        unsafe { self.order.GetCompletedValue() }
    }

    /// Makes `direct` wait for `job` of the up lane: nothing sent to it
    /// from now on runs before the card has run the job. The job must have
    /// been sent ([`Lane::send`]).
    pub(crate) fn hold(&self, direct: &ID3D12CommandQueue, job: u64) -> windows::core::Result<()> {
        let mut holds = lock(&self.holds);
        holds.0 += 1;
        // SAFETY: a live queue and our fences.
        unsafe {
            // For the clock only: the moment the queue gets to the wait.
            let _ = direct.Signal(&self.reached, holds.0);
            direct.Wait(&self.up.fence, job)?;
        }
        self.held[0].fetch_add(1, Ordering::Relaxed);
        if let Some(clock) = &holds.1 {
            let _ = clock.send((holds.0, job));
        }
        Ok(())
    }

    /// Times the holds, in the order they were made: from the moment the
    /// direct queue got to one until the job it waits for is done. (While
    /// it stands at one it gets to no later one, so one thread sees each
    /// in time.)
    fn watch_holds(&self, holds: &Receiver<(u64, u64)>) {
        // SAFETY: a plain event of our own.
        let Ok(event) = (unsafe { CreateEventW(None, false, false, None) }) else {
            return;
        };
        while let Ok((hold, job)) = holds.recv() {
            // SAFETY: our fence and this thread's event.
            unsafe {
                if self.reached.GetCompletedValue() < hold {
                    if self.reached.SetEventOnCompletion(hold, event).is_err() {
                        return;
                    }
                    WaitForSingleObject(event, INFINITE);
                }
            }
            if self.up.done(job) {
                continue;
            }
            let standing = Instant::now();
            if self.up.wait(job, event).is_err() {
                return;
            }
            self.held[1].fetch_add(1, Ordering::Relaxed);
            self.held[2].fetch_add(standing.elapsed().as_micros() as u64, Ordering::Relaxed);
        }
    }
}

/// The copy queues, made at the first ask if [`bits`] says so; `None` when
/// the switch is off or they could not be made (everything then goes
/// through the direct queue, as before).
pub(crate) fn second() -> Option<&'static Second> {
    static SECOND: OnceLock<Option<Second>> = OnceLock::new();
    static CLOCKS: std::sync::Once = std::sync::Once::new();
    if bits() == 0 {
        return None;
    }
    let second = SECOND
        .get_or_init(|| {
            let host = crate::host::host()?;
            match Second::new(&host.device, bits()) {
                Ok(second) => {
                    tracing::info!(
                        fetches = second.fetches(),
                        uploads = second.uploads(),
                        landing_mb = landing_most() >> 20,
                        "copy queues on the card beside the direct queue (VICTORY_QUEUE2={})",
                        bits()
                    );
                    Some(second)
                }
                Err(e) => {
                    tracing::warn!(error = %e, "no copy queues (VICTORY_QUEUE2): everything goes through the direct queue");
                    None
                }
            }
        })
        .as_ref()?;
    CLOCKS.call_once(|| second.clocked());
    Some(second)
}

/// For the frame-rate line: what the copy queues did per frame since the
/// last one. "On their queue" is from a job's send to its end with each
/// moment counted once: its time on the card and any wait of its queue
/// for the direct queue's fences. The direct queue "stood" at a wait if
/// the job was not done when it got there.
pub(crate) fn line(frames: f64) {
    let Some(second) = second() else {
        return;
    };
    let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed) as f64 / frames;
    let ms = |a: &AtomicU64| take(a) / 1000.0;
    let mb = |a: &AtomicU64| take(a) / (1 << 20) as f64;
    let (down, up) = (&second.down.stats, &second.up.stats);
    let (down_jobs, down_mb, down_busy, down_waited) = (take(&down.jobs), mb(&down.bytes), ms(&down.busy_us), ms(&down.waited_us));
    let (up_jobs, up_mb, up_busy) = (take(&up.jobs), mb(&up.bytes), ms(&up.busy_us));
    let [holds, stood, stood_us] = [0, 1, 2].map(|i| take(&second.held[i]));
    let (landing_mb, no_room) = (second.landing[0].load(Ordering::Relaxed) >> 20, take(&second.landing[1]));
    tracing::info!(
        "the copy queues per frame (VICTORY_QUEUE2={}): fetches of what shaders wrote: {down_jobs:.1} jobs, {down_mb:.1} MB, \
         {down_busy:.1} ms on their queue, our threads stood {down_waited:.1} ms waiting for them; title bytes sent up: \
         {up_jobs:.1} jobs, {up_mb:.1} MB, {up_busy:.1} ms on their queue; the direct queue was told to wait for them \
         {holds:.1} times and stood at {stood:.1} of those for {:.1} ms in all; landing buffers on the card: {landing_mb} MB, \
         {no_room:.1} sends found no room in them and went through the direct queue (VICTORY_QUEUE2_LANDING_MB)",
        second.bits,
        stood_us / 1000.0
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::core::s;

    /// A lane's busy time counts each moment once: a job sent while the
    /// one before it still ran adds only what came after that one.
    #[test]
    fn a_lanes_busy_time_counts_each_moment_once() {
        let t = Instant::now();
        let at = |ms: u64| t + std::time::Duration::from_millis(ms);
        // Alone: from the send to the end.
        assert_eq!(busy_stretch(None, at(10), at(14)), 4_000);
        // Sent at 10 while the job before ran until 12: 12 to 14.
        assert_eq!(busy_stretch(Some(at(12)), at(10), at(14)), 2_000);
        // Sent after the one before had finished: from the send.
        assert_eq!(busy_stretch(Some(at(5)), at(10), at(14)), 4_000);
        // (The clock's own lateness never makes a stretch negative.)
        assert_eq!(busy_stretch(Some(at(20)), at(10), at(14)), 0);
    }

    /// A landing buffer is given out again only when the card has passed
    /// its last use, and the smallest one that holds the bytes is taken.
    #[test]
    fn a_landing_buffer_is_free_when_the_card_has_passed_its_use() {
        // (size, held until this value of the host's fence)
        let landings = [(4 << 20, 10), (1 << 20, 12), (2 << 20, 7), (8 << 20, 7)];
        // The card is at 9: the third and fourth are free.
        assert_eq!(pick_landing(&landings, 1 << 20, 9), Some(2));
        assert_eq!(pick_landing(&landings, 3 << 20, 9), Some(3));
        assert_eq!(pick_landing(&landings, 9 << 20, 9), None);
        // At 10 the first is free too, and is the smallest for 3 MiB.
        assert_eq!(pick_landing(&landings, 3 << 20, 10), Some(0));
        // At 6 none is: a use that the card has not passed holds its buffer.
        assert_eq!(pick_landing(&landings, 1, 6), None);
        // The value itself frees it (the signal is behind the read).
        assert_eq!(pick_landing(&landings, 1, 7), Some(2));
        assert_eq!(pick_landing(&[], 1, u64::MAX), None);
    }

    /// A shader of the direct queue that takes a while and then stores a
    /// dword: `at` (bytes) gets `value`.
    const SLOW_STORE: &str = r#"
#define RS "RootConstants(num32BitConstants=3, b0), UAV(u0)"
cbuffer What : register(b0) { uint at; uint value; uint spin; };
RWByteAddressBuffer memory : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    uint h = value;
    [loop]
    for (uint i = 0; i < spin; i++) {
        h = (h ^ memory.Load((i & 63u) * 4u)) * 0x01000193u;
    }
    memory.Store(at, value + (h & 0u));
}
"#;

    /// The software adapter with a direct queue and the copy queues.
    struct Warp {
        device: ID3D12Device,
        direct: ID3D12CommandQueue,
        /// The direct queue's fence and the last value signalled on it.
        fence: ID3D12Fence,
        submitted: u64,
        event: HANDLE,
        second: &'static Second,
        root: ID3D12RootSignature,
        pso: ID3D12PipelineState,
        /// Direct lists sent and not waited for yet.
        flown: Vec<CopyList>,
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl Warp {
        fn new(bits: u32) -> Option<Warp> {
            let serial = crate::host::WARP_SERIAL
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            // SAFETY: plain DXGI and D3D12 set-up on the software adapter.
            unsafe {
                let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).ok()?;
                let adapter: IDXGIAdapter = factory.EnumWarpAdapter().ok()?;
                let desc = adapter.cast::<IDXGIAdapter1>().ok()?.GetDesc1().ok()?;
                if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 == 0 {
                    return None;
                }
                let mut device: Option<ID3D12Device> = None;
                D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut device).ok()?;
                let device = device?;
                let direct: ID3D12CommandQueue = device
                    .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                        Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                        ..Default::default()
                    })
                    .ok()?;
                let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).ok()?;
                let event = CreateEventW(None, false, false, None).ok()?;
                // The clocks' threads live as long as the process: the
                // queues of a test are kept for them.
                let second: &'static Second = Box::leak(Box::new(Second::new(&device, bits).expect("copy queues on WARP")));
                second.clocked();
                let mut code: Option<ID3DBlob> = None;
                D3DCompile(
                    SLOW_STORE.as_ptr().cast(),
                    SLOW_STORE.len(),
                    s!("slow store"),
                    None,
                    None,
                    s!("main"),
                    s!("cs_5_1"),
                    0,
                    0,
                    &mut code,
                    None,
                )
                .expect("the test's shader compiles");
                let code = code?;
                let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
                let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).ok()?;
                let pso: ID3D12PipelineState = device
                    .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                        pRootSignature: std::mem::transmute_copy(&root),
                        CS: D3D12_SHADER_BYTECODE {
                            pShaderBytecode: bytes.as_ptr().cast(),
                            BytecodeLength: bytes.len(),
                        },
                        ..Default::default()
                    })
                    .ok()?;
                Some(Warp {
                    device,
                    direct,
                    fence,
                    submitted: 0,
                    event,
                    second,
                    root,
                    pso,
                    flown: Vec::new(),
                    _serial: serial,
                })
            }
        }

        /// A buffer as the host makes them: in the card's own memory
        /// (`Host::uav_buffer`), or in the PC's memory and mapped
        /// (`Host::shared_buffer`).
        fn buffer(&self, size: u64, mapped: bool) -> (ID3D12Resource, *mut u8) {
            let heap = if mapped {
                D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_CUSTOM,
                    CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
                    MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
                    ..Default::default()
                }
            } else {
                D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                }
            };
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: size,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                ..Default::default()
            };
            let mut buffer: Option<ID3D12Resource> = None;
            let mut data = std::ptr::null_mut();
            // SAFETY: plain creation, and a mapping of CPU-visible memory.
            unsafe {
                self.device
                    .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer)
                    .expect("a test buffer");
                let buffer = buffer.expect("a test buffer");
                if mapped {
                    buffer.Map(0, None, Some(&mut data)).expect("a mapping");
                }
                (buffer, data.cast())
            }
        }

        /// Sends a list of the direct queue made by `record`, with a
        /// signal behind it, and does not wait: the value signalled.
        fn fly(&mut self, record: impl FnOnce(&ID3D12GraphicsCommandList)) -> u64 {
            // SAFETY: plain creation; an open list of ours; our queue and
            // fence.
            unsafe {
                let allocator: ID3D12CommandAllocator =
                    self.device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT).expect("an allocator");
                let list: ID3D12GraphicsCommandList =
                    self.device.CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None).expect("a list");
                record(&list);
                list.Close().expect("the list closes");
                self.direct.ExecuteCommandLists(&[Some(list.cast().expect("a command list"))]);
                self.submitted += 1;
                self.direct.Signal(&self.fence, self.submitted).expect("a signal");
                self.flown.push((allocator, list));
            }
            self.submitted
        }

        /// A direct list that stores `value` at byte `at` of `memory`
        /// after `spin` steps.
        fn store(&mut self, memory: &ID3D12Resource, at: u32, value: u32, spin: u32) -> u64 {
            let (root, pso) = (self.root.clone(), self.pso.clone());
            // SAFETY: a live buffer with unordered access; the shader's
            // own root signature.
            self.fly(|list| unsafe {
                list.SetComputeRootSignature(&root);
                list.SetPipelineState(&pso);
                let constants = [at, value, spin];
                list.SetComputeRoot32BitConstants(0, 3, constants.as_ptr().cast(), 0);
                list.SetComputeRootUnorderedAccessView(1, memory.GetGPUVirtualAddress());
                list.Dispatch(1, 1, 1);
            })
        }

        /// Waits for everything sent to the direct queue and the lanes.
        fn land(&mut self) {
            // SAFETY: our fence and event.
            unsafe {
                self.fence.SetEventOnCompletion(self.submitted, self.event).expect("a wait");
                WaitForSingleObject(self.event, INFINITE);
            }
            for lane in [&self.second.down, &self.second.up] {
                let last = lock(&lane.state).sent;
                lane.wait(last, self.event).expect("a wait");
            }
            assert!(
                // SAFETY: a plain query.
                unsafe { self.device.GetDeviceRemovedReason() }.is_ok(),
                "the software adapter lost the device"
            );
            self.flown.clear();
        }
    }

    /// The dwords of a mapped buffer.
    fn dwords(data: *mut u8, at: usize, n: usize) -> Vec<u32> {
        // SAFETY: the caller's mapping holds them.
        (0..n).map(|i| unsafe { data.cast::<u32>().add(at + i).read_volatile() }).collect()
    }

    /// What shaders wrote, read back through the down lane, is what the
    /// direct queue reads back: the same bytes of a buffer in the card's
    /// memory come down both ways, and the lane's copy is behind the fence
    /// of the submission that wrote them without any wait of ours between.
    #[test]
    fn warp_a_fetch_through_the_copy_queue_brings_what_the_direct_queue_brings() {
        let Some(mut warp) = Warp::new(1) else {
            return;
        };
        const DWORDS: usize = 4096;
        let bytes = (DWORDS * 4) as u64;
        let (card, _) = warp.buffer(bytes, false);
        let (down_direct, direct_data) = warp.buffer(bytes, true);
        let (down_lane, lane_data) = warp.buffer(bytes, true);
        for round in 1..=3u32 {
            // "Shader writes": a few dwords, each by a dispatch.
            let mut behind = 0;
            for i in 0..24u32 {
                behind = warp.store(&card, (i * 163 % DWORDS as u32) * 4, round * 1000 + i, 20_000);
            }
            // Through the lane, behind the writes' fence; no wait of ours.
            let second = warp.second;
            let list = second.down.list().expect("a copy list");
            // SAFETY: an open copy list; live buffers.
            unsafe { list.1.CopyBufferRegion(&down_lane, 0, &card, 0, bytes) };
            let job = second.down.send(list, &[(&warp.fence, behind)], bytes).expect("the fetch is sent");
            // And through the direct queue, as before.
            // SAFETY: an open list; live buffers.
            warp.fly(|list| unsafe { list.CopyBufferRegion(&down_direct, 0, &card, 0, bytes) });
            second.down.wait(job, warp.event).expect("the fetch is waited for");
            assert!(second.down.done(job));
            warp.land();
            let (direct, lane) = (dwords(direct_data, 0, DWORDS), dwords(lane_data, 0, DWORDS));
            assert_eq!(direct, lane, "round {round}: the two ways brought different bytes");
            for i in 0..24u32 {
                assert_eq!(lane[(i * 163 % DWORDS as u32) as usize], round * 1000 + i, "round {round}, write {i}");
            }
        }
        assert_eq!(warp.second.down.stats.jobs.load(Ordering::Relaxed), 3);
    }

    /// Ordering under the fences, nothing waited for on the CPU: write k
    /// goes to the direct queue with a signal behind it, and fetch k, sent
    /// at once to the lane behind that signal, must hold writes 1..=k
    /// (later writes may or may not be in it: they are newer, never
    /// older). Sixteen of each are in the air together.
    #[test]
    fn warp_a_fetch_behind_a_fence_holds_everything_sent_before_that_fence() {
        let Some(mut warp) = Warp::new(1) else {
            return;
        };
        const N: usize = 16;
        let bytes = (N * 4) as u64;
        let (card, _) = warp.buffer(64 << 10, false);
        let (down, data) = warp.buffer(bytes * N as u64, true);
        let second = warp.second;
        for k in 0..N {
            let behind = warp.store(&card, (k * 4) as u32, 0xA000 + k as u32, 60_000);
            let list = second.down.list().expect("a copy list");
            // SAFETY: an open copy list; live buffers; the slot is inside.
            unsafe { list.1.CopyBufferRegion(&down, k as u64 * bytes, &card, 0, bytes) };
            second.down.send(list, &[(&warp.fence, behind)], bytes).expect("the fetch is sent");
        }
        warp.land();
        for k in 0..N {
            let slot = dwords(data, k * N, N);
            for (i, &got) in slot.iter().enumerate().take(k + 1) {
                assert_eq!(got, 0xA000 + i as u32, "fetch {k} ran before write {i}, which was sent before its fence");
            }
            for (i, &got) in slot.iter().enumerate().skip(k + 1) {
                assert!(got == 0 || got == 0xA000 + i as u32, "fetch {k} holds {got:#x} for write {i}");
            }
        }
    }

    /// Title bytes sent up through the up lane are there when the direct
    /// queue goes on: each of sixteen sends is followed, with no wait of
    /// ours, by a direct list that reads the buffer back, and each read
    /// holds its own send and all before it. Each send is made late on
    /// purpose (it stands behind a slow list of the direct queue), so the
    /// read would come first if the direct queue did not wait. The
    /// numbered sends are signalled in order, and the clock saw the holds.
    #[test]
    fn warp_the_direct_queue_goes_on_only_when_what_was_sent_up_is_there() {
        let Some(mut warp) = Warp::new(3) else {
            return;
        };
        const N: usize = 16;
        const PART: u64 = 64 << 10;
        let (staging, staged) = warp.buffer(PART * N as u64, true);
        // A buffer in the card's memory per send: the lane writes one that
        // nothing in the air names yet (as it writes landing buffers).
        let cards: Vec<ID3D12Resource> = (0..N).map(|_| warp.buffer(PART, false).0).collect();
        let (other, _) = warp.buffer(64 << 10, false);
        // What each read brings: the first dword of every part.
        let (down, data) = warp.buffer((N * N * 4) as u64, true);
        let second = warp.second;
        for k in 0..N {
            // The title's bytes of part k (the CPU's side of the copy).
            for i in 0..(PART as usize / 4) {
                // SAFETY: inside the mapping; nothing on the card reads
                // part k before the send below.
                unsafe { staged.cast::<u32>().add(k * (PART as usize / 4) + i).write_volatile(0xB000 + k as u32) };
            }
            let slow = warp.store(&other, 0, k as u32, 80_000);
            let list = second.up.list().expect("a copy list");
            // SAFETY: an open copy list; live buffers.
            unsafe { list.1.CopyBufferRegion(&cards[k], 0, &staging, k as u64 * PART, PART) };
            let job = second.up.send(list, &[(&warp.fence, slow)], PART).expect("the bytes are sent up");
            second.hold(&warp.direct, job).expect("the direct queue waits");
            second.sent_up(&warp.direct, k as u64 + 1);
            // SAFETY: an open list; live buffers; the slots are inside.
            warp.fly(|list| unsafe {
                for (part, card) in cards.iter().enumerate().take(k + 1) {
                    list.CopyBufferRegion(&down, ((k * N + part) * 4) as u64, card, 0, 4);
                }
            });
        }
        warp.land();
        assert_eq!(second.order_done(), N as u64);
        for k in 0..N {
            let read = dwords(data, k * N, N);
            for (part, &got) in read.iter().enumerate().take(k + 1) {
                assert_eq!(got, 0xB000 + part as u32, "the direct queue's read {k} ran before send {part} was on the card");
            }
        }
        assert_eq!(second.held[0].load(Ordering::Relaxed), N as u64);
        assert_eq!(second.up.stats.bytes.load(Ordering::Relaxed), PART * N as u64);
    }

    /// The way title bytes go up with bit 2, as `Host::card_before` does
    /// it: a job of the up lane brings them into a landing buffer while a
    /// slow list of the direct queue is still running, the direct queue
    /// waits for the job and then copies from the landing buffer into the
    /// copy. A shader's store to the same bytes, sent before, must lose to
    /// them (the copy changes in the direct queue's order, as on one
    /// queue), and what a fetch through the down lane brings is what the
    /// direct queue reads. Twelve rounds in the air together, each with a
    /// landing buffer of its own until the card has passed its use.
    #[test]
    fn warp_title_bytes_reach_the_copy_through_a_landing_buffer_in_the_direct_queues_order() {
        let Some(mut warp) = Warp::new(3) else {
            return;
        };
        const N: usize = 12;
        let (staging, staged) = warp.buffer(64 << 10, true);
        let (card, _) = warp.buffer(64 << 10, false);
        let (down, data) = warp.buffer((N * 4) as u64, true);
        let (down_lane, lane_data) = warp.buffer((N * 4) as u64, true);
        let second = warp.second;
        let mut landings = Vec::new();
        for k in 0..N {
            let at = (k * 256) as u64;
            // SAFETY: inside the mapping; read by the card only through
            // the job below.
            unsafe { staged.add(at as usize).cast::<u32>().write_volatile(0xC000 + k as u32) };
            // A shader's (slow) store to the same dword, sent first.
            warp.store(&card, at as u32, 0xDEAD_0000 + k as u32, 80_000);
            // SAFETY: a plain query.
            let completed = unsafe { warp.fence.GetCompletedValue() };
            let landing = second.landing(4, completed, warp.submitted + 1).expect("a landing buffer");
            let list = second.up.list().expect("a copy list");
            // SAFETY: an open copy list; live buffers.
            unsafe { list.1.CopyBufferRegion(&landing, 0, &staging, at, 4) };
            let job = second.up.send(list, &[], 4).expect("the bytes are sent up");
            second.hold(&warp.direct, job).expect("the direct queue waits");
            // SAFETY: an open list; live buffers.
            let read = warp.fly(|list| unsafe {
                list.CopyBufferRegion(&card, at, &landing, 0, 4);
                list.CopyBufferRegion(&down, (k * 4) as u64, &card, at, 4);
            });
            let list = second.down.list().expect("a copy list");
            // SAFETY: an open copy list; live buffers.
            unsafe { list.1.CopyBufferRegion(&down_lane, (k * 4) as u64, &card, at, 4) };
            second.down.send(list, &[(&warp.fence, read)], 4).expect("the fetch is sent");
            landings.push(landing.as_raw() as usize);
        }
        warp.land();
        let want: Vec<u32> = (0..N).map(|k| 0xC000 + k as u32).collect();
        assert_eq!(dwords(data, 0, N), want, "the title's bytes did not reach the copy behind the shader's store");
        assert_eq!(dwords(lane_data, 0, N), want, "the copy queue's fetch differs from the direct queue's");
        // No buffer was given out while its last use was in the air: one
        // given to two rounds was free in between.
        landings.sort_unstable();
        landings.dedup();
        assert!(!landings.is_empty() && landings.len() <= N);
        // Everything has run: the next ask takes one of them, makes none.
        let alive = second.landing[0].load(Ordering::Relaxed);
        // SAFETY: a plain query.
        let completed = unsafe { warp.fence.GetCompletedValue() };
        let again = second.landing(4, completed, warp.submitted + 1).expect("a landing buffer");
        assert!(landings.contains(&(again.as_raw() as usize)));
        assert_eq!(second.landing[0].load(Ordering::Relaxed), alive);
    }

    /// Lists of a lane come back for the next job once the card has run
    /// the job they were in, and not before.
    #[test]
    fn warp_a_lanes_lists_are_used_again_only_after_their_job() {
        let Some(mut warp) = Warp::new(1) else {
            return;
        };
        let (card, _) = warp.buffer(64 << 10, false);
        let (down, _) = warp.buffer(64 << 10, true);
        let second = warp.second;
        // A job that cannot run yet: it waits for a fence value the direct
        // queue reaches only after a slow store.
        let behind = warp.store(&card, 0, 1, 400_000);
        let first = second.down.list().expect("a copy list");
        let first_list = first.1.as_raw();
        // SAFETY: an open copy list; live buffers.
        unsafe { first.1.CopyBufferRegion(&down, 0, &card, 0, 64) };
        let job = second.down.send(first, &[(&warp.fence, behind)], 64).expect("sent");
        if !second.down.done(job) {
            // Still in the air: the next list is another one.
            let next = second.down.list().expect("a copy list");
            assert_ne!(next.1.as_raw(), first_list, "a list was reset while its job had not run");
            second.down.send(next, &[], 0).expect("sent");
        }
        warp.land();
        // Everything has run: a list of before comes back.
        let again = second.down.list().expect("a copy list");
        let spare_left = lock(&second.down.state).spare.len();
        assert!(spare_left <= 1, "lists that have run are taken before new ones are made");
        second.down.send(again, &[], 0).expect("sent");
        warp.land();
    }
}
