//! Buffer loads, stores and atomics.
use super::*;

impl<'a> Emitter<'a> {
    /// Number of dwords moved by a MUBUF dword load/store mnemonic.
    pub(in crate::hlsl) fn mubuf_dwords(m: &str) -> Option<u32> {
        Some(match m.rsplit('_').next()? {
            "dword" | "byte" | "short" | "ubyte" | "sbyte" | "ushort" | "sshort" => 1,
            "dwordx2" => 2,
            "dwordx3" => 3,
            "dwordx4" => 4,
            _ => return None,
        })
    }

    /// The stride of an indexed load through the heap, when another path through the
    /// program loads the same field of the same element at `index << n`.
    pub(in crate::hlsl) fn sibling_stride(&self, ins: &Insn) -> Option<u32> {
        let Some(&Opnd::Vgpr(index)) = ins.src.first() else { return None };
        let here = self.insns.iter().position(|i| i.offset == ins.offset)?;
        let dwords = |m: &str| Self::mubuf_dwords(m).unwrap_or(1) as i64 * 4;
        for (j, shift) in self.insns.iter().enumerate() {
            let k = match (shift.mnemonic.as_str(), shift.src.as_slice()) {
                ("v_lshl_add_u32", [Opnd::Vgpr(v), Opnd::ConstInt(k), ..]) if *v == index => *k,
                ("v_lshlrev_b32", [Opnd::ConstInt(k), Opnd::Vgpr(v), ..]) if *v == index => *k,
                _ => continue,
            };
            if !(2..=10).contains(&k) { continue; }
            let Some(&Opnd::Vgpr(address)) = shift.dst.first() else { continue };
            // The index holds one value throughout.
            let (from, to) = (here.min(j), here.max(j));
            if self.insns[from..to].iter().any(|i| i.dst.contains(&Opnd::Vgpr(index))) { continue; }
            let same = self.insns[j + 1..].iter().take(16)
                .take_while(|i| !i.dst.contains(&Opnd::Vgpr(address)) || i.enc == Enc::Mubuf)
                .any(|i| i.enc == Enc::Mubuf && i.mnemonic.starts_with("buffer_load_dword") && i.aux.mask & 3 == 1
                    && i.src.first() == Some(&Opnd::Vgpr(address))
                    && (i.aux.imm..i.aux.imm + dwords(&i.mnemonic)).contains(&ins.aux.imm));
            if same && ins.aux.imm + dwords(&ins.mnemonic) <= 1 << k { return Some(1 << k); }
        }
        None
    }

    /// Resolve a raw buffer's byte address before any overlapping VGPR is overwritten.
    ///
    /// OFFEN and SOFFSET are byte offsets. A raw descriptor's stride is one byte,
    /// the same stride a shader writes when it builds that descriptor and the size
    /// recorded for a raw binding, so IDXEN is a byte offset too. With both enables
    /// the first address VGPR is the index and the next is the byte offset.
    pub(in crate::hlsl) fn raw_address(&mut self, ins: &Insn) -> Result<String, String> {
        let (indexed, offset) = (ins.aux.mask & 2 != 0, ins.aux.mask & 1 != 0);
        let vector = if indexed {
            let stride = matches!(self.res.get(&self.cur_idx).and_then(|v| v.first()), Some(Res::HeapRaw(_)))
                .then(|| self.sibling_stride(ins)).flatten();
            if (stride.is_none() && !self.raw_byte_index.contains(&ins.offset))
                || !ins.mnemonic.starts_with("buffer_load_dword")
            {
                return Err("raw buffer IDXEN needs the console descriptor stride".into());
            }
            let mut index = r(&ins.src[0])?;
            if let Some(stride) = stride { index = format!("({index} * {stride}u)"); }
            if offset {
                let Opnd::Vgpr(v) = ins.src[0] else { return Err("invalid indexed raw address".into()) };
                let byte_offset = Opnd::Vgpr(v.checked_add(1).ok_or("raw address register overflow")?);
                self.touch(&byte_offset);
                format!("({index} + {})", r(&byte_offset)?)
            } else { index }
        } else if offset { r(&ins.src[0])? } else { "0u".to_string() };
        let address = format!("raw_addr{}", self.tmp);
        self.tmp += 1;
        self.line(&format!("uint {address} = {vector} + {} + {}u;", r(&ins.src[2])?, ins.aux.imm));
        Ok(address)
    }

    pub(in crate::hlsl) fn emit_mtbuf(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        let buffer = match self.res.get(&idx).and_then(|r| r.first()).copied() {
            Some(Res::Raw(slot, false)) => format!("raw_sbuf{slot}"),
            Some(Res::HeapRaw(src)) => self.bind_heap(HeapView::Raw, src),
            _ => return Err("typed buffer load has no verified linear root SRV".into()),
        };
        // Only formatted, full-width loads with a byte offset and no TFE.
        let [first, second] = ins.raw[..] else {
            return Err("invalid MTBUF encoding".into());
        };
        if ins.op >= 4
            || ins.aux.mask != 1
            || first & 0xc000 != 0
            || second & 0x01e0_0000 != 0
            || !self.zero_buffer_offset(ins)
        {
            return Err("unsupported typed buffer load addressing or modifiers".into());
        }
        let [Opnd::Vgpr(base)] = ins.dst[..] else {
            return Err("invalid MTBUF destination".into());
        };
        let address = self.raw_address(ins)?;
        // The supported formats consume one naturally aligned dword, except
        // 16_16_16_16_UINT, whose element is 8 bytes. Read them before updating
        // VGPRs in case VADDR overlaps the result.
        let packed = format!("t{}", self.tmp);
        let packed1 = format!("t{}", self.tmp + 1);
        self.tmp += 2;
        if ins.aux.imm2 == 69 {
            self.line(&format!(
                "uint {packed} = 0u; uint {packed1} = 0u; if (exec_lo & 1u) {{ {packed} = {buffer}.Load({address}); {packed1} = {buffer}.Load({address} + 4u); }}"
            ));
        } else {
            self.line(&format!(
                "uint {packed} = 0u; if (exec_lo & 1u) {packed} = {buffer}.Load({address});"
            ));
        }
        for channel in 0..=ins.op {
            let value = match ins.aux.imm2 {
                // BUFFER_FORMAT_16_16_SNORM.
                24 if channel < 2 => format!(
                    "asuint(max(-1.0f, float(asint({packed} << {}u) >> 16) / 32767.0f))",
                    16 - channel * 16
                ),
                24 => "0u".to_string(),
                // BUFFER_FORMAT_2_10_10_10_UNORM.
                50 => {
                    let max = if channel == 3 { 3 } else { 1023 };
                    format!("asuint(float(({packed} >> {}u) & {max}u) / {max}.0f)", channel * 10)
                }
                // BUFFER_FORMAT_8_8_8_8_UNORM / UINT.
                56 => format!("asuint(float(({packed} >> {}u) & 255u) / 255.0f)", channel * 8),
                60 => format!("(({packed} >> {}u) & 255u)", channel * 8),
                // BUFFER_FORMAT_32_UINT / 32_FLOAT: X only, then (0, 0, 1).
                20 | 22 if channel == 0 => packed.clone(),
                20 | 22 => (if channel == 3 { if ins.aux.imm2 == 22 { "asuint(1.0f)" } else { "1u" } } else { "0u" }).to_string(),
                // BUFFER_FORMAT_16_FLOAT.
                13 if channel == 0 => format!("asuint(f16tof32({packed}))"),
                13 => (if channel == 3 { "asuint(1.0f)" } else { "0u" }).to_string(),
                // BUFFER_FORMAT_16_16_16_16_UINT: four 16-bit unsigned channels.
                69 => tbuf_16_16_16_16_uint_channel(channel, &packed, &packed1),
                // BUFFER_FORMAT_16_16_UNORM / UINT / FLOAT.
                23 | 27 | 29 if channel >= 2 => (if channel == 3 { if ins.aux.imm2 == 27 { "1u" } else { "asuint(1.0f)" } } else { "0u" }).to_string(),
                23 => format!("asuint(float(({packed} >> {}u) & 65535u) / 65535.0f)", channel * 16),
                27 => format!("(({packed} >> {}u) & 65535u)", channel * 16),
                29 => format!("asuint(f16tof32({packed} >> {}u))", channel * 16),
                _ => return Err(format!("unsupported typed buffer format {}", ins.aux.imm2)),
            };
            let dst = base
                .checked_add(channel as u8)
                .ok_or("MTBUF destination overflows")?;
            self.wv(&Opnd::Vgpr(dst), &value)?;
        }
        Ok(())
    }

    pub(in crate::hlsl) fn raw_transfer(&mut self, ins: &Insn, slot: u32, is_uav: bool) -> Result<(), String> {
        let buffer = if is_uav { format!("raw_ubuf{slot}") } else { format!("raw_sbuf{slot}") };
        self.raw_transfer_on(ins, buffer, is_uav)
    }

    /// Raw transfer through a heap index held in `src` (compute heap). Stores use the
    /// writable view of the same descriptor.
    pub(in crate::hlsl) fn heap_raw_transfer(&mut self, ins: &Insn, src: u16) -> Result<(), String> {
        let store = ins.mnemonic.starts_with("buffer_store_");
        let view = if store { HeapView::RawRw } else { HeapView::Raw };
        let buf = self.bind_heap(view, src);
        self.raw_transfer_on(ins, buf, store)
    }

    /// `buffer_load_format_*` with IDXEN through a heap descriptor: a typed view,
    /// whose desktop descriptor supplies the element format and stride.
    pub(in crate::hlsl) fn heap_typed_load(&mut self, idx: usize, ins: &Insn, src: u16) -> Result<(), String> {
        if ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
            return Err("typed heap buffer load with a byte offset unsupported".into());
        }
        let channels = match ins.mnemonic.rsplit('_').next() {
            Some("x") => 1u8, Some("xy") => 2, Some("xyz") => 3, Some("xyzw") => 4,
            _ => return Err(format!("unsupported typed heap load {}", ins.mnemonic)),
        };
        let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
        let index = r(&ins.src[0])?;
        let t = self.tmp;
        self.tmp += 1;
        if self.heap_load_ty.get(&idx) == Some(&[false, true, false]) {
            let buf = self.bind_heap(HeapView::TypedUint, src);
            self.line(&format!("uint4 t{t} = 0; if (exec_lo & 1u) t{t} = {buf}.Load({index});"));
            for k in 0..channels {
                self.wv(&Opnd::Vgpr(vd + k), &format!("t{t}.{}", XYZW[k as usize]))?;
            }
            return Ok(());
        }
        if self.heap_load_ty.get(&idx).is_some_and(|seen| seen[1] || seen[2]) {
            return Err(format!("typed heap load at {:#x}: element type is ambiguous", ins.offset));
        }
        let buf = self.bind_heap(HeapView::Typed, src);
        self.line(&format!("float4 t{t} = 0; if (exec_lo & 1u) t{t} = {buf}.Load({index});"));
        for k in 0..channels {
            self.wv(&Opnd::Vgpr(vd + k), &format!("asuint(t{t}.{})", XYZW[k as usize]))?;
        }
        Ok(())
    }

    /// `buffer_store_format_*` with IDXEN through a heap descriptor: the typed UAV's
    /// desktop descriptor supplies the element format, so store the float channels.
    pub(in crate::hlsl) fn heap_typed_store(&mut self, ins: &Insn, src: u16) -> Result<(), String> {
        if ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
            return Err("typed heap buffer store with a byte offset unsupported".into());
        }
        let channels = match ins.mnemonic.rsplit('_').next() {
            Some("x") => 1u8, Some("xy") => 2, Some("xyz") => 3, Some("xyzw") => 4,
            _ => return Err(format!("unsupported typed heap store {}", ins.mnemonic)),
        };
        let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
        let index = r(&ins.src[0])?;
        let buf = self.bind_heap(HeapView::TypedRw, src);
        let mut value = Vec::new();
        for k in 0..4u8 {
            value.push(if k < channels {
                let o = Opnd::Vgpr(vd + k);
                self.touch(&o);
                format!("asfloat({})", r(&o)?)
            } else if k == 3 { "1.0f".into() } else { "0.0f".into() });
        }
        self.line(&format!("if (exec_lo & 1u) {buf}[{index}] = float4({});", value.join(", ")));
        Ok(())
    }

    pub(in crate::hlsl) fn raw_transfer_on(&mut self, ins: &Insn, buffer: String, is_uav: bool) -> Result<(), String> {
        let load = ins.mnemonic.starts_with("buffer_load_");
        if !load && (!is_uav || !ins.mnemonic.starts_with("buffer_store_")) {
            return Err("unsupported write to raw buffer".into());
        }
        let Opnd::Vgpr(base) = ins.dst[0] else { return Err("raw buffer needs VGPR data".into()); };
        let width = match ins.mnemonic.rsplit('_').next() {
            Some("dword" | "x") => 1, Some("dwordx2") => 2, Some("dwordx3") => 3, Some("dwordx4") => 4,
            _ => return Err(format!("unsupported raw buffer transfer {}", ins.mnemonic)),
        };
        let address = self.raw_address(ins)?;
        let t = self.tmp;
        self.tmp += 1;
        for k in 0..width {
            let offset = format!("{address} + {}u", 4 * k);
            let reg = Opnd::Vgpr(base.checked_add(k).ok_or("raw buffer register overflow")?);
            if load {
                if ins.aux.mask & 2 != 0 {
                    self.line(&format!("uint t{t}_{k} = 0u; if (exec_lo & 1u) t{t}_{k} = {buffer}.Load({offset});"));
                } else {
                    self.line(&format!("uint t{t}_{k} = {buffer}.Load({offset});"));
                }
            } else {
                self.touch(&reg);
                self.line(&format!("if (exec_lo & 1u) {buffer}.Store({offset}, {});", r(&reg)?));
            }
        }
        if load {
            for k in 0..width {
                self.wv(&Opnd::Vgpr(base + k), &format!("t{t}_{k}"))?;
            }
        }
        Ok(())
    }

    /// A conservative first subset: straight-line compute, explicit finite
    /// CBV root table, unchanged base pair, bounded aligned selector, and
    /// only immediate scalar-buffer consumers of the descriptor words.
    pub(in crate::hlsl) fn bounded_cbv_offsets(&self, idx: usize, table: u8, base: u16, dst: u16)
        -> Option<(u8, u32, Vec<(u8,u32)>)>
    {
        use crate::rdna2::clobbers_scalar;
        let ins = self.insns.get(idx)?;
        if self.ex.stage != Stage::Compute || self.packed.is_some() || self.mesh_bindings.is_some()
            || self.compute_heap || table != 4 || base != 4 || ins.op != 2
            || dst < 16 || dst > 102 || dst % 4 != 0
            || self.insns.len() > 32768 || self.insns.iter().any(|op|
                !op.is_complete() || op.enc == Enc::Unknown || op.mnemonic.contains('<')
                || op.mnemonic.starts_with("tb_") || op.mnemonic.starts_with("s_movrel")
                || op.mnemonic.starts_with("s_subvector_loop")
                || op.mnemonic.starts_with("s_cbranch") || matches!(op.mnemonic.as_str(),
                    "s_branch" | "s_setpc_b64" | "s_swappc_b64" | "s_call_b64" |
                    "s_rfe_b64" | "s_trap"))
        { return None; }
        if self.insns[..idx].iter().any(|op| clobbers_scalar(op, 4) || clobbers_scalar(op, 5)) {
            return None;
        }
        let reg = (ins.raw.get(1)? >> 25) as u8;
        if reg > 105 || (dst..dst+4).contains(&u16::from(reg)) { return None; }
        let imm = u32::try_from(ins.aux.imm).ok()?;
        if imm % 32 != 0 { return None; }
        let mask = code_offset::possible_bits(&self.insns, &branch_targets(&self.insns), idx, reg, 8)?;
        if mask == 0 || mask & 31 != 0 || mask.count_ones() > 4 || imm.checked_add(mask)? > 0x00ff_ffff {
            return None;
        }
        let [range] = self.root_table(table)? else { return None };
        if range.range_type != 2 || range.num == 0 || range.num == u32::MAX
            || range.offset_in_table != 0 && range.offset_in_table != u32::MAX {
            return None;
        }
        let mut candidates = Vec::new();
        let mut bits = mask;
        loop {
            let off = imm.checked_add(bits)?;
            let index = off / 32;
            if index >= range.num { return None; }
            let slot = range.base_register.checked_add(index)?;
            if self.ex.resources.iter().filter(|p| p.res_type == 2 && p.space == range.space &&
                p.lower <= slot && slot <= p.upper).count() != 1 { return None; }
            if self.table_binding_space(table, off, 32).ok()? != (Some(2), slot, range.space) {
                return None;
            }
            candidates.push((table, off));
            if bits == 0 { break; }
            bits = (bits - 1) & mask;
        }
        candidates.sort_unstable();
        // The selector stands in for the descriptor; it is not the descriptor
        // data. Until general escape analysis exists, reject any clobber,
        // potential wide source alias or other descriptor use.
        let mut loads = 0usize;
        for op in &self.insns[idx+1..] {
            if (dst..dst+4).any(|reg| clobbers_scalar(op, reg as u8)) { return None; }
            let consumer = op.enc == Enc::Smem && (8..=12).contains(&op.op) &&
                op.src.first() == Some(&Opnd::Sgpr(dst as u8)) &&
                op.raw.get(1).is_some_and(|word| word >> 25 == 0x7d);
            if consumer { loads += 1; }
            if op.enc == Enc::Smem && !consumer && op.raw.get(1).is_some_and(|word|
                (u32::from(dst)..u32::from(dst+4)).contains(&(word >> 25))) {
                return None;
            }
            for (i, src) in op.src.iter().enumerate() {
                if consumer && i == 0 { continue; }
                if matches!(src, Opnd::Sgpr(reg) if (dst.saturating_sub(15)..dst+4).contains(&u16::from(*reg))) {
                    return None;
                }
            }
        }
        if loads == 0 { return None; }
        Some((reg, imm, candidates))
    }

    pub(in crate::hlsl) fn dynamic_code_offset(&self, idx: usize) -> Option<u8> {
        if self.ex.stage != Stage::Compute || self.packed.is_some() || self.mesh_bindings.is_some() {
            return None;
        }
        code_offset::prove(&self.insns, idx, self.ex.isa.len(), self.ex.isa_data.len())
    }

    pub(in crate::hlsl) fn static_smem_offset(&self, ins: &Insn) -> Option<u32> {
        let imm = u32::try_from(ins.aux.imm).ok()?;
        if ins.raw.get(1).copied()? >> 25 == 0x7d { return Some(imm); }
        if self.proven_zero_smem_offset(ins) { return Some(imm); }
        let reg = ins.raw.get(1).copied()? >> 25;
        if reg > 105 { return None; }
        let offset = *self.smem_constants
            .get_or_init(|| mubuf_offsets::proven_smem_constants(&disassemble(&self.ex.isa)))
            .get(&(ins.offset, reg as u8))?;
        let combined = imm.checked_add(offset)?;
        // Static table selectors encode the table in the upper byte.
        // Do not turn unaligned or out-of-range addresses into descriptors.
        (combined <= 0x00ff_ffff && combined % 4 == 0).then_some(combined)
    }

    pub(in crate::hlsl) fn proven_zero_smem_offset(&self, ins: &Insn) -> bool {
        let reg = ins.raw.get(1).copied().unwrap_or(u32::MAX) >> 25;
        reg <= 105 && self.zero_buffer_offsets
            .get_or_init(|| mubuf_offsets::proven_zero(&disassemble(&self.ex.isa)))
            .contains(&(ins.offset, reg as u8))
    }

    pub(in crate::hlsl) fn zero_buffer_offset(&self, ins: &Insn) -> bool {
        match ins.src.get(2) {
            Some(Opnd::ConstInt(0)) => true,
            Some(Opnd::Sgpr(reg)) => self.zero_buffer_offsets
                .get_or_init(|| mubuf_offsets::proven_zero(&disassemble(&self.ex.isa)))
                .contains(&(ins.offset, *reg)),
            _ => false,
        }
    }

    /// MUBUF on a structured buffer: `vaddr` is the element index (IDXEN), the 12-bit
    /// immediate the byte offset inside the element. Dynamic offsets (OFFEN / SOFFSET
    /// register) are not handled yet and fail explicitly.
    pub(in crate::hlsl) fn emit_mubuf(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        // LDS (dword0[16]: the data goes to LDS instead of VGPRs) and TFE (dword1[23]: a
        // status dword after the data) change where results land (bit positions from AMD's
        // machine-readable RDNA2 spec).
        if (ins.raw.first().copied().unwrap_or(0) >> 16) & 1 != 0 {
            return Err("MUBUF LDS (load to LDS) unsupported".into());
        }
        if (ins.raw.get(1).copied().unwrap_or(0) >> 23) & 1 != 0 {
            return Err("MUBUF TFE status return unsupported".into());
        }
        if let Some(Res::CbDyn(slot)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
            if (idxen && offen) || !self.zero_buffer_offset(ins) || !ins.mnemonic.starts_with("buffer_load_dword") {
                return Err("constant-buffer MUBUF form unsupported".into());
            }
            let n = Self::mubuf_dwords(&ins.mnemonic).ok_or("unsupported MUBUF op")?;
            let vd = match ins.dst[0] {
                Opnd::Vgpr(v) => v,
                _ => return Err("bad vdata".into()),
            };
            let base = if offen {
                format!("({:#x}u + {})", ins.aux.imm as u32, r(&ins.src[0])?)
            } else if idxen {
                // Indexed access to a constant buffer = a dynamically indexed cbuffer array.
                // Elements of cbuffer arrays are 16-byte registers, which is the stride the
                // CBV descriptor carries (inferred: the title reads 3 dwords per element).
                format!("({:#x}u + {} * 16u)", ins.aux.imm as u32, r(&ins.src[0])?)
            } else {
                format!("{:#x}u", ins.aux.imm as u32)
            };
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = {base} >> 2;"));
            for k in 0..n {
                self.line(&format!("uint t{t}_{k} = cb{slot}[((t{t} + {k}u) >> 2) & 4095u][(t{t} + {k}u) & 3u];"));
            }
            for k in 0..n {
                self.wv(&Opnd::Vgpr(vd + k as u8), &format!("t{t}_{k}"))?;
            }
            return Ok(());
        }
        let resolved = self.res.get(&idx).and_then(|v| v.first()).copied();
        if let Some(Res::HeapRaw(src)) = resolved {
            if ins.mnemonic.contains("_atomic_") {
                return Err("atomic through a heap descriptor unsupported".into());
            }
            if ins.mnemonic.starts_with("buffer_load_format_") && ins.aux.mask & 2 != 0 {
                return self.heap_typed_load(idx, ins, src);
            }
            if ins.mnemonic.starts_with("buffer_store_format_") && ins.aux.mask & 2 != 0 {
                return self.heap_typed_store(ins, src);
            }
            return self.heap_raw_transfer(ins, src);
        }
        if let Some(Res::Raw(slot, is_uav)) = resolved {
            if !ins.mnemonic.contains("_atomic_") {
                return self.raw_transfer(ins, slot, is_uav);
            }
        }
        if ins.mnemonic.contains("_atomic_") {
            // Buffer atomics → HLSL Interlocked* on the UAV element. With GLC set the
            // pre-operation value is returned in vdata.
            let (offen, idxen, glc) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0, ins.aux.mask & 4 != 0);
            let raw = matches!(resolved, Some(Res::Raw(_, true)));
            let scalar_only = !offen && !idxen && matches!(resolved, Some(Res::Buf(_, true))) && matches!(ins.src[2], Opnd::Sgpr(_));
            if (offen && idxen) || !(raw || scalar_only || self.zero_buffer_offset(ins)) {
                return Err("atomic MUBUF addressing unsupported".into());
            }
            // IDXEN selects an element. OFFEN and SOFFSET add byte offsets.
            // With no register offset the immediate alone selects the counter.
            let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
            let receiver = match resolved {
                Some(Res::Raw(slot, true)) => format!("raw_ubuf{slot}."),
                _ => String::new(),
            };
            let target = match (resolved, offen) {
                (Some(Res::Raw(_, true)), _) => self.raw_address(ins)?,
                (Some(Res::Buf(slot, true)), false) if scalar_only => {
                    let nwords = self.sbuf_words((true, slot));
                    let w = format!("(({} + {:#x}u) >> 2)", r(&ins.src[2])?, ins.aux.imm as u32);
                    format!("ubuf{slot}[{w} / {nwords}u].w[{w} % {nwords}u]")
                }
                (Some(Res::TBuf(slot, true)), false) if ins.aux.imm == 0 => format!("utb{slot}[{index}]"),
                (Some(Res::TBuf(slot, true)), true) => format!("utb{slot}[(({}) + {:#x}u) >> 2]", r(&ins.src[0])?, ins.aux.imm as u32),
                (Some(Res::Buf(slot, true)), false) => {
                    let (nwords, w) = (self.sbuf_words((true, slot)), ins.aux.imm as u32 / 4);
                    format!("ubuf{slot}[{index} + {}u].w[{}]", w / nwords, w % nwords)
                }
                (Some(Res::Buf(slot, true)), true) => {
                    let nwords = self.sbuf_words((true, slot));
                    let w = format!("((({}) + {:#x}u) >> 2)", r(&ins.src[0])?, ins.aux.imm as u32);
                    format!("ubuf{slot}[{w} / {nwords}u].w[{w} % {nwords}u]")
                }
                _ => return Err("unresolved atomic target".into()),
            };
            let vdata = ins.dst[0];
            let val = r(&vdata)?;
            let t = self.tmp;
            self.tmp += 1;
            self.line(&format!("uint t{t} = 0u; if (exec_lo & 1u) {{"));
            match ins.mnemonic.as_str() {
                "buffer_atomic_cmpswap" => {
                    let Opnd::Vgpr(v) = vdata else { return Err("expected atomic VGPR pair".into()) };
                    let compare = Opnd::Vgpr(v.checked_add(1).ok_or("atomic register pair overflow")?);
                    self.touch(&compare);
                    self.line(&format!("{receiver}InterlockedCompareExchange({target}, {}, {val}, t{t});", r(&compare)?));
                }
                "buffer_atomic_inc" | "buffer_atomic_dec" | "buffer_atomic_smin" | "buffer_atomic_smax" => {
                    // The UAV is declared uint. Perform signed comparisons and bounded
                    // increment/decrement against the value actually replaced by CAS.
                    let old = format!("old{t}");
                    let next = match ins.mnemonic.as_str() {
                        "buffer_atomic_inc" => format!("{old} >= {val} ? 0u : {old} + 1u"),
                        "buffer_atomic_dec" => format!("({old} == 0u || {old} > {val}) ? {val} : {old} - 1u"),
                        "buffer_atomic_smin" => format!("asuint(min(asint({old}), asint({val})))"),
                        _ => format!("asuint(max(asint({old}), asint({val})))"),
                    };
                    self.line(&format!("uint {old}; [allow_uav_condition] do {{ {old} = t{t}; {receiver}InterlockedCompareExchange({target}, {old}, {next}, t{t}); }} while (t{t} != {old});"));
                }
                op => {
                    let (func, arg) = match op {
                        "buffer_atomic_add" => ("InterlockedAdd", val),
                        "buffer_atomic_sub" => ("InterlockedAdd", format!("(0u - {val})")),
                        "buffer_atomic_umin" => ("InterlockedMin", val),
                        "buffer_atomic_umax" => ("InterlockedMax", val),
                        "buffer_atomic_and" => ("InterlockedAnd", val),
                        "buffer_atomic_or" => ("InterlockedOr", val),
                        "buffer_atomic_xor" => ("InterlockedXor", val),
                        "buffer_atomic_swap" => ("InterlockedExchange", val),
                        other => return Err(format!("{other} unsupported")),
                    };
                    self.line(&format!("{receiver}{func}({target}, {arg}, t{t});"));
                }
            }
            self.line("}");
            if glc {
                self.wv(&vdata, &format!("t{t}"))?;
            }
            return Ok(());
        }
        if ins.mnemonic == "buffer_store_format_d16_xyzw" {
            let Some(Res::TBuf(slot, true)) = self.res.get(&idx).and_then(|v| v.first()).copied() else {
                return Err("D16 store needs a typed UAV".into());
            };
            if self.tbuf_uint.contains(&slot) || ins.aux.mask & 1 != 0 || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
                return Err("D16 store with atomic view or offset addressing unsupported".into());
            }
            let index = if ins.aux.mask & 2 != 0 { r(&ins.src[0])? } else { "0u".to_string() };
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("invalid D16 data VGPR".into()) };
            let next = Opnd::Vgpr(vd.checked_add(1).ok_or("D16 VGPR pair overflows")?);
            self.touch(&next);
            let (lo, hi) = (r(&ins.dst[0])?, r(&next)?);
            let fields = format!("uint4({lo} & 65535u, {lo} >> 16, {hi} & 65535u, {hi} >> 16)");
            let value = match self.tbuf_ty.get(&(true, slot)) {
                Some(2) => format!("(asint({fields} << 16) >> 16)"),
                Some(_) => fields,
                None => format!("f16tof32({fields})"),
            };
            self.line(&format!("if (exec_lo & 1u) utb{slot}[{index}] = {value};"));
            return Ok(());
        }
        if let Some(Res::TBuf(slot, true)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            if self.tbuf_uint.contains(&slot) {
                // uint-typed UAV (see atomics): single-component raw loads/stores.
                let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
                // A buffer used with atomics has a single 32-bit channel (D3D requires
                // R32_UINT/SINT for Interlocked*), so multi-component `_format_xy/xyz/xyzw`
                // accesses move only X; a load fills the missing channels with (0, 0, 1).
                let comps = match ins.mnemonic.rsplit('_').next() {
                    Some("x") => 1u8,
                    Some("xy") => 2,
                    Some("xyz") => 3,
                    Some("xyzw") => 4,
                    _ => 0,
                };
                if offen || ins.aux.imm != 0 || comps == 0 {
                    return Err("uint typed-buffer MUBUF form unsupported".into());
                }
                // Without IDXEN the access addresses element 0.
                let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
                let vd = ins.dst[0];
                if ins.mnemonic.starts_with("buffer_store") {
                    self.touch(&vd);
                    self.line(&format!("if (exec_lo & 1u) utb{slot}[{index}] = {};", r(&vd)?));
                } else {
                    let t = self.tmp;
                    self.tmp += 1;
                    self.line(&format!("uint t{t} = utb{slot}[{index}];"));
                    self.wv(&vd, &format!("t{t}"))?;
                    if let Opnd::Vgpr(v) = vd {
                        for k in 1..comps {
                            self.wv(&Opnd::Vgpr(v + k), if k == 3 { "1u" } else { "0u" })?;
                        }
                    }
                }
                return Ok(());
            }
        }
        if let Some(Res::TBuf(slot, is_uav)) = self.res.get(&idx).and_then(|v| v.first()).copied() {
            // Typed buffer. The element type is declared float4: the view format is not
            // visible in the shader, so integer-format buffers are a known gap (TECHNICAL_DEBT).
            let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
            if offen || ins.aux.imm != 0 || !self.zero_buffer_offset(ins) {
                return Err("typed-buffer MUBUF with offset addressing unsupported".into());
            }
            let n = match ins.mnemonic.rsplit('_').next() {
                Some("x") => 1u8,
                Some("xy") => 2,
                Some("xyz") => 3,
                Some("xyzw") => 4,
                _ => return Err("unsupported typed-buffer op".into()),
            };
            // Without IDXEN (and with no offset) the access addresses element 0.
            let index = if idxen { r(&ins.src[0])? } else { "0u".to_string() };
            let vd = match ins.dst[0] {
                Opnd::Vgpr(v) => v,
                _ => return Err("bad vdata".into()),
            };
            let nm = if is_uav { format!("utb{slot}") } else { format!("stb{slot}") };
            if ins.mnemonic.starts_with("buffer_store") {
                if !is_uav {
                    return Err("buffer_store_format to a non-UAV table".into());
                }
                let mut c = vec![];
                for k in 0..4u8 {
                    if k < n {
                        let o = Opnd::Vgpr(vd + k);
                        self.touch(&o);
                        c.push(f(&o)?);
                    } else {
                        c.push("0.0".into());
                    }
                }
                match self.tbuf_ty.get(&(is_uav, slot)) {
                    Some(t) => {
                        let mut c = vec![];
                        for k in 0..4u8 {
                            c.push(if k < n { r(&Opnd::Vgpr(vd + k))? } else { "0u".into() });
                        }
                        let ctor = if *t == 2 { "int4" } else { "uint4" };
                        self.line(&format!("if (exec_lo & 1u) {nm}[{index}] = {ctor}({});", c.join(", ")));
                    }
                    None => self.line(&format!("if (exec_lo & 1u) {nm}[{index}] = float4({});", c.join(", "))),
                }
            } else {
                let t = self.tmp;
                self.tmp += 1;
                let ety = match self.tbuf_ty.get(&(is_uav, slot)) {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                self.line(&format!("{ety} t{t} = {nm}[{index}];"));
                for k in 0..n {
                    let c = XYZW[k as usize];
                    let e = match ety {
                        "float4" => format!("asuint(t{t}.{c})"),
                        "int4" => format!("(uint)t{t}.{c}"),
                        _ => format!("t{t}.{c}"),
                    };
                    self.wv(&Opnd::Vgpr(vd + k), &e)?;
                }
            }
            return Ok(());
        }
        let (slot, is_uav) = match self.res.get(&idx).and_then(|v| v.first()) {
            Some(Res::Buf(s, u)) => (*s, *u),
            _ => return Err("unresolved buffer".into()),
        };
        // Address = element index (IDXEN: first address register) * stride + byte offset
        // (OFFEN: the next address register, or the first when there is no index) + imm.
        let (offen, idxen) = (ins.aux.mask & 1 != 0, ins.aux.mask & 2 != 0);
        // SOFFSET is one more byte term of the address (buffer base + soffset + voffset +
        // index * stride + imm); fold a non-zero one into the run-time byte offset.
        let soff = if self.zero_buffer_offset(ins) {
            None
        } else {
            let o = ins.src.get(2).copied().ok_or("missing MUBUF SOFFSET")?;
            if !matches!(o, Opnd::Sgpr(_) | Opnd::ConstInt(_)) {
                return Err("MUBUF with a dynamic SOFFSET is unsupported".into());
            }
            self.touch(&o);
            Some(r(&o)?)
        };
        let n = Self::mubuf_dwords(&ins.mnemonic).ok_or("unsupported MUBUF op")?;
        let a0 = ins.src[0];
        let a1 = match a0 {
            Opnd::Vgpr(v) => Opnd::Vgpr(v + 1),
            other => other,
        };
        let index = if idxen { r(&a0)? } else { "0u".to_string() };
        let voff = match (idxen, offen) {
            (_, false) => None,
            (false, true) => Some(r(&a0)?),
            (true, true) => {
                self.touch(&a1);
                Some(r(&a1)?)
            }
        };
        let voff = match (voff, soff) {
            (Some(v), Some(s)) => Some(format!("({v}) + ({s})")),
            (v, None) => v,
            (None, s) => s,
        };
        let w0 = ins.aux.imm as u32 / 4;
        let vd = match ins.dst[0] {
            Opnd::Vgpr(v) => v,
            _ => return Err("bad vdata".into()),
        };
        let nm = if is_uav { format!("ubuf{slot}") } else { format!("sbuf{slot}") };
        // Element `k` of this access. With a run-time byte offset the buffer is addressed
        // flat over the declared element size (which assumes, like every structured-buffer
        // access here, that the declared element matches the view's stride).
        let nwords = self.sbuf_words((is_uav, slot));
        let t = self.tmp;
        self.tmp += 1;
        if let Some(vo) = &voff {
            self.line(&format!("uint t{t}w = (({vo}) + {:#x}u) >> 2;", ins.aux.imm as u32));
        }
        let elem = |k: u32| match &voff {
            // A constant offset past the element addresses the following elements.
            None if w0 + k >= nwords => format!("{nm}[{index} + {}u].w[{}]", (w0 + k) / nwords, (w0 + k) % nwords),
            None => format!("{nm}[{index}].w[{}]", w0 + k),
            Some(_) => format!("{nm}[{index} + (t{t}w + {k}u) / {nwords}u].w[(t{t}w + {k}u) % {nwords}u]"),
        };
        let subword = match ins.mnemonic.as_str() {
            "buffer_load_ubyte" | "buffer_load_sbyte" | "buffer_store_byte" => Some(8),
            "buffer_load_ushort" | "buffer_load_sshort" | "buffer_store_short" => Some(16),
            _ => None,
        };
        if let Some(bits) = subword {
            let offset = voff.as_deref().unwrap_or("0u");
            // Short accesses use naturally aligned addresses.
            let align = if bits == 16 { 2 } else { 3 };
            self.line(&format!("uint shift{t} = (({offset} + {}u) & {align}u) * 8u;", ins.aux.imm));
            let word = elem(0);
            let mask = (1u32 << bits) - 1;
            let data = Opnd::Vgpr(vd);
            if ins.mnemonic.starts_with("buffer_store") {
                if !is_uav { return Err("subword store requires a UAV".into()); }
                self.touch(&data);
                let value = r(&data)?;
                self.line(&format!("if (exec_lo & 1u) {{ uint old{t}, seen{t} = 0u; [allow_uav_condition] do {{ old{t} = seen{t}; uint v = (old{t} & ~({mask}u << shift{t})) | (({value} & {mask}u) << shift{t}); InterlockedCompareExchange({word}, old{t}, v, seen{t}); }} while (seen{t} != old{t}); }}"));
                return Ok(());
            }
            let field = format!("(({word} >> shift{t}) & {mask}u)");
            let value = if ins.mnemonic.starts_with("buffer_load_s") {
                format!("asuint(asint({field} << {0}) >> {0})", 32-bits)
            } else { field };
            return self.wv(&data, &value);
        }
        if ins.mnemonic.starts_with("buffer_store") {
            if !is_uav {
                return Err("buffer_store to a non-UAV table".into());
            }
            for k in 0..n {
                let o = Opnd::Vgpr(vd + k as u8);
                self.touch(&o);
                self.line(&format!("if (exec_lo & 1u) {} = {};", elem(k), r(&o)?));
            }
        } else {
            for k in 0..n {
                self.line(&format!("uint t{t}_{k} = {};", elem(k)));
            }
            for k in 0..n {
                self.wv(&Opnd::Vgpr(vd + k as u8), &format!("t{t}_{k}"))?;
            }
        }
        Ok(())
    }
}
