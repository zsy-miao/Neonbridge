//! Recognised wave reductions.
use super::*;

/// Whole-wave reductions (HLSL `WaveActiveMin` / `WaveActiveMax`; observed in
/// CocFlattenMainCS, 4f9670de0be37288). The compiled form is:
///
/// ```text
///   s_nand_saveexec_b64 sX, 0        ; save exec, enable every lane
///   v_mov_b32     vI, <identity>
///   v_cndmask_b32 vR, vI, vIn, sX    ; inactive lanes contribute the identity
///   v_min_f32     vR, <dpp>, vR      ; DPP row/quad permutes (src0 = 233 / 250)
///   ...  v_permlanex16_b32, v_min_f32, v_readlane_b32 (lane 32), v_min_f32 ...
///   v_readlane_b32 sRes, vR, 0       ; the result, a scalar
///   s_mov_b64     exec_lo, sX        ; restore exec
/// ```
///
/// A per-lane translation cannot express the cross-lane data movement, but the net effect
/// is exactly a wave reduction, so the region is replaced by: `sX = exec` (exec itself is
/// left alone), the identity move, and one synthetic instruction
/// `sRes = WaveActive{Min,Max}(lane active ? vIn : identity)`. Needs shader model 6 (DXC).
///
/// Inferred, not documented: operand codes 233/250 as DPP with a trailing control dword and
/// SOP1 0x29 as nand-saveexec are taken from the public GFX10 encoding and are consistent
/// with this shader's instruction stream.
/// Anything in the region that does not fit this shape is refused.
pub(in crate::hlsl) fn lower_wave_reductions(insns: &mut [Insn]) -> Result<Option<u32>, String> {
    for op in insns.iter_mut().filter(|op| op.enc == Enc::Vop1 && op.mnemonic == "v_mov_b32" && op.src == [Opnd::Raw(250)]) {
        let ctrl = *op.raw.get(1).ok_or("missing DPP16 move controls")?;
        let src0_reg = ctrl & 0xff;
        let ctrl_field = i64::from((ctrl >> 8) & 0x1ff);
        let neg0 = (ctrl >> 20) & 1;
        let abs0 = (ctrl >> 21) & 1;
        let unsupported = || format!("unsupported DPP16 move at {:#x}", op.offset);
        if neg0 != 0 || abs0 != 0 {
            return Err(unsupported());
        }
        let (opcode, selector) = match ctrl_field {
            0..=0xff => (OP_QUAD_MOVE, ctrl_field),
            0x101..=0x10f => (OP_ROW_SHIFT_MOVE, ctrl_field - 0x100), // row_shl
            0x111..=0x11f => (OP_ROW_SHIFT_MOVE, 0x110 - ctrl_field), // row_shr
            0x121..=0x12f => (OP_ROW_ROR_MOVE, ctrl_field - 0x120),
            0x140 => (OP_ROW_XOR_MOVE, 15), // row_mirror
            0x141 => (OP_ROW_XOR_MOVE, 7),  // row_half_mirror
            0x150..=0x15f => (OP_ROW_SHARE_MOVE, ctrl_field - 0x150), // row_share
            0x160..=0x16f => (OP_ROW_XOR_MOVE, ctrl_field - 0x160), // row_xmask
            _ => return Err(unsupported()),
        };
        op.op = opcode;
        op.src[0] = Opnd::Vgpr(src0_reg as u8);
        op.aux.imm = selector;
    }
    let mut width = None;
    let mut proven_cndmasks = None;
    let is_dpp = |i: &Insn| i.enc == Enc::Vop2 && matches!(i.src.first(), Some(Opnd::Raw(233 | 234 | 250)));
    let mut at = 0;
    while let Some(first) = (at..insns.len()).find(|&i| is_dpp(&insns[i])) {
        let off = insns[first].offset;
        let bad = |what: &str| format!("wave reduction at {off:#x}: {what}");
        let red = insns[first].mnemonic.clone();
        // A full saved-EXEC extrema reduction can begin with quad shuffles,
        // then combine different quads. A quad followed solely by a lane read
        // still selects within its quad and must not become a whole-wave min/max.
        let starts_wave_extrema = matches!(red.as_str(), "v_min_f32" | "v_max_f32")
            && first >= 3 && (insns[first - 3].enc, insns[first - 3].op) == (Enc::Sop1, 0x29)
            && insns[first - 2].mnemonic == "v_mov_b32" && insns[first - 1].mnemonic == "v_cndmask_b32"
            && insns[first + 1..].iter()
                .take_while(|i| is_dpp(i) && i.mnemonic == red && i.dst == insns[first].dst)
                .any(|i| !is_binary_quad_perm(i));
        if is_binary_quad_perm(&insns[first]) && !starts_wave_extrema {
            if red == "v_cndmask_b32" && !proven_cndmasks
                .get_or_insert_with(|| quad_cndmasks_with_lane_vcc(insns)).contains(&first) {
                return Err(bad("quad CNDMASK requires a known per-lane VCC predicate"));
            }
            lower_dpp_binary(&mut insns[first]);
            at = first + 1;
            continue;
        }
        if red == "v_add_f32" && insns[first].src.first() == Some(&Opnd::Raw(250)) {
            return Err(bad("unsupported DPP16 FP32 add controls"));
        }
        let saved_extrema = matches!(red.as_str(), "v_min_f32" | "v_max_f32")
            && (first.checked_sub(3).is_some_and(|p| matches!(insns[p].mnemonic.as_str(), "s_or_saveexec_b32" | "s_or_saveexec_b64"))
                || wave_sum::masked_floating(insns, first));
        if matches!(red.as_str(), "v_add_nc_u32" | "v_min_u32" | "v_max_u32" | "v_min_i32" | "v_max_i32" | "v_add_f32" | "v_and_b32" | "v_or_b32" | "v_xor_b32") || saved_extrema {
            let (required, next) = wave_sum::lower(insns, first).map_err(bad)?;
            if width.replace(required).is_some_and(|w| w != required) {
                return Err(bad("conflicting integer sum wave widths"));
            }
            at = next;
            continue;
        }
        if !matches!(red.as_str(), "v_min_f32" | "v_max_f32") {
            return Err(bad(&format!("unsupported reduction op {red}")));
        }
        let Some(Opnd::Vgpr(vr)) = insns[first].dst.first().copied() else { return Err(bad("no accumulator")) };
        // Prologue: nand_saveexec, identity move, cndmask.
        if first < 3 {
            return Err(bad("no prologue"));
        }
        let (a, mv, cm) = (first - 3, first - 2, first - 1);
        let saved = match (&insns[a].enc, insns[a].op, insns[a].dst.first(), insns[a].src.first()) {
            (Enc::Sop1, 0x29, Some(Opnd::Sgpr(x)), Some(Opnd::ConstInt(0))) => *x,
            _ => return Err(bad("prologue does not start with s_nand_saveexec_b64 sX, 0")),
        };
        let (ident_reg, ident) = match (insns[mv].mnemonic.as_str(), insns[mv].dst.first(), insns[mv].src.first()) {
            ("v_mov_b32", Some(Opnd::Vgpr(v)), Some(Opnd::Literal(k))) => (*v, *k),
            _ => return Err(bad("identity is not a literal move")),
        };
        let c = &insns[cm];
        let vin = match (c.mnemonic.as_str(), c.dst.first(), c.src.first(), c.src.get(1), c.src.get(2)) {
            ("v_cndmask_b32", Some(Opnd::Vgpr(d)), Some(Opnd::Vgpr(i)), Some(Opnd::Vgpr(x)), Some(Opnd::Sgpr(m)))
                if *d == vr && *i == ident_reg && *m == saved =>
            {
                *x
            }
            _ => return Err(bad("unexpected select before the reduction")),
        };
        // Body up to the exec restore.
        let mut end = None;
        let mut last_read = None;
        for i in first..insns.len() {
            let x = &insns[i];
            if x.mnemonic == "s_mov_b64" && x.dst.first() == Some(&Opnd::ExecLo) && x.src.first() == Some(&Opnd::Sgpr(saved)) {
                end = Some(i);
                break;
            }
            let ok = (x.enc == Enc::Vop2 && x.mnemonic == red && x.dst.first() == Some(&Opnd::Vgpr(vr)))
                || (x.enc == Enc::Vop3 && x.op == 0x378 && x.src.first() == Some(&Opnd::Vgpr(vr)))
                || (x.enc == Enc::Vop3 && x.op == 0x360 && x.src.first() == Some(&Opnd::Vgpr(vr)));
            if !ok {
                return Err(bad(&format!("unexpected {} at {:#x} inside the reduction", x.mnemonic, x.offset)));
            }
            if x.enc == Enc::Vop3 && x.op == 0x360 {
                last_read = Some(i);
            }
        }
        let (Some(end), Some(last)) = (end, last_read) else { return Err(bad("no exec restore / result read")) };
        if insns[last].src.get(1) != Some(&Opnd::ConstInt(0)) {
            return Err(bad("result is not read from lane 0"));
        }
        let res = match insns[last].dst.first() {
            Some(Opnd::Vgpr(n)) | Some(Opnd::Sgpr(n)) => *n,
            _ => return Err(bad("bad result register")),
        };
        // Rewrite.
        insns[a].mnemonic = "s_mov_b64".into();
        insns[a].op = 4;
        insns[a].src = vec![Opnd::ExecLo];
        for i in cm..end {
            let x = &mut insns[i];
            x.enc = Enc::Vop1;
            x.op = 0;
            x.mnemonic = "v_nop".into();
            x.dst = vec![Opnd::Vgpr(vr)];
            x.src = vec![];
            x.aux = Default::default();
        }
        let x = &mut insns[last];
        x.enc = Enc::Vop3;
        x.op = OP_WAVE_REDUCE;
        x.mnemonic = if red == "v_min_f32" { "tb_wave_min_f32".into() } else { "tb_wave_max_f32".into() };
        x.dst = vec![Opnd::Sgpr(res)];
        x.src = vec![Opnd::Vgpr(vin), Opnd::Sgpr(saved)];
        x.aux.imm = ident as i64;
        at = end + 1;
    }
    Ok(width)
}
