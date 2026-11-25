//! Facts gathered over the whole program before emission.
use super::*;

impl<'a> Emitter<'a> {
    /// Standard adjacent P1/P2 pairs consume the rasterizer's interpolant. A shader
    /// that reads the partial term or provides its own weights needs the vertex values.
    pub(in crate::hlsl) fn classify_interpolation(&mut self) {
        let mut smooth = BTreeSet::new();
        // A branch into P2 bypasses P1. After a back edge the input registers
        // may already contain values written later in the instruction stream.
        let writes_weight =
            |ins: &Insn| ins.mnemonic != "v_nop" && ins.dst.iter().any(|o| matches!(o, Opnd::Vgpr(0 | 1)));
        let targets: BTreeSet<_> = self
            .insns
            .iter()
            .filter(|i| i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some())
            .map(|i| i.offset as i64 + 4 + 4 * i.aux.imm)
            .collect();
        let has_back_edge = self
            .insns
            .iter()
            .any(|i| i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some() && i.aux.imm < 0);
        let mut original = [!(has_back_edge && self.insns.iter().any(writes_weight)); 2];
        let mut original_weights = !(has_back_edge && self.insns.iter().any(writes_weight));
        let mut i = 0;
        while i < self.insns.len() {
            let ins = &self.insns[i];
            if ins.enc == Enc::Vop3 && matches!(ins.mnemonic.as_str(), "v_interp_p1ll_f16" | "v_interp_p1lv_f16" | "v_interp_p2_f16") && ins.raw.len() > 1 {
                // The rasterizer's own weights give its interpolant; others need the vertex values.
                let own = if ins.mnemonic == "v_interp_p2_f16" { 1 } else { 0 };
                if ins.src.get(1) != Some(&Opnd::Vgpr(own)) || !original[usize::from(own)] {
                    self.pulled_attrs.insert(Self::f16_interp_descriptor(ins.raw[1]).0);
                }
            }
            if ins.enc == Enc::Vinterp {
                let attr = ins.aux.imm as usize;
                if ins.op == 2 && ins.raw[0] & 255 == 2 {
                    self.flat_attrs.insert(attr);
                } else {
                    smooth.insert(attr);
                    let paired = ins.op == 0
                        && ins.src == [Opnd::Vgpr(0)]
                        && original_weights
                        && self.insns.get(i + 1).is_some_and(|p| {
                            p.enc == Enc::Vinterp
                                && p.op == 1
                                && p.src == [Opnd::Vgpr(1)]
                                && !targets.contains(&(p.offset as i64))
                                && p.dst == ins.dst
                                && p.aux.imm == ins.aux.imm
                                && p.aux.imm2 == ins.aux.imm2
                                && ins.dst != [Opnd::Vgpr(1)]
                        });
                    if paired {
                        i += 1;
                    } else {
                        self.pulled_attrs.insert(attr);
                    }
                }
            }
            if writes_weight(ins) {
                original_weights = false;
                for (n, kept) in original.iter_mut().enumerate() {
                    *kept &= !ins.dst.contains(&Opnd::Vgpr(n as u8));
                }
            }
            i += 1;
        }
        self.pulled_attrs.extend(self.flat_attrs.intersection(&smooth));
    }

    /// Element type of each typed buffer, inferred from how the shader uses the values it
    /// loads from (or produces for) the buffer. The buffer view's format is not part of the
    /// shader: the hardware delivers floats for float/normalised formats and integers for
    /// integer formats, and the compiled code simply goes on to treat the registers as one
    /// or the other. HLSL needs the element type declared, so it is recovered from the
    /// first typed uses: conversions from / comparisons as / arithmetic on integers or
    /// floats, and use as a memory index. Bitwise logic and moves are followed, not counted.
    /// Slots with no evidence, or more float than integer evidence, stay float.
    pub(in crate::hlsl) fn infer_tbuf_types(&mut self) {
        // 0 float, 1 uint, 2 sint
        fn class(mn: &str) -> Option<u8> {
            let m = mn.strip_suffix("_e64").unwrap_or(mn);
            if let Some(rest) = m.strip_prefix("v_cvt_") {
                let last = rest.rsplit('_').next().unwrap_or("");
                return Some(if last.starts_with('f') { 0 } else if last.starts_with('i') { 2 } else { 1 });
            }
            if m.starts_with("v_cmp") {
                // The operand type is the last component (`v_cmp_f_i32` is an integer compare
                // whose condition is "false").
                let ty = m.rsplit('_').next().unwrap_or("");
                return if ty.starts_with('f') { Some(0) } else if ty.starts_with('i') { Some(2) } else if ty.starts_with('u') { Some(1) } else { None };
            }
            const SINT: [&str; 7] = ["v_ashr", "v_max_i", "v_min_i", "v_bfe_i", "v_mul_i32", "v_mad_i32", "v_mul_hi_i"];
            const UINT: [&str; 14] = [
                "v_lshl", "v_lshr", "v_bfe_u", "v_mul_lo", "v_mul_hi_u", "v_mul_u32", "v_mad_u32", "v_add_nc", "v_sub_nc", "v_subrev_nc", "v_add_co", "v_sub_co",
                "v_max_u", "v_min_u",
            ];
            if SINT.iter().any(|p| m.starts_with(p)) {
                return Some(2);
            }
            if UINT.iter().any(|p| m.starts_with(p)) {
                return Some(1);
            }
            if m.starts_with("v_") && (m.contains("_f32") || m.contains("_f16") || m.contains("_f64")) {
                return Some(0);
            }
            None
        }
        // Bitwise logic with a small constant (`x & 1`, `x & 0xf`, `x ^ 1`) is integer work:
        // the masks applied to float bit patterns are sign / exponent / mantissa masks, all
        // far larger.
        fn small_mask(u: &Insn) -> bool {
            ["v_and_b32", "v_or_b32", "v_xor_b32"].iter().any(|p| u.mnemonic.starts_with(p))
                && u.src.iter().any(|o| match o {
                    Opnd::ConstInt(c) => *c > 0 && *c <= 64,
                    Opnd::Literal(l) => *l > 0 && *l <= 0xffff,
                    _ => false,
                })
        }
        fn follows(mn: &str) -> bool {
            ["v_mov_b32", "v_and_b32", "v_or_b32", "v_xor_b32", "v_cndmask_b32", "v_not_b32"].iter().any(|p| mn.starts_with(p))
        }
        // Keys are (is image, is_uav, slot): typed buffers and textures are told apart.
        let mut votes: BTreeMap<(bool, bool, u32), [u32; 3]> = BTreeMap::new();
        // (stored-to resource, loaded-from resource): a store of registers loaded from another
        // typed resource says both hold the same kind of element.
        let mut links: Vec<((bool, bool, u32), (bool, bool, u32))> = Vec::new();
        // Textures that are sampled or gathered hold filterable (float) data.
        let mut sampled: BTreeSet<u32> = BTreeSet::new();
        let mut uniform_uint = BTreeMap::new();
        let targets = uniform_texel::branch_targets(&self.insns);
        let n = self.insns.len();
        // The typed resource an instruction reads or writes, with the data registers.
        let typed = |em: &Self, i: usize| -> Option<((bool, bool, u32), BTreeSet<u8>, bool, bool)> {
            let ins = &em.insns[i];
            let Some(&Opnd::Vgpr(vd)) = ins.dst.first() else { return None };
            match em.res.get(&i).and_then(|v| v.first()).copied()? {
                Res::TBuf(slot, is_uav) => {
                    let comps = match ins.mnemonic.rsplit('_').next() {
                        Some("x") => 1u8,
                        Some("xy") => 2,
                        Some("xyz") => 3,
                        Some("xyzw") => 4,
                        _ => return None,
                    };
                    let registers = if ins.mnemonic.contains("_d16_") { comps.div_ceil(2) } else { comps };
                    let regs = (0..registers).map(|k| vd.wrapping_add(k)).collect();
                    Some(((false, is_uav, slot), regs, ins.mnemonic.starts_with("buffer_load"), ins.mnemonic.starts_with("buffer_store")))
                }
                r @ (Res::Tex(..) | Res::Uav(..)) => {
                    let (slot, is_uav) = match r {
                        Res::Tex(s, _) => (s, false),
                        Res::Uav(s, _) => (s, true),
                        _ => unreachable!(),
                    };
                    let regs = (0..image_data_registers(ins)).map(|k| vd.wrapping_add(k)).collect();
                    let load = matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip");
                    Some(((true, is_uav, slot), regs, load, ins.mnemonic == "image_store"))
                }
                _ => None,
            }
        };
        for i in 0..n {
            if let Some(Res::Tex(slot, _)) = self.res.get(&i).and_then(|v| v.first()).copied() {
                let m = &self.insns[i].mnemonic;
                if m.starts_with("image_sample") || m.starts_with("image_gather") {
                    sampled.insert(slot);
                }
            }
            let Some((key, regs, is_load, is_store)) = typed(self, i) else { continue };
            if is_load && key.0 && !key.1 {
                let packed = uniform_texel::packed_uint(&self.insns[i], &self.insns[i + 1..], &targets);
                uniform_uint.entry(key.2).and_modify(|all| *all &= packed).or_insert(packed);
            }
            let v = votes.entry(key).or_insert([0; 3]);
            if is_load {
                // Forward: how are the loaded registers (and plain copies / masks of them) used?
                let mut live = regs;
                for j in i + 1..n {
                    if live.is_empty() {
                        break;
                    }
                    let u = &self.insns[j];
                    // The scan is linear: past an unconditional branch (or the program end)
                    // the following code belongs to another path.
                    if u.enc == Enc::Sopp && (u.op == 1 || u.op == 2) {
                        break;
                    }
                    let uses = u.src.iter().any(|o| matches!(o, Opnd::Vgpr(r) if live.contains(r)));
                    let mem_index = matches!(u.enc, Enc::Mubuf | Enc::Mimg | Enc::Ds) && matches!(u.src.first(), Some(Opnd::Vgpr(r)) if live.contains(r));
                    if mem_index && u.enc == Enc::Mimg && (u.mnemonic.starts_with("image_sample") || u.mnemonic.starts_with("image_gather")) {
                        // Sampling coordinates are floats; only texel fetches index by integer.
                        v[0] += 1;
                    } else if mem_index && !(u.mnemonic.contains("store") && matches!(u.enc, Enc::Mubuf)) {
                        v[1] += 1;
                    } else if uses && !self.epilog_pack.contains_key(&j) {
                        // (The export tail packs whatever type the output has: no evidence.)
                        if let Some(c) = class(&u.mnemonic) {
                            v[c as usize] += 1;
                        } else if small_mask(u) {
                            v[1] += 1;
                        }
                    }
                    let carry = uses && follows(&u.mnemonic);
                    for d in &u.dst {
                        if let Opnd::Vgpr(r) = d {
                            if carry {
                                live.insert(*r);
                            } else if !matches!(u.enc, Enc::Mubuf | Enc::Ds | Enc::Exp) || u.mnemonic.contains("load") || u.mnemonic.contains("read") {
                                live.remove(r);
                            }
                        }
                    }
                }
            } else if is_store {
                // Backward: what produced each stored register?
                for r in regs {
                    for j in (0..i).rev() {
                        let d = &self.insns[j];
                        if d.dst.iter().any(|o| matches!(o, Opnd::Vgpr(x) if *x == r)) {
                            if let Some((k2, _, true, _)) = typed(self, j) {
                                links.push((key, k2));
                                break;
                            }
                            // A conversion's result type is its first type token.
                            let c = match d.mnemonic.strip_prefix("v_cvt_") {
                                Some(rest) => Some(if rest.starts_with('f') { 0 } else if rest.contains("i32") || rest.contains("i16") { 2 } else { 1 }),
                                None => class(&d.mnemonic),
                            };
                            if let Some(c) = c {
                                v[c as usize] += 1;
                            }
                            break;
                        }
                    }
                }
            }
        }
        let mut ty: BTreeMap<(bool, bool, u32), u8> = BTreeMap::new();
        for (k, v) in &votes {
            let ints = v[1] + v[2];
            if ints > v[0] {
                ty.insert(*k, if v[2] > 0 { 2 } else { 1 });
            }
        }
        for (slot, packed) in uniform_uint {
            let key = (true, false, slot);
            if packed && votes.get(&key) == Some(&[0; 3]) && !sampled.contains(&slot) {
                ty.insert(key, 1);
            }
        }
        for (to, from) in links {
            if votes.get(&to).map(|v| v.iter().all(|&x| x == 0)).unwrap_or(true) {
                if let Some(&t) = ty.get(&from) {
                    ty.insert(to, t);
                }
            }
        }
        // Typed loads through the heap, per instruction: a vertex program fetches its index
        // buffer this way. Every path must agree.
        self.heap_load_ty = (0..n)
            .filter(|&i| (self.ex.stage == Stage::Vertex || matches!(self.ex.stage, Stage::Pixel | Stage::Compute) && self.insns[i].enc == Enc::Mimg) && match self.res.get(&i).and_then(|v| v.first()) {
                Some(Res::HeapRaw(_)) => self.insns[i].mnemonic.starts_with("buffer_load_format_"),
                Some(Res::HeapTexture(..)) => matches!(self.insns[i].mnemonic.as_str(), "image_load" | "image_load_mip"),
                _ => false,
            })
            .filter_map(|i| {
                let Some(&Opnd::Vgpr(vd)) = self.insns[i].dst.first() else { return None };
                let mut seen = [false; 3];
                let at: HashMap<i64, usize> = self.insns.iter().enumerate().map(|(k, x)| (x.offset as i64, k)).collect();
                let channels = match self.insns[i].enc {
                    Enc::Mimg => image_data_registers(&self.insns[i]),
                    _ => self.insns[i].mnemonic.rsplit('_').next().map_or(4, str::len) as u8,
                };
                let mut work: Vec<(usize, u8)> = (0..channels).map(|k| (i + 1, vd.wrapping_add(k))).collect();
                let mut visited: BTreeSet<(usize, u8)> = work.iter().copied().collect();
                while let Some((j, reg)) = work.pop() {
                    let Some(u) = self.insns.get(j) else { continue };
                    let uses = u.src.iter().any(|o| *o == Opnd::Vgpr(reg));
                    let address = u.src.first() == Some(&Opnd::Vgpr(reg));
                    // The first typed use on a path decides it. Later ones can be reached
                    // through registers the compiler reused for other values.
                    if uses && u.mnemonic != "v_nop" {
                        let verdict = match class(&u.mnemonic) {
                            Some(c) => Some(usize::from(c)),
                            None if small_mask(u) || (address && matches!(u.enc, Enc::Mubuf | Enc::Mtbuf)) => Some(1),
                            None if follows(&u.mnemonic) => None,
                            None if u.mnemonic.ends_with("_u32") => Some(1),
                            // An export or an operation without a type proves nothing.
                            None => Some(0),
                        };
                        if let Some(c) = verdict {
                            seen[c] = true;
                            continue;
                        }
                    }
                    let mut next = vec![];
                    if u.enc == Enc::Sopp {
                        let target = at.get(&(u.offset as i64 + 4 + u.aux.imm * 4)).copied();
                        if Self::branch_cond(u.op).is_some() || u.op == 2 { next.extend(target); }
                        if u.op != 1 && u.op != 2 { next.push(j + 1); }
                    } else {
                        next.push(j + 1);
                    }
                    let mut regs = vec![];
                    let width = match u.enc {
                        Enc::Mimg => image_data_registers(u),
                        Enc::Mtbuf => (u.op as u8 & 3) + 1,
                        Enc::Mubuf => match u.mnemonic.rsplit('_').next() {
                            Some(x @ ("x" | "xy" | "xyz" | "xyzw")) => x.len() as u8,
                            Some("dwordx2") => 2,
                            Some("dwordx3") => 3,
                            Some("dwordx4") => 4,
                            _ => 1,
                        },
                        _ => 1,
                    };
                    let writes = u.mnemonic != "v_nop" && !u.mnemonic.contains("store")
                        && u.dst.iter().any(|o| matches!(o, Opnd::Vgpr(d) if (*d..d.saturating_add(width)).contains(&reg)));
                    if !writes { regs.push(reg); }
                    if uses && follows(&u.mnemonic) {
                        regs.extend(u.dst.iter().filter_map(|o| match o { Opnd::Vgpr(r) => Some(*r), _ => None }));
                    }
                    for k in next {
                        for &r in &regs {
                            if visited.insert((k, r)) { work.push((k, r)); }
                        }
                    }
                }
                Some((i, seen))
            })
            .collect();
        for ((is_img, is_uav, slot), t) in ty {
            if !is_img {
                self.tbuf_ty.insert((is_uav, slot), t);
            } else if is_uav {
                // Textures used with image atomics already have their own uint declaration.
                if !self.uav_uint.contains(&slot) {
                    self.img_ty.insert((true, slot), t);
                }
            } else if !sampled.contains(&slot) {
                self.img_ty.entry((false, slot)).or_insert(t);
            }
        }
    }

    /// Recognise the colour-export epilogue of a pixel shader.
    ///
    /// Every colour export in the title's pixel shaders has the same shape, whatever the
    /// HLSL output type:
    ///
    /// ```text
    /// s_nop ; s_nop ; v_cvt_pkrtz_f16_f32 vA, x, y ; v_cvt_pkrtz_f16_f32 vB, z, w ; exp mrtN vA, vB (compressed)
    /// ```
    ///
    /// including shaders whose output is `uint4` (the lighting-channel copy writes an R16_UINT
    /// target this way, where a float-to-half conversion of the integer would always give 0).
    /// The export format belongs to the render target, which the shader compiler does not
    /// know, so this tail can only be a default that the console's pipeline creation replaces
    /// per target format (the two leading no-ops are spare room for it). That replacement is
    /// inferred, not observed. What the shader itself computes is the four unpacked operands
    /// (x, y, z, w), so those are exported directly: as integers for an integer output
    /// element, as full floats otherwise (the render-target write then converts to the
    /// target's own format, as a per-format epilogue would).
    ///
    /// Records the pack instructions whose operands must be captured and, per export, which
    /// captures feed it. An export is left alone unless both packed operands come from pack
    /// instructions directly before it with nothing but no-ops in between and no branch
    /// landing inside the sequence.
    pub(in crate::hlsl) fn find_export_epilogs(&mut self) {
        let mut off2idx: HashMap<usize, usize> = HashMap::new();
        for (i, ins) in self.insns.iter().enumerate() {
            off2idx.insert(ins.offset, i);
        }
        let mut targets: BTreeSet<usize> = BTreeSet::new();
        for ins in &self.insns {
            if ins.enc == Enc::Sopp && Self::branch_cond(ins.op).is_some() {
                let t = ins.offset as i64 + 4 + ins.aux.imm * 4;
                if t >= 0 {
                    if let Some(&ti) = off2idx.get(&(t as usize)) {
                        targets.insert(ti);
                    }
                }
            }
        }
        let mut next_id = 0usize;
        for idx in 0..self.insns.len() {
            let ins = &self.insns[idx];
            if ins.enc != Enc::Exp || ins.aux.imm2 & 1 == 0 || ins.aux.imm > 7 {
                continue;
            }
            let (Some(&Opnd::Vgpr(r0)), Some(&Opnd::Vgpr(r1))) = (ins.src.first(), ins.src.get(1)) else { continue };
            let en = ins.aux.mask;
            let (need0, need1) = (en & 0x3 != 0, en & 0xc != 0);
            let (mut p0, mut p1) = (None, None);
            let mut first = idx;
            for j in (idx.saturating_sub(8)..idx).rev() {
                let u = &self.insns[j];
                if matches!(u.mnemonic.as_str(), "s_nop" | "s_waitcnt") {
                    continue;
                }
                if u.mnemonic != "v_cvt_pkrtz_f16_f32" {
                    break;
                }
                let Some(&Opnd::Vgpr(d)) = u.dst.first() else { break };
                if d == r0 && p0.is_none() {
                    p0 = Some(j);
                    first = j;
                }
                if d == r1 && p1.is_none() {
                    p1 = Some(j);
                    first = j;
                }
            }
            if (need0 && p0.is_none()) || (need1 && p1.is_none()) {
                continue;
            }
            if targets.range(first + 1..=idx).next().is_some() {
                continue;
            }
            let mut id_of = |em: &mut Self, p: Option<usize>, need: bool| -> usize {
                match p {
                    Some(j) if need => *em.epilog_pack.entry(j).or_insert_with(|| {
                        next_id += 1;
                        next_id - 1
                    }),
                    _ => usize::MAX,
                }
            };
            let a = id_of(self, p0, need0);
            let b = id_of(self, p1, need1);
            self.epilog_exp.insert(idx, (a, b));
        }
    }

    /// A two-word, non-returning IMAGE_ATOMIC_UMAX addresses one uint64 texel.
    /// Use that declaration only when every access to its typed UAV agrees.
    pub(in crate::hlsl) fn resolve_wide_image_atomics(&mut self) -> Result<(), String> {
        let mut wide = BTreeSet::new();
        let mut other = BTreeSet::new();
        for (&i, resources) in &self.res {
            let op = &self.insns[i];
            if op.enc != Enc::Mimg {
                if let Some((_, candidates)) = self.bsel.get(&i) {
                    for (_, resource) in candidates {
                        if let Res::Raw(slot, true) | Res::Buf(slot, true) | Res::TBuf(slot, true) = resource {
                            other.insert(*slot);
                        }
                    }
                }
                for resource in resources {
                    if let Res::Uav(slot, _) | Res::Raw(slot, true) | Res::Buf(slot, true)
                        | Res::TBuf(slot, true) = resource {
                        other.insert(*slot);
                    }
                }
                continue;
            }
            if let Some(selection) = self.msel.get(&i) {
                for (_, resource) in &selection.tex {
                    if let Res::Uav(slot, _) = resource {
                        other.insert(*slot);
                    }
                }
                continue;
            }
            let [Res::Uav(slot, 1)] = resources.as_slice() else {
                continue;
            };
            if wide_atomic_coordinates(op).is_ok()
                && self.ex.resources.iter().filter(|r| {
                    matches!(r.res_type, 6..=9)
                        && r.space == self.binding_spaces.get(&(1, *slot)).copied().unwrap_or(0)
                        && r.lower <= *slot && *slot <= r.upper
                }).count() == 1
                && self.ex.resources.iter().any(|r| {
                    (r.res_type, r.space, r.lower, r.upper) ==
                        (6, self.binding_spaces.get(&(1, *slot)).copied().unwrap_or(0), *slot, *slot)
                })
            {
                wide.insert(*slot);
            } else {
                other.insert(*slot);
            }
        }
        if !wide.is_disjoint(&other) || !self.wide_visibility_uavs.is_disjoint(&other) {
            return Err("mixed access widths on a raster image".into());
        }
        self.wide_visibility_uavs.extend(wide);
        Ok(())
    }

    /// Registers read before any write in program order — i.e. the shader's entry ABI
    /// (interpolation barycentrics, fragment position, user-data SGPRs, thread ids...).
    pub(in crate::hlsl) fn live_in(&self) -> Vec<String> {
        let mut written: BTreeSet<String> = BTreeSet::new();
        let mut live: BTreeSet<(u8, u32)> = BTreeSet::new();
        for ins in &self.insns {
            if matches!(ins.mnemonic.as_str(), "v_nop" | "s_setreg_imm32_b32") {
                continue;
            }
            // Accumulators and partial writes implicitly read the old destination.
            let keep_other_bits = ins.aux.sdwa.is_none_or(|sd| (sd >> 8) & 7 != 6 && (sd >> 11) & 3 == 2);
            let setreg = ins.mnemonic == "s_setreg_b32";
            let old_dst = ((setreg && modeled_hwreg_mask(ins.raw[0]) != 0)
                || matches!(ins.mnemonic.as_str(), "v_pk_fmac_f16" | "v_fma_f16" | "v_rcp_f16")
                || ((half_round_or_conversion(&ins.mnemonic) || ins.mnemonic == "v_cvt_f16_f32") && keep_other_bits)
                || int16::new_half(ins))
                .then(|| ins.dst.first()).flatten();
            for o in ins.src.iter().chain(old_dst) {
                if let Opnd::Sgpr(n) | Opnd::Vgpr(n) = o {
                    let nm = name(o).unwrap_or_default();
                    if !written.contains(&nm) {
                        live.insert((if matches!(o, Opnd::Vgpr(_)) { 1 } else { 0 }, *n as u32));
                    }
                }
            }
            if setreg {
                continue;
            }
            if let Some(high) = float64::pair_source_highs(ins) {
                if !written.contains(&format!("v{high}")) {
                    live.insert((1, high));
                }
            }
            let atomic = ins.mnemonic.starts_with("image_atomic_");
            if ins.mnemonic.starts_with("image_store") || atomic {
                if let Some(Opnd::Vgpr(base)) = ins.dst.first() {
                    for reg in u32::from(*base)..u32::from(*base) + u32::from(image_data_registers(ins)) {
                        if !written.contains(&format!("v{reg}")) {
                            live.insert((1, reg));
                        }
                    }
                }
                if !atomic || ins.raw[0] & (1 << 13) == 0 {
                    continue;
                }
            }
            // Multi-register destinations: SMEM, buffer and image loads, 64-bit scalar ops.
            let extra = match ins.enc {
                Enc::Smem => (1u32 << (ins.op & 7)) - 1,
                Enc::Mimg => u32::from(image_data_registers(ins)).saturating_sub(1),
                Enc::Mtbuf if ins.op < 4 => ins.op,
                Enc::Mubuf if ins.mnemonic.starts_with("buffer_load_dword") =>
                    Self::mubuf_dwords(&ins.mnemonic).unwrap_or(1) - 1,
                _ if wide_bit_result(&ins.mnemonic) => 1,
                _ if float64::pair_result(&ins.mnemonic) => 1,
                _ => 0,
            };
            for o in &ins.dst {
                if let Ok(nm) = name(o) {
                    written.insert(nm);
                }
                match o {
                    Opnd::Sgpr(n) => (1..=extra).for_each(|k| { written.insert(format!("s{}", *n as u32 + k)); }),
                    Opnd::Vgpr(n) => (1..=extra).for_each(|k| { written.insert(format!("v{}", *n as u32 + k)); }),
                    _ => {}
                }
            }
        }
        live.iter().map(|(k, n)| format!("{}{}", if *k == 1 { "v" } else { "s" }, n)).collect()
    }
}
