//! Split wave64: which compare masks need their high word exchanged.
//!
//! Adapted from xcl b4019c4 "masks read only lane by lane stay in their half" and 7bea019
//! (x64_masks.rs, GPL-2.0, copied with credit under the project's override). In our lane
//! model a compare writes this lane's bit into the low register and, for the high register,
//! the high lanes' ballot (five-star 2781f9c shadow), which in the split form costs one
//! exchange. When no path reads the high register before it is overwritten, the shadow is
//! dead and the exchange can be skipped.
//!
//! xcl's rules, as they apply here:
//! - per-lane readers (`v_cndmask_b32` selector, the carry-in of `v_add_co_ci` /
//!   `v_sub(rev)_co_ci`) read only the low register's lane bit, so they do not read the high one;
//! - every other operand naming the register (SALU, SMEM offsets, SDWA scalar sources,
//!   VALU scalar sources) is a read;
//! - `s_cmov*`, `s_addk`, `s_mulk`, `s_bitset*`, `s_movreld*` keep the old value, so they are
//!   reads, not overwrites; `s_movrels*` reads every register from its base;
//! - EXEC is always live; an indirect jump, the program end, a branch or a branch target
//!   (where another path joins) count as a read.
//!
//! xcl solves the liveness globally per basic block (7bea019); this pass looks only at the
//! straight-line code after the compare and calls the shadow live at any control-flow edge,
//! which can only keep more exchanges than xcl, never fewer.
//! TB_WAVE64_MASK_LIVENESS=1 enables it (opt-in).

use super::{reads_sgpr, Enc, Insn, Opnd};

pub(super) fn on() -> bool {
    std::env::var("TB_WAVE64_MASK_LIVENESS").is_ok_and(|v| v.trim() == "1")
}

fn per_lane_reader(m: &str) -> bool {
    m.starts_with("v_cndmask_b32") || m.starts_with("v_add_co_ci_u32") || m.starts_with("v_sub_co_ci_u32")
        || m.starts_with("v_subrev_co_ci_u32")
}

fn keeps_destination(m: &str) -> bool {
    m.starts_with("s_cmov") || m.starts_with("s_addk") || m.starts_with("s_mulk") || m.starts_with("s_bitset")
        || m.starts_with("s_movrel")
}

/// Whether `ins` reads SGPR `n` (the high word of the pair at `n - 1`) as a whole value.
fn reads_high(ins: &Insn, n: u8) -> bool {
    let m = ins.mnemonic.as_str();
    if m.contains("movrel") || keeps_destination(m) {
        return ins.src.iter().chain(ins.dst.iter()).any(|o| matches!(*o, Opnd::Sgpr(k) if k <= n));
    }
    if per_lane_reader(m) {
        // The mask operand is the last source (a pair at n - 1): only its low lane bit is read.
        return ins.src.iter().enumerate().any(|(i, o)| match *o {
            Opnd::Sgpr(k) if i + 1 == ins.src.len() && k + 1 == n => false,
            Opnd::Sgpr(k) => k == n || (i + 1 == ins.src.len() && k == n),
            _ => false,
        });
    }
    reads_sgpr(ins, n) || ins.src.iter().any(|o| matches!(*o, Opnd::Sgpr(k) if k == n))
}

/// Whether `ins` replaces SGPR `n` whatever its old value.
fn overwrites(ins: &Insn, n: u8) -> bool {
    let m = ins.mnemonic.as_str();
    if keeps_destination(m) || ins.enc == Enc::Sopp || m.contains("sdwa") {
        return false;
    }
    let width: u8 = if m.ends_with("64") || m.starts_with("v_cmp") { 2 } else { 1 };
    let full_kind = m.starts_with("s_mov_") || m.starts_with("s_and_") || m.starts_with("s_or_")
        || m.starts_with("s_xor_") || m.starts_with("s_andn2_") || m.starts_with("s_orn2_")
        || m.starts_with("s_not_") || (m.starts_with("v_cmp_") && !m.starts_with("v_cmpx_"))
        || m.starts_with("v_readlane_b32") || m.starts_with("v_readfirstlane_b32") || m.starts_with("s_load_")
        || m.starts_with("s_buffer_load_");
    full_kind && ins.dst.first().is_some_and(|o| matches!(*o, Opnd::Sgpr(k) if k <= n && n < k.saturating_add(width.max(load_width(m)))))
}

fn load_width(m: &str) -> u8 {
    match m.rsplit('_').next() {
        Some("dwordx2") => 2, Some("dwordx4") => 4, Some("dwordx8") => 8, Some("dwordx16") => 16, _ => 1,
    }
}

/// Whether the compare at `at` writes a high word `dh` that nothing reads before it is overwritten.
pub(super) fn high_dead(insns: &[Insn], at: usize, dh: &Opnd) -> bool {
    let Opnd::Sgpr(n) = *dh else { return false };
    if n >= 106 { return false; } // VCC, EXEC and specials stay exchanged.
    let joins = super::branch_targets(insns);
    for ins in &insns[at + 1..] {
        if joins.contains(&(ins.offset as i64)) || ins.enc == Enc::Sopp || ins.mnemonic.starts_with("s_setpc")
            || ins.mnemonic.starts_with("s_swappc") || ins.mnemonic.starts_with("s_cbranch") {
            return false;
        }
        if reads_high(ins, n) { return false; }
        if overwrites(ins, n) { return true; }
    }
    false
}
