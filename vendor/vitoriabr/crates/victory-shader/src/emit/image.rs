//! GFX10 `MIMG` image loads, stores, atomics, gathers, dimension queries, and
//! texture sampling lowered to SM 6.6 DXIL.

use super::{
    Attr, Bin, Cast, Fields, Inst, Lower, Operand, Reg, ResourceType, V, Why, flags, pred,
};

impl<'a> Lower<'a> {
    pub(super) fn image(&mut self, inst: &Inst, b: usize, i: usize) -> Result<(), Why> {
        let Fields::Image {
            data,
            addr,
            dmask,
            dim,
            unorm,
            a16,
            d16,
            tfe,
            lwe,
            ..
        } = &inst.fields
        else {
            unreachable!()
        };
        if *tfe || *lwe {
            return Err("an image access with tfe or lwe");
        }
        let name = inst.name();
        // A load or sample with no channel enabled returns the first.
        let dmask = if *dmask == 0 { &1 } else { dmask };
        let site = (b, i);
        let guard = self.guard(b, i)?;
        let c = self.cur;
        let (kind, coords) = match dim {
            0 => (1, 1),
            1 => (2, 2),
            2 => (4, 3),
            // Sampled (face coordinates and face, see `cube_direction`) or
            // sized only.
            3 if name.starts_with("image_sample")
                || name.starts_with("image_gather")
                || name == "image_get_resinfo" =>
            {
                (5, 3)
            }
            4 => (6, 2),
            5 => (7, 3),
            _ => return Err("an image dimension (cube loads or MSAA)"),
        };
        // Address VGPRs in order.
        let mut regs = Vec::new();
        for a in addr {
            if let Operand::Vector { reg, dwords } = *a {
                regs.extend((0..dwords).map(|k| reg + k));
            }
        }
        // Full 32-bit address VGPR at `reg_idx` (offset, bias, z-compare).
        let addr_u32 = |l: &mut Self, reg_idx: usize| -> Result<V, Why> {
            let r = *regs.get(reg_idx).ok_or("an image address")?;
            let v = inst.read(Reg::V(r)).ok_or("an image address")?;
            l.i32_of(v)
        };
        // With a16, two 16-bit components per VGPR starting at `base_reg`, low half first.
        let addr_packed = |l: &mut Self, base_reg: usize, k: usize| -> Result<V, Why> {
            let x = addr_u32(l, base_reg + if *a16 { k / 2 } else { k })?;
            if !*a16 {
                return Ok(x);
            }
            let c = l.cur;
            let (sh, mask) = (l.s.m.i32(16 * (k as u32 % 2)), l.s.m.i32(0xFFFF));
            let x = l.s.m.bin(c, Bin::LShr, x, sh);
            Ok(l.s.m.bin(c, Bin::And, x, mask))
        };
        let addr_val = |l: &mut Self, k: usize| -> Result<V, Why> { addr_packed(l, 0, k) };
        let src = self
            .descriptors
            .get(&(site, false))
            .copied()
            .ok_or("an untracked image")?;
        let store = name.starts_with("image_store");
        let uav = store
            || self
                .used_kind(src)
                .is_some_and(|u| u.kind == ResourceType::Uav);
        let props = (kind | if uav { 0x1000 } else { 0 }, 0x409);
        if let Some(op) = name.strip_prefix("image_atomic_") {
            // GLC returns the old value. DXIL implements subtraction by adding
            // the two's complement of the operand at the same width.
            let code = match op {
                "add" | "sub" => 0,
                "and" => 1,
                "or" => 2,
                "xor" => 3,
                "smin" => 4,
                "smax" => 5,
                "umin" => 6,
                "umax" => 7,
                "swap" => 8,
                "cmpswap" => u32::MAX,
                _ => return Err(name),
            };
            let Fields::Image { cache, .. } = &inst.fields else {
                unreachable!()
            };
            if *d16 || *a16 {
                return Err("a 16-bit image atomic");
            }
            // Compare-exchange consumes a replacement followed by a comparand.
            let compare = code == u32::MAX;
            let wide = match (compare, *dmask) {
                (false, 1) | (true, 3) => false,
                (false, 3) | (true, 15) => true,
                _ => return Err("an image atomic data mask"),
            };
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image atomic data");
            };
            let width = if wide { 2 } else { 1 };
            let vals = self.vgprs(inst, data, if compare { 2 * width } else { width })?;
            let (ty, component) = if wide {
                self.s.flags |= flags::INT64 | flags::ATOMIC_TYPED64 | flags::ATOMIC_HEAP64;
                (self.s.m.int(64), 0x107) // one uint64_t in an R32G32_UINT view
            } else {
                (self.s.t.i32, 0x105)
            };
            let original = if wide {
                self.join64(vals[0], vals[1])
            } else {
                vals[0]
            };
            let value = if op == "sub" {
                let zero = self.s.m.int_const(ty, 0);
                self.s.m.bin(c, Bin::Sub, zero, original)
            } else {
                original
            };
            let h = self.handle(site, src, false, (kind | 0x1000, component))?;
            let mut args = vec![h];
            let (intrinsic, opcode) = if compare {
                let comparand = if wide {
                    self.join64(vals[2], vals[3])
                } else {
                    vals[1]
                };
                args.extend(cs);
                args.extend([comparand, value]);
                let intrinsic = if wide {
                    "atomicCompareExchange.i64"
                } else {
                    "atomicCompareExchange.i32"
                };
                (intrinsic, 79)
            } else {
                args.push(self.s.m.i32(code));
                args.extend(cs);
                args.push(value);
                (
                    if wide {
                        "atomicBinOp.i64"
                    } else {
                        "atomicBinOp.i32"
                    },
                    78,
                )
            };
            let at = self.begin_guarded(guard);
            let r = self
                .s
                .op(self.cur, intrinsic, Attr::NoUnwind, ty, opcode, &args);
            if cache.glc {
                let old = self.end_guarded(at, &[(r, original)])[0];
                if wide {
                    let (lo, hi) = self.split64(old);
                    self.write(inst, Reg::V(reg), lo, None);
                    self.write(inst, Reg::V(reg + 1), hi, None);
                } else {
                    self.write(inst, Reg::V(reg), old, None);
                }
            } else {
                self.end_guarded(at, &[]);
            }
            return Ok(());
        }
        if store {
            if name != "image_store" {
                return Err("an image store to a mip level");
            }
            // shortcut: stores are taken to write all four channels of a
            // float format (the console converts by the view's format).
            if *dmask != 0xF {
                return Err("an image store of some channels");
            }
            let iu = self.s.m.undef(self.s.t.i32);
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let Operand::Vector { reg, .. } = *data else {
                return Err("image store data");
            };
            let mut vals = Vec::new();
            for k in 0..4u8 {
                // d16: two f16 channels per VGPR, low half first.
                let r = if *d16 { k / 2 } else { k };
                let v = inst.read(Reg::V(reg + r)).ok_or("image store data")?;
                vals.push(if *d16 {
                    let mut x = self.i32_of(v)?;
                    if k % 2 == 1 {
                        let k16 = self.s.m.i32(16);
                        x = self.s.m.bin(self.cur, Bin::LShr, x, k16);
                    }
                    let f32t = self.s.t.f32;
                    self.s
                        .op(self.cur, "legacyF16ToF32", Attr::ReadNone, f32t, 131, &[x])
                } else {
                    self.float_of(v)?
                });
            }
            let h = self.handle(site, src, false, props)?;
            let at = self.begin_guarded(guard);
            let mut args = vec![h];
            args.extend(cs);
            args.extend(vals);
            args.push(self.s.m.i8(0xF));
            let void = self.s.t.void;
            self.s.op(
                self.cur,
                "textureStore.f32",
                Attr::NoUnwind,
                void,
                67,
                &args,
            );
            self.end_guarded(at, &[]);
            return Ok(());
        }
        let has_cl = name.split('_').any(|t| t == "cl");
        let (op, sample) = match name {
            "image_sample" | "image_sample_o" | "image_sample_cl" | "image_sample_cl_o" => {
                (60, true)
            }
            "image_sample_lz" | "image_sample_l" | "image_sample_lz_o" | "image_sample_l_o" => {
                (62, true)
            }
            "image_sample_b" | "image_sample_b_cl" | "image_sample_b_o" | "image_sample_b_cl_o" => {
                (61, true)
            }
            "image_sample_c_lz" | "image_sample_c_lz_o" => (65, true),
            "image_sample_d" | "image_sample_d_cl" | "image_sample_d_o" | "image_sample_d_cl_o" => {
                (63, true)
            }
            "image_get_lod" if self.s.pixel => (81, true),
            // Implicit derivatives: pixel shaders only.
            "image_sample_c" | "image_sample_c_cl" | "image_sample_c_o" | "image_sample_c_cl_o"
                if self.s.pixel =>
            {
                (64, true)
            }
            "image_gather4" | "image_gather4_lz" | "image_gather4_lz_o" | "image_gather4_l"
            | "image_gather4_l_o" => (73, true),
            "image_load" | "image_load_mip" => (66, false),
            "image_get_resinfo" => (72, false),
            _ => return Err(name),
        };
        if op == 72 {
            // Width, height, depth or layers, levels (per dmask) of the level
            // in the first address: DXIL's dimensions; a channel DXIL leaves
            // out reads 1 (UAVs: one level).
            let h = self.handle(site, src, false, props)?;
            let mip = if uav {
                self.s.m.undef(self.s.t.i32)
            } else {
                addr_val(self, 0)?
            };
            let i32t = self.s.t.i32;
            let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
            let d = self
                .s
                .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
            let one = self.s.m.i32(1);
            let present = match kind {
                1 => [true, false, false, true],
                2 | 5 | 6 => [true, true, false, true],
                _ => [true, true, true, true],
            };
            let Operand::Vector { reg, .. } = *data else {
                return Err("an image result");
            };
            for (k, ch) in (0..4u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                let x = if ch == 3 && uav || !present[ch as usize] {
                    one
                } else {
                    self.s.m.extract(c, d, ch)
                };
                self.write(inst, Reg::V(reg + k as u8), x, guard);
            }
            return Ok(());
        }
        if uav && op != 66 {
            return Err("sampling a UAV");
        }
        let h = self.handle(site, src, false, props)?;
        let fu = self.s.m.undef(self.s.t.f32);
        let iu = self.s.m.undef(self.s.t.i32);
        // A gather at a named level, done as four texel reads (below).
        let mut gathered: Option<[V; 4]> = None;
        let r = if sample {
            let ss = self
                .descriptors
                .get(&(site, true))
                .copied()
                .ok_or("an untracked sampler")?;
            // Comparison: the reference value is the first address after
            // the bias, and the sampler is a comparison sampler.
            let cmp = matches!(op, 64 | 65);
            if cmp && (*a16 || dmask.count_ones() != 1) {
                return Err("a comparison sample of 16-bit coordinates or several channels");
            }
            let sh = self.handle(site, ss, true, (14 | if cmp { 0x8000 } else { 0 }, 0))?;
            // Texel offsets (`_o`) come first: x in bits 0-5, y in 8-13,
            // signed (programmable offsets: gathers only).
            let offset_word = name.ends_with("_o");
            let first = usize::from(offset_word)
                + usize::from(name.starts_with("image_sample_b"))
                + usize::from(cmp);
            // Coordinates the size divides (not the array slice); explicit
            // gradients (x then y) come before the coordinates.
            let spatial = coords - usize::from(kind == 6 || kind == 7);
            // A cube's gradients are the two of its face coordinates.
            let grads = if kind == 5 { 2 } else { spatial };
            let deriv_regs = if op == 63 {
                if *a16 { grads.div_ceil(2) } else { grads }
            } else {
                0
            };
            let coord_base = first + 2 * deriv_regs;
            let mut cs = vec![fu; 4];
            // With a16, sampling coordinates, gradients, LOD and clamp are f16.
            let to_float = |l: &mut Self, x: V| {
                if *a16 {
                    let f = l.s.t.f32;
                    l.s.op(l.cur, "legacyF16ToF32", Attr::ReadNone, f, 131, &[x])
                } else {
                    l.as_float(x)
                }
            };
            if kind == 5 && (*unorm || op == 81) {
                return Err("a cube sample with texel coordinates or a level query");
            }
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                let x = addr_packed(self, coord_base, k)?;
                *slot = to_float(self, x);
            }
            if kind == 5 {
                let dir = self.cube_direction(cs[0], cs[1], cs[2]);
                cs[..3].copy_from_slice(&dir);
            }
            if *unorm {
                // Texel coordinates: divided by the size of level 0.
                let i32t = self.s.t.i32;
                let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                let z = self.s.m.i32(0);
                let d = self
                    .s
                    .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, z]);
                for (k, slot) in cs.iter_mut().enumerate().take(spatial) {
                    let n = self.s.m.extract(c, d, k as u32);
                    let n = self.s.m.cast(c, Cast::UiToFp, n, self.s.t.f32);
                    *slot = self.s.m.bin(c, Bin::SDiv, *slot, n);
                }
            }
            if op == 81 {
                // The clamped then the unclamped level of detail. An array's
                // slice is not part of the coordinates here: the host's
                // `calculateLOD` wants it undef (a 2D array's third
                // coordinate, a 1D array's second and third).
                let (x0, x1, x2) = match kind {
                    6 => (cs[0], fu, fu),
                    7 => (cs[0], cs[1], fu),
                    _ => (cs[0], cs[1], cs[2]),
                };
                let Operand::Vector { reg, .. } = *data else {
                    return Err("an image result");
                };
                let f32t = self.s.t.f32;
                for (k, ch) in (0..2u32).filter(|ch| dmask >> ch & 1 != 0).enumerate() {
                    let clamped = self.s.m.bool(ch == 0);
                    let lod = self.s.op(
                        c,
                        "calculateLOD.f32",
                        Attr::ReadOnly,
                        f32t,
                        81,
                        &[h, sh, x0, x1, x2, clamped],
                    );
                    let x = self.as_int(lod);
                    self.write(inst, Reg::V(reg + k as u8), x, guard);
                }
                return Ok(());
            }
            let offsets = if kind == 5 {
                0
            } else {
                (coords - usize::from(kind == 6 || kind == 7)).min(3)
            };
            let zero = self.s.m.i32(0);
            let mut os = vec![iu; 3];
            for o in os.iter_mut().take(offsets) {
                *o = zero;
            }
            if offset_word && offsets > 0 {
                // The offset word is always a full 32-bit VGPR (even with a16):
                // signed 6-bit fields at bits 0..=5 (x), 8..=13 (y), 16..=21 (z).
                // DXIL sampling ops require immediate offsets in -8..=7, while
                // gathers also accept computed offsets.
                let w = addr_u32(self, 0)?;
                let imm = self.s.m.int_value(w).and_then(|w| {
                    let w = w as u32;
                    let field = |k: usize| {
                        let x = (w >> (8 * k) & 0x3F) as i32;
                        if x >= 32 { x - 64 } else { x }
                    };
                    let fields = [field(0), field(1), field(2)];
                    (op == 73 || fields[..offsets].iter().all(|x| (-8..=7).contains(x)))
                        .then_some(fields)
                });
                if let Some(fields) = imm {
                    for (k, o) in os.iter_mut().enumerate().take(offsets) {
                        *o = self.s.m.i32(fields[k] as u32);
                    }
                } else {
                    for (k, at) in [(0, 26), (1, 18), (2, 10)].into_iter().take(offsets) {
                        // Bits at..at+5 sign-extended: shift up, then down.
                        let left = self.s.m.i32(at);
                        let t = self.s.m.bin(c, Bin::Shl, w, left);
                        let k26 = self.s.m.i32(26);
                        os[k] = self.s.m.bin(c, Bin::AShr, t, k26);
                    }
                }
            }
            let mut args = vec![h, sh];
            args.extend(&cs);
            let clamp = if has_cl {
                let x = addr_packed(self, coord_base, coords)?;
                to_float(self, x)
            } else {
                fu
            };
            match op {
                // Outside a pixel shader there are no screen derivatives:
                // the validator refuses `sample` there ("features
                // incompatible with the shader stage": 730 compute programs,
                // among them the G-buffer resolves and the sky), and what a
                // driver does with it is its own affair.
                // shortcut: such a sample reads level 0 (a bias: that
                // level). The console takes derivatives from neighbouring
                // lanes there; compute them from the lanes if a resolve
                // shows the wrong mip.
                60 if !self.s.pixel => {
                    args.extend(&os);
                    args.push(self.s.m.f32(0));
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                60 => {
                    args.extend(&os);
                    args.push(clamp);
                    self.s
                        .op(c, "sample.f32", Attr::ReadOnly, self.s.t.res_ret, 60, &args)
                }
                61 => {
                    // With a16 the bias is an f16 in the low half of its
                    // word (the word is whole, the value is not).
                    let bias = addr_u32(self, usize::from(offset_word))?;
                    let bias = if *a16 {
                        let low = self.s.m.i32(0xFFFF);
                        let bias = self.s.m.bin(c, Bin::And, bias, low);
                        to_float(self, bias)
                    } else {
                        self.as_float(bias)
                    };
                    args.extend(&os);
                    if self.s.pixel {
                        args.extend([bias, clamp]);
                        self.s.op(
                            c,
                            "sampleBias.f32",
                            Attr::ReadOnly,
                            self.s.t.res_ret,
                            61,
                            &args,
                        )
                    } else {
                        // As above: the bias is the level.
                        args.push(bias);
                        self.s.op(
                            c,
                            "sampleLevel.f32",
                            Attr::ReadOnly,
                            self.s.t.res_ret,
                            62,
                            &args,
                        )
                    }
                }
                63 if kind == 5 => {
                    // A cube with explicit gradients: the host wants them
                    // as changes of the direction, the console gives the
                    // changes of the face coordinates (the face is one
                    // unit wide). The level they select is taken from the
                    // longer of the two, in texels, and sampled directly.
                    // shortcut: no anisotropic filtering for these samples
                    // (the longer axis alone decides the level).
                    let mut len = [fu; 2];
                    for (axis, slot) in len.iter_mut().enumerate() {
                        let mut sum = self.s.m.f32(0);
                        for k in 0..2 {
                            let x = addr_packed(self, first + axis * deriv_regs, k)?;
                            let x = to_float(self, x);
                            let sq = self.s.m.bin(c, Bin::Mul, x, x);
                            sum = self.s.m.bin(c, Bin::Add, sum, sq);
                        }
                        *slot = self.unary(24, sum); // square root
                    }
                    let longer = self.binary(35, len[0], len[1]); // maximum
                    let i32t = self.s.t.i32;
                    let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                    let z = self.s.m.i32(0);
                    let d = self
                        .s
                        .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, z]);
                    let width = self.s.m.extract(c, d, 0);
                    let width = self.s.m.cast(c, Cast::UiToFp, width, self.s.t.f32);
                    let texels = self.s.m.bin(c, Bin::Mul, longer, width);
                    // DXIL Log already returns log2. A footprint of zero
                    // gives -infinity and the sampler takes its finest mip.
                    // SampleLevel has no clamp operand; apply the guest
                    // instruction's minimum LOD before passing the level.
                    let lod = self.unary(23, texels);
                    let lod = if has_cl { self.binary(35, lod, clamp) } else { lod };
                    args.extend(&os);
                    args.push(lod);
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                63 => {
                    let mut g = vec![fu; 6];
                    for k in 0..spatial {
                        let x = addr_packed(self, first, k)?;
                        g[k] = to_float(self, x);
                        let y = addr_packed(self, first + deriv_regs, k)?;
                        g[3 + k] = to_float(self, y);
                    }
                    args.extend(&os);
                    args.extend(g);
                    args.push(clamp);
                    self.s.op(
                        c,
                        "sampleGrad.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        63,
                        &args,
                    )
                }
                64 | 65 => {
                    let reference = addr_u32(self, first - 1)?;
                    let reference = self.as_float(reference);
                    args.extend(&os);
                    args.push(reference);
                    let name = if op == 64 {
                        args.push(clamp);
                        "sampleCmp.f32"
                    } else {
                        "sampleCmpLevelZero.f32"
                    };
                    self.s
                        .op(c, name, Attr::ReadOnly, self.s.t.res_ret, op, &args)
                }
                62 => {
                    let lod = if matches!(name, "image_sample_l" | "image_sample_l_o") {
                        let x = addr_packed(self, coord_base, coords)?;
                        to_float(self, x)
                    } else {
                        self.s.m.f32(0)
                    };
                    args.extend(&os);
                    args.push(lod);
                    self.s.op(
                        c,
                        "sampleLevel.f32",
                        Attr::ReadOnly,
                        self.s.t.res_ret,
                        62,
                        &args,
                    )
                }
                _ => {
                    if name == "image_gather4" {
                        return Err("a gather with implicit level");
                    }
                    if dmask.count_ones() != 1 {
                        return Err("a gather of several channels");
                    }
                    if matches!(name, "image_gather4_l" | "image_gather4_l_o")
                        && coords == 2
                        && spatial == 2
                    {
                        // The host's gather has no level: it read level 0
                        // where the title names one (the depth pyramid its
                        // occlusion tests read: most of the world was
                        // culled against the wrong depths). The four texels
                        // are read at the level instead, in the gather's
                        // order: (u0,v1) (u1,v1) (u1,v0) (u0,v0).
                        // With an offset too (`image_gather4_l_o`): the
                        // offset is in texels of that level.
                        // The title's meshlet culling takes a 4x4 block of
                        // the pyramid as one gather and three offset ones
                        // ((2,0), (0,2), (2,2)); the three read level 0,
                        // two texels beside the first one's place, so a
                        // meshlet three or four texels wide was tested
                        // against a quarter of what it covers.
                        // shortcut: the level is rounded to a whole one and
                        // coordinates past the edge are clamped, whatever
                        // the sampler says.
                        let lod = addr_packed(self, coord_base, coords)?;
                        let lod = to_float(self, lod);
                        let (i32t, f32t) = (self.s.t.i32, self.s.t.f32);
                        let zero = self.s.m.f32(0);
                        let lod = self.binary(35, lod, zero); // maximum
                        let lod = self.unary(26, lod); // round to nearest
                        let mip = self.s.m.cast(c, Cast::FpToUi, lod, i32t);
                        let dims = self.s.m.named_struct("dx.types.Dimensions", vec![i32t; 4]);
                        let d = self
                            .s
                            .op(c, "getDimensions", Attr::ReadOnly, dims, 72, &[h, mip]);
                        let half = self.s.m.f32(0.5f32.to_bits());
                        let one = self.s.m.f32(1.0f32.to_bits());
                        let (mut lo, mut hi) = ([iu; 2], [iu; 2]);
                        for k in 0..2 {
                            let n = self.s.m.extract(c, d, k as u32);
                            let n = self.s.m.cast(c, Cast::UiToFp, n, f32t);
                            let last = self.s.m.bin(c, Bin::Sub, n, one);
                            let t = self.s.m.bin(c, Bin::Mul, cs[k], n);
                            let t = self.s.m.bin(c, Bin::Sub, t, half);
                            let first = self.unary(27, t); // round down
                            // The offset word's field (0 without one).
                            let by = self.s.m.cast(c, Cast::SiToFp, os[k], f32t);
                            let first = self.s.m.bin(c, Bin::Add, first, by);
                            let next = self.s.m.bin(c, Bin::Add, first, one);
                            for (slot, x) in [(&mut lo[k], first), (&mut hi[k], next)] {
                                let x = self.binary(35, x, zero); // maximum
                                let x = self.binary(36, x, last); // minimum
                                *slot = self.s.m.cast(c, Cast::FpToSi, x, i32t);
                            }
                        }
                        let ch = dmask.trailing_zeros();
                        let mut g = [fu; 4];
                        let mut read = None;
                        let texels = [
                            (lo[0], hi[1]),
                            (hi[0], hi[1]),
                            (hi[0], lo[1]),
                            (lo[0], lo[1]),
                        ];
                        for (slot, (x, y)) in g.iter_mut().zip(texels) {
                            let r = self.s.op(
                                c,
                                "textureLoad.f32",
                                Attr::ReadOnly,
                                self.s.t.res_ret,
                                66,
                                &[h, mip, x, y, iu, iu, iu, iu],
                            );
                            *slot = self.s.m.extract(c, r, ch);
                            read = Some(r);
                        }
                        gathered = Some(g);
                        read.expect("four reads")
                    } else {
                        args.extend(&os[..2]);
                        args.push(self.s.m.i32(dmask.trailing_zeros()));
                        self.s.op(
                            c,
                            "textureGather.f32",
                            Attr::ReadOnly,
                            self.s.t.res_ret,
                            73,
                            &args,
                        )
                    }
                }
            }
        } else {
            let mut cs = vec![iu; 3];
            for (k, slot) in cs.iter_mut().enumerate().take(coords) {
                *slot = addr_val(self, k)?;
            }
            let mip = if name == "image_load_mip" {
                addr_val(self, coords)?
            } else if uav {
                // Our declarations have four channels.
                self.s.flags |= flags::TYPED_UAV_LOADS;
                iu
            } else {
                self.s.m.i32(0)
            };
            let mut args = vec![h, mip];
            args.extend(&cs);
            args.extend([iu, iu, iu]);
            self.s.op(
                c,
                "textureLoad.f32",
                Attr::ReadOnly,
                self.s.t.res_ret,
                66,
                &args,
            )
        };
        let Operand::Vector { reg, .. } = *data else {
            return Err("an image result");
        };
        let channels: Vec<u32> = if op == 73 {
            vec![0, 1, 2, 3]
        } else {
            (0..4).filter(|ch| dmask >> ch & 1 != 0).collect()
        };
        let mut xs = Vec::new();
        for ch in channels {
            let x = match gathered {
                Some(g) => g[ch as usize],
                None => self.s.m.extract(c, r, ch),
            };
            xs.push(if *d16 {
                // shortcut: d16 results are taken as floats (rounded to
                // nearest even); integer formats would keep 16 bits.
                let i32t = self.s.t.i32;
                self.s
                    .op(c, "legacyF32ToF16", Attr::ReadNone, i32t, 130, &[x])
            } else {
                self.as_int(x)
            });
        }
        if *d16 {
            // Two channels per VGPR, low half first (an odd last high half 0).
            let k16 = self.s.m.i32(16);
            xs = xs
                .chunks(2)
                .map(|p| match *p {
                    [lo, hi] => {
                        let hi = self.s.m.bin(c, Bin::Shl, hi, k16);
                        self.s.m.bin(c, Bin::Or, lo, hi)
                    }
                    _ => p[0],
                })
                .collect();
        }
        for (k, x) in xs.into_iter().enumerate() {
            self.write(inst, Reg::V(reg + k as u8), x, guard);
        }
        Ok(())
    }

    /// The direction a cube sample's face coordinates stand for: the
    /// console takes s and t in [1, 2] (`v_cubesc`/`v_cubetc` over
    /// `|v_cubema|`, plus 1.5) and the face id (`v_cubeid`), which the
    /// host's cube sampling turns back into the same texel.
    // shortcut: the view is taken as a cube, not a cube array (whose face
    // operand adds 8 × the slice).
    fn cube_direction(&mut self, s: V, t: V, face: V) -> [V; 3] {
        let c = self.cur;
        let k = |l: &mut Self, v: f32| l.s.m.f32(v.to_bits());
        let (two, three) = (k(self, 2.0), k(self, 3.0));
        let u = self.s.m.bin(c, Bin::Mul, s, two);
        let u = self.s.m.bin(c, Bin::Sub, u, three);
        let v = self.s.m.bin(c, Bin::Mul, t, two);
        let v = self.s.m.bin(c, Bin::Sub, v, three);
        let z = self.s.m.f32(0x8000_0000);
        let (nu, nv) = (
            self.s.m.bin(c, Bin::Sub, z, u),
            self.s.m.bin(c, Bin::Sub, z, v),
        );
        let (one, mone) = (k(self, 1.0), k(self, -1.0));
        let f = self.s.m.cast(c, Cast::FpToUi, face, self.s.t.i32);
        // Per face +x, -x, +y, -y, +z, -z.
        let table = [
            [one, mone, u, u, u, nu],
            [nv, nv, one, mone, nv, nv],
            [nu, u, v, nv, one, mone],
        ];
        table.map(|row| {
            let mut r = row[0];
            for (n, &x) in row.iter().enumerate().skip(1) {
                let kn = self.s.m.i32(n as u32);
                let is = self.s.m.cmp(c, pred::EQ, f, kn);
                r = self.s.m.select(c, is, x, r);
            }
            r
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Lowered, Parts, lower};
    use crate::emit::container::SigElement;
    use crate::native::{
        ComputeIo, HwStage, NativeProgram, ResourceType, RootParam, RootType, StageIo, UsedResource,
    };
    use crate::{io, ir, resource, structure};

    const S_ENDPGM: u32 = 0xBF81_0000;

    fn root(root: RootType, user_data: u8, slot: u8) -> RootParam {
        RootParam {
            root,
            table_offset_dwords: 0,
            sgprs: if root == RootType::Descriptor { 2 } else { 1 },
            base_register: 0,
            user_data,
            slot,
        }
    }

    fn texture_tables() -> (Vec<RootParam>, Vec<UsedResource>) {
        let range = |kind, user_data, slot| UsedResource {
            kind,
            root: RootType::Table,
            table_offset_dwords: 0,
            lower_bound: 0,
            stride: 0,
            dwords: if kind == ResourceType::Sampler { 4 } else { 8 },
            user_data,
            slot,
        };
        (
            vec![root(RootType::Table, 0, 0), root(RootType::Table, 1, 1)],
            vec![
                range(ResourceType::Srv, 0, 0),
                range(ResourceType::Sampler, 1, 1),
            ],
        )
    }

    /// Encodes a contiguous-address GFX10 MIMG instruction (`vaddr` at `v0`,
    /// `vdata` at `v12`, `srsrc` at `s[8:15]`, `ssamp` at `s[16:19]`).
    fn encode_mimg(opcode: u16, dim: u8, dmask: u8, a16: bool) -> [u32; 2] {
        [
            0x3C << 26
                | (u32::from(opcode & 0x7F)) << 18
                | (u32::from(dmask & 0xF)) << 8
                | (u32::from(dim & 7)) << 3
                | u32::from(opcode >> 7 & 1),
            (u32::from(a16)) << 30 | (4 << 21) | (2 << 16) | (12 << 8),
        ]
    }

    /// Lowers a synthetic pixel or compute shader executing one MIMG sample,
    /// initializing `v0` to `offset_imm` and `v1..=v9` from root constants
    /// `s2` (or `s_memtime` at `poison_reg` to verify that register is read).
    fn lower_mimg_sample(
        pixel: bool,
        opcode: u16,
        dim: u8,
        dmask: u8,
        a16: bool,
        offset_imm: u32,
        poison_reg: Option<u8>,
    ) -> Result<Lowered, crate::ShaderError> {
        let mut words = vec![
            0xBE94_0300, // s_mov_b32 s20, s0 (SRV table)
            0xBE95_0380, // s_mov_b32 s21, 0
            0xF40C_020A,
            0xFA00_0000, // s_load_dwordx8 s[8:15], s[20:21], null
            0xBE96_0301, // s_mov_b32 s22, s1 (Sampler table)
            0xBE97_0380, // s_mov_b32 s23, 0
            0xF408_040B,
            0xFA00_0000, // s_load_dwordx4 s[16:19], s[22:23], null
            0x7E00_02FF,
            offset_imm, // v_mov_b32 v0, offset_imm
        ];
        if poison_reg.is_some() {
            // s_memtime s[24:25]: unsupported if and only if its value is read.
            words.extend_from_slice(&[0xF490_0600, 0]);
        }
        for r in 1..=9u8 {
            if poison_reg == Some(r) {
                words.push(0x7E00_0218 | (u32::from(r) << 17)); // v_mov_b32 v<r>, s24
            } else {
                words.push(0x7E00_0202 | (u32::from(r) << 17)); // v_mov_b32 v<r>, s2
            }
        }
        words.extend_from_slice(&encode_mimg(opcode, dim, dmask, a16));
        if pixel {
            // exp mrt0, v12, v12, v12, v12 done vm
            words.extend_from_slice(&[0xF800_180F, 0x0C0C_0C0C]);
        } else {
            // v_mov_b32 v10, 0; ds_write_b32 v10, v12 (in-bounds LDS write at 0).
            words.extend_from_slice(&[0x7E14_0280, 0xD834_0000, 0x0000_0C0A]);
        }
        words.push(S_ENDPGM);

        let (mut layout, used) = texture_tables();
        layout.push(root(RootType::Constants, 2, 2));

        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let native = NativeProgram {
            stage: if pixel { HwStage::Ps } else { HwStage::Cs },
            wave64: false,
            vgprs: 32,
            layout,
            used,
            io: if pixel {
                StageIo::Ps {
                    inputs: vec![],
                    input_addr: 0,
                    db_shader_control: 0,
                    patches: vec![],
                }
            } else {
                StageIo::Cs(ComputeIo {
                    rsrc2: 1 << 15 | 3 << 1,
                    threads: [1, 1, 1],
                })
            },
            code,
        };
        let p = ir::build(&native.code, false, 32)?;
        let st = structure::structure(&p)?;
        let res = resource::track(&p, &native.layout, native.stage, Some(&st))?;
        let io = io::raise(&p, &native)?;
        let outputs = if pixel {
            vec![SigElement {
                name: "SV_Target".into(),
                index: 0,
                kind: 16,
                comp: 9,
                interp: 0,
                row: Some(0),
                col: 0,
                cols: 4,
                usage: 0,
                half: false,
            }]
        } else {
            vec![]
        };
        lower(&Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: vec![],
            outputs,
            prims: vec![],
            mesh_stride: 0,
        })
    }

    fn validate_dxil_if_available(dxil: &[u8]) -> Result<(), String> {
        #[cfg(windows)]
        {
            use crate::emit::validator::Validator;
            if let Ok(mut v) = Validator::load(Validator::default_path().as_deref()) {
                let mut copy = dxil.to_vec();
                let ours = copy[4..20].to_vec();
                v.validate(&mut copy)?;
                assert_eq!(copy[4..20], ours[..], "container hash mismatch");
            }
        }
        let _ = dxil;
        Ok(())
    }

    #[test]
    fn mimg_offset_clamp_bias_lod_variants_lower_and_validate() {
        // offset_x = +1 (0x01), offset_y = -2 (0x3E), offset_z = +3 (0x03).
        const OFF_2D: u32 = 0x0000_3E01;
        const OFF_3D: u32 = 0x0003_3E01;

        // (opcode, dmask, pixel_only, a16_supported)
        let cases: &[(u16, u8, bool, bool)] = &[
            (0x031, 0xF, false, true),  // image_sample_cl_o
            (0x032, 0xF, false, true),  // image_sample_d_o
            (0x033, 0xF, false, true),  // image_sample_d_cl_o
            (0x034, 0xF, false, true),  // image_sample_l_o
            (0x035, 0xF, false, true),  // image_sample_b_o
            (0x036, 0xF, false, true),  // image_sample_b_cl_o
            (0x038, 0x1, true, false),  // image_sample_c_o
            (0x039, 0x1, true, false),  // image_sample_c_cl_o
            (0x03F, 0x1, false, false), // image_sample_c_lz_o
        ];

        for &(opcode, dmask, pixel_only, a16_ok) in cases {
            for pixel in [true, false] {
                if pixel_only && !pixel {
                    continue;
                }
                for a16 in [false, true] {
                    if a16 && !a16_ok {
                        continue;
                    }
                    let lowered = lower_mimg_sample(pixel, opcode, 1, dmask, a16, OFF_2D, None)
                        .unwrap_or_else(|e| {
                            panic!("opcode {opcode:#x} pixel={pixel} a16={a16}: {e}")
                        });
                    validate_dxil_if_available(&lowered.dxil).unwrap_or_else(|e| {
                        panic!("DXIL invalid for {opcode:#x} pixel={pixel} a16={a16}: {e}")
                    });
                }
            }
        }

        // 3D `image_sample_o` (0x030, dim=2) with `a16=true` preserves the z offset
        // in bits 16..=21 of the 32-bit offset word (valid in -8..=7, rejected by
        // dxil.dll when out of range at +9).
        let ok_3d = lower_mimg_sample(true, 0x030, 2, 0xF, true, OFF_3D, None).unwrap();
        validate_dxil_if_available(&ok_3d.dxil).expect("3D a16 offset in range");
        #[cfg(windows)]
        {
            use crate::emit::validator::Validator;
            if Validator::load(Validator::default_path().as_deref()).is_ok() {
                let oob_z =
                    lower_mimg_sample(true, 0x030, 2, 0xF, true, 0x0009_3E01, None).unwrap();
                assert!(
                    validate_dxil_if_available(&oob_z.dxil).is_err(),
                    "z offset +9 in bits 16..=21 must not be masked to 0 when a16=true"
                );
            }
        }
    }

    #[test]
    fn mimg_offset_variants_read_clamp_bias_and_explicit_lod_operands() {
        const OFF_2D: u32 = 0x0000_3E01;
        let refused_memtime = |res: Result<Lowered, crate::ShaderError>| {
            matches!(res, Err(crate::ShaderError::Unsupported { .. }))
        };

        // Sanity: poisoning an unread VGPR (`v9`) still succeeds.
        assert!(lower_mimg_sample(true, 0x031, 1, 0xF, false, OFF_2D, Some(9)).is_ok());

        // `image_sample_cl_o` (0x031) in PS: v0=offset, v1=x, v2=y, v3=clamp (a16: v1=(x,y), v2=(clamp,_)).
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x031,
            1,
            0xF,
            false,
            OFF_2D,
            Some(3)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x031,
            1,
            0xF,
            true,
            OFF_2D,
            Some(2)
        )));

        // `image_sample_l_o` (0x034): v0=offset, v1=x, v2=y, v3=lod (must read v3, not force level 0).
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x034,
            1,
            0xF,
            false,
            OFF_2D,
            Some(3)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            false,
            0x034,
            1,
            0xF,
            true,
            OFF_2D,
            Some(2)
        )));

        // `image_sample_b_cl_o` (0x036) in PS: v0=offset, v1=bias, v2=x, v3=y, v4=clamp
        // (a16: v0=offset, v1=bias_f16, v2=(x,y), v3=(clamp,_)).
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x036,
            1,
            0xF,
            false,
            OFF_2D,
            Some(1)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x036,
            1,
            0xF,
            false,
            OFF_2D,
            Some(4)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x036,
            1,
            0xF,
            true,
            OFF_2D,
            Some(1)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x036,
            1,
            0xF,
            true,
            OFF_2D,
            Some(3)
        )));

        // `image_sample_d_cl_o` (0x033): v0=offset, v1..v2=dx, v3..v4=dy, v5..v6=xy, v7=clamp.
        assert!(refused_memtime(lower_mimg_sample(
            false,
            0x033,
            1,
            0xF,
            false,
            OFF_2D,
            Some(7)
        )));

        // `image_sample_c_cl_o` (0x039) in PS: v0=offset, v1=z-compare, v2..v3=xy, v4=clamp.
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x039,
            1,
            0x1,
            false,
            OFF_2D,
            Some(1)
        )));
        assert!(refused_memtime(lower_mimg_sample(
            true,
            0x039,
            1,
            0x1,
            false,
            OFF_2D,
            Some(4)
        )));
    }
}
