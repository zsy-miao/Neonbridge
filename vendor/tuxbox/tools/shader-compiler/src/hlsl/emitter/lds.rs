//! Group shared memory and the lane exchanges encoded with it.
use super::*;

impl<'a> Emitter<'a> {
    /// Group shared memory, ring buffer and lane exchange instructions.
    pub(in crate::hlsl) fn emit_ds(&mut self, idx: usize, ins: &Insn, is_ps: bool) -> Result<(), String> {
        let m = ins.mnemonic.as_str();
        let s = &ins.src;
        let d = ins.dst.first().cloned();
        match ins.enc {
            Enc::Ds
                if !self.is_gs
                    && !self.is_cs
                    && self.ex.stage == Stage::Vertex
                    && matches!(m, "ds_write_b32" | "ds_write2_b32" | "ds_write_b64" | "ds_write2_b64") =>
            {
                // A vertex shader chained in front of a geometry shader hands its outputs over
                // through LDS instead of parameter exports: the vertex record is the output
                // registers back to back, 4 dwords each (observed in a55400ccab602283: dwords
                // 0..3 = TEXCOORD6.xyzw, 4..6 = TEXCOORD8.xyz, matching the output signature).
                // The address register is this vertex's record base and is not needed.
                let off = ins.aux.imm as u32;
                let vg = |o: &Opnd, k: u8| match o {
                    Opnd::Vgpr(n) => Ok(Opnd::Vgpr(n + k)),
                    _ => Err("LDS output write with a non-VGPR source".to_string()),
                };
                // (byte offset, source register) per written dword.
                let writes: Vec<(u32, Opnd)> = match m {
                    "ds_write_b32" => vec![(off & 0xffff, s[1])],
                    "ds_write2_b32" => vec![((off & 0xff) * 4, s[1]), (((off >> 8) & 0xff) * 4, s[2])],
                    "ds_write_b64" => vec![(off & 0xffff, s[1]), ((off & 0xffff) + 4, vg(&s[1], 1)?)],
                    _ => vec![
                        ((off & 0xff) * 8, s[1]),
                        ((off & 0xff) * 8 + 4, vg(&s[1], 1)?),
                        (((off >> 8) & 0xff) * 8, s[2]),
                        (((off >> 8) & 0xff) * 8 + 4, vg(&s[2], 1)?),
                    ],
                };
                let osig = self.ex.output_sig.clone();
                for (byte, src) in writes {
                    let dw = (byte / 4) as usize;
                    // The record is indexed by output *register* (4 dwords each), system
                    // values included: in 008bccdcf9dcecf9 register 1 is SV_Position and is
                    // written at dwords 4..7.
                    let reg = (dw / 4) as u32;
                    let comp = dw % 4;
                    self.touch(&src);
                    let mut hit = false;
                    for (ei, e) in osig.iter().enumerate() {
                        if e.register == reg && (e.mask >> comp) & 1 != 0 {
                            let idx = (0..comp).filter(|c| (e.mask >> c) & 1 != 0).count();
                            let (ty, n) = sig_type(e);
                            let field = if n == 1 { format!("o.o{ei}") } else { format!("o.o{ei}.{}", XYZW[idx]) };
                            let val = if ty.starts_with("float") { f(&src)? } else if ty.starts_with("int") { format!("asint({})", r(&src)?) } else { r(&src)? };
                            self.line(&format!("if (exec_lo & 1u) {field} = {val};"));
                            hit = true;
                        }
                    }
                    if !hit {
                        self.line(&format!("// (LDS output dword {dw} has no output-signature component: ignored)"));
                    }
                }
                Ok(())
            }
            Enc::Ds if self.is_gs && matches!(m, "ds_read_b32" | "ds_read2_b32" | "ds_write_b32" | "ds_write2_b32") => {
                self.emit_gs_lds(ins)
            }
            Enc::Ds if self.is_cs && matches!(m, "ds_read_b64" | "ds_read2_b64" | "ds_read2st64_b64" | "ds_write_b128" | "ds_write_b64" | "ds_write2_b64") => {
                if ins.raw[0] & (1 << 17) != 0 { return Err("GDS is unsupported".into()); }
                self.uses_lds = true;
                let t = self.tmp;
                self.tmp += 1;
                // Snapshot the address before any destination word can overwrite its VGPR.
                self.line(&format!("uint t{t}addr = {};", r(&s[0])?));
                let offset = ins.aux.imm as u32;
                let read = m.starts_with("ds_read");
                let paired = matches!(m, "ds_read2_b64" | "ds_read2st64_b64" | "ds_write2_b64");
                let stride = if m == "ds_read2st64_b64" { 512 } else { 8 };
                let count = if matches!(m, "ds_read_b64" | "ds_write_b64") { 2 } else { 4 };
                for word in 0..count {
                    let (base, k) = if read {
                        (ins.dst[0], word)
                    } else if paired {
                        (s[1 + (word / 2) as usize], word % 2)
                    } else {
                        (s[1], word)
                    };
                    let Opnd::Vgpr(v) = base else { return Err("expected consecutive LDS VGPRs".into()) };
                    let reg = Opnd::Vgpr(v.checked_add(k).ok_or("LDS register span exceeds v255")?);
                    let byte = if paired {
                        ((offset >> (8 * (word / 2))) & 255) * stride + u32::from(word % 2) * 4
                    } else {
                        offset + u32::from(word) * 4
                    };
                    let location = format!("tb_lds[((t{t}addr + {byte}u) >> 2) & 8191u]");
                    if read {
                        self.wv(&reg, &location)?;
                    } else {
                        self.touch(&reg);
                        self.line(&format!("if (exec_lo & 1u) {location} = {};", r(&reg)?));
                    }
                }
                Ok(())
            }
            Enc::Ds
                if self.is_cs
                    && matches!(
                        m,
                        "ds_read_b32" | "ds_read2_b32" | "ds_read2st64_b32" | "ds_write_b32" | "ds_write2_b32" | "ds_write2st64_b32"
                    ) =>
            {
                // Compute: LDS is the thread group's shared memory → an HLSL `groupshared`
                // array addressed by byte address / 4 (32 KiB, the D3D groupshared limit).
                self.uses_lds = true;
                let off = ins.aux.imm as u32;
                // Two-address forms scale their 8-bit offsets by 4 bytes ("2") or by
                // 64 dwords = 256 bytes ("2st64"); express both in units of 4 bytes.
                let scale = if m.contains("st64") { 64 } else { 1 };
                let (off0, off1) = ((off & 0xff) * scale, ((off >> 8) & 0xff) * scale);
                let addr = r(&s[0])?;
                let at = |o: u32| format!("tb_lds[(({addr} + {o:#x}u) >> 2) & 8191u]");
                let vd = match d {
                    Some(Opnd::Vgpr(v)) => v,
                    _ => return Err("bad vdst".into()),
                };
                let t = self.tmp;
                self.tmp += 1;
                match m {
                    "ds_read_b32" => {
                        if self.wave_cull && matches!(self.ex.entry.as_deref(), Some("CullPerPageDrawCommandsCs" | "VirtualSmInvalidateInstancePagesCS" | "GenerateReflectionRaysCS")) {
                            self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) t{t} = {};", at(off & 0xffff)));
                        } else {
                            self.line(&format!("uint t{t} = {};", at(off & 0xffff)));
                        }
                        self.wv(&Opnd::Vgpr(vd), &format!("t{t}"))
                    }
                    "ds_read2_b32" | "ds_read2st64_b32" => {
                        if self.wave_cull && matches!(self.ex.entry.as_deref(), Some("CullPerPageDrawCommandsCs" | "VirtualSmInvalidateInstancePagesCS" | "GenerateReflectionRaysCS")) {
                            self.line(&format!("uint t{t}a = 0u, t{t}b = 0u; if (exec_lo & 1u) {{ t{t}a = {}; t{t}b = {}; }}", at(off0 * 4), at(off1 * 4)));
                        } else {
                            self.line(&format!("uint t{t}a = {}; uint t{t}b = {};", at(off0 * 4), at(off1 * 4)));
                        }
                        self.wv(&Opnd::Vgpr(vd), &format!("t{t}a"))?;
                        self.wv(&Opnd::Vgpr(vd + 1), &format!("t{t}b"))
                    }
                    "ds_write_b32" => {
                        self.line(&format!("if (exec_lo & 1u) {} = {};", at(off & 0xffff), r(&s[1])?));
                        Ok(())
                    }
                    _ => {
                        self.line(&format!(
                            "if (exec_lo & 1u) {{ {} = {}; {} = {}; }}",
                            at(off0 * 4),
                            r(&s[1])?,
                            at(off1 * 4),
                            r(&s[2])?
                        ));
                        Ok(())
                    }
                }
            }
            Enc::Ds if self.is_cs && matches!(m, "ds_write_b16" | "ds_write_b16_d16_hi" | "ds_write_b8" | "ds_read_u16" | "ds_read_i16" | "ds_read_u8" | "ds_read_i8" | "ds_read_u16_d16" | "ds_read_u16_d16_hi") => {
                // Sub-word accesses to group-shared memory: tb_lds is addressed in dwords, so
                // read-modify-write the containing dword.
                self.uses_lds = true;
                let off = ins.aux.imm as u32 & 0xffff;
                let bits = if m.contains("16") { 16u32 } else { 8 };
                let mask = (1u32 << bits) - 1;
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t}a = ({}) + {off:#x}u; uint t{t}s = (t{t}a & 3u) * 8u;", r(&s[0])?));
                let word = format!("tb_lds[(t{t}a >> 2) & 8191u]");
                if m.starts_with("ds_write") {
                    let mut data = r(&s[1])?;
                    if m == "ds_write_b16_d16_hi" {
                        if ins.raw[0] & (1 << 17) != 0 {
                            return Err("unsupported global data store halfword write".into());
                        }
                        data = format!("({data} >> 16)");
                    }
                    // Adjacent lanes can target different halves of the same word.
                    // A plain read-modify-write would lose one lane's update.
                    self.line(&format!("if (exec_lo & 1u) {{ uint t{t}old; InterlockedOr({word}, 0u, t{t}old);"));
                    self.line(&format!("uint t{t}seen; [allow_uav_condition] do {{ t{t}seen = t{t}old;"));
                    self.line(&format!("uint t{t}new = (t{t}seen & ~({mask:#x}u << t{t}s)) | ((({data}) & {mask:#x}u) << t{t}s);"));
                    self.line(&format!("InterlockedCompareExchange({word}, t{t}seen, t{t}new, t{t}old); }} while (t{t}old != t{t}seen); }}"));
                    return Ok(());
                }
                let v = format!("(({word} >> t{t}s) & {mask:#x}u)");
                let dst = d.ok_or("no dst")?;
                let e = match m {
                    "ds_read_u16_d16" => format!("(({} & 0xffff0000u) | {v})", r(&dst)?),
                    "ds_read_u16_d16_hi" => format!("(({} & 0xffffu) | ({v} << 16))", r(&dst)?),
                    _ if m.contains("_i") => format!("asuint(asint({v} << {}) >> {})", 32 - bits, 32 - bits),
                    _ => v,
                };
                return self.wv(&dst, &e);
            }
            Enc::Ds
                if self.is_cs
                    && (m == "ds_wrxchg_rtn_b32" || matches!(
                        m.replace("_rtn", "").as_str(),
                        "ds_add_u32" | "ds_min_u32" | "ds_max_u32" | "ds_and_b32" | "ds_or_b32" | "ds_xor_b32"
                    )) =>
            {
                // Atomic read-modify-write on group-shared memory (the `_rtn` forms also
                // return the old value) → the matching HLSL Interlocked* intrinsic.
                self.uses_lds = true;
                if m == "ds_wrxchg_rtn_b32" && ins.raw[0] & (1 << 17) != 0 {
                    return Err("global data store exchange unsupported".into());
                }
                if m == "ds_wrxchg_rtn_b32" && self.insns.iter().any(|op|
                    op.enc == Enc::Sopp && Self::branch_cond(op.op).is_some()) {
                    return Err("LDS exchange with unverified branch convergence".into());
                }
                let func = match m.replace("_rtn", "").as_str() {
                    "ds_add_u32" => "InterlockedAdd",
                    "ds_min_u32" => "InterlockedMin",
                    "ds_max_u32" => "InterlockedMax",
                    "ds_and_b32" => "InterlockedAnd",
                    "ds_or_b32" => "InterlockedOr",
                    "ds_wrxchg_b32" => "InterlockedExchange",
                    _ => "InterlockedXor",
                };
                let off = ins.aux.imm as u32 & 0xffff;
                let addr = r(&s[0])?;
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!(
                    "uint t{t} = 0u; if (exec_lo & 1u) {{ {func}(tb_lds[(({addr} + {off:#x}u) >> 2) & 8191u], {}, t{t}); }}",
                    r(&s[1])?
                ));
                if m.contains("_rtn") {
                    self.wv(&d.ok_or("no dst")?, &format!("t{t}"))?;
                }
                Ok(())
            }
            Enc::Ds if m == "ds_bpermute_b32" => {
                if self.required_wave_size != Some(32) || ins.raw[0] & 0x3ff00 != 0 {
                    return Err("backward permute requires a verified wave32 compute layout".into());
                }
                let t = self.tmp;
                self.tmp += 1;
                let (addr, data) = (r(&s[0])?, r(&s[1])?);
                self.line(&format!("uint t{t} = WaveReadLaneAt(((exec_lo & 1u) != 0u) ? {data} : 0u, (({addr} + {}u) >> 2) & 31u);", ins.aux.imm));
                self.wv(&d.ok_or("no permute destination")?, &format!("t{t}"))
            }
            Enc::Ds if m == "ds_swizzle_b32" => {
                if self.resolve_wave32 {
                    let off = ins.aux.imm as u32;
                    if ins.raw[0] & (1 << 17) != 0 || (off & 0x8000 != 0 && off & 0xff00 != 0x8000) {
                        return Err("unsupported resolve wave swizzle mode".into());
                    }
                    let lane = "WaveGetLaneIndex()";
                    let partner = if off & 0x8000 != 0 {
                        format!("({lane} & ~3u) | (({:#x}u >> (2u * ({lane} & 3u))) & 3u)", off & 255)
                    } else {
                        format!("(({lane} & {}u) | {}u) ^ {}u", off & 31, (off >> 5) & 31, (off >> 10) & 31)
                    };
                    let source = r(&s[0])?;
                    let t = self.tmp;
                    self.tmp += 1;
                    self.line(&format!("uint t{t} = WaveReadLaneAt((exec_lo & tb_lane_bit) != 0u ? {source} : 0u, {partner});"));
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("t{t}"));
                }
                if self.is_cs && self.packed.as_ref().is_some_and(|p| p.scan_width.is_some()) {
                    let off = ins.aux.imm as u32;
                    if ins.raw[0] & (1 << 17) != 0 || off & 0x8000 != 0 { return Err("unsupported compute swizzle mode".into()); }
                    let t = self.tmp;
                    self.tmp += 1;
                    // Bitmask mode rearranges lanes within each independent group of 32.
                    self.line(&format!("uint t{t}lane = WaveGetLaneIndex();"));
                    self.line(&format!("uint t{t}src = (t{t}lane & ~31u) | (((t{t}lane & {}u) | {}u) ^ {}u);",
                        off & 31, (off >> 5) & 31, (off >> 10) & 31));
                    self.line(&format!("uint t{t}value = WaveReadLaneAt({}, t{t}src);", r(&s[0])?));
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("t{t}value"));
                }
                if self.compute_swizzle {
                    if self.hoisted_swizzle != Some(idx) { self.emit_compute_swizzle(idx, ins, &[])?; }
                    return self.wv(&d.ok_or("no swizzle destination")?, &format!("swizzle{idx}"));
                }
                if !is_ps { return Err("ds_swizzle outside a pixel shader".into()); }
                let off = ins.aux.imm as u32;
                if off & 0x8000 == 0 { return Err("ds_swizzle without QDMode unsupported".into()); }
                self.uses_quad = true;
                let src = r(&s[0])?;
                let t = self.tmp;
                self.tmp += 1;
                let dst = d.ok_or("no dst")?;
                let pattern = off & 255;
                let lane = pattern & 3;
                self.line("#if __SHADER_TARGET_MAJOR >= 6");
                // Keep reads outside the emulated EXEC predicate so helper lanes participate.
                if pattern == lane * 0x55 {
                    self.line(&format!("uint t{t}quad = QuadReadLaneAt({src}, {lane});"));
                    self.wv(&dst, &format!("t{t}quad"))?;
                } else {
                    self.line(&format!("uint4 t{t}quad = uint4(QuadReadLaneAt({src}, 0), QuadReadLaneAt({src}, 1), QuadReadLaneAt({src}, 2), QuadReadLaneAt({src}, 3));"));
                    self.wv(&dst, &format!("t{t}quad[({pattern:#x}u >> (tb_quad * 2u)) & 3u]"))?;
                }
                self.line("#else");
                self.line(&format!("float t{t} = asfloat({src}); uint t{t}s = ({:#x}u >> (tb_quad * 2u)) & 3u;", off & 255));
                self.wv(&dst, &format!("asuint(t{t} + ((float)(t{t}s & 1u) - (float)(tb_quad & 1u)) * ddx_fine(t{t}) + ((float)(t{t}s >> 1) - (float)(tb_quad >> 1)) * ddy_fine(t{t}))"))?;
                self.line("#endif");
                Ok(())
            }
            _ => Err("unsupported encoding".into()),
        }
    }
}
