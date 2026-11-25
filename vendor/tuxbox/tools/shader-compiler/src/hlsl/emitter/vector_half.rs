//! Half precision, packed and mixed precision vector arithmetic.
use super::*;

impl<'a> Emitter<'a> {
    /// Fused multiply and add with mixed precision sources.
    pub(in crate::hlsl) fn emit_fma_mix(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        // VOP3P: D = S0 * S1 + S2 where each source is either a full float or a half
        // float taken from the low/high 16 bits of its register and widened:
        //   OPSEL_HI bit i (dword1[28:27], dword0[14] for i = 2) = 0 -> 32-bit float,
        //                                                         1 -> 16-bit float;
        //   OPSEL bit i (dword0[13:11]) picks the high half when the source is 16-bit;
        //   NEG bit i (dword1[31:29]) negates, NEG_HI bit i (dword0[10:8]) takes |x|.
        let (w0, w1) = (ins.raw[0], ins.raw.get(1).copied().unwrap_or(0));
        let opsel = (w0 >> 11) & 7;
        let opsel_hi = ((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2);
        let (neg, abs) = ((w1 >> 29) & 7, (w0 >> 8) & 7);
        let mut v = Vec::new();
        for i in 0..3 {
            let x = r(&s[i])?;
            let mut e = if (opsel_hi >> i) & 1 != 0 {
                if (opsel >> i) & 1 != 0 { format!("f16tof32(({x}) >> 16)") } else { format!("f16tof32(({x}) & 0xffffu)") }
            } else {
                format!("asfloat({x})")
            };
            if (abs >> i) & 1 != 0 {
                e = format!("abs({e})");
            }
            if (neg >> i) & 1 != 0 {
                e = format!("(-{e})");
            }
            v.push(e);
        }
        let e = format!("{} * {} + {}", v[0], v[1], v[2]);
        return match m.as_str() {
            "v_fma_mix_f32" => self.wv(&d, &format!("asuint({e})")),
            // The f16 result replaces one half of the destination; the other half is kept.
            "v_fma_mixlo_f16" => {
                self.touch(&d);
                self.wv(&d, &format!("(({}) & 0xffff0000u) | tb_f16_rne({e})", r(&d)?))
            }
            _ => {
                self.touch(&d);
                self.wv(&d, &format!("(({}) & 0xffffu) | (tb_f16_rne({e}) << 16)", r(&d)?))
            }
        };
    }

    /// Operations on pairs of half precision values.
    pub(in crate::hlsl) fn emit_packed_f16(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        // Packed half-float ops (VOP3P): two f16 lanes per register. For the low result,
        // OPSEL bit i (dword0[13:11]) picks the low/high half of source i and NEG bit i
        // (dword1[31:29]) negates it; for the high result OPSEL_HI (dword1[28:27],
        // dword0[14]) and NEG_HI (dword0[10:8]) do the same.
        let (w0, w1) = (ins.raw[0], ins.raw.get(1).copied().unwrap_or(0));
        let nsrc = if m == "v_pk_fma_f16" { 3 } else { 2 };
        let sel = [(w0 >> 11) & 7, ((w1 >> 27) & 3) | (((w0 >> 14) & 1) << 2)];
        let neg = [(w1 >> 29) & 7, (w0 >> 8) & 7];
        let mut halves = Vec::new();
        for h in 0..2 {
            let mut v = Vec::new();
            for i in 0..nsrc {
                let x = match &s[i] {
                    o @ Opnd::ConstF32(_) => format!("tb_f16_rne({})", f(o)?),
                    o => r(o)?,
                };
                let mut e = if (sel[h] >> i) & 1 != 0 { format!("f16tof32(({x}) >> 16)") } else { format!("f16tof32(({x}) & 0xffffu)") };
                if (neg[h] >> i) & 1 != 0 {
                    e = format!("(-{e})");
                }
                v.push(e);
            }
            halves.push(match m.as_str() {
                "v_pk_fma_f16" => format!("{} * {} + {}", v[0], v[1], v[2]),
                "v_pk_add_f16" => format!("{} + {}", v[0], v[1]),
                "v_pk_mul_f16" => format!("{} * {}", v[0], v[1]),
                "v_pk_min_f16" => format!("min({}, {})", v[0], v[1]),
                _ => format!("max({}, {})", v[0], v[1]),
            });
        }
        if w0 & (1 << 15) != 0 {
            for value in &mut halves {
                *value = format!("saturate({value})");
            }
        }
        let pack = if self.single_wave32 && matches!(m.as_str(), "v_pk_min_f16" | "v_pk_max_f16") {
            "tb_repack_half"
        } else {
            "tb_f16_rne"
        };
        return self.wv(&d, &format!("({pack}({}) | ({pack}({}) << 16))", halves[0], halves[1]));
    }

    /// Pack two values as normalised 16-bit integers.
    pub(in crate::hlsl) fn emit_pack_unorm16(&mut self, d: Opnd, ins: &Insn) -> Result<(), String> {
        if ins.raw[0] & 0xfc00 != 0 || ins.raw[1] & 0x98000000 != 0 {
            return Err("unsupported UNORM16 pack modifiers".into());
        }
        if self.ex.stage == Stage::Pixel {
            // The direct two-channel export is used for R16G16_UNORM. Until
            // additional export formats have provenance, reject a skipped or
            // indirect pack instead of reinterpreting the bits as FP16.
            let e = self.insns.get(self.cur_idx + 1).ok_or("UNORM16 pack without export")?;
            if e.enc != Enc::Exp || e.aux.imm != 0 || e.aux.mask != 3 || e.aux.imm2 & 1 == 0
                || e.src.first() != Some(&d) || self.insns.iter().any(|i| i.enc == Enc::Sopp
                    && Self::branch_cond(i.op).is_some() && i.offset as i64 + 4 + 4*i.aux.imm == e.offset as i64)
            {
                return Err("unresolved packed UNORM16 pixel export".into());
            }
        }
        let lo = self.fm(ins, 0)?;
        let hi = self.fm(ins, 1)?;
        return self.wv(&d, &format!("((uint)round(saturate({lo}) * 65535.0) | ((uint)round(saturate({hi}) * 65535.0) << 16))"));
    }

    /// Half precision fused multiply and add.
    pub(in crate::hlsl) fn emit_fma_f16(&mut self, s: &[Opnd], d: Opnd, ins: &Insn, h16: &dyn Fn(&Opnd) -> Result<String, String>, hres: &dyn Fn(String) -> String) -> Result<(), String> {
        let sel = (ins.raw[0] >> 11) & 15;
        let src = |i: usize| -> Result<String, String> {
            if sel & (1 << i) == 0 { return h16(&s[i]); }
            // Inline FP constants have a binary16 payload and a zero upper half.
            let value = if matches!(s[i], Opnd::ConstF32(_)) {
                "0.0".to_string()
            } else {
                format!("f16tof32(({}) >> 16)", r(&s[i])?)
            };
            Ok(Self::float_source(value, ins.aux.mask, i))
        };
        let result = hres(format!("tb_fma_half({}, {}, {})", src(0)?, src(1)?, src(2)?));
        self.touch(&d);
        let dest = r(&d)?;
        return self.wv(&d, &if sel & 8 == 0 {
            format!("(({dest} & 0xffff0000u) | {result})")
        } else {
            format!("(({dest} & 0xffffu) | ({result} << 16))")
        });
    }

    /// Unary half precision operations and conversions.
    pub(in crate::hlsl) fn emit_half_unary(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn, unary_op: Option<u8>, modified_half_bits: &dyn Fn() -> Result<String, String>) -> Result<(), String> {
        let has_mode = self.regs.contains("tb_fp_mode");
        let mode = if has_mode { "tb_fp_mode" } else { "0xc0u" };
        let narrow = |value| if has_mode {
            format!("tb_mode_half({value}, {mode})")
        } else {
            format!("tb_f16_rne({value})")
        };
        let mut bits = if let Some(op) = unary_op {
            if let Opnd::ConstF32(value) = s[0] {
                // Inline floats are finite normal halves. Only 1/(2*pi)
                // has a distinct rounded value in the half-precision ROM.
                let mut value = inline_half_value(value);
                if ins.enc == Enc::Vop3 {
                    if ins.aux.mask & 1 != 0 { value = value.abs(); }
                    if ins.aux.mask & 8 != 0 { value = -value; }
                }
                let x = f(&Opnd::ConstF32(value))?;
                format!("tb_finite_unary_f16({x}, {mode}, {op}u)")
            } else {
                format!("tb_unary_f16({}, {mode}, {op}u)", modified_half_bits()?)
            }
        } else {
            let raw = half_operand_bits(&s[0])?;
            let integer = if m == "v_cvt_f16_i16" {
                format!("(asint(({raw}) << 16) >> 16)")
            } else {
                format!("(({raw}) & 0xffffu)")
            };
            narrow(format!("(float){integer}"))
        };
        if ins.enc == Enc::Vop3 && (ins.aux.imm2 != 0 || ins.aux.imm != 0) {
            let scale = ["1.0", "2.0", "4.0", "0.5"][ins.aux.imm2 as usize];
            let clamp = ins.aux.imm != 0;
            bits = format!("tb_half_output({bits}, {mode}, {scale}, {clamp})");
        }
        return self.wv(&d, &format!("(({} & 0xffff0000u) | {bits})", r(&d)?));
    }
}
