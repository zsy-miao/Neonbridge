//! Cross-lane and local-data-share operations for one logical RDNA2 wave64.
//! The dispatcher keeps all 64 invocations at the same scalar PC. A gather
//! must execute before masking the destination, even for lanes outside EXEC.
use super::{r, Emitter};
use crate::{
    rdna2::{Enc, Insn, Opnd},
    ShaderExtract,
};
use Opnd::{ConstInt, Raw, Sgpr, Vgpr};

pub(super) fn declarations(ex: &ShaderExtract) -> Result<String, String> {
    // FSSM declares the guest LDS allocation in bytes. This is 92 or 124
    // bytes across the observed persistent kernels; the staging space for
    // exchanges between native subgroups is declared independently.
    let bytes = ex
        .fssm
        .get(0x10c..0x110)
        .ok_or("missing compute LDS allocation")?;
    let size = u32::from_le_bytes(bytes.try_into().unwrap());
    if !matches!(size, 92 | 124) {
        return Err(format!("unverified persistent LDS allocation: {size}"));
    }
    Ok(format!(
        "#define TB_LDS_WORDS {}u\ngroupshared uint tb_lds[TB_LDS_WORDS];\n",
        size / 4
    ))
}

fn local_memory(em: &mut Emitter<'_>, ins: &Insn) -> Result<(), String> {
    let m = ins.mnemonic.as_str();
    let ([Vgpr(dest)], [addr @ Vgpr(_), data @ Vgpr(_), data2 @ Vgpr(_)]) =
        (&ins.dst[..], &ins.src[..])
    else {
        return Err("invalid persistent LDS operands".into());
    };
    if ins.raw.len() != 2 || ins.raw[0] & (1 << 17) != 0 {
        return Err("persistent kernel requires group-local DS operations".into());
    }
    for op in &ins.src {
        em.touch(op);
    }
    let address = r(addr)?;
    let offset = ins.aux.imm as u32;
    let paired = matches!(m, "ds_read2_b32" | "ds_write2_b32");
    // Snapshot the address before any destinations (which can alias it) are written.
    em.line(&format!("{{ uint base = {address};"));
    for word in 0..if paired { 2 } else { 1 } {
        let byte = if paired {
            ((offset >> (word * 8)) & 255) * 4
        } else {
            offset
        };
        em.line(&format!("{{ uint at = (base + {byte}u) >> 2;"));
        let value = r(if word == 0 { data } else { data2 })?;
        match m {
            "ds_read_b32" | "ds_read2_b32" => {
                let dst = Vgpr(dest.checked_add(word).ok_or("LDS read register overflow")?);
                em.touch(&dst);
                em.line(&format!(
                    "if (tb_active) {} = at < TB_LDS_WORDS ? tb_lds[at] : 0u;",
                    r(&dst)?
                ));
            }
            "ds_write_b32" | "ds_write2_b32" => em.line(&format!(
                "if (tb_active && at < TB_LDS_WORDS) tb_lds[at] = {value};"
            )),
            "ds_or_b32" => em.line(&format!(
                "if (tb_active && at < TB_LDS_WORDS) InterlockedOr(tb_lds[at], {value});"
            )),
            "ds_add_rtn_u32" => {
                em.touch(&Vgpr(*dest));
                em.line(&format!("if (tb_active) {{ uint old = 0u; if (at < TB_LDS_WORDS) InterlockedAdd(tb_lds[at], {value}, old); v{dest} = old; }}"));
            }
            _ => return Err(format!("unsupported persistent LDS operation {m}")),
        }
        em.line("}");
    }
    em.line("} GroupMemoryBarrierWithGroupSync();");
    Ok(())
}

fn dpp_add(em: &mut Emitter<'_>, ins: &Insn) -> Result<(), String> {
    let ([Vgpr(dst)], [Raw(selector), rhs @ Vgpr(_)], [_, ctrl]) =
        (&ins.dst[..], &ins.src[..], &ins.raw[..])
    else {
        return Err("invalid persistent DPP operands".into());
    };
    let src = Vgpr(*ctrl as u8);
    em.touch(&src);
    em.touch(rhs);
    em.touch(&Vgpr(*dst));
    let (lane, valid, zero_invalid) = match selector {
        // All 8 relative lane selectors are packed into the 24-bit control.
        233 => (
            format!(
                "(tb_lane & ~7u) | ((0x{:06x}u >> (3u*(tb_lane & 7u))) & 7u)",
                ctrl >> 8
            ),
            String::from("true"),
            true,
        ),
        250 => {
            // All banks/rows enabled, FI=0, no absolute or negate modifiers.
            if ctrl & 0xfff6_0000 != 0xff00_0000 {
                return Err("unsupported DPP16 modifier".into());
            }
            let (lane, valid) = match (ctrl >> 8) & 0x1ff {
                0x140 => (String::from("tb_lane ^ 15u"), String::from("true")),
                op @ 0x111..=0x11f => {
                    let shift = op & 15;
                    (
                        format!("(tb_lane - {shift}u) & 63u"),
                        format!("(tb_lane & 15u) >= {shift}u"),
                    )
                }
                op => return Err(format!("unsupported DPP16 control {op:#x}")),
            };
            (lane, valid, ctrl & (1 << 19) != 0)
        }
        _ => return Err("unsupported DPP lane selector".into()),
    };
    em.line(&format!(
        "{{ uint peer = {lane}; uint shuffled = tb_read({}, peer);",
        r(&src)?
    ));
    em.line(&format!("bool available = ({valid}) && (((peer < 32u ? exec_lo : exec_hi) >> (peer & 31u)) & 1u) != 0u;"));
    let rhs = r(rhs)?;
    if zero_invalid {
        em.line(&format!(
            "if (tb_active) v{dst} = {rhs} + (available ? shuffled : 0u); }}"
        ));
    } else {
        em.line(&format!(
            "if (tb_active && available) v{dst} = {rhs} + shuffled; }}"
        ));
    }
    Ok(())
}

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) -> Result<bool, String> {
    if ins.enc == Enc::Ds {
        local_memory(em, ins)?;
        return Ok(true);
    }
    if ins.mnemonic == "v_add_nc_u32" && matches!(ins.src.first(), Some(Raw(233 | 250))) {
        dpp_add(em, ins)?;
        return Ok(true);
    }
    match ins.mnemonic.as_str() {
        "v_readfirstlane_b32" => {
            let ([dst @ Sgpr(_)], [src @ Vgpr(_)]) = (&ins.dst[..], &ins.src[..]) else {
                return Err("invalid first lane read".into());
            };
            em.touch(dst);
            em.touch(src);
            // With EXEC entirely clear, the hardware reads lane zero.
            em.line(&format!("{} = tb_read({}, exec_lo != 0u ? firstbitlow(exec_lo) : (exec_hi != 0u ? 32u + firstbitlow(exec_hi) : 0u));", r(dst)?, r(src)?));
        }
        "v_readlane_b32" => {
            // For this VOP3 opcode the encoded VDST field names an SGPR.
            let ([Vgpr(n @ 0..=105)], [src @ Vgpr(_), lane @ (ConstInt(_) | Sgpr(_)), Sgpr(0)]) =
                (&ins.dst[..], &ins.src[..])
            else {
                return Err("invalid indexed lane read".into());
            };
            em.touch(&Sgpr(*n));
            em.touch(src);
            em.touch(lane);
            em.line(&format!("s{n} = tb_read({}, {} & 63u);", r(src)?, r(lane)?));
        }
        "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32" => {
            let ([dst @ Vgpr(_)], [mask, add, Sgpr(0)]) = (&ins.dst[..], &ins.src[..]) else {
                return Err("invalid masked lane count".into());
            };
            em.touch(dst);
            em.touch(mask);
            em.touch(add);
            let half = if ins.mnemonic == "v_mbcnt_lo_u32_b32" {
                "min(tb_lane, 32u)"
            } else {
                "max(tb_lane, 32u) - 32u"
            };
            em.line(&format!("{{ uint n = {half}; uint prior = n == 32u ? ~0u : ((1u << (n & 31u)) - 1u); if (tb_active) {} = {} + countbits({} & prior); }}", r(dst)?, r(add)?, r(mask)?));
        }
        "v_permlanex16_b32" => {
            let ([dst @ Vgpr(_)], [src @ Vgpr(_), lo, hi], [word, arg]) =
                (&ins.dst[..], &ins.src[..], &ins.raw[..])
            else {
                return Err("invalid cross-row permute".into());
            };
            if arg & 0xf800_0000 != 0 {
                return Err("unsupported cross-row permute modifiers".into());
            }
            // These two native encodings select the first or last lane from
            // the adjacent 16-lane row. The latter explicitly fetches inactive lanes.
            let (at, fetch_inactive) = match (lo, hi, word & !0xff) {
                (ConstInt(0), ConstInt(0), 0xd778_0000) => (0, false),
                (ConstInt(-1), ConstInt(-1), 0xd778_4800) => (15, true),
                _ => return Err("unsupported cross-row lane selectors".into()),
            };
            em.touch(src);
            em.touch(dst);
            em.line(&format!(
                "{{ uint peer = ((tb_lane & ~15u) ^ 16u) | {at}u; uint value = tb_read({}, peer);",
                r(src)?
            ));
            let source_guard = if fetch_inactive {
                ""
            } else {
                " && (((peer < 32u ? exec_lo : exec_hi) >> (peer & 31u)) & 1u)"
            };
            em.line(&format!(
                "if (tb_active{source_guard}) {} = value; }}",
                r(dst)?
            ));
        }
        _ => return Ok(false),
    }
    Ok(true)
}
