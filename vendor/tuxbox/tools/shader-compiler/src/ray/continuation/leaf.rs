//! Leaf hit programs address the caller's payload directly and return through
//! a separate word. Validate both address calculations before indexing either.
use super::{alu, continuation, layout, resources, scratch, target};
use crate::rdna2::{Enc, Insn, Opnd, instructions};
use std::ops::Range;
use tuxbox_dxbc::ray::State;

pub(in crate::ray) struct Leaf {
    pub declarations: String,
    pub body: String,
}

pub(super) struct Stack {
    pub words: usize,
    pub return_read: usize,
}

pub(in crate::ray) fn writes(ins: &Insn, vgpr: u8) -> bool {
    if ins.mnemonic == "v_movreld_b32"
        || (ins.mnemonic == "v_swap_b32" && ins.src.first() == Some(&Opnd::Vgpr(vgpr)))
    {
        return true;
    }
    if ins.mnemonic.contains("store")
        || matches!(
            ins.mnemonic.as_str(),
            "v_nop" | "v_readlane_b32" | "v_readfirstlane_b32"
        )
    {
        return false;
    }
    let width = if ins.enc == Enc::Mimg {
        ins.aux.mask.count_ones() as u8
    } else if ins.mnemonic.contains("dwordx4") {
        4
    } else if ins.mnemonic.contains("dwordx3") {
        3
    } else if ins.mnemonic.contains("dwordx2") || ins.mnemonic.ends_with("64") {
        2
    } else {
        1
    };
    ins.dst.iter().any(
        |op| matches!(op, Opnd::Vgpr(base) if vgpr.checked_sub(*base).is_some_and(|i| i < width)),
    )
}

#[derive(Clone, Copy, Eq, PartialEq, Hash)]
enum Location {
    Register(u8),
    Private(i64),
}

// Follow all reaching definitions. A large material can spill the two return
// operands inside separate branches and restore them in different VGPRs.
fn reaches_register(
    ops: &[Insn],
    predecessors: &[Vec<usize>],
    before: usize,
    operand: &Opnd,
    original: u8,
    definition: Option<usize>,
) -> bool {
    use std::collections::HashSet;
    let Opnd::Vgpr(reg) = *operand else {
        return false;
    };
    let mut work = vec![(before, Location::Register(reg))];
    let mut seen = HashSet::new();
    let mut reached_origin = false;
    while let Some((pc, value)) = work.pop() {
        if !seen.insert((pc, value)) {
            continue;
        }
        if pc == 0 {
            if definition.is_some() || value != Location::Register(original) {
                return false;
            }
            reached_origin = true;
        }
        for &previous in &predecessors[pc] {
            if definition == Some(previous) && value == Location::Register(original) {
                reached_origin = true;
                continue;
            }
            let ins = &ops[previous];
            let value = match value {
                Location::Register(reg) if writes(ins, reg) => {
                    if ins.dst.first() != Some(&Opnd::Vgpr(reg)) {
                        return false;
                    }
                    if ins.mnemonic == "v_mov_b32" {
                        let [Opnd::Vgpr(source)] = ins.src.as_slice() else {
                            return false;
                        };
                        Location::Register(*source)
                    } else if ins.mnemonic == "buffer_load_dword"
                        && scratch(ins)
                        && ins.aux.mask == 0
                    {
                        Location::Private(ins.aux.imm)
                    } else {
                        return false;
                    }
                }
                Location::Private(slot)
                    if scratch(ins)
                        && ins.aux.mask == 0
                        && ins.aux.imm == slot
                        && ins.mnemonic.contains("store") =>
                {
                    let [Opnd::Vgpr(source)] = ins.dst.as_slice() else {
                        return false;
                    };
                    if ins.mnemonic != "buffer_store_dword" {
                        return false;
                    }
                    Location::Register(*source)
                }
                other => other,
            };
            work.push((previous, value));
        }
    }
    reached_origin
}

// For live VGPRs, a saved EXEC mask partitions the lanes at an if/else.
// Each lane continuing under the restored mask ran exactly one arm. A scalar
// CFG also includes a path skipping both bodies, which reaches the join only
// with an empty EXEC. Recognize straight-line THEN bodies with no other entry.
fn split_lane_branches(ops: &[Insn], predecessors: &mut [Vec<usize>]) {
    for branch in 1..ops.len() {
        let save = &ops[branch - 1];
        if save.mnemonic != "s_and_saveexec_b32" || ops[branch].mnemonic != "s_cbranch_execz" {
            continue;
        }
        let [Opnd::Sgpr(mask)] = save.dst.as_slice() else {
            continue;
        };
        let at =
            |op| target(op).and_then(|pc| ops.binary_search_by_key(&pc, |ins| ins.offset).ok());
        let Some(split) = at(&ops[branch]) else {
            continue;
        };
        if split <= branch + 1
            || split + 2 >= ops.len()
            || ops[split].mnemonic != "s_andn2_b32"
            || ops[split].dst != [Opnd::ExecLo]
            || ops[split].src != [Opnd::Sgpr(*mask), Opnd::ExecLo]
            || ops[split + 1].mnemonic != "s_cbranch_execz"
            || predecessors[split + 1] != [split]
        {
            continue;
        }
        // In an outlined arm the true path may jump out of line. The false
        // edge still enters ELSE when the saved mask was active. Prove that
        // from the fallthrough edge of an enclosing EXECZ guard.
        let mut guard = branch - 1;
        while guard != 0 && predecessors[guard] == [guard - 1] {
            let prev = &ops[guard - 1];
            if prev.mnemonic == "s_cbranch_execz" {
                predecessors[split].retain(|&i| i != branch);
                predecessors[split + 2].push(branch);
                break;
            }
            if target(prev).is_some()
                || prev.dst.contains(&Opnd::ExecLo)
                || prev.mnemonic.contains("saveexec")
            {
                break;
            }
            guard -= 1;
        }
        if !predecessors[split].contains(&branch)
            || predecessors[split] != [branch, split - 1]
            || (branch + 1..split).any(|i| {
                predecessors[i] != [i - 1]
                    || target(&ops[i]).is_some()
                    || ops[i].dst.contains(&Opnd::ExecLo)
                    || ops[i].dst.contains(&Opnd::Sgpr(*mask))
            })
        {
            continue;
        }
        let Some(join) = at(&ops[split + 1]) else {
            continue;
        };
        if join <= split + 2
            || ops[join].mnemonic != "s_mov_b32"
            || ops[join].dst != [Opnd::ExecLo]
            || ops[join].src != [Opnd::Sgpr(*mask)]
            || predecessors[split + 2] != [split + 1]
        {
            continue;
        }
        predecessors[split].retain(|&i| i != branch);
        predecessors[split + 2][0] = branch;
    }
}

impl Stack {
    fn analyze(isa: &[u8], span: &Range<usize>, words: usize) -> Result<Self, String> {
        if !(1..=64).contains(&words) {
            return Err("unsupported ray leaf payload width".into());
        }
        let mut ops: Vec<_> = instructions(&isa[span.clone()]).collect();
        for ins in &mut ops {
            ins.offset += span.start;
        }
        let mut predecessors = vec![Vec::new(); ops.len()];
        for (i, op) in ops.iter().enumerate() {
            if i + 1 < ops.len()
                && !matches!(
                    op.mnemonic.as_str(),
                    "s_branch" | "s_setpc_b64" | "s_endpgm"
                )
            {
                predecessors[i + 1].push(i);
            }
            if let Some(pc) = target(op) {
                let dest = ops
                    .binary_search_by_key(&pc, |op| op.offset)
                    .map_err(|_| "ray leaf branch outside of program")?;
                predecessors[dest].push(i);
            }
        }
        split_lane_branches(&ops, &mut predecessors);
        let prefix = ops
            .iter()
            .position(|i| target(i).is_some())
            .unwrap_or(ops.len());
        let address = |ins: &Insn| {
            ins.mnemonic == "v_lshl_add_u32"
                && matches!(
                    ins.src.as_slice(),
                    [Opnd::Vgpr(_), Opnd::ConstInt(2), Opnd::Vgpr(_)]
                )
        };
        let payload = ops
            .iter()
            .enumerate()
            .position(|(i, op)| {
                address(op)
                    && matches!(op.dst.as_slice(), [Opnd::Vgpr(_)])
                    && reaches_register(&ops, &predecessors, i, &op.src[0], 9, None)
                    && reaches_register(&ops, &predecessors, i, &op.src[2], 26, None)
            })
            .ok_or("missing ray leaf payload address")?;
        let Opnd::Vgpr(base) = ops[payload].dst[0] else {
            unreachable!()
        };
        let return_read = ops
            .iter()
            .rposition(|i| scratch(i) && i.aux.mask == 1)
            .ok_or("missing ray leaf return token")?;
        let read = &ops[return_read];
        let ret = ops
            .iter()
            .position(|i| i.mnemonic == "s_setpc_b64")
            .ok_or("missing ray leaf return")?;
        if read.mnemonic != "buffer_load_dword"
            || read.dst != [Opnd::Vgpr(8)]
            || read.aux.imm != 0
            || return_read >= ret
            || ops[return_read + 1..ret]
                .iter()
                .any(|i| i.src.contains(&Opnd::Vgpr(8)) || target(i).is_some())
        {
            return Err("unsupported ray leaf return token".into());
        }
        let exit = ops[..return_read]
            .iter()
            .rposition(|i| address(i) && i.dst.first() == read.src.first())
            .ok_or("missing ray leaf return address")?;
        let Opnd::Vgpr(return_base) = ops[exit].dst[0] else {
            return Err("invalid ray leaf return base".into());
        };
        let valid_return = reaches_register(&ops, &predecessors, exit, &ops[exit].src[0], 7, None)
            && reaches_register(&ops, &predecessors, exit, &ops[exit].src[2], 26, None)
            && !ops[exit + 1..return_read]
                .iter()
                .any(|op| writes(op, return_base));
        if payload >= prefix
            || payload >= exit
            || !valid_return
            || ops.iter().filter_map(target).any(|pc| {
                pc <= ops[payload].offset || (ops[exit].offset < pc && pc <= ops[ret].offset)
            })
        {
            return Err("unsupported ray leaf payload or return address lifetime".into());
        }
        // The allocator can reuse the original payload VGPR and reload its
        // address into another VGPR for the final stores. Bypass the graph
        // walk for the common case where the same VGPR remains live.
        let stable_base = !ops[payload + 1..exit].iter().any(|op| writes(op, base));
        for (index, ins) in ops[..return_read]
            .iter()
            .enumerate()
            .filter(|(_, i)| scratch(i) && i.aux.mask == 1)
        {
            let word_offset = ins.aux.imm;
            if index <= payload
                || (index >= exit && return_base == base)
                || (!(stable_base && index < exit && ins.src[0] == Opnd::Vgpr(base))
                    && !reaches_register(
                        &ops,
                        &predecessors,
                        index,
                        &ins.src[0],
                        base,
                        Some(payload),
                    ))
                || !matches!(
                    ins.mnemonic.as_str(),
                    "buffer_load_dword" | "buffer_store_dword"
                )
                || !(0..(words * 4) as i64).contains(&word_offset)
                || word_offset % 4 != 0
            {
                return Err(format!(
                    "unsupported ray leaf scratch access at {:#x}",
                    ins.offset
                ));
            }
        }
        Ok(Self {
            words,
            return_read: read.offset,
        })
    }
}

pub(in crate::ray) fn leaf_body(state: &State<'_>, payload_words: usize) -> Result<Leaf, String> {
    let span = layout::leaf_span(state)?;
    let stack = Stack::analyze(state.program, &span, payload_words)?;
    let bindings = resources::Bindings::analyze(state, std::slice::from_ref(&span))?;
    let mut body = String::new();
    continuation(
        state.program,
        span.start,
        span.end,
        &mut body,
        &bindings,
        Some(&stack),
        false,
    )?;
    let mut declarations = bindings.declarations;
    declarations.push_str(alu::HLSL);
    Ok(Leaf { declarations, body })
}

#[cfg(test)]
#[path = "../../tests/ray/continuation/leaf.rs"]
mod tests;
