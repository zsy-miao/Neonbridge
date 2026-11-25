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
mod tests {
    use super::{Plan, Site, decode, handle_unchecked, plan_loop, plan_single};
    use windows::Win32::System::Diagnostics::Debug::CONTEXT;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, PAGE_PROTECTION_FLAGS,
        PAGE_READWRITE, VirtualAlloc, VirtualFree, VirtualProtect,
    };

    /// The first loop of the ProjectScarlett 1.0.2.1 clearing routine at RVA
    /// 0x4E8AA70, with the following `mov rax, r10`.
    const SCARLETT_LOOP: [u8; 15] =
        [0x0F, 0x01, 0xFC, 0x48, 0x83, 0xC0, 0x40, 0x4C, 0x39, 0xD0, 0x72, 0xF4, 0x4C, 0x89, 0xD0];

    #[test]
    fn decodes_the_scarlett_loops() {
        assert_eq!(decode(&SCARLETT_LOOP), Some(Site::Loop { end_reg: 10 }));
        // cmp rax, r9 / r8 / rdx: the later loops in the same routine.
        let mut bytes = SCARLETT_LOOP;
        bytes[9] = 0xC8;
        assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 9 }));
        bytes[9] = 0xC0;
        assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 8 }));
        bytes[7] = 0x48;
        bytes[9] = 0xD0;
        assert_eq!(decode(&bytes), Some(Site::Loop { end_reg: 2 }));
    }

    #[test]
    fn other_forms_decode_as_a_single_line_or_not_at_all() {
        assert_eq!(decode(&[0x0F, 0x01, 0xFC]), Some(Site::Single));
        // Truncated loop, wrong stride, wrong branch target, cmp rax, rax,
        // cmp with a memory operand, and a REX.B that compares another register.
        for (index, value) in [(6, 0x80), (11, 0xF2), (9, 0xC0), (9, 0x10), (7, 0x49)] {
            let mut bytes = SCARLETT_LOOP;
            bytes[index] = value;
            if index == 9 && value == 0xC0 {
                bytes[7] = 0x48;
            }
            assert_eq!(decode(&bytes), Some(Site::Single), "byte {index} = {value:#x}");
        }
        assert_eq!(decode(&SCARLETT_LOOP[..11]), Some(Site::Single));
        // Not clzero: monitorx (0F 01 FA), mwaitx (0F 01 FB), rdpru (0F 01 FD),
        // and clzero with an address-size prefix (EAX form).
        assert_eq!(decode(&[0x0F, 0x01, 0xFA]), None);
        assert_eq!(decode(&[0x0F, 0x01, 0xFB]), None);
        assert_eq!(decode(&[0x0F, 0x01, 0xFD]), None);
        assert_eq!(decode(&[0x67, 0x0F, 0x01, 0xFC]), None);
        assert_eq!(decode(&[0x0F, 0x01]), None);
    }

    #[test]
    fn plans_match_the_instruction_sequence() {
        // Reference model: run the loop one line at a time.
        fn model(rax: u64, end: u64) -> (u64, u64, u64) {
            let (mut rax, mut lines) = (rax, Vec::new());
            loop {
                lines.push(rax & !63);
                rax += 64;
                if rax >= end {
                    break;
                }
            }
            (lines[0], lines.len() as u64 * 64, rax)
        }
        let rip = 0x1_4000_0000;
        for rax in [0x1000u64, 0x1001, 0x103F, 0x1040, 0x1234] {
            for end in [0u64, 0x1000, 0x1040, 0x1041, 0x1080, 0x10BF, 0x10C0, 0x10C1, 0x4000] {
                let (first, bytes, exit) = model(rax, end);
                assert_eq!(
                    plan_loop(rip, rax, end),
                    Some(Plan { first, bytes, rax: exit, resume: rip + 7 }),
                    "rax {rax:#x} end {end:#x}"
                );
            }
        }
        assert_eq!(plan_loop(rip, u64::MAX - 10, u64::MAX), None);
        assert_eq!(
            plan_single(rip, 0x1234),
            Plan { first: 0x1200, bytes: 64, rax: 0x1234, resume: rip + 3 }
        );
    }

    struct Pages(*mut u8, usize);

    impl Pages {
        fn new(len: usize) -> Self {
            // SAFETY: a fresh private allocation owned by this guard.
            let base = unsafe { VirtualAlloc(None, len, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
            assert!(!base.is_null());
            Pages(base.cast(), len)
        }
        fn bytes(&self) -> &mut [u8] {
            // SAFETY: the allocation is committed and readable for `len` bytes.
            unsafe { core::slice::from_raw_parts_mut(self.0, self.1) }
        }
    }

    impl Drop for Pages {
        fn drop(&mut self) {
            // SAFETY: releases the allocation made in `new`.
            unsafe { VirtualFree(self.0.cast(), 0, MEM_RELEASE) }.unwrap();
        }
    }

    fn context(rip: *const u8, rax: u64, r10: u64) -> CONTEXT {
        CONTEXT { Rip: rip as u64, Rax: rax, R10: r10, ..Default::default() }
    }

    #[test]
    fn clears_the_whole_loop_range_and_resumes_at_the_compare() {
        let code = Pages::new(0x1000);
        code.bytes()[..SCARLETT_LOOP.len()].copy_from_slice(&SCARLETT_LOOP);
        let data = Pages::new(0x2000);
        data.bytes().fill(0xAA);
        let base = data.0 as u64;
        // rax = object + 0x40 and r10 = object + 0x39C0, as in the title.
        let (rax, end) = (base + 0x40, base + 0x39C0 - 0x2000);
        let mut ctx = context(code.0, rax, end);
        let range = (code.0 as usize, code.0 as usize + 0x1000);
        let (site, plan) = handle_unchecked(&mut ctx, range.0, range.1).unwrap();
        assert_eq!(site, Site::Loop { end_reg: 10 });
        assert_eq!(ctx.Rip, code.0 as u64 + 7);
        assert!(ctx.Rax >= end && ctx.Rax - 64 < end);
        assert_eq!(plan.first, rax);
        let cleared = (plan.first - base) as usize..(plan.first - base + plan.bytes) as usize;
        for (offset, &byte) in data.bytes().iter().enumerate() {
            let expected = if cleared.contains(&offset) { 0 } else { 0xAA };
            assert_eq!(byte, expected, "offset {offset:#x}");
        }
    }

    #[test]
    fn falls_back_to_one_line_and_refuses_unwritable_memory() {
        let code = Pages::new(0x1000);
        code.bytes()[..SCARLETT_LOOP.len()].copy_from_slice(&SCARLETT_LOOP);
        let range = (code.0 as usize, code.0 as usize + 0x1000);
        let data = Pages::new(0x2000);
        data.bytes().fill(0xAA);
        let base = data.0 as u64;
        let mut old = PAGE_PROTECTION_FLAGS(0);
        // SAFETY: protects the second page of the guard's own allocation.
        unsafe { VirtualProtect(data.0.add(0x1000).cast(), 0x1000, PAGE_READONLY, &mut old) }
            .unwrap();

        // The loop would run into the read-only page: clear only the first line.
        let mut ctx = context(code.0, base + 0xFC0, base + 0x1100);
        let (site, plan) = handle_unchecked(&mut ctx, range.0, range.1).unwrap();
        assert_eq!(site, Site::Single);
        assert_eq!((plan.first, plan.bytes), (base + 0xFC0, 64));
        assert_eq!((ctx.Rip, ctx.Rax), (code.0 as u64 + 3, base + 0xFC0));
        assert!(data.bytes()[0xFC0..0x1000].iter().all(|&b| b == 0));
        assert!(data.bytes()[..0xFC0].iter().all(|&b| b == 0xAA));

        // A line in the read-only page is left to fault as it would on hardware.
        let mut ctx = context(code.0, base + 0x1000, base + 0x1100);
        assert_eq!(handle_unchecked(&mut ctx, range.0, range.1), None);
        assert_eq!(ctx.Rip, code.0 as u64);

        // Outside the title image, or not a clzero: not handled.
        let mut ctx = context(code.0, base, base + 0x40);
        assert_eq!(handle_unchecked(&mut ctx, range.0 + 1, range.1), None);
        code.bytes()[2] = 0xFA;
        assert_eq!(handle_unchecked(&mut ctx, range.0, range.1), None);
        // SAFETY: restores the page so the guard can release it.
        unsafe { VirtualProtect(data.0.add(0x1000).cast(), 0x1000, old, &mut old) }.unwrap();
    }
}
