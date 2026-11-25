//! Identify the native triangle candidate ABI. The callee saves belong to
//! the Xbox scheduler. The body, including its opacity test, runs in DXR.
use super::super::continuation;
use crate::rdna2::{Insn, Opnd, instructions};
use std::{collections::BTreeSet, ops::Range};

fn private_slot(op: &Insn, store: bool) -> Option<(u8, i64)> {
    let [Opnd::Vgpr(v)] = op.dst.as_slice() else {
        return None;
    };
    (op.mnemonic
        == if store {
            "buffer_store_dword"
        } else {
            "buffer_load_dword"
        }
        && continuation::scratch(op)
        && op.aux.mask == 0
        && *v >= 34)
        .then_some((*v, op.aux.imm))
}

fn stack_address(op: &Insn, dst: u8, base: u8) -> bool {
    op.mnemonic == "v_lshl_add_u32"
        && op.dst == [Opnd::Vgpr(dst)]
        && op.src == [Opnd::Vgpr(base), Opnd::ConstInt(2), Opnd::Vgpr(33)]
}

// Removing the lane spill is valid only when its complete save/restore pair
// surrounds the leaf; some leaves also preserve the lane carrier in scratch.
fn candidate_frame(ops: &[Insn]) -> Option<(&[Insn], usize, bool)> {
    let (ret, prefix) = ops.split_last()?;
    if ret.mnemonic != "s_setpc_b64" || ret.src != [Opnd::Sgpr(30)] {
        return None;
    }
    let Some(lane_save) = prefix
        .iter()
        .position(|op| op.mnemonic == "v_writelane_b32")
    else {
        return Some((prefix, ret.offset, false));
    };
    let lane_restore = prefix
        .iter()
        .position(|op| op.mnemonic == "v_readlane_b32")?;
    let [reg] = ops[lane_save].dst.as_slice() else {
        return None;
    };
    if ops[lane_save].src != [Opnd::Sgpr(9), Opnd::ConstInt(0), Opnd::Sgpr(0)]
        || ops[lane_restore].dst != [Opnd::Vgpr(9)]
        || ops[lane_restore].src != [*reg, Opnd::ConstInt(0), Opnd::Sgpr(0)]
    {
        return None;
    }
    let body = prefix.get(lane_save + 1..lane_restore)?;
    if lane_save == 0 && lane_restore + 1 == prefix.len() {
        return Some((body, ops[lane_restore].offset, false));
    }
    let [base, save, first, restore, others @ ..] = ops.get(..lane_save)? else {
        return None;
    };
    if base.mnemonic != "s_mov_b32"
        || base.dst != [Opnd::Sgpr(5)]
        || base.src != [Opnd::Sgpr(32)]
        || save.mnemonic != "s_or_saveexec_b64"
        || save.src != [Opnd::ConstInt(-1)]
        || restore.mnemonic != "s_mov_b64"
        || restore.dst != [Opnd::ExecLo]
        || restore.src != save.dst
        || first.dst.as_slice() != [*reg]
    {
        return None;
    }
    let [loads @ .., save2, last, restore2] = prefix.get(lane_restore + 1..)? else {
        return None;
    };
    // A small number of leaves schedule vertex/address arithmetic before
    // spilling s9. Keep that arithmetic together with both lane operations.
    let interleaved = others.iter().any(|op| !stack_address(op, 8, 9))
        && loads.is_empty()
        && others.iter().all(|op| {
            op.mnemonic.starts_with("v_")
                && !op.mnemonic.contains("lane")
                && !op.mnemonic.contains("store")
        });
    if save2.mnemonic != save.mnemonic
        || save2.src != save.src
        || restore2.mnemonic != restore.mnemonic
        || restore2.dst != restore.dst
        || restore2.src != save2.dst
        || private_slot(first, true)? != private_slot(last, false)?
        || !interleaved
            && others
                .iter()
                .filter(|op| !stack_address(op, 8, 9))
                .map(|op| private_slot(op, true))
                .collect::<Option<BTreeSet<_>>>()?
                != loads
                    .iter()
                    .map(|op| private_slot(op, false))
                    .collect::<Option<BTreeSet<_>>>()?
    {
        return None;
    }

    if interleaved {
        return Some((
            &prefix[4..=lane_restore],
            ops[lane_restore].offset + ops[lane_restore].dwords * 4,
            false,
        ));
    }
    Some((
        body,
        ops[lane_restore].offset,
        others.iter().any(|op| stack_address(op, 8, 9)),
    ))
}

pub(super) fn opacity_body(code: &[u8], words: usize, constants: bool) -> Option<Range<usize>> {
    let ops: Vec<_> = instructions(code)
        .filter(|op| {
            !matches!(
                op.mnemonic.as_str(),
                "s_nop" | "v_nop" | "s_waitcnt" | "s_waitcnt_vscnt" | "s_code_end"
            )
        })
        .collect();
    let ret = ops.iter().position(|ins| ins.mnemonic == "s_setpc_b64")?;
    // Outlined waits followed by an unconditional jump rejoin the main leaf.
    // The continuation emitter redirects branches targeting these islands.
    if ops[ret + 1..].iter().any(|op| op.mnemonic != "s_branch") {
        return None;
    }
    let (body, end, initial_payload) = candidate_frame(&ops[..=ret])?;
    // v26/v27 are the instance and triangle nodes; v31/v32 are the
    // DXR barycentrics. Each payload access must be an immediate dword
    // offset from the v9 payload pointer. v7 indexes the saved return.
    // Some larger leaves keep scalar s9 in private scratch and allocate
    // callee-saved low VGPRs as temporaries once their incoming values have
    // been saved. The continuation emitter executes all those spills/reloads.
    let private_frame = body.first().is_some_and(|op| {
        op.mnemonic == "v_mov_b32" && op.dst == [Opnd::Vgpr(8)] && op.src == [Opnd::Sgpr(9)]
    }) && body
        .iter()
        .take_while(|op| !continuation::scratch(op) && continuation::target(op).is_none())
        .any(|op| {
            op.mnemonic == "s_mov_b32" && op.dst == [Opnd::Sgpr(5)] && op.src == [Opnd::Sgpr(32)]
        })
        && body.last().is_some_and(|op| {
            op.mnemonic == "v_readfirstlane_b32"
                && op.dst == [Opnd::Sgpr(9)]
                && op.src == [Opnd::Vgpr(33)]
        });
    let mut saved = BTreeSet::new();
    let mut initialized = BTreeSet::new();
    let mut result = Vec::new();
    let mut pops = 0;
    for (i, op) in body.iter().enumerate() {
        if private_frame && continuation::scratch(op) && op.aux.mask == 0 {
            let [Opnd::Vgpr(v)] = op.dst.as_slice() else {
                return None;
            };
            if op.mnemonic == "buffer_store_dword"
                && op.aux.imm >= 0
                && op.aux.imm < 4096
                && op.aux.imm % 4 == 0
            {
                saved.insert(op.aux.imm);
            } else if op.mnemonic != "buffer_load_dword" || !saved.contains(&op.aux.imm) {
                return None;
            }
            if op.mnemonic == "buffer_load_dword" {
                initialized.insert(*v);
            }
            continue;
        }
        if (op.mnemonic == "s_getpc_b64" && !constants)
            || !op.is_complete()
            || op.mnemonic.contains("store")
            || matches!(op.mnemonic.as_str(), "s_swappc_b64" | "s_setpc_b64")
        {
            return None;
        }
        if op.src.iter().any(|arg| {
            matches!(arg, Opnd::Vgpr(v) if *v < 34 &&
            !matches!(v, 0|7..=9|16|17|19..=33) && !initialized.contains(v))
        }) {
            return None;
        }
        if private_frame {
            for v in [1, 2, 3, 4, 5, 6, 10, 11, 12, 13, 14, 15, 18] {
                if continuation::writes(op, v) {
                    initialized.insert(v);
                }
            }
        }
        if op.mnemonic == "v_and_b32" && op.src == [Opnd::Literal(0xffff00ff), Opnd::Vgpr(28)] {
            result.push(op);
        } else if continuation::writes(op, 28) {
            result.push(op);
        }
        if continuation::scratch(op) {
            let Some(Opnd::Vgpr(address_reg)) = op.src.first() else {
                return None;
            };
            if op.mnemonic != "buffer_load_dword" || op.aux.mask != 1 {
                return None;
            }
            let mut previous = body[..i].iter().rev();
            let address = previous.find(|ins| {
                continuation::target(ins).is_some() || continuation::writes(ins, *address_reg)
            });
            if address.is_some_and(|ins| stack_address(ins, *address_reg, 7))
                && op.aux.imm == 0
                && op.dst == [Opnd::Vgpr(8)]
            {
                pops += 1;
            } else if !(address.map_or(initial_payload && *address_reg == 8, |ins| {
                stack_address(ins, *address_reg, 9)
            }) && op.aux.imm >= 0
                && op.aux.imm % 4 == 0
                && (op.aux.imm as usize / 4) < words)
            {
                return None;
            }
        }
    }
    let [clear, reject] = result.as_slice() else {
        return None;
    };
    let [Opnd::Vgpr(temp)] = clear.dst.as_slice() else {
        return None;
    };
    if pops != 2
        || clear.mnemonic != "v_and_b32"
        || clear.src != [Opnd::Literal(0xffff00ff), Opnd::Vgpr(28)]
        || reject.mnemonic != "v_or_b32"
        || reject.src != [Opnd::Literal(0x100), Opnd::Vgpr(*temp)]
        || reject.dst != [Opnd::Vgpr(28)]
        || body
            .iter()
            .filter(|op| clear.offset < op.offset && op.offset < reject.offset)
            .any(|op| continuation::target(op).is_some() || continuation::writes(op, *temp))
    {
        return None;
    }
    Some(body.first()?.offset..end)
}
