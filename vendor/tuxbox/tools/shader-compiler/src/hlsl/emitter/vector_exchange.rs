//! Vector instructions that read other lanes.
use super::*;

impl<'a> Emitter<'a> {
    /// Broadcast the first active lane to a scalar register.
    pub(in crate::hlsl) fn emit_readfirstlane(&mut self, s: &[Opnd], d: Opnd) -> Result<(), String> {
        let value = r(&s[0])?;
        if self.resolve_wave32 {
            return self.ws(&d, &format!("WaveReadLaneAt({value}, exec_lo == 0u ? 0u : uint(firstbitlow(exec_lo)))"));
        }
        if self.ex.stage == Stage::Mesh || (self.is_cs && compute_group(self.ex) == [1, 1, 1]) {
            return self.ws(&d, &value);
        }
        // A pixel wave holds helper lanes, which host wave intrinsics skip. Give
        // each pixel a wave of its own: it elects itself, so a waterfall over
        // heap indices runs once with that pixel's index.
        if self.pixel_heap || lone_lane::on() {
            return self.ws(&d, &value);
        }
        // Scalar writes reach every invocation. Select the first lane whose guest
        // EXEC bit is set, even when the executing host wave also has masked lanes.
        let t = self.tmp;
        self.tmp += 1;
        self.line("#if __SHADER_TARGET_MAJOR >= 6");
        self.line(&format!("uint t{t} = WaveActiveMin((exec_lo & 1u) ? WaveGetLaneIndex() : 0xffffffffu);"));
        self.ws(&d, &format!("WaveReadLaneAt({value}, t{t} == 0xffffffffu ? 0u : t{t})"))?;
        self.line("#else");
        self.ws(&d, &value)?;
        self.line("#endif");
        return Ok(());
    }

    /// Read a lane from the other half of the wave.
    pub(in crate::hlsl) fn emit_permlanex16(&mut self, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        let flags = (ins.raw[0] >> 11) & 0xf;
        if flags != 0 || ins.aux.mask != 0 || ins.aux.imm != 0 || ins.aux.imm2 != 0 {
            return Err("permlanex16 modifiers unsupported".into());
        }
        let (value, lo, hi) = (r(&s[0])?, r(&s[1])?, r(&s[2])?);
        if self.pixel_wave { return self.wv(&d, if self.pixel_sum { "0u" } else { &value }); }
        if self.portable_wave64 {
            // portable_single_wave permits only the recognized cross-row
            // exchange inside a full-EXEC reduction with selectors zero.
            if s[1..] != [Opnd::ConstInt(0), Opnd::ConstInt(0)] {
                return Err("unverified portable wave64 cross-row selectors".into());
            }
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = tb_reduction_read(gtid.x, {value}, (gtid.x & ~15u) ^ 16u);"));
            return self.wv(&d, &format!("t{t}"));
        }
        let t = self.tmp;
        self.tmp += 1;
        self.line(&format!("uint t{t}lane = WaveGetLaneIndex(); uint t{t}sel = (t{t}lane & 8u) ? {hi} : {lo};"));
        self.line(&format!("uint t{t}src = ((t{t}lane & ~15u) ^ 16u) | ((t{t}sel >> (4u * (t{t}lane & 7u))) & 15u);"));
        self.line(&format!("uint t{t}value = WaveReadLaneAt({value}, t{t}src);"));
        // FI=0, BOUND_CTRL=0: an inactive source preserves the destination.
        self.line(&format!("uint4 t{t}active = WaveActiveBallot((exec_lo & 1u) != 0u);"));
        self.line(&format!("if (t{t}active[t{t}src >> 5] & (1u << (t{t}src & 31u))) {{"));
        self.wv(&d, &format!("t{t}value"))?;
        self.line("}");
        return Ok(());
    }

    /// Quad and row exchanges.
    pub(in crate::hlsl) fn emit_quad_exchange(&mut self, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        if self.dispatched && (!self.pixel_wave || is_row_move(ins)) {
            return Err("quad exchange in a block-dispatched program".into());
        }
        if ins.op != OP_DPP_BINARY {
            let value = format!("q{}", self.cur_idx);
            let control = ins.raw[1];
            if control >> 24 != 0xff {
                if self.ex.stage != Stage::Compute {
                    return Err("masked DPP16 move requires a known compute wave width".into());
                }
                // Destination masks do not affect which lanes can supply the source.
                let rows = control >> 28;
                let banks = (control >> 24) & 15;
                let row = format!("({rows:#x}u >> (WaveGetLaneIndex() >> 4u))");
                let bank = format!("({banks:#x}u >> ((WaveGetLaneIndex() >> 2u) & 3u))");
                return self.wv(&d, &format!("(({row} & {bank} & 1u) != 0u) ? {value} : {}", r(&d)?));
            }
            return self.wv(&d, &value);
        }
        let peer = format!("q{}", self.cur_idx);
        let value = match ins.mnemonic.as_str() {
            op @ ("v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32") => {
                let (a, b) = dpp_fp32_sources(ins, peer.clone())?;
                match op {
                    "v_add_f32" => format!("asuint(asfloat({a}) + asfloat({b}))"),
                    "v_sub_f32" => format!("asuint(asfloat({a}) - asfloat({b}))"),
                    "v_subrev_f32" => format!("asuint(asfloat({b}) - asfloat({a}))"),
                    "v_mul_f32" => format!("asuint(asfloat({a}) * asfloat({b}))"),
                    _ => format!("asuint(tb_mull(asfloat({a}), asfloat({b})))"),
                }
            }
            "v_min_f32" | "v_max_f32" => self.fp32_extrema(ins, peer.clone())?,
            "v_cvt_pkrtz_f16_f32" => {
                let (lo, hi) = dpp_fp32_sources(ins, peer.clone())?;
                format!("(tb_f16_rtz(asfloat({lo})) | (tb_f16_rtz(asfloat({hi})) << 16))")
            }
            "v_or_b32" => format!("{peer} | {}", r(&s[1])?),
            "v_and_b32" => format!("{peer} & {}", r(&s[1])?),
            "v_xor_b32" => format!("{peer} ^ {}", r(&s[1])?),
            "v_xnor_b32" => format!("~({peer} ^ {})", r(&s[1])?),
            "v_cndmask_b32" => format!("(vcc_lo & 1u) != 0u ? {} : {peer}", r(&s[1])?),
            "v_add_nc_u32" => format!("{peer} + {}", r(&s[1])?),
            "v_sub_nc_u32" => format!("{peer} - {}", r(&s[1])?),
            "v_subrev_nc_u32" => format!("{} - {peer}", r(&s[1])?),
            "v_lshlrev_b32" => format!("{} << ({peer} & 31u)", r(&s[1])?),
            "v_lshrrev_b32" => format!("{} >> ({peer} & 31u)", r(&s[1])?),
            "v_ashrrev_i32" => format!("asuint(asint({}) >> ({peer} & 31u))", r(&s[1])?),
            "v_min_i32" => format!("asuint(min(asint({peer}), asint({})))", r(&s[1])?),
            "v_max_i32" => format!("asuint(max(asint({peer}), asint({})))", r(&s[1])?),
            "v_min_u32" => format!("min({peer}, {})", r(&s[1])?),
            "v_max_u32" => format!("max({peer}, {})", r(&s[1])?),
            op => mul24_expr(op, &peer, &r(&s[1])?).ok_or("unknown quad binary operation")?,
        };
        // The source exchange precedes both the destination mask and EXEC.
        let mut guards = Vec::new();
        if let Some(mask) = group_extrema::destination_mask(ins) {
            if self.ex.stage != Stage::Compute {
                return Err("DPP16 destination masks require a logical compute lane index".into());
            }
            let rows = mask >> 4;
            let banks = mask & 15;
            if rows != 15 {
                let row_bits = group_extrema::row_index_mask(self, rows, banks)?;
                guards.push(format!("(({rows:#x}u >> ((tb_lane >> 4u) & {row_bits}u)) & 1u) != 0u"));
            }
            if banks != 15 { guards.push(format!("(({banks:#x}u >> ((tb_lane >> 2u) & 3u)) & 1u) != 0u")); }
        }
        if !group_extrema::is_dpp8(ins) && ins.raw[1] & (3 << 18) == 0 {
            guards.push(format!("{peer}active"));
        }
        if guards.is_empty() { return self.wv(&d, &value); }
        return self.wv(&d, &format!("{} ? ({value}) : {}", guards.join(" && "), r(&d)?));
    }

    /// One step of a recognised wave reduction.
    pub(in crate::hlsl) fn emit_dpp_reduce(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        // Read the partner before the EXEC-guarded write, including when VDST aliases VSRC.
        let t = self.tmp;
        self.tmp += 1;
        let acc = r(&s[0])?;
        if self.pixel_wave {
            // The other lanes hold the identity, so each step keeps this one's value.
            self.pixel_sum = m.starts_with("v_add");
            return self.wv(&d, &acc);
        }
        let id = if self.portable_wave64 { "gtid.x" } else { "WaveGetLaneIndex()" };
        let control = ins.raw[1] >> 8; // The reduction matcher verified this word.
        let lane = match (ins.aux.imm, control) {
            (0, _) => format!("{id} ^ 1u"),
            (1, 0x936012) => format!("({id} & ~3u) | (({id} ^ 2u) & 2u)"),
            (2, 0x000924) => format!("({id} & ~7u) | (({id} ^ 4u) & 4u)"),
            (1, 0xb3e21a) => format!("{id} ^ 2u"),
            (2, 0x688fac) => format!("{id} ^ 4u"),
            (1 | 2, _) => format!("({id} & ~7u) | (({control:#x}u >> (3u * ({id} & 7u))) & 7u)"),
            (3, _) => format!("{id} ^ 15u"),
            _ => return Err("invalid integer sum step".into()),
        };
        let partner = if self.portable_wave64 {
            format!("tb_reduction_read(gtid.x, {acc}, {lane})")
        } else { format!("WaveReadLaneAt({acc}, {lane})") };
        let combined = match m.as_str() {
            "v_add_nc_u32" => format!("{partner} + {acc}"),
            "v_and_b32" => format!("{partner} & {acc}"),
            "v_or_b32" => format!("{partner} | {acc}"),
            "v_xor_b32" => format!("{partner} ^ {acc}"),
            "v_min_u32" => format!("min({partner}, {acc})"),
            "v_max_u32" => format!("max({partner}, {acc})"),
            "v_min_i32" => format!("asuint(min(asint({partner}), asint({acc})))"),
            "v_max_i32" => format!("asuint(max(asint({partner}), asint({acc})))"),
            "v_add_f32" => format!("asuint(asfloat({partner}) + asfloat({acc}))"),
            "v_min_f32" => format!("asuint(min(asfloat({partner}), asfloat({acc})))"),
            "v_max_f32" => format!("asuint(max(asfloat({partner}), asfloat({acc})))"),
            _ => return Err("invalid DPP reduction operation".into()),
        };
        self.line(&format!("uint t{t} = {combined};"));
        return self.wv(&d, &format!("t{t}"));
    }

    /// DPP8 exchange between neighbouring lanes.
    pub(in crate::hlsl) fn emit_dpp8_exchange(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        if !self.portable_wave64 { return Err("DPP8 exchange without convergent group".into()); }
        let a = r(&s[0])?;
        let b = r(&s[1])?;
        let selector = ins.aux.imm as u32;
        let t = self.tmp;
        self.tmp += 1;
        self.line(&format!("uint t{t} = tb_reduction_read(gtid.x, {a}, (gtid.x & ~7u) | (({selector:#x}u >> (3u * (gtid.x & 7u))) & 7u));"));
        let value = match m.as_str() {
            "v_add_nc_u32" => format!("t{t} + {b}"),
            "v_and_b32" => format!("t{t} & {b}"),
            "v_or_b32" => format!("t{t} | {b}"),
            "v_xor_b32" => format!("t{t} ^ {b}"),
            "v_min_u32" => format!("min(t{t}, {b})"),
            "v_max_u32" => format!("max(t{t}, {b})"),
            "v_min_i32" => format!("asuint(min(asint(t{t}), asint({b})))"),
            "v_max_i32" => format!("asuint(max(asint(t{t}), asint({b})))"),
            _ => return Err("unsupported DPP8 exchange operation".into()),
        };
        return self.wv(&d, &value);
    }
}
