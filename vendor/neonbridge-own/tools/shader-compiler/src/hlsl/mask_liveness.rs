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
//! Like xcl 7bea019 the liveness is solved over the whole program: a backward fixpoint of
//! whole-live SGPRs after each instruction across branches, joins and loops, computed once per
//! program. A branch only follows its edges (s_waitcnt, s_nop and other SOPP non-branches are no
//! edge); an indirect jump (`s_setpc`, `s_swappc`, `s_rfe`) or a branch whose target is not an
//! instruction makes every SGPR live. The end reads nothing: SGPRs are HLSL locals here, not
//! the xcl runner's outputs, and a loop guard's early exit only leads to the end.
//! TB_WAVE64_MASK_GLOBAL=0 keeps the earlier straight-line scan (live at any branch or join).
//! On by default; TB_WAVE64_MASK_LIVENESS=0 turns it off.

use super::{reads_sgpr, Enc, Insn, Opnd};

pub(super) fn on() -> bool {
    std::env::var("TB_WAVE64_MASK_LIVENESS").map_or(true, |v| v.trim() != "0")
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
    if global() {
        return with_live_out(insns, |live| live.get(at).is_some_and(|&l| l & (1u128 << n) == 0));
    }
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

fn global() -> bool {
    std::env::var("TB_WAVE64_MASK_GLOBAL").map_or(true, |v| v.trim() != "0")
}

/// SGPRs 0..106 (VCC and up are never skipped).
const TRACKED: u8 = 106;
const ALL: u128 = (1u128 << TRACKED) - 1;

/// Successors of instruction `i`; None: unknown (an indirect jump or a target that is no
/// instruction), every SGPR live.
fn successors(insns: &[Insn], index: &std::collections::HashMap<i64, usize>, i: usize) -> Option<Vec<usize>> {
    let ins = &insns[i];
    let m = ins.mnemonic.as_str();
    if m.starts_with("s_setpc") || m.starts_with("s_swappc") || m.starts_with("s_rfe") {
        return None;
    }
    let mut out = Vec::new();
    if m == "s_branch" || m.starts_with("s_cbranch_") {
        out.push(*index.get(&(ins.offset as i64 + 4 + ins.aux.imm * 4))?);
    }
    if !(m == "s_branch" || m.starts_with("s_endpgm")) && i + 1 < insns.len() {
        out.push(i + 1);
    }
    Some(out)
}

/// xcl 7bea019 whole_live_out: the SGPRs some path may read whole after each instruction.
fn live_out(insns: &[Insn]) -> Vec<u128> {
    let index: std::collections::HashMap<i64, usize> = insns.iter().enumerate().map(|(i, s)| (s.offset as i64, i)).collect();
    let (mut uses, mut defs) = (vec![0u128; insns.len()], vec![0u128; insns.len()]);
    for (i, ins) in insns.iter().enumerate() {
        for n in 0..TRACKED {
            if reads_high(ins, n) {
                uses[i] |= 1 << n;
            } else if overwrites(ins, n) {
                defs[i] |= 1 << n;
            }
        }
    }
    let succ: Vec<Option<Vec<usize>>> = (0..insns.len()).map(|i| successors(insns, &index, i)).collect();
    let mut out = vec![0u128; insns.len()];
    let mut live_in = vec![0u128; insns.len()];
    loop {
        let mut changed = false;
        for i in (0..insns.len()).rev() {
            let o = match &succ[i] {
                None => ALL,
                Some(s) => s.iter().fold(0, |a, &j| a | live_in[j]),
            };
            let l = uses[i] | (o & !defs[i]);
            if o != out[i] || l != live_in[i] {
                out[i] = o;
                live_in[i] = l;
                changed = true;
            }
        }
        if !changed {
            return out;
        }
    }
}

/// The program's live-out sets, solved once per program (keyed by the slice).
fn with_live_out<R>(insns: &[Insn], f: impl FnOnce(&[u128]) -> R) -> R {
    use std::cell::RefCell;
    thread_local! {
        static CACHE: RefCell<Option<((usize, usize, u64, u64), Vec<u128>)>> = const { RefCell::new(None) };
    }
    // Keyed by the instructions' content, not the slice's address: a later program's list
    // can sit at the same address with the same length (churn-check saw stale results).
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for i in insns {
            (i.offset, i.op, i.mnemonic.as_str(), i.aux.imm, &i.raw).hash(&mut h);
        }
        (insns.as_ptr() as usize, insns.len(), h.finish(), 0u64)
    };
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.as_ref().is_none_or(|(k, _)| *k != key) {
            *c = Some((key, live_out(insns)));
        }
        f(&c.as_ref().expect("solved").1)
    })
}
