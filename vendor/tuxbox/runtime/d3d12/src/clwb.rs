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
#[path = "tests/clwb.rs"]
mod tests;
