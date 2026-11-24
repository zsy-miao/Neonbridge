//! 6ixpack: the console CPU is Zen 2, and titles use AMD-only instructions
//! that raise STATUS_ILLEGAL_INSTRUCTION on Intel CPUs. A vectored handler
//! does their work and steps over them:
//! - `clzero` (0F 01 FC) zeroes the 64-byte line at rAX. The compiler's fill
//!   loop `clzero; add rax,0x40; cmp rax,reg; jb -12` is done in one trap.
//! - `monitorx` / `mwaitx` (0F 01 FA / FB) are wait hints; mwaitx yields.
//! - `rdpru` (0F 01 FD) reads MPERF/APERF; both become the TSC.
//! - SSE4a `extrq` / `insertq` (66|F2 0F 78|79, register forms) run on the
//!   saved XMM registers (from VIctory's loader).
use std::ffi::c_void;
use windows_sys::Win32::Foundation::STATUS_ILLEGAL_INSTRUCTION;
use windows_sys::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, M128A};

const CONTINUE_EXECUTION: i32 = -1;
const CONTINUE_SEARCH: i32 = 0;

unsafe extern "system" {
    fn SwitchToThread() -> i32;
}

/// rAX..r15 in the CONTEXT's register order.
fn registers(context: &mut CONTEXT) -> &mut [u64; 16] {
    // SAFETY: CONTEXT stores Rax through R15 as 16 consecutive u64 fields.
    unsafe { &mut *(&raw mut context.Rax).cast::<[u64; 16]>() }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    // SAFETY: the system passes valid exception pointers; the faulting
    // instruction's bytes were just fetched, so they are readable.
    unsafe {
        let record = &*(*info).ExceptionRecord;
        if record.ExceptionCode != STATUS_ILLEGAL_INSTRUCTION {
            return CONTINUE_SEARCH;
        }
        let context = &mut *(*info).ContextRecord;
        let code = std::slice::from_raw_parts(context.Rip as *const u8, 3);
        if code[0] != 0x0F || code[1] != 0x01 {
            return match sse4a(context.Rip as *const u8, &mut context.Anonymous.FltSave.XmmRegisters) {
                Some(len) => {
                    context.Rip += len as u64;
                    CONTINUE_EXECUTION
                }
                None => CONTINUE_SEARCH,
            };
        }
        match code[2] {
            0xFC => {
                let line = context.Rax & !63;
                // clzero; add rax,0x40; cmp rax,reg (48|4C 39 /r, rm=rax); jb -12
                let tail = std::slice::from_raw_parts((context.Rip + 3) as *const u8, 9);
                let end = (tail[..4] == [0x48, 0x83, 0xC0, 0x40]
                    && matches!(tail[4], 0x48 | 0x4C)
                    && tail[5] == 0x39
                    && tail[6] & 0xC7 == 0xC0
                    && tail[7..9] == [0x72, 0xF4])
                    .then(|| ((tail[6] >> 3) & 7) as usize | if tail[4] == 0x4C { 8 } else { 0 })
                    .filter(|&r| r != 0)
                    .map(|r| registers(context)[r]);
                match end {
                    Some(end) if end > context.Rax => {
                        // The loop runs while rax < end, one line per pass.
                        let passes = (end - context.Rax).div_ceil(64);
                        let last = context.Rax + (passes - 1) * 64;
                        std::ptr::write_bytes(line as *mut u8, 0, ((last & !63) + 64 - line) as usize);
                        context.Rax += passes * 64;
                        context.EFlags &= !1; // jb not taken
                        context.Rip += 12;
                    }
                    _ => {
                        std::ptr::write_bytes(line as *mut u8, 0, 64);
                        context.Rip += 3;
                    }
                }
            }
            0xFA => context.Rip += 3,
            0xFB => {
                SwitchToThread();
                context.Rip += 3;
            }
            0xFD => {
                let tsc = core::arch::x86_64::_rdtsc();
                context.Rax = tsc & 0xffff_ffff;
                context.Rdx = tsc >> 32;
                context.Rip += 3;
            }
            _ => return CONTINUE_SEARCH,
        }
        CONTINUE_EXECUTION
    }
}

/// SSE4a's `extrq`/`insertq` (register forms, the only ones there are) on
/// the saved XMM registers; returns the instruction's length.
unsafe fn sse4a(code: *const u8, xmm: &mut [M128A; 16]) -> Option<usize> {
    // SAFETY: the faulting instruction's bytes are readable to its end.
    let b = |i: usize| unsafe { code.add(i).read() };
    let prefix = b(0);
    if !matches!(prefix, 0x66 | 0xF2) {
        return None;
    }
    let (rex, at) = if b(1) & 0xF0 == 0x40 { (b(1), 2) } else { (0, 1) };
    if b(at) != 0x0F || !matches!(b(at + 1), 0x78 | 0x79) {
        return None;
    }
    let modrm = b(at + 2);
    if modrm >> 6 != 3 {
        return None;
    }
    let dst = usize::from((modrm >> 3) & 7 | (rex & 4) << 1);
    let src = usize::from(modrm & 7 | (rex & 1) << 3);
    let immediate = b(at + 1) == 0x78;
    let (fields, len) = if immediate {
        ((b(at + 3), b(at + 4)), at + 5)
    } else {
        // extrq reads its fields from src's low half, insertq from its high.
        let f = if prefix == 0x66 { xmm[src].Low } else { xmm[src].High as u64 };
        ((f as u8, (f >> 8) as u8), at + 3)
    };
    let (n, shift) = (u32::from(fields.0 & 63), u32::from(fields.1 & 63));
    let mask = if n == 0 { u64::MAX } else { (1u64 << n) - 1 };
    if prefix == 0x66 {
        if immediate && (modrm >> 3) & 7 != 0 {
            return None;
        }
        let target = if immediate { src } else { dst };
        xmm[target].Low = (xmm[target].Low >> shift) & mask;
    } else {
        let value = (xmm[src].Low & mask) << shift;
        xmm[dst].Low = xmm[dst].Low & !(mask << shift) | value;
    }
    Some(len)
}

pub(crate) unsafe fn install() {
    let handler: unsafe extern "system" fn(*mut EXCEPTION_POINTERS) -> i32 = handler;
    let installed: *mut c_void = unsafe { AddVectoredExceptionHandler(1, Some(handler)) };
    if installed.is_null() {
        crate::loader::log("console CPU instruction handler was not installed");
    }
}
