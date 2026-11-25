//! Quad and row exchanges.
use super::*;

pub(in crate::hlsl) fn is_row_move(i: &Insn) -> bool {
    i.enc == Enc::Vop1 && matches!(i.op, OP_ROW_XOR_MOVE | OP_ROW_SHARE_MOVE | OP_ROW_ROR_MOVE | OP_ROW_SHIFT_MOVE)
}

pub(in crate::hlsl) fn is_quad_exchange(i: &Insn) -> bool {
    (i.enc == Enc::Vop2 && i.op == OP_DPP_BINARY)
        || (i.enc == Enc::Vop1 && i.op == OP_QUAD_MOVE)
        || is_row_move(i)
}

pub(in crate::hlsl) fn lower_dpp_binary(ins: &mut Insn) {
    let control = ins.raw[1];
    let selector = if group_extrema::is_dpp8(ins) { control >> 8 } else { (control >> 8) & 0x1ff };
    ins.op = OP_DPP_BINARY;
    ins.aux.imm = i64::from(selector);
    ins.src[0] = Opnd::Vgpr(control as u8);
}

pub(in crate::hlsl) fn dpp_fp32_sources(ins: &Insn, first: String) -> Result<(String, String), String> {
    // DPP8 encodes lane selectors in the same bits, and ordinary VOP2
    // instructions can have a literal extension instead of a DPP control.
    let flags = if ins.raw.first().is_some_and(|word| word & 0x1ff == 250) {
        *ins.raw.get(1).ok_or("missing DPP16 controls")? >> 20
    } else {
        0
    };
    let modified = |bits, flags: u32| {
        compare::sign_modifiers(bits, 0x8000_0000, flags & 2 != 0, flags & 1 != 0)
    };
    Ok((modified(first, flags), modified(r(&ins.src[1])?, flags >> 2)))
}

pub(in crate::hlsl) fn is_binary_quad_perm(i: &Insn) -> bool {
    if i.enc != Enc::Vop2 || i.src.first() != Some(&Opnd::Raw(250)) {
        return false;
    }
    let Some(control) = i.raw.get(1) else { return false };
    // A quad selector uses eight bits. Row and bank masks govern the write.
    if control & 0x0003_0000 != 0 {
        return false;
    }
    match i.mnemonic.as_str() {
        "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32"
        | "v_cvt_pkrtz_f16_f32" | "v_min_f32" | "v_max_f32" => true,
        "v_or_b32" | "v_and_b32" | "v_xor_b32" | "v_xnor_b32"
        | "v_lshlrev_b32" | "v_lshrrev_b32" | "v_ashrrev_i32"
        | "v_add_nc_u32" | "v_sub_nc_u32" | "v_subrev_nc_u32"
        | "v_min_i32" | "v_max_i32" | "v_min_u32" | "v_max_u32"
        | "v_mul_i32_i24" | "v_mul_hi_i32_i24" | "v_mul_u32_u24" | "v_mul_hi_u32_u24"
        | "v_cndmask_b32" => control & 0x00f0_0000 == 0, // No FP source modifiers.
        _ => false,
    }
}

// VOP2 CNDMASK reads the destination lane's VCC. A vector compare writes a
// per-invocation predicate here, while a scalar write can hold a packed mask.
pub(in crate::hlsl) fn quad_cndmasks_with_lane_vcc(insns: &[Insn]) -> BTreeSet<usize> {
    let targets = branch_targets(insns);
    let mut selects = BTreeSet::new();
    let mut lane_vcc = false;
    for (index, ins) in insns.iter().enumerate() {
        if targets.contains(&(ins.offset as i64)) { lane_vcc = false; }
        if lane_vcc && ins.mnemonic == "v_cndmask_b32" && is_binary_quad_perm(ins) {
            selects.insert(index);
        }
        if ins.mnemonic.starts_with("v_cmpx_") { continue; } // RDNA2 CMPX writes EXEC.
        if ins.dst.first() == Some(&Opnd::VccLo) {
            lane_vcc = ins.mnemonic.starts_with("v_cmp_")
                || (ins.mnemonic == "s_mov_b64" && matches!(ins.src[..], [Opnd::ConstInt(0 | -1)]));
        } else {
            lane_vcc &= match ins.enc {
                Enc::Vop1 | Enc::Vop2 => matches!(ins.dst[..], [Opnd::Vgpr(_)])
                    && !ins.mnemonic.contains("_co_"),
                Enc::Sop1 => matches!(ins.mnemonic.as_str(), "s_mov_b32" | "s_mov_b64" | "s_wqm_b32" | "s_wqm_b64")
                    && !ins.dst.contains(&Opnd::VccHi),
                Enc::Sopp => matches!(ins.mnemonic.as_str(), "s_nop" | "s_waitcnt"),
                _ => false,
            };
        }
    }
    selects
}
