//! XTaskQueue – GDK's async dispatch primitive (contract: libHttpClient XTaskQueue.h, MIT).
//!
//! Dispatch modes: Manual (caller pumps `XTaskQueueDispatch`), ThreadPool,
//! SerializedThreadPool (pool, one at a time) and Immediate (run on the submitting
//! thread). `XTaskQueueTerminate` runs the queue's pending callbacks (delayed ones included)
//! with `canceled = true` before its own callback; after it the queue takes no new work
//! (`E_ABORT`, like libHttpClient's `VerifyNotTerminated`).
//!
//! Ports are reference-counted (`Arc<Port>`) so composite queues created via
//! `XTaskQueueGetPort` + `XTaskQueueCreateComposite` can share underlying work/completion
//! ports without terminating or cancelling items belonging to sibling queues.

#![allow(non_snake_case)]

use std::collections::{BinaryHeap, HashMap, HashSet, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use crate::util::{E_INVALIDARG, HRESULT, S_OK};

pub const MODE_MANUAL: u32 = 0;
pub const MODE_THREAD_POOL: u32 = 1;
pub const MODE_SERIALIZED_THREAD_POOL: u32 = 2;
pub const MODE_IMMEDIATE: u32 = 3;
pub const PORT_WORK: u32 = 0;
pub const PORT_COMPLETION: u32 = 1;
pub const E_NO_TASK_QUEUE: HRESULT = 0x8007_01ABu32 as i32;

pub type XTaskQueueCallback = unsafe extern "system" fn(*mut c_void, bool);
pub type XTaskQueueTerminatedCallback = unsafe extern "system" fn(*mut c_void);
pub type XTaskQueueMonitorCallback = unsafe extern "system" fn(*mut c_void, *mut c_void, u32);

#[derive(Clone, Copy)]
pub struct Item {
    pub cb: usize,
    pub ctx: usize,
}

impl Item {
    pub unsafe fn run(self, canceled: bool) {
        let f: XTaskQueueCallback = std::mem::transmute(self.cb);
        f(self.ctx as *mut c_void, canceled);
    }
}

struct QueuedItem {
    item: Item,
    queue: Arc<Queue>,
    port: u32,
}

#[derive(Clone, Copy)]
struct Monitor {
    token: u64,
    queue_handle: usize,
    ctx: usize,
    cb: usize,
}

pub struct Port {
    mode: u32,
    items: Mutex<VecDeque<QueuedItem>>,
    cv: Condvar,
    running: AtomicBool,
}

impl Port {
    fn new(mode: u32) -> Arc<Port> {
        Arc::new(Port {
            mode,
            items: Mutex::new(VecDeque::new()),
            cv: Condvar::new(),
            running: AtomicBool::new(false),
        })
    }
}

struct QueueState {
    terminated: bool,
    inflight: [usize; 2],
}

pub struct Queue {
    ports: [Arc<Port>; 2],
    #[allow(dead_code)]
    owns_ports: bool,
    state: Mutex<QueueState>,
    idle_cv: Condvar,
    monitors: Mutex<Vec<Monitor>>,
}

/// What an `XTaskQueueHandle` points to.
pub struct QueueHandle {
    magic: u64,
    pub queue: Arc<Queue>,
}

const MAGIC: u64 = 0x5843_4C51_5545_5545; // "XCLQUEUE"

// ---------------------------------------------------------------------------------------
// shared worker pool + timer
// ---------------------------------------------------------------------------------------

type Job = Box<dyn FnOnce() + Send>;

fn pool() -> &'static Mutex<mpsc::Sender<Job>> {
    static POOL: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    POOL.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8);
        for i in 0..n {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("xcl-taskqueue-{i}"))
                .spawn(move || loop {
                    let job = { rx.lock().unwrap().recv() };
                    match job {
                        Ok(j) => j(),
                        Err(_) => break,
                    }
                })
                .expect("spawn task queue worker");
        }
        Mutex::new(tx)
    })
}

fn spawn(job: Job) {
    let _ = pool().lock().unwrap().send(job);
}

struct Timed {
    at: Instant,
    seq: u64,
    queue: Arc<Queue>,
    port: u32,
    item: Item,
}
impl PartialEq for Timed {
    fn eq(&self, o: &Self) -> bool {
        self.at == o.at && self.seq == o.seq
    }
}
impl Eq for Timed {}
impl PartialOrd for Timed {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Timed {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // min-heap on (at, seq)
        o.at.cmp(&self.at).then(o.seq.cmp(&self.seq))
    }
}

fn timer() -> &'static (Mutex<BinaryHeap<Timed>>, Condvar) {
    static T: OnceLock<(Mutex<BinaryHeap<Timed>>, Condvar)> = OnceLock::new();
    static STARTED: AtomicBool = AtomicBool::new(false);
    let t = T.get_or_init(|| (Mutex::new(BinaryHeap::new()), Condvar::new()));
    if !STARTED.swap(true, Ordering::AcqRel) {
        std::thread::Builder::new()
            .name("xcl-taskqueue-timer".into())
            .spawn(|| {
                let (m, cv) = timer();
                let mut g = m.lock().unwrap();
                loop {
                    let now = Instant::now();
                    match g.peek() {
                        None => g = cv.wait(g).unwrap(),
                        Some(t) if t.at <= now => {
                            let t = g.pop().unwrap();
                            drop(g);
                            let _ = submit_now(&t.queue, t.port, t.item);
                            g = m.lock().unwrap();
                        }
                        Some(t) => {
                            let d = t.at - now;
                            g = cv.wait_timeout(g, d).unwrap().0;
                        }
                    }
                }
            })
            .expect("spawn timer");
    }
    t
}

static SEQ: AtomicU64 = AtomicU64::new(0);
static NEXT_MONITOR_TOKEN: AtomicU64 = AtomicU64::new(1);

// ---------------------------------------------------------------------------------------
// core operations
// ---------------------------------------------------------------------------------------

impl Queue {
    pub fn new(work_mode: u32, completion_mode: u32) -> Arc<Queue> {
        Arc::new(Queue {
            ports: [Port::new(work_mode), Port::new(completion_mode)],
            owns_ports: true,
            state: Mutex::new(QueueState { terminated: false, inflight: [0, 0] }),
            idle_cv: Condvar::new(),
            monitors: Mutex::new(Vec::new()),
        })
    }

    pub fn composite(work: Arc<Port>, completion: Arc<Port>) -> Arc<Queue> {
        Arc::new(Queue {
            ports: [work, completion],
            owns_ports: false,
            state: Mutex::new(QueueState { terminated: false, inflight: [0, 0] }),
            idle_cv: Condvar::new(),
            monitors: Mutex::new(Vec::new()),
        })
    }

    pub fn is_terminated(&self) -> bool {
        self.state.lock().unwrap().terminated
    }

    fn done_one(&self, port: u32) {
        let mut state = self.state.lock().unwrap();
        state.inflight[port as usize] -= 1;
        if state.inflight[0] == 0 && state.inflight[1] == 0 {
            self.idle_cv.notify_all();
        }
    }

    unsafe fn execute(self: &Arc<Queue>, port: u32, item: Item) {
        let canceled = self.is_terminated();
        item.run(canceled);
        self.done_one(port);
    }

    fn notify_monitors(&self, port: u32) {
        let snapshot: Vec<Monitor> = self.monitors.lock().unwrap().clone();
        for m in snapshot {
            unsafe {
                let f: XTaskQueueMonitorCallback = std::mem::transmute(m.cb);
                f(m.ctx as *mut c_void, m.queue_handle as *mut c_void, port);
            }
        }
    }
}

fn drain_serialized(p: Arc<Port>) {
    loop {
        let next = p.items.lock().unwrap().pop_front();
        match next {
            Some(qi) => unsafe { qi.queue.execute(qi.port, qi.item) },
            None => {
                p.running.store(false, Ordering::Release);
                // re-check: an item may have arrived after pop but before the store
                let more = !p.items.lock().unwrap().is_empty();
                if more && !p.running.swap(true, Ordering::AcqRel) {
                    continue;
                }
                break;
            }
        }
    }
}

/// Submit an item to `port` immediately. Returns `false` if `q` is already terminated.
///
/// Lock order is always `p.items` (or `timer`) -> `q.state` so submission and `XTaskQueueTerminate`
/// cannot race or deadlock.
pub fn submit_now(q: &Arc<Queue>, port: u32, item: Item) -> bool {
    let p = &q.ports[port as usize];
    match p.mode {
        MODE_IMMEDIATE => {
            {
                let mut state = q.state.lock().unwrap();
                if state.terminated {
                    return false;
                }
                state.inflight[port as usize] += 1;
            }
            q.notify_monitors(port);
            // SAFETY: Caller guarantees `item.cb` is a valid `XTaskQueueCallback` for `item.ctx`.
            unsafe { q.execute(port, item) };
        }
        MODE_THREAD_POOL => {
            {
                let mut state = q.state.lock().unwrap();
                if state.terminated {
                    return false;
                }
                state.inflight[port as usize] += 1;
            }
            q.notify_monitors(port);
            let q2 = q.clone();
            spawn(Box::new(move || unsafe { q2.execute(port, item) }));
        }
        MODE_SERIALIZED_THREAD_POOL => {
            {
                let mut items = p.items.lock().unwrap();
                {
                    let mut state = q.state.lock().unwrap();
                    if state.terminated {
                        return false;
                    }
                    state.inflight[port as usize] += 1;
                }
                items.push_back(QueuedItem { item, queue: q.clone(), port });
            }
            q.notify_monitors(port);
            if !p.running.swap(true, Ordering::AcqRel) {
                let p2 = p.clone();
                spawn(Box::new(move || drain_serialized(p2)));
            }
        }
        _ => {
            {
                let mut items = p.items.lock().unwrap();
                {
                    let mut state = q.state.lock().unwrap();
                    if state.terminated {
                        return false;
                    }
                    state.inflight[port as usize] += 1;
                }
                items.push_back(QueuedItem { item, queue: q.clone(), port });
            }
            p.cv.notify_one();
            q.notify_monitors(port);
        }
    }
    true
}

/// Submit an item to `port` after `delay_ms`. Returns `false` if `q` is already terminated.
pub fn submit_delayed(q: &Arc<Queue>, port: u32, delay_ms: u32, item: Item) -> bool {
    if delay_ms == 0 {
        return submit_now(q, port, item);
    }
    let (m, cv) = timer();
    let mut heap = m.lock().unwrap();
    if q.is_terminated() {
        return false;
    }
    heap.push(Timed {
        at: Instant::now() + Duration::from_millis(delay_ms as u64),
        seq: SEQ.fetch_add(1, Ordering::Relaxed),
        queue: q.clone(),
        port,
        item,
    });
    drop(heap);
    cv.notify_all();
    true
}

/// Dispatch one item from a Manual port. Returns true when something ran.
pub fn dispatch(q: &Arc<Queue>, port: u32, timeout_ms: u32) -> bool {
    let p = &q.ports[port as usize];
    if p.mode != MODE_MANUAL {
        return false;
    }
    let mut g = p.items.lock().unwrap();
    if g.is_empty() && timeout_ms != 0 {
        let dur = if timeout_ms == u32::MAX { Duration::from_secs(u64::MAX / 4) } else { Duration::from_millis(timeout_ms as u64) };
        let deadline = Instant::now() + dur.min(Duration::from_secs(365 * 24 * 3600));
        while g.is_empty() && !q.is_terminated() {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            g = p.cv.wait_timeout(g, deadline - now).unwrap().0;
        }
    }
    let Some(qi) = g.pop_front() else { return false };
    drop(g);
    unsafe { qi.queue.execute(qi.port, qi.item) };
    true
}

// ---------------------------------------------------------------------------------------
// handles + port handles + process queue
// ---------------------------------------------------------------------------------------

/// Live queue handles: a handle is only read while it is in here (under the lock), so one
/// closed on another thread – or a stale value in an XAsyncBlock – is not dereferenced.
static HANDLES: Mutex<Option<HashSet<usize>>> = Mutex::new(None);

/// Weak registry of `XTaskQueuePortHandle` values handed out by `XTaskQueueGetPort`.
/// GDK has no close function for port handles; weak references keep them valid as long as
/// any queue referencing the port is alive without leaking memory.
static PORT_HANDLES: Mutex<Option<HashMap<usize, Weak<Port>>>> = Mutex::new(None);

fn register_port_handle(p: &Arc<Port>) -> *mut c_void {
    let key = Arc::as_ptr(p) as usize;
    let mut guard = PORT_HANDLES.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    if map.len() > 64 {
        map.retain(|_, w| w.strong_count() > 0);
    }
    map.insert(key, Arc::downgrade(p));
    key as *mut c_void
}

fn port_from_handle(h: *mut c_void) -> Option<Arc<Port>> {
    if h.is_null() {
        return None;
    }
    let guard = PORT_HANDLES.lock().unwrap();
    guard.as_ref()?.get(&(h as usize))?.upgrade()
}

pub fn new_handle(q: Arc<Queue>) -> *mut QueueHandle {
    let h = Box::into_raw(Box::new(QueueHandle { magic: MAGIC, queue: q }));
    HANDLES.lock().unwrap().get_or_insert_with(HashSet::new).insert(h as usize);
    h
}

pub unsafe fn queue_from(h: *mut c_void) -> Option<Arc<Queue>> {
    if h.is_null() {
        return process_queue();
    }
    let live = HANDLES.lock().unwrap();
    if !live.as_ref().is_some_and(|s| s.contains(&(h as usize))) {
        return None;
    }
    let qh = &*(h as *const QueueHandle);
    (qh.magic == MAGIC).then(|| qh.queue.clone())
}

struct ProcessQueueState {
    initialized: bool,
    queue: Option<Arc<Queue>>,
}

impl ProcessQueueState {
    fn current(&mut self) -> Option<Arc<Queue>> {
        if !self.initialized {
            self.initialized = true;
            self.queue = Some(Queue::new(MODE_THREAD_POOL, MODE_THREAD_POOL));
        }
        self.queue.clone()
    }

    fn set(&mut self, queue: Option<Arc<Queue>>) {
        self.initialized = true;
        self.queue = queue;
    }
}

static PROCESS_QUEUE: Mutex<ProcessQueueState> = Mutex::new(ProcessQueueState {
    initialized: false,
    queue: None,
});

pub fn process_queue() -> Option<Arc<Queue>> {
    PROCESS_QUEUE.lock().unwrap().current()
}

// ---------------------------------------------------------------------------------------
// exported API
// ---------------------------------------------------------------------------------------

pub unsafe extern "system" fn XTaskQueueCreate(work: u32, completion: u32, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || work > 3 || completion > 3 {
        return E_INVALIDARG;
    }
    *out = new_handle(Queue::new(work, completion)) as *mut c_void;
    S_OK
}

pub unsafe extern "system" fn XTaskQueueGetPort(h: *mut c_void, port: u32, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() || port > 1 {
        return E_INVALIDARG;
    }
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    *out = register_port_handle(&q.ports[port as usize]);
    S_OK
}

pub unsafe extern "system" fn XTaskQueueCreateComposite(
    work_port: *mut c_void,
    completion_port: *mut c_void,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    let (Some(w), Some(c)) = (port_from_handle(work_port), port_from_handle(completion_port)) else {
        return E_INVALIDARG;
    };
    *out = new_handle(Queue::composite(w, c)) as *mut c_void;
    S_OK
}

pub unsafe extern "system" fn XTaskQueueDuplicateHandle(h: *mut c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    match queue_from(h) {
        Some(q) => {
            *out = new_handle(q) as *mut c_void;
            S_OK
        }
        None => E_INVALIDARG,
    }
}

pub unsafe extern "system" fn XTaskQueueCloseHandle(h: *mut c_void) {
    if h.is_null() {
        return;
    }
    let removed = HANDLES.lock().unwrap().as_mut().is_some_and(|s| s.remove(&(h as usize)));
    if removed {
        let qh = h as *mut QueueHandle;
        (*qh).magic = 0;
        drop(Box::from_raw(qh));
    }
}

pub unsafe extern "system" fn XTaskQueueSubmitCallback(h: *mut c_void, port: u32, ctx: *mut c_void, cb: usize) -> HRESULT {
    if cb == 0 || port > 1 {
        return E_INVALIDARG;
    }
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    if !submit_now(&q, port, Item { cb, ctx: ctx as usize }) {
        return crate::util::E_ABORT;
    }
    S_OK
}

pub unsafe extern "system" fn XTaskQueueSubmitDelayedCallback(h: *mut c_void, port: u32, delay: u32, ctx: *mut c_void, cb: usize) -> HRESULT {
    if cb == 0 || port > 1 {
        return E_INVALIDARG;
    }
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    // A terminated queue takes no new work (libHttpClient: VerifyNotTerminated). A delayed
    // callback accepted now would run after the termination callback.
    if !submit_delayed(&q, port, delay, Item { cb, ctx: ctx as usize }) {
        return crate::util::E_ABORT;
    }
    S_OK
}

pub unsafe extern "system" fn XTaskQueueDispatch(h: *mut c_void, port: u32, timeout: u32) -> bool {
    if port > 1 {
        return false;
    }
    match queue_from(h) {
        Some(q) => dispatch(&q, port, timeout),
        None => false,
    }
}

pub unsafe extern "system" fn XTaskQueueIsEmpty(h: *mut c_void, port: u32) -> bool {
    match queue_from(h) {
        Some(q) if port <= 1 => {
            let items = q.ports[port as usize].items.lock().unwrap();
            let state = q.state.lock().unwrap();
            !items.iter().any(|qi| Arc::ptr_eq(&qi.queue, &q)) && state.inflight[port as usize] == 0
        }
        _ => true,
    }
}

pub unsafe extern "system" fn XTaskQueueTerminate(h: *mut c_void, wait: bool, ctx: *mut c_void, cb: usize) -> HRESULT {
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    q.state.lock().unwrap().terminated = true;
    // Cancel only queued items submitted through `q`; sibling queues sharing a composite port
    // keep their pending work untouched. Wake any manual-port dispatchers waiting on this port.
    for port in 0..2 {
        let mine = {
            let mut items = q.ports[port].items.lock().unwrap();
            let mut mine = Vec::new();
            let mut keep = VecDeque::with_capacity(items.len());
            while let Some(qi) = items.pop_front() {
                if Arc::ptr_eq(&qi.queue, &q) {
                    mine.push(qi);
                } else {
                    keep.push_back(qi);
                }
            }
            *items = keep;
            mine
        };
        q.ports[port].cv.notify_all();
        for qi in mine {
            qi.queue.execute(qi.port, qi.item);
        }
    }
    // Delayed callbacks of `q` still waiting for their time are canceled too, before the
    // termination callback (libHttpClient: CancelPendingEntries when a port terminates). They
    // only count as in flight once due, so the wait below would not hold the callback for them,
    // and they would run (canceled) after it – when the title may already have freed its context.
    let delayed: Vec<Timed> = {
        let (m, cv) = timer();
        let mut heap = m.lock().unwrap();
        let (mine, keep): (Vec<Timed>, Vec<Timed>) = std::mem::take(&mut *heap).into_vec().into_iter().partition(|t| Arc::ptr_eq(&t.queue, &q));
        *heap = BinaryHeap::from(keep);
        cv.notify_all();
        mine
    };
    for t in delayed {
        t.queue.state.lock().unwrap().inflight[t.port as usize] += 1;
        t.queue.execute(t.port, t.item);
    }
    let ctx = ctx as usize; // raw pointers are not Send
    let finish = move || {
        let mut g = q.state.lock().unwrap();
        while g.inflight[0] != 0 || g.inflight[1] != 0 {
            g = q.idle_cv.wait(g).unwrap();
        }
        drop(g);
        if cb != 0 {
            let f: XTaskQueueTerminatedCallback = std::mem::transmute(cb);
            f(ctx as *mut c_void);
        }
    };
    if wait {
        finish();
    } else {
        spawn(Box::new(finish));
    }
    S_OK
}

pub unsafe extern "system" fn XTaskQueueGetCurrentProcessTaskQueue(out: *mut *mut c_void) -> bool {
    if out.is_null() {
        return false;
    }
    match process_queue() {
        Some(queue) => {
            *out = new_handle(queue) as *mut c_void;
            true
        }
        None => {
            *out = std::ptr::null_mut();
            false
        }
    }
}

pub unsafe extern "system" fn XTaskQueueSetCurrentProcessTaskQueue(h: *mut c_void) {
    if h.is_null() {
        PROCESS_QUEUE.lock().unwrap().set(None);
    } else if let Some(queue) = queue_from(h) {
        PROCESS_QUEUE.lock().unwrap().set(Some(queue));
    }
}

#[repr(C)]
pub struct RegistrationToken {
    pub token: u64,
}

struct Waiter {
    queue: Arc<Queue>,
    port: u32,
    item: Item,
    wait: usize,
}

unsafe extern "system" fn waiter_fired(ctx: *mut c_void, _timed_out: bool) {
    let w = &*(ctx as *const Waiter);
    if !w.queue.is_terminated() {
        let _ = submit_now(&w.queue, w.port, w.item);
    }
}

pub unsafe extern "system" fn XTaskQueueRegisterWaiter(
    h: *mut c_void,
    port: u32,
    handle: *mut c_void,
    ctx: *mut c_void,
    cb: usize,
    token: *mut RegistrationToken,
) -> HRESULT {
    use windows_sys::Win32::System::Threading::{RegisterWaitForSingleObject, INFINITE, WT_EXECUTEDEFAULT};
    if port > 1 || token.is_null() || cb == 0 {
        return E_INVALIDARG;
    }
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    if q.is_terminated() {
        return crate::util::E_ABORT;
    }
    let w = Box::into_raw(Box::new(Waiter { queue: q, port, item: Item { cb, ctx: ctx as usize }, wait: 0 }));
    let mut wh = std::ptr::null_mut();
    if RegisterWaitForSingleObject(&mut wh, handle, Some(waiter_fired), w as *const c_void, INFINITE, WT_EXECUTEDEFAULT) == 0 {
        drop(Box::from_raw(w));
        return crate::util::E_FAIL;
    }
    (*w).wait = wh as usize;
    (*token).token = w as u64;
    S_OK
}

pub unsafe extern "system" fn XTaskQueueUnregisterWaiter(_h: *mut c_void, token: RegistrationToken) {
    use windows_sys::Win32::System::Threading::UnregisterWaitEx;
    let w = token.token as *mut Waiter;
    if w.is_null() {
        return;
    }
    UnregisterWaitEx((*w).wait as *mut c_void, windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE);
    drop(Box::from_raw(w));
}

pub unsafe extern "system" fn XTaskQueueRegisterMonitor(
    h: *mut c_void,
    ctx: *mut c_void,
    cb: usize,
    token: *mut RegistrationToken,
) -> HRESULT {
    let Some(q) = queue_from(h) else { return E_INVALIDARG };
    if token.is_null() || cb == 0 {
        return E_INVALIDARG;
    }
    let id = NEXT_MONITOR_TOKEN.fetch_add(1, Ordering::Relaxed);
    q.monitors.lock().unwrap().push(Monitor {
        token: id,
        queue_handle: h as usize,
        ctx: ctx as usize,
        cb,
    });
    (*token).token = id;
    S_OK
}

pub unsafe extern "system" fn XTaskQueueUnregisterMonitor(h: *mut c_void, token: RegistrationToken) {
    let Some(q) = queue_from(h) else { return };
    q.monitors.lock().unwrap().retain(|m| m.token != token.token);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static HITS: AtomicU32 = AtomicU32::new(0);
    static CANCELED: AtomicU32 = AtomicU32::new(0);
    static MONITOR_PORTS: AtomicU32 = AtomicU32::new(0);

    unsafe extern "system" fn bump(ctx: *mut c_void, canceled: bool) {
        HITS.fetch_add(ctx as usize as u32, Ordering::SeqCst);
        if canceled {
            CANCELED.fetch_add(1, Ordering::SeqCst);
        }
    }

    unsafe extern "system" fn on_monitor(ctx: *mut c_void, _q: *mut c_void, port: u32) {
        let bit = 1u32 << port;
        if !ctx.is_null() {
            let slot = &*(ctx as *const AtomicU32);
            slot.fetch_or(bit, Ordering::SeqCst);
        } else {
            MONITOR_PORTS.fetch_or(bit, Ordering::SeqCst);
        }
    }

    #[test]
    fn closed_and_stray_handles_are_not_read() {
        unsafe {
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(0, 0, &mut h), 0);
            assert!(queue_from(h).is_some());
            XTaskQueueCloseHandle(h);
            assert!(queue_from(h).is_none(), "a closed handle");
            XTaskQueueCloseHandle(h); // twice: nothing to free
            assert!(queue_from(0x1235 as *mut c_void).is_none(), "a stray (misaligned) value is not dereferenced");
        }
    }

    #[test]
    fn process_queue_state_distinguishes_default_disabled_and_replacement() {
        let mut state = ProcessQueueState { initialized: false, queue: None };
        let default = state.current().expect("default process queue was not materialized");
        assert_eq!(default.ports[PORT_WORK as usize].mode, MODE_THREAD_POOL);
        assert_eq!(default.ports[PORT_COMPLETION as usize].mode, MODE_THREAD_POOL);

        state.set(None);
        assert!(state.current().is_none(), "disabled process queue was silently recreated");

        let replacement = Queue::new(MODE_MANUAL, MODE_IMMEDIATE);
        state.set(Some(replacement.clone()));
        let current = state.current().expect("replacement process queue was lost");
        assert!(Arc::ptr_eq(&current, &replacement));
        drop(replacement);
        assert!(state.current().is_some(), "process queue did not retain its own reference");
    }

    #[test]
    fn manual_threadpool_immediate_delayed_and_terminate() {
        unsafe {
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_MANUAL, MODE_IMMEDIATE, &mut h), S_OK);
            let before = HITS.load(Ordering::SeqCst);
            assert_eq!(XTaskQueueSubmitCallback(h, PORT_WORK, 1 as _, bump as usize), S_OK);
            assert_eq!(HITS.load(Ordering::SeqCst), before, "manual port must wait for dispatch");
            assert!(XTaskQueueDispatch(h, PORT_WORK, 0));
            assert_eq!(HITS.load(Ordering::SeqCst), before + 1);
            assert!(!XTaskQueueDispatch(h, PORT_WORK, 0));
            // immediate completion port runs inline
            XTaskQueueSubmitCallback(h, PORT_COMPLETION, 10 as _, bump as usize);
            assert_eq!(HITS.load(Ordering::SeqCst), before + 11);
            // delayed on manual port: dispatch with timeout waits for it
            XTaskQueueSubmitDelayedCallback(h, PORT_WORK, 30, 100 as _, bump as usize);
            assert!(XTaskQueueDispatch(h, PORT_WORK, 2000));
            assert_eq!(HITS.load(Ordering::SeqCst), before + 111);
            // terminate cancels pending manual work
            XTaskQueueSubmitCallback(h, PORT_WORK, 1000 as _, bump as usize);
            let c0 = CANCELED.load(Ordering::SeqCst);
            assert_eq!(XTaskQueueTerminate(h, true, std::ptr::null_mut(), 0), S_OK);
            assert_eq!(CANCELED.load(Ordering::SeqCst), c0 + 1);
            XTaskQueueCloseHandle(h);

            // thread pool + serialized
            for mode in [MODE_THREAD_POOL, MODE_SERIALIZED_THREAD_POOL] {
                let mut t = std::ptr::null_mut();
                XTaskQueueCreate(mode, mode, &mut t);
                let b = HITS.load(Ordering::SeqCst);
                for _ in 0..50 {
                    XTaskQueueSubmitCallback(t, PORT_WORK, 1 as _, bump as usize);
                }
                XTaskQueueTerminate(t, true, std::ptr::null_mut(), 0);
                assert_eq!(HITS.load(Ordering::SeqCst), b + 50);
                XTaskQueueCloseHandle(t);
            }
        }
    }

    #[test]
    fn terminate_cancels_delayed_callbacks_before_its_own_callback() {
        static ORDER: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
        unsafe extern "system" fn delayed(_ctx: *mut c_void, canceled: bool) {
            ORDER.lock().unwrap().push(if canceled { "delayed, canceled" } else { "delayed, ran" });
        }
        unsafe extern "system" fn terminated(_ctx: *mut c_void) {
            ORDER.lock().unwrap().push("terminated");
        }
        unsafe {
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_THREAD_POOL, MODE_THREAD_POOL, &mut h), S_OK);
            assert_eq!(XTaskQueueSubmitDelayedCallback(h, PORT_WORK, 60_000, std::ptr::null_mut(), delayed as usize), S_OK);
            assert_eq!(XTaskQueueTerminate(h, true, std::ptr::null_mut(), terminated as usize), S_OK);
            assert_eq!(*ORDER.lock().unwrap(), ["delayed, canceled", "terminated"]);
            XTaskQueueCloseHandle(h);
        }
    }

    #[test]
    fn composite_queue_termination_preserves_shared_parent_port_work() {
        struct Rec {
            hits: AtomicU32,
            canceled: AtomicU32,
            val: u32,
        }
        unsafe extern "system" fn record(ctx: *mut c_void, canceled: bool) {
            let r = &*(ctx as *const Rec);
            r.hits.fetch_add(r.val, Ordering::SeqCst);
            if canceled {
                r.canceled.fetch_add(1, Ordering::SeqCst);
            }
        }
        unsafe {
            let mut base = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_MANUAL, MODE_MANUAL, &mut base), S_OK);
            let mut work_port = std::ptr::null_mut();
            let mut comp_port = std::ptr::null_mut();
            assert_eq!(XTaskQueueGetPort(base, PORT_WORK, &mut work_port), S_OK);
            assert_eq!(XTaskQueueGetPort(base, PORT_COMPLETION, &mut comp_port), S_OK);
            let mut comp = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreateComposite(work_port, comp_port, &mut comp), S_OK);

            // Submit one item on base and one on the composite queue sharing the same manual work port.
            let base_rec = Rec { hits: AtomicU32::new(0), canceled: AtomicU32::new(0), val: 1 };
            let comp_rec = Rec { hits: AtomicU32::new(0), canceled: AtomicU32::new(0), val: 2 };
            assert_eq!(XTaskQueueSubmitCallback(base, PORT_WORK, &base_rec as *const Rec as *mut c_void, record as usize), S_OK);
            assert_eq!(XTaskQueueSubmitCallback(comp, PORT_WORK, &comp_rec as *const Rec as *mut c_void, record as usize), S_OK);

            // Terminating the composite queue cancels ONLY the composite queue's item.
            assert_eq!(XTaskQueueTerminate(comp, true, std::ptr::null_mut(), 0), S_OK);
            assert_eq!(comp_rec.canceled.load(Ordering::SeqCst), 1);
            assert_eq!(comp_rec.hits.load(Ordering::SeqCst), 2);
            assert_eq!(base_rec.hits.load(Ordering::SeqCst), 0);
            assert!(!XTaskQueueIsEmpty(base, PORT_WORK));

            // Base queue's item still dispatches normally (canceled == false).
            assert!(XTaskQueueDispatch(base, PORT_WORK, 0));
            assert_eq!(base_rec.canceled.load(Ordering::SeqCst), 0);
            assert_eq!(base_rec.hits.load(Ordering::SeqCst), 1);
            assert!(XTaskQueueIsEmpty(base, PORT_WORK));

            XTaskQueueCloseHandle(comp);
            XTaskQueueCloseHandle(base);

            // Once all queues owning the port are closed, stale port handles are rejected.
            let mut stale = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreateComposite(work_port, comp_port, &mut stale), E_INVALIDARG);
        }
    }

    #[test]
    fn monitor_fires_on_submit_and_stops_after_unregister() {
        unsafe extern "system" fn noop(_ctx: *mut c_void, _canceled: bool) {}
        unsafe {
            let mut q = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_MANUAL, MODE_IMMEDIATE, &mut q), S_OK);
            let seen = AtomicU32::new(0);
            let mut tok = RegistrationToken { token: 0 };
            assert_eq!(
                XTaskQueueRegisterMonitor(q, &seen as *const AtomicU32 as *mut c_void, on_monitor as usize, &mut tok),
                S_OK
            );
            assert_ne!(tok.token, 0);
            assert_eq!(XTaskQueueSubmitCallback(q, PORT_WORK, std::ptr::null_mut(), noop as usize), S_OK);
            assert_eq!(seen.load(Ordering::SeqCst), 1 << PORT_WORK);
            assert_eq!(XTaskQueueSubmitCallback(q, PORT_COMPLETION, std::ptr::null_mut(), noop as usize), S_OK);
            assert_eq!(seen.load(Ordering::SeqCst), (1 << PORT_WORK) | (1 << PORT_COMPLETION));

            XTaskQueueUnregisterMonitor(q, tok);
            seen.store(0, Ordering::SeqCst);
            assert_eq!(XTaskQueueSubmitCallback(q, PORT_WORK, std::ptr::null_mut(), noop as usize), S_OK);
            assert_eq!(seen.load(Ordering::SeqCst), 0);

            XTaskQueueTerminate(q, true, std::ptr::null_mut(), 0);
            XTaskQueueCloseHandle(q);
        }
    }

    #[test]
    fn a_terminated_queue_takes_no_new_work() {
        static RAN: AtomicU32 = AtomicU32::new(0);
        unsafe extern "system" fn never(_ctx: *mut c_void, _canceled: bool) {
            RAN.fetch_add(1, Ordering::SeqCst);
        }
        unsafe {
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_THREAD_POOL, MODE_THREAD_POOL, &mut h), S_OK);
            assert_eq!(XTaskQueueTerminate(h, true, std::ptr::null_mut(), 0), S_OK);
            assert_eq!(XTaskQueueSubmitCallback(h, PORT_WORK, std::ptr::null_mut(), never as usize), crate::util::E_ABORT);
            assert_eq!(XTaskQueueSubmitDelayedCallback(h, PORT_WORK, 10, std::ptr::null_mut(), never as usize), crate::util::E_ABORT);
            let mut token = RegistrationToken { token: 0 };
            let event = windows_sys::Win32::System::Threading::CreateEventW(std::ptr::null(), 1, 1, std::ptr::null());
            assert_eq!(XTaskQueueRegisterWaiter(h, PORT_WORK, event, std::ptr::null_mut(), never as usize, &mut token), crate::util::E_ABORT);
            std::thread::sleep(Duration::from_millis(100));
            assert_eq!(RAN.load(Ordering::SeqCst), 0, "work submitted after Terminate ran");
            windows_sys::Win32::Foundation::CloseHandle(event);
            XTaskQueueCloseHandle(h);
        }
    }

    #[test]
    fn dispatch_infinite_wakes_and_returns_false_on_terminate() {
        use std::sync::Barrier;
        // 50-iteration stress test covering both ports and both racing / already-blocked dispatchers.
        for iter in 0..50 {
            let port = (iter % 2) as u32;
            unsafe {
                let mut h = std::ptr::null_mut();
                assert_eq!(XTaskQueueCreate(MODE_MANUAL, MODE_MANUAL, &mut h), S_OK);
                let h_addr = h as usize;
                let barrier = Arc::new(Barrier::new(2));
                let bar_d = barrier.clone();
                let (tx, rx) = mpsc::channel();
                let dispatcher = std::thread::spawn(move || {
                    bar_d.wait();
                    let ran = XTaskQueueDispatch(h_addr as *mut c_void, port, u32::MAX);
                    let _ = tx.send(ran);
                });
                barrier.wait();
                if iter % 2 == 0 {
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert_eq!(XTaskQueueTerminate(h, true, std::ptr::null_mut(), 0), S_OK);
                let ran = rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|_| panic!("XTaskQueueDispatch(INFINITE) hung after Terminate on iter {iter}"));
                dispatcher.join().unwrap();
                assert!(!ran);
                // Dispatching INFINITE on an already-terminated queue must return false immediately.
                let (tx2, rx2) = mpsc::channel();
                let post = std::thread::spawn(move || {
                    let _ = tx2.send(XTaskQueueDispatch(h_addr as *mut c_void, port, u32::MAX));
                });
                let ran2 = rx2.recv_timeout(Duration::from_secs(2)).expect("post-terminate Dispatch(INFINITE) hung");
                post.join().unwrap();
                assert!(!ran2);
                XTaskQueueCloseHandle(h);
            }
        }
    }

    #[test]
    fn is_empty_tracks_work_and_completion_ports_independently() {
        unsafe extern "system" fn noop(_ctx: *mut c_void, _canceled: bool) {}
        struct Gate {
            started: (Mutex<bool>, Condvar),
            release: (Mutex<bool>, Condvar),
        }
        unsafe extern "system" fn gated_work(ctx: *mut c_void, _canceled: bool) {
            let g = &*(ctx as *const Gate);
            {
                let mut s = g.started.0.lock().unwrap();
                *s = true;
                g.started.1.notify_all();
            }
            let mut r = g.release.0.lock().unwrap();
            while !*r {
                r = g.release.1.wait(r).unwrap();
            }
        }

        unsafe {
            // (a) Manual work + manual completion ports: queued items are tracked per port.
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_MANUAL, MODE_MANUAL, &mut h), S_OK);
            assert!(XTaskQueueIsEmpty(h, PORT_WORK));
            assert!(XTaskQueueIsEmpty(h, PORT_COMPLETION));

            assert_eq!(XTaskQueueSubmitCallback(h, PORT_WORK, std::ptr::null_mut(), noop as usize), S_OK);
            assert!(!XTaskQueueIsEmpty(h, PORT_WORK));
            assert!(XTaskQueueIsEmpty(h, PORT_COMPLETION), "work item must not mark completion port non-empty");

            assert_eq!(XTaskQueueSubmitCallback(h, PORT_COMPLETION, std::ptr::null_mut(), noop as usize), S_OK);
            assert!(!XTaskQueueIsEmpty(h, PORT_WORK));
            assert!(!XTaskQueueIsEmpty(h, PORT_COMPLETION));

            assert!(XTaskQueueDispatch(h, PORT_WORK, 0));
            assert!(XTaskQueueIsEmpty(h, PORT_WORK), "completion item must not mark work port non-empty");
            assert!(!XTaskQueueIsEmpty(h, PORT_COMPLETION));

            assert!(XTaskQueueDispatch(h, PORT_COMPLETION, 0));
            assert!(XTaskQueueIsEmpty(h, PORT_WORK));
            assert!(XTaskQueueIsEmpty(h, PORT_COMPLETION));
            XTaskQueueCloseHandle(h);

            // (b) ThreadPool work + Manual completion: in-flight work on PORT_WORK leaves PORT_COMPLETION empty.
            let mut h2 = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_THREAD_POOL, MODE_MANUAL, &mut h2), S_OK);
            let gate = Gate {
                started: (Mutex::new(false), Condvar::new()),
                release: (Mutex::new(false), Condvar::new()),
            };
            assert_eq!(XTaskQueueSubmitCallback(h2, PORT_WORK, &gate as *const Gate as *mut c_void, gated_work as usize), S_OK);
            {
                let mut s = gate.started.0.lock().unwrap();
                while !*s {
                    s = gate.started.1.wait(s).unwrap();
                }
            }
            assert!(!XTaskQueueIsEmpty(h2, PORT_WORK));
            assert!(XTaskQueueIsEmpty(h2, PORT_COMPLETION), "in-flight work must not make completion port non-empty");
            {
                let mut r = gate.release.0.lock().unwrap();
                *r = true;
                gate.release.1.notify_all();
            }
            assert_eq!(XTaskQueueTerminate(h2, true, std::ptr::null_mut(), 0), S_OK);
            assert!(XTaskQueueIsEmpty(h2, PORT_WORK));
            assert!(XTaskQueueIsEmpty(h2, PORT_COMPLETION));
            XTaskQueueCloseHandle(h2);
        }
    }

    #[test]
    fn register_waiter_validates_port_and_ignores_terminated_queue() {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{CreateEventW, SetEvent};

        unsafe extern "system" fn count_hit(ctx: *mut c_void, _canceled: bool) {
            let hits = &*(ctx as *const AtomicU32);
            hits.fetch_add(1, Ordering::SeqCst);
        }

        unsafe {
            let mut h = std::ptr::null_mut();
            assert_eq!(XTaskQueueCreate(MODE_THREAD_POOL, MODE_THREAD_POOL, &mut h), S_OK);
            let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            assert!(!event.is_null());

            // Out-of-bounds port must be rejected with E_INVALIDARG instead of panicking in waiter_fired.
            let mut bad_tok = RegistrationToken { token: 0 };
            assert_eq!(XTaskQueueRegisterWaiter(h, 2, event, std::ptr::null_mut(), count_hit as usize, &mut bad_tok), E_INVALIDARG);
            assert_eq!(XTaskQueueRegisterWaiter(h, 99, event, std::ptr::null_mut(), count_hit as usize, &mut bad_tok), E_INVALIDARG);

            // Register a live waiter, terminate the queue, then signal the handle: callback must not run.
            let hits = AtomicU32::new(0);
            let mut tok = RegistrationToken { token: 0 };
            assert_eq!(
                XTaskQueueRegisterWaiter(h, PORT_WORK, event, &hits as *const AtomicU32 as *mut c_void, count_hit as usize, &mut tok),
                S_OK
            );
            assert_eq!(XTaskQueueTerminate(h, true, std::ptr::null_mut(), 0), S_OK);
            assert_ne!(SetEvent(event), 0);
            std::thread::sleep(Duration::from_millis(50));
            XTaskQueueUnregisterWaiter(h, tok);
            assert_eq!(hits.load(Ordering::SeqCst), 0, "waiter submitted work to a terminated queue");
            assert!(XTaskQueueIsEmpty(h, PORT_WORK));

            CloseHandle(event);
            XTaskQueueCloseHandle(h);
        }
    }
}
