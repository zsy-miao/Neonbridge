//! Instructions the console's CPU (Zen 2) has and an Intel PC's lacks, emulated: the title's
//! code raises STATUS_ILLEGAL_INSTRUCTION on them (the title zeroes memory with a `clzero` loop at
//! its first frames), and the vectored handler steps over them here.
//!
//! Ported from XCL (github.com/ivall/xcl, GPL-2.0-only): `insn.rs` (decoder, as
//! `console_insn_decode.rs`, unchanged) and `eh.rs` `emulate_console_only`.

#[path = "console_insn_decode.rs"]
mod decode;

use decode::ConsoleOnly;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use windows_sys::Win32::System::Diagnostics::Debug::CONTEXT;

const CONTINUE_EXECUTION: i32 = -1;

/// Whether what Windows said about memory is remembered per thread (the regions it found
/// writable, the code bytes it found readable) and not asked again at every emulated
/// instruction. A query walks the pages of the region it answers for: asked from outside, it
/// took 2-9 ms in the title's heaps of 0.6-2.2 GB and 89 ms in its 6.2 GB page pool (run 30),
/// and the title zeroes blocks of them with `clzero` loops all through a frame: six of its
/// threads spent 25-78 % of their time inside that query (runs 29 and 30), about 27 s of
/// processor time in Windows per 9 s frame. On by default; `VICTORY_CLZERO_ASK=1` asks every
/// time, as before.
fn remembers() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CLZERO_ASK").is_none_or(|v| v != "1"))
}

/// Answers a remembered region gives before Windows is asked about it again: one freed or
/// protected meanwhile is forgotten then.
const ANSWERS: u32 = 4096;

thread_local! {
    /// The regions Windows last said are committed and writable (start, end), newest first,
    /// and the answers they gave since they were emptied.
    static WRITABLE: Cell<([(usize, usize); 4], u32)> = const { Cell::new(([(0, 0); 4], 0)) };
    /// Faulting addresses and the code bytes found readable there, newest first.
    static CODE: Cell<[(usize, usize); 8]> = const { Cell::new([(0, 0); 8]) };
}

/// Instructions emulated, the times Windows was asked about memory for them, and the bytes
/// `clzero` loops zeroed, since the start.
static EMULATED: AtomicU64 = AtomicU64::new(0);
static ASKED: AtomicU64 = AtomicU64::new(0);
static ZEROED: AtomicU64 = AtomicU64::new(0);

/// Exceptions taken by kind since the last line: a lone `clzero`, the 12-byte loop, the
/// 15-byte loop taken whole, `monitorx`, `mwaitx`, everything else; and the 64-byte lines the
/// 15-byte loops zeroed (each was an exception of its own before they were taken whole).
static KINDS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
const K_CLZERO: usize = 0;
const K_LOOP12: usize = 1;
const K_LOOP15: usize = 2;
const K_MONITORX: usize = 3;
const K_MWAITX: usize = 4;
const K_OTHER: usize = 5;
const K_LOOP15_LINES: usize = 6;

/// Waits the title made with `monitorx` / `mwaitx` (it waits for a fence that way, reading
/// the fence's value in memory: its +0x4fb0ad0) that have ended since the last line: how
/// many, their microseconds, and both again for those of 1 ms and more, 5 ms and more,
/// 20 ms and more.
static WAITS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Waits said in the log one by one.
static WAITS_SAID: AtomicU64 = AtomicU64::new(0);
/// When the last line was written, in milliseconds of [`clock_us`].
static LINE_AT: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The address and the value this thread last armed a monitor for (`monitorx`: rax, and
    /// the title's wanted value in rsi).
    static MONITOR: Cell<(usize, u64)> = const { Cell::new((0, 0)) };
    /// This thread's wait under way: when it began and when its last `mwaitx` was, in
    /// microseconds, what it waits for, and its `mwaitx` count.
    static WAIT: Cell<(u64, u64, usize, u64, u32)> = const { Cell::new((0, 0, 0, 0, 0)) };
}

/// Microseconds on the performance counter.
fn clock_us() -> u64 {
    static PER_SECOND: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let per_second = *PER_SECOND.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: a valid out pointer.
        unsafe { windows_sys::Win32::System::Performance::QueryPerformanceFrequency(&mut f) };
        f.max(1)
    });
    let mut now = 0i64;
    // SAFETY: a valid out pointer.
    unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut now) };
    (now as u128 * 1_000_000 / per_second as u128) as u64
}

/// Whether events are said one by one now: four seconds of every two minutes of the wall
/// clock, the same seconds in which the fence meter of the graphics side says its own
/// (`victory_gpu::fence::pace`), so that the two can be laid side by side.
fn detail_now() -> bool {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).is_ok_and(|d| d.as_secs() % 120 < 4)
}

/// Whether the 15-byte `clzero` loop is taken whole (`VICTORY_CLZERO_LOOP15=0`: line by
/// line, as before).
fn loop15() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_CLZERO_LOOP15").is_none_or(|v| v != "0"))
}

/// A wait of this thread has ended: counted, and said if it is one to say.
fn wait_ended(begin: u64, last: u64, address: usize, wanted: u64, spins: u32) {
    let us = last.saturating_sub(begin);
    for (i, least) in [0u64, 1_000, 5_000, 20_000].into_iter().enumerate() {
        if us >= least {
            WAITS[2 * i].fetch_add(1, Relaxed);
            WAITS[2 * i + 1].fetch_add(us, Relaxed);
        }
    }
    if us >= 500 && (detail_now() || WAITS_SAID.fetch_add(1, Relaxed) < 40) {
        // SAFETY: no preconditions.
        let thread = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
        tracing::info!(
            "mwait: thread {thread} waited {:.1} ms for the value at {address:#x} to reach {wanted} ({spins} mwaitx)",
            us as f64 / 1e3
        );
    }
}

/// A `mwaitx` of this thread: part of the wait under way, or the first of a new one.
fn wait_step() {
    let now = clock_us();
    let (address, wanted) = MONITOR.with(Cell::get);
    let (begin, last, was_at, was_for, spins) = WAIT.with(Cell::get);
    if begin != 0 && was_at == address && was_for == wanted && now.saturating_sub(last) < 2_000 {
        WAIT.with(|w| w.set((begin, now, address, wanted, spins + 1)));
        return;
    }
    if begin != 0 {
        wait_ended(begin, last, was_at, was_for, spins);
    }
    WAIT.with(|w| w.set((now, now, address, wanted, 1)));
}

/// A line every five seconds: exceptions a second by kind, and the waits that ended.
fn line() {
    let now = clock_us() / 1000;
    let last = LINE_AT.load(Relaxed);
    if now < last + 5_000 || LINE_AT.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    if last == 0 {
        return;
    }
    let seconds = (now - last) as f64 / 1e3;
    let k: Vec<f64> = KINDS.iter().map(|k| k.swap(0, Relaxed) as f64 / seconds).collect();
    let w: Vec<f64> = WAITS.iter().map(|w| w.swap(0, Relaxed) as f64).collect();
    tracing::info!(
        "console CPU instructions, exceptions a second: clzero alone {:.0}, its 12-byte loop {:.0}, its 15-byte loop taken whole {:.0} ({:.0} lines a second, each an exception \
         before; VICTORY_CLZERO_LOOP15=0: again), monitorx {:.0}, mwaitx {:.0}, others {:.0} | mwaitx waits ended a second: {:.0}, {:.1} ms of thread time a second; of 1 ms and \
         more {:.1} ({:.1} ms), of 5 ms and more {:.1} ({:.1} ms), of 20 ms and more {:.1} ({:.1} ms)",
        k[K_CLZERO],
        k[K_LOOP12],
        k[K_LOOP15],
        k[K_LOOP15_LINES],
        k[K_MONITORX],
        k[K_MWAITX],
        k[K_OTHER],
        w[0] / seconds,
        w[1] / seconds / 1e3,
        w[2] / seconds,
        w[3] / seconds / 1e3,
        w[4] / seconds,
        w[5] / seconds / 1e3,
        w[6] / seconds,
        w[7] / seconds / 1e3,
    );
}

fn readable(va: usize, bytes: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let found = unsafe { VirtualQuery(va as *const _, &mut m, size_of_val(&m)) } != 0;
    ASKED.fetch_add(1, Relaxed);
    found && m.State == MEM_COMMIT && m.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && m.BaseAddress as usize + m.RegionSize >= va + bytes
}

fn writable(va: usize, bytes: usize) -> bool {
    writable_to(va, va + bytes) == Some(va + bytes)
}

/// Where the committed, writable memory that `va` lies in ends, at most `end`; None if `va`
/// is not in such memory. A region found is remembered ([`remembers`]).
/// shortcut: a remembered region that was freed or protected since is written all the same
/// for up to [`ANSWERS`] answers: the write then faults inside the handler, which the guard
/// on view memory opens as it does any write, and which is a crash otherwise (as the
/// unchecked single-line emulation behind this one already is).
fn writable_to(va: usize, end: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQuery};
    let remember = remembers();
    if remember {
        let (regions, answers) = WRITABLE.with(Cell::get);
        if let Some(&(_, to)) = regions.iter().find(|&&(from, to)| from <= va && va < to) {
            let next = if answers >= ANSWERS { ([(0, 0); 4], 0) } else { (regions, answers + 1) };
            WRITABLE.with(|w| w.set(next));
            return Some(to.min(end));
        }
    }
    // SAFETY: plain data filled in by the query.
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let found = unsafe { VirtualQuery(va as *const _, &mut m, size_of_val(&m)) } != 0;
    ASKED.fetch_add(1, Relaxed);
    if !found || m.State != MEM_COMMIT || m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 {
        return None;
    }
    let (from, to) = (m.BaseAddress as usize, m.BaseAddress as usize + m.RegionSize);
    if remember {
        WRITABLE.with(|w| {
            let (mut regions, answers) = w.get();
            regions.rotate_right(1);
            regions[0] = (from, to);
            w.set((regions, answers));
        });
    }
    Some(to.min(end))
}

/// The register `r` of `ctx` (CONTEXT order: rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8..r15).
fn register(ctx: &mut CONTEXT, r: u8) -> &mut u64 {
    match r & 15 {
        0 => &mut ctx.Rax,
        1 => &mut ctx.Rcx,
        2 => &mut ctx.Rdx,
        3 => &mut ctx.Rbx,
        4 => &mut ctx.Rsp,
        5 => &mut ctx.Rbp,
        6 => &mut ctx.Rsi,
        7 => &mut ctx.Rdi,
        8 => &mut ctx.R8,
        9 => &mut ctx.R9,
        10 => &mut ctx.R10,
        11 => &mut ctx.R11,
        12 => &mut ctx.R12,
        13 => &mut ctx.R13,
        14 => &mut ctx.R14,
        _ => &mut ctx.R15,
    }
}

/// Where the lines of the loop `mov rax, ptr; add ptr, 0x40; clzero; cmp ptr, end; jb` end,
/// from the state the loop has at its `clzero`: the line at `rax` is zeroed now, and one
/// more at `ptr` for every pass the `cmp` sends back. None if this is not the plain case
/// the loop makes (the line at `rax` on a 64-byte boundary, `ptr` the next line, under
/// 2 GiB in all): then every line is taken alone, as the instruction itself does it.
fn loop15_end(rax: usize, ptr: usize, end: usize) -> Option<usize> {
    if rax & 63 != 0 || ptr != rax.wrapping_add(64) || ptr < rax {
        return None;
    }
    let more = if ptr < end { (end - ptr).div_ceil(64) } else { 0 };
    let last = ptr.checked_add(more.checked_mul(64)?)?;
    (last - rax < 2 << 30).then_some(last)
}

/// The `clzero` at `rip` in the loop `mov rax, ptr; add ptr, 0x40; clzero; cmp ptr, end; jb`,
/// with all the lines the loop would still zero: `Some(true)` when they are zeroed and the
/// registers stand as the loop leaves them at its last `clzero` (the caller steps over the
/// instruction; the title's own `cmp` and `jb` then leave the loop), `Some(false)` when the
/// first line is not writable (the real instruction faults there too), and when only a
/// first part was writable the registers stand at the line after it, where the instruction
/// runs again. None if it is not that loop: the lone instruction is emulated.
///
/// # Safety
/// `rip` is the faulting instruction of `ctx`, whose bytes were read.
unsafe fn clzero_loop15(ctx: &mut CONTEXT, rip: usize, ptr_reg: u8, end_reg: u8) -> Option<bool> {
    // The seven bytes in front of it: in the same page as the instruction, which ran.
    if rip & 0xFFF < 7 && !readable(rip - 7, 7) {
        return None;
    }
    // SAFETY: readable, as checked or as the instruction's own page.
    let before = unsafe { std::slice::from_raw_parts((rip - 7) as *const u8, 7) };
    if !decode::loop15_head(before, ptr_reg) || end_reg == 0 || end_reg == ptr_reg {
        return None;
    }
    let rax = ctx.Rax as usize;
    let ptr = *register(ctx, ptr_reg) as usize;
    let end = *register(ctx, end_reg) as usize;
    let last = loop15_end(rax, ptr, end)?;
    // Region by region, as the 12-byte loop: a block may lie across several views.
    let mut at = rax;
    while at < last {
        let Some(to) = writable_to(at, last) else { break };
        // A region ends on a page boundary, so `to` is on a line boundary as `at` is.
        // SAFETY: committed and writable from `at` to `to`.
        unsafe { std::ptr::write_bytes(at as *mut u8, 0, to - at) };
        at = to;
    }
    if at == rax {
        return Some(false);
    }
    ZEROED.fetch_add((at - rax) as u64, Relaxed);
    KINDS[K_LOOP15_LINES].fetch_add(((at - rax) / 64) as u64, Relaxed);
    if at < last {
        // What follows is not writable: the loop stands at that line's `clzero` now, where
        // the real one faults too. The instruction runs again (the caller must not step).
        ctx.Rax = at as u64;
        *register(ctx, ptr_reg) = (at + 64) as u64;
        ctx.Rip = (rip as u64).wrapping_sub(3);
        return Some(true);
    }
    ctx.Rax = (last - 64) as u64;
    *register(ctx, ptr_reg) = last as u64;
    Some(true)
}

/// An illegal instruction at `ctx.Rip`: a console-only one is emulated and stepped over
/// (`CONTINUE_EXECUTION`); anything else returns 0 (the crash report and the title's handlers
/// see it).
///
/// # Safety
/// `ctx` is the faulting thread's context, from the exception being dispatched.
pub(crate) unsafe fn emulate(ctx: &mut CONTEXT) -> i32 {
    let rip = ctx.Rip as usize;
    // The same few instructions fault again and again: their bytes are asked about once.
    let known = if remembers() { CODE.with(Cell::get) } else { [(0, 0); 8] };
    let n = match known.iter().find(|&&(at, n)| at == rip && n > 0) {
        Some(&(_, n)) => n,
        None => {
            if !readable(rip, 1) {
                return 0;
            }
            let n = (1..=16).rev().find(|&n| readable(rip, n)).unwrap_or(0);
            if remembers() {
                let mut known = known;
                known.rotate_right(1);
                known[0] = (rip, n);
                CODE.with(|c| c.set(known));
            }
            n
        }
    };
    // SAFETY: `n` bytes at `rip` are readable.
    let code = unsafe { std::slice::from_raw_parts(rip as *const u8, n) };
    let Some((what, len)) = decode::decode(code) else { return 0 };
    static SEEN: std::sync::Mutex<Vec<std::mem::Discriminant<ConsoleOnly>>> = std::sync::Mutex::new(Vec::new());
    if let Ok(mut seen) = SEEN.lock()
        && !seen.contains(&std::mem::discriminant(&what))
    {
        seen.push(std::mem::discriminant(&what));
        if what != ConsoleOnly::Ud2 {
            tracing::warn!(instruction = ?what, at = format_args!("{rip:#x}"), "a console CPU instruction this CPU lacks: emulated (logged once)");
        }
    }
    match what {
        ConsoleOnly::Ud2 => return 0,
        ConsoleOnly::Clwb => {
            // nothing to write back on the PC: patched to NOPs, no exception next time
            use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
            let mut old = 0u32;
            // SAFETY: the instruction's own bytes, made writable for the patch and restored.
            unsafe {
                if VirtualProtect(rip as *const _, len, PAGE_EXECUTE_READWRITE, &mut old) != 0 {
                    std::slice::from_raw_parts_mut(rip as *mut u8, len).fill(0x90);
                    VirtualProtect(rip as *const _, len, old, &mut old);
                }
            }
        }
        ConsoleOnly::Monitorx => {
            KINDS[K_MONITORX].fetch_add(1, Relaxed);
            MONITOR.with(|m| m.set((ctx.Rax as usize, ctx.Rsi)));
        }
        ConsoleOnly::Mwaitx => {
            KINDS[K_MWAITX].fetch_add(1, Relaxed);
            wait_step();
            std::hint::spin_loop()
        }
        ConsoleOnly::Clzero | ConsoleOnly::ClzeroStep { .. } => {
            // The loop `mov rax, ptr; add ptr, 0x40; clzero; cmp ptr, end; jb` (the title's
            // +0x4e8ab4c and three more like it zero 896 to 7,040 bytes each, a line an
            // exception) is taken whole from the line it stands at.
            if let ConsoleOnly::ClzeroStep { ptr_reg, end_reg } = what
                && loop15()
                && let Some(done) = unsafe { clzero_loop15(ctx, rip, ptr_reg, end_reg) }
            {
                if !done {
                    return 0;
                }
                ctx.Rip += len as u64;
                EMULATED.fetch_add(1, Relaxed);
                KINDS[K_LOOP15].fetch_add(1, Relaxed);
                line();
                return CONTINUE_EXECUTION;
            }
            KINDS[K_CLZERO].fetch_add(1, Relaxed);
            // with an address-size prefix (0x67) the line is at EAX, zero-extended
            let a32 = code[..len - 3].contains(&0x67);
            let line = (if a32 { ctx.Rax & 0xFFFF_FFFF } else { ctx.Rax } as usize) & !63;
            if !writable(line, 64) {
                return 0; // the real instruction faults there too
            }
            // SAFETY: the 64-byte line is committed and writable.
            unsafe { std::ptr::write_bytes(line as *mut u8, 0, 64) };
        }
        ConsoleOnly::ClzeroLoop { end_reg } => {
            KINDS[K_LOOP12].fetch_add(1, Relaxed);
            // the whole `clzero; add rax,40h; cmp rax,end; jb` loop at once
            let regs: [u64; 16] = [
                ctx.Rax, ctx.Rcx, ctx.Rdx, ctx.Rbx, ctx.Rsp, ctx.Rbp, ctx.Rsi, ctx.Rdi, ctx.R8, ctx.R9, ctx.R10, ctx.R11, ctx.R12, ctx.R13, ctx.R14, ctx.R15,
            ];
            let end = regs[end_reg as usize & 15] as usize;
            let start = ctx.Rax as usize;
            if end > start && end - start < 2 << 30 {
                // Region by region: a block may lie across several (the title's pages are
                // mapped one view each), and one answer for all of it was a refusal, after
                // which every 64-byte line took an exception of its own.
                let mut at = start;
                while at < end {
                    let Some(to) = writable_to(at, end) else { break };
                    // SAFETY: committed and writable from `at` to `to`.
                    unsafe { std::ptr::write_bytes(at as *mut u8, 0, to - at) };
                    at = to;
                }
                if at == start {
                    return 0;
                }
                ZEROED.fetch_add((at - start) as u64, Relaxed);
                if at < end {
                    // What follows is not writable: the loop goes on there (the same
                    // instruction again), where the real one faults too.
                    ctx.Rax = at as u64;
                    return CONTINUE_EXECUTION;
                }
            }
            ctx.Rax = end as u64;
        }
        ConsoleOnly::Rdpru => {
            let mut v = 0i64;
            // SAFETY: a valid out pointer.
            unsafe { windows_sys::Win32::System::Performance::QueryPerformanceCounter(&mut v) };
            let v = if ctx.Rcx as u32 <= 1 { v as u64 } else { 0 };
            ctx.Rax = v & 0xFFFF_FFFF;
            ctx.Rdx = v >> 32;
        }
        ConsoleOnly::Rdpid(r) => {
            // SAFETY: no preconditions.
            let cpu = unsafe { windows_sys::Win32::System::Threading::GetCurrentProcessorNumber() } as u64;
            let regs: [&mut u64; 16] = [
                &mut ctx.Rax, &mut ctx.Rcx, &mut ctx.Rdx, &mut ctx.Rbx, &mut ctx.Rsp, &mut ctx.Rbp, &mut ctx.Rsi, &mut ctx.Rdi, &mut ctx.R8, &mut ctx.R9, &mut ctx.R10, &mut ctx.R11, &mut ctx.R12, &mut ctx.R13,
                &mut ctx.R14, &mut ctx.R15,
            ];
            *regs[r as usize & 15] = cpu;
        }
        // the low quadword changes; the high one is zeroed, as a Zen 5 does
        ConsoleOnly::Extrq { dst, src, len, idx } => {
            // SAFETY: the context's XMM registers (CONTEXT_FLOATING_POINT is captured for exceptions).
            let x = unsafe { &mut ctx.Anonymous.FltSave.XmmRegisters };
            let (len, idx) = match src {
                Some(s) => {
                    let c = x[s as usize & 15].Low;
                    (c as u8, (c >> 8) as u8)
                }
                None => (len, idx),
            };
            let d = &mut x[dst as usize & 15];
            d.Low = decode::extrq(d.Low, len, idx);
            d.High = 0;
        }
        ConsoleOnly::Insertq { dst, src, reg, len, idx } => {
            // SAFETY: as above.
            let x = unsafe { &mut ctx.Anonymous.FltSave.XmmRegisters };
            let s = x[src as usize & 15];
            let (len, idx) = if reg { (s.High as u8, (s.High >> 8) as u8) } else { (len, idx) };
            let d = &mut x[dst as usize & 15];
            d.Low = decode::insertq(d.Low, s.Low, len, idx);
            d.High = 0;
        }
    }
    if !matches!(
        what,
        ConsoleOnly::Monitorx | ConsoleOnly::Mwaitx | ConsoleOnly::Clzero | ConsoleOnly::ClzeroStep { .. } | ConsoleOnly::ClzeroLoop { .. }
    ) {
        KINDS[K_OTHER].fetch_add(1, Relaxed);
    }
    line();
    ctx.Rip += len as u64;
    // How often this runs at all (each one is an exception: a trip into Windows and back).
    let emulated = EMULATED.fetch_add(1, Relaxed) + 1;
    if emulated >= 1 << 16 && emulated.is_power_of_two() {
        tracing::info!(
            emulated,
            asked = ASKED.load(Relaxed),
            zeroed_mb = ZEROED.load(Relaxed) >> 20,
            "console CPU instructions emulated so far, the times Windows was asked about memory for them, and what clzero loops zeroed"
        );
    }
    CONTINUE_EXECUTION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What the title's loop leaves, run line by line as the processor would: the lines it
    /// zeroed and the registers (rax, ptr) at the end, from the state at its first `clzero`.
    fn by_hand(mut rax: usize, mut ptr: usize, end: usize) -> (Vec<usize>, usize, usize) {
        let mut lines = vec![rax & !63];
        while ptr < end {
            rax = ptr;
            ptr += 0x40;
            lines.push(rax & !63);
        }
        (lines, rax, ptr)
    }

    #[test]
    fn the_15_byte_loop_ends_where_the_processor_would_end_it() {
        for (rax, len) in [(0x1000usize, 7040usize), (0x1000, 3264), (0x1000, 3776), (0x1000, 896), (0x4000, 64), (0x4000, 65), (0x4000, 100), (0x4000, 1), (0x4000, 0), (0x8000, 128)] {
            let (ptr, end) = (rax + 64, rax + len);
            let (lines, last_rax, last_ptr) = by_hand(rax, ptr, end);
            let last = loop15_end(rax, ptr, end).expect("the plain case");
            assert_eq!(last, last_ptr, "rax {rax:#x} len {len}");
            assert_eq!(last - 64, last_rax, "rax {rax:#x} len {len}");
            assert_eq!((last - rax) / 64, lines.len(), "rax {rax:#x} len {len}");
            assert_eq!(*lines.last().unwrap(), last - 64);
        }
        // an end before the pointer: the line at rax alone
        assert_eq!(loop15_end(0x1000, 0x1040, 0x800), Some(0x1040));
        // not the plain case: a line not on a boundary, a pointer that is not the next line
        assert_eq!(loop15_end(0x1008, 0x1048, 0x2000), None);
        assert_eq!(loop15_end(0x1000, 0x1080, 0x2000), None);
        assert_eq!(loop15_end(0x1000, 0x1040, 0x1000 + (3 << 30)), None, "over 2 GiB");
    }

    /// The loop as the title has it at +0x4e8ab4c, for the given registers.
    fn code(ptr_reg: u8, end_reg: u8) -> Vec<u8> {
        let (ph, pl, eh, el) = (ptr_reg >> 3, ptr_reg & 7, end_reg >> 3, end_reg & 7);
        vec![
            0x48 | ph << 2, 0x89, 0xC0 | pl << 3, // mov rax, ptr
            0x48 | ph, 0x83, 0xC0 | pl, 0x40, // add ptr, 0x40
            0x0F, 0x01, 0xFC, // clzero
            0x48 | eh << 2 | ph, 0x39, 0xC0 | el << 3 | pl, // cmp ptr, end
            0x72, 0xF1, // jb -15
            0x90,
        ]
    }

    #[test]
    fn the_15_byte_loop_is_zeroed_whole_and_leaves_the_loop_s_registers() {
        for (ptr_reg, end_reg, len) in [(10u8, 11u8, 7040usize), (9, 10, 3264), (8, 9, 3776), (2, 8, 896), (10, 11, 64), (10, 11, 100)] {
            let code = code(ptr_reg, end_reg);
            // a block of 0xAA with room on both sides, its start on a line boundary
            let mut block = vec![0xAAu8; 16384];
            let base = (block.as_mut_ptr() as usize + 4096 + 63) & !63;
            // SAFETY: plain data.
            let mut ctx: CONTEXT = unsafe { std::mem::zeroed() };
            ctx.Rip = code.as_ptr() as u64 + 7;
            ctx.Rax = base as u64;
            *register(&mut ctx, ptr_reg) = base as u64 + 64;
            *register(&mut ctx, end_reg) = (base + len) as u64;
            // SAFETY: the context points at the code above.
            assert_eq!(unsafe { emulate(&mut ctx) }, CONTINUE_EXECUTION);
            let (lines, last_rax, last_ptr) = by_hand(base, base + 64, base + len);
            assert_eq!(ctx.Rip, code.as_ptr() as u64 + 10, "at the loop's own cmp");
            assert_eq!(ctx.Rax as usize, last_rax);
            assert_eq!(*register(&mut ctx, ptr_reg) as usize, last_ptr);
            assert_eq!(*register(&mut ctx, end_reg) as usize, base + len, "the end is left alone");
            let zeroed = lines.len() * 64;
            let at = base - block.as_ptr() as usize;
            assert!(block[at..at + zeroed].iter().all(|&b| b == 0), "{zeroed} bytes zeroed");
            assert!(block[..at].iter().all(|&b| b == 0xAA), "nothing before the block");
            assert!(block[at + zeroed..].iter().all(|&b| b == 0xAA), "nothing after its last line");
        }
    }

    #[test]
    fn a_clzero_that_only_looks_like_the_loop_is_one_line() {
        // the same bytes behind it, another head in front (add ptr, 0x80): one line, one step
        let mut code = code(10, 11);
        code[6] = 0x80;
        let mut block = vec![0xAAu8; 8192];
        let base = (block.as_mut_ptr() as usize + 2048 + 63) & !63;
        // SAFETY: plain data.
        let mut ctx: CONTEXT = unsafe { std::mem::zeroed() };
        ctx.Rip = code.as_ptr() as u64 + 7;
        ctx.Rax = base as u64;
        ctx.R10 = base as u64 + 64;
        ctx.R11 = base as u64 + 640;
        // SAFETY: the context points at the code above.
        assert_eq!(unsafe { emulate(&mut ctx) }, CONTINUE_EXECUTION);
        assert_eq!(ctx.Rip, code.as_ptr() as u64 + 10);
        assert_eq!((ctx.Rax, ctx.R10), (base as u64, base as u64 + 64), "registers untouched");
        let at = base - block.as_ptr() as usize;
        assert!(block[at..at + 64].iter().all(|&b| b == 0));
        assert!(block[at + 64..].iter().all(|&b| b == 0xAA), "one line only");
    }
}
