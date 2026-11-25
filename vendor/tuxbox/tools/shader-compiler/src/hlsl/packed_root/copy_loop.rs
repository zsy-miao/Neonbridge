//! The compiler rotates the first iteration of a strided copy to skip the mask
//! restore. Verify both paths before reusing the same resource bindings on a backedge.
use super::*;

impl PackedRoot {
    pub(super) fn copy_loop(
        &mut self,
        idx: usize,
        insns: &[Insn],
        args: &[Argument],
        state: &mut State,
        joins: &BTreeMap<usize, State>,
    ) -> Result<Option<usize>, &'static str> {
        let Some(block) = insns.get(idx..idx + 13) else {
            return Ok(None);
        };
        let [entry, restore, nop1, nop2, nop3, load, save, wait, store, advance, compare, retire, back] =
            block
        else {
            return Ok(None);
        };
        if entry.mnemonic != "s_branch" || back.mnemonic != "s_cbranch_scc1" {
            return Ok(None);
        }
        let target = |i: &Insn| i.offset as i64 + 4 + 4 * i.aux.imm;
        let [Opnd::Sgpr(saved), Opnd::Sgpr(live)] = restore.src[..] else {
            return Ok(None);
        };
        let [Opnd::Vgpr(address)] = advance.dst[..] else {
            return Ok(None);
        };
        let [Opnd::Sgpr(stride), Opnd::Vgpr(advance_addr)] = advance.src[..] else {
            return Ok(None);
        };
        let [Opnd::Sgpr(limit), Opnd::Vgpr(compare_addr)] = compare.src[..] else {
            return Ok(None);
        };
        let Some(gate) = insns[..idx]
            .iter()
            .rposition(|i| i.enc == Enc::Sopp && Emitter::branch_cond(i.op).is_some())
        else {
            return Ok(None);
        };
        let setup = &insns[gate + 1..idx];
        let Some(after) = insns.get(idx + block.len()) else {
            return Ok(None);
        };
        let initializes_live = setup.iter().rfind(|i| i.dst.contains(&Opnd::Sgpr(live)));
        if !initializes_live.is_some_and(|i| i.mnemonic == "s_mov_b32" && i.src == [Opnd::ExecLo])
            || setup.iter().any(|i| {
                !matches!(
                    i.mnemonic.as_str(),
                    "s_mov_b32" | "s_lshl_b32" | "s_load_dwordx16" | "s_waitcnt"
                ) || i.dst.contains(&Opnd::ExecLo)
            })
            || insns[gate].mnemonic != "s_cbranch_execz"
            || target(&insns[gate]) != after.offset as i64
            || after.mnemonic != "s_mov_b32"
            || after.dst != [Opnd::ExecLo]
            || !matches!(after.src[..], [Opnd::Sgpr(n)] if n != live && n != saved && !state[n as usize].opaque())
            || !insns[..gate]
                .last()
                .is_some_and(|i| i.mnemonic == "v_cmpx_gt_u32" && i.src == compare.src)
            || [limit, stride].iter().any(|n| *n == saved || *n == live)
            || advance_addr != address
            || compare_addr != address
            || insns[..gate].iter().any(|i| {
                i.enc == Enc::Sopp
                    && Emitter::branch_cond(i.op).is_some()
                    && (insns[gate].offset as i64..=entry.offset as i64).contains(&target(i))
            })
        {
            return Ok(None);
        }
        if target(entry) != nop2.offset as i64
            || target(back) != restore.offset as i64
            || restore.mnemonic != "s_and_b32"
            || restore.dst != [Opnd::ExecLo]
            || saved == live
            || [nop1, nop2, nop3].iter().any(|i| i.mnemonic != "s_nop")
            || load.mnemonic != "buffer_load_dwordx4"
            || store.mnemonic != "buffer_store_dwordx4"
            || load.dst != store.dst
            || [load, store].iter().any(|i| {
                i.src.first() != Some(&Opnd::Vgpr(address))
                    || i.aux.mask != 1
                    || i.aux.imm != 0
                    || i.src.get(2) != Some(&Opnd::ConstInt(0))
            })
            || save.mnemonic != "s_mov_b32"
            || save.dst != [Opnd::Sgpr(saved)]
            || save.src != [Opnd::ExecLo]
            || wait.mnemonic != "s_waitcnt"
            || advance.mnemonic != "v_add_nc_u32"
            || compare.mnemonic != "v_cmpx_le_u32"
            || compare.dst != [Opnd::VccLo]
            || retire.mnemonic != "s_andn2_b32"
            || retire.dst != [Opnd::Sgpr(live)]
            || retire.src != [Opnd::Sgpr(live), Opnd::ExecLo]
        {
            return Ok(None);
        }
        if joins.range(restore.offset..=back.offset).next().is_some() {
            return Err("external branch into strided copy loop");
        }
        let mut iteration = |first: usize, state: &mut State| {
            for (i, ins) in block.iter().enumerate().take(block.len() - 1).skip(first) {
                self.step(idx + i, ins, args, state)?;
            }
            Ok::<(), &'static str>(())
        };
        iteration(3, state)?;
        let first_exit = *state;
        iteration(1, state)?;
        if *state != first_exit {
            return Err("unstable scalar provenance in strided copy loop");
        }
        let end = idx + block.len() - 1;
        // These are physical ballot words on Scarlett. Their individual bits
        // served as per-invocation predicates and cannot be exported as integers.
        state[saved as usize] = Word::Unavailable;
        state[live as usize] = Word::Unavailable;
        self.copy_backedges.insert(end);
        Ok(Some(end))
    }

    // Apply the node-bounds reset only to lane zero when requested by the caller.
    // The straight-line region establishes SCC from plain shader data and keeps
    // EXEC/VCC as per-invocation mask bits all the way to AND_SAVEEXEC.
    pub(super) fn copy_tail_mask(
        &mut self,
        idx: usize,
        insns: &[Insn],
        args: &[Argument],
        state: &mut State,
        joins: &BTreeMap<usize, State>,
    ) -> Result<Option<usize>, &'static str> {
        let Some([cmp, wait, select, drain, union, invert, save]) = insns.get(idx..idx + 7) else {
            return Ok(None);
        };
        let [Opnd::Sgpr(mask)] = select.dst[..] else {
            return Ok(None);
        };
        let reg = Opnd::Sgpr(mask);
        if !matches!(cmp.src[..], [Opnd::Sgpr(_), Opnd::ConstInt(0)])
            || wait.mnemonic != "s_waitcnt"
            || select.mnemonic != "s_cselect_b32"
            || select.src != [Opnd::ExecLo, Opnd::ConstInt(0)]
            || drain.raw.as_slice() != [0xbbfd0000]
            || union.mnemonic != "s_or_b32"
            || union.src != [Opnd::VccLo, reg]
            || union.dst != [reg]
            || invert.mnemonic != "s_xor_b32"
            || invert.src != [reg, Opnd::ConstInt(-1)]
            || invert.dst != [reg]
            || save.mnemonic != "s_and_saveexec_b32"
            || save.src != [reg]
            || save.dst != [reg]
            || joins.range(wait.offset..=save.offset).next().is_some()
        {
            return Ok(None);
        }
        self.step(idx, cmp, args, state)?;
        state[mask as usize] = Word::Unavailable;
        Ok(Some(idx + 6))
    }
}

#[cfg(test)]
#[path = "../../tests/hlsl/packed_root/copy_loop.rs"]
mod tests;
