//! Fallback for `monitorx` and `mwaitx` on host CPUs without MONITORX.
//!
//! Xbox Series X|S (Zen 2) implements AMD's user-mode wait instructions
//! `monitorx` (`0F 01 FA`, arm a watch on the address in RAX) and `mwaitx`
//! (`0F 01 FB`, wait for a write to it or for the optional timer in EBX). CPUID
//! `0x8000_0001` ECX bit 29 reports them. Intel CPUs do not implement them and
//! raise `EXCEPTION_ILLEGAL_INSTRUCTION`.
//!
//! Neither instruction changes registers, flags or memory, and `mwaitx` may
//! return at any time, so callers re-check their condition in a loop. A no-op
//! is therefore a valid implementation: the wait becomes a spin.
//!
//! Wait loops execute these instructions continuously, so trapping each one
//! is far too slow. On the first fault at a site the handler rewrites the
//! second opcode byte from `01` to `1F` in place: `0F 1F FA` and `0F 1F FB` are
//! the reserved-NOP form `0F 1F /r` with the same length. That single-byte
//! store is atomic, so another thread sees either the original instruction or
//! the NOP. A thread that faulted on the original bytes before the store
//! resumes at the same address and runs the NOP. If the page cannot be made
//! writable, the handler steps over the instruction instead.
//!
//! ProjectScarlett 1.0.2.1 waits this way at RVA 0x4FB0BD7 and 0x4FB0D21.
use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, FlushInstructionCache};
use windows::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect};
use windows::Win32::System::Threading::GetCurrentProcess;

const LEN: usize = 3;
const NOP_OPCODE: u8 = 0x1F;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
    Monitorx,
    Mwaitx,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Rewrote the instruction to a NOP; resume at the same address.
    Patched(Wait),
    /// Another thread already rewrote it; resume at the same address.
    AlreadyPatched,
    /// The page could not be made writable; stepped over the instruction.
    Skipped(Wait),
}

fn host_has_monitorx() -> bool {
    static MONITORX_SUPPORTED: LazyLock<bool> = LazyLock::new(|| {
        #[cfg(target_arch = "x86_64")]
        {
            // Rust 1.95 still marks `__cpuid` unsafe; newer toolchains make it safe.
            #[allow(unused_unsafe)]
            unsafe {
                let max_extended = core::arch::x86_64::__cpuid(0x8000_0000).eax;
                max_extended >= 0x8000_0001
                    && (core::arch::x86_64::__cpuid(0x8000_0001).ecx & (1 << 29)) != 0
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            false
        }
    });
    *MONITORX_SUPPORTED
}

/// Decode a prefix-free `monitorx` or `mwaitx` at the start of `bytes`.
pub(crate) fn decode(bytes: &[u8]) -> Option<Wait> {
    match bytes {
        [0x0F, 0x01, 0xFA, ..] => Some(Wait::Monitorx),
        [0x0F, 0x01, 0xFB, ..] => Some(Wait::Mwaitx),
        _ => None,
    }
}

/// Whether `bytes` are a wait instruction this module has already rewritten.
fn is_patched(bytes: &[u8]) -> bool {
    matches!(bytes, [0x0F, NOP_OPCODE, 0xFA | 0xFB, ..])
}

/// Rewrite the opcode byte at `rip + 1` to `1F` and flush the instruction cache.
///
/// # Safety
/// `rip..rip + 3` must be a mapped `monitorx` or `mwaitx` instruction.
unsafe fn patch_to_nop(rip: usize) -> Result<(), ()> {
    let opcode = rip + 1;
    let mut old_protect = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: changes the protection of one byte of the caller's own image,
    // keeping it executable.
    unsafe { VirtualProtect(opcode as *const c_void, 1, PAGE_EXECUTE_READWRITE, &mut old_protect) }
        .map_err(|_| ())?;
    // SAFETY: `opcode` is now writable; a single-byte store is atomic, so other
    // threads decode either `0F 01 xx` or `0F 1F xx`.
    unsafe {
        AtomicU8::from_ptr(opcode as *mut u8).store(NOP_OPCODE, Ordering::Release);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(rip as *const c_void), LEN);
        let mut ignored = PAGE_PROTECTION_FLAGS(0);
        let _ = VirtualProtect(opcode as *const c_void, 1, old_protect, &mut ignored);
    }
    Ok(())
}

/// Handle the fault at `context.Rip` without checking the host CPU.
fn handle_unchecked(context: &mut CONTEXT, start: usize, end: usize) -> Option<Outcome> {
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) {
        return None;
    }
    // Serialize patching so two threads faulting at one site don't race on
    // the page protection.
    static PATCHING: Mutex<()> = Mutex::new(());
    let _guard = PATCHING.lock().unwrap_or_else(|e| e.into_inner());
    let (buf, avail) = crate::clwb::read_code_bytes(rip, end);
    let bytes = &buf[..avail];
    if is_patched(bytes) {
        // A NOP cannot raise #UD, so this thread faulted on the original bytes
        // before another thread's store. Run the NOP.
        return Some(Outcome::AlreadyPatched);
    }
    let wait = decode(bytes)?;
    // SAFETY: `decode` matched a wait instruction at `rip`.
    if unsafe { patch_to_nop(rip) }.is_ok() {
        Some(Outcome::Patched(wait))
    } else {
        context.Rip += LEN as u64;
        Some(Outcome::Skipped(wait))
    }
}

/// Handle `EXCEPTION_ILLEGAL_INSTRUCTION` inside `[start, end)` when caused by
/// `monitorx` or `mwaitx` on a host CPU without MONITORX. Returns `true` if
/// execution should resume.
pub(crate) fn handle_illegal_instruction(context: &mut CONTEXT, start: usize, end: usize) -> bool {
    if host_has_monitorx() {
        return false;
    }
    let rip = context.Rip as usize;
    let Some(outcome) = handle_unchecked(context, start, end) else {
        return false;
    };
    let name = |wait| if wait == Wait::Monitorx { "monitorx" } else { "mwaitx" };
    let line = match outcome {
        Outcome::Patched(wait) => Some(format!(
            "rewrote unsupported {} at RVA {:#x} to a NOP (host lacks MONITORX)",
            name(wait),
            rip - start
        )),
        Outcome::Skipped(wait) => {
            static LOGGED: Mutex<Vec<usize>> = Mutex::new(Vec::new());
            let mut logged = LOGGED.lock().unwrap_or_else(|e| e.into_inner());
            (!logged.contains(&rip)).then(|| {
                logged.push(rip);
                format!(
                    "stepped over unsupported {} at RVA {:#x} (host lacks MONITORX, page not writable)",
                    name(wait),
                    rip - start
                )
            })
        }
        Outcome::AlreadyPatched => None,
    };
    if let Some(line) = line {
        crate::diagnostics::log(&line);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{Outcome, Wait, decode, handle_unchecked, is_patched};
    use windows::Win32::System::Diagnostics::Debug::CONTEXT;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, PAGE_READONLY, VirtualAlloc, VirtualFree,
        VirtualProtect, VirtualQuery,
    };

    /// monitorx; mwaitx; ret
    const WAITS: [u8; 7] = [0x0F, 0x01, 0xFA, 0x0F, 0x01, 0xFB, 0xC3];

    struct Code(*mut u8);

    impl Code {
        fn new(bytes: &[u8], protect: PAGE_PROTECTION_FLAGS) -> Self {
            // SAFETY: a fresh private allocation owned by this guard.
            let base: *mut u8 =
                unsafe { VirtualAlloc(None, 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }
                    .cast();
            assert!(!base.is_null());
            // SAFETY: the page is writable and at least `bytes.len()` long.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), base, bytes.len()) };
            let mut old = PAGE_PROTECTION_FLAGS(0);
            // SAFETY: changes the protection of the guard's own page.
            unsafe { VirtualProtect(base.cast(), 0x1000, protect, &mut old) }.unwrap();
            Code(base)
        }
        fn bytes(&self, len: usize) -> Vec<u8> {
            // SAFETY: the page is committed and readable.
            unsafe { core::slice::from_raw_parts(self.0, len) }.to_vec()
        }
        fn range(&self) -> (usize, usize) {
            (self.0 as usize, self.0 as usize + 0x1000)
        }
        fn protection(&self) -> u32 {
            let mut info = MEMORY_BASIC_INFORMATION::default();
            // SAFETY: queries the guard's own page.
            unsafe { VirtualQuery(Some(self.0.cast()), &mut info, size_of_val(&info)) };
            info.Protect.0
        }
    }

    impl Drop for Code {
        fn drop(&mut self) {
            // SAFETY: releases the allocation made in `new`.
            unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }.unwrap();
        }
    }

    #[test]
    fn decodes_only_the_two_wait_instructions() {
        // ProjectScarlett 1.0.2.1 at RVA 0x4FB0BD7 and 0x4FB0D52.
        assert_eq!(decode(&[0x0F, 0x01, 0xFA, 0x48, 0x8B, 0x17]), Some(Wait::Monitorx));
        assert_eq!(decode(&[0x0F, 0x01, 0xFB, 0x48, 0x8B, 0x17]), Some(Wait::Mwaitx));
        // clzero, rdpru, monitor, mwait, prefixed forms and truncated bytes.
        for bytes in [
            &[0x0F, 0x01, 0xFC][..],
            &[0x0F, 0x01, 0xFD],
            &[0x0F, 0x01, 0xC8],
            &[0x0F, 0x01, 0xC9],
            &[0x67, 0x0F, 0x01, 0xFA],
            &[0xF3, 0x0F, 0x01, 0xFB],
            &[0x0F, 0x01],
        ] {
            assert_eq!(decode(bytes), None, "{bytes:02x?}");
        }
        assert!(is_patched(&[0x0F, 0x1F, 0xFA]) && is_patched(&[0x0F, 0x1F, 0xFB]));
        assert!(!is_patched(&[0x0F, 0x1F, 0x00]) && !is_patched(&[0x0F, 0x1F]));
    }

    #[test]
    fn rewritten_waits_run_as_nops_on_this_cpu() {
        let code = Code::new(&[0x0F, 0x1F, 0xFA, 0x0F, 0x1F, 0xFB, 0xC3], PAGE_EXECUTE_READ);
        // SAFETY: the page holds `nop edx; nop ebx; ret`.
        let run: extern "C" fn() = unsafe { core::mem::transmute(code.0) };
        run();
    }

    #[test]
    fn patches_read_only_code_in_place_and_restores_its_protection() {
        let code = Code::new(&WAITS, PAGE_EXECUTE_READ);
        let (start, end) = code.range();
        let mut ctx =
            CONTEXT { Rip: start as u64, Rax: 0x1234, Rcx: 2, EFlags: 0x246, ..Default::default() };
        let before = ctx;
        assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::Patched(Wait::Monitorx)));
        // Resumes at the same address, now a NOP; registers untouched.
        assert_eq!(
            (ctx.Rip, ctx.Rax, ctx.Rcx, ctx.EFlags),
            (before.Rip, before.Rax, before.Rcx, before.EFlags)
        );
        // A second thread that faulted on the original bytes runs the NOP.
        assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::AlreadyPatched));
        ctx.Rip += 3;
        assert_eq!(handle_unchecked(&mut ctx, start, end), Some(Outcome::Patched(Wait::Mwaitx)));
        assert_eq!(code.bytes(7), [0x0F, 0x1F, 0xFA, 0x0F, 0x1F, 0xFB, 0xC3]);
        assert_eq!(code.protection(), PAGE_EXECUTE_READ.0);
        // The patched routine now runs natively.
        // SAFETY: the page holds `nop edx; nop ebx; ret`.
        let run: extern "C" fn() = unsafe { core::mem::transmute(code.0) };
        run();
    }

    #[test]
    fn leaves_other_code_alone() {
        let code = Code::new(&[0x0F, 0x01, 0xFC, 0x0F, 0x1F, 0x00], PAGE_READONLY);
        let (start, end) = code.range();
        // clzero, an ordinary NOP, and an address outside the title image.
        let mut ctx = CONTEXT { Rip: start as u64, ..Default::default() };
        assert_eq!(handle_unchecked(&mut ctx, start, end), None);
        ctx.Rip += 3;
        assert_eq!(handle_unchecked(&mut ctx, start, end), None);
        ctx.Rip = start as u64;
        assert_eq!(handle_unchecked(&mut ctx, start + 1, end), None);
        assert_eq!(code.bytes(6), [0x0F, 0x01, 0xFC, 0x0F, 0x1F, 0x00]);
    }
}
