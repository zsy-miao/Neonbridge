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

/// (Neptune) The title waits in loops of MONITORX and MWAITX, which this CPU lacks: every pass
/// was two exceptions (a fifth of the title's main thread in one sample). The three bytes
/// become a NOP, or PAUSE and a NOP, the first time each is met: the loop then spins as a PC
/// one does. Written with the byte behind them in one store, so a thread running into the
/// instruction meanwhile sees the old one or the new one. `VICTORY_PATCH_WAITS=0`: as before.
fn patch_wait(rip: usize, len: usize, bytes: [u8; 3]) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if len != 3 || !*ON.get_or_init(|| std::env::var_os("VICTORY_PATCH_WAITS").is_none_or(|v| v != "0")) {
        return;
    }
    if !readable(rip, 4) {
        return;
    }
    let mut old = 0u32;
    // SAFETY: four readable bytes of code, made writable for one store and restored; the
    // fourth byte is written back as it was read.
    unsafe {
        if VirtualProtect(rip as *const _, 4, PAGE_EXECUTE_READWRITE, &mut old) != 0 {
            let behind = *((rip + 3) as *const u8);
            let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], behind]);
            std::ptr::write_unaligned(rip as *mut u32, word);
            VirtualProtect(rip as *const _, 4, old, &mut old);
            static TOLD: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if TOLD.fetch_add(1, Relaxed) < 8 {
                tracing::info!(at = format_args!("{rip:#x}"), "a wait instruction this CPU lacks: replaced where it stands");
            }
        }
    }
}

/// (Neptune) Instructions emulated so far by kind: clzero, clzero loops, monitorx/mwaitx,
/// rdpru, rdpid, extrq, insertq, others.
static KINDS: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Clzero loops replaced where they stand ([`patch_clzero_loop`]).
static LOOPS_REPLACED: AtomicU64 = AtomicU64::new(0);

/// Executable memory near the title's code for [`patch_clzero_loop`]: its start, how much
/// of it is used, its size (0: not made yet; `usize::MAX`: it could not be made).
static NEAR: std::sync::Mutex<(usize, usize, usize)> = std::sync::Mutex::new((0, 0, 0));

/// (Neptune) The title zeroes memory with `clzero; add rax, 40h; cmp rax, <reg>; jb` loops,
/// and this CPU has no CLZERO: every run of such a loop was an exception, a trip into Windows
/// and back and through every handler (8 million in five minutes, some 2,600 a frame, on the
/// threads that record the frame's draws). The twelve bytes of the loop become a jump to a
/// few instructions of ours that do what the emulation did: zero `[rax, reg)` if `reg` is
/// above `rax`, leave `reg` in `rax`, with the flags of the loop's last compare. Other
/// registers are as they were. `VICTORY_PATCH_CLZERO=0`: emulated every time, as before.
fn patch_clzero_loop(rip: usize, end_reg: u8) {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("VICTORY_PATCH_CLZERO").is_none_or(|v| v != "0")) {
        return;
    }
    let r = end_reg & 15;
    // Not against rax or rsp (the code below pushes), and the first eight bytes in one
    // cache line: they are written with one store, so that a thread arriving there
    // meanwhile runs the old loop or the jump.
    if r == 0 || r == 4 || rip % 64 > 56 || !readable(rip, 12) {
        return;
    }
    let Ok(mut near) = NEAR.lock() else { return };
    // SAFETY: twelve readable bytes of code.
    let now = unsafe { std::slice::from_raw_parts(rip as *const u8, 12) };
    if now[0] != 0x0F || now[1] != 0x01 || now[2] != 0xFC {
        // Another thread replaced it already.
        return;
    }
    const SIZE: usize = 64 << 10;
    if near.2 == 0 {
        // Within reach of a 32-bit jump from the title's code: tried every 16 MiB, below
        // and above.
        let base = rip & !0xFFFF;
        let mut made = 0usize;
        'find: for step in 1..=100usize {
            for at in [base.wrapping_sub(step << 24), base.wrapping_add(step << 24)] {
                // SAFETY: a reservation of our own, where the address is free.
                let p = unsafe { VirtualAlloc(at as *const _, SIZE, MEM_RESERVE | MEM_COMMIT, PAGE_EXECUTE_READWRITE) } as usize;
                if p != 0 {
                    made = p;
                    break 'find;
                }
            }
        }
        *near = if made == 0 { (0, 0, usize::MAX) } else { (made, 0, SIZE) };
    }
    if near.2 == usize::MAX || near.1 + 48 > near.2 {
        return;
    }
    let at = near.0 + near.1;
    let back = rip + 12;
    let rex = 0x48 | u8::from(r >= 8);
    let mut code: Vec<u8> = vec![
        0x57,                           // push rdi
        0x51,                           // push rcx
        rex, 0x8B, 0xC8 | (r & 7),      // mov rcx, reg (the end)
        0x48, 0x89, 0xC7,               // mov rdi, rax (the start)
        0x48, 0x29, 0xF9,               // sub rcx, rdi (the bytes)
        0x76, 0x04,                     // jbe +4: nothing to zero
        0x31, 0xC0,                     // xor eax, eax
        0xF3, 0xAA,                     // rep stosb
        0x59,                           // pop rcx
        0x5F,                           // pop rdi
        rex, 0x8B, 0xC0 | (r & 7),      // mov rax, reg
        rex, 0x3B, 0xC0 | (r & 7),      // cmp rax, reg
        0xE9,                           // jmp back
    ];
    let from = at + code.len() + 4;
    let Ok(rel_back) = i32::try_from(back as i64 - from as i64) else { return };
    code.extend_from_slice(&rel_back.to_le_bytes());
    let Ok(rel_there) = i32::try_from(at as i64 - (rip as i64 + 5)) else { return };
    // SAFETY: memory of our own, with room (checked above); nothing runs it yet.
    unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), at as *mut u8, code.len()) };
    near.1 += code.len().next_multiple_of(16);
    let mut old = 0u32;
    // SAFETY: the loop's own twelve bytes, made writable for the patch and restored. The jump
    // and the three bytes behind it go in with one store; the rest of the loop is never
    // reached again (it was only entered through its first instruction).
    unsafe {
        if VirtualProtect(rip as *const _, 12, PAGE_EXECUTE_READWRITE, &mut old) != 0 {
            let rel = rel_there.to_le_bytes();
            let word = u64::from_le_bytes([0xE9, rel[0], rel[1], rel[2], rel[3], 0xCC, 0xCC, 0xCC]);
            std::ptr::write_unaligned(rip as *mut u64, word);
            VirtualProtect(rip as *const _, 12, old, &mut old);
            let n = LOOPS_REPLACED.fetch_add(1, Relaxed) + 1;
            if n <= 16 {
                tracing::info!(at = format_args!("{rip:#x}"), end_reg = r, n, "a clzero loop replaced where it stands");
            }
        }
    }
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
    // (Neptune) What kind, for the counts in the log, and so that the lock below is only
    // taken until each kind has been seen once (it was taken for every instruction, by
    // every thread).
    let kind = match what {
        ConsoleOnly::Clzero => 0,
        ConsoleOnly::ClzeroLoop { .. } => 1,
        ConsoleOnly::Monitorx | ConsoleOnly::Mwaitx => 2,
        ConsoleOnly::Rdpru => 3,
        ConsoleOnly::Rdpid(_) => 4,
        ConsoleOnly::Extrq { .. } => 5,
        ConsoleOnly::Insertq { .. } => 6,
        _ => 7,
    };
    KINDS[kind].fetch_add(1, Relaxed);
    static SEEN_ALL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    static SEEN: std::sync::Mutex<Vec<std::mem::Discriminant<ConsoleOnly>>> = std::sync::Mutex::new(Vec::new());
    if SEEN_ALL.load(Relaxed) & (1 << kind) == 0
        && let Ok(mut seen) = SEEN.lock()
        && !seen.contains(&std::mem::discriminant(&what))
    {
        if kind != 2 && kind != 5 && kind != 6 {
            // (Kinds of one instruction each: seen for good.)
            SEEN_ALL.fetch_or(1 << kind, Relaxed);
        }
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
        ConsoleOnly::Monitorx => patch_wait(rip, len, [0x0F, 0x1F, 0x00]),
        ConsoleOnly::Mwaitx => {
            std::hint::spin_loop();
            patch_wait(rip, len, [0xF3, 0x90, 0x90]);
        }
        ConsoleOnly::Clzero => {
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
            patch_clzero_loop(rip, end_reg);
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
    ctx.Rip += len as u64;
    // How often this runs at all (each one is an exception: a trip into Windows and back).
    let emulated = EMULATED.fetch_add(1, Relaxed) + 1;
    if emulated >= 1 << 16 && emulated.is_power_of_two() {
        tracing::info!(
            emulated,
            asked = ASKED.load(Relaxed),
            zeroed_mb = ZEROED.load(Relaxed) >> 20,
            clzero = KINDS[0].load(Relaxed),
            clzero_loops = KINDS[1].load(Relaxed),
            waits = KINDS[2].load(Relaxed),
            rdpru = KINDS[3].load(Relaxed),
            rdpid = KINDS[4].load(Relaxed),
            extrq = KINDS[5].load(Relaxed),
            insertq = KINDS[6].load(Relaxed),
            loops_replaced = LOOPS_REPLACED.load(Relaxed),
            "console CPU instructions emulated so far, the times Windows was asked about memory for them, and what clzero loops zeroed"
        );
    }
    CONTINUE_EXECUTION
}
