//! Software implementation of the Series XAPU single-stream Opus command queue.
mod commands;
mod dsp;
mod opus;

use commands::{Completion, Stream};
use std::{
    collections::{BTreeMap, VecDeque},
    ptr,
    sync::{LazyLock, Mutex, MutexGuard},
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};
use windows_sys::Win32::{
    Foundation::CloseHandle,
    System::{
        Diagnostics::Debug::OutputDebugStringA,
        Threading::{CreateEventW, SetEvent},
    },
};

const INVALID: i32 = 0x8007_0057u32 as i32;
const POINTER: i32 = 0x8000_4003u32 as i32;
const NO_MEMORY: i32 = 0x8007_000eu32 as i32;
const QUEUE_FULL: i32 = 0x8ac8_1000u32 as i32;
const PENDING: i32 = 0x8ac8_1001u32 as i32;
const BAD_STREAM: i32 = 0x8ac8_1003u32 as i32;
const NO_RESOURCES: i32 = 0x8ac8_1004u32 as i32;
const MAX_STREAMS: usize = 500;

struct Arena {
    base: usize,
    size: usize,
}

impl Arena {
    fn new(size: usize) -> Option<Self> {
        // The software queue gives each title its own page-aligned audio working memory.
        let base = unsafe {
            VirtualAlloc(
                ptr::null(),
                size.max(1),
                MEM_RESERVE | MEM_COMMIT,
                PAGE_READWRITE,
            )
        } as usize;
        (base != 0).then_some(Self { base, size })
    }

    fn contains(&self, address: usize, len: usize) -> bool {
        address
            .checked_sub(self.base)
            .is_some_and(|offset| offset <= self.size && len <= self.size - offset)
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        unsafe { VirtualFree(self.base as *mut _, 0, MEM_RELEASE) };
    }
}

struct Connection {
    arena: Arena,
    streams: Vec<Stream>,
    depth: usize,
    results: VecDeque<Completion>,
    submitted: u32,
    received: u32,
    event: usize,
}

impl Drop for Connection {
    fn drop(&mut self) {
        if self.event != 0 {
            unsafe { CloseHandle(self.event as *mut _) };
        }
    }
}

#[derive(Default)]
struct Connections {
    next: usize,
    live: BTreeMap<usize, Connection>,
}

impl Connections {
    fn stream_count(&self) -> usize {
        self.live
            .values()
            .map(|connection| connection.streams.len())
            .sum()
    }
}

static CONNECTIONS: LazyLock<Mutex<Connections>> = LazyLock::new(Mutex::default);
fn connections() -> MutexGuard<'static, Connections> {
    CONNECTIONS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// Guest records are packed and inputs can be unaligned. Outputs belong to the caller.
/// # Safety
/// A non-null config must hold six readable dwords. The memory and handle outputs must be writable and disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XApuConnect(
    config: *const [u32; 6],
    memory: *mut [usize; 2],
    handle: *mut usize,
) -> i32 {
    if handle.is_null() {
        return POINTER;
    }
    unsafe { handle.write_unaligned(0) };
    if memory.is_null() {
        return POINTER;
    }
    unsafe { memory.write_unaligned([0; 2]) };
    if config.is_null() {
        return INVALID;
    }
    let [version, stream_count, depth, bytes, overlap, flags] = unsafe { config.read_unaligned() };
    if version != 1
        || !(1..=500).contains(&stream_count)
        || !(1..=16).contains(&depth)
        || flags & !0xf != 0
    {
        return INVALID;
    }
    if bytes > 0x2000_0000 {
        return NO_MEMORY;
    }
    if overlap != 0 {
        return INVALID;
    }
    let mut all = connections();
    if all.stream_count() + stream_count as usize > MAX_STREAMS {
        return NO_RESOURCES;
    }
    let Some(id) = all.next.checked_add(1) else {
        return NO_RESOURCES;
    };
    let Some(arena) = Arena::new(bytes as usize) else {
        return NO_MEMORY;
    };
    let event = if flags & 8 != 0 {
        let event = unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) } as usize;
        if event == 0 {
            return NO_MEMORY;
        }
        event
    } else {
        0
    };
    let base = arena.base;
    let streams = (0..stream_count).map(|_| Stream::default()).collect();
    all.next = id;
    all.live.insert(
        id,
        Connection {
            arena,
            streams,
            depth: depth as usize,
            results: VecDeque::new(),
            submitted: 0,
            received: 0,
            event,
        },
    );
    unsafe {
        memory.write_unaligned([base, event]);
        handle.write_unaligned(id);
        OutputDebugStringA(
            c"TuxBox xapu: software Opus connection ready\n"
                .as_ptr()
                .cast(),
        );
    }
    0
}

#[unsafe(no_mangle)]
pub extern "system" fn XApuDisconnect(handle: usize) -> i32 {
    if connections().live.remove(&handle).is_some() {
        0
    } else {
        INVALID
    }
}

/// # Safety
/// The command must be readable for its record type. Referenced audio memory must remain exclusively accessible during submission. A non-null sequence must be writable.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XApuEnqueueCommand(
    handle: usize,
    command: *const u8,
    sequence: *mut u32,
) -> i32 {
    if command.is_null() {
        return POINTER;
    }
    let mut all = connections();
    let Some(connection) = all.live.get_mut(&handle) else {
        return INVALID;
    };
    let stream = unsafe { command.add(4).cast::<u32>().read_unaligned() } as usize;
    let Some(state) = connection.streams.get_mut(stream) else {
        return BAD_STREAM;
    };
    if state.pending >= connection.depth {
        return QUEUE_FULL;
    }
    let completion = match unsafe { state.submit(command, &connection.arena) } {
        Ok(completion) => completion,
        Err(status) => return status,
    };
    state.pending += 1;
    connection.results.push_back(completion);
    if connection.event != 0 {
        unsafe { SetEvent(connection.event as *mut _) };
    }
    connection.submitted = connection.submitted.wrapping_add(1);
    if !sequence.is_null() {
        unsafe { sequence.write_unaligned(connection.submitted) };
    }
    0
}

/// # Safety
/// A non-null received pointer must be writable for one DWORD.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XApuGetResultCount(handle: usize, received: *mut u32) -> u32 {
    let all = connections();
    let Some(connection) = all.live.get(&handle) else {
        return 0;
    };
    if !received.is_null() {
        unsafe { received.write_unaligned(connection.received) };
    }
    connection.results.len() as u32
}

/// # Safety
/// The output must have room for an 80-byte process result or a 48-byte control result as appropriate for the next completion.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XApuDequeueResult(handle: usize, result: *mut u8) -> i32 {
    if result.is_null() {
        return POINTER;
    }
    let mut all = connections();
    let Some(connection) = all.live.get_mut(&handle) else {
        return INVALID;
    };
    let Some(completion) = connection.results.pop_front() else {
        return PENDING;
    };
    let stream = &mut connection.streams[completion.stream()];
    stream.pending -= 1;
    if completion.kind() == 2 {
        stream.decoder = None;
    }
    let status = unsafe { completion.write_to(result) };
    connection.received = connection.received.wrapping_add(1);
    status
}

/// # Safety
/// A non-null counts pointer must be writable for two DWORDs.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn XApuGetUtilization(
    version: u32,
    category: u32,
    counts: *mut [u32; 2],
) -> i32 {
    if counts.is_null() {
        return POINTER;
    }
    unsafe { counts.write_unaligned([0; 2]) };
    if !(1..=3).contains(&version) || !(1..=2).contains(&category) {
        return INVALID;
    }
    let free = MAX_STREAMS - connections().stream_count();
    unsafe { counts.write_unaligned([free as u32, MAX_STREAMS as u32]) };
    0
}
