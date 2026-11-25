//! Image sampling, loads, stores and atomics.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn require_compute_derivatives(&mut self) -> Result<(), String> {
        if self.is_cs && !self.compute_derivatives {
            // SM 6.6 groups derivatives into runs of four X lanes or 2x2 XY quads.
            let [x, y, z] = self.ex.psv_threads.ok_or("missing compute derivative thread group")?;
            let linear = x % 4 == 0 && y == 1 && z == 1;
            let planar = x % 2 == 0 && y % 2 == 0;
            if x == 0 || y == 0 || z == 0 || !(linear || planar) {
                return Err(format!("compute derivatives need a quad-compatible thread group, got {x}x{y}x{z}"));
            }
            self.compute_derivatives = true;
        }
        Ok(())
    }

    pub(in crate::hlsl) fn emit_mimg(&mut self, idx: usize, ins: &Insn) -> Result<(), String> {
        if self.packed.as_ref().is_some_and(|p| p.wide_images.contains(&idx)) {
            return packed_root::ps09_raster::emit_wide_image(self, ins, "wide_u10");
        }
        let rs = self.res.get(&idx).cloned().ok_or("unresolved image")?;
        // Instruction bits that change operand or result layout (RDNA2 ISA Table 41; bit
        // positions from AMD's machine-readable RDNA2 spec): UNORM dword0[12], TFE [16],
        // LWE [17]; A16 dword1[30], D16 [31].
        let w0 = ins.raw.first().copied().unwrap_or(0);
        let w1 = ins.raw.get(1).copied().unwrap_or(0);
        let unorm = (w0 >> 12) & 1 != 0;
        let a16 = (w1 >> 30) & 1 != 0;
        let d16 = (w1 >> 31) & 1 != 0;
        if (w0 >> 16) & 3 != 0 {
            // TFE / LWE append a status dword (texel fault for partially resident textures,
            // LOD warning) after the data.
            return Err("MIMG TFE/LWE status return unsupported".into());
        }
        // Address operand `i` (in dwords, or 16-bit halves when `a16`) as an expression:
        // non-NSA addresses are consecutive VGPRs from vaddr, NSA lists each VGPR.
        let nsa = ins.aux.imm2 as usize;
        let vaddr = match ins.src[0] {
            Opnd::Vgpr(n) => n,
            _ => return Err("bad vaddr".into()),
        };
        let addr_reg = |i: usize| -> Opnd { if nsa > 0 { ins.src[i] } else { Opnd::Vgpr(vaddr + i as u8) } };
        // Texel-addressed ops (no sampler): image_load / image_store, on SRVs or UAVs.
        let atomic = match ins.mnemonic.as_str() {
            "image_atomic_add" => Some("InterlockedAdd"),
            "image_atomic_umin" => Some("InterlockedMin"),
            "image_atomic_umax" => Some("InterlockedMax"),
            "image_atomic_or" => Some("InterlockedOr"),
            _ => None,
        };
        let heap_load = match rs.first().copied() {
            Some(Res::HeapTexture(src, dim)) if matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip") => Some((src, dim)),
            _ => None,
        };
        if let (Some(Res::HeapTexture(src, 1)), "image_store", false) = (rs.first().copied(), ins.mnemonic.as_str(), d16) {
            let mut c = vec![];
            for i in 0..2 {
                let o = addr_reg(if a16 { i / 2 } else { i });
                self.touch(&o);
                c.push(if a16 { format!("(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2)) } else { r(&o)? });
            }
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
            let (mut comp, mut n) = (vec![], 0u8);
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    let o = Opnd::Vgpr(vd + n);
                    self.touch(&o);
                    comp.push(r(&o)?);
                    n += 1;
                } else {
                    comp.push("0u".into());
                }
            }
            let tex = self.bind_heap(HeapView::Tex2DRwUint, src);
            self.line(&format!("if (exec_lo & 1u) {tex}[uint2({})] = uint4({});", c.join(", "), comp.join(", ")));
            return Ok(());
        }
        if let Some((src, dim)) = heap_load {
            let seen = self.heap_load_ty.get(&idx).copied();
            let uint = self.compute_heap && seen == Some([false, true, false]) && dim == 1 && !d16;
            if !uint && self.pixel_heap && seen != Some([true, false, false]) {
                return Err("heap texel load of an unknown component type in a pixel shader".into());
            }
            if self.ex.stage == Stage::Vertex && seen.is_some_and(|seen| seen[1] || seen[2]) {
                return Err(format!("heap texel load at {:#x}: integer view unsupported", ins.offset));
            }
            // Texel load through a heap image descriptor (compute heap). Unless every use of
            // the texel takes an integer, a float view is assumed.
            let (view, ncoord) = match dim {
                1 if uint => (HeapView::Tex2DUint, 2),
                0 => (HeapView::Tex1D, 1), 1 => (HeapView::Tex2D, 2), 2 => (HeapView::Tex3D, 3),
                4 => (HeapView::Tex1DArray, 2), 5 => (HeapView::Tex2DArray, 3),
                _ => return Err(format!("heap texture load dim {dim} unsupported")),
            };
            let mut c = vec![];
            for i in 0..ncoord + usize::from(ins.mnemonic == "image_load_mip") {
                // With A16 each address is a 16-bit uint, two per VGPR.
                let o = addr_reg(if a16 { i / 2 } else { i });
                self.touch(&o);
                c.push(if a16 {
                    format!("(int)(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2))
                } else {
                    format!("(int){}", r(&o)?)
                });
            }
            if ins.mnemonic == "image_load" {
                c.push("0".into());
            }
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad vdata".into()) };
            let t = self.tmp;
            self.tmp += 1;
            let tex = self.bind_heap(view, src);
            let (ty, cast) = if uint { ("uint4", "") } else { ("float4", "asuint") };
            self.line(&format!("{ty} t{t} = {tex}.Load(int{}({}));", c.len(), c.join(", ")));
            if d16 {
                let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
                return self.write_d16(vd, &chans);
            }
            let mut n = 0u8;
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    self.wv(&Opnd::Vgpr(vd + n), &format!("{cast}(t{t}.{})", XYZW[ch]))?;
                    n += 1;
                }
            }
            return Ok(());
        }
        if atomic.is_some() || matches!(ins.mnemonic.as_str(), "image_load" | "image_load_mip" | "image_store" | "image_store_mip") {
            let with_mip = ins.mnemonic == "image_load_mip";
            let (slot, dim, is_uav) = match rs.first() {
                Some(Res::Tex(t, d)) => (*t, *d, false),
                Some(Res::Uav(t, d)) => (*t, *d, true),
                _ => return Err("unresolved image".into()),
            };
            // Integer address `i`: a dword, or with A16 a 16-bit uint, two per VGPR.
            let addr = |em: &mut Self, i: usize| -> Result<String, String> {
                if a16 {
                    let o = addr_reg(i / 2);
                    em.touch(&o);
                    Ok(format!("(({} >> {}u) & 0xffffu)", r(&o)?, 16 * (i % 2)))
                } else {
                    let o = addr_reg(i);
                    em.touch(&o);
                    r(&o)
                }
            };
            let ncoord = match dim {
                0 => 1,     // 1D
                1 => 2,     // 2D
                2 | 5 => 3, // 3D / 2D array
                4 => 2,     // 1D array
                6 => 2,     // 2D multisampled (the next component is the sample index)
                _ => return Err(format!("image dim {dim} unsupported for load/store")),
            };
            let store = ins.mnemonic.starts_with("image_store");
            if ins.mnemonic == "image_store_mip" {
                // Desktop UAV descriptors select one mip at view creation.
                if a16 || !self.immediate_zero_before(idx, addr_reg(ncoord)) {
                    return Err("image_store_mip requires a statically zero relative mip".into());
                }
            }
            let mut c = vec![];
            for i in 0..ncoord {
                c.push(addr(self, i)?);
            }
            let vd = match ins.dst[0] {
                Opnd::Vgpr(n) => n,
                _ => return Err("bad vdata".into()),
            };
            let as_uint = is_uav && self.uav_uint.contains(&slot);
            let ity = self.img_ty.get(&(is_uav, slot)).copied();
            // A packed zero stored to a scalar uint UAV survives either widening convention.
            let uint_zero = store && as_uint && ins.aux.mask == 1 && self.immediate_zero_before(idx, Opnd::Vgpr(vd));
            if d16 && !uint_zero && (as_uint || ity.is_some() || ins.mnemonic.starts_with("image_atomic_")) {
                // The conversion of integer formats to 16 bits is not specified.
                return Err("D16 image access to an integer-typed image unsupported".into());
            }
            if let Some(func) = atomic {
                // Image atomics → Interlocked* on a RWTexture<uint> texel; vdata carries the
                // operand in and the previous value out.
                if ins.aux.mask == 3 && as_uint && dim == 1 {
                    let visibility = matches!(self.ex.stage, Stage::Pixel | Stage::Compute)
                        && wide_atomic_coordinates(ins).is_ok()
                        && self.wide_visibility_uavs.contains(&slot);
                    if !visibility {
                        return Err("unknown two-word visibility atomic".into());
                    }
                    self.wide_visibility_uavs.insert(slot);
                    return packed_root::ps09_raster::emit_wide_image(self, ins, &format!("uav{slot}"));
                }
                if ins.aux.mask != 1 {
                    return Err("image atomic requires a verified two-word binding for DMASK=3 or DMASK=1 for one word".into());
                }
                if !as_uint {
                    return Err("image atomic on a non-UAV table".into());
                }
                let o = Opnd::Vgpr(vd);
                self.touch(&o);
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint t{t} = 0u;"));
                self.line(&format!("if (exec_lo & 1u) {func}(uav{slot}[uint{ncoord}({})], {}, t{t});", c.join(", "), r(&o)?));
                if ins.raw[0] & (1 << 13) != 0 { // GLC requests the old value in VDATA.
                    self.wv(&o, &format!("t{t}"))?;
                }
                return Ok(());
            }
            if store && as_uint {
                let o = Opnd::Vgpr(vd);
                self.touch(&o);
                self.line(&format!("if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = {};", c.join(", "), r(&o)?));
                return Ok(());
            }
            if ins.mnemonic == "image_load" && as_uint {
                let t = self.tmp;
                self.tmp += 1;
                self.line(&format!("uint4 t{t} = uint4(uav{slot}[uint{ncoord}({})], 0u, 0u, 0u);", c.join(", ")));
                let mut n = 0u8;
                for ch in 0..4 {
                    if (ins.aux.mask >> ch) & 1 != 0 {
                        self.wv(&Opnd::Vgpr(vd + n), &format!("t{t}.{}", XYZW[ch]))?;
                        n += 1;
                    }
                }
                return Ok(());
            }
            if store {
                if !is_uav {
                    return Err("image_store to a non-UAV table".into());
                }
                if let Some(t) = ity {
                    // Integer texels: the registers are stored as they are.
                    let mut comp = vec![];
                    let mut n = 0u8;
                    for ch in 0..4 {
                        if (ins.aux.mask >> ch) & 1 != 0 {
                            let o = Opnd::Vgpr(vd + n);
                            self.touch(&o);
                            comp.push(r(&o)?);
                            n += 1;
                        } else {
                            comp.push("0u".into());
                        }
                    }
                    let ctor = if t == 2 { "int4" } else { "uint4" };
                    self.line(&format!("if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = {ctor}({});", c.join(", "), comp.join(", ")));
                    return Ok(());
                }
                // Enabled components come from consecutive VGPRs (D16: halves of consecutive
                // VGPRs, low half first); missing components are stored as 0.
                let mut comp = vec![];
                let mut n = 0u8;
                for ch in 0..4 {
                    if (ins.aux.mask >> ch) & 1 != 0 {
                        let o = Opnd::Vgpr(vd + if d16 { n / 2 } else { n });
                        self.touch(&o);
                        comp.push(if d16 { format!("f16tof32({} >> {}u)", r(&o)?, 16 * (n % 2)) } else { f(&o)? });
                        n += 1;
                    } else {
                        comp.push("0.0".into());
                    }
                }
                self.line(&format!(
                    "if (exec_lo & 1u) uav{slot}[uint{ncoord}({})] = float4({});",
                    c.join(", "),
                    comp.join(", ")
                ));
            } else {
                let t = self.tmp;
                self.tmp += 1;
                // image_load_mip carries the mip level as one more address component.
                let mip = if with_mip { format!("(int){}", addr(self, ncoord)?) } else { "0".to_string() };
                let e = if dim == 6 {
                    if is_uav {
                        return Err("multisampled UAV load unsupported".into());
                    }
                    let sample = addr(self, ncoord)?;
                    format!("tex{slot}.Load(int2({}), (int){sample})", c.iter().map(|x| format!("(int){x}")).collect::<Vec<_>>().join(", "))
                } else if is_uav {
                    if with_mip {
                        return Err("image_load_mip on a UAV unsupported".into());
                    }
                    format!("uav{slot}[uint{ncoord}({})]", c.join(", "))
                } else {
                    format!("tex{slot}.Load(int{}({}, {mip}))", ncoord + 1, c.iter().map(|x| format!("(int){x}")).collect::<Vec<_>>().join(", "))
                };
                let ety = match ity {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                self.line(&format!("{ety} t{t} = {e};"));
                let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
                if d16 {
                    return self.write_d16(vd, &chans);
                }
                for (n, x) in chans.iter().enumerate() {
                    let e = match ety {
                        "float4" => format!("asuint({x})"),
                        "int4" => format!("(uint){x}"),
                        _ => x.clone(),
                    };
                    self.wv(&Opnd::Vgpr(vd + n as u8), &e)?;
                }
            }
            return Ok(());
        }
        let (tex, dim) = match rs.first() {
            Some(Res::Tex(slot, d)) => (format!("tex{slot}"), *d),
            Some(Res::HeapTexture(src, dim)) => {
                let (src, dim) = (*src, *dim);
                let view = match dim {
                    1 => HeapView::Tex2D,
                    3 => HeapView::TexCube,
                    _ => HeapView::Tex2DArray,
                };
                (self.bind_heap(view, src), dim)
            }
            _ => return Err("unresolved image".into()),
        };
        if ins.mnemonic == "image_get_resinfo" {
            if dim != 1 || a16 || d16 || ins.aux.mask & 4 != 0 {
                return Err("image_get_resinfo supports 2D width, height and mip count".into());
            }
            let t = self.tmp;
            self.tmp += 1;
            let Opnd::Vgpr(vd) = ins.dst[0] else { return Err("bad resinfo destination".into()) };
            self.line(&format!("uint w{t}, h{t}, levels{t}; {tex}.GetDimensions({}, w{t}, h{t}, levels{t});", r(&ins.src[0])?));
            let mut dst = vd;
            for (bit, field) in [(1,"w"), (2,"h"), (8,"levels")] {
                if ins.aux.mask & bit != 0 {
                    self.wv(&Opnd::Vgpr(dst), &format!("{field}{t}"))?;
                    dst = dst.wrapping_add(1);
                }
            }
            return Ok(());
        }
        let sslot = match rs.get(1) {
            Some(Res::Samp(s)) => Some(*s),
            Some(Res::HeapSampler(_) | Res::HeapSamplerDyn(_)) => None,
            _ => return Err("image op without sampler unsupported".into()),
        };
        // RDNA2 sample/gather family. Opcode = family base (0x20 sample, 0x40 gather4) +
        // modifier (0 none, 1 cl, 2 d, 3 d_cl, 4 l, 5 b, 6 b_cl, 7 lz) + 8 (depth compare)
        // + 0x10 (texel offset). Address operands arrive in this order:
        //   [offset] [bias] [zref] [ddx.. ddy..] coords.. [lod] [lod clamp]
        let op = ins.op;
        let gather = (0x40..0x60).contains(&op);
        if !gather && op != 0x60 && !(0x20..0x40).contains(&op) {
            return Err("unsupported image op".into());
        }
        let (modi, cmp, has_off) = (op & 7, op & 8 != 0, op & 0x10 != 0);
        // (coordinate count, derivative components, offset components)
        let (ncoord, nder, noff) = match dim {
            0 => (1usize, 1usize, 1usize),
            1 => (2, 2, 2),
            2 => (3, 3, 3),
            3 => (3, 3, 0),
            4 => (2, 1, 1),
            5 => (3, 2, 2),
            _ => return Err(format!("image dim {dim} unsupported")),
        };
        // Address operands. [offset] [bias] [zref] take a dword each, then the derivatives,
        // then coordinates, lod and clamp. With A16 (ISA Table 41: "PCF reference ... is
        // 32bit"; layout as Mesa's ACO emits it, aco_select_nir.cpp visit_tex /
        // emit_pack_v1): the bias is an f16 in the low half of its own dword, derivatives
        // stay 32-bit (16-bit derivatives are the separate _g16 opcodes), and coordinates,
        // lod and clamp are f16s packed two per VGPR, continuing across those operands.
        let mut next = 0usize;
        let mut take = |em: &mut Self| -> Opnd {
            let o = addr_reg(next);
            next += 1;
            em.touch(&o);
            o
        };
        // Texel offset: packed signed 6-bit fields (x [5:0], y [13:8], z [21:16]).
        let offset = if has_off {
            let o = take(self);
            if noff == 0 {
                return Err("texel offset on a cube image".into());
            }
            let sx = |v: u32, sh: u32| (((v >> sh) & 0x3f) as i32) << 26 >> 26;
            let reg = match o {
                Opnd::Vgpr(n) => n,
                _ => return Err("bad offset operand".into()),
            };
            match self.const_vgpr(idx, reg) {
                Some(v) => {
                    let c: Vec<String> = (0..noff).map(|k| sx(v, 8 * k as u32).to_string()).collect();
                    Some(if noff == 1 { c[0].clone() } else { format!("int{noff}({})", c.join(", ")) })
                }
                None if gather => {
                    // SM5 Gather accepts a run-time offset.
                    let x = r(&o)?;
                    let c: Vec<String> =
                        (0..noff).map(|k| format!("(asint({x} << {}) >> 26)", 26 - 8 * k)).collect();
                    Some(if noff == 1 { c[0].clone() } else { format!("int{noff}({})", c.join(", ")) })
                }
                None => return Err("non-constant texel offset on a sample op".into()),
            }
        } else {
            None
        };
        let bias = if matches!(modi, 5 | 6) {
            let o = take(self);
            Some(if a16 { format!("f16tof32({})", r(&o)?) } else { f(&o)? })
        } else {
            None
        };
        let zref = if cmp { Some(f(&take(self))?) } else { None };
        let grads = if matches!(modi, 2 | 3) {
            let mut dx = vec![];
            for _ in 0..nder {
                dx.push(f(&take(self))?);
            }
            let mut dy = vec![];
            for _ in 0..nder {
                dy.push(f(&take(self))?);
            }
            let v = |c: &Vec<String>| if c.len() == 1 { c[0].clone() } else { format!("float{}({})", c.len(), c.join(", ")) };
            Some((v(&dx), v(&dy)))
        } else {
            None
        };
        let base = next;
        let mut half = 0usize;
        let mut takef = |em: &mut Self| -> Result<String, String> {
            let k = half;
            half += 1;
            if a16 {
                let o = addr_reg(base + k / 2);
                em.touch(&o);
                Ok(format!("f16tof32({} >> {}u)", r(&o)?, 16 * (k % 2)))
            } else {
                let o = addr_reg(base + k);
                em.touch(&o);
                f(&o)
            }
        };
        let mut coords = vec![];
        for _ in 0..ncoord {
            coords.push(takef(self)?);
        }
        let lod = if modi == 4 { Some(takef(self)?) } else { None };
        let clamp = if matches!(modi, 1 | 3 | 6) { Some(takef(self)?) } else { None };
        if unorm {
            // Unnormalised (texel-space) coordinates: divide by the size of the view's mip 0.
            // Array slices are never normalised.
            let (outs, n) = match dim {
                0 => ("w, l", 1),
                4 => ("w, e, l", 1),
                1 => ("w, h, l", 2),
                5 => ("w, h, e, l", 2),
                2 => ("w, h, d, l", 3),
                _ => return Err("unnormalised coordinates on a cube image unsupported".into()),
            };
            let t = self.tmp;
            self.tmp += 1;
            let decl: Vec<String> = outs.split(", ").map(|v| format!("{v}{t}")).collect();
            self.line(&format!("uint {}; {tex}.GetDimensions(0u, {});", decl.join(", "), decl.join(", ")));
            for (k, sz) in ["w", "h", "d"].iter().take(n).enumerate() {
                coords[k] = format!("({} / (float){sz}{t})", coords[k]);
            }
        }
        // Cube images are addressed by hardware (s, t, face) coordinates (see tb_cube_dir).
        let cvec = if dim == 3 {
            format!("tb_cube_dir({})", coords.join(", "))
        } else if ncoord == 1 {
            coords[0].clone()
        } else {
            format!("float{ncoord}({})", coords.join(", "))
        };
        let smp = if let Some(Res::HeapSampler(key)) = rs.get(1).copied() {
            let name = format!("tb_sampler{idx}");
            let ty = if cmp { "SamplerComparisonState" } else { "SamplerState" };
            self.line(&format!("{ty} {name} = SamplerDescriptorHeap[tb_index{}_{}.{}];",
                key.register, key.space, XYZW[key.component as usize]));
            name
        } else if let Some(Res::HeapSamplerDyn(reg)) = rs.get(1).copied() {
            let name = format!("tb_sampler{idx}");
            let ty = if cmp { "SamplerComparisonState" } else { "SamplerState" };
            let index = ssrc_name(u32::from(reg))?;
            self.regs.insert(index.clone());
            self.line(&format!("{ty} {name} = SamplerDescriptorHeap[NonUniformResourceIndex({index})];"));
            name
        } else if let Some(s) = sslot {
            if cmp { format!("scmp{s}") } else { format!("samp{s}") }
        } else { return Err("missing sampler binding".into()); };
        let zero_off = if noff == 1 { "0".to_string() } else { format!("int{}({})", noff.max(1), vec!["0"; noff.max(1)].join(", ")) };
        // Optional trailing ", offset[, clamp]" arguments.
        let tail = |offset: &Option<String>, clamp: &Option<String>| -> Result<String, String> {
            Ok(match (offset, clamp) {
                (None, None) => String::new(),
                (Some(o), None) => format!(", {o}"),
                (o, Some(c)) => {
                    if noff == 0 {
                        return Err("LOD clamp on a cube image unsupported".into());
                    }
                    format!(", {}, {c}", o.clone().unwrap_or_else(|| zero_off.clone()))
                }
            })
        };
        let t = self.tmp;
        self.tmp += 1;
        let vd = match ins.dst[0] {
            Opnd::Vgpr(n) => n,
            _ => return Err("bad vdata".into()),
        };
        if op == 0x60 {
            if ins.aux.mask & !3 != 0 || d16 {
                return Err("image_get_lod requires 32-bit X/Y results".into());
            }
            self.require_compute_derivatives()?;
            self.line(&format!("float2 t{t} = float2({tex}.CalculateLevelOfDetail({smp}, {cvec}), {tex}.CalculateLevelOfDetailUnclamped({smp}, {cvec}));"));
            let mut dst = vd;
            for (bit, component) in [(1, "x"), (2,"y")] {
                if ins.aux.mask & bit != 0 {
                    self.wv(&Opnd::Vgpr(dst), &format!("asuint(t{t}.{component})"))?;
                    dst = dst.wrapping_add(1);
                }
            }
            return Ok(());
        }
        if gather {
            // gather4: dmask picks the channel; the four texels fill vdata..vdata+3.
            // These hierarchical depth passes convert their chosen integer mip to
            // f32 before issuing four red gathers on the same texture and sampler.
            let tslot = match rs.first().copied() {
                Some(Res::Tex(tslot, _)) => tslot,
                Some(Res::HeapTexture(..)) if modi != 4 && !cmp => u32::MAX,
                _ => return Err("unsupported direct heap gather".into()),
            };
            let depth_gather = match (self.ex.entry.as_deref(), tslot) {
                (Some("VirtualSmInvalidateInstancePagesCS"), 4) => {
                    self.wave_cull || packed_root::matches_program(&self.ex.isa, 7304, 0x8a6c22fd0266c12c)
                }
                (Some("GenerateConservativeDepthBufferCS"), 0) => {
                    packed_root::matches_program(&self.ex.isa, 812, 0xe3633db262ecebf9)
                }
                _ => false,
            };
            if modi == 4 && dim == 1 && !cmp && !has_off && !d16 && !a16 && !unorm
                && ins.aux.mask == 1 && sslot == Some(0)
                && ((self.is_cs && (matches!((self.ex.entry.as_deref(), tslot), (Some("InstanceCull" | "InstanceCullVSM"), 3) | (Some("PersistentClusterCull"), 3 | 5)) || (self.wave_cull && self.ex.entry.as_deref() == Some("CullPerPageDrawCommandsCs") && tslot == 0) || depth_gather
                    || (self.ex.entry.as_deref() == Some("InstanceCullBuildInstanceIdBufferCS") && tslot == 4
                        && [
                            (7072, 0x1fe3061314fe6030), (9944, 0x41f68c7d6b4a40ec),
                            (4492, 0x21c1b76c01e1b9ff), (7428, 0x94a6f79d724d3e25),
                            (7480, 0xf939a9961a9370bf), (10112, 0x056954bb8a896632),
                            (4372, 0x8279f28dabfc3ed1), (7008, 0xa4b009faa821ebab),
                            (6952, 0x54a23d7bfe154e74), (4316, 0xc0486c5c6f4e7160),
                            (7536, 0x6ae08e11efe46334), (7372, 0x17c731ece44188a8),
                            (10052, 0xf857b533d91c99c5), (4436, 0x7d6788e101f4109b),
                            (10008, 0x8685661900d8e524), (6912, 0x9381ab449cbbb186),
                        ].into_iter().any(|(size, hash)| packed_root::matches_program(&self.ex.isa, size, hash)))))
                    || (self.ex.stage == Stage::Pixel && tslot == 0
                        && self.ex.entry.as_deref() == Some("HZBTestPS")
                        && packed_root::matches_program(&self.ex.isa, 2108, 0x49d7bcccdaa0f843)))
                && matches!(ins.raw.first(), Some(0xf1100108 | 0xf110010a))
            {
                // This HZB program supplies an integer mip and a point sampler.
                // Gather around floor(uv * size - 0.5) in D3D component order.
                // Sampling texel centres preserves the bound sampler's edge mode.
                let lod = lod.as_ref().ok_or("missing HZB gather mip")?;
                self.line(&format!("uint w{t}, h{t}, m{t}; {tex}.GetDimensions(0u, w{t}, h{t}, m{t});"));
                self.line(&format!("uint l{t} = (uint)clamp({lod}, 0.0, float(max(1u, m{t}) - 1u));"));
                self.line(&format!("{tex}.GetDimensions(l{t}, w{t}, h{t}, m{t}); float2 sz{t} = max(float2(w{t},h{t}), 1.0);"));
                self.line(&format!("float2 base{t} = floor({cvec} * sz{t} - 0.5) + 0.5;"));
                let taps = ["0,1", "1,1", "1,0", "0,0"].map(|xy|
                    format!("{tex}.SampleLevel({smp}, (base{t} + float2({xy}))/sz{t}, l{t}).r"));
                self.line(&format!("float4 t{t} = float4({});", taps.join(", ")));
                for (k, channel) in XYZW.iter().enumerate() {
                    self.wv(&Opnd::Vgpr(vd + k as u8), &format!("asuint(t{t}.{channel})"))?;
                }
                return Ok(());
            }
            if modi == 4 && zref.is_none() && matches!(dim, 1 | 5) {
                // gather4_l: SM 6.x Gather has no LOD operand. Sample the four texel
                // centres of the footprint at the requested mip instead. A sample exactly at
                // a texel centre returns that texel under point or bilinear filtering, and
                // keeps the bound sampler's address mode. The mip rounds to the nearest level.
                let chan = match ins.aux.mask {
                    1 => "r",
                    2 => "g",
                    4 => "b",
                    8 => "a",
                    m => return Err(format!("gather4 with dmask {m:#x} unsupported")),
                };
                let lod = lod.as_ref().ok_or("missing gather4_l lod")?;
                let (uv, slice, dims, ext) = if dim == 5 {
                    (format!("({cvec}).xy"), Some(format!("({cvec}).z")), format!("w{t}, h{t}, e{t}, m{t}"), format!("uint e{t};"))
                } else {
                    (cvec.clone(), None, format!("w{t}, h{t}, m{t}"), String::new())
                };
                self.line(&format!("uint w{t}, h{t}, m{t}; {ext} {tex}.GetDimensions(0u, {dims});"));
                self.line(&format!("uint l{t} = (uint)clamp(round({lod}), 0.0, float(max(1u, m{t}) - 1u));"));
                self.line(&format!("{tex}.GetDimensions(l{t}, {dims}); float2 sz{t} = max(float2(w{t}, h{t}), 1.0);"));
                let off2 = offset.as_ref().map(|o| format!(" + float2({o})")).unwrap_or_default();
                self.line(&format!("float2 base{t} = floor({uv} * sz{t} - 0.5) + 0.5{off2};"));
                // D3D gather order: (0,1), (1,1), (1,0), (0,0) relative to the top-left texel.
                let taps = ["0,1", "1,1", "1,0", "0,0"].map(|xy| {
                    let p = format!("(base{t} + float2({xy})) / sz{t}");
                    let p = match &slice { Some(s) => format!("float3({p}, {s})"), None => p };
                    format!("{tex}.SampleLevel({smp}, {p}, l{t}).{chan}")
                });
                self.line(&format!("float4 t{t} = float4({});", taps.join(", ")));
                if d16 {
                    let chans: Vec<String> = XYZW.iter().map(|c| format!("t{t}.{c}")).collect();
                    return self.write_d16(vd, &chans);
                }
                for (k, channel) in XYZW.iter().enumerate() {
                    self.wv(&Opnd::Vgpr(vd + k as u8), &format!("asuint(t{t}.{channel})"))?;
                }
                return Ok(());
            }
            if !matches!(modi, 0 | 7) {
                return Err("gather4 with explicit LOD/bias unsupported".into());
            }
            let chan = match ins.aux.mask {
                1 => "Red",
                2 => "Green",
                4 => "Blue",
                8 => "Alpha",
                m => return Err(format!("gather4 with dmask {m:#x} unsupported")),
            };
            let off = offset.map(|o| format!(", {o}")).unwrap_or_default();
            let call = match &zref {
                Some(z) => format!("{tex}.GatherCmp{chan}({smp}, {cvec}, {z}{off})"),
                None => format!("{tex}.Gather{chan}({smp}, {cvec}{off})"),
            };
            self.line(&format!("float4 t{t} = {call};"));
            if d16 {
                let chans: Vec<String> = XYZW.iter().map(|c| format!("t{t}.{c}")).collect();
                return self.write_d16(vd, &chans);
            }
            for k in 0..4u8 {
                self.wv(&Opnd::Vgpr(vd + k), &format!("asuint(t{t}.{})", XYZW[k as usize]))?;
            }
            return Ok(());
        }
        // FXC (SM5) refuses implicit-derivative samples inside a loop it cannot unroll
        // (X3570/X3511). The console GPU computes those derivatives per quad regardless of
        // control flow; there is no SM5 equivalent, so inside emitted loops such samples
        // are APPROXIMATED with LOD 0 (exact for single-mip textures, too sharp otherwise).
        let in_loop = self.loop_depth > 0;
        if in_loop && matches!(modi, 0 | 1 | 5 | 6) {
            self.line("// APPROX-LOOP-SAMPLE: implicit-derivative sample inside a loop emitted as LOD 0");
        }
        let call = if let Some(z) = &zref {
            // Depth-compare sampling returns a single value.
            match modi {
                0 if in_loop => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                0 => format!("{tex}.SampleCmp({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                4 if !a16 && self.immediate_zero_before(idx, addr_reg(base + ncoord)) => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                7 => format!("{tex}.SampleCmpLevelZero({smp}, {cvec}, {z}{})", tail(&offset, &None)?),
                _ => return Err("depth-compare sample with explicit LOD/bias/gradient unsupported in SM5".into()),
            }
        } else {
            match modi {
                0 | 1 if in_loop => format!("{tex}.SampleLevel({smp}, {cvec}, 0.0{})", tail(&offset, &None)?),
                5 | 6 if in_loop => format!("{tex}.SampleLevel({smp}, {cvec}, {}{})", bias.clone().ok_or("missing bias")?, tail(&offset, &None)?),
                0 | 1 => format!("{tex}.Sample({smp}, {cvec}{})", tail(&offset, &clamp)?),
                2 | 3 => {
                    let (dx, dy) = grads.clone().ok_or("missing gradients")?;
                    format!("{tex}.SampleGrad({smp}, {cvec}, {dx}, {dy}{})", tail(&offset, &clamp)?)
                }
                4 => format!("{tex}.SampleLevel({smp}, {cvec}, {}{})", lod.clone().ok_or("missing lod")?, tail(&offset, &None)?),
                5 | 6 => format!("{tex}.SampleBias({smp}, {cvec}, {}{})", bias.clone().ok_or("missing bias")?, tail(&offset, &clamp)?),
                _ => format!("{tex}.SampleLevel({smp}, {cvec}, 0.0{})", tail(&offset, &None)?),
            }
        };
        if !in_loop && matches!(modi, 0 | 1 | 5 | 6) {
            self.require_compute_derivatives()?;
        }
        if zref.is_some() {
            self.line(&format!("float t{t} = {call};"));
            // The compare result is replicated to every requested channel.
            if d16 {
                let chans: Vec<String> = (0..ins.aux.mask.count_ones()).map(|_| format!("t{t}")).collect();
                return self.write_d16(vd, &chans);
            }
            let mut n = 0u8;
            for ch in 0..4 {
                if (ins.aux.mask >> ch) & 1 != 0 {
                    self.wv(&Opnd::Vgpr(vd + n), &format!("asuint(t{t})"))?;
                    n += 1;
                }
            }
            return Ok(());
        }
        self.line(&format!("float4 t{t} = {call};"));
        if d16 {
            let chans: Vec<String> = (0..4).filter(|ch| (ins.aux.mask >> ch) & 1 != 0).map(|ch| format!("t{t}.{}", XYZW[ch])).collect();
            return self.write_d16(vd, &chans);
        }
        let mut n = 0u8;
        for ch in 0..4 {
            if (ins.aux.mask >> ch) & 1 != 0 {
                let dst = Opnd::Vgpr(vd + n);
                self.wv(&dst, &format!("asuint(t{t}.{})", XYZW[ch]))?;
                n += 1;
            }
        }
        Ok(())
    }

    /// D16 image result: `vals` (float expressions) converted to f16 (round to nearest
    /// even) and packed two per VGPR from `vd`, low half first (ISA Table 41, D16 / DMASK).
    /// With an odd count the high half of the last VGPR is written as 0.
    pub(in crate::hlsl) fn write_d16(&mut self, vd: u8, vals: &[String]) -> Result<(), String> {
        for (k, pair) in vals.chunks(2).enumerate() {
            let lo = format!("tb_f16_rne({})", pair[0]);
            let e = match pair.get(1) {
                Some(hi) => format!("({lo} | (tb_f16_rne({hi}) << 16))"),
                None => lo,
            };
            self.wv(&Opnd::Vgpr(vd + k as u8), &e)?;
        }
        Ok(())
    }

    /// A literal move immediately preceding the use, with no branch entering after
    /// the move. The two instructions therefore see the same active lanes.
    pub(in crate::hlsl) fn immediate_zero_before(&self, idx: usize, reg: Opnd) -> bool {
        let Some(mov) = self.insns[..idx].iter().rev().find(|i| i.mnemonic != "s_waitcnt") else { return false; };
        mov.enc == Enc::Vop1 && mov.mnemonic == "v_mov_b32" && mov.aux.sdwa.is_none()
            && mov.dst.first() == Some(&reg)
            && matches!(mov.src.first(), Some(Opnd::ConstInt(0) | Opnd::Literal(0)))
            && !self.insns.iter().any(|i| {
                let target = i.offset as i64 + 4 + 4 * i.aux.imm;
                i.enc == Enc::Sopp && Self::branch_cond(i.op).is_some()
                    && target > mov.offset as i64 && target <= self.insns[idx].offset as i64
            })
    }

    /// Statically known value of a VGPR at instruction `idx`: the most recent write in
    /// program order must be a `v_mov_b32` of a constant. Used where HLSL demands a
    /// literal (texel offsets).
    pub(in crate::hlsl) fn const_vgpr(&self, idx: usize, reg: u8) -> Option<u32> {
        for ins in self.insns[..idx].iter().rev() {
            if ins.dst.iter().any(|d| matches!(d, Opnd::Vgpr(n) if *n == reg)) {
                if ins.mnemonic != "v_mov_b32" {
                    return None;
                }
                return match ins.src.first() {
                    Some(Opnd::ConstInt(v)) => Some(*v as i32 as u32),
                    Some(Opnd::Literal(v)) => Some(*v),
                    Some(Opnd::ConstF32(f)) => Some(f.to_bits()),
                    _ => None,
                };
            }
        }
        None
    }
}
