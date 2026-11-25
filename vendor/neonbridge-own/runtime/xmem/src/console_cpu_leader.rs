//! Pieces of ztrx victory-loader's console-instruction emulation used by
//! `console_cpu.rs`, each behind its own switch there.
//! Copied from ztrx victory-loader console_insn.rs and crash.rs (GPL-3.0,
//! credit to its authors). Differences: no ASKED/EMULATED counters (we have no
//! tracing here), and the VICTORY_CLZERO_ASK switch is not read: the caching
//! is always on when these paths are.
use std::cell::Cell;

/// Copied from ztrx victory-loader console_insn.rs:31-33 (GPL-3.0, credit to its authors).
const ANSWERS: u32 = 4096;

thread_local! {
    // Copied from ztrx victory-loader console_insn.rs:35-41 (GPL-3.0, credit to its authors).
    static WRITABLE: Cell<([(usize, usize); 4], u32)> = const { Cell::new(([(0, 0); 4], 0)) };
    static CODE: Cell<[(usize, usize); 8]> = const { Cell::new([(0, 0); 8]) };
}

/// Copied from ztrx victory-loader console_insn.rs:49-57 (GPL-3.0, credit to its authors).
fn readable(va: usize, bytes: usize) -> bool {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
    // SAFETY: plain data filled in by the query.
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let found = unsafe { VirtualQuery(va as *const _, &mut m, size_of_val(&m)) } != 0;
    found && m.State == MEM_COMMIT && m.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && m.BaseAddress as usize + m.RegionSize >= va + bytes
}

/// The faulting instruction's bytes (at most 16) copied into `out`, only
/// after a readability check, cached per Rip; returns how many are readable.
/// Copied from ztrx victory-loader console_insn.rs:107-127 (GPL-3.0, credit
/// to its authors); difference: the bytes are copied out instead of sliced.
pub(super) fn code_bytes(rip: usize, out: &mut [u8; 16]) -> usize {
    let known = CODE.with(Cell::get);
    let n = match known.iter().find(|&&(at, n)| at == rip && n > 0) {
        Some(&(_, n)) => n,
        None => {
            if !readable(rip, 1) {
                return 0;
            }
            let n = (1..=16).rev().find(|&n| readable(rip, n)).unwrap_or(0);
            let mut known = known;
            known.rotate_right(1);
            known[0] = (rip, n);
            CODE.with(|c| c.set(known));
            n
        }
    };
    // SAFETY: `n` bytes at `rip` are readable.
    unsafe { std::ptr::copy_nonoverlapping(rip as *const u8, out.as_mut_ptr(), n) };
    n
}

/// Copied from ztrx victory-loader console_insn.rs:59-61 (GPL-3.0, credit to its authors).
pub(super) fn writable(va: usize, bytes: usize) -> bool {
    writable_to(va, va + bytes) == Some(va + bytes)
}

/// Where committed, writable memory from `va` ends, at most `end`.
/// Copied from ztrx victory-loader console_insn.rs:69-98 (GPL-3.0, credit to its authors).
pub(super) fn writable_to(va: usize, end: usize) -> Option<usize> {
    use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE_READWRITE, PAGE_READWRITE, VirtualQuery};
    let (regions, answers) = WRITABLE.with(Cell::get);
    if let Some(&(_, to)) = regions.iter().find(|&&(from, to)| from <= va && va < to) {
        let next = if answers >= ANSWERS { ([(0, 0); 4], 0) } else { (regions, answers + 1) };
        WRITABLE.with(|w| w.set(next));
        return Some(to.min(end));
    }
    // SAFETY: plain data filled in by the query.
    let mut m: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: a valid out pointer of the right size.
    let found = unsafe { VirtualQuery(va as *const _, &mut m, size_of_val(&m)) } != 0;
    if !found || m.State != MEM_COMMIT || m.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) == 0 {
        return None;
    }
    let (from, to) = (m.BaseAddress as usize, m.BaseAddress as usize + m.RegionSize);
    WRITABLE.with(|w| {
        let (mut regions, answers) = w.get();
        regions.rotate_right(1);
        regions[0] = (from, to);
        w.set((regions, answers));
    });
    Some(to.min(end))
}

/// `clwb [base]; add base,0x40; cmp base,limit; jbe top`.
/// Copied from ztrx victory-loader crash.rs:235-277 (GPL-3.0, credit to its authors).
pub(super) struct ClwbLoop {
    pub base: u8,
    pub limit: u8,
    /// Bytes from `clwb` to the instruction after `jbe`.
    pub len: usize,
}

/// Copied from ztrx victory-loader crash.rs:242-277 (GPL-3.0, credit to its authors).
pub(super) fn clwb_loop(code: &[u8]) -> Option<ClwbLoop> {
    // clwb [base]: 66 [40|41] 0f ae, ModRM mod 00, reg 6, no SIB or displacement.
    let (rex, at) = match code.get(1)? {
        0x40 | 0x41 => (code[1], 2),
        _ => (0, 1),
    };
    if *code.first()? != 0x66 || code.get(at..at + 2)? != [0x0f, 0xae] {
        return None;
    }
    let modrm = *code.get(at + 2)?;
    let rm = modrm & 7;
    if modrm >> 6 != 0 || (modrm >> 3) & 7 != 6 || rm == 4 || rm == 5 {
        return None;
    }
    let base = rm | (rex & 1) << 3;
    // add base, 0x40: REX.W(+B) 83 /0 ib.
    let add = at + 3;
    if *code.get(add..add + 4)? != [0x48 | base >> 3, 0x83, 0xc0 | (base & 7), 0x40] {
        return None;
    }
    // cmp base, limit: REX.W 39 /r with base in r/m.
    let cmp = add + 4;
    let &[cmp_rex @ 0x48..=0x4f, 0x39, cmp_modrm] = code.get(cmp..cmp + 3)? else {
        return None;
    };
    if cmp_modrm >> 6 != 3 || (cmp_modrm & 7 | (cmp_rex & 1) << 3) != base {
        return None;
    }
    let limit = (cmp_modrm >> 3) & 7 | (cmp_rex & 4) << 1;
    // jbe top.
    let jump = cmp + 3;
    let len = jump + 2;
    if *code.get(jump)? != 0x76 || i64::from(*code.get(jump + 1)? as i8) != -(len as i64) {
        return None;
    }
    (limit != base).then_some(ClwbLoop { base, limit, len })
}
