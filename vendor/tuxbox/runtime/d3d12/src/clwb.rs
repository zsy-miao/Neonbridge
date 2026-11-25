//! Compatibility for `clwb` on host CPUs without the CLWB instruction.
//!
//! Xbox Series X|S (Zen 2) supports `clwb` (`66 [REX] 0F AE /6`, CPUID leaf 7
//! EBX bit 24), while pre-Ice Lake Intel CPUs raise `EXCEPTION_ILLEGAL_INSTRUCTION`
//! when a title executes a cache-line write-back loop.
//!
//! On first fault at a title `clwb` site:
//! - If the host supports `clflushopt` (CPUID leaf 7 EBX bit 23, Skylake and
//!   newer), rewrite the ModRM byte atomically from `/6` to `/7` in place
//!   (`clwb` -> `clflushopt`), flush the instruction cache, and re-execute at
//!   the same `Rip`. Subsequent iterations run at native speed without trapping.
//! - Otherwise (or if the page cannot be made writable), advance `Rip` past the
//!   instruction, treating the write-back hint as a no-op.
use core::ffi::c_void;
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::{LazyLock, Mutex};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, FlushInstructionCache};
use windows::Win32::System::Memory::{
    MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect,
    VirtualQuery,
};
use windows::Win32::System::Threading::GetCurrentProcess;

const MAX_X86_INSN_LEN: usize = 15;
const MEM_COMMIT_STATE: u32 = 0x1000;
/// Readable page protection mask (READONLY, READWRITE, WRITECOPY, EXECUTE_READ,
/// EXECUTE_READWRITE, EXECUTE_WRITECOPY).
const PAGE_READABLE_MASK: u32 = 0xEE;
const PAGE_GUARD_FLAG: u32 = 0x100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct CpuCacheFeatures {
    pub(crate) clwb: bool,
    pub(crate) clflushopt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct DecodedClwb {
    pub(crate) len: usize,
    pub(crate) modrm_offset: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    PatchToClflushopt(DecodedClwb),
    Skip(usize),
    Retry,
    Unhandled,
}

fn cpu_features() -> CpuCacheFeatures {
    static FEATURES: LazyLock<CpuCacheFeatures> = LazyLock::new(detect_cpu_features);
    *FEATURES
}

fn detect_cpu_features() -> CpuCacheFeatures {
    #[cfg(target_arch = "x86_64")]
    {
        // Rust 1.95 still marks `__cpuid` unsafe; newer toolchains make it safe.
        #[allow(unused_unsafe)]
        let (max_leaf, ebx) = unsafe {
            let max_leaf = core::arch::x86_64::__cpuid(0).eax;
            let ebx = if max_leaf >= 7 {
                core::arch::x86_64::__cpuid_count(7, 0).ebx
            } else {
                0
            };
            (max_leaf, ebx)
        };
        if max_leaf < 7 {
            return CpuCacheFeatures { clwb: false, clflushopt: false };
        }
        CpuCacheFeatures {
            clflushopt: (ebx & (1 << 23)) != 0,
            clwb: (ebx & (1 << 24)) != 0,
        }
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        CpuCacheFeatures { clwb: false, clflushopt: false }
    }
}

/// Decode an x86-64 `clwb m8` instruction (`66 [REX] 0F AE /6`, `mod != 11`).
///
/// Rejects `LOCK`/`REP`/`REPNE` prefixes, missing `0x66`, register operands
/// (`mod == 3`, which encodes `tpause`), any other `/reg` extension, truncated
/// encodings, and encodings exceeding the 15-byte x86-64 maximum length.
pub(crate) fn decode_clwb(bytes: &[u8]) -> Option<DecodedClwb> {
    let mut i = 0usize;
    let mut has_66 = false;

    // Legacy prefixes (Groups 2, 3, 4). Group 1 (F0/F2/F3) is invalid for CLWB.
    while i < bytes.len() {
        match bytes[i] {
            0x66 => {
                has_66 = true;
                i += 1;
            }
            0x26 | 0x2E | 0x36 | 0x3E | 0x64 | 0x65 | 0x67 => {
                i += 1;
            }
            _ => break,
        }
        if i >= MAX_X86_INSN_LEN {
            return None;
        }
    }

    if !has_66 {
        return None;
    }

    // At most one REX prefix immediately preceding the 0F AE opcode.
    if i < bytes.len() && (0x40..=0x4F).contains(&bytes[i]) {
        i += 1;
    }

    // Two-byte opcode 0F AE + ModRM byte.
    if i + 3 > bytes.len() || bytes[i] != 0x0F || bytes[i + 1] != 0xAE {
        return None;
    }
    let modrm_offset = i + 2;
    let modrm = bytes[modrm_offset];
    let mod_bits = modrm >> 6;
    let reg_bits = (modrm >> 3) & 0x7;
    let rm_bits = modrm & 0x7;

    // CLWB requires a memory operand (mod != 3) and opcode extension /6.
    if mod_bits == 3 || reg_bits != 6 {
        return None;
    }

    let mut next = modrm_offset + 1;
    let disp_len = if rm_bits == 4 {
        // SIB byte present.
        if next >= bytes.len() {
            return None;
        }
        let sib = bytes[next];
        next += 1;
        match mod_bits {
            0 => if (sib & 0x7) == 5 { 4 } else { 0 },
            1 => 1,
            2 => 4,
            _ => unreachable!(),
        }
    } else {
        match mod_bits {
            0 => if rm_bits == 5 { 4 } else { 0 },
            1 => 1,
            2 => 4,
            _ => unreachable!(),
        }
    };

    let len = next + disp_len;
    if len > MAX_X86_INSN_LEN || len > bytes.len() {
        return None;
    }
    Some(DecodedClwb { len, modrm_offset })
}

fn decide(bytes: &[u8], rip: usize, features: CpuCacheFeatures, patched: &[usize]) -> Action {
    if features.clwb {
        return Action::Unhandled;
    }
    if patched.contains(&rip) {
        return Action::Retry;
    }
    let Some(inst) = decode_clwb(bytes) else {
        return Action::Unhandled;
    };
    if features.clflushopt {
        Action::PatchToClflushopt(inst)
    } else {
        Action::Skip(inst.len)
    }
}

/// Safely copy up to 15 committed, readable, non-guard bytes starting at `rip`
/// within `[rip, end)`, seamlessly handling instructions that straddle a page
/// boundary between regions with different protections.
pub(crate) fn read_code_bytes(rip: usize, end: usize) -> ([u8; MAX_X86_INSN_LEN], usize) {
    let mut buf = [0u8; MAX_X86_INSN_LEN];
    let limit = end.saturating_sub(rip).min(MAX_X86_INSN_LEN);
    let mut copied = 0usize;

    while copied < limit {
        let addr = rip + copied;
        let mut region = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: `VirtualQuery` inspects the caller's own address space.
        let queried = unsafe {
            VirtualQuery(Some(addr as *const c_void), &mut region, size_of_val(&region))
        };
        if queried == 0
            || region.State.0 != MEM_COMMIT_STATE
            || (region.Protect.0 & PAGE_GUARD_FLAG) != 0
            || (region.Protect.0 & PAGE_READABLE_MASK) == 0
        {
            break;
        }
        let region_end = (region.BaseAddress as usize).saturating_add(region.RegionSize);
        let chunk = region_end.saturating_sub(addr).min(limit - copied);
        if chunk == 0 {
            break;
        }
        // SAFETY: `addr..addr + chunk` lies inside a committed, readable,
        // non-guard region of the process address space.
        let src = unsafe { core::slice::from_raw_parts(addr as *const u8, chunk) };
        buf[copied..copied + chunk].copy_from_slice(src);
        copied += chunk;
    }

    (buf, copied)
}

/// Atomically rewrite a decoded `clwb` instruction's ModRM byte from `/6` to
/// `/7` (`clflushopt`) and flush the instruction cache.
///
/// # Safety
/// `rip..rip + inst.len` must point to a valid mapped `clwb` instruction.
unsafe fn patch_clwb_to_clflushopt(rip: usize, inst: DecodedClwb) -> Result<(), ()> {
    let modrm_addr = rip + inst.modrm_offset;
    let mut old_protect = PAGE_PROTECTION_FLAGS(0);
    // SAFETY: Protecting the single ModRM byte leaves execute and read enabled
    // on the page while allowing an atomic 1-byte store.
    unsafe {
        VirtualProtect(
            modrm_addr as *const c_void,
            1,
            PAGE_EXECUTE_READWRITE,
            &mut old_protect,
        )
    }
    .map_err(|_| ())?;

    // SAFETY: `modrm_addr` is writable and valid for 1 byte; flipping bit 3
    // turns ModRM `/6` (`clwb`) into `/7` (`clflushopt`) atomically.
    unsafe {
        let slot = AtomicU8::from_ptr(modrm_addr as *mut u8);
        let old_byte = slot.load(Ordering::Relaxed);
        slot.store(old_byte | 0x08, Ordering::Release);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(rip as *const c_void), inst.len);
        let mut ignored = PAGE_PROTECTION_FLAGS(0);
        let _ = VirtualProtect(modrm_addr as *const c_void, 1, old_protect, &mut ignored);
    }
    Ok(())
}

struct PatchState {
    patched: Vec<usize>,
    skipped: Vec<usize>,
}

fn handle_with_features(
    context: &mut CONTEXT,
    start: usize,
    end: usize,
    features: CpuCacheFeatures,
    state_lock: &Mutex<PatchState>,
) -> (bool, Option<String>) {
    if features.clwb {
        return (false, None);
    }
    let rip = context.Rip as usize;
    if !(start..end).contains(&rip) {
        return (false, None);
    }

    let mut state = state_lock.lock().unwrap_or_else(|e| e.into_inner());
    let (buf, avail) = read_code_bytes(rip, end);
    if avail == 0 {
        return (false, None);
    }
    let bytes = &buf[..avail];

    match decide(bytes, rip, features, &state.patched) {
        Action::Unhandled => (false, None),
        Action::Retry => (true, None),
        Action::PatchToClflushopt(inst) => {
            // SAFETY: `inst` was decoded from `rip..rip + avail`.
            if unsafe { patch_clwb_to_clflushopt(rip, inst) }.is_ok() {
                state.patched.push(rip);
                (
                    true,
                    Some(format!(
                        "rewrote unsupported clwb at RVA {:#x} to clflushopt",
                        rip - start
                    )),
                )
            } else {
                context.Rip = context.Rip.wrapping_add(inst.len as u64);
                let line = if !state.skipped.contains(&rip) {
                    state.skipped.push(rip);
                    Some(format!(
                        "skipped unsupported clwb at RVA {:#x} (page not writable)",
                        rip - start
                    ))
                } else {
                    None
                };
                (true, line)
            }
        }
        Action::Skip(len) => {
            context.Rip = context.Rip.wrapping_add(len as u64);
            let line = if !state.skipped.contains(&rip) {
                state.skipped.push(rip);
                Some(format!(
                    "skipped unsupported clwb at RVA {:#x} (host lacks clflushopt)",
                    rip - start
                ))
            } else {
                None
            };
            (true, line)
        }
    }
}

/// Handle `EXCEPTION_ILLEGAL_INSTRUCTION` inside `[start, end)` when caused by
/// `clwb` on a host CPU without CLWB support. Returns `true` if execution
/// should resume.
pub(crate) fn handle_illegal_instruction(context: &mut CONTEXT, start: usize, end: usize) -> bool {
    let features = cpu_features();
    if features.clwb {
        return false;
    }
    static STATE: Mutex<PatchState> =
        Mutex::new(PatchState { patched: Vec::new(), skipped: Vec::new() });

    let (handled, log_line) = handle_with_features(context, start, end, features, &STATE);
    if let Some(line) = log_line {
        crate::diagnostics::log(&line);
    }
    handled
}

#[cfg(test)]
mod tests {
    use super::{
        Action, CpuCacheFeatures, DecodedClwb, PatchState, cpu_features, decide, decode_clwb,
        handle_with_features, patch_clwb_to_clflushopt,
    };
    use std::sync::{Arc, Barrier, Mutex};
    use windows::Win32::System::Diagnostics::Debug::CONTEXT;
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READ,
        PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_PROTECTION_FLAGS, PAGE_READWRITE,
        VirtualAlloc, VirtualFree, VirtualProtect, VirtualQuery,
    };

    #[test]
    fn decodes_issue_71_and_addressing_forms() {
        // Exact encodings reported on pre-Ice Lake Intel CPUs (Issue #71),
        // followed by trailing instruction bytes that must not be consumed.
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x31, 0x49, 0x83]),
            Some(DecodedClwb { len: 4, modrm_offset: 3 })
        );
        assert_eq!(
            decode_clwb(&[0x66, 0x41, 0x0F, 0xAE, 0x31, 0x49]),
            Some(DecodedClwb { len: 5, modrm_offset: 4 })
        );
        assert_eq!(
            decode_clwb(&[0x66, 0x41, 0x0F, 0xAE, 0x30, 0xC3]),
            Some(DecodedClwb { len: 5, modrm_offset: 4 })
        );
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x30]),
            Some(DecodedClwb { len: 4, modrm_offset: 3 })
        );

        // RIP-relative: clwb [rip + disp32]
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x35, 0x10, 0x20, 0x30, 0x40, 0x90]),
            Some(DecodedClwb { len: 8, modrm_offset: 3 })
        );
        // SIB without displacement: clwb [rsp]
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x34, 0x24]),
            Some(DecodedClwb { len: 5, modrm_offset: 3 })
        );
        // SIB with mod=0, base=5 (disp32): clwb [disp32]
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x34, 0x25, 0x00, 0x10, 0x00, 0x00]),
            Some(DecodedClwb { len: 9, modrm_offset: 3 })
        );
        // mod=1 disp8: clwb [rcx + 0x40]
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0x71, 0x40]),
            Some(DecodedClwb { len: 5, modrm_offset: 3 })
        );
        // mod=1 SIB + disp8 with REX: clwb [r12 + 0x40]
        assert_eq!(
            decode_clwb(&[0x66, 0x41, 0x0F, 0xAE, 0x74, 0x24, 0x40]),
            Some(DecodedClwb { len: 7, modrm_offset: 4 })
        );
        // mod=2 disp32: clwb [rcx + 0x80]
        assert_eq!(
            decode_clwb(&[0x66, 0x0F, 0xAE, 0xB1, 0x80, 0x00, 0x00, 0x00]),
            Some(DecodedClwb { len: 8, modrm_offset: 3 })
        );
        // mod=2 SIB + disp32 with REX + segment + address-size override
        assert_eq!(
            decode_clwb(&[0x65, 0x67, 0x66, 0x43, 0x0F, 0xAE, 0xB4, 0xC8, 0x10, 0, 0, 0]),
            Some(DecodedClwb { len: 12, modrm_offset: 6 })
        );
    }

    #[test]
    fn exhaustive_modrm_prefix_and_sib_decoder_adversarial() {
        // 1. Exhaustive ModRM sweep (0x00..=0xFF): only mod != 3 && reg == 6
        //    (exactly 24 values) may decode as CLWB.
        let mut accepted_modrm = 0usize;
        for modrm in 0u8..=255 {
            let bytes = [0x66, 0x0F, 0xAE, modrm, 0x24, 0, 0, 0, 0, 0];
            let mod_bits = modrm >> 6;
            let reg_bits = (modrm >> 3) & 7;
            let rm_bits = modrm & 7;
            let decoded = decode_clwb(&bytes);
            if mod_bits != 3 && reg_bits == 6 {
                accepted_modrm += 1;
                let expected_len = 4
                    + usize::from(rm_bits == 4)
                    + match (mod_bits, rm_bits) {
                        (0, 5) => 4,
                        (0, _) => 0, // sib=0x24 has base=4 != 5
                        (1, _) => 1,
                        (2, _) => 4,
                        _ => unreachable!(),
                    };
                assert_eq!(
                    decoded,
                    Some(DecodedClwb { len: expected_len, modrm_offset: 3 }),
                    "modrm={modrm:#04x}"
                );
                // Flipping bit 3 must turn /6 into /7 and be rejected.
                let mut rewritten = bytes;
                rewritten[3] |= 0x08;
                assert_eq!(decode_clwb(&rewritten), None, "rewritten modrm={modrm:#04x}");
            } else {
                assert_eq!(decoded, None, "unexpectedly accepted modrm={modrm:#04x}");
            }
        }
        assert_eq!(accepted_modrm, 24);

        // 2. Exhaustive leading-byte sweep before `66 0F AE 31`: only the 8
        //    valid legacy prefixes (Groups 2/3/4) may be accepted; Group 1
        //    (F0/F2/F3), REX before 0x66, and opcodes must be rejected.
        let valid_prefixes = [0x26u8, 0x2E, 0x36, 0x3E, 0x64, 0x65, 0x66, 0x67];
        for lead in 0u8..=255 {
            let bytes = [lead, 0x66, 0x0F, 0xAE, 0x31];
            let decoded = decode_clwb(&bytes);
            if valid_prefixes.contains(&lead) {
                assert_eq!(
                    decoded,
                    Some(DecodedClwb { len: 5, modrm_offset: 4 }),
                    "lead={lead:#04x}"
                );
            } else {
                assert_eq!(decoded, None, "unexpectedly accepted lead={lead:#04x}");
            }
        }

        // 3. Exhaustive SIB sweep across mod=0, 1, 2 (768 combinations).
        for mod_bits in 0u8..=2 {
            let modrm = (mod_bits << 6) | (6 << 3) | 4;
            for sib in 0u8..=255 {
                let bytes = [0x66, 0x0F, 0xAE, modrm, sib, 0, 0, 0, 0];
                let expected_disp = match mod_bits {
                    0 => if (sib & 7) == 5 { 4 } else { 0 },
                    1 => 1,
                    2 => 4,
                    _ => unreachable!(),
                };
                let expected_len = 5 + expected_disp;
                assert_eq!(
                    decode_clwb(&bytes),
                    Some(DecodedClwb { len: expected_len, modrm_offset: 3 })
                );
                // Truncating by 1 byte must be rejected.
                assert_eq!(decode_clwb(&bytes[..expected_len - 1]), None);
            }
        }
    }

    #[test]
    fn rejects_non_clwb_and_malformed_encodings() {
        // Missing 0x66 (xsaveopt [rcx])
        assert_eq!(decode_clwb(&[0x0F, 0xAE, 0x31]), None);
        // Already clflushopt (/7 instead of /6)
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE, 0x39]), None);
        // Register operand (mod == 3 -> tpause eax)
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE, 0xF0]), None);
        // Forbidden Group 1 prefixes (LOCK, REPNE, REPE)
        assert_eq!(decode_clwb(&[0xF0, 0x66, 0x0F, 0xAE, 0x31]), None);
        assert_eq!(decode_clwb(&[0xF2, 0x66, 0x0F, 0xAE, 0x31]), None);
        assert_eq!(decode_clwb(&[0xF3, 0x66, 0x0F, 0xAE, 0x31]), None);
        // Prefix after REX or duplicate REX
        assert_eq!(decode_clwb(&[0x41, 0x66, 0x0F, 0xAE, 0x31]), None);
        assert_eq!(decode_clwb(&[0x66, 0x40, 0x41, 0x0F, 0xAE, 0x31]), None);
        // Exact 15-byte boundary accepted; 16-byte rejected.
        let exact_15 = [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x41, 0x0F, 0xAE, 0xB4, 0x24, 1, 2, 3, 4];
        assert_eq!(decode_clwb(&exact_15), Some(DecodedClwb { len: 15, modrm_offset: 9 }));
        let over_16 = [0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x41, 0x0F, 0xAE, 0xB4, 0x24, 1, 2, 3, 4];
        assert_eq!(decode_clwb(&over_16), None);
        // Truncated encodings
        assert_eq!(decode_clwb(&[]), None);
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE]), None);
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE, 0x34]), None); // missing SIB
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE, 0x71]), None); // missing disp8
        assert_eq!(decode_clwb(&[0x66, 0x0F, 0xAE, 0x35, 0x00, 0x00]), None); // short disp32
    }

    #[test]
    fn decision_handles_cpu_capabilities_and_concurrent_retry() {
        let clwb_bytes = [0x66, 0x41, 0x0F, 0xAE, 0x31];
        let patched_bytes = [0x66, 0x41, 0x0F, 0xAE, 0x39];
        let rip = 0x14000_1000;

        // CPU with native CLWB never intercepts.
        let with_clwb = CpuCacheFeatures { clwb: true, clflushopt: true };
        assert_eq!(decide(&clwb_bytes, rip, with_clwb, &[]), Action::Unhandled);

        // Pre-Ice Lake Skylake..Coffee Lake: patches to clflushopt.
        let skylake = CpuCacheFeatures { clwb: false, clflushopt: true };
        assert_eq!(
            decide(&clwb_bytes, rip, skylake, &[]),
            Action::PatchToClflushopt(DecodedClwb { len: 5, modrm_offset: 4 })
        );

        // Second thread arriving after the first thread patched the same Rip
        // sees `clflushopt` bytes and retries cleanly instead of crashing.
        assert_eq!(decide(&patched_bytes, rip, skylake, &[rip]), Action::Retry);
        // Unrelated clflushopt site not in `patched` is never claimed.
        assert_eq!(decide(&patched_bytes, rip + 0x10, skylake, &[rip]), Action::Unhandled);

        // Haswell/Broadwell (neither CLWB nor CLFLUSHOPT): skips instruction.
        let haswell = CpuCacheFeatures { clwb: false, clflushopt: false };
        assert_eq!(decide(&clwb_bytes, rip, haswell, &[]), Action::Skip(5));
    }

    #[test]
    fn patches_rx_code_page_restores_protection_and_runs_clflushopt() {
        // Allocate a private page with `clwb [rcx]; ret`, lock it as RX, patch
        // in place, confirm protection restoration, and execute when supported.
        let page = unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!page.is_null());

        let code: [u8; 5] = [0x66, 0x0F, 0xAE, 0x31, 0xC3];
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), page.cast::<u8>(), code.len());
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(page, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }

        let inst = decode_clwb(&code).unwrap();
        let patched = unsafe { patch_clwb_to_clflushopt(page as usize, inst) };
        assert!(patched.is_ok());

        let after = unsafe { core::slice::from_raw_parts(page.cast::<u8>(), code.len()) };
        assert_eq!(after, &[0x66, 0x0F, 0xAE, 0x39, 0xC3]);

        let mut info = MEMORY_BASIC_INFORMATION::default();
        let queried = unsafe { VirtualQuery(Some(page), &mut info, size_of_val(&info)) };
        assert_ne!(queried, 0);
        assert_eq!(info.Protect, PAGE_EXECUTE_READ);

        if cpu_features().clflushopt {
            let line = [0u8; 64];
            let run: extern "win64" fn(*const u8) = unsafe { core::mem::transmute(page) };
            run(line.as_ptr());
        }

        unsafe {
            VirtualFree(page, 0, MEM_RELEASE).unwrap();
        }
    }

    #[test]
    fn straddling_page_boundary_guard_and_noaccess_adversarial() {
        // Allocate two adjacent 4 KiB pages with different protections so
        // VirtualQuery stops at the first page boundary. Place `clwb [rcx]; ret`
        // starting 2 bytes before the boundary (bytes 0..2 on page 0, ModRM+ret on page 1).
        let base = unsafe { VirtualAlloc(None, 8192, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!base.is_null());
        let start = base as usize;
        let end = start + 8192;
        let straddle = start + 4094;
        let code: [u8; 5] = [0x66, 0x0F, 0xAE, 0x31, 0xC3];
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), straddle as *mut u8, code.len());
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(base, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
            VirtualProtect((start + 4096) as *const _, 4096, PAGE_EXECUTE_READWRITE, &mut old)
                .unwrap();
        }

        let state = Mutex::new(PatchState { patched: Vec::new(), skipped: Vec::new() });
        let skylake = CpuCacheFeatures { clwb: false, clflushopt: true };
        let mut ctx = CONTEXT { Rip: straddle as u64, ..Default::default() };
        let (handled, log) = handle_with_features(&mut ctx, start, end, skylake, &state);
        assert!(handled);
        assert!(log.is_some());
        assert_eq!(ctx.Rip, straddle as u64);

        let after = unsafe { core::slice::from_raw_parts(straddle as *const u8, code.len()) };
        assert_eq!(after, &[0x66, 0x0F, 0xAE, 0x39, 0xC3]);
        if cpu_features().clflushopt {
            let line = [0u8; 64];
            let run: extern "win64" fn(*const u8) = unsafe { core::mem::transmute(straddle) };
            run(line.as_ptr());
        }

        // Now mark page 1 PAGE_NOACCESS and place another CLWB straddling into it:
        // must safely refuse without faulting.
        let straddle2 = start + 4093;
        unsafe {
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(base, 4096, PAGE_READWRITE, &mut old).unwrap();
            core::ptr::copy_nonoverlapping(code.as_ptr(), straddle2 as *mut u8, 3);
            VirtualProtect(base, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
            VirtualProtect((start + 4096) as *const _, 4096, PAGE_NOACCESS, &mut old).unwrap();
        }
        let mut ctx2 = CONTEXT { Rip: straddle2 as u64, ..Default::default() };
        assert_eq!(handle_with_features(&mut ctx2, start, end, skylake, &state), (false, None));
        assert_eq!(ctx2.Rip, straddle2 as u64);

        // Guard page on page 1 must also be rejected without triggering the guard.
        unsafe {
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(
                (start + 4096) as *const _,
                4096,
                PAGE_EXECUTE_READ | PAGE_GUARD,
                &mut old,
            )
            .unwrap();
        }
        assert_eq!(handle_with_features(&mut ctx2, start, end, skylake, &state), (false, None));

        // Out-of-bounds Rip and non-CLWB instructions (`ud2`, `lock clwb`) must be rejected.
        let mut oob = CONTEXT { Rip: (end + 16) as u64, ..Default::default() };
        assert_eq!(handle_with_features(&mut oob, start, end, skylake, &state), (false, None));

        unsafe {
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(base, 4096, PAGE_READWRITE, &mut old).unwrap();
            let bad: [u8; 7] = [0x0F, 0x0B, 0xF0, 0x66, 0x0F, 0xAE, 0x31]; // ud2; lock clwb [rcx]
            core::ptr::copy_nonoverlapping(bad.as_ptr(), base.cast::<u8>(), bad.len());
            VirtualProtect(base, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }
        let mut ud2_ctx = CONTEXT { Rip: start as u64, ..Default::default() };
        assert_eq!(handle_with_features(&mut ud2_ctx, start, end, skylake, &state), (false, None));
        let mut lock_ctx = CONTEXT { Rip: (start + 2) as u64, ..Default::default() };
        assert_eq!(handle_with_features(&mut lock_ctx, start, end, skylake, &state), (false, None));

        unsafe {
            VirtualFree(base, 0, MEM_RELEASE).unwrap();
        }
    }

    #[test]
    fn eight_threads_racing_same_clwb_patch_once_and_all_resume() {
        let page = unsafe { VirtualAlloc(None, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        assert!(!page.is_null());
        let start = page as usize;
        let end = start + 4096;
        let code: [u8; 5] = [0x66, 0x0F, 0xAE, 0x31, 0xC3];
        unsafe {
            core::ptr::copy_nonoverlapping(code.as_ptr(), page.cast::<u8>(), code.len());
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(page, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }

        let state = Arc::new(Mutex::new(PatchState { patched: Vec::new(), skipped: Vec::new() }));
        let barrier = Arc::new(Barrier::new(8));
        let skylake = CpuCacheFeatures { clwb: false, clflushopt: true };

        let mut handles = Vec::new();
        for _ in 0..8 {
            let state = Arc::clone(&state);
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                let mut ctx = CONTEXT { Rip: start as u64, ..Default::default() };
                barrier.wait();
                let (handled, log) = handle_with_features(&mut ctx, start, end, skylake, &state);
                assert!(handled);
                assert_eq!(ctx.Rip, start as u64);
                log.is_some()
            }));
        }

        let log_count = handles.into_iter().map(|h| h.join().unwrap()).filter(|&logged| logged).count();
        // Exactly one thread performs the patch & logs; the other 7 retry cleanly.
        assert_eq!(log_count, 1);
        assert_eq!(state.lock().unwrap().patched, vec![start]);

        let after = unsafe { core::slice::from_raw_parts(page.cast::<u8>(), code.len()) };
        assert_eq!(after, &[0x66, 0x0F, 0xAE, 0x39, 0xC3]);

        let mut info = MEMORY_BASIC_INFORMATION::default();
        assert_ne!(unsafe { VirtualQuery(Some(page), &mut info, size_of_val(&info)) }, 0);
        assert_eq!(info.Protect, PAGE_EXECUTE_READ);

        // Haswell fallback path advances Rip by instruction length without patching,
        // and logs only once across repeated hits on the same hot loop site.
        unsafe {
            let mut old = PAGE_PROTECTION_FLAGS(0);
            VirtualProtect(page, 4096, PAGE_READWRITE, &mut old).unwrap();
            core::ptr::copy_nonoverlapping(code.as_ptr(), page.cast::<u8>(), code.len());
            VirtualProtect(page, 4096, PAGE_EXECUTE_READ, &mut old).unwrap();
        }
        let haswell_state = Mutex::new(PatchState { patched: Vec::new(), skipped: Vec::new() });
        let haswell = CpuCacheFeatures { clwb: false, clflushopt: false };
        let mut hctx = CONTEXT { Rip: start as u64, ..Default::default() };
        let (h1, l1) = handle_with_features(&mut hctx, start, end, haswell, &haswell_state);
        assert!(h1 && l1.is_some());
        assert_eq!(hctx.Rip, (start + 4) as u64);
        // Second hit on same site skips silently without spamming logs.
        hctx.Rip = start as u64;
        let (h2, l2) = handle_with_features(&mut hctx, start, end, haswell, &haswell_state);
        assert!(h2 && l2.is_none());
        assert_eq!(hctx.Rip, (start + 4) as u64);
        // Bytes untouched on Haswell path.
        assert_eq!(unsafe { core::slice::from_raw_parts(page.cast::<u8>(), code.len()) }, &code);

        unsafe {
            VirtualFree(page, 0, MEM_RELEASE).unwrap();
        }
    }
}
