//! Compares, sub-dword operands, and integer operations wider than a register.
use super::*;

impl<'a> Emitter<'a> {
    /// 64-bit shifts.
    pub(in crate::hlsl) fn emit_shift64(&mut self, m: String, s: &[Opnd], d: Opnd) -> Result<(), String> {
        // 64-bit vector shifts: value and result are register pairs (lo, hi).
        let pair_hi = |o: &Opnd| match o {
            Opnd::Vgpr(n) => Opnd::Vgpr(n + 1),
            other => hi(other),
        };
        let n = format!("({} & 63u)", r(&s[0])?);
        self.touch(&pair_hi(&s[1]));
        let (lo, hi_e) = (r(&s[1])?, r(&pair_hi(&s[1]))?);
        let t = self.tmp;
        self.tmp += 1;
        let (rl, rh) = match m.as_str() {
            "v_lshlrev_b64" => (
                format!("(({n} >= 32u) ? 0u : ({lo} << ({n} & 31u)))"),
                format!("(({n} == 0u) ? {hi_e} : (({n} >= 32u) ? ({lo} << ({n} - 32u)) : (({hi_e} << ({n} & 31u)) | ({lo} >> (32u - ({n} & 31u))))))"),
            ),
            "v_lshrrev_b64" => (
                format!("(({n} == 0u) ? {lo} : (({n} >= 32u) ? ({hi_e} >> ({n} - 32u)) : (({lo} >> ({n} & 31u)) | ({hi_e} << (32u - ({n} & 31u))))))"),
                format!("(({n} >= 32u) ? 0u : ({hi_e} >> ({n} & 31u)))"),
            ),
            _ => (
                format!("(({n} == 0u) ? {lo} : (({n} >= 32u) ? asuint(asint({hi_e}) >> ({n} - 32u)) : (({lo} >> ({n} & 31u)) | ({hi_e} << (32u - ({n} & 31u))))))"),
                format!("(({n} >= 32u) ? asuint(asint({hi_e}) >> 31) : asuint(asint({hi_e}) >> ({n} & 31u)))"),
            ),
        };
        self.line(&format!("uint t{t}l = {rl}; uint t{t}h = {rh};"));
        self.wv(&d, &format!("t{t}l"))?;
        let dh = pair_hi(&d);
        self.touch(&dh);
        return self.wv(&dh, &format!("t{t}h"));
    }

    /// Vector compares into a lane mask.
    pub(in crate::hlsl) fn emit_compare(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        let mask = ins.aux.mask;
        let c = if m.ends_with("_i64") || m.ends_with("_u64") {
            self.compare_int64(ins, mask)?
        } else if m.ends_with("_f64") || m.ends_with("_f32") || m.ends_with("_f16") {
            self.compare_float(ins, mask)?
        } else {
            self.int_compare(&m, &s[0], &s[1])?
        };
        if self.resolve_wave32 {
            let vote = format!("WaveActiveBallot((exec_lo & tb_lane_bit) != 0u && {c}).x");
            return self.ws(if m.starts_with("v_cmpx_") { &Opnd::ExecLo } else { &d }, &vote);
        }
        if m.starts_with("v_cmpx_") {
            self.line(&format!("exec_lo = (((exec_lo & 1u) != 0u) && {c}) ? 1u : 0u; exec_hi = 0u;"));
            return Ok(());
        }
        if self.packed.as_ref().is_some_and(|p| p.ballots.contains(&self.cur_idx)) {
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint4 t{t}ballot = WaveActiveBallot(((exec_lo & 1u) != 0u) && {c});"));
            self.ws(&d, &format!("t{t}ballot.x"))?;
            return self.ws(&hi(&d), &format!("t{t}ballot.y"));
        }
        self.ws(&d, &format!("(((exec_lo & 1u) != 0u) && {c}) ? 1u : 0u"))?;
        if self.one_word_vector_mask() { return Ok(()); } // The next SGPR can hold an independent mask.
        let dh = hi(&d);
        if !matches!(dh, Opnd::Null | Opnd::ConstInt(_)) {
            self.touch(&dh);
            self.ws(&dh, "0u")?;
        }
        return Ok(());
    }

    /// Operations on bytes and halves of registers.
    pub(in crate::hlsl) fn emit_sdwa(&mut self, m: String, d: Opnd, ins: &Insn, sd: u32, half_binary: bool, unary_op: Option<u8>, half_int_src: bool, half_int_result: bool) -> Result<(), String> {
        // SDWA: operate on a selected byte/word of each source. Materialise the selected
        // values into temporaries, then translate the instruction as usual.
        let is_cmp = ins.enc == Enc::Vopc;
        let dst_sel = if is_cmp { 6 } else { (sd >> 8) & 7 };
        if dst_sel == 7 {
            return Err("SDWA destination select 7 is reserved".into());
        }
        let mut c = ins.clone();
        c.aux.sdwa = None;
        if clamped_product24(ins) {
            if sd & 0xc000 != 0 || (sd >> 11) & 3 == 3 {
                return Err("unsupported SDWA destination control for 24-bit multiply".into());
            }
            c.aux.imm = 1; // Saturate the full result before packing the destination.
        }
        // Reuse VOP3 floating modifiers after source selection and before destination packing.
        let half_alu = half_binary || half_int_result || unary_op.is_some() || matches!(m.as_str(), "v_rcp_f16" | "v_exp_f16" | "v_cvt_f32_f16");
        let float_alu = half_alu || matches!(m.as_str(),
            "v_add_f32" | "v_sub_f32" | "v_subrev_f32" | "v_mul_f32" | "v_mul_legacy_f32"
            | "v_min_f32" | "v_max_f32" | "v_fract_f32" | "v_trunc_f32" | "v_ceil_f32"
            | "v_rndne_f32" | "v_floor_f32");
        // Integer conversions accept floating output modifiers but keep integer source rules.
        let int_to_float = matches!(m.as_str(),
            "v_cvt_f32_i32" | "v_cvt_f32_u32" | "v_cvt_off_f32_i4" | "v_cvt_f32_ubyte0"
            | "v_cvt_f32_ubyte1" | "v_cvt_f32_ubyte2" | "v_cvt_f32_ubyte3");
        let float_source_modifiers = float_alu || m == "v_cvt_f16_f32";
        if float_source_modifiers || int_to_float || half_int_src {
            c.enc = Enc::Vop3;
            c.aux.imm = i64::from((sd >> 13) & 1);
            c.aux.imm2 = i64::from((sd >> 14) & 3);
        }
        if sd & 0x3030_0000 != 0 {
            if !(float_source_modifiers || (is_cmp && (m.ends_with("_f16") || m.ends_with("_f32"))))
                || (m.contains("_class_") && sd & 0x3000_0000 != 0)
            {
                return Err("unsupported SDWA neg/abs for this operand type".into());
            }
            // SDWAB Table 89: convert the independent source fields into the
            // same ABS[1:0], NEG[4:3] mask consumed by the VOP3 comparator.
            for (i, shift) in [(0, 16), (1, 24)] {
                c.aux.mask |= (((sd >> (shift + 5)) & 1) as u8) << i;
                c.aux.mask |= (((sd >> (shift + 4)) & 1) as u8) << (i + 3);
            }
        }
        for (i, shift) in [(0usize, 16u32), (1usize, 24u32)] {
            if i >= c.src.len() {
                continue;
            }
            let sel = (sd >> shift) & 7;
            let sext = (sd >> (shift + 3)) & 1 != 0;
            if sel == 6 {
                continue;
            }
            let mut x = r(&c.src[i])?;
            let half_constant = half_int_src || ((is_cmp || half_alu) && m.ends_with("_f16")
                && (i == 0 || !m.contains("_class_")));
            if half_constant && matches!(c.src[i], Opnd::ConstF32(_))
            {
                // The inline constant ROM supplies binary16 before byte/word selection.
                x = format!("tb_f16_rne(asfloat({x}))");
            }
            let (bits, lsb) = match sel {
                0..=3 => (8u32, sel * 8),
                4 | 5 => (16, (sel - 4) * 16),
                _ => return Err(format!("SDWA select {sel} is reserved")),
            };
            let e = if sext {
                format!("asuint(asint({x} << {}) >> {})", 32 - bits - lsb, 32 - bits)
            } else {
                format!("(({x} >> {lsb}) & {:#x}u)", (1u32 << bits) - 1)
            };
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint tb_s{t} = {e};"));
            c.src[i] = Opnd::Temp(t as u16);
        }
        if dst_sel == 6 && !half_round_or_conversion(&m) && m != "v_cvt_f16_f32" {
            return self.emit_vec(&c);
        }
        // Partial destination: compute into a temporary, then place the selected
        // byte/word into the register. DST_UNUSED (bits 12:11): 0 = pad with zeros,
        // 1 = sign-extend upwards, 2 = keep the other bits of the destination.
        let t = self.tmp;
        self.tmp += 1;
        self.line(&format!("uint tb_s{t} = 0u;"));
        c.dst = vec![Opnd::Temp(t as u16)];
        self.emit_vec(&c)?;
        if dst_sel == 6 {
            // SDWA DWORD writes the entire zero-extended 16-bit result.
            return self.wv(&d, &format!("tb_s{t}"));
        }
        let (bits, lsb) = if dst_sel <= 3 { (8u32, dst_sel * 8) } else { (16, (dst_sel - 4) * 16) };
        let mask = (1u32 << bits) - 1;
        let val = format!("(tb_s{t} & {mask:#x}u)");
        let e = match (sd >> 11) & 3 {
            0 => format!("({val} << {lsb})"),
            1 => format!("asuint(asint({val} << {}) >> {})", 32 - bits, 32 - bits - lsb),
            2 => {
                self.touch(&d);
                format!("((({}) & {:#x}u) | ({val} << {lsb}))", r(&d)?, !(mask << lsb))
            }
            _ => return Err("SDWA DST_UNUSED 3 is reserved".into()),
        };
        return self.wv(&d, &e);
    }

    /// Multiply two words and add a 64-bit value.
    pub(in crate::hlsl) fn emit_mad64(&mut self, m: String, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        let signed = m == "v_mad_i64_i32";
        let Opnd::Vgpr(low) = d else { return Err("expected VGPR pair".into()); };
        let high = Opnd::Vgpr(low.checked_add(1).ok_or("VGPR pair overflows")?);
        // LLVM writes the carry to the null register when it is unused.
        let cout = match (ins.raw[0] >> 8) & 127 {
            n @ 0..=104 if !signed => Opnd::Sgpr(n as u8),
            106 if !signed => Opnd::VccLo,
            125 => Opnd::Null,
            _ => return Err("unsupported multiply-add carry destination".into()),
        };
        if signed && (self.ex.stage != Stage::Compute
            || !ins.src[..3].iter().all(|s| matches!(s, Opnd::Vgpr(_)))) {
            return Err("unverified signed 64-bit multiply-add source or stage".into());
        }
        if ins.raw[0] & 0x8000 != 0 || ins.raw[1] >> 27 != 0 {
            return Err("unsupported multiply-add modifiers".into());
        }
        let (a, b, c, ch) = (r(&s[0])?, r(&s[1])?, r(&s[2])?, self.pair_high(&s[2])?);
        let t = self.tmp;
        self.tmp += 1;
        // The low product word is the same for signed and unsigned inputs;
        // the signed high word comes from the signed 32x32 product.
        let mulhi = if signed { "tb_mulhi_i32" } else { "tb_mulhi_u32" };
        self.line(&format!("uint t{t}lo = {a} * {b} + {c}; uint t{t}sum = {mulhi}({a}, {b}) + {ch};"));
        self.line(&format!("uint t{t}hi = t{t}sum + uint(t{t}lo < {c}); uint t{t}co = uint(t{t}sum < {ch} || t{t}hi < t{t}sum);"));
        self.wv(&d, &format!("t{t}lo"))?;
        self.touch(&high);
        self.wv(&high, &format!("t{t}hi"))?;
        if cout == Opnd::Null {
            return Ok(());
        }
        if self.resolve_wave32 {
            return self.ws(&cout, &format!("WaveActiveBallot((exec_lo & tb_lane_bit) != 0u && t{t}co != 0u).x"));
        }
        self.ws(&cout, &format!("(exec_lo & 1u) * t{t}co"))?;
        if self.one_word_vector_mask() { return Ok(()); }
        return self.ws(&hi(&cout), "0u");
    }

    /// Dot product of four signed bytes.
    pub(in crate::hlsl) fn emit_dot4(&mut self, s: &[Opnd], d: Opnd, ins: &Insn) -> Result<(), String> {
        let compact = ins.enc == Enc::Vop2 && ins.aux.sdwa.is_none();
        let mut rhs = r(&s[1])?;
        let acc = if compact {
            &d
        } else if ins.op == 0x416 {
            // OP_SEL and OP_SEL_HI choose source 1's half for each dot
            // pair. These three combinations were observed in FSPX.
            rhs = match (ins.raw[0] & 0xff00, ins.raw[1] >> 27) {
                (0x4000, 3) => rhs,
                (0x5000, 1) => format!("(({rhs} >> 16u) | ({rhs} << 16u))"),
                (0x4000, 1) => format!("(({rhs} & 65535u) * 0x10001u)"),
                _ => return Err("unsupported signed byte dot product modifiers".into()),
            };
            &s[2]
        } else {
            return Err("unsupported signed byte dot product encoding".into());
        };
        let lhs = r(&s[0])?;
        let acc = r(acc)?;
        let value = if self.fspx_wave.is_some() {
            format!("asuint(dot4add_i8packed({lhs}, {rhs}, asint({acc})))")
        } else {
            format!("tb_dot4_i8({lhs}, {rhs}, {acc})")
        };
        return self.wv(&d, &value);
    }
}
