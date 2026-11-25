//! Fallback for `clzero` on host CPUs without the CLZERO instruction.
//!
//! Xbox Series X|S (Zen 2) implements AMD's `clzero` (`0F 01 FC`, CPUID
//! `0x8000_0008` EBX bit 0). It zeroes the 64-byte cache line that contains the
//! address in RAX and leaves RAX unchanged. Intel CPUs do not implement it and
//! raise `EXCEPTION_ILLEGAL_INSTRUCTION`.
//!
//! No other three-byte instruction zeroes a line, so the site cannot be
//! rewritten in place the way `clwb` is. Instead the handler performs the store
//! and resumes after the instruction.
//!
//! ProjectScarlett 1.0.2.1 clears object fields with back-to-back loops of the
//! form `clzero; add rax, 0x40; cmp rax, reg; jb clzero`. When that exact loop
//! follows the faulting instruction, the handler clears every line the loop
//! would clear, leaves RAX at its exit value and resumes at the `cmp`, so the
//! CPU evaluates the loop condition itself. Each loop then costs one trap
//! instead of one per line.
use core::ffi::c_void;
use std::sync::{LazyLock, Mutex};
use windows::Win32::System::Diagnostics::Debug::CONTEXT;
use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, VirtualQuery};

const CLZERO: [u8; 3] = [0x0F, 0x01, 0xFC];
const LINE: u64 = 64;
/// `add rax, 0x40`
const ADD_RAX_LINE: [u8; 4] = [0x48, 0x83, 0xC0, 0x40];
/// Bytes from the `clzero` to the `cmp` of a recognized loop.
const LOOP_CMP_OFFSET: u64 = (CLZERO.len() + ADD_RAX_LINE.len()) as u64;
const MEM_COMMIT_STATE: u32 = 0x1000;
const PAGE_GUARD_FLAG: u32 = 0x100;
/// Writable page protection mask (READWRITE, WRITECOPY, EXECUTE_READWRITE,
/// EXECUTE_WRITECOPY).
const PAGE_WRITABLE_MASK: u32 = 0xCC;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Site {
    /// A `clzero` that is not part of a recognized loop.
    Single,
    /// `clzero; add rax, 0x40; cmp rax, <end_reg>; jb clzero`.
    Loop { end_reg: u8 },
}

/// Lines a fallback clears, and where execution resumes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) first: u64,
    pub(crate) bytes: u64,
    pub(crate) rax: u64,
    pub(crate) resume: u64,
}

fn host_has_clzero() -> bool {
    static CLZERO_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
        #[cfg(target_arch = "x86_64")]
        {
            // Rust 1.95 still marks `__cpuid` unsafe; newer toolchains make it safe.
            #[allow(unused_unsafe)]
            unsafe {
                let max_extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
                max_extended >= 0x8000_0008
                    && (core::arch::x86_64::__cpuid(0x8000_0008).ebx & 1) != 0
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    });
    *CLZERO_SUPPORTED
}

/// Decode a prefix-free `clzero` at the start of `bytes`, and the loop that
/// follows it when it matches exactly.
pub(crate) fn decode(bytes: &[u8]) -> Option<Site> {
    if !bytes.starts_with(&CLZERO) {
        return None;
    }
    let rest = &bytes[CLZERO.len()..];
    // add rax, 0x40 ; cmp rax, r64 (REX.W [REX.R] 39 /r, mod 11, rm rax) ;
    // jb rel8 -12, back to the clzero.
    if let [a, b, c, d, rex, 0x39, modrm, 0x72, 0xF4, ..] = *rest {
        if [a, b, c, d] == ADD_RAX_LINE
            && (rex == 0x48 || rex == 0x4C)
            && modrm >> 6 == 3
            && modrm & 7 == 0
        {
            let end_reg = ((modrm >> 3) & 7) | if rex == 0x4C { 8 } else { 0 };
            if end_reg != 0 {
                return Some(Site::Loop { end_reg });
            }
        }
    }
    Some(Site::Single)
}

/// The lines one `clzero` at `rip` clears. RAX is unchanged.
pub(crate) fn plan_single(rip: u64, rax: u64) -> Plan {
    Plan { first: rax & !(LINE - 1), bytes: LINE, rax, resume: rip + CLZERO.len() as u64 }
}

/// The lines the recognized loop clears, starting from `rax`, until RAX is no
/// longer below `end`. Returns `None` if the arithmetic would wrap.
pub(crate) fn plan_loop(rip: u64, rax: u64, end: u64) -> Option<Plan> {
    let first = rax & !(LINE - 1);
    let after_first = rax.checked_add(LINE)?;
    let extra = if after_first < end { (end - after_first).div_ceil(LINE) } else { 0 };
    let bytes = extra.checked_add(1)?.checked_mul(LINE)?;
    first.checked_add(bytes)?;
    Some(Plan { first, bytes, rax: rax.checked_add(bytes)?, resume: rip + LOOP_CMP_OFFSET })
}

fn register(context: &CONTEXT, index: u8) -> u64 {
    match index {
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

/// Whether every byte of `[start, start + len)` is committed, writable and not
/// a guard page.
fn writable(start: u64, len: u64) -> bool {
    let (Ok(start), Ok(len)) = (usize::try_from(start), usize::try_from(len)) else {
        return false;
    };
    let Some(end) = start.checked_add(len) else { return false };
    let mut addr = start;
    while addr < end {
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `VirtualQuery` inspects the caller's own address space.
        let queried = unsafe {
            VirtualQuery(Some(addr as *const c_void), &mut region, size_of_val(&region))
        };
        if queried == 0
            || region.State.0 != MEM_COMMIT_STATE
            || (region.Protect.0 & PAGE_GUARD_FLAG) != 0
            || (region.Protect.0 & PAGE_WRITABLE_MASK) == 0
        {
            return false;
        }
        let region_end = (region.BaseAddress as usize).saturating_add(region.RegionSize);
        if region_end <= addr {
            return false;
        }
        addr = region_end;
    }
    true
}

/// Apply the fallback without checking the host CPU. Returns whether execution
/// should resume, and the plan that was applied.
fn handle_unchecked(context: &mut CONTEXT, start: usize, end: usize) -> Option<(Site, Plan)> {
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) {
        return None;
    }
    let (buf, avail) = crate::clwb::read_code_bytes(rip, end);
    let site = decode(&buf[..avail])?;
    let rip = rip as u64;
    let single = plan_single(rip, context.Rax);
    let (site, plan) = match site {
        Site::Loop { end_reg } => match plan_loop(rip, context.Rax, register(context, end_reg)) {
            Some(plan) if writable(plan.first, plan.bytes) => (site, plan),
            // Clear one line at a time; the line that real hardware would fault
            // on is then left to fault here as well.
            _ => (Site::Single, single),
        },
        Site::Single => (site, single),
    };
    if !writable(plan.first, plan.bytes) {
        return None;
    }
    // SAFETY: `writable` verified the range is committed and writable.
    unsafe { core::ptr::write_bytes(plan.first as usize as *mut u8, 0, plan.bytes as usize) };
    context.Rax = plan.rax;
    context.Rip = plan.resume;
    Some((site, plan))
}

/// Handle `EXCEPTION_ILLEGAL_INSTRUCTION` inside `[start, end)` when caused by
/// `clzero` on a host CPU without CLZERO. Returns `true` if execution should
/// resume.
pub(crate) fn handle_illegal_instruction(context: &mut CONTEXT, start: usize, end: usize) -> bool {
    if host_has_clzero() {
        return false;
    }
    let rip = context.Rip as usize;
    let Some((site, _)) = handle_unchecked(context, start, end) else {
        return false;
    };
    static LOGGED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
    if !logged.contains(&rip) {
        logged.push(rip);
        drop(logged);
        let form = if site == Site::Single { "clzero" } else { "clzero loop" };
        crate::diagnostics::log(&format!(
            "emulated unsupported {form} at RVA {:#x} (host lacks CLZERO)",
            rip - start
        ));
    }
    true
}

#[cfg(test)]
#[path = "tests/clzero.rs"]
mod tests;
