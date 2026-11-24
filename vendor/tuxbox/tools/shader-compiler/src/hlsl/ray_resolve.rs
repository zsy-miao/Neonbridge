//! These resolve kernels use one native wave per group. Scalar branches and
//! lane reads must see the whole EXEC mask while vector writes remain masked.
use super::{compute_group, Enc, Insn, Opnd, ShaderExtract, Stage};

pub(super) const READ_LANE: &str = "tb_resolve_readlane";

pub(super) fn applies(ex: &ShaderExtract) -> bool {
    ex.stage == Stage::Compute
        && ex.entry.as_deref().is_some_and(|name| name.starts_with("CS_RaytracingResolve_"))
        && compute_group(ex) == [32, 1, 1]
}

pub(super) fn wave32(ex: &ShaderExtract, insns: &[Insn]) -> Result<bool, String> {
    if !applies(ex)
        || !insns.iter().any(|op| {
            op.mnemonic.starts_with("v_cmp_") && matches!(op.dst[..], [Opnd::Sgpr(n)] if n & 1 != 0)
        })
    {
        return Ok(false);
    }
    // DXIL leaves the mapping of thread IDs to lanes unspecified. Guest
    // v0 can be assigned from WaveGetLaneIndex because this is one full wave
    // and the shader does not use group-shared addressing or barriers.
    if let Some(op) = insns.iter().find(|op| {
        (op.enc == Enc::Ds && op.mnemonic != "ds_swizzle_b32")
            || matches!(
                op.mnemonic.as_str(),
                "s_barrier"
                    | "s_setpc_b64"
                    | "s_swappc_b64"
                    | "v_mbcnt_lo_u32_b32"
                    | "v_mbcnt_hi_u32_b32"
                    | "v_readlane_regrd_b32"
                    | "v_writelane_regwr_b32"
            )
            || op.src.iter().chain(&op.dst).any(|r| {
                matches!(r, Opnd::VccHi | Opnd::ExecHi)
                    || (matches!(r, Opnd::VccLo | Opnd::ExecLo)
                        && (op.mnemonic.ends_with("_b64")
                            || op.mnemonic.ends_with("_i64") && op.enc == Enc::Sop1))
            })
    }) {
        return Err(format!(
            "resolve Wave32 cannot lower {} at {:#x}",
            op.mnemonic, op.offset
        ));
    }
    Ok(true)
}

pub(super) fn group_x_register(ex: &ShaderExtract) -> Option<usize> {
    tuxbox_dxbc::compute_table::resolve_constants_register(&ex.fssm).map(|_| 12)
}
