//! Descriptor tracking through one block of instructions.
use super::*;

impl<'a> Emitter<'a> {
    /// Transfer function for one basic block (also records resources / reports errors).
    pub(in crate::hlsl) fn res_block(
        &mut self,
        insns: &[Insn],
        sym: &mut HashMap<u16, Sym>,
        written: &mut BTreeSet<u16>,
        range: std::ops::Range<usize>,
        track_heap_origin: bool,
    ) -> Result<(), String> {
        let dbg_reg = std::env::var("TB_DFDEBUG").ok().and_then(|v| v.parse::<u16>().ok());
        let mut dbg_prev: Option<Sym> = dbg_reg.and_then(|r| sym.get(&r).copied());
        let block_start = range.start;
        let inline_samplers = self.resource_heap_base.is_some()
            || (self.ex.root_sig.is_none() && self.ex.resources.iter().any(|r| r.res_type == 1))
            // Literal sampler words can name any static sampler with identical state.
            || self.ex.root_sig.as_ref().is_some_and(|r| !r.static_samplers.is_empty())
            || (self.ex.stage == Stage::Compute && self.ex.root_sig.as_ref().is_some_and(|r| !r.static_samplers.is_empty())
                && self.ex.resources.iter().filter(|r| r.res_type == 1).count() > 1);
        for idx in range {
            if let Some(r) = dbg_reg {
                let cur = sym.get(&r).copied();
                if cur != dbg_prev && idx > 0 {
                    eprintln!("  s{r}: {dbg_prev:?} -> {cur:?} by {:#x} {}", insns[idx - 1].offset, insns[idx - 1].mnemonic);
                }
                dbg_prev = cur;
            }
            let ins = &insns[idx];
            let heap_origin = track_heap_origin.then_some(idx);
            // Read reaching sources before invalidating every result word.
            // In-place copies/selects must see the old value, while multiword
            // image/buffer results must not leave stale offset proofs behind.
            let heap_value = heap_offsets::transfer(ins, sym, self.compute_heap, self.pixel_heap);
            // Judged before the instruction's own writes: a load may overwrite its SOFFSET.
            if self.pixel_heap {
                lane_reads::step(ins, sym, self.one_word_vector_mask());
            }
            // Dataflow key of a scalar location: SGPR number, or 1000+ for a lane-spill slot.
            let sg = |o: &Opnd| match o {
                Opnd::Sgpr(n) => Some(*n as u16),
                Opnd::Temp(n) if *n >= LANE_TEMP_BASE => Some(1000 + (*n - LANE_TEMP_BASE)),
                _ => None,
            };
            if self.mesh_bindings.is_some() && ins.mnemonic != "v_nop" {
                for (i, source) in ins.src.iter().enumerate() {
                    let Some(Sym::MeshRawAddress { word, .. }) = sg(source).and_then(|s| sym.get(&s)) else {
                        continue;
                    };
                    let allowed = ins.mnemonic == "s_mov_b32"
                        || (i == 2 && matches!(ins.mnemonic.as_str(), "v_mbcnt_lo_u32_b32" | "v_mbcnt_hi_u32_b32"))
                        // VOP3 uses a spare SRC2 field for these binary operations.
                        || (ins.enc == Enc::Vop3 && i == 2 && *source == Opnd::Sgpr(0)
                            && (ins.mnemonic.starts_with("v_cmp") || matches!(ins.mnemonic.as_str(),
                                "v_sub_f32" | "v_subrev_f32" | "v_add_f32" | "v_mul_f32" | "v_mul_legacy_f32" | "v_max_f32" | "v_min_f32"
                                | "v_mul_hi_u32" | "v_mul_lo_u32" | "v_mul_hi_i32" | "v_mul_u32_u24" | "v_ldexp_f32"
                                | "v_lshlrev_b32" | "v_lshrrev_b32" | "v_ashrrev_i32" | "v_and_b32" | "v_or_b32" | "v_xor_b32"
                                | "v_add_nc_u32" | "v_sub_nc_u32" | "v_subrev_nc_u32" | "v_max_u32" | "v_min_u32" | "v_max_i32" | "v_min_i32"
                                | "v_bfm_b32" | "v_bcnt_u32_b32" | "v_cvt_pkrtz_f16_f32" | "v_cvt_pknorm_i16_f32" | "v_cvt_pknorm_u16_f32")))
                        || (ins.mnemonic == "s_or_b32" && i == 0 && *word == 1
                            && ins.src.get(1) == Some(&Opnd::Literal(65536)))
                        || (ins.enc == Enc::Smem && ins.op >= 8 && i == 0 && *word == 0)
                        || (matches!(ins.enc, Enc::Mubuf | Enc::Mtbuf) && i == 1 && *word == 0);
                    if !allowed {
                        return Err(format!("mesh root address used as shader data at {:#x}", ins.offset));
                    }
                }
            }
            if idx >= block_start + 3 && ins.mnemonic == "s_or_b32" {
              if let Some((base, slot)) = self.direct_raw_descriptor(&insns[idx-3..=idx], written) {
                sym.insert(base, Sym::DirectRaw(slot));
                sym.remove(&(base + 1));
                written.insert(base + 1);
                continue;
              }
            }
            match ins.enc {
                Enc::Sop1 | Enc::Sop2
                    if self.mesh_bindings.is_some()
                        && matches!(ins.mnemonic.as_str(), "s_bitset1_b32" | "s_or_b32")
                        && ins.src.get(if ins.mnemonic == "s_or_b32" { 1 } else { 0 })
                            == Some(&if ins.mnemonic == "s_or_b32" { Opnd::Literal(65536) } else { Opnd::ConstInt(16) }) =>
                {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let source = if ins.mnemonic == "s_or_b32" { ins.src.first().and_then(sg) } else { Some(d) };
                        let high = source.and_then(|s| sym.get(&s)).copied();
                        sym.remove(&d);
                        if let Some(Sym::MeshRawAddress { register, word: 1 }) = high {
                            sym.insert(d, Sym::MeshRawAddress { register, word: 2 });
                        }
                        written.insert(d);
                    }
                }
                Enc::Sop2 if matches!(ins.mnemonic.as_str(), "s_xor_b32" | "s_or_b32" | "s_and_b32")
                    && ins.src.len() == 2 && ins.src.iter().any(|o| matches!(o, Opnd::ConstInt(_) | Opnd::Literal(_)))
                    && ins.src.iter().any(|o| matches!(sg(o).and_then(|s| sym.get(&s)), Some(Sym::HeapOffset | Sym::HeapSamplerOffset))) =>
                {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let known = |o: &Opnd| sg(o).and_then(|s| sym.get(&s).copied());
                        let literal = |o: &Opnd| match o {
                            Opnd::ConstInt(v) => Some(*v as u32),
                            Opnd::Literal(v) => Some(*v),
                            _ => None,
                        };
                        let result = [(0, 1), (1, 0)].into_iter().find_map(|(a,b)| {
                            let kind = known(&ins.src[a])?;
                            let mask = match kind { Sym::HeapOffset => 31, Sym::HeapSamplerOffset => 15, _ => return None };
                            let value = literal(&ins.src[b])?;
                            (ins.mnemonic == "s_and_b32" || value & mask == 0).then_some(kind)
                        });
                        // The ordinary scalar write invalidates all other facts.
                        sym.remove(&d);
                        written.insert(d);
                        if let Some(kind) = result { sym.insert(d, kind); }
                    }
                }
                Enc::Sop2 if self.resource_heap_base.is_some() && ins.mnemonic == "s_lshl_b32" && ins.src.get(1) == Some(&Opnd::ConstInt(5)) => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::HeapOffset);
                        written.insert(d);
                    }
                }
                Enc::Sop1 if inline_samplers && ins.mnemonic == "s_brev_b32" && matches!(ins.src.first(), Some(Opnd::ConstInt(_) | Opnd::Literal(_))) => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let word = match ins.src[0] { Opnd::ConstInt(v) => v as u32, Opnd::Literal(v) => v, _ => unreachable!() };
                        sym.insert(d, Sym::InlineWord(word.reverse_bits()));
                        written.insert(d);
                    }
                }
                Enc::Sopk if inline_samplers && ins.mnemonic == "s_movk_i32" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::InlineWord(ins.aux.imm as i16 as i32 as u32));
                        written.insert(d);
                    }
                }
                Enc::Sop2 if ins.mnemonic == "s_lshl_b32" && ins.src.get(1) == Some(&Opnd::ConstInt(4)) && ins.src.first().and_then(sg).is_some() => {
                    if let (Some(d), Some(source)) = (ins.dst.first().and_then(sg), ins.src.first().and_then(sg)) {
                        let entry = match sym.get(&source) {
                            Some(Sym::Table(t)) => Some(*t),
                            None if !written.contains(&source) => u8::try_from(source).ok(),
                            _ => None,
                        };
                        if let Some(index) = entry.and_then(|n| sampler_heap::index(self.ex, n)) {
                            sym.insert(d, Sym::SamplerOffset(index));
                        } else if self.pixel_heap {
                            sym.insert(d, Sym::HeapSamplerOffset);
                        } else { sym.remove(&d); }
                        written.insert(d);
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_mov_b32" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if inline_samplers || self.mesh_bindings.is_some() {
                            let literal = match ins.src.first() {
                                Some(Opnd::ConstInt(i)) => Some(*i as u32),
                                Some(Opnd::Literal(v)) => Some(*v),
                                _ => None,
                            };
                            if let Some(v) = literal {
                                sym.insert(d, Sym::InlineWord(v));
                                written.insert(d);
                                continue;
                            }
                        }
                        match ins.src.first().and_then(sg).and_then(|s| sym.get(&s).cloned()) {
                            Some(v) => {
                                sym.insert(d, v);
                            }
                            None => {
                                // A never-written user SGPR copied before use is still a table pointer.
                                let s = ins.src.first().and_then(sg);
                                match s {
                                    Some(s) if !written.contains(&s) => {
                                        sym.insert(d, Sym::Table(s as u8));
                                    }
                                    _ => {
                                        sym.remove(&d);
                                    }
                                }
                            }
                        }
                        written.insert(d);
                        if let Some(s) = ins.src.first().and_then(sg) {
                            if matches!(sym.get(&d), Some(Sym::Desc { k: 0, .. }) | Some(Sym::Multi { k: 0, .. })) {
                                self.dsel_moves.insert(idx, (d, s));
                            }
                        }
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_mov_b64" => {
                    // 64-bit moves copy descriptors two dwords at a time
                    // (`s_mov_b64 s44, s84` + `s_mov_b64 s46, s86` moves a 4-dword descriptor).
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        let s = ins.src.first().and_then(sg);
                        for k in 0..2u16 {
                            match s.and_then(|s| sym.get(&(s + k)).cloned()) {
                                Some(v) => {
                                    if k == 0 && matches!(v, Sym::Desc { k: 0, .. } | Sym::Multi { k: 0, .. }) {
                                        self.dsel_moves.insert(idx, (d, s.unwrap_or(d)));
                                    }
                                    sym.insert(d + k, v);
                                }
                                None => match s {
                                    // A never-written user SGPR is a root table pointer (see
                                    // s_mov_b32); a 64-bit copy carries it in the low half.
                                    Some(s) if k == 0 && !written.contains(&s) => {
                                        sym.insert(d, Sym::Table(s as u8));
                                    }
                                    _ => {
                                        sym.remove(&(d + k));
                                    }
                                },
                            }
                            written.insert(d + k);
                        }
                    }
                }
                // s_setreg_* program a hardware mode register; the encoding's register
                // field is not an SGPR destination.
                Enc::Sopk if ins.mnemonic.starts_with("s_setreg") => {}
                // A code pointer taken through a VGPR: `v = index*4 + sCode` followed by
                // `v_readfirstlane sBase, v` and `s_load [sBase]` is a constant-table lookup
                // with a run-time index (ColorDeficiencyMain, cc45bda725a0f774). VGPRs are
                // tracked under keys 2000+ for exactly this.
                Enc::Vop1 | Enc::Vop2 | Enc::Vop3 if heap_value.is_some() => {
                    if let Some(Opnd::Vgpr(v)) = ins.dst.first() {
                        sym.insert(2000 + *v as u16, heap_value.unwrap());
                    }
                }
                Enc::Vop2 | Enc::Vop3
                    if matches!(ins.mnemonic.as_str(), "v_add_co_u32" | "v_add_nc_u32")
                        && ins.src.iter().any(|o| sg(o).map(|s| sym.get(&s) == Some(&Sym::Code)).unwrap_or(false)) =>
                {
                    if let Some(Opnd::Vgpr(v)) = ins.dst.first() {
                        sym.insert(2000 + *v as u16, Sym::Code);
                    }
                }
                Enc::Vop1 if ins.mnemonic == "v_readfirstlane_b32" => {
                    let code = matches!(ins.src.first(), Some(Opnd::Vgpr(v)) if sym.get(&(2000 + *v as u16)) == Some(&Sym::Code));
                    let heap = match ins.src.first() {
                        Some(Opnd::Vgpr(v)) => sym.get(&(2000 + *v as u16)).copied()
                            .filter(|s| matches!(s, Sym::HeapOffset | Sym::HeapSamplerOffset)),
                        _ => None,
                    };
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if let Some(heap) = heap {
                            // readfirstlane copies one lane's byte offset into an SGPR.
                            // Later uses index ResourceDescriptorHeap[sN]: the SGPR is
                            // uniform until something writes it, which rebinds the handle.
                            sym.insert(d, heap);
                        } else if code {
                            sym.insert(d, Sym::Code);
                        } else {
                            sym.remove(&d);
                        }
                        written.insert(d);
                    }
                }
                Enc::Sop1 if ins.mnemonic == "s_getpc_b64" => {
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        sym.insert(d, Sym::Code);
                        sym.remove(&(d + 1));
                        written.insert(d);
                        written.insert(d + 1);
                    }
                }
                Enc::Sop2 if matches!(ins.mnemonic.as_str(), "s_add_u32" | "s_add_i32") => {
                    // Pointer arithmetic keeps a code pointer a code pointer.
                    let is_code = ins.src.iter().any(|o| sg(o).map(|s| sym.get(&s) == Some(&Sym::Code)).unwrap_or(false));
                    if let Some(d) = ins.dst.first().and_then(sg) {
                        if is_code {
                            sym.insert(d, Sym::Code);
                        } else {
                            sym.remove(&d);
                        }
                        written.insert(d);
                    }
                }
                Enc::Smem if ins.op <= 4 && ins.src.first().and_then(sg).map(|b| sym.get(&b) == Some(&Sym::Code)).unwrap_or(false) => {
                    // Read from an embedded constant table.
                    let n = 1u32 << ins.op;
                    let d = ins.dst.first().and_then(sg).ok_or("s_load: bad dst")?;
                    if self.static_smem_offset(ins).is_none()
                        && self.dynamic_code_offset(idx).is_none() {
                        return Err(format!("s_load at {:#x}: code-table read with dynamic soffset unsupported", ins.offset));
                    }
                    self.uses_code = true;
                    self.res.insert(idx, vec![Res::CodeTbl]);
                    for k in 0..n {
                        sym.remove(&(d + k as u16));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Smem if ins.op <= 4 => {
                    let n = 1u32 << ins.op;
                    let b = ins.src.first().and_then(sg).ok_or("s_load: bad base")?;
                    let d = ins.dst.first().and_then(sg).ok_or("s_load: bad dst")?;
                    let table = match sym.get(&b) {
                        Some(Sym::Table(t)) => *t,
                        None if !written.contains(&b) => b as u8,
                        other => {
                            return Err(format!(
                                "s_load at {:#x}: base s{b} is not a root table pointer ({other:?})",
                                ins.offset
                            ))
                        }
                    };
                    if (ins.raw.get(1).copied().unwrap_or(0) >> 25) != 0x7d {
                        let soff = (ins.raw[1] >> 25) as u16;
                        // A pixel wave includes helper lanes, which host wave intrinsics skip:
                        // an index read from one lane is not the guest's there.
                        // In compute the heap pointer itself is verified (the one FSSM class-5
                        // user record), so any descriptor-sized load from it at a run-time
                        // SOFFSET is a heap fetch, however the byte offset was computed.
                        // Nothing tells a constant buffer from a raw buffer in the heap.
                        if self.pixel_heap && n == 4 && self.resource_heap_base == Some(table) {
                            return Err(format!("s_load at {:#x}: heap buffer in a pixel shader", ins.offset));
                        }
                        let heap_offset = sym.get(&soff) == Some(&Sym::HeapOffset)
                            || (self.compute_heap && !self.pixel_heap && soff < 106 && written.contains(&soff));
                        if self.resource_heap_base == Some(table) && matches!(n, 4 | 8)
                            && (ins.aux.imm == 0 || (ins.aux.imm >= 0 && ins.aux.imm % 32 == 0 && sym.get(&soff) == Some(&Sym::HeapOffset)))
                            && heap_offset {
                            for k in 0..n as u16 {
                                sym.insert(d + k, Sym::HeapWord(k as u8, heap_origin));
                                written.insert(d + k);
                            }
                            sym.insert(d, Sym::HeapDescriptor(n, heap_origin));
                            self.res.insert(idx, vec![Res::HeapLoad(soff)]);
                            continue;
                        }
                        // A sampler picked at run time from the directly indexed compute sampler
                        // heap (CS_RaytracingResolve: per-material index << 4, then a waterfall).
                        // Statically resolved sampler offsets keep their own path below.
                        if self.sampler_heap_base == Some(table) && n == 4 && ins.aux.imm == 0
                            && soff < 106 && written.contains(&soff)
                            && (!self.pixel_heap || sym.get(&soff) == Some(&Sym::HeapSamplerOffset))
                            && !matches!(sym.get(&soff), Some(Sym::SamplerOffset(_)))
                        {
                            for k in 0..4 {
                                sym.insert(d + k, Sym::HeapWord(k as u8, heap_origin));
                                written.insert(d + k);
                            }
                            sym.insert(d, Sym::HeapSamplerIndex(heap_origin));
                            self.res.insert(idx, vec![Res::HeapSamplerLoad(soff)]);
                            continue;
                        }
                        if let Some(Sym::SamplerOffset(index)) = sym.get(&soff).copied() {
                            // Four dwords per sampler, on the heap pointer following
                            // s8 (table) and its immediately following root constants.
                            if u32::from(table) == 9 + index.count && n == 4 && ins.aux.imm == 0 {
                                for k in 0..4 {
                                    sym.remove(&(d+k));
                                    written.insert(d+k);
                                }
                                sym.insert(d, Sym::HeapSampler(index));
                                continue;
                            }
                        }
                        // Preserve heap-specific lowering above. Only fold an ordinary
                        // descriptor-table offset when every reaching path proves its value.
                    }
                    let off = if let Some(off) = self.static_smem_offset(ins) {
                        off
                    } else if let Some((reg, imm, candidates)) = self.bounded_cbv_offsets(idx, table, b, d) {
                        let set = match self.multi_sets.iter().position(|v| *v == candidates) {
                            Some(set) => set,
                            None => {
                                self.multi_sets.push(candidates);
                                self.multi_sets.len() - 1
                            }
                        };
                        let set = u16::try_from(set).map_err(|_| "too many descriptor sets")?;
                        for k in 0..n {
                            sym.insert(d + k as u16, Sym::Multi { set, k });
                            written.insert(d + k as u16);
                        }
                        self.dsel_runtime.insert(idx, (d, table, reg, imm));
                        continue;
                    } else {
                        return Err(format!("s_load at {:#x}: dynamic soffset unsupported", ins.offset));
                    };
                    if self.mesh_bindings.as_ref().is_some_and(|mesh| {
                        self.sampler_heap_base != Some(table)
                            && (!matches!(n, 4 | 8) || !mesh.tables.contains_key(&(table, off)))
                    }) {
                        return Err(format!("unverified mesh descriptor load at {:#x}", ins.offset));
                    }
                    for k in 0..n {
                        sym.insert(d + k as u16, Sym::Desc { table, off, k });
                        self.dsel_loads.insert(idx, (d, ((table as u32) << 24) | off));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Smem => {
                    let n = 1u32 << (ins.op - 8);
                    let b = ins.src.first().and_then(sg).ok_or("s_buffer_load: bad base")?;
                    let d = ins.dst.first().and_then(sreg_num).map(u16::from).ok_or("s_buffer_load: bad dst")?;
                    if let Some(raw) = self.mesh_raw_descriptor(sym, b)? {
                        self.res.insert(idx, vec![raw]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    if heap_descriptor(sym, b, 4) {
                        self.res.insert(idx, vec![Res::HeapBuffer(b)]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    if let Some(Sym::DirectRaw(slot)) = sym.get(&b) {
                        self.res.insert(idx, vec![Res::Raw(*slot, false)]);
                        for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                        continue;
                    }
                    // A V# built inline from a root descriptor's GPU address (FSSM class 0x100
                    // row at that user SGPR, e.g. VS_LinearDepth's s[11:12]) reads the raw SRV
                    // that PSV declares at the row's register.
                    if let Some(Sym::Table(t)) = sym.get(&b).copied() {
                        if let Some(raw) = self.root_raw_srv(t) {
                            self.res.insert(idx, vec![raw]);
                            for k in d..d+n as u16 { sym.remove(&k); written.insert(k); }
                            continue;
                        }
                    }
                    // CBV descriptors sit on a 32-byte stride (cb1 is loaded from table offset
                    // 0x20 in MainOnePassPointLightPS).
                    let cands = self.desc_candidates(sym.get(&b)).ok_or_else(|| {
                        format!("s_buffer_load at {:#x}: base s{b} is not a buffer descriptor ({:?})", ins.offset, sym.get(&b))
                    })?;
                    // A dynamic SOFFSET register indexes the buffer at run time: size the
                    // cbuffer declaration to the D3D12 maximum (64 KiB = 4096 vec4).
                    let dynamic = (ins.raw.get(1).copied().unwrap_or(0) >> 25) != 0x7d;
                    let end = if dynamic { 4096 * 4 } else { ins.aux.imm as u32 / 4 + n };
                    let mut out: Vec<(u32, Res)> = Vec::new();
                    // A scalar load reads whatever buffer the descriptor names. From the CBV
                    // table that is a constant buffer; from the SRV / UAV table it is a
                    // (structured) buffer addressed by byte offset, e.g. the sky-light
                    // spherical-harmonics buffer read by ReflectionEnvironmentSkyLighting.
                    // Only the unambiguous case is handled; with several candidate
                    // descriptors the load is still treated as a constant buffer (debt).
                    for (table, off) in &cands {
                        let (kind, slot) = self.table_binding(*table, *off, 32)?;
                        let buffer_kind = kind.filter(|k| *k <= 1 && cands.len() == 1);
                        if let Some(k) = buffer_kind {
                            if let Some(raw) = self.raw_binding(slot, k == 1) {
                                out.push((((*table as u32) << 24) | off, raw));
                                continue;
                            }
                            let key = (k == 1, slot);
                            let e = self.sbuf.entry(key).or_insert(0);
                            *e = (*e).max(4);
                            self.sbuf_flat.insert(key);
                            out.push((((*table as u32) << 24) | off, Res::Buf(slot, k == 1)));
                            continue;
                        }
                        let e = self.cb_size.entry(slot).or_insert(0);
                        *e = (*e).max(end);
                        out.push((((*table as u32) << 24) | off, Res::Cb(slot)));
                    }
                    self.res.insert(idx, vec![out[0].1]);
                    if out.len() > 1 {
                        self.bsel.insert(idx, (b, out));
                    }
                    for k in 0..n {
                        sym.remove(&(d + k as u16));
                        written.insert(d + k as u16);
                    }
                }
                Enc::Mtbuf => {
                    let rs = ins.src.get(1).and_then(sg).ok_or("mtbuf: bad srsrc")?;
                    if self.compute_heap && heap_descriptor(sym, rs, 4) {
                        self.res.insert(idx, vec![Res::HeapRaw(rs)]);
                        continue;
                    }
                    let root_raw = match sym.get(&rs).copied() {
                        Some(Sym::Table(t)) if self.mesh_bindings.is_none() => self.root_raw_srv(t),
                        _ => None,
                    };
                    let raw = match root_raw {
                        Some(raw) => raw,
                        None => self.mesh_raw_descriptor(sym, rs)?
                            .ok_or("typed buffer load requires a verified linear root SRV")?,
                    };
                    self.res.insert(idx, vec![raw]);
                }
                // buffer_gl0_inv / buffer_gl1_inv (MUBUF 0x71 / 0x72) have no resource operands.
                Enc::Mubuf if ins.op == 0x71 || ins.op == 0x72 => {}
                Enc::Mubuf => {
                    let rs = ins.src.get(1).and_then(sg).ok_or("mubuf: bad srsrc")?;
                    // Never retain a provisional proof from another CFG visit.
                    self.raw_byte_index.remove(&ins.offset);
                    if let Some(raw) = self.mesh_raw_descriptor(sym, rs)? {
                        self.raw_byte_index.insert(ins.offset);
                        self.res.insert(idx, vec![raw]);
                        continue;
                    }
                    if let Some(Sym::DirectRaw(slot)) = sym.get(&rs) {
                        if raw_index::local_byte_descriptor(&insns[block_start..idx], rs) {
                            self.raw_byte_index.insert(ins.offset);
                        }
                        self.res.insert(idx, vec![Res::Raw(*slot, false)]);
                        continue;
                    }
                    if self.compute_heap && heap_descriptor(sym, rs, 4) {
                        self.res.insert(idx, vec![Res::HeapRaw(rs)]);
                        continue;
                    }
                    if let Some(Sym::Table(t)) = sym.get(&rs).copied() {
                        if let Some(raw) = self.root_raw_srv(t) {
                            self.res.insert(idx, vec![raw]);
                            continue;
                        }
                    }
                    let cands = self.desc_candidates(sym.get(&rs)).ok_or_else(|| {
                        format!("mubuf at {:#x}: s{rs} not a buffer descriptor ({:?})", ins.offset, sym.get(&rs))
                    })?;
                    let mut out: Vec<(u32, Res)> = Vec::new();
                    for (table, off) in &cands {
                        out.push((((*table as u32) << 24) | off, self.mubuf_res(ins, *off, *table)?));
                    }
                    self.res.insert(idx, vec![out[0].1]);
                    if out.len() > 1 {
                        self.bsel.insert(idx, (rs, out));
                    }
                }
                Enc::Mimg if matches!(ins.op, 230 | 231) => {
                    return Err(format!(
                        "raw BVH node intersection requires traversal translation at {:#x}",
                        ins.offset
                    ));
                }
                Enc::Mimg => {
                    let n = ins.src.len();
                    let rs = ins.src.get(n - 2).and_then(sg).ok_or("mimg: bad srsrc")?;
                    let ss = ins.src.get(n - 1).and_then(sg).ok_or("mimg: bad ssamp")?;
                    let dim = ins.aux.imm as u32;
                    let mut tex_res: Vec<(u32, Res)> = Vec::new();
                    if heap_descriptor(sym, rs, 8) {
                        let load = self.compute_heap && matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip")
                            && matches!(dim, 0 | 1 | 2 | 4 | 5);
                        // The heap does not name a view's component type. Sampling needs a
                        // float view; a texel load does not (see `heap_load_ty`).
                        let cube = self.compute_heap && dim == 3;
                        let size = self.compute_heap && dim == 1 && ins.mnemonic == "image_get_resinfo";
                        let lod = self.pixel_heap && dim == 1 && ins.mnemonic == "image_get_lod";
                        // A texel store passes its registers on unconverted, whatever the view.
                        let store = ins.mnemonic == "image_store" && dim == 1 && self.compute_heap
                            && matches!(self.ex.stage, Stage::Compute | Stage::Pixel);
                        if !load && !store && !lod && !size && (!(ins.mnemonic.starts_with("image_sample")
                            || matches!(ins.mnemonic.as_str(), "image_gather4_lz" | "image_gather4_lz_o")) || !(matches!(dim, 1 | 5) || cube)) {
                            return Err(format!("unsupported direct heap texture operation {} dim {dim}", ins.mnemonic));
                        }
                        tex_res.push((0, Res::HeapTexture(rs, dim)));
                    } else {
                        let tex_c = self.desc_candidates(sym.get(&rs)).ok_or_else(|| {
                            format!("mimg at {:#x}: s{rs} not an image descriptor ({:?})", ins.offset, sym.get(&rs))
                        })?;
                        for (ttable, off) in &tex_c {
                            let (kind, tslot) = self.table_binding(*ttable, *off, 32)?;
                            let is_uav = kind == Some(1);
                            if is_uav && ins.mnemonic.starts_with("image_atomic_") {
                                self.uav_uint.insert(tslot);
                            }
                            let map = if is_uav { &mut self.uav } else { &mut self.tex };
                            if let Some(prev) = map.insert(tslot, dim) {
                                if prev != dim {
                                    return Err(format!("image slot {tslot} used with two dimensions"));
                                }
                            }
                            tex_res.push((((*ttable as u32) << 24) | off, if is_uav { Res::Uav(tslot, dim) } else { Res::Tex(tslot, dim) }));
                        }
                    }
                    let mut samp_res: Vec<(u32, Res)> = Vec::new();
                    if ins.mnemonic.starts_with("image_sample") || ins.mnemonic.starts_with("image_gather") || ins.mnemonic == "image_get_lod" {
                        if let Some(candidates) = self.desc_candidates(sym.get(&ss)) {
                            for (table, offset) in candidates {
                                if Some(table) == self.sampler_heap_base {
                                    // Static index into the sampler descriptor heap.
                                    if offset % 16 != 0 {
                                        return Err("unaligned sampler heap offset".into());
                                    }
                                    self.heap_sampler_slots.insert(offset / 16);
                                    samp_res.push((((table as u32) << 24) | offset, Res::Samp(offset / 16)));
                                    continue;
                                }
                                let (_, slot) = self.table_binding(table, offset, 16)?;
                                self.table_samplers.insert(slot);
                                samp_res.push((((table as u32) << 24) | offset, Res::Samp(slot)));
                            }
                        } else if self.chosen_samplers.contains(&idx)
                            || matches!(sym.get(&ss), Some(Sym::HeapSamplerIndex(_))) && heap_words(sym, ss, 4)
                        {
                            self.uses_heap_sampler = true;
                            samp_res.push((0, Res::HeapSamplerDyn(ss)));
                        } else if let Some(Sym::HeapSampler(index)) = sym.get(&ss) {
                            self.uses_heap_sampler = true;
                            samp_res.push((0, Res::HeapSampler(*index)));
                        } else if let Some((slot, space)) = if let Some(&slot) = self.private_samplers.get(&ins.offset) {
                            Some((slot, 32765))
                        } else if inline_samplers {
                            let word = |n| match sym.get(&(ss + n)) {
                                Some(Sym::InlineWord(w)) => Some(*w),
                                _ => None,
                            };
                            let words = (|| Some([word(0)?, word(1)?, word(2)?, word(3)?]))().or_else(|| {
                                (self.ex.stage == Stage::Compute).then(|| sampler_heap::guarded_words(insns, idx, ss as u8, compute_group(self.ex))).flatten()
                            });
                            words.and_then(|w| sampler_heap::inline_sampler(self.ex, w, ins.op & 8 != 0))
                                .or_else(|| self.static_sampler())
                        } else { self.static_sampler() } {
                            if self.binding_spaces.insert((3, slot), space).is_some_and(|old| old != space) {
                                return Err(format!("static sampler s{slot} overlaps another space"));
                            }
                            self.table_samplers.insert(slot);
                            samp_res.push((0, Res::Samp(slot)));
                        } else {
                            let words: Vec<_> = (0..4).map(|n| match sym.get(&(ss + n)) { Some(Sym::InlineWord(w)) => format!("{w:#x}"), other => format!("{other:?}") }).collect();
                            return Err(format!("mimg at {:#x}: s{ss} not a sampler descriptor ({:?}) words [{}]", ins.offset, sym.get(&ss), words.join(", ")));
                        }
                        for &(_, sampler) in &samp_res {
                            let Res::Samp(sslot) = sampler else { continue };
                            // Depth-compare variants (opcode bit 3) need a comparison sampler.
                            if ins.op & 8 != 0 {
                                self.samp_cmp.insert(sslot);
                            } else {
                                self.samp.insert(sslot);
                            }
                        }
                    }
                    let mut v = vec![tex_res[0].1];
                    if let Some(s) = samp_res.first() {
                        v.push(s.1);
                    }
                    if tex_res.len() > 1 || samp_res.len() > 1 {
                        self.msel.insert(idx, MSel { tex_reg: rs, samp_reg: ss, tex: tex_res, samp: samp_res });
                    }
                    self.res.insert(idx, v);
                }
                Enc::Vop1 if ins.mnemonic == "v_nop" => {}

                _ => {
                    for d in &ins.dst {
                        if let Opnd::Vgpr(v) = d {
                            sym.remove(&(2000 + *v as u16));
                            if float64::pair_result(&ins.mnemonic) {
                                sym.remove(&(2001 + *v as u16));
                            }
                        }
                        if let Some(n) = sg(d) {
                            sym.remove(&n);
                            written.insert(n);
                            if wide_bit_result(&ins.mnemonic) {
                                sym.remove(&(n + 1));
                                written.insert(n + 1);
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
