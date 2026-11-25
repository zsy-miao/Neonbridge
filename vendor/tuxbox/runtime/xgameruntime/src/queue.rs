//! Manual ports are serviced by Dispatch. Other ports drive the same queue on
//! worker threads (or the submitting thread for an immediately runnable item).
use super::{ABORT, INVALID, OK, Object, POINTER};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::{Arc, Condvar, LazyLock, Mutex, MutexGuard};
use std::time::{Duration, Instant};

type Callback = unsafe extern "system" fn(*mut c_void, bool);
type Terminated = unsafe extern "system" fn(*mut c_void);
type Handle = usize;
const MANUAL: u32 = 0;
const SERIAL: u32 = 2;
const IMMEDIATE: u32 = 3;
const INFINITE: u32 = u32::MAX;
const OUT_OF_MEMORY: i32 = 0x8007000eu32 as i32;

#[derive(Clone, Copy)]
struct Job {
    callback: Callback,
    // An opaque guest context; only its callback interprets the address.
    context: usize,
    due: Instant,
}

#[derive(Default)]
struct Port {
    pending: VecDeque<Job>,
    running: usize,
}

enum Ending {
    Open,
    Draining(Option<(Terminated, usize)>),
    FinalCallback,
    Finished,
}

struct State {
    ports: [Port; 2],
    ending: Ending,
}

struct Queue {
    modes: [u32; 2],
    state: Mutex<State>,
    changed: Condvar,
}

struct Handles {
    last: usize,
    queues: HashMap<Handle, Arc<Queue>>,
}

static HANDLES: LazyLock<Mutex<Handles>> = LazyLock::new(|| {
    Mutex::new(Handles {
        last: 0,
        queues: HashMap::new(),
    })
});
static PROCESS_QUEUE: LazyLock<Arc<Queue>> = LazyLock::new(|| Queue::new([1, 1]));

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn lookup(handle: Handle) -> Option<Arc<Queue>> {
    lock(&HANDLES).queues.get(&handle).cloned()
}

unsafe fn publish(out: *mut Handle, queue: Arc<Queue>) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    let mut registry = lock(&HANDLES);
    let Some(id) = registry.last.checked_add(1) else {
        return OUT_OF_MEMORY;
    };
    registry.last = id;
    registry.queues.insert(id, queue);
    unsafe {
        out.write(id);
    }
    OK
}

enum Run {
    Job(Job, bool),
    Final(Terminated, usize),
}

impl Queue {
    fn new(modes: [u32; 2]) -> Arc<Self> {
        Arc::new(Self {
            modes,
            state: Mutex::new(State {
                ports: Default::default(),
                ending: Ending::Open,
            }),
            changed: Condvar::new(),
        })
    }

    // All pending work must drain before the completion port and its final
    // callback. The callback can use or close its own queue handle.
    fn next(&self, s: &mut State, port: usize) -> Option<Run> {
        let canceled = matches!(s.ending, Ending::Draining(_));
        let work = &s.ports[0];
        if canceled && port == 1 && (work.running != 0 || !work.pending.is_empty()) {
            return None;
        }
        let p = &mut s.ports[port];
        if self.modes[port] == SERIAL && p.running != 0 {
            return None;
        }
        let now = Instant::now();
        if let Some(i) = p.pending.iter().position(|job| canceled || job.due <= now) {
            let job = p.pending.remove(i)?;
            p.running += 1;
            return Some(Run::Job(job, canceled));
        }
        if port == 1
            && p.running == 0
            && let Ending::Draining(Some((cb, ctx))) = s.ending
        {
            s.ending = Ending::FinalCallback;
            return Some(Run::Final(cb, ctx));
        }
        None
    }

    fn settle(&self, s: &mut State) {
        if matches!(s.ending, Ending::Draining(None))
            && s.ports
                .iter()
                .all(|p| p.running == 0 && p.pending.is_empty())
        {
            s.ending = Ending::Finished;
        }
        self.changed.notify_all();
    }

    fn execute(&self, port: usize, run: Run) {
        // No queue lock is held while executing guest code. The dispatcher
        // owns an Arc, so callbacks may also release the last public handle.
        match run {
            Run::Job(job, canceled) => {
                unsafe {
                    (job.callback)(job.context as *mut c_void, canceled);
                }
                let mut s = lock(&self.state);
                s.ports[port].running -= 1;
                self.settle(&mut s);
            }
            Run::Final(cb, ctx) => {
                unsafe {
                    cb(ctx as *mut c_void);
                }
                let mut s = lock(&self.state);
                s.ending = Ending::Finished;
                self.changed.notify_all();
            }
        }
    }

    fn drive(&self, port: usize, timeout: Option<Instant>, automatic: bool) -> bool {
        let mut s = lock(&self.state);
        loop {
            self.settle(&mut s);
            if let Some(run) = self.next(&mut s, port) {
                drop(s);
                self.execute(port, run);
                return true;
            }
            if matches!(s.ending, Ending::Finished) {
                return false;
            }
            let p = &s.ports[port];
            // An automatic dispatcher only lives while its port has work
            // or a termination callback to service. Manual dispatch can wait
            // for a subsequent Submit or Terminate call.
            let waiting_final = port == 1 && matches!(s.ending, Ending::Draining(Some(_)));
            if automatic && p.pending.is_empty() && !waiting_final {
                return false;
            }
            let next = if matches!(s.ending, Ending::Open)
                && !(self.modes[port] == SERIAL && p.running != 0)
            {
                p.pending.iter().map(|j| j.due).min()
            } else {
                None
            };
            let deadline = match (next, timeout) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
            if let Some(deadline) = deadline {
                let now = Instant::now();
                if timeout.is_some_and(|end| now >= end) {
                    return false;
                }
                s = self
                    .changed
                    .wait_timeout(s, deadline.saturating_duration_since(now))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            } else {
                s = self.changed.wait(s).unwrap_or_else(|e| e.into_inner());
            }
        }
    }

    fn worker(self: &Arc<Self>, port: usize) -> Result<(), std::io::Error> {
        let queue = Arc::clone(self);
        std::thread::Builder::new()
            .spawn(move || {
                queue.drive(port, None, true);
            })
            .map(|_| ())
    }
}

pub(super) unsafe extern "system" fn create(
    _: &Object,
    work: u32,
    completion: u32,
    out: *mut Handle,
) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe {
        out.write(0);
    }
    if work > IMMEDIATE || completion > IMMEDIATE {
        return INVALID;
    }
    unsafe { publish(out, Queue::new([work, completion])) }
}
pub(super) unsafe extern "system" fn duplicate(
    _: &Object,
    handle: Handle,
    out: *mut Handle,
) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe {
        out.write(0);
    }
    let Some(q) = lookup(handle) else {
        return INVALID;
    };
    unsafe { publish(out, q) }
}
pub(super) unsafe extern "system" fn process_queue(_: &Object, out: *mut Handle) -> bool {
    unsafe { publish(out, Arc::clone(&PROCESS_QUEUE)) == OK }
}
pub(super) extern "system" fn close(_: &Object, handle: Handle) {
    lock(&HANDLES).queues.remove(&handle);
}
pub(super) extern "system" fn dispatch(
    _: &Object,
    handle: Handle,
    port: u32,
    timeout: u32,
) -> bool {
    let Some(q) = lookup(handle) else {
        return false;
    };
    if port > 1 {
        return false;
    }
    let deadline =
        (timeout != INFINITE).then(|| Instant::now() + Duration::from_millis(timeout.into()));
    q.drive(port as usize, deadline, false)
}
pub(super) extern "system" fn submit(
    this: &Object,
    handle: Handle,
    port: u32,
    context: *mut c_void,
    callback: Option<Callback>,
) -> i32 {
    submit_delayed(this, handle, port, 0, context, callback)
}
pub(super) extern "system" fn submit_delayed(
    _: &Object,
    handle: Handle,
    port: u32,
    delay: u32,
    context: *mut c_void,
    callback: Option<Callback>,
) -> i32 {
    let (Some(q), Some(callback)) = (lookup(handle), callback) else {
        return INVALID;
    };
    if port > 1 {
        return INVALID;
    }
    let port = port as usize;
    let mut s = lock(&q.state);
    if !matches!(s.ending, Ending::Open) {
        return ABORT;
    }
    let job = Job {
        context: context as usize,
        callback,
        due: Instant::now() + Duration::from_millis(delay.into()),
    };
    // Worker creation is fallible. Hold the lock through publication so that a
    // failed submit cannot be observed by another dispatcher.
    if q.modes[port] == IMMEDIATE && delay == 0 {
        s.ports[port].running += 1;
        drop(s);
        q.execute(port, Run::Job(job, false));
    } else {
        if q.modes[port] != MANUAL && q.worker(port).is_err() {
            return OUT_OF_MEMORY;
        }
        s.ports[port].pending.push_back(job);
        q.changed.notify_all();
    }
    OK
}
pub(super) extern "system" fn terminate(
    _: &Object,
    handle: Handle,
    wait: bool,
    context: *mut c_void,
    callback: Option<Terminated>,
) -> i32 {
    let Some(q) = lookup(handle) else {
        return INVALID;
    };
    let mut s = lock(&q.state);
    if !matches!(s.ending, Ending::Open) {
        return ABORT;
    }
    // A registered callback runs on the completion port. Manual completion
    // requires a dispatch from the guest, just as it does for a submitted job.
    if callback.is_some() && q.modes[1] != MANUAL && q.worker(1).is_err() {
        return OUT_OF_MEMORY;
    }
    s.ending = Ending::Draining(callback.map(|cb| (cb, context as usize)));
    q.settle(&mut s);
    if wait {
        drop(
            q.changed
                .wait_while(s, |s| !matches!(s.ending, Ending::Finished))
                .unwrap_or_else(|e| e.into_inner()),
        );
    }
    OK
}

/// Run `callback(context, false)` on the completion port of a queue handle,
/// or of the process queue for a null handle (XAsyncBlock semantics).
pub(super) unsafe fn submit_completion(handle: Handle, context: *mut c_void, callback: Callback) -> i32 {
    unsafe { submit_port(handle, 1, context, callback, Duration::ZERO) }
}

/// As `submit_completion`, due after `delay`.
pub(super) unsafe fn submit_completion_after(handle: Handle, context: *mut c_void, callback: Callback, delay: Duration) -> i32 {
    unsafe { submit_port(handle, 1, context, callback, delay) }
}

/// Queue `callback(context, false)` on port 0 (work) or 1 (completion).
pub(super) unsafe fn submit_port(handle: Handle, port: usize, context: *mut c_void, callback: Callback, delay: Duration) -> i32 {
    let queue = if handle == 0 { Some(Arc::clone(&PROCESS_QUEUE)) } else { lookup(handle) };
    let Some(q) = queue else { return INVALID };
    let mut s = lock(&q.state);
    if !matches!(s.ending, Ending::Open) {
        return ABORT;
    }
    let job = Job { context: context as usize, callback, due: Instant::now() + delay };
    // Same delivery rules as submit_delayed.
    if q.modes[port] == IMMEDIATE && delay.is_zero() {
        s.ports[port].running += 1;
        drop(s);
        q.execute(port, Run::Job(job, false));
    } else {
        if q.modes[port] != MANUAL && q.worker(port).is_err() {
            return OUT_OF_MEMORY;
        }
        s.ports[port].pending.push_back(job);
        q.changed.notify_all();
    }
    OK
}

// Port handles name one port of one queue. A composite queue built from two
// ports of the same queue is that queue; ports of different queues use the
// work port's queue for both (completions then run on its completion port).
static PORTS: LazyLock<Mutex<HashMap<usize, (Arc<Queue>, u32)>>> = LazyLock::new(Default::default);

/// XTaskQueueGetPort (threading slot 14).
pub(super) unsafe extern "system" fn get_port(_: &Object, handle: Handle, port: u32, out: *mut usize) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    let queue = if handle == 0 { Some(Arc::clone(&PROCESS_QUEUE)) } else { lookup(handle) };
    let (Some(q), true) = (queue, port <= 1) else { return INVALID };
    let mut ports = lock(&PORTS);
    let id = 0x5000_0000 + ports.len() * 2 + port as usize;
    ports.insert(id, (q, port));
    unsafe { out.write(id) };
    OK
}

/// XTaskQueueCreateComposite (threading slot 13).
pub(super) unsafe extern "system" fn create_composite(_: &Object, work: usize, completion: usize, out: *mut Handle) -> i32 {
    if out.is_null() {
        return POINTER;
    }
    unsafe { out.write(0) };
    let ports = lock(&PORTS);
    let (Some((work_queue, _)), Some((completion_queue, _))) = (ports.get(&work), ports.get(&completion)) else {
        return INVALID;
    };
    if !Arc::ptr_eq(work_queue, completion_queue) {
        super::report(format_args!("composite queue from ports of two queues uses the work queue for both"));
    }
    let queue = Arc::clone(work_queue);
    drop(ports);
    unsafe { publish(out, queue) }
}
