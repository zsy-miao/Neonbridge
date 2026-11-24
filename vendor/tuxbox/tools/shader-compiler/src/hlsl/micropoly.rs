//! Nanite's software rasterizer shares a 4×4 transform and up to 256
//! transformed vertices across the workgroup. Each 64-lane wave separately
//! chooses a scanline loop when one of its triangles is wider than four pixels.
use super::{compute_group, fssm_bindings, r, Emitter, Enc, Insn, Opnd};

pub(super) fn prepare(em: &mut Emitter<'_>) -> Result<(), String> {
    if compute_group(em.ex) != [128, 1, 1]
        || em.ex.fssm.get(0x10c..0x110) != Some(&3136u32.to_le_bytes())
    {
        return Err("unsupported MicropolyRasterize workgroup or LDS allocation".into());
    }
    for (&(uav, slot), words) in &mut em.sbuf {
        *words = fssm_bindings::structured_words(em.ex, slot, uav)?;
    }
    let mut counts = em
        .insns
        .iter()
        .enumerate()
        .filter(|(_, op)| op.mnemonic == "s_bcnt1_i32_b64");
    let Some((i, vote)) = counts.next() else {
        return Err("missing raster width vote".into());
    };
    if counts.next().is_some()
        || vote.src != [Opnd::VccLo]
        || !matches!(vote.dst.as_slice(), [Opnd::Sgpr(_)])
    {
        return Err("unsupported raster width vote operands".into());
    }
    // Every invocation reaches the vote exactly once. The only guest lanes
    // allowed to skip it have already terminated. Afterwards the independent
    // raster loops may read LDS, but may no longer synchronize the workgroup.
    let end = em
        .insns
        .last()
        .filter(|op| op.mnemonic == "s_endpgm")
        .ok_or("missing rasterizer termination")?
        .offset as i64;
    for (j, op) in em.insns.iter().enumerate() {
        if j > i && op.mnemonic == "s_barrier" {
            return Err("group barrier inside the raster loops".into());
        }
        if op.enc != Enc::Sopp || Emitter::branch_cond(op.op).is_none() {
            continue;
        }
        let target = op.offset as i64 + 4 + 4 * op.aux.imm;
        if (op.aux.imm < 0 && target <= vote.offset as i64)
            || (j < i && target > vote.offset as i64 && target < end)
        {
            return Err("raster control flow crosses the workgroup vote".into());
        }
    }
    em.micropoly_vote = Some(i);
    Ok(())
}

pub(super) fn emit_vote(
    em: &mut Emitter<'_>,
    vote: &Insn,
    skipped: &[usize],
) -> Result<(), String> {
    let guards: Vec<_> = skipped.iter().map(|i| format!("!j{i}")).collect();
    let reached = if guards.is_empty() {
        String::from("true")
    } else {
        guards.join(" && ")
    };
    let dst = vote.dst[0];
    em.touch(&dst);
    em.line("if ((gtid.x & 63u) == 0u) tb_raster_votes[gtid.x >> 6] = 0u;");
    em.line("GroupMemoryBarrierWithGroupSync();");
    em.line(&format!(
        "if (({reached}) && (vcc_lo & 1u)) InterlockedAdd(tb_raster_votes[gtid.x >> 6], 1u);"
    ));
    em.line("GroupMemoryBarrierWithGroupSync();");
    em.line(&format!(
        "if ({reached}) {{ {} = tb_raster_votes[gtid.x >> 6]; scc = ({} != 0u);",
        r(&dst)?,
        r(&dst)?
    ));
    // The nested X/Y backedges carry separate EXEC masks. Keeping their true
    // branch targets preserves both masks across iterations in optimized DXIL.
    let entry = em.micropoly_vote.ok_or("missing raster vote position")? + 1;
    em.emit_body_dispatch(false, entry)?;
    em.line("}");
    Ok(())
}
