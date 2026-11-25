//! Scalar instructions: control, integer and bit operations.
use super::*;

impl<'a> Emitter<'a> {
    /// Program control: branches, barriers, messages and mode changes.
    pub(in crate::hlsl) fn emit_sopp(&mut self, idx: usize, ins: &Insn, br: Option<usize>) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let d = ins.dst.first().cloned();
        match ins.enc {
            Enc::Sopp if matches!(ins.op, 0x24 | 0x25) => {
                // MODE[7:0] exposes the guest round and denorm settings to S_GETREG.
                // V_CVT_F16_F32 also consumes these settings.
                let shift = if ins.op == 0x24 { 0 } else { 4 };
                self.regs.insert("tb_fp_mode".into());
                self.line(&format!("tb_fp_mode = (tb_fp_mode & ~{:#x}u) | {:#x}u;", 15 << shift, (ins.raw[0] & 15) << shift));
                Ok(())
            }
            Enc::Sopp if ins.op == 0x10 => self.emit_sendmsg(ins),
            Enc::Sopp if ins.op == 0x0a && self.is_cs => {
                if self.packed.as_ref().is_some_and(|p| p.device_sync_barriers.contains(&idx)) {
                    self.line("AllMemoryBarrierWithGroupSync();");
                } else {
                    self.line("GroupMemoryBarrierWithGroupSync();");
                }
                Ok(())
            }
            Enc::Sopp => {
                if m == "s_swappc_b64" {
                    let link = d.ok_or("missing local PC link register")?;
                    self.ws(&link, &format!("{:#x}u", ins.offset + 4))?;
                    self.ws(&hi(&link), "0u")?;
                }
                let cond = match ins.op {
                    2 => Some("true".to_string()),
                    4 => Some("scc == 0u".into()),
                    5 => Some("scc != 0u".into()),
                    6 => Some("(vcc_lo & 1u) == 0u".into()),
                    7 => Some("(vcc_lo & 1u) != 0u".into()),
                    // The recorded guard reaches every invocation before the
                    // full-mask reduction, including guest-inactive lanes.
                    8 if self.reduction_execz_guards.contains(&idx) =>
                        Some("!WaveActiveAnyTrue((exec_lo & 1u) != 0u)".into()),
                    8 => Some("(exec_lo & 1u) == 0u".into()),
                    9 => Some("(exec_lo & 1u) != 0u".into()),
                    // A non-final s_endpgm ends the program on this path: jump to the end label.
                    1 if br.is_some() => Some("true".to_string()),
                    0 | 1 | 0x0a | 0x0c | 0x1f | 0x20 | 0x21 | 0x23 | 0x24 | 0x25 => None,
                    _ => return Err("unsupported SOPP".into()),
                };
                if let (Some(c), Some(t)) = (cond, br) {
                    let c = if self.resolve_wave32 || self.lockstep.is_some() {
                        wave_cull::branch_condition(self, ins, &c)
                    } else { c };
                    self.line(&format!("if ({c}) j{t} = true;"));
                }
                Ok(())
            }
            _ => Err("unsupported encoding".into()),
        }
    }

    /// Scalar operations on two sources.
    pub(in crate::hlsl) fn emit_sop2(&mut self, idx: usize, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        let d = d.ok_or("no dst")?;
        let (a, b) = (r(&s[0])?, r(&s[1])?);
        if matches!(m, "s_lshr_b64" | "s_ashr_i64" | "s_bfe_u64" | "s_bfe_i64") {
            self.touch(&hi(&s[0]));
            let upper = r(&hi(&s[0]))?;
            let t = self.tmp;
            self.tmp += 1;
            let signed = m.ends_with("i64");
            let call = if m.starts_with("s_bfe") { "tb_field64" } else { "tb_shr64" };
            self.line(&format!("uint2 t{t} = {call}({a}, {upper}, {b}, {signed}); scc = any(t{t} != 0u);"));
            self.ws(&d, &format!("t{t}.x"))?;
            return self.ws(&hi(&d), &format!("t{t}.y"));
        }
        if m == "s_bfm_b64" || m == "s_cselect_b64" || m == "s_lshl_b64" {
            let t = self.tmp;
            self.tmp += 1;
            let (lo, hi_e) = if m == "s_lshl_b64" {
                self.touch(&hi(&s[0]));
                let ah = r(&hi(&s[0]))?;
                (
                    format!("((({b} & 63u) >= 32u) ? 0u : ({a} << ({b} & 31u)))"),
                    format!(
                        "((({b} & 63u) == 0u) ? {ah} : ((({b} & 63u) >= 32u) ? ({a} << (({b} & 63u) - 32u)) : (({ah} << ({b} & 31u)) | ({a} >> (32u - ({b} & 31u))))))"
                    ),
                )
            } else if m == "s_bfm_b64" {
                // Intersect [start, end) with each 32-bit half of the destination.
                self.line(&format!("uint t{t}start = {b} & 63u, t{t}end = t{t}start + ({a} & 63u);"));
                let word_mask = |base: u32| {
                    let start = low_mask(&format!("max(t{t}start, {base}u) - {base}u"));
                    let end = low_mask(&format!("max(t{t}end, {base}u) - {base}u"));
                    format!("({end} & ~{start})")
                };
                (word_mask(0), word_mask(32))
            } else {
                self.touch(&hi(&s[0]));
                self.touch(&hi(&s[1]));
                (
                    format!("((scc != 0u) ? {a} : {b})"),
                    format!("((scc != 0u) ? {} : {})", r(&hi(&s[0]))?, r(&hi(&s[1]))?),
                )
            };
            self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {hi_e};"));
            if m == "s_lshl_b64" {
                self.line(&format!("scc = ((t{t}l | t{t}h) != 0u) ? 1u : 0u;"));
            }
            self.ws(&d, &format!("t{t}l"))?;
            let dh = hi(&d);
            self.touch(&dh);
            return self.ws(&dh, &format!("t{t}h"));
        }
        let is64 = m.ends_with("_b64");
        let bit = |op: &str, a: &str, b: &str| -> Option<String> {
            Some(match op {
                "and" => format!("({a} & {b})"),
                "or" => format!("({a} | {b})"),
                "xor" => format!("({a} ^ {b})"),
                "andn2" => format!("({a} & ~{b})"),
                "orn2" => format!("({a} | ~{b})"),
                "nand" => format!("(~({a} & {b}))"),
                "nor" => format!("(~({a} | {b}))"),
                "xnor" => format!("(~({a} ^ {b}))"),
                _ => return None,
            })
        };
        let opname = m.trim_start_matches("s_").trim_end_matches("_b32").trim_end_matches("_b64");
        if let Some(lo) = bit(opname, &a, &b) {
            if is64 {
                self.touch(&hi(&s[0]));
                self.touch(&hi(&s[1]));
                let (ah, bh) = (r(&hi(&s[0]))?, r(&hi(&s[1]))?);
                let hi_e = bit(opname, &ah, &bh).unwrap();
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {hi_e};"));
                self.ws(&d, &format!("t{t}l"))?;
                let dh = hi(&d);
                self.touch(&dh);
                self.ws(&dh, &format!("t{t}h"))?;
                self.line(&format!("scc = ((t{t}l | t{t}h) != 0u) ? 1u : 0u;"));
                if is_ps && m == "s_andn2_b64" && pixel_quad::guards_expansion(&self.insns, idx) {
                    self.line("#if __SHADER_TARGET_MAJOR >= 6");
                    self.line(&pixel_quad::vote(t, &format!("t{t}l")));
                    self.line(&format!("scc = q{t} | (uint)WaveActiveAnyTrue(q{t} != 0u);"));
                    self.line("#endif");
                }
            } else {
                self.ws(&d, &lo)?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
            }
            return Ok(());
        }
        let e = match m {
            "s_add_u32" | "s_add_i32" => format!("({a} + {b})"),
            "s_sub_u32" | "s_sub_i32" => format!("({a} - {b})"),
            "s_addc_u32" => format!("({a} + {b} + (scc & 1u))"),
            "s_subb_u32" => format!("({a} - {b} - (scc & 1u))"),
            "s_mul_i32" => format!("({a} * {b})"),
            // The subtraction wraps before taking the absolute value, including INT_MIN.
            "s_absdiff_i32" => format!("(asint({a} - {b}) < 0 ? ({b} - {a}) : ({a} - {b}))"),
            "s_mul_hi_u32" => format!("tb_mulhi_u32({a}, {b})"),
            "s_mul_hi_i32" => format!("tb_mulhi_i32({a}, {b})"),
            "s_lshl_b32" => format!("({a} << ({b} & 31u))"),
            "s_lshr_b32" => format!("({a} >> ({b} & 31u))"),
            "s_ashr_i32" => format!("asuint(asint({a}) >> ({b} & 31u))"),
            "s_min_u32" => format!("min({a}, {b})"),
            "s_max_u32" => format!("max({a}, {b})"),
            "s_min_i32" => format!("asuint(min(asint({a}), asint({b})))"),
            "s_max_i32" => format!("asuint(max(asint({a}), asint({b})))"),
            "s_cselect_b32" => format!("((scc != 0u) ? {a} : {b})"),
            "s_bfe_u32" => format!("(({a} >> ({b} & 31u)) & {})", low_mask(&format!("({b} >> 16) & 127u"))),
            "s_bfe_i32" => format!("tb_bfe_i32({a}, {b})"),
            "s_bfm_b32" => format!("(((1u << ({a} & 31u)) - 1u) << ({b} & 31u))"),
            "s_pack_ll_b32_b16" => format!("(({a} & 65535u) | ({b} << 16))"),
            "s_pack_lh_b32_b16" => format!("(({a} & 65535u) | ({b} & 0xffff0000u))"),
            "s_pack_hh_b32_b16" => format!("(({a} >> 16) | ({b} & 0xffff0000u))"),
            "s_lshl1_add_u32" => format!("(({a} << 1) + {b})"),
            "s_lshl2_add_u32" => format!("(({a} << 2) + {b})"),
            "s_lshl3_add_u32" => format!("(({a} << 3) + {b})"),
            "s_lshl4_add_u32" => format!("(({a} << 4) + {b})"),
            _ => return Err("unsupported SOP2".into()),
        };
        let t = self.tmp;
        self.tmp += 1;
        let result = format!("t{t}");
        self.line(&format!("uint {result} = {e};"));
        // SCC may depend on both original sources even when SDST aliases one of them.
        let scc = match m {
            "s_add_i32" => Some(signed_add_overflow(&a, &b, &result)),
            "s_sub_i32" => Some(format!("((({a} ^ {b}) & ({a} ^ {result})) >> 31)")),
            // The carry is of the full sum, so bits the shift moves past
            // bit 31 set it as well as a wrapped addition.
            "s_lshl1_add_u32" => Some(shift_add_carry(&a, &b, &result, 1)),
            "s_lshl2_add_u32" => Some(shift_add_carry(&a, &b, &result, 2)),
            "s_lshl3_add_u32" => Some(shift_add_carry(&a, &b, &result, 3)),
            "s_lshl4_add_u32" => Some(shift_add_carry(&a, &b, &result, 4)),
            "s_add_u32" => Some(format!("(({a} + {b}) < {a}) ? 1u : 0u")),
            "s_addc_u32" => Some(format!("((({a} + {b}) < {a}) || (({a} + {b} + (scc & 1u)) < ({a} + {b}))) ? 1u : 0u")),
            "s_subb_u32" => Some(format!("(({b} > {a}) || ((scc & 1u) > ({a} - {b}))) ? 1u : 0u")),
            "s_sub_u32" => Some(format!("({b} > {a}) ? 1u : 0u")),
            "s_min_i32" => Some(format!("(asint({a}) < asint({b})) ? 1u : 0u")),
            "s_max_i32" => Some(format!("(asint({a}) > asint({b})) ? 1u : 0u")),
            "s_min_u32" => Some(format!("({a} < {b}) ? 1u : 0u")),
            "s_max_u32" => Some(format!("({a} > {b}) ? 1u : 0u")),
            "s_lshl_b32" | "s_lshr_b32" | "s_ashr_i32" | "s_bfe_u32" | "s_bfe_i32" | "s_absdiff_i32" => Some(format!("({result} != 0u) ? 1u : 0u")),
            _ => None,
        };
        if let Some(flag) = scc {
            self.line(&format!("scc = {flag};"));
        }
        self.ws(&d, &result)
    }

    /// Scalar operations on one source.
    pub(in crate::hlsl) fn emit_sop1(&mut self, idx: usize, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        let d = d.ok_or("no dst")?;
        let a = r(&s[0])?;
        match m {
            "s_mov_b32" => self.ws(&d, &a),
            GROUP_READ => {
                if self.hoisted_swizzle != Some(idx) { self.emit_compute_swizzle(idx, ins, &[])?; }
                self.ws(&d, &format!("swizzle{idx}"))
            }
            ray_resolve::READ_LANE => {
                let lane = r(&s[1])?;
                self.ws(&d, &format!("WaveReadLaneAt({a}, ({lane}) & 31u)"))
            }
            // D[S0[4:0]] = 0 / 1
            "s_bitset0_b32" => {
                self.touch(&d);
                let cur = r(&d)?;
                self.ws(&d, &format!("({cur} & ~(1u << (({a}) & 31u)))"))
            }
            "s_bitset1_b32" => {
                self.touch(&d);
                let cur = r(&d)?;
                self.ws(&d, &format!("({cur} | (1u << (({a}) & 31u)))"))
            }
            "s_wqm_b32" | "s_wqm_b64" => {
                // For an ordinary scalar mask, replicate each nonempty nibble.
                // EXEC instead holds one guest bit per invocation.
                let t = self.tmp;
                self.tmp += 1;
                let wide = m.ends_with("_b64");
                let lane_mask = is_ps && pixel_quad::saved_exec(&self.insns, idx);
                if s[0] == Opnd::ExecLo && lone_lane::on() {
                    self.line(&format!("uint t{t}l = exec_lo & 1u, t{t}h = 0u;"));
                } else if s[0] == Opnd::ExecLo && !self.resolve_wave32 {
                    self.line("#if __SHADER_TARGET_MAJOR >= 6");
                    self.line(&format!("uint4 t{t}ballot = WaveActiveBallot((exec_lo & 1u) != 0u);"));
                    self.line(&format!("uint t{t}lane = WaveGetLaneIndex(); uint t{t}l = ((t{t}ballot[t{t}lane >> 5] >> (t{t}lane & 28u)) & 15u) != 0u;"));
                    self.line("#else");
                    self.line(&format!("uint4 t{t}ballot = uint4(exec_lo,0,0,0); uint t{t}l = exec_lo & 1u;"));
                    self.line("#endif");
                    self.line(&format!("uint t{t}h = 0u;"));
                } else if lane_mask {
                    self.line("#if __SHADER_TARGET_MAJOR >= 6");
                    self.line(&pixel_quad::vote(t, &a));
                    self.line(&format!("uint t{t}l = q{t}, t{t}h = 0u;"));
                    self.line(&format!("scc = q{t} | (uint)WaveActiveAnyTrue(q{t} != 0u);"));
                    self.line("#else");
                    self.line(&format!("uint t{t}l = {a} & 1u, t{t}h = 0u; scc = t{t}l;"));
                    self.line("#endif");
                } else {
                    let expand = |word: &str| format!("((({word} | ({word} >> 1) | ({word} >> 2) | ({word} >> 3)) & 0x11111111u) * 15u)");
                    self.line(&format!("uint t{t}l = {};", expand(&a)));
                    let upper = if wide {
                        self.touch(&hi(&s[0]));
                        expand(&r(&hi(&s[0]))?)
                    } else { "0u".into() };
                    self.line(&format!("uint t{t}h = {upper};"));
                }
                if s[0] == Opnd::ExecLo && !self.resolve_wave32 && !lone_lane::on() {
                    self.line(&format!("scc = any(t{t}ballot != 0u);"));
                } else if !lane_mask {
                    self.line(&format!("scc = ((t{t}l | t{t}h) != 0u);"));
                }
                self.ws(&d, &format!("t{t}l"))?;
                if wide { self.ws(&hi(&d), &format!("t{t}h"))?; }
                Ok(())
            }
            "s_mov_b64" | "s_cmov_b64" | "s_not_b64" => {
                self.touch(&hi(&s[0]));
                let ah = r(&hi(&s[0]))?;
                let t = self.tmp;
                self.tmp += 1;
                let (lo, upper) = match m {
                    "s_not_b64" => (format!("~{a}"), format!("~{ah}")),
                    "s_cmov_b64" => (format!("scc ? {a} : {}", r(&d)?), format!("scc ? {ah} : {}", r(&hi(&d))?)),
                    _ => (a, ah),
                };
                self.line(&format!("uint t{t}l = {lo}; uint t{t}h = {upper};"));
                if m == "s_not_b64" { self.line(&format!("scc = ((t{t}l | t{t}h) != 0u);")); }
                self.ws(&d, &format!("t{t}l"))?;
                let dh = hi(&d);
                self.touch(&dh);
                self.ws(&dh, &format!("t{t}h"))
            }
            "s_not_b32" => {
                self.ws(&d, &format!("(~{a})"))?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                Ok(())
            }
            "s_cmov_b32" => self.ws(&d, &format!("((scc != 0u) ? {a} : {})", name(&d)?)),
            "s_brev_b32" => self.ws(&d, &format!("reversebits({a})")),
            COUNT_LANES => {
                let def = *self.vote_uses.get(&idx).ok_or("lane count without its predicate")?;
                self.ws(&d, &format!("tb_votes{def}"))?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                Ok(())
            }
            "s_bcnt1_i32_b64" => {
                self.touch(&hi(&s[0]));
                // Population count of a 64-bit value — typically the number of active
                // lanes (wave-level allocation). One invocation = one lane, so a lane
                // mask counts as 0 or 1.
                let ah = r(&hi(&s[0]))?;
                self.ws(&d, &format!("(countbits({a}) + countbits({ah}))"))?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                Ok(())
            }
            "s_bcnt1_i32_b32" => {
                self.ws(&d, &format!("countbits({a})"))?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                Ok(())
            }
            "s_flbit_i32_b32" => self.ws(&d, &format!("({a} == 0u ? ~0u : 31u - firstbithigh({a}))")),
            "s_flbit_i32_b64" => {
                self.touch(&hi(&s[0]));
                let ah = r(&hi(&s[0]))?;
                self.ws(&d, &flbit_b64_expr(&a, &ah))
            }
            "s_ff0_i32_b32" => self.ws(&d, &ff0_b32_expr(&a)),
            "s_ff0_i32_b64" => {
                self.touch(&hi(&s[0]));
                let ah = r(&hi(&s[0]))?;
                self.ws(&d, &ff0_b64_expr(&a, &ah))
            }
            "s_ff1_i32_b32" => self.ws(&d, &format!("firstbitlow({a})")),
            // s_ff1_i32_b64 is firstbitlow over the 64-bit value: the low word
            // decides when it is nonzero, otherwise the high word.
            "s_ff1_i32_b64" => {
                self.touch(&hi(&s[0]));
                let ah = r(&hi(&s[0]))?;
                self.ws(&d, &ff1_b64_expr(&a, &ah))
            }
            "s_sext_i32_i8" => self.ws(&d, &format!("asuint(asint({a} << 24) >> 24)")),
            "s_sext_i32_i16" => self.ws(&d, &format!("asuint(asint({a} << 16) >> 16)")),
            "s_abs_i32" => {
                self.ws(&d, &format!("asuint(abs(asint({a})))"))?;
                self.line(&format!("scc = ({} != 0u) ? 1u : 0u;", name(&d)?));
                Ok(())
            }
            "s_getpc_b64" => {
                // Address of the next instruction, relative to the program start.
                self.ws(&d, &format!("{:#x}u", ins.offset as u32 + 4))?;
                let dh = hi(&d);
                self.touch(&dh);
                self.ws(&dh, "0u")
            }
            "s_swappc_b64" if self.ex.stage == Stage::Vertex => {
                // Vertex-fetch shader call: inputs are materialised from the D3D input layout.
                self.line("// (fetch-shader call: vertex attributes come from the input layout)");
                Ok(())
            }
            _ if m.contains("_saveexec_") || m.contains("_wrexec_") => {
                let wide = m.ends_with("_b64");
                let combine = |src: &str, exec: &str| -> Result<String, String> {
                    Ok(match m.split('_').nth(1) {
                        Some("and") => format!("{src} & {exec}"),
                        Some("or") => format!("{src} | {exec}"),
                        Some("xor") => format!("{src} ^ {exec}"),
                        Some("nand") => format!("~({src} & {exec})"),
                        Some("andn2") => format!("{src} & ~{exec}"),
                        Some("orn2") => format!("{src} | ~{exec}"),
                        Some("andn1") => format!("~{src} & {exec}"),
                        _ => return Err("unsupported saveexec".into()),
                    })
                };
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t}l = {};", combine(&a, "exec_lo")?));
                if wide {
                    self.touch(&hi(&s[0]));
                    self.line(&format!("uint t{t}h = {};", combine(&r(&hi(&s[0]))?, "exec_hi")?));
                }
                let (lo, high) = if m.contains("_wrexec_") {
                    (format!("t{t}l"), format!("t{t}h"))
                } else {
                    ("exec_lo".into(), "exec_hi".into())
                };
                self.ws(&d, &lo)?;
                if wide {
                    self.ws(&hi(&d), &high)?;
                    self.line(&format!("exec_hi = t{t}h;"));
                }
                let mask = if wide { "(exec_lo | exec_hi)" } else { "exec_lo" };
                self.line(&format!("exec_lo = t{t}l; scc = ({mask} != 0u);"));
                Ok(())
            }
            // Return from a vertex shader chained in front of a geometry shader: the
            // program ends here (it is followed directly by s_endpgm).
            "s_setpc_b64" if self.ex.stage == Stage::Vertex && idx + 2 >= self.insns.len() => {
                self.line("// (return to the chained stage)");
                Ok(())
            }
            _ => Err("unsupported SOP1".into()),
        }
    }

    /// Scalar operations with an immediate.
    pub(in crate::hlsl) fn emit_sopk(&mut self, ins: &Insn) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let d = ins.dst.first().cloned();
        let d = d.ok_or("no dst")?;
        let imm = format!("{:#x}u", ins.aux.imm as i32 as u32);
        let mask = modeled_hwreg_mask(ins.raw[0]);
        match m {
            "s_version" => Ok(()),
            "s_movk_i32" => self.ws(&d, &imm),
            "s_cmovk_i32" => self.ws(&d, &format!("((scc != 0u) ? {imm} : {})", name(&d)?)),
            "s_setreg_imm32_b32" | "s_setreg_b32" | "s_getreg_b32"
                if mask != 0 => {
                let id = ins.raw[0] & 63;
                let shift = (ins.raw[0] >> 6) & 31;
                let width = ((ins.raw[0] >> 11) & 31) + 1;
                if shift + width > 32 { return Err("hardware register field exceeds word".into()); }
                // Reads must be known in full. Writes need only update the
                // intersection with the modeled register.
                if m == "s_getreg_b32" && mask != u32::MAX >> (32 - width) {
                    return Err("unmodeled MODE read field".into());
                }
                let reg = if id == 1 { "tb_fp_mode".to_string() } else { format!("tb_flat_scratch{}", id & 1) };
                self.regs.insert(reg.clone());
                if m == "s_getreg_b32" {
                    return self.ws(&d, &format!("(({reg} >> {shift}) & {mask:#x}u)"));
                }
                let value = if m == "s_setreg_imm32_b32" {
                    format!("{:#x}u", ins.raw.get(1).ok_or("missing hardware-register literal")?)
                } else {
                    self.touch(&d);
                    r(&d)?
                };
                self.line(&format!("{reg} = ({reg} & ~{:#x}u) | (({value} & {mask:#x}u) << {shift});", mask << shift));
                Ok(())
            }
            // Other hardware-register writes and counter waits do not change the modeled state.
            "s_setreg_imm32_b32" | "s_setreg_b32" | "s_waitcnt_vscnt" | "s_waitcnt_vmcnt"
            | "s_waitcnt_expcnt" | "s_waitcnt_lgkmcnt" => Ok(()),
            "s_addk_i32" => {
                let a = name(&d)?;
                let sum = format!("t{}", self.tmp);
                self.tmp += 1;
                self.line(&format!("uint {sum} = {a} + {imm};"));
                self.line(&format!("scc = {};", signed_add_overflow(&a, &imm, &sum)));
                self.ws(&d, &sum)
            }
            "s_mulk_i32" => self.ws(&d, &format!("({} * {imm})", name(&d)?)),
            _ if m.starts_with("s_cmpk_") => {
                let fake = m.replace("s_cmpk_", "s_cmp_");
                let c = self.int_compare(&fake, &d, &Opnd::ConstInt(ins.aux.imm))?;
                self.line(&format!("scc = {c} ? 1u : 0u;"));
                Ok(())
            }
            _ => Err("unsupported SOPK".into()),
        }
    }

    /// Scalar compares.
    pub(in crate::hlsl) fn emit_sopc(&mut self, ins: &Insn) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let c = match m {
            "s_bitcmp0_b32" => format!("((({} >> ({} & 31u)) & 1u) == 0u)", r(&s[0])?, r(&s[1])?),
            "s_bitcmp1_b32" => format!("((({} >> ({} & 31u)) & 1u) != 0u)", r(&s[0])?, r(&s[1])?),
            "s_cmp_eq_u64" => format!(
                "({} == {} && {} == {})",
                r(&s[0])?,
                r(&s[1])?,
                r(&hi(&s[0]))?,
                r(&hi(&s[1]))?
            ),
            "s_cmp_lg_u64" => format!(
                "({} != {} || {} != {})",
                r(&s[0])?,
                r(&s[1])?,
                r(&hi(&s[0]))?,
                r(&hi(&s[1]))?
            ),
            _ => self.int_compare(m, &s[0], &s[1])?,
        };
        self.line(&format!("scc = {c} ? 1u : 0u;"));
        Ok(())
    }
}
