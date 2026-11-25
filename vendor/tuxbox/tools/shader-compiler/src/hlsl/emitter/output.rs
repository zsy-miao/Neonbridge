//! Declarations, helper code and the finished HLSL.
use super::*;

impl<'a> Emitter<'a> {
    pub(in crate::hlsl) fn push_helpers(&self, h: &mut String) {
        h.push_str(CUBE_HELPERS);
        if self.insns.iter().any(clamped_product24) {
            h.push_str(MAD24_SAT);
        }
        h.push_str(if self.single_wave32 { HALF_RNE_STRAIGHT_LINE } else { HALF_RNE });
        h.push_str(HALF_HELPERS);
        let has_fp_mode = self.regs.contains("tb_fp_mode");
        if self.insns.iter().any(|ins| matches!(ins.mnemonic.as_str(), "v_cvt_u32_f32" | "v_cvt_i32_f32" | "v_cvt_pk_u8_f32")
            || (has_fp_mode && matches!(ins.mnemonic.as_str(), "v_cvt_f32_i32" | "v_cvt_f32_u32"))) {
            h.push_str(include_str!("../../float_to_integer.hlsl"));
        }
        if self.insns.iter().any(|ins| ins.mnemonic == "v_fract_f32") {
            h.push_str(include_str!("../../fract_f32.hlsl"));
        }
        if self.insns.iter().any(|ins| ins.mnemonic == "v_ldexp_f32") {
            h.push_str(include_str!("../../ldexp_f32.hlsl"));
        }
        if self.insns.iter().any(|i| i.op == OP_DPP_BINARY && matches!(i.mnemonic.as_str(), "v_min_f32" | "v_max_f32")) {
            h.push_str(r#"
uint tb_quad_extrema(uint a, uint b, bool maximum, uint mode) {
    if ((mode & 0x10u) == 0u) {
        if ((a & 0x7fffffffu) < 0x00800000u) a &= 0x80000000u;
        if ((b & 0x7fffffffu) < 0x00800000u) b &= 0x80000000u;
    }
    // Order -0 below +0. Keep operands as bits for the non-IEEE NaN selection rule.
    uint keyA = (a & 0x80000000u) != 0u ? ~a : a ^ 0x80000000u;
    uint keyB = (b & 0x80000000u) != 0u ? ~b : b ^ 0x80000000u;
    uint result = (maximum ? keyA > keyB : keyA < keyB) ? a : b;
    if ((a & 0x7fffffffu) > 0x7f800000u) result = b;
    else if ((b & 0x7fffffffu) > 0x7f800000u) result = a;
    if ((mode & 0x20u) == 0u && (result & 0x7fffffffu) < 0x00800000u) result &= 0x80000000u;
    return result;
}
"#);
        }
        if self.insns.iter().any(|i| i.mnemonic.starts_with("v_cmp") && (i.mnemonic.ends_with("_f64") || i.mnemonic.ends_with("_f32") || i.mnemonic.ends_with("_f16"))) {
            h.push_str(compare::FLOAT_HELPERS);
        }
        if self.insns.iter().any(|i| float64::handles(&i.mnemonic)) {
            h.push_str(float64::HELPERS);
        }
        if self.insns.iter().any(|i| matches!(i.mnemonic.as_str(), "s_lshr_b64" | "s_ashr_i64" | "s_bfe_u64" | "s_bfe_i64")) {
            h.push_str(r#"
uint2 tb_shr64(uint lo, uint hi, uint n, bool sign) {
    n &= 63u;
    uint fill = sign ? asuint(asint(hi) >> 31) : 0u;
    if (n == 0u) return uint2(lo, hi);
    uint k = n & 31u;
    uint upper = sign ? asuint(asint(hi) >> k) : hi >> k;
    return n >= 32u ? uint2(upper, fill) : uint2((lo >> k) | (hi << (32u-k)), upper);
}
uint2 tb_field64(uint lo, uint hi, uint control, bool sign) {
    uint offset = control & 63u, width = min((control >> 16) & 127u, 64u-offset);
    if (width == 0u) return uint2(0u,0u);
    uint2 v = tb_shr64(lo, hi, offset, false);
    uint bit = width - 1u;
    uint fill = sign && (((bit < 32u ? v.x : v.y) >> (bit & 31u)) & 1u) ? ~0u : 0u;
    uint2 mask = uint2(width >= 32u ? ~0u : (1u << width)-1u,
                       width == 64u ? ~0u : width <= 32u ? 0u : (1u << (width-32u))-1u);
    return (v & mask) | (fill & ~mask);
}
"#);
        }
        if self.insns.iter().any(|i| normalized_half_to_integer(&i.mnemonic)) {
            h.push_str(include_str!("../../normalized_half.hlsl"));
        }
        let uses_half_unary = self.insns.iter().any(|i| half_unary_op(&i.mnemonic).is_some());
        let uses_half_output = self.insns.iter().any(|i| {
            let modified = (i.enc == Enc::Vop3 && (i.aux.imm != 0 || i.aux.imm2 != 0))
                || i.aux.sdwa.is_some_and(|sd| sd & 0xe000 != 0);
            modified && (half_unary_op(&i.mnemonic).is_some()
                || integer_to_half(&i.mnemonic) || i.mnemonic == "v_cvt_f16_f32")
        });
        if self.regs.contains("tb_fp_mode") || uses_half_unary || uses_half_output {
            h.push_str(r#"
// V_CVT_F16_F32 uses the destination round mode. Work with magnitudes
// so tiny directed results survive the host's single-precision FTZ mode.
uint tb_mode_half(float value, uint mode) {
    uint bits = asuint(value), magnitude = bits & 0x7fffffffu;
    if (magnitude < 0x00800000u && (mode & 0x10u) == 0u)
        return (bits >> 16) & 0x8000u;
    // FP16_OVFL saturates finite overflow, preserving true infinities.
    if ((mode & 0x800000u) != 0u && magnitude > 0x477fe000u && magnitude < 0x7f800000u)
        return ((bits >> 16) & 0x8000u) | 0x7bffu;
    uint rounding = (mode >> 2) & 3u;
    uint h = tb_f16_cvt(value, rounding != 0u);
    bool away = rounding == ((bits >> 31) != 0u ? 2u : 1u);
    if (away && magnitude < 0x7f800000u && asuint(f16tof32(h & 0x7fffu)) < magnitude) h++;
    return (h & 0x7fffu) < 0x400u && (mode & 0x80u) == 0u ? h & 0x8000u : h;
}
"#);
        }
        if uses_half_unary {
            h.push_str(include_str!("../../half_unary.hlsl"));
        }
        if uses_half_output {
            h.push_str(r#"
uint tb_half_output(uint bits, uint mode, float scale, bool clampResult) {
    // Scaling preserves NaN payloads. CLAMP clears them only in DX10 mode.
    if ((bits & 0x7fffu) > 0x7c00u)
        return clampResult && (mode & 0x100u) != 0u ? 0u : bits;
    // On RDNA2, OMOD is disabled with output denormals or IEEE mode.
    if (scale != 1.0 && (mode & 0x280u) == 0u) {
        float x = (bits & 0x7fffu) == 0u ? 0.0 : f16tof32(bits);
        bits = tb_mode_half(x * scale, mode);
    }
    if (!clampResult) return bits;
    return (bits & 0x8000u) != 0u ? 0u : min(bits, 0x3c00u);
}
"#);
        }
        if self.insns.iter().any(|i| matches!(i.mnemonic.as_str(), "v_fma_f16" | "v_pk_fmac_f16" | "v_fmaak_f16" | "v_fmamk_f16")) {
            h.push_str(r#"
// A binary16 product is exact in binary32. Recover the residual of the addition
// before rounding to half: a rounded float sum alone can land on the wrong tie.
float tb_fma_half(float a, float b, float addend) {
    precise float p = a * b;
    precise float sum = p + addend;
    uint bits = asuint(sum);
    if ((bits & 0x7fffffffu) < 0x7f800000u && (bits & 1u) == 0u) {
        precise float residual = abs(p) >= abs(addend) ? addend - (sum - p) : p - (sum - addend);
        if (residual != 0.0) bits += ((residual > 0.0) == (sum > 0.0)) ? 1u : 0xffffffffu;
    }
    return asfloat(bits);
}
uint tb_fmac_half(uint a, uint b, uint c) {
    return tb_f16_rne(tb_fma_half(f16tof32(a & 65535u), f16tof32(b & 65535u), f16tof32(c & 65535u)));
}
"#);
        }
    }

    /// Declarations of every register variable the body uses. Shaders with
    /// register-relative moves keep their VGPRs in one array (`tb_v`), with each `vN` name
    /// defined as an element of it, so `VGPR[n + M0]` is a plain dynamic index.
    pub(in crate::hlsl) fn reg_decls(&self) -> String {
        let is_v = |r: &str| r.len() > 1 && r.starts_with('v') && r[1..].bytes().all(|b| b.is_ascii_digit());
        let mut out = String::new();
        let scalars: Vec<String> =
            self.regs.iter().filter(|r| !(self.vreg_array && is_v(r))).map(|r| {
                if r == "tb_fp_mode" { format!("{r} = 0xc0u") } else { format!("{r} = 0u") }
            }).collect();
        if !scalars.is_empty() {
            out.push_str(&format!("    uint {};", scalars.join(", ")));
        }
        if self.vreg_array {
            out.push_str("\n    uint tb_v[256] = (uint[256])0;");
            for n in 0..256 {
                out.push_str(&format!("\n#define v{n} tb_v[{n}]"));
            }
        }
        out
    }

    /// Samplers indexed statically from the sampler descriptor heap. HLSL only
    /// allows SamplerDescriptorHeap inside a function, so these are locals of main.
    pub(in crate::hlsl) fn heap_sampler_locals(&self, h: &mut String) {
        for slot in &self.heap_sampler_slots {
            if self.samp_cmp.contains(slot) {
                let _ = writeln!(h, "    SamplerComparisonState scmp{slot} = SamplerDescriptorHeap[{slot}];");
            } else {
                let _ = writeln!(h, "    SamplerState samp{slot} = SamplerDescriptorHeap[{slot}];");
            }
        }
    }

    /// HLSL declarations of every resource the shader uses (constant buffers, textures,
    /// samplers, buffers, UAVs, the embedded code table and LDS).
    pub(in crate::hlsl) fn resource_decls(&self) -> Result<String, String> {
        let ex = self.ex;
        if let Some(slot) = self.table_samplers.intersection(&self.heap_sampler_slots).next() {
            return Err(format!("sampler s{slot} is used from the root signature and from the heap"));
        }
        let mut h = String::new();
        if let Some(ref packed) = self.packed { packed.declarations(&mut h); }
        let root_constants = sampler_heap::constants(ex).or_else(|| {
            // A packed plan already reads its arguments by name.
            if self.packed.is_some() { return None; }
            sampler_heap::rootless_constants(ex)
        });
        if root_constants.is_none() && self.packed.is_none()
            && (self.pixel_heap || (self.compute_heap && ex.stage == Stage::Vertex))
        {
            // A heap index taken from an unresolved root constant would silently be zero.
            if let Some(reg) = sampler_heap::rootless_constant_sgprs(ex).iter().find(|r| self.live.contains(r)) {
                return Err(format!("root constant {reg} of a root-less program is not resolved"));
            }
        }
        if let Some((_, key)) = root_constants {
            let (slot, space, count) = (key.register, key.space, key.count);
            if self.cb_size.contains_key(&slot) && self.binding_spaces.get(&(2, slot)).copied().unwrap_or(0) == space { return Err("root constants overlap a table CBV".into()); }
            let members = match count {
                1 => format!("uint tb_index{slot}_{space};"),
                2..=4 => format!("uint{count} tb_index{slot}_{space};"),
                // Scalars pack without padding: the buffer is exactly the constants.
                _ => (0..count).map(|k| format!("uint {};", sampler_heap::constant_name(&key, k))).collect::<Vec<_>>().join(" "),
            };
            writeln!(h, "cbuffer TBIndex{slot}_{space} : register(b{slot}, space{space}) {{ {members} }};").unwrap();
        }
        if self.resource_heap_base.is_some() && (self.heap_map || !self.compute_heap) {
            h.push_str("ByteAddressBuffer tb_heap_map : register(t0, space32766);\nstruct TBHeapConstants { uint4 data[4096]; };\nuint tb_heap_index(uint i) { return tb_heap_map.Load(0) == 0u ? i : tb_heap_map.Load(4u+i*4u); }\n");
        } else if self.compute_heap {
            // Without TuxBox's index map the guest heap index is the host heap index.
            h.push_str("struct TBHeapConstants { uint4 data[4096]; };\nuint tb_heap_index(uint i) { return i; }\n");
        }
        for (slot, dwords) in &self.cb_size {
            let space = self.space_suffix(2, *slot);
            let _ = writeln!(h, "cbuffer CB{slot} : register(b{}{space}) {{ uint4 cb{slot}[{}]; }};", self.register_of(2, *slot), dwords.div_ceil(4).max(1));
        }
        for (slot, dim) in &self.tex {
            let ty = match dim {
                0 => "Texture1D",
                4 => "Texture1DArray",
                1 => "Texture2D",
                2 => "Texture3D",
                3 => "TextureCube",
                5 => "Texture2DArray",
                6 => "Texture2DMS",
                _ => return Err(format!("texture dim {dim} unsupported")),
            };
            let e = match self.img_ty.get(&(false, *slot)) {
                Some(2) => "int4",
                Some(_) => "uint4",
                None => "float4",
            };
            let space = self.space_suffix(0, *slot);
            let _ = writeln!(h, "{ty}<{e}> tex{slot} : register(t{}{space});", self.register_of(0, *slot));
        }
        for slot in self.samp.iter().filter(|s| !self.heap_sampler_slots.contains(s)) {
            let space = self.space_suffix(3, *slot);
            let _ = writeln!(h, "SamplerState samp{slot} : register(s{}{space});", self.register_of(3, *slot));
        }
        for slot in &self.samp_cmp {
            let space = self.space_suffix(3, *slot);
            if self.samp.contains(slot) {
                return Err(format!("sampler s{slot} used both as a regular and a comparison sampler"));
            }
            if self.heap_sampler_slots.contains(slot) {
                continue;
            }
            let _ = writeln!(h, "SamplerComparisonState scmp{slot} : register(s{}{space});", self.register_of(3, *slot));
        }
        let device_fence = self.persistent_queue || self.packed.as_ref().is_some_and(|p| !p.device_sync_before.is_empty() || !p.device_sync_barriers.is_empty())
            || (self.is_cs && self.insns.iter().any(|i| i.enc == Enc::Mubuf && matches!(i.op, 0x71 | 0x72)));
        let coherent = if device_fence { "globallycoherent " } else { "" };
        for (is_uav, slot) in &self.raw {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            if *is_uav {
                let _ = writeln!(h, "{coherent}RWByteAddressBuffer raw_ubuf{slot} : register(u{}{space});", self.register_of(1, *slot));
            } else {
                let _ = writeln!(h, "ByteAddressBuffer raw_sbuf{slot} : register(t{}{space});", self.register_of(0, *slot));
            }
        }
        for (is_uav, slot) in self.sbuf.keys() {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            let (ty, nm, reg) = if *is_uav { ("RWStructuredBuffer", "ubuf", "u") } else { ("StructuredBuffer", "sbuf", "t") };
            let qualifier = if *is_uav { coherent } else { "" };
            let _ = writeln!(h, "struct TbElem_{nm}{slot} {{ uint w[{}]; }};
{qualifier}{ty}<TbElem_{nm}{slot}> {nm}{slot} : register({reg}{}{space});", self.sbuf_words((*is_uav, *slot)), self.register_of(u32::from(*is_uav), *slot));
            if self.sbuf_flat.contains(&(*is_uav, *slot)) {
                // Word at a byte offset: the element stride is a property of the view, not of
                // the shader, so it is queried.
                let _ = writeln!(h, "uint tb_sbw_{nm}{slot}(uint off) {{ uint n, st; {nm}{slot}.GetDimensions(n, st); st = max(st, 4u); uint e = off / st; return {nm}{slot}[e].w[(off - e * st) >> 2]; }}");
            }
        }
        for (is_uav, slot) in &self.tbuf {
            let space = self.space_suffix(u32::from(*is_uav), *slot);
            let _ = if *is_uav && self.tbuf_uint.contains(slot) {
                writeln!(h, "{coherent}RWBuffer<uint> utb{slot} : register(u{}{space});", self.register_of(1, *slot))
            } else {
                let e = match self.tbuf_ty.get(&(*is_uav, *slot)) {
                    Some(2) => "int4",
                    Some(_) => "uint4",
                    None => "float4",
                };
                if *is_uav {
                    writeln!(h, "{coherent}RWBuffer<{e}> utb{slot} : register(u{}{space});", self.register_of(1, *slot))
                } else {
                    writeln!(h, "Buffer<{e}> stb{slot} : register(t{}{space});", self.register_of(0, *slot))
                }
            };
        }
        for (slot, dim) in &self.uav {
            // Textures used with image atomics hold one uint per texel.
            let e = match self.img_ty.get(&(true, *slot)) {
                Some(2) => "int4",
                Some(_) => "uint4",
                None => "float4",
            };
            let ty = match (dim, self.uav_uint.contains(slot)) {
                (0, false) => format!("RWTexture1D<{e}>"),
                (1, false) => format!("RWTexture2D<{e}>"),
                (2, false) => format!("RWTexture3D<{e}>"),
                (4, false) => format!("RWTexture1DArray<{e}>"),
                (5, false) => format!("RWTexture2DArray<{e}>"),
                (0, true) => "RWTexture1D<uint>".to_string(),
                (1, true) if self.wide_visibility_uavs.contains(slot) => "RWTexture2D<uint64_t>".to_string(),
                (1, true) => "RWTexture2D<uint>".to_string(),
                (2, true) => "RWTexture3D<uint>".to_string(),
                (4, true) => "RWTexture1DArray<uint>".to_string(),
                (5, true) => "RWTexture2DArray<uint>".to_string(),
                _ => return Err(format!("UAV dim {dim} unsupported")),
            };
            let space = self.space_suffix(1, *slot);
            let _ = writeln!(h, "{coherent}{ty} uav{slot} : register(u{}{space});", self.register_of(1, *slot));
        }
        if self.uses_code {
            // Constant tables embedded after the program in the code section. Code pointers
            // are byte offsets from the program start, so readers subtract the program
            // length (in dwords) before indexing. Only the tail is emitted: FXC allows at
            // most 4096 indexable literals.
            let words: Vec<String> = ex
                .isa_data
                .get(ex.isa.len()..)
                .unwrap_or(&[])
                .chunks_exact(4)
                .map(|c| format!("{:#x}", u32::from_le_bytes([c[0], c[1], c[2], c[3]])))
                .collect();
            let _ = writeln!(h, "static const uint tb_code[{}] = {{ {} }};", words.len().max(1), if words.is_empty() { "0".to_string() } else { words.join(", ") });
        }
        if self.micropoly_vote.is_some() {
            writeln!(h, "groupshared uint tb_lds[784];\ngroupshared uint tb_raster_votes[2];").unwrap();
        } else if self.is_cs && self.uses_lds {
            let words = self.reflected_lds_words.or_else(|| wave_cull::lds_words(self)).unwrap_or(8192);
            let _ = writeln!(h, "groupshared uint tb_lds[{words}];");
        }
        Ok(h)
    }

    pub(in crate::hlsl) fn finish(&mut self, is_ps: bool, output_order: &[String], stencil_target: Option<u32>) -> Result<String, String> {
        let ex = self.ex;
        let mut h = String::new();
        let _ = writeln!(h, "// Generated by tuxbox-xshader from RDNA2 microcode ({:?}, {} instrs).", ex.stage, self.insns.len());
        self.push_helpers(&mut h);
        h.push_str(&self.resource_decls()?);
        if self.is_cs {
            // Local thread ID is in v0..v2. Packed argument blocks use s4 for group X;
            // the descriptor-table ABI places group IDs after the root parameters.
            let [tx, ty_, tz] = compute_group(ex);
            let nparams = ex.root_sig.as_ref().map(|r| r.params.len()).unwrap_or(4);
            let g = match self.packed.as_ref().map(|p| &p.abi) {
                None if self.resolve_wave32 || lone_lane::on() && ray_resolve::applies(ex) => ray_resolve::group_x_register(ex)
                    .ok_or("unverified resolve compute user-data layout")?,
                Some(RootAbi::RegisterWave32 | RootAbi::RegisterWave64) => 6 + 2 * (nparams - 2), // two tables followed by root CBV pairs
                Some(RootAbi::ArgumentBlock) => 4,
                Some(RootAbi::Ps09Raster) => 11,
                None if ex.root_sig.as_ref().is_some_and(|r| r.flags & 0x400 != 0 && r.params.len() == 2 && r.params.iter().all(|p| matches!(p.kind, crate::meta::RootParamKind::Table(_))))
                    && tuxbox_dxbc::compute_table::two_tables_and_heap(&ex.fssm) => 7,
                None if sampler_heap::user_data_end(ex).is_some() =>
                    sampler_heap::user_data_end(ex).unwrap(),
                None => sampler_heap::constants(ex).map_or(4 + nparams, |(start, c)| usize::from(start) + c.count as usize),
            };
            if self.compute_swizzle || self.indexed_lane {
                writeln!(h, "groupshared uint tb_swizzle[{}];", tx * ty_ * tz).unwrap();
            }
            if let Some(width) = self.required_wave_size.filter(|_| !self.portable_wave64) {
                writeln!(h, "[WaveSize({width})]").unwrap();
            }
            // These kernels enable all 64 lanes for a reduction. Start the extra
            // rows inactive so they never participate in the guest's memory work.
            let padded = self.wave_cull && matches!([tx, ty_, tz], [4, 4, 1] | [32, 1, 1]);
            let host_y = if padded { 64 / tx } else { ty_ };
            let _ = writeln!(h, "[numthreads({tx}, {host_y}, {tz})]
void main(uint3 gid : SV_GroupID, uint3 gtid : SV_GroupThreadID) {{");
            let active = if self.resolve_wave32 { "0xffffffffu".to_owned() }
                else if padded { format!("uint(gtid.y < {ty_}u)") } else { "1u".to_owned() };
            let _ = writeln!(h, "    uint exec_lo = {active}, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;");
            self.heap_sampler_locals(&mut h);
            for r in ["v0", "v1", "v2"] {
                self.regs.insert(r.to_string());
            }
            for k in 0..3 {
                self.regs.insert(format!("s{}", g + k));
            }
            let _ = writeln!(h, "{}", self.reg_decls());
            if self.page_waves || self.wave_cull {
                writeln!(h, "    uint tb_wave_tid = gtid.x + {tx}u * (gtid.y + {ty_}u * gtid.z);").unwrap();
            }
            let masked_dpp = self.insns.iter().any(|i| i.op == OP_DPP_BINARY && group_extrema::destination_mask(i).is_some());
            if self.compute_swizzle || self.indexed_lane || self.single_wave32 || masked_dpp {
                writeln!(h, "    uint tb_lane = gtid.x + {tx}u * (gtid.y + {ty_}u * gtid.z);").unwrap();
            }
            if self.resolve_wave32 {
                writeln!(h, "    v0 = WaveGetLaneIndex(); v1 = gtid.y; v2 = gtid.z; uint tb_lane_bit = 1u << v0;").unwrap();
            } else {
                writeln!(h, "    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;").unwrap();
            }
            let _ = writeln!(h, "    s{} = gid.x; s{} = gid.y; s{} = gid.z;", g, g + 1, g + 2);
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut h);
            if let Some(plan) = &self.packed { plan.init_args(&mut h); }
            h.push_str(&self.heap_handle_decls());
            h.push_str(&self.body);
            h.push_str("}
");
            return Ok(h);
        }
        // Outputs matching per-vertex pixel-shader inputs must use the same modifier, so the
        // producer needs the macro too (both stages are compiled for the same shader model).
        let out_uses_macro = !is_ps && self.out_interp.0.iter().any(|(_, m)| m == "TB_VERTEX_INPUT");
        if !self.pulled_attrs.is_empty() || out_uses_macro {
            h.push_str("#if __SHADER_TARGET_MAJOR >= 6\n#define TB_VERTEX_INPUT nointerpolation\n#else\n#define TB_VERTEX_INPUT\n#endif\n");
        }
        let _ = writeln!(h, "struct SIn {{");
        // HLSL requires system-generated inputs (SV_IsFrontFace, SV_SampleIndex,
        // SV_PrimitiveID, ...) to follow all ordinary parameters, so emit them last.
        // Field names stay tied to the signature element index.
        let sysgen = |e: &SigElement| is_ps && !matches!(e.sysval, 0 | 1);
        for pass in 0..2 {
            for (ei, e) in ex.input_sig.iter().enumerate() {
                if sysgen(e) != (pass == 1) {
                    continue;
                }
                let (ty, _) = sig_type(e);
                // DXIL validation: with a conservative depth output (SV_DepthLessEqual /
                // SV_DepthGreaterEqual) the position input must be centroid-interpolated.
                let cons_depth = ex.output_sig.iter().any(|o| {
                    let s = semantic(o);
                    s.starts_with("SV_DepthLessEqual") || s.starts_with("SV_DepthGreaterEqual")
                });
                // A parameter read exclusively with V_INTERP_MOV P0 comes from
                // the provoking vertex. Keep other attributes smoothly interpolated.
                let attr = self.ps_attrs.iter().position(|p| e.sysval == 0 && p.registers().contains(&e.register));
                let flat_half = e.comp_type == 6 && ex.flat_inputs.contains(&e.register);
                let flat_p0 = is_ps && attr.is_some_and(|a| self.flat_attrs.contains(&a));
                let pulled = is_ps && attr.is_some_and(|a| self.pulled_attrs.contains(&a));
                let interp = if !is_ps {
                    ""
                } else if e.sysval == 1 && cons_depth {
                    "linear noperspective centroid "
                } else if pulled {
                    "TB_VERTEX_INPUT "
                } else if !matches!(e.comp_type, 3 | 6) || flat_p0 || flat_half {
                    "nointerpolation "
                } else if e.name.contains("_centroid") {
                    "centroid "
                } else {
                    ""
                };
                // SV_Position is not mirrored: the producer declares it `precise` with its own
                // rules, and it had a register of its own in every DXC listing checked.
                if is_ps && !interp.is_empty() && e.sysval != 1 {
                    self.ps_in_interp.0.push((semantic(e), interp.trim_end().to_string()));
                }
                let _ = writeln!(h, "    {interp}{ty} i{ei} : {};", semantic(e));
            }
        }
        if !self.pulled_attrs.is_empty() {
            h.push_str("#if __SHADER_TARGET_MAJOR >= 6\n    float3 tb_bary : SV_Barycentrics;\n#endif\n");
        }
        let pos_field = match ex.input_sig.iter().position(|e| e.sysval == 1) {
            Some(ei) => format!("i.i{ei}"),
            None => "i.tb_pos".to_string(),
        };
        if is_ps && self.uses_quad && pos_field == "i.tb_pos" {
            let _ = writeln!(h, "    float4 tb_pos : SV_Position;");
        }
        let _ = writeln!(h, "}};\nstruct SOut {{");
        let mut outputs: Vec<_> = ex.output_sig.iter().enumerate().collect();
        if !output_order.is_empty() && matches!(ex.stage, Stage::Vertex | Stage::Geometry) {
            outputs.sort_by_cached_key(|(_, e)| {
                let name = semantic(e);
                output_order.iter().position(|s| s.eq_ignore_ascii_case(&name)).unwrap_or(usize::MAX)
            });
        }
        for (ei, e) in outputs {
            let (ty, _) = sig_type(e);
            if let Some(slot) = stencil_target.filter(|_| e.sysval == 69) {
                let _ = writeln!(h, "    uint o{ei} : SV_Target{slot};");
            } else if matches!(e.sysval, 65 | 67 | 68) {
                let _ = writeln!(h, "    float od : {};", semantic(e).trim_end_matches('0'));
            } else if e.sysval == 1 && !is_ps {
                // `precise`: no fast-math on the position computation. The title relies on a
                // depth-only pass and the base pass producing identical depth (the base pass
                // tests without writing); the two are separate shaders here, and without this
                // the compiler may contract or reorder them differently (observed as holes
                // in the character where the base pass failed the depth test).
                let _ = writeln!(h, "    precise {ty} o{ei} : {};", semantic(e));
            } else {
                let interp = if is_ps { None } else { self.out_interp.get(&semantic(e)) };
                let interp = interp.map(|m| format!("{m} ")).unwrap_or_default();
                let _ = writeln!(h, "    {interp}{ty} o{ei} : {};", semantic(e));
            }
        }
        let no_out = ex.output_sig.is_empty();
        if no_out {
            // No outputs (e.g. depth-only / kill-only pixel shader): HLSL forbids empty structs.
            let cut = h.rfind("struct SOut {").unwrap_or(h.len());
            h.truncate(cut);
            let _ = writeln!(h, "void main(SIn i) {{");
        } else if self.is_gs {
            let (nv, _, _, prim, stream) = self.gs_dims()?;
            let _ = writeln!(
                h,
                "}};\n[maxvertexcount({})]\nvoid main({prim} SIn i[{nv}], inout {stream}<SOut> stream) {{",
                self.gs_max_vertices()
            );
        } else {
            let _ = writeln!(h, "}};");
            if is_ps && ex.output_sig.iter().all(|out| out.sysval == 64)
                && crate::meta::pixel_depth_before_shader(&ex.fssm)
            {
                h.push_str("[earlydepthstencil]\n");
            }
            let _ = writeln!(h, "SOut main(SIn i) {{\n    SOut o = (SOut)0;");
        }
        let _ = writeln!(h, "    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;");
        self.heap_sampler_locals(&mut h);
        // Vertex attributes arrive after the fetch call in consecutive groups of four VGPRs
        // from v4, one group per input register the shader reads. A register none of whose
        // elements is read (read mask 0 in the input signature) gets no group: in the skinned
        // depth-only shader d15c18830a4616f9 ATTRIBUTE1/2 (tangents) are unread and the bone
        // indices of ATTRIBUTE3 arrive in v8..v11, not v16..v19.
        let mut init = String::new();
        if self.is_gs {
            // GS entry ABI (from WriteToSliceMainGS): s3 carries the wave's primitive count in
            // bits [15:8]; v0/v1 hold the vertex offsets (v0 = offset0 | offset1 << 16, v1 =
            // offset2), in dwords. One invocation = one primitive.
            let (nv, in_dw, out_dw, _, _) = self.gs_dims()?;
            let _ = writeln!(init, "    uint tb_in[{nv}][{in_dw}]; uint tb_gs[{}]; uint tb_emit = 0u;", out_dw * self.gs_max_vertices());
            let _ = writeln!(init, "    {{ for (uint a = 0u; a < {}u; a++) tb_gs[a] = 0u; }}", out_dw * self.gs_max_vertices());
            let _ = writeln!(init, "    {{ for (uint k = 0u; k < {nv}u; k++) for (uint a = 0u; a < {in_dw}u; a++) tb_in[k][a] = 0u; }}");
            for k in 0..nv {
                for (ei, e) in ex.input_sig.iter().enumerate() {
                    let mut n = 0;
                    for c in 0..4usize {
                        if (e.mask >> c) & 1 != 0 {
                            let acc = format!("i[{k}].i{ei}.{}", XYZW[n]);
                            let val = if e.comp_type == 3 { format!("asuint({acc})") } else { format!("(uint)({acc})") };
                            let _ = writeln!(init, "    tb_in[{k}][{}] = {val};", e.register as usize * 4 + c);
                            n += 1;
                        }
                    }
                }
            }
            for rname in ["s3", "v0", "v1"] {
                self.regs.insert(rname.to_string());
            }
            let _ = writeln!(init, "    s3 = 0x100u; v0 = 0x04000000u; v1 = 0x800u;");
        }
        // In the merged vertex-stage prologue, s3[7:0] is the vertex count.
        // The per-invocation HLSL register model handles one active vertex.
        let merged_count = ex.stage == Stage::Vertex && self.insns.windows(2).take(8).any(|pair| {
            let (count, mask) = (&pair[0], &pair[1]);
            matches!(count.mnemonic.as_str(), "s_bfe_i32" | "s_bfe_u32")
                && count.src == [Opnd::Sgpr(3), Opnd::Literal(0x80000)]
                && mask.mnemonic == "s_bfm_b64" && mask.dst == [Opnd::ExecLo]
                && matches!(mask.src.as_slice(), [width, Opnd::ConstInt(0)] if count.dst == [*width])
        });
        if merged_count {
            init.push_str("    s3 = 1u;\n");
        }
        // ES-GS puts five GS parameters before the ordinary four VS input registers.
        // A standalone VS receives vertex and instance IDs even when it pulls its
        // own buffers and never calls a hardware fetch shader.
        let merged_fetch = merged_count && self.insns.iter().take(16).any(|ins| {
            ins.mnemonic == "s_swappc_b64" && ins.dst == [Opnd::Sgpr(12)] && ins.src == [Opnd::Sgpr(12)]
        });
        let standalone_vs = ex.stage == Stage::Vertex && !merged_count;
        if merged_fetch || standalone_vs {
            for (i, elem) in ex.input_sig.iter().enumerate() {
                let reg = match (elem.sysval, merged_fetch) {
                    (6, false) => "v0", // SV_VertexID
                    (8, false) => "v3", // SV_InstanceID
                    (6, true) => "v5",
                    (8, true) => "v8",
                    _ => continue,
                };
                if standalone_vs && !self.live.iter().any(|r| r == reg) {
                    continue;
                }
                self.regs.insert(reg.to_string());
                let _ = writeln!(init, "    {reg} = (uint)i.i{i};");
            }
        }
        if !is_ps && !self.is_gs {
            let mut group = 0usize;
            for reg in distinct_regs(ex.input_sig.iter().filter(|e| e.sysval == 0)) {
                let read = ex.input_sig.iter().any(|e| e.sysval == 0 && e.register == reg && e.rw_mask != 0);
                if ex.stage == Stage::Vertex && !read {
                    continue;
                }
                let first = if merged_fetch { 9 + 4 * group } else if ex.stage == Stage::Vertex { 4 + 4 * group } else { 4 + 4 * reg as usize };
                group += 1;
                for c in 0..4usize {
                    let v = format!("v{}", first + c);
                    self.regs.insert(v.clone());
                    let val = match sig_lookup(&ex.input_sig, reg, c) {
                        Some((ei, k)) => {
                            let acc = format!("i.i{ei}.{}", XYZW[k]);
                            if ex.input_sig[ei].comp_type == 3 { format!("asuint({acc})") } else { format!("(uint)({acc})") }
                        }
                        // Unsupplied components default to (0,0,0,1) like the hardware fetch.
                        None => (if c == 3 { "0x3f800000u" } else { "0u" }).to_string(),
                    };
                    let _ = writeln!(init, "    {v} = {val};");
                }
            }
        }
        if !self.regs.is_empty() {
            let _ = writeln!(h, "{}", self.reg_decls());
        }
        if is_ps && self.uses_quad {
            let _ = writeln!(init, "    uint tb_quad = ((uint){pos_field}.x & 1u) | (((uint){pos_field}.y & 1u) << 1);");
        }
        if ex.stage == Stage::Vertex {
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut init);
        }
        if is_ps {
            if !self.pulled_attrs.is_empty() {
                init.push_str("#if __SHADER_TARGET_MAJOR >= 6\n");
                for (reg, weight) in [("v0","y"),("v1","z")] {
                    if self.regs.contains(reg) { let _ = writeln!(init, "    {reg} = asuint(i.tb_bary.{weight});"); }
                }
                init.push_str("#endif\n");
            }
            // The native rasterizer packs enabled inputs into consecutive VGPRs.
            sampler_heap::init_constants(ex, self.packed.is_some(), &self.regs, &mut init);
            if let Some(plan) = &self.packed { plan.init_args(&mut init); }
            if let Some(mask) = ex.ps09_pixel_inputs {
                // Bit 7 (line stipple) occupies v0 in both observed layouts.
                // Neither shader reads it. The next enabled input occupies v1.
                if self.live.iter().any(|r| r == "v0") {
                    return Err("PS09 line-stipple input unsupported".into());
                }
                match mask {
                    0x8080 => {
                        let ei = ex.input_sig.iter().position(|e| e.sysval == 1).ok_or("missing PS09 raster position")?;
                        if self.regs.contains("v1") {
                            writeln!(init, "    v1 = (uint(i.i{ei}.x) & 65535u) | (uint(i.i{ei}.y) << 16);").unwrap();
                        }
                    }
                    0x1080 => {
                        let ei = ex.input_sig.iter().position(|e| e.sysval == 9).ok_or("missing PS09 front-face input")?;
                        if self.regs.contains("v1") {
                            writeln!(init, "    v1 = asuint(i.i{ei} != 0u ? 1.0 : -1.0);").unwrap();
                        }
                    }
                    _ => return Err(format!("unsupported PS09 pixel input mask {mask:#x}")),
                }
            } else {
                use crate::meta::{pixel_system_vgpr, PixelSystemInput};
                let layer = ex.input_sig.iter().position(|e| e.sysval == 4 && e.rw_mask != 0);
                let ancillary = layer.and_then(|field| {
                    pixel_system_vgpr(&ex.fssm, PixelSystemInput::Ancillary).map(|slot| (format!("v{slot}"), field))
                });
                if let Some(ei) = ex.input_sig.iter().position(|e| e.sysval == 1) {
                    let p = format!("i.i{ei}");
                    let positions = crate::meta::position_vgprs(&ex.fssm, ex.input_sig[ei].rw_mask);
                    // POS_W_FLOAT is the interpolated clip W, matching D3D SV_Position.w.
                    // Native shaders issue a reciprocal themselves when they need 1/W.
                    let w = if positions.is_some() { format!("{p}.w") } else { format!("1.0 / {p}.w") };
                    let exprs = [format!("{p}.x"), format!("{p}.y"), format!("{p}.z"), w];
                    let positions = positions.unwrap_or([Some(2), Some(3), Some(4), Some(5)]);
                    for (slot, expr) in positions.into_iter().zip(&exprs) {
                        let Some(slot) = slot else { continue; };
                        let v = format!("v{slot}");
                        if self.live.contains(&v) && !ancillary.as_ref().is_some_and(|(reg, _)| *reg == v) {
                            let _ = writeln!(init, "    {v} = asuint({expr});");
                        }
                    }
                }
                if let (Some(slot), Some(field)) = (
                    pixel_system_vgpr(&ex.fssm, PixelSystemInput::FrontFace),
                    ex.input_sig.iter().position(|e| e.sysval == 9 && e.rw_mask != 0),
                ) {
                    let reg = format!("v{slot}");
                    if self.live.contains(&reg) {
                        // Conventional DX pixel programs consume the integer facing
                        // predicate. The FSSM register is zero on a back-facing primitive.
                        let _ = writeln!(init, "    {reg} = i.i{field} != 0u ? 0xffffffffu : 0u;");
                    }
                }
                if let Some((reg, field)) = ancillary.filter(|(reg, _)| self.live.contains(reg)) {
                    // RTA occupies bits 28:16 and the sample number bits 11:8.
                    let mut packed = format!("((uint)i.i{field} & 8191u) << 16");
                    if let Some(sample) = ex.input_sig.iter().position(|e| e.sysval == 10 && e.rw_mask != 0) {
                        let _ = write!(packed, " | (((uint)i.i{sample} & 15u) << 8)");
                    }
                    let _ = writeln!(init, "    {reg} = {packed};");
                }
            }
        }
        h.push_str(&init);
        h.push_str(&self.heap_handle_decls());
        h.push_str(&self.body);
        // Native stencil reference export stores the low byte. An R8_UINT
        // render target instead saturates a 32-bit shader result at 255.
        if let Some(ei) = stencil_target.and_then(|_| ex.output_sig.iter().position(|e| e.sysval == 69)) {
            let _ = writeln!(h, "    o.o{ei} &= 255u;");
        }
        h.push_str(if no_out || self.is_gs { "}\n" } else { "    return o;\n}\n" });
        Ok(h)
    }
}
