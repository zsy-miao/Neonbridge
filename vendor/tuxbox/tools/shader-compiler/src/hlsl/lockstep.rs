//! Wave-wide branches for programs whose wave operations sit in control flow
//! the targeted guards cannot verify.
//!
//! The console branches once per wave. The generic emitter branches once per
//! invocation and keeps only the lane's own bit of EXEC and other lane masks.
//! Here every invocation of a wave follows the same path instead: a branch on a
//! mask votes across the wave, and so does the condition code a mask operation
//! sets. That is only faithful while every scalar register holds either a value
//! all lanes agree on or a lane mask, so a program that treats a mask as a
//! number, or a number as a mask, is refused.

use std::collections::BTreeMap;

use super::{ray_resolve, Emitter, Enc, Insn, Opnd, COUNT_LANES};

/// What a scalar register holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// The same value in every lane.
    Uniform,
    /// A lane mask, of which each invocation holds its own bit.
    Mask,
    /// All zeros or all ones, which reads correctly either way.
    Constant,
    /// A uniform value on one path and a mask on another.
    Conflict,
}

impl Class {
    fn join(self, other: Self) -> Self {
        use Class::*;
        match (self, other) {
            (a, b) if a == b => a,
            (Constant, b) => b,
            (a, Constant) => a,
            _ => Conflict,
        }
    }

    fn is_value(self) -> bool {
        matches!(self, Class::Uniform | Class::Constant)
    }

    fn is_mask(self) -> bool {
        matches!(self, Class::Mask | Class::Constant)
    }
}

/// A condition code to recompute across the wave after one instruction.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Vote {
    pub mask: Opnd,
    /// The code is set when no lane's bit is.
    pub none: bool,
}

const VCC: u16 = 106;
const EXEC: u16 = 126;
const TEMPS: u16 = 256;

fn register(operand: &Opnd) -> Option<u16> {
    Some(match *operand {
        Opnd::Sgpr(n) => u16::from(n),
        Opnd::VccLo => VCC,
        Opnd::VccHi => VCC + 1,
        Opnd::ExecLo => EXEC,
        Opnd::ExecHi => EXEC + 1,
        Opnd::Temp(n) => TEMPS.checked_add(n)?,
        _ => return None,
    })
}

#[derive(Clone, PartialEq)]
struct State(BTreeMap<u16, Class>);

impl State {
    fn entry() -> Self {
        State(BTreeMap::from([
            (VCC, Class::Constant),
            (VCC + 1, Class::Constant),
            (EXEC, Class::Mask),
            (EXEC + 1, Class::Mask),
        ]))
    }

    fn get(&self, register: u16) -> Class {
        // A register nothing has written holds what the wave started with.
        self.0.get(&register).copied().unwrap_or(Class::Uniform)
    }

    fn set(&mut self, register: u16, width: u16, class: Class) {
        for step in 0..width {
            self.0.insert(register + step, class);
        }
    }

    /// The class of `width` words of one source operand.
    fn read(&self, operand: &Opnd, width: u16) -> Class {
        match *operand {
            Opnd::ConstInt(0 | -1) | Opnd::Literal(u32::MAX) => Class::Constant,
            _ => match register(operand) {
                Some(first) => (1..width).fold(self.get(first), |class, step| class.join(self.get(first + step))),
                None => Class::Uniform,
            },
        }
    }

    fn join(&mut self, other: &State) -> bool {
        let before = self.clone();
        let keys: Vec<u16> = self.0.keys().chain(other.0.keys()).copied().collect();
        for key in keys {
            let class = self.get(key).join(other.get(key));
            self.0.insert(key, class);
        }
        *self != before
    }
}

fn wide(mnemonic: &str) -> u16 {
    if ["_b64", "_u64", "_i64"].iter().any(|suffix| mnemonic.ends_with(suffix)) { 2 } else { 1 }
}

fn logic(mnemonic: &str) -> bool {
    ["s_and_b", "s_or_b", "s_xor_b", "s_andn2_b", "s_orn2_b", "s_nand_b", "s_nor_b", "s_xnor_b", "s_not_b"]
        .iter()
        .any(|prefix| mnemonic.starts_with(prefix))
}

/// How many of a vector instruction's decoded sources it reads. The long
/// encoding has three source fields whether or not the operation uses them.
fn operands(op: &Insn) -> usize {
    let name = op.mnemonic.as_str();
    if name == "v_nop" {
        return 0;
    }
    const THREE: [&str; 22] = ["mad", "fma", "bfe", "bfi", "alignbit", "alignbyte", "min3", "max3", "med3", "lerp",
        "sad", "cube", "div_fixup", "div_fmas", "div_scale", "xor3", "add3", "or3", "and_or", "lshl_or", "lshl_add",
        "perm"];
    let three = THREE.iter().any(|key| name.contains(key)) || name.starts_with("v_cndmask") || name.contains("_ci_")
        || name.starts_with("v_add_lshl") || name.starts_with("v_xad");
    // The accumulating forms add to their destination.
    let three = three && !name.starts_with("v_mac_") && !name.starts_with("v_fmac_");
    const ONE: [&str; 15] = ["v_mov_", "v_cvt_", "v_rcp_", "v_rsq_", "v_sqrt_", "v_rndne_", "v_floor_", "v_ceil_",
        "v_trunc_", "v_fract_", "v_exp_", "v_log_", "v_sin_", "v_cos_", "v_frexp_"];
    let used = if op.enc != Enc::Vop3 { 3 } else if ONE.iter().any(|prefix| name.starts_with(prefix))
        || matches!(name, "v_not_b32" | "v_bfrev_b32") { 1 } else if three { 3 } else { 2 };
    used.min(op.src.len())
}

/// Apply `op` to `state`. `None` refuses the program.
fn step(op: &Insn, state: &mut State, width: u16) -> Option<Option<Vote>> {
    let name = op.mnemonic.as_str();
    if matches!(name, "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" | "s_rfe_b64" | "s_trap") {
        return None;
    }
    match op.enc {
        Enc::Sopp => {
            let valid = match op.op {
                6 | 7 => state.get(VCC).is_mask(),
                8 | 9 => state.get(EXEC).is_mask(),
                _ => true,
            };
            valid.then_some(None)
        }
        Enc::Sop1 | Enc::Sop2 | Enc::Sopc | Enc::Sopk => {
            if name.starts_with("s_setreg") {
                return Some(None);
            }
            let size = wide(name);
            let destination = op.dst.first().and_then(register);
            let mut inputs = op.src.iter().fold(Class::Constant, |class, source| class.join(state.read(source, size)));
            // SOPK arithmetic and compares read the register they name.
            if op.enc == Enc::Sopk && !name.starts_with("s_movk") && !name.starts_with("s_getreg") {
                inputs = inputs.join(state.read(op.dst.first()?, 1));
            }
            // A literal other than the two masks is an ordinary number.
            if inputs == Class::Conflict {
                return None;
            }
            if name == ray_resolve::READ_LANE || name == COUNT_LANES || name == "tb_first_active_lane"
                || (name.starts_with("s_ff1_i32") && op.src == [Opnd::ExecLo])
            {
                state.set(destination?, 1, Class::Uniform);
                return Some(None);
            }
            if name.starts_with("tb_") {
                return None;
            }
            if name.contains("saveexec") {
                let exec = state.get(EXEC);
                let combined = exec.join(state.read(op.src.first()?, size));
                if !combined.is_mask() {
                    return None;
                }
                state.set(destination?, size, exec);
                state.set(EXEC, size, combined);
                return Some((combined == Class::Mask).then_some(Vote { mask: Opnd::ExecLo, none: false }));
            }
            if op.enc == Enc::Sopc {
                if inputs.is_value() {
                    return Some(None);
                }
                // A whole mask compared with zero asks whether any lane is set.
                let none = if name.starts_with("s_cmp_eq_") { true }
                    else if name.starts_with("s_cmp_lg_") { false }
                    else { return None };
                return match op.src[..] {
                    [mask, Opnd::ConstInt(0)] | [Opnd::ConstInt(0), mask] => Some(Some(Vote { mask, none })),
                    _ => None,
                };
            }
            // The program counter takes no input, whatever the source field says.
            if name == "s_getpc_b64" {
                state.set(op.dst.first().and_then(register)?, 2, Class::Uniform);
                return Some(None);
            }
            if op.dst == [Opnd::Null] {
                return Some(None);
            }
            let destination = destination?;
            if matches!(name, "s_mov_b32" | "s_mov_b64") {
                state.set(destination, size, inputs);
                return Some(None);
            }
            if name.starts_with("s_cselect_b") {
                state.set(destination, size, inputs);
                return Some(None);
            }
            // The emitter votes the condition code of a whole-quad EXEC itself.
            if name.starts_with("s_wqm_b") && op.src == [Opnd::ExecLo] {
                state.set(destination, size, inputs);
                return Some(None);
            }
            if logic(name) {
                state.set(destination, size, inputs);
                return Some((inputs == Class::Mask).then_some(Vote { mask: op.dst[0], none: false }));
            }
            if !inputs.is_value() {
                return None;
            }
            state.set(destination, size, Class::Uniform);
            Some(None)
        }
        Enc::Smem => {
            if !name.contains("load_dword") {
                return None;
            }
            let base = if name.starts_with("s_buffer") { 4 } else { 2 };
            let offset = u16::try_from(op.raw.get(1)? >> 25).ok()?;
            if !op.src.iter().all(|source| state.read(source, base).is_value())
                || (offset < 106 && !state.get(offset).is_value())
            {
                return None;
            }
            let words = [("x16", 16), ("x8", 8), ("x4", 4), ("x2", 2)]
                .iter()
                .find(|(suffix, _)| name.ends_with(suffix))
                .map_or(1, |&(_, words)| words);
            state.set(register(op.dst.first()?)?, words, Class::Uniform);
            Some(None)
        }
        Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc => {
            if !state.get(EXEC).is_mask() {
                return None;
            }
            let lane_read = matches!(name, "v_readlane_b32" | "v_readfirstlane_b32") || name == ray_resolve::READ_LANE;
            let carry = name.contains("_co_") || name.contains("_ci_") || name.starts_with("v_div_scale")
                || name.starts_with("v_mad_u64") || name.starts_with("v_mad_i64");
            let selects = name.starts_with("v_cndmask") || name.contains("_ci_") || name.starts_with("v_div_fmas");
            // The 32-bit encodings read and write VCC without naming it.
            if selects && op.src.len() < 3 && !state.get(VCC).is_mask() {
                return None;
            }
            for (position, source) in op.src.iter().enumerate().take(operands(op)) {
                if register(source).is_none() {
                    continue;
                }
                let class = state.read(source, wide(name));
                if selects && position == 2 { class.is_mask() } else { class.is_value() }.then_some(())?;
            }
            if name.starts_with("v_cmp") {
                for destination in op.dst.iter().filter_map(register) {
                    state.set(destination, width, Class::Mask);
                }
                if name.starts_with("v_cmpx") {
                    state.set(EXEC, width, Class::Mask);
                } else if op.dst.is_empty() {
                    state.set(VCC, width, Class::Mask);
                }
                return Some(None);
            }
            for destination in op.dst.iter().filter_map(register) {
                if !lane_read {
                    return None;
                }
                state.set(destination, 1, Class::Uniform);
            }
            if carry {
                if op.enc == Enc::Vop3 {
                    state.set(u16::try_from((op.raw.first()? >> 8) & 0x7f).ok()?, width, Class::Mask);
                } else {
                    state.set(VCC, width, Class::Mask);
                }
            }
            Some(None)
        }
        _ => {
            // Memory, interpolation and export instructions take descriptors and
            // offsets from scalar registers, and write none.
            // Only sampling reads the sampler an image instruction names.
            let used = if op.enc == Enc::Mimg && !["sample", "gather", "lod"].iter().any(|k| name.contains(k)) {
                2
            } else {
                op.src.len()
            };
            if op.dst.iter().any(|destination| register(destination).is_some())
                || !op.src.iter().take(used).filter(|source| register(source).is_some())
                    .all(|source| state.read(source, 4).is_value())
            {
                return None;
            }
            Some(None)
        }
    }
}

/// The votes that make every branch of the program one decision per wave of
/// `width` lanes, or `None` when that cannot be shown.
pub(super) fn prove(insns: &[Insn], width: u32) -> Option<BTreeMap<usize, Vote>> {
    let words = u16::try_from(width / 32).ok()?;
    let index: BTreeMap<i64, usize> = insns.iter().enumerate().map(|(i, op)| (op.offset as i64, i)).collect();
    let end = insns.last().map_or(0, |op| (op.offset + op.dwords * 4) as i64);
    let mut states: Vec<Option<State>> = vec![None; insns.len()];
    if let Some(first) = states.first_mut() {
        *first = Some(State::entry());
    }
    let mut pending = vec![0usize];
    let mut votes = BTreeMap::new();
    while let Some(at) = pending.pop() {
        let Some(op) = insns.get(at) else { continue };
        let mut state = states[at].clone()?;
        // Classes only move towards `Conflict`, so a later visit may need a vote
        // an earlier one did not, or refuse, but never the reverse.
        if let Some(vote) = step(op, &mut state, words)? {
            votes.insert(at, vote);
        }
        let mut next = Vec::with_capacity(2);
        let branch = op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some();
        if branch {
            let target = op.offset as i64 + 4 + 4 * op.aux.imm;
            if target < end {
                next.push(*index.get(&target)?);
            }
        }
        if !(op.enc == Enc::Sopp && (op.op == 1 || (branch && op.op == 2))) {
            next.push(at + 1);
        }
        for successor in next.into_iter().filter(|&successor| successor < insns.len()) {
            let changed = match &mut states[successor] {
                Some(existing) => existing.join(&state),
                empty => {
                    *empty = Some(state.clone());
                    true
                }
            };
            if changed {
                pending.push(successor);
            }
        }
    }
    Some(votes)
}

/// The wave-wide form of a branch on a lane mask.
pub(super) fn branch_condition(op: u32) -> Option<String> {
    let (register, taken_when_none) = match op {
        6 => ("vcc_lo", true),
        7 => ("vcc_lo", false),
        8 => ("exec_lo", true),
        9 => ("exec_lo", false),
        _ => return None,
    };
    let not = if taken_when_none { "!" } else { "" };
    Some(format!("{not}WaveActiveAnyTrue(({register} & 1u) != 0u)"))
}

#[cfg(test)]
#[path = "../tests/hlsl/lockstep.rs"]
mod tests;
