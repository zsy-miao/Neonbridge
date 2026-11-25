//! FidelityFX tensor passes form a readiness word by ORing four DPP16
//! permutations within a row, then exchanging rows. Some passes run one
//! wave64 per group. Others run several independent 32-lane waves.
use super::{
    packed_root::{convolution_layout, RootAbi},
    r, Emitter, Insn, Opnd, ShaderExtract,
};

pub(super) fn width(ex: &ShaderExtract, insns: &[Insn]) -> Option<u32> {
    if ex.fspx_config.is_none()
        || !insns
            .iter()
            .any(|i| i.mnemonic == "v_or_b32" && i.src.first() == Some(&Opnd::Raw(250)))
    {
        return None;
    }
    match convolution_layout(ex)? {
        RootAbi::RegisterWave64 => Some(64),
        RootAbi::RegisterWave32 => Some(32),
        _ => None,
    }
}

// Native wave32 makes per-wave branches independent without placing a
// workgroup barrier inside a branch that another wave could skip.
pub(super) const WAVE32: &str = r#"
uint2 tb_page_ballot(uint tid, bool vote) { return uint2(WaveActiveBallot(vote).x, 0u); }
uint tb_page_read(uint tid, uint value, uint lane) { return WaveReadLaneAt(value, lane & 31u); }
"#;

pub(super) fn emit(em: &mut Emitter<'_>, idx: usize, i: &Insn, width: u32) -> Result<bool, String> {
    match i.mnemonic.as_str() {
        // SCC tests whether the entire mask is nonempty. The poll and its
        // outer loop must agree across both 32-lane hardware subgroups.
        "s_and_b64" if width == 64 && i.src.get(1) == Some(&Opnd::ExecLo) => {
            let dst = &i.dst[0];
            em.ws(dst, &format!("({} & exec_lo & 1u)", r(&i.src[0])?))?;
            em.ws(&super::hi(dst), "0u")?;
            em.line(&format!(
                "scc = any(tb_page_ballot(tb_wave_tid, {} != 0u));",
                r(dst)?
            ));
        }
        "s_andn2_wrexec_b64" if width == 64 => {
            let dst = &i.dst[0];
            em.ws(dst, &format!("({} & ~exec_lo & 1u)", r(&i.src[0])?))?;
            em.ws(&super::hi(dst), "0u")?;
            em.line(&format!("exec_lo = {}; exec_hi = 0u; scc = any(tb_page_ballot(tb_wave_tid, exec_lo != 0u));", r(dst)?));
        }
        "v_or_b32" if i.src.first() == Some(&Opnd::Raw(250)) => {
            let word = *i.raw.get(1).ok_or("missing DPP16 control")?;
            // All banks and rows enabled, FI=0, no input modifiers. All lanes
            // are enabled by SAVEEXEC before these four operations.
            let xor = match word >> 8 {
                0xff04b1 => 1,  // quad_perm [1, 0, 3, 2]
                0xff044e => 2,  // quad_perm [2, 3, 0, 1]
                0xff0541 => 7,  // row_half_mirror
                0xff0540 => 15, // row_mirror
                _ => return Err("unsupported FidelityFX DPP16 OR permutation".into()),
            };
            let src = r(&Opnd::Vgpr(word as u8))?;
            let other = r(&i.src[1])?;
            em.line(&format!(
                "uint partner{idx} = tb_page_read(tb_wave_tid, {src}, tb_wave_tid ^ {xor}u);"
            ));
            em.wv(&i.dst[0], &format!("partner{idx} | {other}"))?;
        }
        "v_writelane_b32" => {
            // WRITELANE ignores EXEC, including when the named lane is inactive.
            let src = r(&i.src[0])?;
            let lane = r(&i.src[1])?;
            let dst = r(&i.dst[0])?;
            em.line(&format!(
                "if ((tb_wave_tid & {mask}u) == ({lane} & {mask}u)) {dst} = {src};",
                mask = width - 1
            ));
        }
        "s_mov_b32" if width == 32 && i.dst == [Opnd::ExecLo] => {
            // A literal is a physical bit mask. Saved EXEC and VCC registers
            // in these passes already hold this invocation's individual bit.
            let value = r(&i.src[0])?;
            let value = match i.src[0] {
                Opnd::ConstInt(_) | Opnd::Literal(_) => {
                    format!("({value} >> (tb_wave_tid & 31u)) & 1u")
                }
                Opnd::Sgpr(_) | Opnd::VccLo => format!("{value} & 1u"),
                _ => return Err("unexpected source of FidelityFX EXEC mask".into()),
            };
            em.ws(&Opnd::ExecLo, &value)?;
        }
        "v_mbcnt_lo_u32_b32" if width == 32 && i.src[0] == Opnd::ConstInt(-1) => {
            em.wv(
                &i.dst[0],
                &format!("(tb_wave_tid & 31u) + {}", r(&i.src[1])?),
            )?;
        }
        _ => return Ok(false),
    }
    Ok(true)
}
