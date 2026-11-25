//! Forward SGPR analysis shared by static table offsets and read-lane selectors.
use super::{Emitter, Enc, Insn, Opnd};
use crate::rdna2::clobbers_scalar;
use std::collections::{BTreeMap, VecDeque};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Proof {
    Exact,
    UpperBound,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Value {
    Exact(u32),
    AtMost(u32),
}

impl Value {
    fn upper(self) -> u32 {
        match self {
            Self::Exact(n) | Self::AtMost(n) => n,
        }
    }

    fn exact(self) -> Option<u32> {
        match self {
            Self::Exact(n) => Some(n),
            Self::AtMost(_) => None,
        }
    }

    fn join(self, other: Self, proof: Proof) -> Option<Self> {
        if self == other {
            Some(self)
        } else if proof == Proof::UpperBound {
            Some(Self::AtMost(self.upper().max(other.upper())))
        } else {
            None
        }
    }
}

pub(super) fn at_reads(
    insns: &[Insn],
    proof: Proof,
    register_at: impl Fn(&Insn) -> Option<u8>,
) -> BTreeMap<(usize, u8), u32> {
    if insns.is_empty()
        || insns.len() > 32768
        || !insns.iter().any(|i| register_at(i).is_some())
        || insns.iter().any(|i| {
            !i.is_complete()
                || i.enc == Enc::Unknown
                || i.mnemonic.contains('<')
                || i.mnemonic.starts_with("s_movreld_")
                || i.mnemonic.starts_with("s_subvector_loop_")
                || (i.mnemonic.starts_with("s_cbranch_") && Emitter::branch_cond(i.op).is_none())
                || matches!(
                    i.mnemonic.as_str(),
                    "s_setpc_b64"
                        | "s_swappc_b64"
                        | "s_rfe_b64"
                        | "s_call_b64"
                        | "s_movrelsd_2_b32"
                        | "s_trap"
                )
        })
    {
        return BTreeMap::new();
    }

    type State = BTreeMap<u8, Value>;
    let mut input: Vec<Option<State>> = vec![None; insns.len()];
    input[0] = Some(State::new());
    let mut work = VecDeque::from([0]);
    let mut iterations = 0usize;
    while let Some(index) = work.pop_front() {
        iterations += 1;
        if iterations > insns.len() * 128 {
            return BTreeMap::new();
        }
        let ins = &insns[index];
        let incoming = input[index].as_ref().expect("reachable");
        let mut out = incoming.clone();
        out.retain(|&reg, _| !clobbers_scalar(ins, reg));
        let fact = |op: &Opnd| match op {
            Opnd::Sgpr(r @ 0..=105) => incoming.get(r).copied(),
            Opnd::ConstInt(v) => Some(Value::Exact(*v as u32)),
            Opnd::Literal(v) => Some(Value::Exact(*v)),
            _ => None,
        };
        let value = |op: &Opnd| fact(op).and_then(Value::exact);
        if let [Opnd::Sgpr(dst @ 0..=105)] = ins.dst.as_slice() {
            let known = match (ins.mnemonic.as_str(), ins.src.as_slice()) {
                ("s_mov_b32", [a]) => value(a),
                ("s_add_u32" | "s_add_i32", [a, b]) => {
                    value(a).zip(value(b)).map(|(a, b)| a.wrapping_add(b))
                }
                ("s_sub_u32" | "s_sub_i32", [a, b]) => {
                    value(a).zip(value(b)).map(|(a, b)| a.wrapping_sub(b))
                }
                ("s_and_b32", [a, b]) => value(a).zip(value(b)).map(|(a, b)| a & b),
                ("s_or_b32", [a, b]) => value(a).zip(value(b)).map(|(a, b)| a | b),
                ("s_xor_b32", [a, b]) => value(a).zip(value(b)).map(|(a, b)| a ^ b),
                ("s_lshl_b32", [a, b]) => value(a).zip(value(b)).map(|(a, b)| a << (b & 31)),
                ("s_lshr_b32", [a, b]) => value(a).zip(value(b)).map(|(a, b)| a >> (b & 31)),
                _ => None,
            };
            let result = known.map(Value::Exact).or_else(|| {
                if proof == Proof::Exact {
                    return None;
                }
                match (ins.mnemonic.as_str(), ins.src.as_slice()) {
                    ("s_mov_b32", [a]) => fact(a),
                    ("s_cmov_b32", [a]) => incoming.get(dst)?.join(fact(a)?, proof),
                    ("s_cselect_b32", [a, b]) => fact(a)?.join(fact(b)?, proof),
                    // AND can only clear bits, even if one input is unknown.
                    ("s_and_b32", [a, b]) => [fact(a), fact(b)]
                        .into_iter()
                        .flatten()
                        .map(Value::upper)
                        .min()
                        .map(Value::AtMost),
                    _ => None,
                }
            });
            if let Some(v) = result {
                out.insert(*dst, v);
            }
            if proof == Proof::UpperBound && *dst <= 104 && *dst % 2 == 0 {
                let operands = match (ins.mnemonic.as_str(), ins.src.as_slice()) {
                    ("s_mov_b64", [a]) => Some((a, None)),
                    ("s_cmov_b64", [a]) => Some((a, Some(&ins.dst[0]))),
                    ("s_cselect_b64", [a, b]) => Some((a, Some(b))),
                    _ => None,
                };
                if let Some((source, alternate)) = operands {
                    // Both words read the incoming state even when a source aliases the destination.
                    for offset in 0..2 {
                        let word = |op: &Opnd| match op {
                            Opnd::Sgpr(r @ 0..=104) if r % 2 == 0 => {
                                incoming.get(&(r + offset)).copied()
                            }
                            Opnd::ConstInt(_) if offset == 0 => fact(op),
                            Opnd::ConstInt(_) => fact(&super::hi(op)),
                            _ => None,
                        };
                        let value = word(source).and_then(|a| match alternate {
                            Some(b) => a.join(word(b)?, proof),
                            None => Some(a),
                        });
                        if let Some(v) = value {
                            out.insert(dst + offset, v);
                        }
                    }
                }
            }
        }
        let mut merge = |next: usize| {
            let Some(slot) = input.get_mut(next) else {
                return;
            };
            let mut joined = slot.clone().unwrap_or_else(|| out.clone());
            joined.retain(|r, v| {
                if let Some(merged) = out.get(r).and_then(|&other| v.join(other, proof)) {
                    *v = merged;
                    true
                } else {
                    false
                }
            });
            if slot.as_ref() != Some(&joined) {
                *slot = Some(joined);
                work.push_back(next);
            }
        };
        if ins.enc == Enc::Sopp && Emitter::branch_cond(ins.op).is_some() {
            let target = ins.offset as i64 + 4 + 4 * ins.aux.imm;
            let Ok(next) = insns.binary_search_by_key(&target, |i| i.offset as i64) else {
                return BTreeMap::new();
            };
            merge(next);
            if ins.op == 2 {
                continue;
            }
        }
        if ins.mnemonic != "s_endpgm" {
            merge(index + 1);
        }
    }
    let mut result = BTreeMap::new();
    for (ins, state) in insns.iter().zip(input) {
        if let Some(reg) = register_at(ins) {
            if let Some(v) = state.and_then(|s| s.get(&reg).copied()) {
                result.insert((ins.offset, reg), v.upper());
            }
        }
    }
    result
}
