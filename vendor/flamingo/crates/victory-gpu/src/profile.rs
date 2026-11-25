//! Where the layer's threads spend their time: a sampling profiler
//! (`VICTORY_PROFILE=1`, diagnostics builds; off otherwise).
//!
//! The log's own counters say how long the phases of a frame take, not
//! what a thread does inside them: in the graphics driver, in a kernel
//! call, waiting for a lock. A thread of ours ([`run`]) looks, a thousand
//! times a second (`VICTORY_PROFILE_HZ`), at the threads that matter:
//!
//! * the submitting thread and the recording threads, which name
//!   themselves by what they call ([`seen`]: [`crate::host::Host`]'s
//!   submissions, the list calls);
//! * our workers, by the names we start them with (`victory-pipelines-N`,
//!   `victory-wide-N`, `victory-late-wait`) and the title's main thread;
//! * the busiest of the other threads (the title's own).
//!
//! A look ([`Sampler::capture`]) suspends the thread, reads its registers
//! and copies the used part of its stack with the kernel's own read, and
//! lets it go: between the two nothing of ours takes a lock, allocates or
//! reads memory that may be gone. The stack is walked afterwards, in the
//! copy ([`Sampler::walk`]), by each image's unwind data (the Windows
//! loader registered it for title images too), every read checked to stay
//! inside the copy. A thread that has used no processor cycles since the
//! last look is where it was: it is not suspended again.
//!
//! Names are found when the report is written, not when a sample is
//! taken ([`Symbols`]): our executable through its PDB (inlined calls
//! too), system and driver code by export or `module+offset`, title code
//! as `title+offset` of the function's start.
//!
//! The report is written every `VICTORY_PROFILE_EVERY` seconds (30) and at
//! exit to `VICTORY_PROFILE_OUT` (`%TEMP%\victory_profile_<pid>.txt`), the
//! stacks for a flame graph beside it (`.folded`). Per kind of thread:
//! time by module, by function (own and with what it calls), where it
//! waited and for which of our functions, the hottest lines of ours. Times
//! are milliseconds per presented frame ([`frame`]).

use std::cell::Cell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE, HMODULE, LocalFree,
};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CONTEXT, CONTEXT_CONTROL_AMD64, CONTEXT_INTEGER_AMD64, GetThreadContext, IMAGEHLP_LINEW64,
    IMAGEHLP_MODULEW64, ReadProcessMemory, RtlLookupFunctionEntry, RtlVirtualUnwind, SYMBOL_INFOW,
    UNW_FLAG_NHANDLER,
};
use windows_sys::Win32::System::LibraryLoader::{
    GetModuleFileNameW, GetModuleHandleW, GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32,
    LoadLibraryExA,
};
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc};
use windows_sys::Win32::System::ProcessStatus::{
    K32EnumProcessModules, K32GetModuleInformation, MODULEINFO,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, GetCurrentThreadId, GetThreadDescription, GetThreadId,
    GetThreadTimes, OpenThread, ResumeThread, SetThreadPriority, SuspendThread, THREAD_GET_CONTEXT,
    THREAD_PRIORITY_TIME_CRITICAL, THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION,
    THREAD_SUSPEND_RESUME, WaitForSingleObject,
};

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    /// The processor cycles a thread has used so far.
    fn QueryThreadCycleTime(thread: HANDLE, cycles: *mut u64) -> i32;
}

#[link(name = "ntdll", kind = "raw-dylib")]
unsafe extern "system" {
    /// The thread of `process` after `thread` (null: the first), opened
    /// with `access`; an NTSTATUS, not zero after the last.
    fn NtGetNextThread(
        process: HANDLE,
        thread: HANDLE,
        access: u32,
        attributes: u32,
        flags: u32,
        next: *mut HANDLE,
    ) -> i32;
    /// `(thread, class, out, size, written)`: an NTSTATUS.
    fn NtQueryInformationThread(
        thread: HANDLE,
        class: i32,
        information: *mut c_void,
        size: u32,
        written: *mut u32,
    ) -> i32;
}

// ---------------------------------------------------------------------
// What the layer's threads tell the profiler.
// ---------------------------------------------------------------------

/// 0: not asked yet, [`OFF`], [`ON`].
static STATE: AtomicU8 = AtomicU8::new(0);
const OFF: u8 = 1;
const ON: u8 = 2;

/// Frames presented since the profiler started.
static FRAMES: AtomicU64 = AtomicU64::new(0);

/// What a thread was seen doing.
#[derive(Clone, Copy)]
pub(crate) enum Seen {
    /// It called a submission ([`crate::host::Host`]).
    Submit = 0,
    /// It made a list call.
    Record = 1,
}

/// A thread that called [`seen`], and how often for each [`Seen`]. A line
/// of the cache each: the recording threads count here at every list call.
#[repr(align(64))]
struct Known {
    thread: AtomicU32,
    seen: [AtomicU64; 2],
}

const KNOWN: usize = 64;
static KNOWN_THREADS: [Known; KNOWN] = [const {
    Known {
        thread: AtomicU32::new(0),
        seen: [const { AtomicU64::new(0) }; 2],
    }
}; KNOWN];

/// Submissions after which a thread is a submitting thread, and list calls
/// after which it is a recording thread: a thread that reads a texture
/// back once while the world loads is neither.
const SUBMITS: u64 = 8;
const LIST_CALLS: u64 = 64;

thread_local! {
    /// This thread's place in [`KNOWN_THREADS`], plus one; 0 before it has
    /// one, 255 if there was none left.
    static MINE: Cell<u8> = const { Cell::new(0) };
}

/// Whether the profiler runs; the first call reads `VICTORY_PROFILE` and
/// starts it.
#[inline]
fn on() -> bool {
    match STATE.load(Relaxed) {
        ON => true,
        OFF => false,
        _ => decide(),
    }
}

#[cold]
fn decide() -> bool {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let started = Config::from_env().is_some_and(start);
        let state = if started { ON } else { OFF };
        STATE.store(state, Relaxed);
    });
    STATE.load(Relaxed) == ON
}

/// The calling thread was seen doing `what`: it is followed once it has
/// done so often enough. Costs a load when the profiler is off.
#[inline]
pub(crate) fn seen(what: Seen) {
    if !on() {
        return;
    }
    let mine = MINE.with(Cell::get);
    let known = match mine {
        0 => match claim() {
            Some(known) => known,
            None => return,
        },
        255 => return,
        at => &KNOWN_THREADS[usize::from(at) - 1],
    };
    known.seen[what as usize].fetch_add(1, Relaxed);
}

/// A place in [`KNOWN_THREADS`] for the calling thread.
#[cold]
fn claim() -> Option<&'static Known> {
    // SAFETY: a plain query.
    let id = unsafe { GetCurrentThreadId() };
    for (at, known) in KNOWN_THREADS.iter().enumerate() {
        if known
            .thread
            .compare_exchange(0, id, Relaxed, Relaxed)
            .is_ok()
        {
            known.seen[0].store(0, Relaxed);
            known.seen[1].store(0, Relaxed);
            MINE.with(|m| m.set(at as u8 + 1));
            return Some(known);
        }
    }
    MINE.with(|m| m.set(255));
    None
}

/// A frame was presented.
#[inline]
pub(crate) fn frame() {
    if on() {
        FRAMES.fetch_add(1, Relaxed);
    }
}

// ---------------------------------------------------------------------
// The switches.
// ---------------------------------------------------------------------

/// The kinds of thread followed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Role {
    Submit,
    Record,
    Late,
    Wide,
    Pipelines,
    TitleMain,
    Busy,
}

impl Role {
    const ALL: [Role; 7] = [
        Role::Submit,
        Role::Record,
        Role::Late,
        Role::Wide,
        Role::Pipelines,
        Role::TitleMain,
        Role::Busy,
    ];

    /// As `VICTORY_PROFILE_THREADS` and the stacks file name it.
    fn name(self) -> &'static str {
        match self {
            Role::Submit => "submit",
            Role::Record => "record",
            Role::Late => "late-wait",
            Role::Wide => "wide",
            Role::Pipelines => "pipelines",
            Role::TitleMain => "title-main",
            Role::Busy => "busy",
        }
    }

    fn says(self) -> &'static str {
        match self {
            Role::Submit => "SUBMITTING (threads that send lists to the card: Host::submit)",
            Role::Record => "RECORDING (threads that make list calls and do not submit)",
            Role::Late => "LATE WAIT (victory-late-wait: settles submissions sent ahead)",
            Role::Wide => "WIDE WORKERS (victory-wide-N: copies and compares of title memory)",
            Role::Pipelines => "PIPELINE WORKERS (victory-pipelines-N)",
            Role::TitleMain => "TITLE MAIN (title-main)",
            Role::Busy => "OTHER BUSY THREADS (the busiest threads with none of the roles above)",
        }
    }

    /// The role of a thread we named.
    fn of_name(name: &str) -> Option<Role> {
        if name.starts_with("victory-pipelines-") {
            Some(Role::Pipelines)
        } else if name.starts_with("victory-wide-") {
            Some(Role::Wide)
        } else if name == "victory-late-wait" {
            Some(Role::Late)
        } else if name == "title-main" {
            Some(Role::TitleMain)
        } else {
            None
        }
    }
}

/// Frames kept of a stack at most (`VICTORY_PROFILE_DEPTH` asks for fewer).
const MAX_DEPTH: usize = 96;

#[derive(Clone)]
struct Config {
    /// Looks per second.
    hz: u32,
    /// The report.
    out: PathBuf,
    /// Seconds between reports.
    every: f64,
    /// Seconds before the first look, and after how many more the last.
    after: f64,
    length: Option<f64>,
    /// Frames kept of a stack, from the innermost.
    depth: usize,
    /// Bytes copied of a stack at most.
    stack: usize,
    /// Threads followed at most, and of those the busy ones with no role.
    most: usize,
    busy: usize,
    /// The roles followed, by `Role as usize`.
    roles: [bool; 7],
    /// Whether calls the compiler inlined are shown as frames of their own.
    inline: bool,
    /// Whether a thread that used no cycles since the last look is taken
    /// to be where it was.
    reuse: bool,
    /// The share of a thread's time it may stand still for our looks
    /// (0: no limit): a thread whose looks cost more is looked at less
    /// often.
    budget: f64,
    /// Microseconds a tick is put off by at most, a different time each
    /// tick ([`Pace::wait`]).
    jitter: u64,
}

fn env_number(name: &str) -> Option<f64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

impl Config {
    fn new(hz: u32, out: PathBuf) -> Config {
        Config {
            hz,
            out,
            every: 30.0,
            after: 0.0,
            length: None,
            depth: 48,
            stack: 128 << 10,
            most: 24,
            busy: 6,
            roles: [true; 7],
            inline: true,
            reuse: true,
            budget: 0.03,
            jitter: 250,
        }
    }

    /// `VICTORY_PROFILE=1` and the switches beside it; `None` without it.
    fn from_env() -> Option<Config> {
        if !std::env::var_os("VICTORY_PROFILE").is_some_and(|v| v == "1") {
            return None;
        }
        let out = std::env::var_os("VICTORY_PROFILE_OUT").map_or_else(
            || std::env::temp_dir().join(format!("victory_profile_{}.txt", std::process::id())),
            PathBuf::from,
        );
        let hz = env_number("VICTORY_PROFILE_HZ").map_or(1000.0, |v| v.clamp(1.0, 4000.0));
        let mut config = Config::new(hz as u32, out);
        let seconds = |name: &str, least: f64| env_number(name).map(|v| v.max(least));
        config.every = seconds("VICTORY_PROFILE_EVERY", 1.0).unwrap_or(config.every);
        config.after = seconds("VICTORY_PROFILE_AFTER", 0.0).unwrap_or(0.0);
        config.length = seconds("VICTORY_PROFILE_FOR", 1.0);
        let count = |name: &str, usual: usize, most: usize| {
            env_number(name).map_or(usual, |v| (v.max(0.0) as usize).min(most))
        };
        config.depth = count("VICTORY_PROFILE_DEPTH", config.depth, MAX_DEPTH).max(4);
        config.stack = count("VICTORY_PROFILE_STACK_KB", 128, 1024).max(4) << 10;
        config.most = count("VICTORY_PROFILE_MAX_THREADS", config.most, 64).max(1);
        config.busy = count("VICTORY_PROFILE_BUSY", config.busy, 32);
        config.inline = std::env::var_os("VICTORY_PROFILE_INLINE").is_none_or(|v| v != "0");
        config.reuse = std::env::var_os("VICTORY_PROFILE_REUSE").is_none_or(|v| v != "0");
        config.budget = env_number("VICTORY_PROFILE_BUDGET")
            .map_or(config.budget, |v| v.clamp(0.0, 100.0) / 100.0);
        config.jitter = count("VICTORY_PROFILE_JITTER_US", 250, 2000) as u64;
        if let Ok(list) = std::env::var("VICTORY_PROFILE_THREADS") {
            let named: Vec<&str> = list.split(',').map(str::trim).collect();
            for role in Role::ALL {
                config.roles[role as usize] = named.contains(&role.name());
            }
        }
        Some(config)
    }
}

// ---------------------------------------------------------------------
// Samples: a tree of calls per thread.
// ---------------------------------------------------------------------

/// A hash for small keys of ours (addresses and indices), not for
/// anything a stranger chooses.
#[derive(Default, Clone, Copy)]
struct Fast(u64);

impl Hasher for Fast {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(u64::from(b));
        }
    }
    fn write_u8(&mut self, v: u8) {
        self.write_u64(u64::from(v));
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
    fn write_usize(&mut self, v: usize) {
        self.write_u64(v as u64);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<Fast>>;

/// A frame as called from its parent's: `address` is the instruction the
/// thread was at (an innermost frame) or a byte inside the call that has
/// not returned (the others), so both lie in the function they belong to.
struct Node {
    parent: u32,
    address: u64,
    /// Of the samples that ended here: microseconds of wall clock they
    /// stand for, microseconds of those the thread was on a processor (by
    /// its cycle counter), and how many they were.
    wall: u64,
    cpu: u64,
    hits: u32,
}

/// The address of the node that stands for the frames not kept, between
/// the thread's first frame and the outermost one kept.
const CUT: u64 = 1;
/// The address of the node samples go to once the trees are full.
const FULL: u64 = 2;

/// The calls of one thread, each sample at the node of its innermost frame.
struct Tree {
    nodes: Vec<Node>,
    children: FastMap<(u32, u64), u32>,
}

impl Tree {
    fn new() -> Tree {
        Tree {
            nodes: vec![Node {
                parent: 0,
                address: 0,
                wall: 0,
                cpu: 0,
                hits: 0,
            }],
            children: FastMap::default(),
        }
    }

    fn child(&mut self, parent: u32, address: u64) -> u32 {
        *self.children.entry((parent, address)).or_insert_with(|| {
            self.nodes.push(Node {
                parent,
                address,
                wall: 0,
                cpu: 0,
                hits: 0,
            });
            self.nodes.len() as u32 - 1
        })
    }

    /// The node of a stack, `frames` from the innermost; `whole` if they
    /// reach the thread's first frame. With `room` false no node is made:
    /// what is new goes to the [`FULL`] node.
    fn place(&mut self, frames: &[u64], whole: bool, room: bool) -> u32 {
        let mut at = 0;
        if !whole {
            at = self.child(0, CUT);
        }
        for &address in frames.iter().rev() {
            at = match self.children.get(&(at, address)) {
                Some(&next) => next,
                None if room => self.child(at, address),
                None => return self.child(0, FULL),
            };
        }
        at
    }
}

/// The trees' nodes together, at most (56 bytes each with their index).
const MOST_NODES: usize = 1_500_000;

// ---------------------------------------------------------------------
// Looking at a thread.
// ---------------------------------------------------------------------

/// `GetThreadContext` and `RtlVirtualUnwind` want the context 16-byte
/// aligned, and windows-sys declares it without that alignment.
#[repr(C, align(16))]
struct Aligned(CONTEXT);

/// Zeroed bytes kept after the room for a stack's copy: what a frame's
/// unwinding reads past the copied part (a frame cut by the limit, a
/// return address that was none) is read from here.
const SLACK: usize = 2 << 20;
/// Copied of a stack that is not the thread's own (a fiber's) at most.
const FOREIGN: usize = 32 << 10;
/// What a thread needs open for [`Sampler::capture`], and to be asked
/// whether it has ended (`SYNCHRONIZE`).
const ACCESS: u32 =
    THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION | 0x0010_0000;
/// Microseconds of processor time a waiting thread is taken to use at
/// most for being suspended and resumed (the kernel wakes it for both):
/// about 6 were measured. A thread that used no more since a look that
/// suspended it has not left its wait.
const SETTLING_US: f64 = 30.0;
/// The share of a suspension, as we time it (from asking for it to having
/// let go), that the thread really stands still: the rest is the kernel
/// reaching the thread and returning to us. Measured with threads whose
/// work is counted, eight of them always running: about 20 us of work
/// lost a look (1.8 to 2.1 % at 960 looks a second) for 25 to 28 us held.
const STOOD: f64 = 0.8;
/// A suspension counted as no longer than this in a thread's running
/// average (nanoseconds): the long ones are a thread that was not on a
/// processor when we asked, which costs it nothing.
const COST_MOST: f64 = 250_000.0;

struct Thread {
    id: u32,
    handle: HANDLE,
    name: String,
    /// Where it started: what a thread without a name is told by.
    start: u64,
    role: Role,
    /// Its stack's reservation: a stack pointer inside is the thread's own.
    low: usize,
    high: usize,
    /// Its cycle counter at the last look, the node of that look, and
    /// whether the thread was suspended for it.
    cycles: u64,
    last: Option<u32>,
    stood: bool,
    /// It is looked at every `every` ticks ([`Config::budget`]), next in
    /// `turn` ticks; `owed` is the time since its last look
    /// (microseconds), `cost` what a suspension of it takes (nanoseconds,
    /// a running average).
    every: u32,
    turn: u32,
    owed: u64,
    cost: f64,
    /// Bytes of its stack copied at a look; 0: all of it. Less once a
    /// walk has shown how many the kept frames take.
    want: usize,
    tree: Tree,
    /// Still running, as of the last [`Sampler::rescan`].
    alive: bool,
    round: u64,
    /// Looks: all, those answered without suspending it, those that
    /// failed, and those whose stack did not reach its first frame.
    looks: u64,
    reused: u64,
    missed: u64,
    cut: u64,
}

/// A thread with no role, watched for how busy it is.
struct Other {
    handle: HANDLE,
    cycles: u64,
    name: String,
    round: u64,
}

struct Sampler {
    config: Config,
    me: u32,
    threads: Vec<Thread>,
    others: FastMap<u32, Other>,
    round: u64,
    scanned: Instant,
    context: Box<Aligned>,
    /// The copy of a stack: `config.stack + 16` bytes, then [`SLACK`].
    copy: *mut u8,
    room: usize,
    /// How far the copy has bytes that are not zero.
    dirty: usize,
    frames: [u64; MAX_DEPTH],
    /// Cycles of the processor's counter per microsecond.
    cycles_per_us: f64,
    /// The cycles a waiting thread uses at most to be suspended and let
    /// go again ([`SETTLING_US`]).
    settling: u64,
    clock: Instant,
    /// The count of frames presented ([`FRAMES`]; a test's own).
    presents: &'static AtomicU64,
    /// The last tick, on `clock`, and the frame count then.
    last: Duration,
    frames_then: u64,
    /// Since the start: ticks, late ones, microseconds they stand for,
    /// frames presented meanwhile.
    ticks: u64,
    late: u64,
    wall: u64,
    presented: u64,
    /// Suspensions: how many, their time together and the longest
    /// (nanoseconds), and how many by length ([`held_class`]).
    held: u64,
    held_ns: u64,
    held_longest: u64,
    held_by: [u64; 256],
    nodes: usize,
    symbols: Option<Symbols>,
}

impl Sampler {
    fn new(config: Config) -> Sampler {
        let room = config.stack + 16 + SLACK;
        // SAFETY: fresh zeroed memory of ours, kept for the process's life.
        let copy = unsafe { VirtualAlloc(null(), room, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!copy.is_null(), "no memory for the profiler's stack copy");
        let clock = Instant::now();
        let cycles_per_us = cycles_per_us();
        // SAFETY: a zeroed CONTEXT is valid.
        let context = Box::new(Aligned(unsafe { std::mem::zeroed() }));
        Sampler {
            config,
            // SAFETY: a plain query.
            me: unsafe { GetCurrentThreadId() },
            threads: Vec::new(),
            others: FastMap::default(),
            round: 0,
            scanned: clock,
            context,
            copy: copy.cast(),
            room,
            dirty: 0,
            frames: [0; MAX_DEPTH],
            cycles_per_us,
            settling: (SETTLING_US * cycles_per_us) as u64,
            clock,
            presents: &FRAMES,
            last: clock.elapsed(),
            frames_then: FRAMES.load(Relaxed),
            ticks: 0,
            late: 0,
            wall: 0,
            presented: 0,
            held: 0,
            held_ns: 0,
            held_longest: 0,
            held_by: [0; 256],
            nodes: 0,
            symbols: None,
        }
    }

    /// The time under which `share` of the suspensions stayed, in
    /// microseconds (to a quarter of a power of two).
    fn held_within(&self, share: f64) -> f64 {
        let mut left = (self.held as f64 * share) as u64;
        for (class, &count) in self.held_by.iter().enumerate() {
            if count > left {
                // The class's upper end: its lower end as [`held_class`]
                // gives it, plus its width.
                let (bits, quarter) = (class / 4, class as u64 % 4);
                let under = if bits < 2 {
                    class as u64 + 1
                } else {
                    (5 + quarter) << (bits - 2)
                };
                return under as f64 / 1000.0;
            }
            left -= count;
        }
        self.held_longest as f64 / 1000.0
    }

    /// Time that passed while the sampler did something else (a report) is
    /// nobody's: the next tick counts from now.
    fn resync(&mut self) {
        self.last = self.clock.elapsed();
        self.frames_then = self.presents.load(Relaxed);
    }

    /// One look at every followed thread.
    fn tick(&mut self) {
        let now = self.clock.elapsed();
        let passed = now.saturating_sub(self.last).as_micros() as u64;
        self.last = now;
        let period = 1_000_000 / u64::from(self.config.hz.max(1));
        // A stall of ours is not the threads' time.
        let weight = passed.min(20 * period);
        self.late += u64::from(passed > 2 * period);
        let frames = self.presents.load(Relaxed);
        self.presented += frames.saturating_sub(self.frames_then);
        self.frames_then = frames;
        self.wall += weight;
        self.ticks += 1;
        for at in 0..self.threads.len() {
            let thread = &mut self.threads[at];
            if !thread.alive {
                continue;
            }
            // A thread looked at every few ticks: its look stands for all
            // the time since its last one.
            thread.owed += weight;
            thread.turn = thread.turn.saturating_sub(1);
            if thread.turn == 0 {
                thread.turn = thread.every;
                let weight = std::mem::take(&mut thread.owed);
                self.look(at, weight);
            }
        }
    }

    fn look(&mut self, at: usize, weight: u64) {
        let reuse = self.config.reuse;
        let thread = &mut self.threads[at];
        thread.looks += 1;
        let mut cycles = 0;
        // SAFETY: a thread handle of ours and a valid out pointer.
        if unsafe { QueryThreadCycleTime(thread.handle, &mut cycles) } == 0 {
            thread.missed += 1;
            return;
        }
        let ran = cycles.saturating_sub(thread.cycles);
        thread.cycles = cycles;
        // It has not run since the last look: it is where it was. A
        // thread that was suspended for that look ran to be suspended and
        // let go, which is not running its own code.
        let still = ran == 0 || thread.stood && ran <= self.settling;
        let last = thread.last;
        let node = match last {
            Some(node) if still && reuse => {
                thread.reused += 1;
                thread.stood = false;
                node
            }
            _ => {
                let Some((got, held)) = self.capture(at) else {
                    self.threads[at].missed += 1;
                    self.threads[at].last = None;
                    return;
                };
                let (frames, whole, used) = self.walk(at, got);
                let room = self.nodes < MOST_NODES;
                let (hz, budget) = (f64::from(self.config.hz), self.config.budget);
                let deep = frames >= self.config.depth.min(MAX_DEPTH);
                let thread = &mut self.threads[at];
                thread.stood = true;
                // The kept frames took `used` bytes: that and a quarter more
                // next time. A walk that ran out of copy first: all again.
                thread.want = if deep { used + used / 4 + 1024 } else { 0 };
                // What a look costs the thread: about [`STOOD`] of the time we
                // hold it (the rest is the kernel reaching it). Over the
                // budget, it is looked at less often.
                let held = (held as f64).min(COST_MOST);
                thread.cost = if thread.cost == 0.0 {
                    held
                } else {
                    thread.cost + (held - thread.cost) / 16.0
                };
                thread.every = if budget > 0.0 {
                    (STOOD * thread.cost * hz / 1e9 / budget)
                        .ceil()
                        .clamp(1.0, 32.0) as u32
                } else {
                    1
                };
                let before = thread.tree.nodes.len();
                let node = thread.tree.place(&self.frames[..frames], whole, room);
                self.nodes += thread.tree.nodes.len() - before;
                thread.cut += u64::from(!whole);
                node
            }
        };
        let cpu = ((ran as f64 / self.cycles_per_us) as u64).min(weight);
        let thread = &mut self.threads[at];
        thread.last = Some(node);
        let node = &mut thread.tree.nodes[node as usize];
        node.wall += weight;
        node.cpu += cpu;
        node.hits += 1;
    }

    /// Suspends the thread, reads its registers into `self.context` and
    /// copies its stack from the stack pointer up into `self.copy`, and
    /// resumes it; the bytes copied, or `None` if it could not be looked
    /// at (it ended).
    ///
    /// While the thread stands still it may hold any lock of the process
    /// (the heap's, the loader's, the unwinder's tables'): nothing here
    /// between the suspension and the resumption takes one or allocates.
    /// The stack is read by the kernel (`ReadProcessMemory`), which fails
    /// where a plain read would fault (a thread that is ending).
    fn capture(&mut self, at: usize) -> Option<(usize, u64)> {
        let thread = &self.threads[at];
        let (handle, low, high) = (thread.handle, thread.low, thread.high);
        let context = &mut self.context.0;
        context.ContextFlags = CONTEXT_CONTROL_AMD64 | CONTEXT_INTEGER_AMD64;
        let limit = match thread.want {
            0 => self.config.stack,
            want => want.min(self.config.stack),
        };
        let began = Instant::now();
        // SAFETY: a thread of this process that is not this one; the
        // context is aligned and ours; the copy has `limit + 16` bytes.
        let (read, offset, got) = unsafe {
            if SuspendThread(handle) == u32::MAX {
                return None;
            }
            let read = GetThreadContext(handle, context) != 0;
            let (mut offset, mut got) = (0, 0);
            if read {
                let rsp = context.Rsp as usize;
                let want = if rsp >= low && rsp < high {
                    (high - rsp).min(limit)
                } else {
                    limit.min(FOREIGN)
                };
                // The copy keeps the stack's alignment: unwinding loads
                // saved vector registers with aligned moves.
                offset = rsp & 15;
                ReadProcessMemory(
                    GetCurrentProcess(),
                    rsp as *const c_void,
                    self.copy.add(offset).cast(),
                    want,
                    &mut got,
                );
                got = got.min(want);
            }
            ResumeThread(handle);
            (read, offset, got)
        };
        let held = began.elapsed().as_nanos() as u64;
        self.held += 1;
        self.held_ns += held;
        self.held_longest = self.held_longest.max(held);
        self.held_by[held_class(held).0] += 1;
        if !read {
            return None;
        }
        // What an earlier, longer copy left behind it.
        let end = offset + got;
        if end < self.dirty {
            // SAFETY: inside the copy.
            unsafe { self.copy.add(end).write_bytes(0, self.dirty - end) };
        }
        self.dirty = end;
        Some((got, held))
    }

    /// Walks the stack copied by [`Sampler::capture`] (`got` bytes from
    /// the stack pointer in `self.context`) into `self.frames`, from the
    /// innermost: the count, whether the thread's first frame was
    /// reached, and the bytes of the copy the frames took.
    ///
    /// The thread runs again, so the walk reads the copy: the stack
    /// pointer and every saved register that points into the copied part
    /// of the stack are moved into the copy before each step (frame
    /// pointers are such registers), and a step is taken only if all it
    /// can read lies in the copy or the zeroes after it ([`frame_fits`]).
    fn walk(&mut self, at: usize, got: usize) -> (usize, bool, usize) {
        let (low, high) = (self.threads[at].low, self.threads[at].high);
        let depth = self.config.depth.min(MAX_DEPTH);
        let context = &mut self.context.0;
        let lo = context.Rsp as usize;
        let start = self.copy as usize + (lo & 15);
        let end = start + got;
        let limit = self.copy as usize + self.room;
        let top = if lo >= low && lo < high {
            high
        } else {
            lo + got
        };
        let mut clipped = false;
        self.frames[0] = context.Rip;
        let mut frames = 1;
        let (mut blind, mut last) = (0, 0);
        while frames < depth {
            // Into the copy: what points into the stack as it was.
            for register in [
                &mut context.Rsp,
                &mut context.Rbx,
                &mut context.Rbp,
                &mut context.Rsi,
                &mut context.Rdi,
                &mut context.R12,
                &mut context.R13,
                &mut context.R14,
                &mut context.R15,
            ] {
                let value = *register as usize;
                if value >= lo && value < top {
                    if value - lo < got {
                        *register = (start + (value - lo)) as u64;
                    } else {
                        // Above what was copied: reads there find zeroes.
                        *register = end as u64;
                        clipped = true;
                    }
                }
            }
            let rsp = context.Rsp as usize;
            // Out of the copy, or not upwards: the walk ends.
            if rsp < start || rsp + 8 > end || rsp <= last {
                break;
            }
            last = rsp;
            let mut image = 0;
            // SAFETY: looks up the unwind data covering `Rip`, if any.
            let entry = unsafe { RtlLookupFunctionEntry(context.Rip, &mut image, null_mut()) };
            if entry.is_null() {
                // Code without unwind data is a leaf function (the
                // system's call stubs are): its return address is at
                // `rsp`. Twice in a row it is no stack of calls.
                blind += 1;
                if blind > 2 {
                    break;
                }
                // SAFETY: `rsp..rsp + 8` is inside the copy.
                context.Rip = unsafe { (rsp as *const u64).read_unaligned() };
                context.Rsp += 8;
            } else {
                blind = 0;
                // SAFETY: `entry` is `image`'s, as just looked up.
                if !unsafe { frame_fits(image, entry.cast(), context, start, end, limit) } {
                    break;
                }
                let (mut data, mut frame) = (null_mut(), 0);
                // SAFETY: `entry` belongs to `image`; the context is our
                // aligned copy, and what the step reads is in our copy of
                // the stack (checked above).
                unsafe {
                    RtlVirtualUnwind(
                        UNW_FLAG_NHANDLER,
                        image,
                        context.Rip,
                        entry,
                        context,
                        &mut data,
                        &mut frame,
                        null_mut(),
                    )
                };
            }
            if context.Rip == 0 {
                // The first frame's caller: none. (Zeroes read above a
                // clipped copy look the same.)
                return (frames, !clipped, last + 8 - start);
            }
            self.frames[frames] = context.Rip - 1;
            frames += 1;
        }
        (frames, false, (last + 8).saturating_sub(start))
    }

    /// Follows the thread `id` as `role`; `handle`, if not null, is open
    /// with [`ACCESS`] and becomes ours.
    fn follow(&mut self, id: u32, role: Role, handle: HANDLE, name: Option<String>) {
        if let Some(thread) = self.threads.iter_mut().find(|t| t.alive && t.id == id) {
            // Seen doing more than it was followed for.
            if (role as usize) < thread.role as usize {
                thread.role = role;
            }
            thread.round = self.round;
            if !handle.is_null() {
                // SAFETY: ours, as the caller says.
                unsafe { CloseHandle(handle) };
            }
            return;
        }
        let handle = if handle.is_null() {
            // SAFETY: opens a thread of this process by its id.
            unsafe { OpenThread(ACCESS, 0, id) }
        } else {
            handle
        };
        if handle.is_null() {
            return;
        }
        if self.threads.iter().filter(|t| t.alive).count() >= self.config.most {
            // SAFETY: opened above or given to us.
            unsafe { CloseHandle(handle) };
            return;
        }
        let (low, high) = stack_of(handle);
        let mut cycles = 0;
        // SAFETY: a thread handle of ours and a valid out pointer.
        unsafe { QueryThreadCycleTime(handle, &mut cycles) };
        self.threads.push(Thread {
            id,
            handle,
            name: name.unwrap_or_else(|| thread_name(handle)),
            start: start_of(handle),
            role,
            low,
            high,
            cycles,
            last: None,
            stood: false,
            every: 1,
            turn: 1,
            owed: 0,
            cost: 0.0,
            want: 0,
            tree: Tree::new(),
            alive: true,
            round: self.round,
            looks: 0,
            reused: 0,
            missed: 0,
            cut: 0,
        });
    }

    /// Finds the threads to follow: those the hooks have seen, those with
    /// a name of ours, and the busiest of the rest; and notes the ones
    /// that ended.
    fn rescan(&mut self) {
        let passed = self.scanned.elapsed();
        self.scanned = Instant::now();
        self.round += 1;
        let wanted = self.config.roles;
        for known in &KNOWN_THREADS {
            let id = known.thread.load(Relaxed);
            if id == 0 || id == self.me {
                continue;
            }
            let role = if known.seen[Seen::Submit as usize].load(Relaxed) >= SUBMITS {
                Role::Submit
            } else if known.seen[Seen::Record as usize].load(Relaxed) >= LIST_CALLS {
                Role::Record
            } else {
                continue;
            };
            if wanted[role as usize] {
                self.follow(id, role, null_mut(), None);
            }
        }
        // Every thread of the process: (cycles since the last scan, id) of
        // those with no role.
        let mut busy: Vec<(u64, u32)> = Vec::new();
        let mut previous: HANDLE = null_mut();
        loop {
            let mut next = null_mut();
            // SAFETY: our own process; `previous` is null or a thread
            // handle the last pass gave us, closed once passed on.
            let status = unsafe {
                let status = NtGetNextThread(
                    GetCurrentProcess(),
                    previous,
                    THREAD_QUERY_LIMITED_INFORMATION,
                    0,
                    0,
                    &mut next,
                );
                if !previous.is_null() {
                    CloseHandle(previous);
                }
                status
            };
            if status != 0 || next.is_null() {
                break;
            }
            previous = next;
            // SAFETY: a thread handle just opened.
            let id = unsafe { GetThreadId(next) };
            if id == 0 || id == self.me {
                continue;
            }
            if let Some(thread) = self.threads.iter_mut().find(|t| t.alive && t.id == id) {
                thread.round = self.round;
                continue;
            }
            let round = self.round;
            let other = match self.others.entry(id) {
                std::collections::hash_map::Entry::Occupied(o) => o.into_mut(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    // SAFETY: opens a thread of this process by its id.
                    let handle = unsafe { OpenThread(ACCESS, 0, id) };
                    if handle.is_null() {
                        continue;
                    }
                    let mut cycles = 0;
                    // SAFETY: a thread handle of ours, a valid out pointer.
                    unsafe { QueryThreadCycleTime(handle, &mut cycles) };
                    v.insert(Other {
                        handle,
                        cycles,
                        name: String::new(),
                        round,
                    })
                }
            };
            other.round = round;
            if other.name.is_empty() {
                other.name = thread_name(other.handle);
            }
            if other.name == "victory-profile" {
                continue;
            }
            // SAFETY: a thread handle of ours, opened with `SYNCHRONIZE`.
            if unsafe { WaitForSingleObject(other.handle, 0) } == 0 {
                continue;
            }
            if let Some(role) = Role::of_name(&other.name) {
                if wanted[role as usize]
                    && let Some(other) = self.others.remove(&id)
                {
                    self.follow(id, role, other.handle, Some(other.name));
                }
                continue;
            }
            let mut cycles = 0;
            // SAFETY: a thread handle of ours and a valid out pointer.
            unsafe { QueryThreadCycleTime(other.handle, &mut cycles) };
            busy.push((cycles.saturating_sub(other.cycles), id));
            other.cycles = cycles;
        }
        // Threads that ended: no longer looked at; what was sampled stays.
        // (A thread we hold a handle to is still listed after its end.)
        let round = self.round;
        // SAFETY: a thread handle of ours, opened with `SYNCHRONIZE`.
        let ended = |handle: HANDLE| unsafe { WaitForSingleObject(handle, 0) } == 0;
        for thread in self
            .threads
            .iter_mut()
            .filter(|t| t.alive && (t.round != round || ended(t.handle)))
        {
            thread.alive = false;
            // SAFETY: ours.
            unsafe { CloseHandle(thread.handle) };
            thread.handle = null_mut();
            for known in &KNOWN_THREADS {
                let _ = known
                    .thread
                    .compare_exchange(thread.id, 0, Relaxed, Relaxed);
            }
        }
        self.others.retain(|_, other| {
            let gone = other.round != round || ended(other.handle);
            if gone {
                // SAFETY: ours.
                unsafe { CloseHandle(other.handle) };
            }
            !gone
        });
        // The busiest of the rest: a quarter of a processor or more.
        if wanted[Role::Busy as usize] {
            let least = (0.25 * passed.as_micros() as f64 * self.cycles_per_us) as u64;
            busy.sort_unstable_by(|a, b| b.cmp(a));
            for (ran, id) in busy {
                let followed = self
                    .threads
                    .iter()
                    .filter(|t| t.alive && t.role == Role::Busy)
                    .count();
                if ran < least.max(1) || followed >= self.config.busy {
                    break;
                }
                if let Some(other) = self.others.remove(&id) {
                    self.follow(id, Role::Busy, other.handle, Some(other.name));
                }
            }
        }
    }
}

/// Whether unwinding one frame of the function `entry` of the image at
/// `image` from `context` reads only our copy of a stack (`start..end`)
/// and the zeroes after it (to `limit`): the stack pointer, and the frame
/// pointer if the function has set one up, must lie in the copy, and what
/// the function's unwind codes take off the stack must fit in the zeroes.
///
/// `RtlVirtualUnwind` trusts the registers it is given. Ours come from a
/// thread stopped anywhere, and a return address may be none (code
/// without unwind data in the middle of a stack): a frame pointer that
/// points nowhere would be read through.
///
/// # Safety
/// `entry` is the function table entry of `image` for `context.Rip`.
unsafe fn frame_fits(
    image: u64,
    entry: *const [u32; 3],
    context: &CONTEXT,
    start: usize,
    end: usize,
    limit: usize,
) -> bool {
    // SAFETY: the caller's entry, and the unwind data it names in its
    // loaded image: a header of four bytes, `count` two-byte codes, and
    // after them (made even) the entry of the function this one is a
    // part of, if the header says so.
    unsafe {
        let [begin, finish, mut data] = entry.read_unaligned();
        let at = context
            .Rip
            .wrapping_sub(image)
            .wrapping_sub(u64::from(begin));
        // The frame register the function itself names (its first part).
        let mut pointer = 0;
        // Bytes above the frame's base that unwinding may read: the
        // return address, a machine frame's tail.
        let mut reach = 64usize;
        // Bases other than the stack pointer: frame pointers set up.
        let mut bases = [0usize; 4];
        let mut based = 0;
        for part in 0..8 {
            if data & 1 != 0 {
                return false;
            }
            let info = (image + u64::from(data)) as *const u8;
            let [version, _prolog, count, frame] = info.cast::<[u8; 4]>().read();
            if !matches!(version & 7, 1 | 2) {
                return false;
            }
            let chained = version >> 3 & 4 != 0;
            if part == 0 {
                pointer = frame & 15;
            }
            let codes = info.add(4);
            let count = usize::from(count);
            let slot = |i: usize| codes.add(2 * i).cast::<u16>().read_unaligned();
            let mut i = 0;
            let (mut found, mut set) = (false, false);
            while i < count {
                let [offset, op] = codes.add(2 * i).cast::<[u8; 2]>().read();
                let (op, op_info) = (op & 15, usize::from(op >> 4));
                i += match op {
                    // A register pushed.
                    0 => {
                        reach += 8;
                        1
                    }
                    // A large allocation: its size in the next one or two slots.
                    1 if op_info == 0 => {
                        reach += usize::from(slot(i + 1)) * 8;
                        2
                    }
                    1 => {
                        reach += usize::from(slot(i + 1)) | usize::from(slot(i + 2)) << 16;
                        3
                    }
                    // A small allocation.
                    2 => {
                        reach += op_info * 8 + 8;
                        1
                    }
                    // The frame pointer set: done already if the thread is
                    // past that instruction, or in a later part of the function.
                    3 => {
                        found = true;
                        set = part > 0 || chained || at >= u64::from(offset);
                        1
                    }
                    // Registers saved in the frame (near, far), an epilogue
                    // note and a spare code, vector registers saved.
                    4 | 8 => 2,
                    5 | 9 => 3,
                    6 => 1,
                    7 => 2,
                    // A machine frame.
                    10 => {
                        reach += 48;
                        1
                    }
                    _ => return false,
                };
            }
            if frame & 15 != 0 && (set || !found || part > 0 || chained) {
                let register = gpr(context, frame & 15) as usize;
                let back = usize::from(frame >> 4) * 16;
                if register < start + back || register > end || based == bases.len() {
                    return false;
                }
                bases[based] = register - back;
                based += 1;
            }
            if !chained {
                let rsp = context.Rsp as usize;
                let code = image + u64::from(finish) - context.Rip;
                return rsp >= start
                    && rsp < end
                    && reach <= SLACK
                    && rsp + reach <= limit
                    && bases[..based].iter().all(|base| base + reach <= limit)
                    && epilogue_fits(context, code as usize, pointer, start, end);
            }
            let parent = codes
                .add(2 * ((count + 1) & !1))
                .cast::<[u32; 3]>()
                .read_unaligned();
            data = parent[2];
        }
        false
    }
}

/// Whether the instructions at `context.Rip` (`code` bytes of them are the
/// function's), if the unwinder takes them for the end of a function,
/// leave the stack pointer inside our copy of a stack (`start..end`).
/// `pointer` is the function's frame register (0: none).
///
/// Where a thread stands at a function's last instructions, the
/// unwinder does not use the unwind codes: it runs those instructions
/// itself, an `add rsp, n` or a `lea rsp, [frame register + n]`, then the
/// `pop`s, then the return. An address that is no instruction's start can
/// read as a `pop rsp`, or as an `add` of any number: this reads the
/// bytes as the unwinder does and refuses what would take the stack
/// pointer out of the copy. It takes a little more for a function's end
/// than the unwinder does (any jump after the `pop`s, wherever it goes).
///
/// # Safety
/// `code` bytes at `context.Rip` are readable.
unsafe fn epilogue_fits(
    context: &CONTEXT,
    code: usize,
    pointer: u8,
    start: usize,
    end: usize,
) -> bool {
    let rip = context.Rip as *const u8;
    // Past the function's end reads as a breakpoint: nothing of an epilogue.
    let byte = |i: usize| {
        if i < code {
            // SAFETY: inside the `code` bytes the caller says are readable.
            unsafe { rip.add(i).read() }
        } else {
            0xcc
        }
    };
    let number = |i: usize| i32::from_le_bytes([byte(i), byte(i + 1), byte(i + 2), byte(i + 3)]);
    let moved = |from: u64, by: i32| (from as usize).wrapping_add(by as isize as usize);
    let mut rsp = context.Rsp as usize;
    let mut at = 0;
    if byte(0) == 0x48 && byte(1) == 0x83 && byte(2) == 0xc4 {
        // add rsp, n (a byte).
        rsp = moved(context.Rsp, i32::from(byte(3) as i8));
        at = 4;
    } else if byte(0) == 0x48 && byte(1) == 0x81 && byte(2) == 0xc4 {
        // add rsp, n.
        rsp = moved(context.Rsp, number(3));
        at = 7;
    } else if byte(0) & 0xfe == 0x48 && byte(1) == 0x8d {
        // lea rsp, [register + n]: taken only from the frame register.
        let register = (byte(0) & 1) << 3 | byte(2) & 7;
        let (near, far) = (byte(2) & 0xf8 == 0x60, byte(2) & 0xf8 == 0xa0);
        if register != 0 && register == pointer && (near || far) {
            let by = if near {
                i32::from(byte(3) as i8)
            } else {
                number(3)
            };
            rsp = moved(gpr(context, register), by);
            at = if near { 4 } else { 7 };
        }
    }
    // The `pop`s.
    let (mut pops, mut pops_rsp) = (0, false);
    loop {
        let (op, rex) = (byte(at), byte(at) & 0xf0 == 0x40);
        let register = if rex { byte(at + 1) } else { op };
        if register & 0xf8 != 0x58 || pops == 64 {
            break;
        }
        pops_rsp |= register & 7 == 4 && !(rex && op & 1 != 0);
        at += if rex { 2 } else { 1 };
        pops += 1;
    }
    // A return or a jump: only then are the instructions before it run.
    let leaves = matches!(byte(at), 0xc3 | 0xc2 | 0xeb | 0xe9)
        || byte(at) == 0xf3 && byte(at + 1) == 0xc3
        || byte(at) == 0xff && byte(at + 1) == 0x25
        || byte(at) & 0xf0 == 0x40 && byte(at + 1) == 0xff && byte(at + 2) & 0x38 == 0x20;
    !leaves || (!pops_rsp && pops < 64 && rsp >= start && rsp + 8 * (pops + 1) <= end)
}

/// The class of a time in nanoseconds: four to a power of two (the
/// index), and the class's lower end.
fn held_class(ns: u64) -> (usize, u64) {
    if ns < 4 {
        return (ns as usize, ns);
    }
    let bits = 63 - ns.leading_zeros() as usize;
    let quarter = ns >> (bits - 2) & 3;
    (bits * 4 + quarter as usize, (4 + quarter) << (bits - 2))
}

/// The general register numbered `n` as unwind data numbers them.
fn gpr(context: &CONTEXT, n: u8) -> u64 {
    match n & 15 {
        0 => context.Rax,
        1 => context.Rcx,
        2 => context.Rdx,
        3 => context.Rbx,
        4 => context.Rsp,
        5 => context.Rbp,
        6 => context.Rsi,
        7 => context.Rdi,
        8 => context.R8,
        9 => context.R9,
        10 => context.R10,
        11 => context.R11,
        12 => context.R12,
        13 => context.R13,
        14 => context.R14,
        _ => context.R15,
    }
}

/// The processor counter's cycles per microsecond, measured.
fn cycles_per_us() -> f64 {
    let began = Instant::now();
    // SAFETY: reads the processor's time stamp counter.
    let from = unsafe { std::arch::x86_64::_rdtsc() };
    std::thread::sleep(Duration::from_millis(20));
    // SAFETY: as above.
    let cycles = unsafe { std::arch::x86_64::_rdtsc() }.wrapping_sub(from);
    (cycles as f64 / began.elapsed().as_micros().max(1) as f64).max(1.0)
}

/// The reservation of a thread's stack, lowest address and end; (0, 0) if
/// it cannot be asked (the thread ended).
fn stack_of(thread: HANDLE) -> (usize, usize) {
    /// `THREAD_BASIC_INFORMATION` (ntddk).
    #[repr(C)]
    struct Basic {
        exit_status: i32,
        teb: usize,
        client_id: [usize; 2],
        affinity: usize,
        priority: i32,
        base_priority: i32,
    }
    let mut basic = Basic {
        exit_status: 0,
        teb: 0,
        client_id: [0; 2],
        affinity: 0,
        priority: 0,
        base_priority: 0,
    };
    // SAFETY: writes `basic`, whose layout and size are the system's.
    let status = unsafe {
        NtQueryInformationThread(
            thread,
            0,
            (&raw mut basic).cast(),
            size_of::<Basic>() as u32,
            null_mut(),
        )
    };
    if status != 0 || basic.teb == 0 {
        return (0, 0);
    }
    // The thread's block: the stack's end at +8, the start of its
    // reservation at +0x1478. Read by the kernel: the thread may end.
    let word = |at: usize| {
        let (mut value, mut got) = (0usize, 0);
        // SAFETY: reads eight bytes into `value`; fails if they are gone.
        let read = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                at as *const c_void,
                (&raw mut value).cast(),
                8,
                &mut got,
            )
        };
        if read != 0 && got == 8 { value } else { 0 }
    };
    let (high, low) = (word(basic.teb + 8), word(basic.teb + 0x1478));
    if low != 0 && low < high {
        (low, high)
    } else {
        (0, 0)
    }
}

/// The address a thread started at; 0 if it cannot be asked.
fn start_of(thread: HANDLE) -> u64 {
    let mut start = 0u64;
    // SAFETY: writes one pointer-sized start address
    // (`ThreadQuerySetWin32StartAddress` is 9).
    unsafe {
        NtQueryInformationThread(thread, 9, (&raw mut start).cast(), 8, null_mut());
    }
    start
}

fn thread_name(thread: HANDLE) -> String {
    let mut text: *mut u16 = null_mut();
    // SAFETY: the system allocates the string; we free it with LocalFree.
    unsafe {
        if GetThreadDescription(thread, &mut text) < 0 || text.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| *text.add(i) != 0).count();
        let name = String::from_utf16_lossy(std::slice::from_raw_parts(text, len));
        LocalFree(text.cast());
        name
    }
}

// ---------------------------------------------------------------------
// Names, found when a report is written.
// ---------------------------------------------------------------------

/// Whose code a frame is.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Kind {
    Ours = 0,
    Driver = 1,
    Direct3d = 2,
    System = 3,
    Title = 4,
    Other = 5,
}

const KINDS: [&str; 6] = [
    "ours",
    "graphics driver",
    "d3d12 / dxgi",
    "ntdll + kernel",
    "title",
    "other",
];

/// The kind of a module by its file name (lower case).
fn kind_of(file: &str, ours: bool) -> Kind {
    let starts = |list: &[&str]| list.iter().any(|p| file.starts_with(p));
    if ours {
        Kind::Ours
    } else if file.starts_with("victory-image-") {
        Kind::Title
    } else if starts(&["nv", "amd", "ati", "igd", "igc", "ig1", "ig9", "ig7", "ig8"]) {
        Kind::Driver
    } else if starts(&["d3d", "dxgi", "dxcore", "dxil", "dcomp", "dwmapi"]) {
        Kind::Direct3d
    } else if matches!(
        file,
        "ntdll.dll" | "kernelbase.dll" | "kernel32.dll" | "win32u.dll"
    ) {
        Kind::System
    } else {
        Kind::Other
    }
}

struct Module {
    base: u64,
    end: u64,
    /// What its frames are named by: `title`, or its file name.
    label: String,
    kind: Kind,
}

/// A function as the report names it.
struct Func {
    name: String,
    /// Of ours, but the language's runtime (`std::`, `core::`, `alloc::`).
    runtime: bool,
    /// A call that waits ([`waits`]).
    wait: bool,
}

/// What an address is.
struct Resolved {
    /// The functions it is in, outermost first: more than one where the
    /// compiler inlined calls (ours only).
    funcs: Box<[u32]>,
    kind: Option<Kind>,
    /// Its source line (ours), an index into `Symbols::lines`.
    line: Option<u32>,
}

type SymFromAddr = unsafe extern "system" fn(HANDLE, u64, *mut u64, *mut SYMBOL_INFOW) -> i32;
type SymFromInline =
    unsafe extern "system" fn(HANDLE, u64, u32, *mut u64, *mut SYMBOL_INFOW) -> i32;
type SymInlineQuery =
    unsafe extern "system" fn(HANDLE, u64, u32, u64, u64, *mut u32, *mut u32) -> i32;
type SymLine = unsafe extern "system" fn(HANDLE, u64, *mut u32, *mut IMAGEHLP_LINEW64) -> i32;
type SymLineInline =
    unsafe extern "system" fn(HANDLE, u64, u32, u64, *mut u32, *mut IMAGEHLP_LINEW64) -> i32;

/// The system's symbol library (`dbghelp.dll`), loaded when the first
/// report is written: a build that does not profile does not load it.
struct DbgHelp {
    process: HANDLE,
    refresh: unsafe extern "system" fn(HANDLE) -> i32,
    from_addr: SymFromAddr,
    line: Option<SymLine>,
    module: Option<unsafe extern "system" fn(HANDLE, u64, *mut IMAGEHLP_MODULEW64) -> i32>,
    /// Inlined calls: how many at an address, their first context, a name
    /// and a line by context.
    inline: Option<(
        unsafe extern "system" fn(HANDLE, u64) -> u32,
        SymInlineQuery,
        SymFromInline,
        Option<SymLineInline>,
    )>,
}

/// A symbol with room for its name.
#[repr(C)]
struct Symbol {
    info: SYMBOL_INFOW,
    name: [u16; 1024],
}

impl DbgHelp {
    fn open() -> Option<DbgHelp> {
        // SAFETY: loads the system's own dbghelp.dll and looks its
        // functions up by name; each is called with the signature the
        // system documents for it. The library's "process" is a second
        // handle to ours, so its tables are not the ones Rust's own
        // backtraces set up for the plain handle.
        unsafe {
            let library = LoadLibraryExA(
                c"dbghelp.dll".as_ptr().cast(),
                null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            );
            if library.is_null() {
                return None;
            }
            macro_rules! find {
                ($name:literal) => {
                    GetProcAddress(library, $name.as_ptr().cast()).map(|f| std::mem::transmute(f))
                };
            }
            let get_options: unsafe extern "system" fn() -> u32 = find!(c"SymGetOptions")?;
            let set_options: unsafe extern "system" fn(u32) -> u32 = find!(c"SymSetOptions")?;
            let initialize: unsafe extern "system" fn(HANDLE, *const u16, i32) -> i32 =
                find!(c"SymInitializeW")?;
            let refresh = find!(c"SymRefreshModuleList")?;
            let from_addr = find!(c"SymFromAddrW")?;
            let inline = match (
                find!(c"SymAddrIncludeInlineTrace"),
                find!(c"SymQueryInlineTrace"),
                find!(c"SymFromInlineContextW"),
            ) {
                (Some(count), Some(query), Some(name)) => {
                    Some((count, query, name, find!(c"SymGetLineFromInlineContextW")))
                }
                _ => None,
            };
            let mut process = null_mut();
            let me = GetCurrentProcess();
            if DuplicateHandle(me, me, me, &mut process, 0, 0, DUPLICATE_SAME_ACCESS) == 0 {
                return None;
            }
            // Names undecorated, lines loaded, a module's symbols read
            // when first asked for, no dialogs. Symbols are looked for
            // where the image says and beside our executable only: no
            // symbol server is asked (`_NT_SYMBOL_PATH` is not used when
            // a path is given).
            const UNDNAME: u32 = 0x2;
            const DEFERRED_LOADS: u32 = 0x4;
            const LOAD_LINES: u32 = 0x10;
            const FAIL_CRITICAL_ERRORS: u32 = 0x200;
            const NO_PROMPTS: u32 = 0x80000;
            set_options(
                get_options()
                    | UNDNAME
                    | DEFERRED_LOADS
                    | LOAD_LINES
                    | FAIL_CRITICAL_ERRORS
                    | NO_PROMPTS,
            );
            let folder = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.to_path_buf()))
                .unwrap_or_default();
            let path: Vec<u16> = folder
                .to_string_lossy()
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            if initialize(process, path.as_ptr(), 1) == 0 {
                CloseHandle(process);
                return None;
            }
            Some(DbgHelp {
                process,
                refresh,
                from_addr,
                line: find!(c"SymGetLineFromAddrW64"),
                module: find!(c"SymGetModuleInfoW64"),
                inline,
            })
        }
    }

    /// A symbol's name and address from a lookup that fills `symbol`.
    fn named(lookup: impl FnOnce(*mut SYMBOL_INFOW) -> i32) -> Option<(String, u64)> {
        // SAFETY: a zeroed symbol is valid; its header says how much room
        // follows for the name, and the name read is inside that room.
        unsafe {
            let mut symbol: Box<Symbol> = Box::new(std::mem::zeroed());
            symbol.info.SizeOfStruct = size_of::<SYMBOL_INFOW>() as u32;
            symbol.info.MaxNameLen = symbol.name.len() as u32;
            // The whole of it: the name runs on past the header.
            let whole = (&raw mut *symbol).cast::<SYMBOL_INFOW>();
            if lookup(whole) == 0 {
                return None;
            }
            let len = ((*whole).NameLen as usize).min(symbol.name.len());
            let name = whole
                .cast::<u8>()
                .add(std::mem::offset_of!(SYMBOL_INFOW, Name))
                .cast::<u16>();
            let text = std::slice::from_raw_parts(name, len);
            let end = text.iter().position(|&c| c == 0).unwrap_or(len);
            Some((String::from_utf16_lossy(&text[..end]), (*whole).Address))
        }
    }

    /// The symbol at or before `address`: its name and its own address.
    fn symbol(&self, address: u64) -> Option<(String, u64)> {
        let mut displacement = 0;
        // SAFETY: the library's own call with a symbol of ours to fill.
        Self::named(|symbol| unsafe {
            (self.from_addr)(self.process, address, &mut displacement, symbol)
        })
    }

    /// The functions `address` is in, outermost first (the calls the
    /// compiler inlined there, if `inlined`), and the context of the
    /// innermost for [`DbgHelp::line`].
    fn functions(&self, address: u64, inlined: bool) -> (Vec<String>, Option<u32>) {
        if inlined && let Some((count, query, name, _)) = self.inline {
            // SAFETY: the library's own calls, as documented: the count
            // of inlined calls at an address, the context of the first,
            // then a name per context; the last context is the function
            // itself.
            unsafe {
                let inlines = count(self.process, address);
                let (mut first, mut index) = (0, 0);
                if inlines > 0
                    && query(
                        self.process,
                        address,
                        0,
                        address,
                        address,
                        &mut first,
                        &mut index,
                    ) != 0
                {
                    let mut names: Vec<String> = (first..=first + inlines)
                        .filter_map(|context| {
                            let mut displacement = 0;
                            Self::named(|symbol| {
                                name(self.process, address, context, &mut displacement, symbol)
                            })
                        })
                        .map(|(name, _)| name)
                        .collect();
                    if !names.is_empty() {
                        names.reverse();
                        return (names, Some(first));
                    }
                }
            }
        }
        (
            self.symbol(address)
                .into_iter()
                .map(|(name, _)| name)
                .collect(),
            None,
        )
    }

    /// The source file and line of `address` (of the inlined call with
    /// `context`, if any).
    fn line(&self, address: u64, context: Option<u32>) -> Option<(String, u32)> {
        // SAFETY: the library's own calls with a line of ours to fill;
        // the file name it points to is a string of its own.
        unsafe {
            let mut line: IMAGEHLP_LINEW64 = std::mem::zeroed();
            line.SizeOfStruct = size_of::<IMAGEHLP_LINEW64>() as u32;
            let mut displacement = 0;
            let found = match (context, self.inline.and_then(|i| i.3)) {
                (Some(context), Some(by_context)) => {
                    by_context(
                        self.process,
                        address,
                        context,
                        0,
                        &mut displacement,
                        &mut line,
                    ) != 0
                }
                _ => false,
            } || self.line.is_some_and(|by_address| {
                by_address(self.process, address, &mut displacement, &mut line) != 0
            });
            if !found || line.FileName.is_null() {
                return None;
            }
            let len = (0..1024)
                .take_while(|&i| *line.FileName.add(i) != 0)
                .count();
            let file = String::from_utf16_lossy(std::slice::from_raw_parts(line.FileName, len));
            Some((file, line.LineNumber))
        }
    }

    /// Whether the module at `base` has its symbols from a PDB, and what
    /// the library says it loaded.
    fn has_pdb(&self, base: u64) -> (bool, String) {
        let Some(module) = self.module else {
            return (false, String::new());
        };
        // SAFETY: the library's own call with a sized structure to fill.
        unsafe {
            let mut info: Box<IMAGEHLP_MODULEW64> = Box::new(std::mem::zeroed());
            info.SizeOfStruct = size_of::<IMAGEHLP_MODULEW64>() as u32;
            if module(self.process, base, &mut *info) == 0 {
                return (false, String::new());
            }
            let text = |chars: &[u16]| {
                let end = chars.iter().position(|&c| c == 0).unwrap_or(chars.len());
                String::from_utf16_lossy(&chars[..end])
            };
            // SymPdb is 3 and SymDia 7.
            (matches!(info.SymType, 3 | 7), text(&info.LoadedPdbName))
        }
    }
}

/// The start of the function `address` is in, by its image's unwind data
/// (the first part's, for a function in several); `None` for code that
/// has none.
fn function_start(address: u64) -> Option<u64> {
    let mut image = 0;
    // SAFETY: looks up the unwind data covering `address`, if any, and
    // reads it in its loaded image: a part of a function names the entry
    // of the function it belongs to after its codes.
    unsafe {
        let entry = RtlLookupFunctionEntry(address, &mut image, null_mut());
        if entry.is_null() {
            return None;
        }
        let mut entry = entry.cast::<[u32; 3]>().read_unaligned();
        for _ in 0..8 {
            if entry[2] & 1 != 0 {
                break;
            }
            let info = (image + u64::from(entry[2])) as *const u8;
            let [version, _, count, _] = info.cast::<[u8; 4]>().read();
            if version >> 3 & 4 == 0 {
                break;
            }
            let after = info.add(4 + 2 * ((usize::from(count) + 1) & !1));
            entry = after.cast::<[u32; 3]>().read_unaligned();
        }
        Some(image + u64::from(entry[0]))
    }
}

/// Whether a thread whose innermost frame is `name` waits there: the
/// kernel's waits, by their names in ntdll and win32u (`Nt` or `Zw`).
fn waits(name: &str) -> bool {
    let function = name.rsplit('!').next().unwrap_or(name);
    let Some(call) = function
        .strip_prefix("Nt")
        .or_else(|| function.strip_prefix("Zw"))
    else {
        return false;
    };
    [
        "WaitFor",
        "DelayExecution",
        "RemoveIoCompletion",
        "SignalAndWait",
        "UserMsgWaitForMultipleObjects",
        "UserGetMessage",
        "UserWaitMessage",
        "GdiDdDDIWaitFor",
    ]
    .iter()
    .any(|wait| call.starts_with(wait))
}

/// What kind of wait the frames between a waiting call and the function
/// of ours that waited make it.
fn wait_kind(path: &[&str]) -> &'static str {
    let has = |what: &str| path.iter().any(|name| name.contains(what));
    if has("SleepConditionVariable") || has("Condvar") {
        "condition variable"
    } else if has("SRWLock") {
        "lock (SRW)"
    } else if has("CriticalSection") {
        "lock (critical section)"
    } else if has("Mutex") || has("RwLock") || has("Once") {
        "lock (ours)"
    } else if has("park") {
        "parked"
    } else if has("Sleep") || has("sleep") || has("DelayExecution") {
        "sleep"
    } else if has("GdiDdDDIWaitFor") {
        "graphics kernel"
    } else if has("WaitOnAddress") || has("WaitForAlertByThreadId") {
        "wait on address"
    } else if has("IoCompletion") || has("GetQueuedCompletionStatus") {
        "I/O completion"
    } else if has("MsgWait") || has("GetMessage") || has("WaitMessage") {
        "window messages"
    } else if has("WaitForMultipleObjects") || has("WaitForSingleObject") {
        "event or handle"
    } else {
        "other wait"
    }
}

struct Symbols {
    dbghelp: Option<DbgHelp>,
    inline: bool,
    /// Loaded modules by base, as of the report being written.
    modules: Vec<Module>,
    /// Whether our executable's names come from its PDB, and which.
    pdb: (bool, String),
    funcs: Vec<Func>,
    by_name: HashMap<String, u32>,
    lines: Vec<String>,
    by_line: HashMap<String, u32>,
    resolved: Vec<Resolved>,
    by_address: FastMap<u64, u32>,
}

impl Symbols {
    fn new(inline: bool) -> Symbols {
        Symbols {
            dbghelp: DbgHelp::open(),
            inline,
            modules: Vec::new(),
            pdb: (false, String::new()),
            funcs: Vec::new(),
            by_name: HashMap::new(),
            lines: Vec::new(),
            by_line: HashMap::new(),
            resolved: Vec::new(),
            by_address: FastMap::default(),
        }
    }

    /// Lists the loaded modules anew (a report's first step).
    fn refresh(&mut self) {
        // SAFETY: plain queries of our own process with sized buffers.
        unsafe {
            if let Some(dbghelp) = &self.dbghelp {
                (dbghelp.refresh)(dbghelp.process);
            }
            let me = GetCurrentProcess();
            let mut handles: Vec<HMODULE> = vec![null_mut(); 2048];
            let mut bytes = 0;
            let size = (handles.len() * size_of::<HMODULE>()) as u32;
            if K32EnumProcessModules(me, handles.as_mut_ptr(), size, &mut bytes) == 0 {
                return;
            }
            handles.truncate(bytes as usize / size_of::<HMODULE>());
            let exe = GetModuleHandleW(null());
            let mut modules = Vec::with_capacity(handles.len());
            for handle in handles {
                let mut info = MODULEINFO {
                    lpBaseOfDll: null_mut(),
                    SizeOfImage: 0,
                    EntryPoint: null_mut(),
                };
                if K32GetModuleInformation(me, handle, &mut info, size_of::<MODULEINFO>() as u32)
                    == 0
                {
                    continue;
                }
                let mut path = [0u16; 520];
                let len = GetModuleFileNameW(handle, path.as_mut_ptr(), path.len() as u32);
                let path = String::from_utf16_lossy(&path[..len as usize]);
                let file = path.rsplit(['\\', '/']).next().unwrap_or(&path).to_owned();
                let kind = kind_of(&file.to_ascii_lowercase(), handle == exe);
                let base = handle as u64;
                modules.push(Module {
                    base,
                    end: base + u64::from(info.SizeOfImage),
                    label: file,
                    kind,
                });
            }
            modules.sort_by_key(|m| m.base);
            // The title's images are named by their content: the largest
            // is the title itself, the others its libraries.
            let largest = modules
                .iter()
                .filter(|m| m.kind == Kind::Title)
                .map(|m| m.end - m.base)
                .max();
            for module in modules.iter_mut().filter(|m| m.kind == Kind::Title) {
                module.label = if Some(module.end - module.base) == largest {
                    "title".to_owned()
                } else {
                    let hash = module.label.trim_start_matches("victory-image-");
                    format!("title-{}", &hash[..hash.len().min(6)])
                };
            }
            self.modules = modules;
            if let Some(dbghelp) = &self.dbghelp
                && !self.pdb.0
            {
                // The executable's symbols are read when first asked for.
                dbghelp.symbol(function_start as *const () as u64);
                self.pdb = dbghelp.has_pdb(exe as u64);
            }
        }
    }

    fn func(&mut self, name: String, kind: Option<Kind>) -> u32 {
        if let Some(&at) = self.by_name.get(&name) {
            return at;
        }
        // (By the name before its type arguments: a closure of ours is one.)
        let own_name = name.split('<').next().unwrap_or(&name);
        let runtime = kind == Some(Kind::Ours)
            && [
                "std::", "core::", "alloc::", "__rust", "__chkstk", "mem", "str",
            ]
            .iter()
            .any(|p| own_name.starts_with(p));
        self.funcs.push(Func {
            wait: waits(&name),
            name: name.clone(),
            runtime,
        });
        self.by_name.insert(name, self.funcs.len() as u32 - 1);
        self.funcs.len() as u32 - 1
    }

    /// What `address` is, as an index into `self.resolved`.
    fn resolve(&mut self, address: u64) -> u32 {
        if let Some(&at) = self.by_address.get(&address) {
            return at;
        }
        let resolved = self.look_up(address);
        self.resolved.push(resolved);
        let at = self.resolved.len() as u32 - 1;
        self.by_address.insert(address, at);
        at
    }

    fn look_up(&mut self, address: u64) -> Resolved {
        let marker = match address {
            CUT => Some("[frames not kept]"),
            FULL => Some("[samples after the trees were full]"),
            _ => None,
        };
        if let Some(name) = marker {
            let func = self.func(name.to_owned(), None);
            return Resolved {
                funcs: Box::new([func]),
                kind: None,
                line: None,
            };
        }
        let at = self.modules.partition_point(|m| m.base <= address);
        let module = at
            .checked_sub(1)
            .map(|at| &self.modules[at])
            .filter(|m| address < m.end);
        let Some(module) = module else {
            let func = self.func("[code in no loaded module]".to_owned(), Some(Kind::Other));
            return Resolved {
                funcs: Box::new([func]),
                kind: Some(Kind::Other),
                line: None,
            };
        };
        let (base, kind, label) = (module.base, module.kind, module.label.clone());
        let start = function_start(address);
        // By the function's start in its module: what code with no names
        // (the title, the driver) is told apart by.
        let plain = || match start {
            Some(start) => format!("{label}+{:#x}", start - base),
            None => format!("{label}+{:#x}", (address - base) & !15),
        };
        let mut line = None;
        let names = match (&self.dbghelp, kind) {
            (_, Kind::Title) | (None, _) => vec![plain()],
            (Some(dbghelp), Kind::Ours) if self.pdb.0 => {
                let (names, context) = dbghelp.functions(address, self.inline);
                line = dbghelp.line(address, context);
                if names.is_empty() {
                    vec![plain()]
                } else {
                    names
                }
            }
            (Some(dbghelp), _) => {
                // An export names the function only if it starts it: the
                // nearest export below is some other function's.
                let export = dbghelp.symbol(address).filter(|&(_, at)| match start {
                    Some(start) => at >= start && at <= address,
                    None => at <= address && address - at < 0x100,
                });
                match export {
                    Some((name, _)) => {
                        let module = label.rsplit_once('.').map_or(&*label, |(stem, _)| stem);
                        vec![format!("{module}!{name}")]
                    }
                    None => vec![plain()],
                }
            }
        };
        let funcs = names
            .into_iter()
            .map(|name| self.func(name, Some(kind)))
            .collect();
        let line = line.map(|(file, line)| {
            // The last three parts of the path: the crate, `src`, the file.
            let mut parts: Vec<&str> = file.rsplit(['\\', '/']).take(3).collect();
            parts.reverse();
            let text = format!("{}:{line}", parts.join("\\"));
            *self.by_line.entry(text.clone()).or_insert_with(|| {
                self.lines.push(text);
                self.lines.len() as u32 - 1
            })
        });
        Resolved {
            funcs,
            kind: Some(kind),
            line,
        }
    }
}

// ---------------------------------------------------------------------
// The report.
// ---------------------------------------------------------------------

/// A function's time in a role, microseconds.
#[derive(Default, Clone, Copy)]
struct Row {
    /// In the function itself, and of that on a processor.
    own: u64,
    cpu: u64,
    /// In it and in what it called.
    all: u64,
}

/// What a role's threads did.
#[derive(Default)]
struct RoleReport {
    threads: Vec<usize>,
    wall: u64,
    cpu: u64,
    waiting: u64,
    funcs: FastMap<u32, Row>,
    /// By [`Kind`]: innermost frame's, the nearest frame's that is not the
    /// system's, and with what it called.
    modules: [[u64; 3]; 6],
    /// (kind of wait, the function that waited, through whose code) to
    /// microseconds.
    waits: HashMap<(&'static str, u32, Option<Kind>), u64>,
    lines: FastMap<(u32, u32), u64>,
    /// Stacks as function indices, outermost first.
    stacks: HashMap<Vec<u32>, u64>,
}

/// A written report, and its numbers for the tests.
struct Report {
    text: String,
    folded: String,
    #[cfg_attr(not(test), allow(dead_code))]
    roles: Vec<RoleSummary>,
}

/// A role's numbers as the tests ask for them (microseconds).
#[cfg_attr(not(test), allow(dead_code))]
struct RoleSummary {
    role: Role,
    wall: u64,
    waiting: u64,
    /// (name, own, with what it called), every function.
    functions: Vec<(String, u64, u64)>,
    /// (kind, the function that waited, time).
    waits: Vec<(String, String, u64)>,
}

/// The stacks written to the stacks file: those with this share of their
/// role's time or more; the rest go to one line per role.
const FOLD_LEAST: f64 = 0.00005;

impl Sampler {
    fn report(&mut self) -> Report {
        let began = Instant::now();
        let inline = self.config.inline;
        let stood = (self.held_within(0.5), self.held_within(0.99));
        let symbols = self.symbols.get_or_insert_with(|| Symbols::new(inline));
        symbols.refresh();
        // Every node's address, named.
        let named: Vec<Vec<u32>> = self
            .threads
            .iter()
            .map(|t| {
                t.tree
                    .nodes
                    .iter()
                    .map(|n| symbols.resolve(n.address))
                    .collect()
            })
            .collect();
        let starts: Vec<u32> = self
            .threads
            .iter()
            .map(|t| symbols.resolve(t.start))
            .collect();
        let symbols = &*symbols;
        let mut roles: Vec<RoleReport> = Role::ALL.iter().map(|_| RoleReport::default()).collect();
        // Per thread: wall, on a processor, waiting (microseconds).
        let mut per_thread = vec![[0u64; 3]; self.threads.len()];
        // Per thread: own time by function, for the one it is mostly in.
        let mut mostly: Vec<FastMap<u32, u64>> = vec![FastMap::default(); self.threads.len()];
        let mut stamp = 0u32;
        let mut seen = vec![0u32; symbols.funcs.len()];
        let mut path: Vec<(u32, Option<Kind>)> = Vec::new();
        for (t, thread) in self.threads.iter().enumerate() {
            let role = &mut roles[thread.role as usize];
            role.threads.push(t);
            let nodes = &thread.tree.nodes;
            for (i, node) in nodes.iter().enumerate().skip(1) {
                if node.wall == 0 {
                    continue;
                }
                // The stack, innermost first, inlined calls as frames.
                path.clear();
                let mut at = i;
                while at != 0 {
                    let resolved = &symbols.resolved[named[t][at] as usize];
                    path.extend(resolved.funcs.iter().rev().map(|&f| (f, resolved.kind)));
                    at = nodes[at].parent as usize;
                }
                let (wall, cpu) = (node.wall, node.cpu);
                role.wall += wall;
                role.cpu += cpu;
                per_thread[t][0] += wall;
                per_thread[t][1] += cpu;
                *mostly[t].entry(path[0].0).or_default() += wall;
                let (leaf, leaf_kind) = path[0];
                let row = role.funcs.entry(leaf).or_default();
                row.own += wall;
                row.cpu += cpu;
                stamp += 1;
                let mut kinds = [false; 6];
                // Every thread starts in the system's code: those outermost
                // frames are not the system "with what it calls".
                let start = path
                    .iter()
                    .rposition(|p| p.1.is_some_and(|k| k != Kind::System));
                for (at, &(func, kind)) in path.iter().enumerate() {
                    if seen[func as usize] != stamp {
                        seen[func as usize] = stamp;
                        role.funcs.entry(func).or_default().all += wall;
                    }
                    if let Some(kind) = kind
                        && start.is_some_and(|start| at <= start)
                    {
                        kinds[kind as usize] = true;
                    }
                }
                for (kind, &on) in kinds.iter().enumerate() {
                    role.modules[kind][2] += if on { wall } else { 0 };
                }
                if let Some(kind) = leaf_kind {
                    role.modules[kind as usize][0] += wall;
                }
                // Whose time it is: the innermost frame that is not the
                // system's (a kernel call is its caller's).
                let owner = path
                    .iter()
                    .filter_map(|&(_, kind)| kind)
                    .find(|&kind| !matches!(kind, Kind::System | Kind::Other))
                    .or(leaf_kind);
                if let Some(kind) = owner {
                    role.modules[kind as usize][1] += wall;
                }
                let ours = |&(func, kind): &(u32, Option<Kind>)| {
                    kind == Some(Kind::Ours) && !symbols.funcs[func as usize].runtime
                };
                if symbols.funcs[leaf as usize].wait {
                    role.waiting += wall;
                    per_thread[t][2] += wall;
                    // The function of ours that waited (the title's, on a
                    // thread with none of ours), and what lies between.
                    let caller = path
                        .iter()
                        .position(ours)
                        .or_else(|| path.iter().position(|p| p.1 == Some(Kind::Title)));
                    let between = &path[..caller.unwrap_or(path.len())];
                    let names: Vec<&str> = between
                        .iter()
                        .map(|&(f, _)| &*symbols.funcs[f as usize].name)
                        .collect();
                    let through = between.iter().filter_map(|&(_, kind)| kind).find(|kind| {
                        matches!(
                            kind,
                            Kind::Driver | Kind::Direct3d | Kind::Title | Kind::Other
                        )
                    });
                    let caller = caller.map_or(u32::MAX, |at| path[at].0);
                    *role
                        .waits
                        .entry((wait_kind(&names), caller, through))
                        .or_default() += wall;
                }
                let resolved = &symbols.resolved[named[t][i] as usize];
                if let Some(line) = resolved.line {
                    *role.lines.entry((line, leaf)).or_default() += wall;
                }
                let stack: Vec<u32> = path.iter().rev().map(|&(f, _)| f).collect();
                *role.stacks.entry(stack).or_default() += wall;
            }
        }

        // Written out.
        let frames = self.presented;
        let seconds = self.wall as f64 / 1e6;
        // Microseconds to the unit of the tables.
        let (unit, per) = if frames > 0 {
            ("ms a frame", 1000.0 * frames as f64)
        } else {
            ("ms a second", 1000.0 * seconds.max(1e-9))
        };
        let ms = |us: u64| us as f64 / per;
        let share = |us: u64, of: u64| 100.0 * us as f64 / of.max(1) as f64;
        let name = |func: u32| match symbols.funcs.get(func as usize) {
            Some(func) => &*func.name,
            None => "[no function of ours or the title's]",
        };
        // For the tables: a long name's start and end (the stacks file has it whole).
        let short = |func: u32, most: usize| {
            let name = name(func);
            let chars = name.chars().count();
            if chars <= most {
                return name.to_owned();
            }
            let head: String = name.chars().take(most * 2 / 3).collect();
            let tail: String = name.chars().skip(chars - most / 3).collect();
            format!("{head}...{tail}")
        };
        let mut text = String::new();
        let mut folded = String::new();
        let mut summaries = Vec::new();
        let w = &mut text;
        let _ = writeln!(w, "VIctory profile, process {}", std::process::id());
        let _ = writeln!(
            w,
            "sampled {seconds:.1} s in {} looks ({} asked a second, {:.2} ms between looks, {} late); \
             {frames} frames presented meanwhile{}",
            self.ticks,
            self.config.hz,
            self.wall as f64 / 1000.0 / self.ticks.max(1) as f64,
            self.late,
            if frames > 0 {
                format!(
                    " ({:.1} ms a frame)",
                    self.wall as f64 / 1000.0 / frames as f64
                )
            } else {
                " (none: the tables are per second of sampling)".to_owned()
            },
        );
        let alive = self.threads.iter().filter(|t| t.alive).count();
        let _ = writeln!(
            w,
            "{} threads followed ({alive} still running); a thread looked at stood still under {:.0} us \
             half the times, under {:.0} us 99 in 100, {:.0} us at longest, {:.1} us on average, {} \
             times; {} call sites kept{}",
            self.threads.len(),
            stood.0,
            stood.1,
            self.held_longest as f64 / 1000.0,
            self.held_ns as f64 / 1000.0 / self.held.max(1) as f64,
            self.held,
            self.nodes,
            if self.nodes >= MOST_NODES {
                " (FULL: later new stacks are one line)"
            } else {
                ""
            },
        );
        if self.config.budget > 0.0 {
            let _ = writeln!(
                w,
                "a thread may stand still for looks {:.1} % of its time (VICTORY_PROFILE_BUDGET): one \
                 whose looks cost more is looked at every 2nd, 3rd... tick while it runs (\"every\" \
                 below); its samples stand for more time each, so the times stay right",
                100.0 * self.config.budget
            );
        }
        let (mine, since) = (own_cpu(), self.clock.elapsed());
        let _ = writeln!(
            w,
            "the sampler's thread used {:.1} % of one processor",
            100.0 * mine.as_secs_f64() / since.as_secs_f64().max(1e-9),
        );
        let _ = writeln!(
            w,
            "names of ours: {}",
            match (&symbols.dbghelp, &symbols.pdb) {
                (None, _) =>
                    "dbghelp.dll could not be used: every frame is module+offset".to_owned(),
                (Some(_), (true, pdb)) => format!("from {pdb}"),
                (Some(_), (false, _)) =>
                    "NO PDB FOUND for the executable: its frames are module+offset of each \
                     function's start (keep victory.pdb beside victory.exe)"
                        .to_owned(),
            }
        );
        let _ = writeln!(w);
        let _ = writeln!(
            w,
            "How to read. Every number is {unit} unless it is a %. A role's time is its threads' \
             together: five recording threads can make more than a frame."
        );
        let _ = writeln!(
            w,
            "  own      time with the function as the innermost frame; a call into the kernel \
             (ntdll!Nt..., win32u!Nt...) is the kernel working or waiting for that caller"
        );
        let _ = writeln!(w, "  with     own time plus everything the function called");
        let _ = writeln!(
            w,
            "  on cpu   share of the own time the thread was on a processor (its cycle counter): \
             low means it stood there blocked"
        );
        let _ = writeln!(
            w,
            "  waiting  samples whose innermost frame is one of the kernel's waits; under WAITS \
             they are given to the function of ours that waited"
        );
        let _ = writeln!(
            w,
            "  looks    times a thread was sampled; not susp: of those, the ones answered without \
             suspending it (it had not run since the look before); cut: stacks that did not reach \
             the thread's first frame; look us: what a suspension of it takes"
        );
        let _ = writeln!(
            w,
            "  title+0x1234 is the title's function starting at that offset; \
             nvwgf2umx.dll+0x1234 a driver function with no name; [frames not kept] marks stacks \
             deeper than {} frames or {} KB",
            self.config.depth,
            self.config.stack >> 10,
        );
        let _ = writeln!(w);
        let _ = writeln!(w, "THREADS");
        let _ = writeln!(
            w,
            "  {:<10} {:>7}  {:<30} {:>9} {:>8} {:>9} {:>9} {:>8} {:>7} {:>7} {:>5} {:>8}  mostly in (own time)",
            "role",
            "thread",
            "name, or where it started",
            unit,
            "on cpu %",
            "waiting %",
            "looks",
            "not susp",
            "failed",
            "cut",
            "every",
            "look us"
        );
        for role in Role::ALL {
            for &t in &roles[role as usize].threads {
                let thread = &self.threads[t];
                let [wall, cpu, waiting] = per_thread[t];
                let said = if thread.name.is_empty() {
                    let start = &symbols.resolved[starts[t] as usize];
                    format!("at {}", short(start.funcs[0], 27))
                } else {
                    thread.name.chars().take(30).collect::<String>()
                };
                let top = mostly[t].iter().max_by_key(|&(&func, &us)| (us, func));
                let top = top.map_or(String::new(), |(&func, &us)| {
                    format!("  {} ({:.0} %)", short(func, 70), share(us, wall))
                });
                let _ = writeln!(
                    w,
                    "  {:<10} {:>7}  {:<30} {:>9.1} {:>8.1} {:>9.1} {:>9} {:>8} {:>7} {:>7} {:>5} {:>8.1}{}{top}",
                    role.name(),
                    thread.id,
                    said,
                    ms(wall),
                    share(cpu, wall),
                    share(waiting, wall),
                    thread.looks,
                    thread.reused,
                    thread.missed,
                    thread.cut,
                    thread.every,
                    thread.cost / 1000.0,
                    if thread.alive { "" } else { "  (ended)" },
                );
            }
        }
        for role in Role::ALL {
            let r = &roles[role as usize];
            if r.wall == 0 {
                continue;
            }
            let _ = writeln!(w);
            let _ = writeln!(
                w,
                "=== {} ===  {} thread{}: {:.1} {unit}, on a processor {:.1} ({:.0} %), waiting {:.1} ({:.0} %)",
                role.says(),
                r.threads.len(),
                if r.threads.len() == 1 { "" } else { "s" },
                ms(r.wall),
                ms(r.cpu),
                share(r.cpu, r.wall),
                ms(r.waiting),
                share(r.waiting, r.wall),
            );
            let _ = writeln!(
                w,
                "  BY MODULE (own: the innermost frame's; for: system time given to the nearest \
                 caller that is ours, the driver's, Direct3D's or the title's; with: anywhere on \
                 the stack above the thread's start)"
            );
            let _ = writeln!(
                w,
                "    {:<18} {:>9} {:>6} {:>9} {:>6} {:>9} {:>6}",
                "", "own", "%", "for", "%", "with", "%"
            );
            for (kind, label) in KINDS.iter().enumerate() {
                let [own, owner, all] = r.modules[kind];
                let _ = writeln!(
                    w,
                    "    {:<18} {:>9.2} {:>6.1} {:>9.2} {:>6.1} {:>9.2} {:>6.1}",
                    label,
                    ms(own),
                    share(own, r.wall),
                    ms(owner),
                    share(owner, r.wall),
                    ms(all),
                    share(all, r.wall),
                );
            }
            let mut rows: Vec<(u32, Row)> = r.funcs.iter().map(|(&f, &row)| (f, row)).collect();
            let table = |w: &mut String, title: &str, rows: &[(u32, Row)]| {
                let _ = writeln!(w, "  {title}");
                let _ = writeln!(
                    w,
                    "    {:>9} {:>6} {:>9} {:>6} {:>7}  function",
                    "own", "%", "with", "%", "on cpu%"
                );
                for &(func, row) in rows.iter().take(40) {
                    let _ = writeln!(
                        w,
                        "    {:>9.2} {:>6.1} {:>9.2} {:>6.1} {:>7}  {}",
                        ms(row.own),
                        share(row.own, r.wall),
                        ms(row.all),
                        share(row.all, r.wall),
                        if row.own > 0 {
                            format!("{:.0}", share(row.cpu, row.own))
                        } else {
                            "-".into()
                        },
                        short(func, 150),
                    );
                }
            };
            rows.sort_by(|a, b| b.1.own.cmp(&a.1.own).then(a.0.cmp(&b.0)));
            let own = rows.iter().take_while(|row| row.1.own > 0).count();
            table(&mut *w, "FUNCTIONS BY OWN TIME (top 40)", &rows[..own]);
            // Every stack of a thread has its start-up frames: they say nothing.
            rows.retain(|&(func, _)| {
                let func = &symbols.funcs[func as usize];
                let start = ["!RtlUserThreadStart", "!BaseThreadInitThunk"];
                !func.runtime && !start.iter().any(|s| func.name.ends_with(s))
            });
            rows.sort_by(|a, b| b.1.all.cmp(&a.1.all).then(a.0.cmp(&b.0)));
            table(
                &mut *w,
                "FUNCTIONS WITH WHAT THEY CALL (top 40; without the thread's start and the language's runtime)",
                &rows,
            );
            let mut waits: Vec<_> = r.waits.iter().map(|(&key, &us)| (us, key)).collect();
            waits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.1.cmp(&b.1.1)));
            if !waits.is_empty() {
                let _ = writeln!(
                    w,
                    "  WAITS ({:.1} {unit}, {:.0} % of the role), by the function that waited",
                    ms(r.waiting),
                    share(r.waiting, r.wall)
                );
                let _ = writeln!(
                    w,
                    "    {:>9} {:>6}  {:<24} {:<16} waited in",
                    "", "%", "kind", "inside"
                );
                for &(us, (kind, caller, through)) in waits.iter().take(30) {
                    let _ = writeln!(
                        w,
                        "    {:>9.2} {:>6.1}  {:<24} {:<16} {}",
                        ms(us),
                        share(us, r.wall),
                        kind,
                        through.map_or("-", |kind| KINDS[kind as usize]),
                        short(caller, 150),
                    );
                }
            }
            let mut lines: Vec<_> = r.lines.iter().map(|(&key, &us)| (us, key)).collect();
            lines.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            if !lines.is_empty() {
                let _ = writeln!(w, "  HOTTEST LINES OF OURS (own time)");
                for &(us, (line, func)) in lines.iter().take(25) {
                    let _ = writeln!(
                        w,
                        "    {:>9.2} {:>6.1}  {}  ({})",
                        ms(us),
                        share(us, r.wall),
                        symbols.lines[line as usize],
                        short(func, 100),
                    );
                }
            }
            let mut stacks: Vec<(&Vec<u32>, u64)> =
                r.stacks.iter().map(|(s, &us)| (s, us)).collect();
            stacks.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            let fold = |stack: &[u32]| {
                let mut line = String::from(role.name());
                for &func in stack {
                    line.push(';');
                    line.extend(name(func).chars().map(|c| match c {
                        ';' => ':',
                        ' ' => '_',
                        c => c,
                    }));
                }
                line
            };
            let _ = writeln!(
                w,
                "  STACKS (the 12 heaviest of {}, their innermost 10 frames, {unit}; all of them \
                 whole, in microseconds, in the .folded file)",
                stacks.len()
            );
            for &(stack, us) in stacks.iter().take(12) {
                let from = stack.len().saturating_sub(10);
                let _ = writeln!(
                    w,
                    "    {:>9.2} {:>6.1}  {}{}",
                    ms(us),
                    share(us, r.wall),
                    if from > 0 { "... " } else { "" },
                    stack[from..]
                        .iter()
                        .map(|&f| short(f, 60))
                        .collect::<Vec<_>>()
                        .join(" > "),
                );
            }
            let least = (FOLD_LEAST * r.wall as f64) as u64;
            let mut rest = 0;
            for &(stack, us) in &stacks {
                if us >= least {
                    let _ = writeln!(folded, "{} {us}", fold(stack));
                } else {
                    rest += us;
                }
            }
            if rest > 0 {
                let _ = writeln!(
                    folded,
                    "{};[stacks_under_{:.3}_%_each] {rest}",
                    role.name(),
                    100.0 * FOLD_LEAST
                );
            }
            summaries.push(RoleSummary {
                role,
                wall: r.wall,
                waiting: r.waiting,
                functions: r
                    .funcs
                    .iter()
                    .map(|(&f, row)| (name(f).to_owned(), row.own, row.all))
                    .collect(),
                waits: waits
                    .iter()
                    .map(|&(us, (kind, caller, _))| (kind.to_owned(), name(caller).to_owned(), us))
                    .collect(),
            });
        }
        let _ = writeln!(w);
        let _ = writeln!(
            w,
            "(this report took {:.0} ms to write; sampling stood still meanwhile)",
            began.elapsed().as_secs_f64() * 1000.0
        );
        Report {
            text,
            folded,
            roles: summaries,
        }
    }

    /// Writes the report and the stacks file, each whole or not at all (a
    /// process ended in the middle leaves the last ones).
    fn write(&mut self) {
        let report = self.report();
        let out = self.config.out.clone();
        let put = |file: PathBuf, text: &str| {
            let mut partial = file.clone().into_os_string();
            partial.push(".part");
            std::fs::write(&partial, text).and_then(|()| std::fs::rename(&partial, &file))
        };
        let mut stacks = out.clone().into_os_string();
        stacks.push(".folded");
        let written =
            put(out.clone(), &report.text).and_then(|()| put(stacks.into(), &report.folded));
        static SAID: AtomicBool = AtomicBool::new(false);
        match written {
            Ok(()) if !SAID.swap(true, Relaxed) => {
                tracing::info!(file = %out.display(), "profile written (and again as the run goes on; the stacks for a flame graph are beside it, .folded)");
            }
            Ok(()) => {}
            Err(e) => tracing::warn!(error = %e, file = %out.display(), "profile not written"),
        }
        self.resync();
    }
}

/// The processor time the calling thread has used.
fn own_cpu() -> Duration {
    use windows_sys::Win32::Foundation::FILETIME;
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
    // SAFETY: our own thread and four valid out pointers.
    unsafe {
        GetThreadTimes(
            GetCurrentThread(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    };
    let ticks = |t: FILETIME| u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime);
    Duration::from_nanos((ticks(kernel) + ticks(user)) * 100)
}

// ---------------------------------------------------------------------
// The sampler's thread.
// ---------------------------------------------------------------------

/// Asks the sampler to write its last report and end; set when it has.
static STOP: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
/// The sampler's thread, as a handle: to see at exit whether it still runs.
static SAMPLER: std::sync::atomic::AtomicPtr<c_void> =
    std::sync::atomic::AtomicPtr::new(null_mut());

unsafe extern "C" {
    fn atexit(f: extern "C" fn()) -> i32;
}

/// At process exit (registered after the loader's own exit function, so
/// run before it ends the other threads): the last report is written.
extern "C" fn at_exit() {
    STOP.store(true, Relaxed);
    let thread = SAMPLER.load(Relaxed);
    let waiting = Instant::now();
    while !DONE.load(Relaxed) && waiting.elapsed() < Duration::from_secs(10) {
        // SAFETY: the sampler's thread handle, never closed; signalled
        // when the thread is gone, however it went.
        if thread.is_null() || unsafe { WaitForSingleObject(thread, 5) } == 0 {
            break;
        }
    }
}

/// The sampler's clock: a timer of the system's finer kind, which fires
/// every period by itself where the period is whole milliseconds (a
/// sleep of a millisecond takes one and a half).
struct Pace {
    timer: HANDLE,
    period: Duration,
    /// The timer fires by itself; otherwise it is set before each wait,
    /// for `next`.
    repeats: bool,
    next: Instant,
    /// Microseconds a tick is put off by at most, and the state of the
    /// numbers that say by how much.
    jitter: u64,
    random: u64,
}

impl Pace {
    fn new(hz: u32, jitter: u64) -> Pace {
        use windows_sys::Win32::System::Threading::{
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimer,
        };
        let hz = hz.max(1);
        let period = Duration::from_secs_f64(1.0 / f64::from(hz));
        // SAFETY: makes a timer of ours (all access), unnamed.
        let timer = unsafe {
            CreateWaitableTimerExW(
                null(),
                null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                0x1f_0003,
            )
        };
        let mut repeats = false;
        if !timer.is_null() && hz <= 1000 && 1000 % hz == 0 {
            // First in a period from now (100 ns units, negative: from
            // now), then every period (milliseconds).
            let first = -(period.as_nanos() as i64 / 100);
            // SAFETY: our timer; no completion routine.
            repeats = unsafe {
                SetWaitableTimer(timer, &first, (1000 / hz) as i32, None, null(), 0) != 0
            };
        }
        Pace {
            timer,
            period,
            repeats,
            next: Instant::now() + period,
            // Less than the period, so ticks stay in order.
            jitter: jitter.min(period.as_micros() as u64 / 2),
            random: 0x9e37_79b9_7f4a_7c15,
        }
    }

    /// Waits for the next tick.
    ///
    /// The timer wakes us on the system's clock, and so are woken the
    /// threads that sleep: looked at then, a thread that sleeps a
    /// millisecond at a time is always just waking (a test's sleeping
    /// thread was seen out of its wait 12 % of the time; it is 0.5 %). So
    /// the look is put off by a time that differs from tick to tick,
    /// spent spinning: no timer is that fine.
    fn wait(&mut self) {
        self.wait_for_timer();
        if self.jitter > 0 {
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            let until = Instant::now() + Duration::from_micros(self.random % (self.jitter + 1));
            while Instant::now() < until {
                std::hint::spin_loop();
            }
        }
    }

    fn wait_for_timer(&mut self) {
        use windows_sys::Win32::System::Threading::{INFINITE, SetWaitableTimer};
        if self.repeats {
            // SAFETY: our timer.
            unsafe { WaitForSingleObject(self.timer, INFINITE) };
            return;
        }
        let now = Instant::now();
        if self.next > now {
            let due = -((self.next - now).as_nanos() as i64 / 100).max(1);
            // SAFETY: our timer, set to fire once and waited for.
            let set = !self.timer.is_null()
                && unsafe { SetWaitableTimer(self.timer, &due, 0, None, null(), 0) } != 0;
            if set {
                // SAFETY: our timer.
                unsafe { WaitForSingleObject(self.timer, INFINITE) };
            } else {
                std::thread::sleep(self.next - now);
            }
        }
        // A tick that came late is not made up for: the next is a period on.
        self.next = (self.next + self.period).max(Instant::now());
    }
}

/// Starts the sampler; false if its thread could not be made.
fn start(config: Config) -> bool {
    use std::os::windows::io::IntoRawHandle;
    let says = format!(
        "profiling (VICTORY_PROFILE=1): {} looks a second at the submitting, recording and worker \
         threads, the report in {} every {:.0} s and at exit",
        config.hz,
        config.out.display(),
        config.every
    );
    match std::thread::Builder::new()
        .name("victory-profile".into())
        .spawn(move || run(config))
    {
        Ok(thread) => {
            SAMPLER.store(thread.into_raw_handle().cast(), Relaxed);
            // SAFETY: registers a function with the host CRT's exit table.
            unsafe { atexit(at_exit) };
            tracing::info!("{says}");
            true
        }
        Err(e) => {
            tracing::warn!(error = %e, "no thread for the profiler");
            false
        }
    }
}

fn run(config: Config) {
    // Above the threads it looks at: a late look stretches a sample, and
    // a sampler taken off its processor between suspending a thread and
    // letting it go leaves that thread standing. It sleeps most of the time.
    // SAFETY: a plain call on our own thread.
    unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL) };
    let wait = |seconds: f64| {
        let until = Instant::now() + Duration::from_secs_f64(seconds);
        while Instant::now() < until && !STOP.load(Relaxed) {
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    wait(config.after);
    let mut pace = Pace::new(config.hz, config.jitter);
    let (every, length) = (
        Duration::from_secs_f64(config.every),
        config.length.map(Duration::from_secs_f64),
    );
    let mut sampler = Sampler::new(config);
    sampler.rescan();
    sampler.resync();
    let began = Instant::now();
    let (mut scan, mut report) = (began + SCAN, began + every);
    while !STOP.load(Relaxed) && length.is_none_or(|length| began.elapsed() < length) {
        pace.wait();
        sampler.tick();
        let now = Instant::now();
        if now >= scan {
            sampler.rescan();
            scan = now + SCAN;
        }
        if now >= report {
            sampler.write();
            report = Instant::now() + every;
        }
    }
    sampler.write();
    DONE.store(true, Relaxed);
}

/// Between two searches for threads to follow.
const SCAN: Duration = Duration::from_secs(1);

#[cfg(test)]
mod tests;
