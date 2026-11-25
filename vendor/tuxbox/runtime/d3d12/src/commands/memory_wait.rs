//! Translate waits on host-visible Series fence words at a native list boundary.
//! The game can write markers and sample timestamps before waiting. Keep these
//! prefix operations ordered without disturbing bindings on the main command list.
use super::*;
use std::sync::atomic::{AtomicI32, AtomicU64};
use std::sync::mpsc;
use std::time::Duration;
use windows::Win32::Foundation::HANDLE;
use windows::core::Result;

pub(super) enum Packet {
    Marker(D3D12_WRITEBUFFERIMMEDIATE_PARAMETER),
    Timestamp(ID3D12QueryHeap, u32, ID3D12Resource, u64),
}

impl Packet {
    fn emit(&self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        unsafe {
            match self {
                Self::Marker(write) => {
                    list.cast::<ID3D12GraphicsCommandList2>()?
                        .WriteBufferImmediate(1, write, None);
                }
                Self::Timestamp(heap, index, dest, offset) => {
                    list.EndQuery(heap, D3D12_QUERY_TYPE_TIMESTAMP, *index);
                    list.ResolveQueryData(
                        heap,
                        D3D12_QUERY_TYPE_TIMESTAMP,
                        *index,
                        1,
                        dest,
                        *offset,
                    );
                }
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Prefix {
    pending: Vec<Packet>,
    in_body: bool,
    pub(super) segments: Vec<Arc<Segment>>,
}

impl Prefix {
    pub(super) fn push(&mut self, list: &ID3D12GraphicsCommandList, packet: Packet) -> Result<()> {
        if self.in_body {
            return packet.emit(list);
        }
        self.pending.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
        self.pending.push(packet);
        Ok(())
    }

    pub(super) fn finish(&mut self, list: &ID3D12GraphicsCommandList) -> Result<()> {
        if self.in_body {
            return Ok(());
        }
        self.in_body = true;
        for packet in self.pending.drain(..) {
            packet.emit(list)?;
        }
        Ok(())
    }
}

struct Word {
    resource: ID3D12Resource,
    mapped: usize,
    /// 64-bit word (slot 74) or 32-bit word (slot 73, Wait32X).
    wide: bool,
}

impl Drop for Word {
    fn drop(&mut self) {
        unsafe { self.resource.Unmap(0, Some(&D3D12_RANGE::default())) };
    }
}

impl Word {
    fn map(list: &List, address: u64, wide: bool) -> Result<Self> {
        let bytes = word_bytes(wide);
        if address == 0 || !address.is_multiple_of(bytes as u64) {
            return Err(E_INVALIDARG.into());
        }
        let (buffer, gpu) =
            unsafe { device::command_buffer_address(parent(&list.parent), address, bytes as u64)? };
        let resource = buffer.ok_or(E_NOTIMPL)?;
        let offset = gpu
            .checked_sub(unsafe { resource.GetGPUVirtualAddress() })
            .ok_or(E_INVALIDARG)? as usize;
        let range = D3D12_RANGE {
            Begin: offset,
            End: offset.checked_add(bytes).ok_or(E_INVALIDARG)?,
        };
        let mut base = ptr::null_mut();
        unsafe { resource.Map(0, Some(&range), Some(&mut base))? };
        if base.is_null() {
            unsafe { resource.Unmap(0, Some(&D3D12_RANGE::default())) };
            return Err(E_FAIL.into());
        }
        Ok(Self {
            resource,
            mapped: base.addr() + offset,
            wide,
        })
    }

    fn reached(&self, value: u64, comparison: u32) -> bool {
        // An unreadable word counts as met: the title released the memory
        // under the wait, so nothing will write it (five-star c933e88).
        self.load().is_none_or(|now| compare(now, value, comparison))
    }

    /// The word now, or None if its page is gone. The COM resource and its CPU
    /// mapping stay alive until all queued waits release their segment, but
    /// the CPU address of an imported heap is the title's own memory, which
    /// the title can unmap (five-star-recomp c933e88, GPL-3.0: their watcher
    /// faulted on a page the story load unmapped; the same class as test133's
    /// released-page copy, 09a97d4). The read goes through ReadProcessMemory,
    /// which fails instead of faulting; an aligned 4- or 8-byte copy is one
    /// load. A reserved page of a lazy title view (xmem lazy_commit.rs) was
    /// never written and reads as zero. TUXBOX_WAIT_SAFE_READ=0 loads directly.
    fn load(&self) -> Option<u64> {
        if crate::env_toggle!("TUXBOX_WAIT_SAFE_READ") == Some("0") {
            return Some(if self.wide {
                unsafe { AtomicU64::from_ptr(self.mapped as *mut u64).load(Ordering::Acquire) }
            } else {
                u64::from(unsafe { std::sync::atomic::AtomicU32::from_ptr(self.mapped as *mut u32).load(Ordering::Acquire) })
            });
        }
        use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        use windows::Win32::System::Memory::{MEM_MAPPED, MEM_RESERVE, MEMORY_BASIC_INFORMATION, VirtualQuery};
        use windows::Win32::System::Threading::GetCurrentProcess;
        let mut word = 0u64;
        let bytes = word_bytes(self.wide);
        let mut read = 0;
        let ok = unsafe { ReadProcessMemory(GetCurrentProcess(), self.mapped as *const _, (&raw mut word).cast(), bytes, Some(&mut read)) };
        if ok.is_ok() && read == bytes {
            std::sync::atomic::fence(Ordering::Acquire);
            return Some(word);
        }
        let mut region = MEMORY_BASIC_INFORMATION::default();
        let found = unsafe { VirtualQuery(Some(self.mapped as *const _), &mut region, size_of_val(&region)) } != 0;
        if found && region.State == MEM_RESERVE && region.Type == MEM_MAPPED {
            return Some(0);
        }
        static GONE: AtomicU64 = AtomicU64::new(0);
        let n = GONE.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::diagnostics::log_line(format_args!(
                "GPU memory wait word at CPU {:#x} is unreadable (state {:#x}): counted as met ({n} so far; TUXBOX_WAIT_SAFE_READ=0 loads directly)",
                self.mapped, if found { region.State.0 } else { 0 }));
        }
        None
    }
}

#[cfg(test)]
#[test]
fn wait32_words_are_dword_sized_and_aligned() {
    assert_eq!(word_bytes(false), 4);
    assert_eq!(word_bytes(true), 8);
    assert!(0x1004u64.is_multiple_of(word_bytes(false) as u64));
    assert!(!0x1004u64.is_multiple_of(word_bytes(true) as u64));
}

/// D3D12_COMPARISON_FUNC of `now` against `target` (the console's memory
/// waits use the same values); unknown ones wait as GREATER_EQUAL (7).
fn compare(now: u64, target: u64, comparison: u32) -> bool {
    match comparison {
        1 => false,
        2 => now < target,
        3 => now == target,
        4 => now <= target,
        5 => now > target,
        6 => now != target,
        8 => true,
        _ => now >= target,
    }
}

pub(super) struct Segment {
    prefix: Option<(ID3D12CommandList, ID3D12CommandAllocator)>,
    word: Word,
    target: u64,
    comparison: u32,
    address: u64,
}

/// Guest addresses GPU memory waits were recorded on (up to 4096, then the
/// set restarts). A left-out list's CPU value write only raises these words.
static WAIT_WORDS: std::sync::Mutex<Option<std::collections::HashSet<u64>>> = std::sync::Mutex::new(None);

fn note_wait_word(address: u64) {
    let mut words = WAIT_WORDS.lock().unwrap_or_else(|e| e.into_inner());
    let words = words.get_or_insert_with(Default::default);
    if words.len() >= 4096 {
        words.clear();
    }
    words.insert(address);
}

/// Whether a GPU memory wait was recorded on `address`.
pub(super) fn is_wait_word(address: u64) -> bool {
    WAIT_WORDS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|w| w.contains(&address))
}

/// Guest addresses of waits that stalled 2 s (up to 16). Writes recorded or
/// submitted later to the same 4 KiB page are logged, to tell a missing write
/// from one queued behind the wait (rebox d69d0da).
static STALLED: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
static STALLED_COUNT: AtomicU64 = AtomicU64::new(0);

/// Whether `address` lies on the page of a wait that stalled.
pub(super) fn on_stalled_page(address: u64) -> bool {
    let n = STALLED_COUNT.load(Ordering::Relaxed).min(16) as usize;
    n != 0 && STALLED[..n].iter().any(|a| a.load(Ordering::Relaxed) & !0xfff == address & !0xfff)
}

/// A recorded write to a stalled page: logged now, and kept on the list so
/// its submission is logged too.
pub(super) fn note_write(list: &List, state: &mut super::Recording, what: &str, address: u64, value: u64) {
    writers::recorded(address, value, list.native.as_raw() as usize);
    if !on_stalled_page(address) { return; }
    static LOGGED: AtomicU64 = AtomicU64::new(0);
    if LOGGED.fetch_add(1, Ordering::Relaxed) < 32 {
        crate::diagnostics::log_line(format_args!(
            "{what} {address:#x} = {value:#x} recorded in list {list:p} (thread {}, guest RVA {:#x}), on the page of a stalled GPU memory wait",
            unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, crate::survey::title_rva().unwrap_or(0)));
    }
    if state.stalled_writes.len() < 8 { state.stalled_writes.push(address); }
}

fn word_bytes(wide: bool) -> usize {
    if wide { 8 } else { 4 }
}

/// List slot 73 (Wait32X): the 32-bit form of slot 74, the same comparisons
/// on a dword (Research port-everything item 2). TUXBOX_MEMORY_WAITS=0 skips
/// it like slot 74; TUXBOX_WAIT32=0 leaves it inert as when unmapped.
pub(super) unsafe extern "system" fn wait_value32(
    list: &List,
    address: u64,
    comparison: u32,
    target: u32,
    flags: u32,
) {
    if crate::env_toggle!("TUXBOX_WAIT32") == Some("0") {
        return;
    }
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "32-bit GPU memory wait (list slot 73) first at {address:#x}, comparison {comparison}, target {target:#x} (TUXBOX_WAIT32=0 leaves it inert)"));
    unsafe { wait_value(list, address, comparison, u64::from(target), flags, false) }
}

pub(super) unsafe extern "system" fn wait_value64(
    list: &List,
    address: u64,
    comparison: u32,
    target: u64,
    flags: u32,
) {
    unsafe { wait_value(list, address, comparison, target, flags, true) }
}

unsafe fn wait_value(
    list: &List,
    address: u64,
    comparison: u32,
    target: u64,
    flags: u32,
    wide: bool,
) {
    // TUXBOX_MEMORY_WAITS=0 (rebox d35c75f, re-derived): a bisect switch for
    // stalls; the GPU memory wait is not recorded.
    if crate::env_toggle!("TUXBOX_MEMORY_WAITS") == Some("0") {
        static SKIPPED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&SKIPPED, || format!(
            "GPU memory waits skipped (TUXBOX_MEMORY_WAITS=0); first at {address:#x}"));
        return;
    }
    note_wait_word(address);
    let mut state = recording(list);
    if !state.ensure_open(&list.native) {
        return;
    }
    let result = (|| -> Result<()> {
        // The fifth argument is stack junk on some callers (rebox), so only
        // the comparison selects the wait.
        let _ = flags;
        // Every comparison is honoured (only 7, GREATER_EQUAL, was; others
        // refused the list); NEVER waits as ALWAYS rather than forever.
        let comparison = match comparison { 1 => 8, 2..=8 => comparison, _ => {
            static ODD: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&ODD, || format!("GPU memory wait comparison {comparison} waits as >="));
            7
        } };
        // test62: the word's heap import failed (0x8007000e, commit
        // exhausted) and the error refused the whole list, so the writes it
        // carried never ran and other queues' waits stalled 2 s each. Drop
        // only this wait (ordering with the signalling queue loosens);
        // TUXBOX_WAIT_MAP_STRICT=1 refuses the list again.
        let word = match Word::map(list, address, wide) {
            Ok(word) => word,
            Err(e) if address != 0 && crate::env_toggle!("TUXBOX_WAIT_MAP_STRICT") != Some("1") => {
                static DROPPED: AtomicU64 = AtomicU64::new(0);
                let n = DROPPED.fetch_add(1, Ordering::Relaxed);
                if n < 4 || (n + 1).is_power_of_two() {
                    crate::diagnostics::log_line(format_args!(
                        "GPU memory wait {address:#x} dropped: its word could not be mapped ({:?}; {} so far)", e.code(), n + 1));
                }
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        if comparison == 8 || (comparison == 7 && target == 0) {
            return Ok(());
        }
        if state.prefix.in_body && super::split::on() {
            // Split the list here: the work before the wait runs ahead of it (split).
            let segment = Arc::new(Segment { prefix: None, word, target, comparison, address });
            if unsafe { super::split::split(list, &mut state, segment.clone())? } {
                capture_wait(list, "split the list", address, target);
                return Ok(());
            }
            if super::split::hoist(&mut state, segment) {
                hoisted(list, address, target);
                return Ok(());
            }
            capture_wait(list, "skipped: the list cannot split", address, target);
            skip_barrier(list);
            static UNSPLIT: AtomicU64 = AtomicU64::new(0);
            let n = UNSPLIT.fetch_add(1, Ordering::Relaxed);
            if n < 4 || (n + 1).is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "GPU memory wait {address:#x} >= {target:#x} after rendering commands skipped: the list cannot split (open queries, split barriers, predication, a bundle or 32 splits; {} so far)",
                    n + 1));
            }
            return Ok(());
        }
        if state.prefix.in_body {
            if super::split::hoist(&mut state, Arc::new(Segment { prefix: None, word, target, comparison, address })) {
                hoisted(list, address, target);
                return Ok(());
            }
            // Waits are honoured only at a native list boundary. Failing here
            // rejected the whole list (black frames); dropping the wait keeps
            // the rendering and only loosens ordering with the signalling queue.
            capture_wait(list, "skipped: after rendering commands", address, target);
            skip_barrier(list);
            static SKIPPED: AtomicU64 = AtomicU64::new(0);
            let n = SKIPPED.fetch_add(1, Ordering::Relaxed);
            if n < 4 || (n + 1).is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "GPU memory wait {address:#x} >= {target:#x} after rendering commands skipped ({} so far)",
                    n + 1
                ));
            }
            return Ok(());
        }
        capture_wait(list, "at the list boundary", address, target);
        state
            .prefix
            .segments
            .try_reserve(1)
            .map_err(|_| E_OUTOFMEMORY)?;
        let prefix = if state.prefix.pending.is_empty() {
            None
        } else {
            let mut device: Option<ID3D12Device> = None;
            unsafe {
                list.native.GetDevice(&mut device)?;
            }
            let device = device.ok_or(E_FAIL)?;
            let kind = unsafe { list.native.GetType() };
            let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(kind)? };
            let native: ID3D12GraphicsCommandList =
                unsafe { device.CreateCommandList(0, kind, &allocator, None)? };
            crate::dred::name(&native, "tuxbox memory wait prefix");
            for packet in state.prefix.pending.drain(..) {
                packet.emit(&native)?;
            }
            unsafe {
                native.Close()?;
            }
            Some((native.into(), allocator))
        };
        state.prefix.segments.push(Arc::new(Segment {
            prefix,
            word,
            target,
            comparison,
            address,
        }));
        Ok(())
    })();
    if let Err(error) = result {
        crate::diagnostics::recording_error(format_args!(
            "{}-bit GPU memory wait {address:#x} >= {target:#x} ({:?})",
            word_bytes(wide) * 8,
            error.code()
        ));
        { state.error = error.code(); state.error_at = (file!(), line!()); }
    }
}

/// Who last wrote a GPU wait word (rebox 956a736): the host list that
/// recorded a marker write or fill at its guest address, and the queue and time that list was last
/// submitted. A timed-out wait logs them (TUXBOX_WAIT_WRITERS=0 skips the
/// bookkeeping).
mod writers {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    use std::time::Instant;

    fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| !std::env::var("TUXBOX_WAIT_WRITERS").is_ok_and(|v| v.trim() == "0"))
    }

    #[derive(Default)]
    struct Table {
        /// 8-byte word -> (host list, value).
        writes: HashMap<u64, (usize, u64, Instant)>,
        /// host list -> (queue, when).
        submits: HashMap<usize, (usize, Instant)>,
        /// host list -> the queue's progress fence value after its last submission.
        done_at: HashMap<usize, (usize, u64)>,
        /// queue -> (progress fence, last value signalled).
        progress: HashMap<usize, (windows::Win32::Graphics::Direct3D12::ID3D12Fence, u64)>,
        /// host list -> its current recording's word writes (first 16).
        list_writes: HashMap<usize, Vec<(u64, u64)>>,
    }

    /// After a wait times out, the next TRACE_EVENTS waits and list
    /// submissions are logged in queue order with their writes (rebox
    /// 76f7a13, re-derived; TUXBOX_WAIT_ORDER=0 off), so a cross-queue cycle
    /// reads straight from the log.
    const TRACE_EVENTS: u32 = 24;
    static TRACE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn order_on() -> bool {
        on() && crate::env_toggle!("TUXBOX_WAIT_ORDER") != Some("0")
    }

    pub(super) fn arm() {
        if order_on() {
            TRACE.store(TRACE_EVENTS, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Take one trace event if armed.
    fn traced() -> bool {
        TRACE.try_update(std::sync::atomic::Ordering::Relaxed, std::sync::atomic::Ordering::Relaxed,
            |n| n.checked_sub(1)).is_ok()
    }

    pub(super) fn reset(list: usize) {
        if order_on() {
            table().list_writes.remove(&list);
        }
    }

    pub(super) fn wait_queued(queue: usize, address: u64, comparison: u32, target: u64) {
        if order_on() && traced() {
            crate::diagnostics::log_line(format_args!(
                "wait order: queue {queue:#x} waits {address:#018x} (comparison {comparison}) for {target:#x}"));
        }
    }

    fn table() -> std::sync::MutexGuard<'static, Table> {
        static TABLE: OnceLock<Mutex<Table>> = OnceLock::new();
        TABLE.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner())
    }

    pub(super) fn recorded(address: u64, value: u64, list: usize) {
        if !on() {
            return;
        }
        let mut t = table();
        if t.writes.len() >= 1 << 14 {
            t.writes.clear();
        }
        t.writes.insert(address & !7, (list, value, Instant::now()));
        if order_on() {
            if t.list_writes.len() >= 1 << 12 {
                t.list_writes.clear();
            }
            let writes = t.list_writes.entry(list).or_default();
            if writes.len() < 16 {
                writes.push((address, value));
            }
        }
    }

    pub(super) fn submitted(queue: usize, lists: &[*mut core::ffi::c_void]) {
        if !on() {
            return;
        }
        let now = Instant::now();
        let mut t = table();
        if t.submits.len() >= 1 << 14 {
            t.submits.clear();
        }
        for &list in lists {
            t.submits.insert(list as usize, (queue, now));
        }
        if order_on() && TRACE.load(std::sync::atomic::Ordering::Relaxed) != 0 {
            for &list in lists {
                let Some(writes) = t.list_writes.get(&(list as usize)).filter(|w| !w.is_empty()) else { continue };
                if !traced() {
                    break;
                }
                let text: Vec<String> = writes.iter().map(|(a, v)| format!("{a:#018x}={v:#x}")).collect();
                crate::diagnostics::log_line(format_args!(
                    "wait order: queue {queue:#x} runs list {:#x} writing {}", list as usize, text.join(", ")));
            }
        }
    }

    /// test74: 70 waits stalled 2 s although the list writing their value was
    /// submitted milliseconds after the wait began. After a submission the
    /// queue signals a progress fence; a wait whose recorded writer (a list
    /// writing a value that satisfies it) has passed that fence is released
    /// then, logged with the value the word reads. TUXBOX_WAIT_WRITER_RELEASE=0
    /// waits for the 2 s watchdog as before.
    pub(super) fn writer_release_on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| on() && crate::env_toggle!("TUXBOX_WAIT_WRITER_RELEASE") != Some("0"))
    }

    /// Signals the queue's progress fence after a submission of `lists`.
    pub(super) fn progress(queue: &windows::Win32::Graphics::Direct3D12::ID3D12CommandQueue, lists: &[*mut core::ffi::c_void]) {
        use windows::Win32::Graphics::Direct3D12::*;
        use windows::core::Interface;
        if !writer_release_on() || lists.is_empty() {
            return;
        }
        let key = queue.as_raw() as usize;
        let mut t = table();
        if !t.progress.contains_key(&key) {
            let mut device: Option<ID3D12Device> = None;
            let Some(device) = unsafe { queue.GetDevice(&mut device) }.ok().and(device) else { return };
            let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else { return };
            t.progress.insert(key, (fence, 0));
        }
        let Some((fence, value)) = t.progress.get_mut(&key) else { return };
        *value += 1;
        let value = *value;
        if unsafe { queue.Signal(&*fence, value) }.is_err() {
            return;
        }
        if t.done_at.len() >= 1 << 14 {
            t.done_at.clear();
        }
        for &list in lists {
            t.done_at.insert(list as usize, (key, value));
        }
    }

    /// The value the recorded writer of `address` wrote, once that writer's
    /// last submission has completed on the GPU.
    pub(super) fn writer_done(address: u64, since: Instant, target: u64, comparison: u32) -> Option<u64> {
        let t = table();
        let &(list, value, recorded) = t.writes.get(&(address & !7))?;
        if !super::compare(value, target, comparison) {
            return None;
        }
        // Submitted since it recorded this write, not an earlier recording.
        if t.submits.get(&list).is_none_or(|&(_, at)| at < recorded) {
            return None;
        }
        // The value check keeps an earlier frame's write (a lower value)
        // from releasing this wait; `since` only bounds the log wording.
        let _ = since;
        let &(queue, done) = t.done_at.get(&list)?;
        let (fence, _) = t.progress.get(&queue)?;
        (unsafe { fence.GetCompletedValue() } >= done).then_some(value)
    }

    /// The queue the recorded writer of `address` was last submitted on.
    pub(super) fn writer_queue(address: u64) -> Option<usize> {
        let t = table();
        let &(list, ..) = t.writes.get(&(address & !7))?;
        t.submits.get(&list).map(|&(queue, _)| queue)
    }

    pub(super) fn describe(address: u64, waiting_queue: usize, since: Instant) -> String {
        if !on() {
            return String::new();
        }
        let t = table();
        let Some(&(list, value, _)) = t.writes.get(&(address & !7)) else {
            return "; no recorded list writes it".into();
        };
        match t.submits.get(&list) {
            Some(&(queue, at)) => format!(
                "; list {list:#x} writing {value:#x} was last submitted {}{queue:#x} {} ms {} the wait",
                if queue == waiting_queue { "on the same queue " } else { "on queue " },
                if at <= since { (since - at).as_millis() } else { (at - since).as_millis() },
                if at <= since { "before" } else { "after" },
            ),
            None => format!("; list {list:#x} writing {value:#x} was recorded but never submitted"),
        }
    }
}

/// five-star-recomp efcc782 (GPL-3.0): a word the watchdog had to release
/// once is waited on briefly afterwards (TUXBOX_WAIT_DEAD_MS, 100; 0 waits the
/// full release each time), until a wait on it completes on its own. test111
/// lost 2 s on each of 7 frame-cell waits in a row.
static DEAD: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());

fn wait_stacks_count() -> u64 {
    crate::env_toggle!("TUXBOX_WAIT_STACKS").and_then(|v| v.trim().parse().ok()).unwrap_or(3)
}

fn wait_stacks_after() -> Duration {
    Duration::from_millis(crate::env_toggle!("TUXBOX_WAIT_STACKS_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(1500))
}

fn dead_ms() -> u64 {
    static MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *MS.get_or_init(|| crate::env_toggle!("TUXBOX_WAIT_DEAD_MS").and_then(|v| v.trim().parse().ok()).unwrap_or(100))
}

fn dead(address: u64) -> bool {
    DEAD.lock().unwrap_or_else(|e| e.into_inner()).contains(&address)
}

fn mark_dead(address: u64, on: bool) {
    let mut d = DEAD.lock().unwrap_or_else(|e| e.into_inner());
    let at = d.iter().position(|&a| a == address);
    match (on, at) {
        (true, None) => { if d.len() >= 4096 { d.remove(0); } d.push(address); }
        (false, Some(i)) => { d.swap_remove(i); }
        _ => {}
    }
}

struct Job {
    queue: usize,
    fence: ID3D12Fence,
    segment: Arc<Segment>,
    device: ID3D12Device,
}

pub(super) struct Worker(mpsc::Sender<Job>);

/// GPU memory waits the worker is holding right now (word not yet reached).
/// The present throttle releases early while one is held (five-star bd100ba).
static HELD: AtomicU64 = AtomicU64::new(0);

/// Bumped at each present: a held wait whose word no queued list writes is
/// released then (rebox a1ebac2 `value_wait_force`, GPL-2.0, credit to its
/// authors): no queue will ever write it, so it would only wait out the
/// watchdog with the present behind it. TUXBOX_MEMORY_WAIT_FORCE=0 waits.
/// TUXBOX_PRESENT_RELEASE_ALL=1 releases every held wait at a present, as
/// five-star b828157 `MemoryWaits.ReleaseAll` does (GPL-3.0, marquezini).
static PRESENTS: AtomicU64 = AtomicU64::new(0);

pub(super) fn presented() {
    PRESENTS.fetch_add(1, Ordering::AcqRel);
}

/// Bumped by `release_all`: every wait held when it moves gives up at once,
/// the way a TUXBOX_PRESENT_RELEASE_ALL=1 present does (five-star bd100ba).
static RELEASE_ALL: AtomicU64 = AtomicU64::new(0);

/// Releases every held GPU memory wait now and returns how many were held.
pub(super) fn release_all() -> usize {
    let held = HELD.load(Ordering::Relaxed);
    RELEASE_ALL.fetch_add(1, Ordering::AcqRel);
    held as usize
}

fn present_release(address: u64) -> bool {
    if crate::env_toggle!("TUXBOX_PRESENT_RELEASE_ALL") == Some("1") {
        return true;
    }
    crate::env_toggle!("TUXBOX_MEMORY_WAIT_FORCE") != Some("0") && writers::writer_queue(address).is_none()
}

fn hoisted(list: &List, address: u64, target: u64) {
    capture_wait(list, "moved to the start of its host list", address, target);
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "GPU memory wait {address:#x} >= {target:#x} after rendering commands moved to the start of its host list ({n} so far; TUXBOX_WAIT_HOIST=0 skips it)"));
    }
}

/// TuxBox 3f2a717: a wait that cannot be honoured inside a list still orders
/// the work already recorded in it with a global UAV barrier, so the list's
/// own earlier writes land before what follows. A value another queue or the
/// CPU produces is still not waited for. TUXBOX_WAIT_SKIP_BARRIER=0 records
/// nothing, as before.
fn skip_barrier(list: &List) {
    use windows::Win32::Graphics::Direct3D12::*;
    if crate::env_toggle!("TUXBOX_WAIT_SKIP_BARRIER") == Some("0")
        || unsafe { list.native.GetType() } == D3D12_COMMAND_LIST_TYPE_COPY {
        return;
    }
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER { pResource: std::mem::ManuallyDrop::new(None) }),
        },
    };
    // The list is open and recording; a null resource names every UAV.
    unsafe { list.native.ResourceBarrier(&[barrier]) };
}

/// Whether a GPU memory wait is holding a queue's work back.
pub(super) fn held() -> bool {
    HELD.load(Ordering::Relaxed) != 0
}

struct Holding;
impl Drop for Holding {
    fn drop(&mut self) {
        HELD.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Worker {
    fn new(error: Arc<AtomicI32>) -> Result<Self> {
        let (sender, jobs) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("TuxBox GPU memory waits".into())
            .spawn(move || {
                for job in jobs {
                    let result = (|| -> Result<()> {
                        unsafe { job.fence.SetEventOnCompletion(1, HANDLE::default())? };
                        // A word nobody writes stalled the queue for good, with
                        // every later fence on it, and logged nothing. Log the
                        // stall and release the queue after a watchdog period.
                        let started = std::time::Instant::now();
                        let mut warned = false;
                        let mut dumped = false;
                        let was_dead = dead_ms() != 0 && dead(job.segment.address);
                        let mut released = false;
                        let presents = PRESENTS.load(Ordering::Acquire);
                        let releases = RELEASE_ALL.load(Ordering::Acquire);
                        let _holding = (!job.segment.word.reached(job.segment.target, job.segment.comparison))
                            .then(|| { HELD.fetch_add(1, Ordering::Relaxed); Holding });
                        while !job.segment.word.reached(job.segment.target, job.segment.comparison) {
                            unsafe {
                                job.device.GetDeviceRemovedReason()?;
                            }
                            let waited = started.elapsed();
                            if writers::writer_release_on() && waited >= Duration::from_millis(4)
                                && let Some(value) = writers::writer_done(job.segment.address, started, job.segment.target, job.segment.comparison)
                            {
                                static EARLY: AtomicU64 = AtomicU64::new(0);
                                let n = EARLY.fetch_add(1, Ordering::Relaxed) + 1;
                                if n <= 8 || n.is_power_of_two() {
                                    let now = job.segment.word.load().unwrap_or(u64::MAX);
                                    crate::diagnostics::log_line(format_args!(
                                        "GPU memory wait {:#x} >= {:#x} on queue {:#x} released after {} ms: the list writing {value:#x} there completed, yet the word reads {now:#x}{} ({n} so far; TUXBOX_WAIT_WRITER_RELEASE=0 waits)",
                                        job.segment.address, job.segment.target, job.queue, waited.as_millis(),
                                        super::page_mapping::remapped_under_import(job.segment.address)));
                                }
                                break;
                            }
                            if RELEASE_ALL.load(Ordering::Acquire) != releases {
                                static FORCED: AtomicU64 = AtomicU64::new(0);
                                let n = FORCED.fetch_add(1, Ordering::Relaxed) + 1;
                                if n <= 8 || n.is_power_of_two() {
                                    crate::diagnostics::log_line(format_args!(
                                        "GPU memory wait {:#x} >= {:#x} on queue {:#x} released by release_memory_waits after {} ms ({n} so far)",
                                        job.segment.address, job.segment.target, job.queue, waited.as_millis()));
                                }
                                break;
                            }
                            if PRESENTS.load(Ordering::Acquire) != presents && present_release(job.segment.address) {
                                static AT_PRESENT: AtomicU64 = AtomicU64::new(0);
                                let n = AT_PRESENT.fetch_add(1, Ordering::Relaxed) + 1;
                                if n <= 8 || n.is_power_of_two() {
                                    crate::diagnostics::log_line(format_args!(
                                        "GPU memory wait {:#x} >= {:#x} on queue {:#x} released at a present after {} ms: no queued list writes it ({n} so far; TUXBOX_MEMORY_WAIT_FORCE=0 waits)",
                                        job.segment.address, job.segment.target, job.queue, waited.as_millis()));
                                }
                                break;
                            }
                            // five-star-recomp ad6e007: dump every thread's stack once a
                            // wait stays pending 1.5 s, for the first three waits.
                            // TUXBOX_WAIT_STACKS sets how many (0 off), TUXBOX_WAIT_STACKS_MS when.
                            if !dumped && waited >= wait_stacks_after() {
                                dumped = true;
                                static DUMPS: AtomicU64 = AtomicU64::new(0);
                                let n = DUMPS.fetch_add(1, Ordering::Relaxed) + 1;
                                if n <= wait_stacks_count() {
                                    crate::diagnostics::crash::wait_stacks(job.segment.address, job.segment.target, job.queue, waited.as_millis(), n as u32);
                                }
                            }
                            if !warned && waited >= Duration::from_secs(2) {
                                warned = true;
                                let slot = STALLED_COUNT.fetch_add(1, Ordering::Relaxed) as usize;
                                if slot < 16 { STALLED[slot].store(job.segment.address, Ordering::Relaxed); }
                                writers::arm();
                                static STALLS: AtomicU64 = AtomicU64::new(0);
                                if slow::per_address(slow::Kind::Stalled, job.segment.address, &STALLS).is_some() {
                                crate::diagnostics::log_line(format_args!(
                                    "GPU memory wait {:#x} >= {:#x} on queue {:#x} stalled 2 s{}{}{}",
                                    job.segment.address, job.segment.target, job.queue,
                                    writers::describe(job.segment.address, job.queue, started),
                                    writers::writer_queue(job.segment.address).map(crate::diagnostics::queue_waits::describe).unwrap_or_default(),
                                    super::page_mapping::remapped_under_import(job.segment.address)
                                ));
                                }
                            }
                            // rebox 69005a5 releases after 2 s; TUXBOX_WAIT_RELEASE_MS
                            // overrides (the old 10 s was most of a frame budget per stall).
                            let release = Duration::from_millis(crate::env_toggle!("TUXBOX_WAIT_RELEASE_MS")
                                .and_then(|v| v.parse().ok()).unwrap_or(2000));
                            let release = if was_dead { release.min(Duration::from_millis(dead_ms())) } else { release };
                            if waited >= release {
                                released = true;
                                mark_dead(job.segment.address, true);
                                static RELEASED: AtomicU64 = AtomicU64::new(0);
                                if slow::per_address(slow::Kind::Released, job.segment.address, &RELEASED).is_some() {
                                    crate::diagnostics::log_line(format_args!(
                                        "GPU memory wait {:#x} >= {:#x} released by the watchdog after {:?} ({} so far)",
                                        job.segment.address, job.segment.target, release, RELEASED.load(Ordering::Relaxed)
                                    ));
                                }
                                break;
                            }
                            std::thread::sleep(Duration::from_millis(1));
                        }
                        // five-star ad07dd9 (GPL-3.0, marquezini): an address that
                        // timed out is not waited on in full again, for good.
                        // TUXBOX_WAIT_DEAD_STICKY=0 forgets it once a wait on it
                        // completes on its own, as before.
                        if !released && was_dead && crate::env_toggle!("TUXBOX_WAIT_DEAD_STICKY") == Some("0") {
                            mark_dead(job.segment.address, false);
                        }
                        slow::memory_wait(job.segment.address, job.segment.target, job.queue, started.elapsed());
                        crate::diagnostics::cost::add(
                            crate::diagnostics::cost::Bucket::MemoryWait, started.elapsed().as_nanos() as u64);
                        unsafe { job.fence.Signal(2)?; }
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FIRST, || format!(
                            "GPU memory wait completed target={:#x}", job.segment.target
                        ));
                        Ok(())
                    })();
                    if let Err(cause) = result {
                        let _ = error.compare_exchange(
                            S_OK.0,
                            cause.code().0,
                            Ordering::AcqRel,
                            Ordering::Acquire,
                        );
                        // Fail the device rather than releasing work behind a broken gate.
                        if let Ok(device) = job.device.cast::<ID3D12Device5>() {
                            unsafe { device.RemoveDevice() };
                        }
                        break;
                    }
                }
            })
            .map_err(|_| E_OUTOFMEMORY)?;
        Ok(Self(sender))
    }
}

pub(super) type Splits = Vec<(usize, Arc<Segment>)>;

/// Wait diagnostics (rebox f65379f, re-derived; TUXBOX_WAIT_LOGS=0 off):
/// a queue Wait on a fence value not yet signalled when it is queued, and a
/// memory wait met only after TUXBOX_SLOW_WAIT_MS (default 50 ms).
pub(super) mod slow {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;
    use windows::Win32::Graphics::Direct3D12::ID3D12Fence;

    fn on() -> bool {
        crate::env_toggle!("TUXBOX_WAIT_LOGS") != Some("0")
    }

    pub(crate) fn threshold() -> Duration {
        Duration::from_millis(crate::env_toggle!("TUXBOX_SLOW_WAIT_MS").and_then(|v| v.parse().ok()).unwrap_or(50))
    }

    /// Slow, stalled and released waits log at most this many times per
    /// address and kind (rebox a8ee5d6), so stalls on different counters all
    /// show and one hot address cannot flood the log. TUXBOX_WAIT_LOG_CAP=0
    /// restores the old global thinning (first 8, then powers of two).
    fn cap() -> u32 {
        crate::env_toggle!("TUXBOX_WAIT_LOG_CAP").and_then(|v| v.parse().ok()).unwrap_or(4)
    }

    #[derive(Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum Kind { Slow, Stalled, Released }

    fn admit(seen: &mut std::collections::HashMap<(Kind, u64), u32>, key: (Kind, u64), cap: u32) -> Option<u32> {
        let n = seen.entry(key).or_insert(0);
        (*n < cap).then(|| { *n += 1; *n })
    }

    /// Whether this wait at `address` logs: Some(its count for that address),
    /// or with the cap off, the global thinning over `global`.
    pub(crate) fn per_address(kind: Kind, address: u64, global: &AtomicU64) -> Option<u64> {
        let cap = cap();
        if cap == 0 {
            return due(global);
        }
        static SEEN: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<(Kind, u64), u32>>> =
            std::sync::LazyLock::new(Default::default);
        let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
        global.fetch_add(1, Ordering::Relaxed);
        admit(&mut seen, (kind, address), cap).map(u64::from)
    }

    /// Log the first 8, then at powers of two.
    fn due(count: &AtomicU64) -> Option<u64> {
        let n = count.fetch_add(1, Ordering::Relaxed) + 1;
        (n <= 8 || n.is_power_of_two()).then_some(n)
    }

    pub(crate) fn queue_wait(queue: usize, fence: &ID3D12Fence, value: u64) {
        if !on() {
            return;
        }
        let now = unsafe { fence.GetCompletedValue() };
        if now >= value {
            return;
        }
        static COUNT: AtomicU64 = AtomicU64::new(0);
        if let Some(n) = due(&COUNT) {
            crate::diagnostics::log_line(format_args!(
                "queue {queue:#x} waits on fence {:p} for {value} not yet signalled (now {now}; {n} such waits)",
                windows::core::Interface::as_raw(fence)
            ));
        }
    }

    pub(crate) fn memory_wait(address: u64, target: u64, queue: usize, waited: Duration) {
        if !on() || waited < threshold() {
            return;
        }
        static COUNT: AtomicU64 = AtomicU64::new(0);
        if let Some(n) = per_address(Kind::Slow, address, &COUNT) {
            crate::diagnostics::log_line(format_args!(
                "GPU memory wait {address:#x} >= {target:#x} on queue {queue:#x} met after {waited:?} (slow wait {n} here, {} in all)",
                COUNT.load(Ordering::Relaxed)
            ));
        }
    }

    #[cfg(test)]
    #[test]
    fn each_address_logs_up_to_the_cap() {
        let mut seen = std::collections::HashMap::new();
        let a: Vec<_> = (0..6).filter_map(|_| admit(&mut seen, (Kind::Slow, 0x1000), 4)).collect();
        assert_eq!(a, [1, 2, 3, 4]);
        // Another counter, or another kind at the same counter, still logs.
        assert_eq!(admit(&mut seen, (Kind::Slow, 0x2000), 4), Some(1));
        assert_eq!(admit(&mut seen, (Kind::Stalled, 0x1000), 4), Some(1));
    }

    #[cfg(test)]
    #[test]
    fn logging_thins_out_after_eight() {
        let c = AtomicU64::new(0);
        let logged: Vec<u64> = (0..40).filter_map(|_| due(&c)).collect();
        assert_eq!(logged, vec![1, 2, 3, 4, 5, 6, 7, 8, 16, 32]);
    }
}

/// The lists submitted before the wait of list `index` (from `start`), and
/// where the rest begins: the batch splits before each waiting list, not
/// once per batch. Marker writes a list records before its wait go in the
/// segment's prefix, which runs before the wait's fence, and each queue has
/// its own wait worker, so a cross-queue write/wait pair cannot hold both
/// queues (the cycle rebox 424eab7 fixes does not arise here).
pub(super) fn list_reset(list: usize) {
    writers::reset(list);
}

fn split_before(start: usize, index: usize) -> (core::ops::Range<usize>, usize) {
    (start..index.max(start), index.max(start))
}

#[cfg(test)]
mod split_tests {
    use super::split_before;

    /// Lists 0..5 with waits at the start of lists 1 and 3: 0 runs, wait,
    /// 1..3 run (2 writes the word 3 waits on), wait, then 3..5.
    #[test]
    fn a_batch_splits_before_each_waiting_list() {
        let mut start = 0;
        let mut runs = Vec::new();
        for index in [1, 3] {
            let (before, next) = split_before(start, index);
            runs.push(before);
            start = next;
        }
        runs.push(start..5);
        assert_eq!(runs, [0..1, 1..3, 3..5]);
        // Two waiting lists in a row: nothing runs between their waits.
        assert_eq!(split_before(3, 3), (3..3, 3));
    }
}

fn capture_wait(list: &List, outcome: &str, address: u64, target: u64) {
    if crate::diagnostics::capture::active() {
        crate::diagnostics::capture::note("memory wait", list as *const List as usize, outcome,
            format_args!("{address:#x} >= {target:#x}"));
    }
}

pub(super) unsafe fn submit(queue: &Queue, host: &[*mut c_void], splits: Splits) -> Result<()> {
    let run = |lists: &[*mut c_void]| {
        if !lists.is_empty() {
            // TUXBOX_GPU_SEGMENTS=1 times each segment on the GPU.
            crate::diagnostics::gpu_segments::around(&queue.native, lists.len(), || unsafe {
                (queue.native.vtable().ExecuteCommandLists)(
                    queue.native.as_raw(),
                    lists.len() as u32,
                    lists.as_ptr(),
                )
            });
            crate::dred::check(&queue.native, "guest command lists");
            writers::progress(&queue.native, lists);
        }
    };
    writers::submitted(queue.native.as_raw() as usize, host);
    let mut start = 0;
    for (index, segment) in splits {
        // Every list before the waiting one runs first (rebox 7f08cf4): a wait
        // on a word an earlier list of the same batch writes must not hold it.
        let (before, next) = split_before(start, index);
        run(&host[before]);
        start = next;
        let mut device: Option<ID3D12Device> = None;
        unsafe {
            queue.native.GetDevice(&mut device)?;
        }
        let device = device.ok_or(E_FAIL)?;
        let fence = unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE)? };
        let worker = match queue.state.wait_worker.get() {
            Some(worker) => worker,
            None => {
                let new = Worker::new(queue.state.placement.error.clone())?;
                queue.state.wait_worker.get_or_init(|| new)
            }
        };
        if let Some((prefix, _)) = &segment.prefix {
            run(&[prefix.as_raw()]);
        }
        writers::wait_queued(queue.native.as_raw() as usize, segment.address, segment.comparison, segment.target);
        // Fence 1 retires the entire prefix before the CPU reads the guest word.
        // Fence 2 holds the suffix on the GPU without blocking the guest submission thread.
        unsafe {
            queue.native.Signal(&fence, 1)?;
        }
        worker
            .0
            .send(Job {
                queue: queue.native.as_raw() as usize,
                fence: fence.clone(),
                segment,
                device,
            })
            .map_err(|_| E_FAIL)?;
        unsafe {
            queue.native.Wait(&fence, 2)?;
            crate::diagnostics::queue_waits::note(queue.native.as_raw() as usize, "GPU memory wait", &fence, 2);
        }
    }
    run(&host[start..]);
    Ok(())
}
