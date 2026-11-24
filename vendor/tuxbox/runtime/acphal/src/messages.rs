//! IAcpHal coprocessor connection and message queue (TUXBOX_ACP_MESSAGES=0:
//! the slots stay inert counters). Facts (re-derived, not copied):
//!   slot 0 connect(this, command queue size, message queue size, flags)
//!   slot 1 disconnect(this)               -> posts DISCONNECTED (0x80)
//!   slot 2 submit(this, command, id, frame, data, notification) (xma.rs)
//!   slot 3 pop_message(this, out) -> bool (28-byte message)
//!   slot 4 message_count(this) -> u32
//! Command 2 registers message kinds (data word 0 is the mask), 3 removes
//! them, 1 loads a flowgraph (completes at once: FLOWGRAPH_COMPLETED 0x02 with
//! data word 1). A command with a non-zero id is acknowledged with
//! COMMAND_COMPLETED (0x10): command, id, audio frame (128 samples at 48 kHz).
//! A full queue drops the message and counts it in the next one queued.
use core::ffi::c_void;
use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

const LOAD_FLOWGRAPH: u32 = 1;
const REGISTER_MESSAGES: u32 = 2;
const UNREGISTER_MESSAGES: u32 = 3;
const FLOWGRAPH_COMPLETED: u32 = 0x02;
const COMMAND_COMPLETED: u32 = 0x10;
const DISCONNECTED: u32 = 0x80;
const FRAMES_PER_SECOND: u64 = 375;
const E_INVALIDARG: i32 = 0x8007_0057u32 as i32;
const E_FAIL: i32 = 0x8000_4005u32 as i32;

#[repr(C, packed)]
#[derive(Clone, Copy)]
struct Message {
    kind: u32,
    dropped: u32,
    usec: u32,
    data: [u8; 16],
}

struct State {
    connected: bool,
    mask: u32,
    capacity: usize,
    dropped: u32,
    queue: VecDeque<Message>,
    posted: u64,
    popped: u64,
}

static STATE: Mutex<State> = Mutex::new(State {
    connected: false,
    mask: 0,
    capacity: 0,
    dropped: 0,
    queue: VecDeque::new(),
    posted: 0,
    popped: 0,
});

fn since() -> Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    *START.get_or_init(Instant::now)
}

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("TUXBOX_ACP_MESSAGES").is_none_or(|v| v != "0");
        crate::probe::log(format_args!("coprocessor messages {}", if on { "on (TUXBOX_ACP_MESSAGES=0 turns them off)" } else { "off" }));
        on
    })
}

fn usec() -> u64 {
    since().elapsed().as_micros() as u64
}

fn post(s: &mut State, kind: u32, data: &[u8]) {
    if s.mask & kind == 0 {
        return;
    }
    if s.queue.len() >= s.capacity.max(1) {
        s.dropped = s.dropped.saturating_add(1);
        return;
    }
    let mut message = Message { kind, dropped: std::mem::take(&mut s.dropped), usec: usec() as u32, data: [0; 16] };
    message.data[..data.len().min(16)].copy_from_slice(&data[..data.len().min(16)]);
    s.queue.push_back(message);
    s.posted += 1;
    if s.posted <= 4 {
        crate::probe::log(format_args!("coprocessor message {kind:#x} queued ({} waiting)", s.queue.len()));
    }
}

/// From submit (slot 2): message registration, flowgraph and completion.
pub(crate) fn command(command: u32, id: u64, word: &dyn Fn(usize) -> Option<u32>) {
    if !enabled() {
        return;
    }
    let mut s = state();
    match command {
        REGISTER_MESSAGES => {
            s.mask |= word(0).unwrap_or(0);
            crate::probe::log(format_args!("coprocessor messages registered: mask {:#x}", s.mask));
        }
        UNREGISTER_MESSAGES => s.mask &= !word(0).unwrap_or(0),
        LOAD_FLOWGRAPH => {
            let flowgraph = word(1).unwrap_or(0);
            post(&mut s, FLOWGRAPH_COMPLETED, &flowgraph.to_le_bytes());
        }
        _ => {}
    }
    if id != 0 {
        let mut done = [0u8; 16];
        done[..4].copy_from_slice(&command.to_le_bytes());
        done[4..12].copy_from_slice(&id.to_le_bytes());
        done[12..].copy_from_slice(&((usec() * FRAMES_PER_SECOND / 1_000_000) as u32).to_le_bytes());
        post(&mut s, COMMAND_COMPLETED, &done);
    }
}

pub(crate) unsafe extern "system" fn connect(this: *mut c_void, commands: usize, messages: usize, flags: usize) -> i32 {
    let probe = unsafe { crate::probe::method::<0>(this, commands, messages, flags) };
    if !enabled() {
        return probe;
    }
    if commands as u32 == 0 {
        return E_INVALIDARG;
    }
    since();
    let mut s = state();
    s.connected = true;
    s.capacity = messages as u32 as usize;
    0
}

pub(crate) unsafe extern "system" fn disconnect(this: *mut c_void, a: usize, b: usize, c: usize) -> i32 {
    let probe = unsafe { crate::probe::method::<1>(this, a, b, c) };
    if !enabled() {
        return probe;
    }
    let mut s = state();
    if !s.connected {
        return E_FAIL;
    }
    post(&mut s, DISCONNECTED, &[]);
    s.connected = false;
    0
}

pub(crate) unsafe extern "system" fn pop_message(this: *mut c_void, out: usize, b: usize, c: usize) -> i32 {
    let probe = unsafe { crate::probe::method::<3>(this, out, b, c) };
    if !enabled() {
        return probe;
    }
    let mut s = state();
    let Some(message) = s.queue.pop_front() else { return 0 };
    s.popped += 1;
    if out != 0 {
        // SAFETY: the title passes room for one message.
        unsafe { (out as *mut Message).write_unaligned(message) };
    }
    1
}

pub(crate) unsafe extern "system" fn message_count(this: *mut c_void, a: usize, b: usize, c: usize) -> i32 {
    let probe = unsafe { crate::probe::method::<4>(this, a, b, c) };
    if !enabled() {
        return probe;
    }
    state().queue.len() as i32
}
