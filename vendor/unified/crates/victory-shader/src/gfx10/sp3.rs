//! Instruction text in AMD's SP3 assembly syntax.

use std::fmt::{self, Display, Write};

use super::fields::{Cache, Dpp, Fields, NULL, Operand, Valu};
use super::{Encoding, Instruction};

impl Instruction {
    /// The instruction in SP3 syntax, e.g. `v_add_f32 v1, 0.5, v8`.
    /// Branch targets are written `label_<byte offset>`.
    pub fn text(&self, wave64: bool) -> String {
        let name = self.name().unwrap_or("?");
        let ops = operands(self, name, &self.fields(wave64));
        if ops.is_empty() {
            name.to_owned()
        } else {
            format!("{name} {ops}")
        }
    }
}

impl Display for Operand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Operand::Scalar { code, dwords: 1 } => f.write_str(&scalar_name(code)),
            Operand::Scalar {
                code: 106,
                dwords: 2,
            } => f.write_str("vcc"),
            Operand::Scalar {
                code: 126,
                dwords: 2,
            } => f.write_str("exec"),
            Operand::Scalar { code, dwords } if code < 106 => {
                write!(f, "s[{code}:{}]", code + dwords - 1)
            }
            Operand::Scalar { code, dwords } if (108..124).contains(&code) => {
                write!(f, "ttmp[{}:{}]", code - 108, code - 108 + dwords - 1)
            }
            Operand::Scalar { code, .. } => f.write_str(&scalar_name(code)),
            Operand::Vector { reg, dwords: 1 } => write!(f, "v{reg}"),
            Operand::Vector { reg, dwords } => {
                write!(f, "v[{reg}:{}]", reg as u32 + dwords as u32 - 1)
            }
            Operand::Const(code) => f.write_str(&const_name(code)),
            Operand::Literal(value) => write!(f, "lit(0x{value:08x})"),
            Operand::Attr { index, chan, .. } => write!(
                f,
                "attr{index}.{}",
                ['x', 'y', 'z', 'w'][usize::from(chan & 3)]
            ),
        }
    }
}

fn scalar_name(code: u8) -> String {
    match code {
        0..=105 => format!("s{code}"),
        106 => "vcc_lo".into(),
        107 => "vcc_hi".into(),
        108..=123 => format!("ttmp{}", code - 108),
        124 => "m0".into(),
        125 => "null".into(),
        126 => "exec_lo".into(),
        _ => "exec_hi".into(),
    }
}

fn const_name(code: u8) -> String {
    match code {
        128..=192 => (i32::from(code) - 128).to_string(),
        193..=208 => (192 - i32::from(code)).to_string(),
        240 => "0.5".into(),
        241 => "-0.5".into(),
        242 => "1.0".into(),
        243 => "-1.0".into(),
        244 => "2.0".into(),
        245 => "-2.0".into(),
        246 => "4.0".into(),
        247 => "-4.0".into(),
        248 => "0.15915494".into(),
        235 => "src_shared_base".into(),
        236 => "src_shared_limit".into(),
        237 => "src_private_base".into(),
        238 => "src_private_limit".into(),
        239 => "src_pops_exiting_wave_id".into(),
        251 => "src_vccz".into(),
        252 => "src_execz".into(),
        253 => "src_scc".into(),
        254 => "src_lds_direct".into(),
        _ => format!("<src {code}>"),
    }
}

fn join(ops: impl IntoIterator<Item = String>) -> String {
    ops.into_iter().collect::<Vec<_>>().join(", ")
}

fn operands(inst: &Instruction, name: &str, fields: &Fields) -> String {
    let mut out = String::new();
    match fields {
        Fields::Sop { dst, src, imm } => {
            let regs = join(dst.iter().chain(src).map(|o| o.to_string()));
            out = match (name, imm) {
                ("s_getreg_b32", Some(imm)) => format!("{regs}, {}", hwreg(*imm)),
                ("s_setreg_b32" | "s_setreg_imm32_b32", Some(imm)) => {
                    format!("{}, {regs}", hwreg(*imm))
                }
                ("s_call_b64", Some(imm)) => format!("{regs}, {}", label(inst, *imm)),
                (_, Some(imm)) if regs.is_empty() => format!("0x{imm:04x}"),
                (_, Some(imm)) => format!("{regs}, 0x{imm:04x}"),
                (_, None) => regs,
            };
        }
        Fields::Sopp { imm } => {
            out = match name {
                "s_endpgm"
                | "s_barrier"
                | "s_code_end"
                | "s_endpgm_saved"
                | "s_endpgm_ordered_ps_done"
                | "s_wakeup"
                | "s_icache_inv"
                | "s_ttracedata" => String::new(),
                "s_branch" => label(inst, *imm),
                n if n.starts_with("s_cbranch_") => label(inst, *imm),
                "s_waitcnt" => waitcnt(*imm),
                "s_sendmsg" | "s_sendmsghalt" => sendmsg(*imm),
                _ => format!("0x{imm:04x}"),
            };
        }
        Fields::Smem {
            data,
            base,
            offset,
            soffset,
            cache,
        } => {
            if let Some(data) = data {
                let off = match (soffset, offset) {
                    (Operand::Scalar { code: NULL, .. }, 0) => soffset.to_string(),
                    (Operand::Scalar { code: NULL, .. }, _) => {
                        format!("0x{:06x}", offset & 0x1F_FFFF)
                    }
                    (_, 0) => soffset.to_string(),
                    _ => format!("{soffset} offset:0x{:06x}", offset & 0x1F_FFFF),
                };
                out = format!("{data}, {base}, {off}");
            }
            flags(&mut out, cache, false);
        }
        Fields::Valu(v) => out = valu(name, v, inst.encoding),
        Fields::Vintrp { dst, src, attr } => {
            let src = match (name, src) {
                ("v_interp_mov_f32", Operand::Const(p)) => ["p10", "p20", "p0"]
                    .get(usize::from(*p))
                    .map_or("p?".into(), |p| p.to_string()),
                _ => src.to_string(),
            };
            out = format!("{dst}, {src}, {attr}");
        }
        Fields::Ds {
            dst,
            addr,
            data,
            offset0,
            offset1,
            gds,
        } => {
            out = join(dst.iter().chain(addr).chain(data).map(|o| o.to_string()));
            if super::fields::ds_pair(name) {
                if *offset0 != 0 {
                    let _ = write!(out, " offset0:{offset0}");
                }
                if *offset1 != 0 {
                    let _ = write!(out, " offset1:{offset1}");
                }
            } else {
                let offset = u16::from(*offset1) << 8 | u16::from(*offset0);
                if offset != 0 {
                    let _ = write!(out, " offset:{offset}");
                }
            }
            if *gds {
                out.push_str(" gds");
            }
        }
        Fields::Buffer {
            data,
            addr,
            rsrc,
            soffset,
            offset,
            offen,
            idxen,
            format,
            lds,
            tfe,
            cache,
        } => {
            if let Some(data) = data {
                out = format!("{data}, {addr}, {rsrc}, {soffset}");
                if *idxen {
                    out.push_str(" idxen");
                }
                if *offen {
                    out.push_str(" offen");
                }
                if *offset != 0 {
                    let _ = write!(out, " offset:{offset}");
                }
                if let Some(format) = format {
                    let _ = write!(out, " format:[{}]", buffer_format(*format));
                }
            }
            flags(&mut out, cache, false);
            if *lds {
                out.push_str(" lds");
            }
            if *tfe {
                out.push_str(" tfe");
            }
        }
        Fields::Image {
            data,
            addr,
            rsrc,
            sampler,
            dmask,
            dim,
            unorm,
            r128,
            a16,
            d16,
            tfe,
            lwe,
            cache,
        } => {
            let addr = match addr.as_slice() {
                [one] => one.to_string(),
                many => format!(
                    "[{}]",
                    many.iter()
                        .map(|o| o.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
            };
            out = join(
                [data.to_string(), addr, rsrc.to_string()]
                    .into_iter()
                    .chain(sampler.map(|s| s.to_string())),
            );
            if *dmask != 1 {
                let _ = write!(out, " dmask:0x{dmask:x}");
            }
            let dims = [
                "1D",
                "2D",
                "3D",
                "CUBE",
                "1D_ARRAY",
                "2D_ARRAY",
                "2D_MSAA",
                "2D_MSAA_ARRAY",
            ];
            let _ = write!(out, " dim:SQ_RSRC_IMG_{}", dims[usize::from(*dim & 7)]);
            for (on, text) in [(*r128, "r128"), (*unorm, "unorm")] {
                if on {
                    out.push(' ');
                    out.push_str(text);
                }
            }
            flags(&mut out, cache, true);
            for (on, text) in [(*a16, "a16"), (*d16, "d16"), (*tfe, "tfe"), (*lwe, "lwe")] {
                if on {
                    out.push(' ');
                    out.push_str(text);
                }
            }
        }
        Fields::Flat {
            dst,
            addr,
            data,
            saddr,
            offset,
            lds,
            cache,
        } => {
            let addr = addr.map_or("off".into(), |a| a.to_string());
            out = join(
                dst.iter()
                    .map(|o| o.to_string())
                    .chain([addr])
                    .chain(data.iter().chain(saddr).map(|o| o.to_string())),
            );
            if *offset != 0 {
                let _ = write!(out, " inst_offset:{offset}");
            }
            flags(&mut out, cache, false);
            if *lds {
                out.push_str(" lds");
            }
        }
        Fields::Exp {
            target,
            src,
            compr,
            done,
            vm,
        } => {
            let target = match *target {
                0..=7 => format!("mrt{target}"),
                8 => "mrtz".into(),
                9 => "null".into(),
                12..=15 => format!("pos{}", target - 12),
                20 => "prim".into(),
                32..=63 => format!("param{}", target - 32),
                _ => format!("<target {target}>"),
            };
            out = join(
                [target].into_iter().chain(
                    src.iter()
                        .map(|s| s.map_or("off".into(), |o| o.to_string())),
                ),
            );
            for (on, text) in [(*done, " done"), (*compr, " compr"), (*vm, " vm")] {
                if on {
                    out.push_str(text);
                }
            }
        }
    }
    out
}

/// Cache flags; images write `dlc` first.
fn flags(out: &mut String, cache: &Cache, dlc_first: bool) {
    let dlc = (cache.dlc, " dlc");
    let (first, last) = if dlc_first {
        (dlc, (false, ""))
    } else {
        ((false, ""), dlc)
    };
    for (on, text) in [first, (cache.glc, " glc"), (cache.slc, " slc"), last] {
        if on {
            out.push_str(text);
        }
    }
    if out.starts_with(' ') {
        out.remove(0);
    }
}

fn label(inst: &Instruction, imm: u16) -> String {
    let target = inst.offset as i64 + 4 * inst.words.len() as i64 + 4 * i64::from(imm as i16);
    format!("label_{target:04X}")
}

fn hwreg(imm: u16) -> String {
    let (id, offset, size) = (imm & 0x3F, imm >> 6 & 0x1F, (imm >> 11 & 0x1F) + 1);
    let name = match id {
        1 => "HW_REG_MODE".into(),
        20 => "HW_REG_SHADER_FLAT_SCRATCH_LO".into(),
        21 => "HW_REG_SHADER_FLAT_SCRATCH_HI".into(),
        25 => "HW_REG_POPS_PACKER".into(),
        _ => id.to_string(),
    };
    if offset == 0 && size == 32 {
        format!("hwreg({name})")
    } else {
        format!("hwreg({name}, {offset}, {size})")
    }
}

fn waitcnt(imm: u16) -> String {
    let vm = imm & 0xF | (imm >> 14) << 4;
    let exp = imm >> 4 & 7;
    let lgkm = imm >> 8 & 0x3F;
    let mut parts = Vec::new();
    if vm != 0x3F {
        parts.push(format!("vmcnt({vm})"));
    }
    if exp != 7 {
        parts.push(format!("expcnt({exp})"));
    }
    if lgkm != 0x3F {
        parts.push(format!("lgkmcnt({lgkm})"));
    }
    if matches!(parts.len(), 0 | 3) {
        return format!("0x{imm:04x}");
    }
    parts.join(" & ")
}

fn sendmsg(imm: u16) -> String {
    let (msg, op, stream) = (imm & 0xF, imm >> 4 & 7, imm >> 8 & 3);
    let msg_name = match msg {
        2 => "MSG_GS".into(),
        3 => "MSG_GS_DONE".into(),
        7 => "MSG_ORDERED_PS_DONE".into(),
        9 => "MSG_GS_ALLOC_REQ".into(),
        _ => msg.to_string(),
    };
    let op_name = ["GS_OP_NOP", "GS_OP_CUT", "GS_OP_EMIT", "GS_OP_EMIT_CUT"]
        .get(usize::from(op))
        .map_or(op.to_string(), |s| s.to_string());
    if msg == 3 && op == 0 {
        format!("sendmsg({msg_name}, {op_name})")
    } else {
        format!("sendmsg({msg_name}, {op_name}, {stream})")
    }
}

fn buffer_format(format: u8) -> String {
    const NAMES: [&str; 78] = [
        "INVALID",
        "8_UNORM",
        "8_SNORM",
        "8_USCALED",
        "8_SSCALED",
        "8_UINT",
        "8_SINT",
        "16_UNORM",
        "16_SNORM",
        "16_USCALED",
        "16_SSCALED",
        "16_UINT",
        "16_SINT",
        "16_FLOAT",
        "8_8_UNORM",
        "8_8_SNORM",
        "8_8_USCALED",
        "8_8_SSCALED",
        "8_8_UINT",
        "8_8_SINT",
        "32_UINT",
        "32_SINT",
        "32_FLOAT",
        "16_16_UNORM",
        "16_16_SNORM",
        "16_16_USCALED",
        "16_16_SSCALED",
        "16_16_UINT",
        "16_16_SINT",
        "16_16_FLOAT",
        "10_11_11_UNORM",
        "10_11_11_SNORM",
        "10_11_11_USCALED",
        "10_11_11_SSCALED",
        "10_11_11_UINT",
        "10_11_11_SINT",
        "10_11_11_FLOAT",
        "11_11_10_UNORM",
        "11_11_10_SNORM",
        "11_11_10_USCALED",
        "11_11_10_SSCALED",
        "11_11_10_UINT",
        "11_11_10_SINT",
        "11_11_10_FLOAT",
        "10_10_10_2_UNORM",
        "10_10_10_2_SNORM",
        "10_10_10_2_USCALED",
        "10_10_10_2_SSCALED",
        "10_10_10_2_UINT",
        "10_10_10_2_SINT",
        "2_10_10_10_UNORM",
        "2_10_10_10_SNORM",
        "2_10_10_10_USCALED",
        "2_10_10_10_SSCALED",
        "2_10_10_10_UINT",
        "2_10_10_10_SINT",
        "8_8_8_8_UNORM",
        "8_8_8_8_SNORM",
        "8_8_8_8_USCALED",
        "8_8_8_8_SSCALED",
        "8_8_8_8_UINT",
        "8_8_8_8_SINT",
        "32_32_UINT",
        "32_32_SINT",
        "32_32_FLOAT",
        "16_16_16_16_UNORM",
        "16_16_16_16_SNORM",
        "16_16_16_16_USCALED",
        "16_16_16_16_SSCALED",
        "16_16_16_16_UINT",
        "16_16_16_16_SINT",
        "16_16_16_16_FLOAT",
        "32_32_32_UINT",
        "32_32_32_SINT",
        "32_32_32_FLOAT",
        "32_32_32_32_UINT",
        "32_32_32_32_SINT",
        "32_32_32_32_FLOAT",
    ];
    NAMES
        .get(usize::from(format))
        .map_or(format!("{format}"), |n| format!("BUF_FMT_{n}"))
}

fn valu(name: &str, v: &Valu, encoding: Encoding) -> String {
    let bit = |mask: u8, i: usize| mask >> i & 1 != 0;
    let mix = name.starts_with("v_fma_mix");
    let src = |i: usize| {
        let o = v.src[i];
        let mut s = o.to_string();
        if let Some(sdwa) = v.sdwa
            && bit(sdwa.sext, i)
        {
            s = format!("sext({s})");
        }
        if encoding == Encoding::Vop3p {
            if mix {
                if bit(v.opsel_hi, i) {
                    s = format!("{}({s})", if bit(v.opsel, i) { "sel_hi" } else { "sel_lo" });
                }
                if bit(v.neg_hi, i) {
                    s = format!("abs({s})");
                }
                if bit(v.neg, i) {
                    s = format!("-{s}");
                }
            } else {
                if bit(v.neg, i) && bit(v.neg_hi, i) {
                    s = format!("-{s}");
                }
                match (bit(v.opsel, i), bit(v.opsel_hi, i)) {
                    (false, true) => {}
                    (false, false) => s = format!("sel_lo({s})"),
                    (true, true) => s = format!("sel_hi({s})"),
                    (true, false) => s = format!("sel_hi_lo({s})"),
                }
            }
        } else {
            if bit(v.abs, i) {
                s = format!("abs({s})");
            }
            if bit(v.neg, i) {
                s = format!("-{s}");
            }
            // v_permlane*: the opsel bits are fi and bound_ctrl.
            if bit(v.opsel, i) && !name.starts_with("v_permlane") {
                s = format!("sel_hi({s})");
            }
        }
        s
    };
    let mut ops: Vec<String> = v
        .dst
        .iter()
        .chain(&v.carry)
        .map(|o| o.to_string())
        .collect();
    let mut order: Vec<usize> = (0..v.src.len()).collect();
    if name.starts_with("v_interp_") && order.len() > 1 {
        // dst, i/j, attribute, then the p1 result for p2.
        order.swap(0, 1);
    }
    ops.extend(order.into_iter().map(src));
    let mut out = ops.join(", ");
    let mut suffix = |s: &str| {
        out.push(' ');
        out.push_str(s);
    };
    if let Some(Operand::Attr { high: true, .. }) = v.src.first() {
        suffix("high");
    }
    let omod = ["", "mul:2", "mul:4", "div:2"][usize::from(v.omod & 3)];
    if name.starts_with("v_permlane") {
        if v.opsel & 1 != 0 {
            suffix("fi:1");
        }
        if v.opsel & 2 != 0 {
            suffix("bound_ctrl:1");
        }
    }
    if let Some(sdwa) = v.sdwa {
        if v.clamp {
            suffix("clamp");
        }
        if !omod.is_empty() {
            suffix(omod);
        }
        let sel = [
            "BYTE_0", "BYTE_1", "BYTE_2", "BYTE_3", "WORD_0", "WORD_1", "DWORD", "?",
        ];
        if sdwa.dst_sel != 6 {
            suffix(&format!("dst_sel:{}", sel[usize::from(sdwa.dst_sel & 7)]));
        }
        if sdwa.dst_unused != 0 {
            let unused = ["UNUSED_PAD", "UNUSED_SEXT", "UNUSED_PRESERVE", "?"];
            suffix(&format!(
                "dst_unused:{}",
                unused[usize::from(sdwa.dst_unused & 3)]
            ));
        }
        for (i, s) in sdwa.src_sel.iter().enumerate() {
            if *s != 6 {
                suffix(&format!("src{i}_sel:{}", sel[usize::from(s & 7)]));
            }
        }
    } else {
        if !omod.is_empty() {
            suffix(omod);
        }
        if v.clamp {
            suffix("clamp");
        }
    }
    match v.dpp {
        Some(Dpp::Row {
            ctrl,
            row_mask,
            bank_mask,
            bound_ctrl,
            fi,
        }) => {
            suffix(&match ctrl {
                0x000..=0x0FF => format!(
                    "quad_perm:[{},{},{},{}]",
                    ctrl & 3,
                    ctrl >> 2 & 3,
                    ctrl >> 4 & 3,
                    ctrl >> 6 & 3
                ),
                0x101..=0x10F => format!("row_shl:{}", ctrl & 0xF),
                0x111..=0x11F => format!("row_shr:{}", ctrl & 0xF),
                0x121..=0x12F => format!("row_ror:{}", ctrl & 0xF),
                0x140 => "row_mirror".into(),
                0x141 => "row_half_mirror".into(),
                0x150..=0x15F => format!("row_share:{}", ctrl & 0xF),
                0x160..=0x16F => format!("row_xmask:{}", ctrl & 0xF),
                _ => format!("dpp_ctrl:0x{ctrl:x}"),
            });
            if row_mask != 0xF {
                suffix(&format!("row_mask:0x{row_mask:x}"));
            }
            if bank_mask != 0xF {
                suffix(&format!("bank_mask:0x{bank_mask:x}"));
            }
            if bound_ctrl {
                suffix("bound_ctrl:1");
            }
            if fi {
                suffix("fi:1");
            }
        }
        Some(Dpp::Lanes { sel, fi }) => {
            let lanes: Vec<String> = (0..8).map(|i| (sel >> (3 * i) & 7).to_string()).collect();
            suffix(&format!("dpp8:[{}]", lanes.join(",")));
            if fi {
                suffix("fi:1");
            }
        }
        None => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::gfx10::{Fields, Operand, decode};

    /// Text of each instruction in `words` (a straight-line program).
    fn texts(words: &[u32], wave64: bool) -> Vec<String> {
        let mut code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        code.extend(0xBF81_0000u32.to_le_bytes()); // s_endpgm
        let insts = decode(&code).unwrap();
        insts[..insts.len() - 1]
            .iter()
            .map(|i| i.text(wave64))
            .collect()
    }

    fn one(words: &[u32], wave64: bool) -> String {
        let t = texts(words, wave64);
        assert_eq!(t.len(), 1, "{t:?}");
        t[0].clone()
    }

    #[test]
    fn scalar() {
        assert_eq!(
            texts(&[0xBF88_0001, 0xBF8C_0070, 0xBE80_03FF, 0x1234_5678], true),
            [
                "s_cbranch_execz label_0008",
                "s_waitcnt vmcnt(0) & lgkmcnt(0)",
                "s_mov_b32 s0, lit(0x12345678)",
            ]
        );
        assert_eq!(one(&[0x8782_027E], true), "s_and_b64 s[2:3], exec, s[2:3]");
        assert_eq!(
            one(&[0xF408_0501, 0xFA00_0040], true),
            "s_load_dwordx4 s[20:23], s[2:3], 0x000040"
        );
        assert_eq!(
            one(&[0xF420_0144, 0x0400_0000], true),
            "s_buffer_load_dword s5, s[8:11], s2"
        );
    }

    #[test]
    fn lane_masks_follow_the_wave_size() {
        let cmp = [0x7C08_06F0]; // v_cmp_gt_f32 with 0.5 and v3
        assert_eq!(one(&cmp, false), "v_cmp_gt_f32 vcc_lo, 0.5, v3");
        assert_eq!(one(&cmp, true), "v_cmp_gt_f32 vcc, 0.5, v3");
        let vop3 = [0xD401_0106, 0x0001_E501]; // v_cmp_lt_f32 to s6, |v1| < 1.0
        assert_eq!(one(&vop3, false), "v_cmp_lt_f32 s6, abs(v1), 1.0");
        assert_eq!(one(&vop3, true), "v_cmp_lt_f32 s[6:7], abs(v1), 1.0");
        assert_eq!(
            one(&[0x0202_0480], false),
            "v_cndmask_b32 v1, 0, v2, vcc_lo"
        );
        assert_eq!(
            one(&[0x5006_0801], true),
            "v_add_co_ci_u32 v3, vcc, s1, v4, vcc"
        );
    }

    #[test]
    fn vector_modifiers() {
        assert_eq!(
            one(&[0x1004_0AFF, 0x3F00_0001], true),
            "v_mul_f32 v2, lit(0x3f000001), v5"
        );
        assert_eq!(
            one(&[0xD54B_8004, 0x2BC0_0501], true),
            "v_fma_f32 v4, -v1, s2, 0.5 mul:2 clamp"
        );
        assert_eq!(
            one(&[0x7E02_16F9, 0x0605_0602], true),
            "v_cvt_f32_f16 v1, v2 src0_sel:WORD_1"
        );
        assert_eq!(
            one(&[0x7E06_02E9, 0x0539_7701], true),
            "v_mov_b32 v3, v1 dpp8:[7,6,5,4,3,2,1,0]"
        );
        assert_eq!(
            one(&[0x7E0A_02FA, 0xFF08_B106], true),
            "v_mov_b32 v5, v6 quad_perm:[1,0,3,2] bound_ctrl:1"
        );
        assert_eq!(
            one(&[0xCC20_0C01, 0x2C10_0702], true),
            "v_fma_mix_f32 v1, -sel_hi(v2), s3, abs(v4)"
        );
    }

    #[test]
    fn memory_and_export() {
        assert_eq!(
            one(&[0xE034_7010, 0x0102_0402], true),
            "buffer_load_dwordx2 v[4:5], v[2:3], s[8:11], s1 idxen offen offset:16 glc"
        );
        assert_eq!(
            one(&[0xD8DC_0302, 0x0600_0001], true),
            "ds_read2_b32 v[6:7], v1 offset0:2 offset1:3"
        );
        assert_eq!(
            one(&[0xF800_1C0F, 0x0000_0302], true),
            "exp mrt0, v2, v2, v3, v3 done compr vm"
        );
    }

    #[test]
    fn image_d16_data_is_packed() {
        // Three f16 channels occupy two VGPRs; NSA lists the addresses.
        let words = [0xF09C_070A, 0x8024_0801, 0x0000_0005];
        assert_eq!(
            one(&words, true),
            "image_sample_lz v[8:9], [v1,v5], s[16:23], s[4:7] dmask:0x7 dim:SQ_RSRC_IMG_2D d16"
        );
        let code: Vec<u8> = words
            .iter()
            .chain(&[0xBF81_0000])
            .flat_map(|w| w.to_le_bytes())
            .collect();
        let Fields::Image { data, addr, .. } = decode(&code).unwrap()[0].fields(true) else {
            panic!("not an image instruction");
        };
        assert_eq!(data, Operand::Vector { reg: 8, dwords: 2 });
        assert_eq!(addr.len(), 2);
    }
}
