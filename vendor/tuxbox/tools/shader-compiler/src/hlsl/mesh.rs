//! Translate the Scarlett NGG mesh ABI. The program computes one logical
//! invocation per vertex/primitive, then asks the geometry engine to allocate
//! export space. A desktop workgroup can service several such logical lanes.
use super::{
    disassemble, distinct_regs, mesh_bindings, mesh_exports, r, semantic, sig_type, Emitter,
    InputInterp, Translated,
};
use crate::{
    meta::SigElement,
    rdna2::{Enc, Insn, Opnd},
    ShaderExtract,
};
use std::{collections::BTreeSet, fmt::Write};

pub(super) fn translate(
    ex: &ShaderExtract,
    interpolation: Option<&InputInterp>,
) -> Result<Translated, String> {
    let info = ex.mesh_info.ok_or("missing mesh PSV runtime information")?;
    if !matches!(info.vertices, 128 | 256)
        || info.threads != [u32::from(info.vertices), 1, 1]
        || info.primitives != 128
        || info.topology != 2
        || info.shared_bytes != 0
        || info.payload != 0
    {
        return Err(format!(
            "unsupported mesh launch or output configuration: {info:?}"
        ));
    }
    if ex
        .output_sig
        .iter()
        .filter(|e| e.sysval == 1 && e.comp_type == 3 && e.mask == 15)
        .count()
        != 1
        || !ex.input_sig.is_empty()
    {
        return Err("mesh stage requires a full position and no incoming vertex signature".into());
    }
    let mut body = disassemble(&ex.isa);
    let exports = mesh_exports::recognize(&body, info.vertices)?;
    let has_primitive_attributes = ex.primitive_sig.iter().any(|element| element.sysval == 0);
    let has_attribute_exports = !exports.primitive_exports.is_empty();
    if has_primitive_attributes != has_attribute_exports {
        return Err("mesh primitive attribute exports do not match PSG1".into());
    }
    body.truncate(exports.start);
    let group_shift = if info.vertices == 128 { 7 } else { 8 };
    validate_launch(&body, group_shift, exports.lane_index)?;
    let root = ex.root_sig.as_ref().ok_or("mesh root signature missing")?;
    let bindings = if info.vertices == 128 {
        Some(mesh_bindings::MeshBindings::read(ex).ok_or("unverified mesh shader bindings")?)
    } else {
        for (parameter, range_type) in [(9, 0), (10, 2)] {
            if !root.params.get(parameter).is_some_and(|p| {
                p.visibility == 7
                    && matches!(&p.kind, crate::meta::RootParamKind::Table(ranges)
                    if ranges.len() == 1 && ranges[0].range_type == range_type && ranges[0].space == 0)
            }) {
                return Err("unrecognized mesh SRV/CBV root-table layout".into());
            }
        }
        None
    };
    validate_lane_ops(&body)?;
    let mut em = Emitter::new(ex, body);
    // The decoder exposes an unused third operand (s0) on two-input VOP3 instructions.
    if let Some(reg) = em.live_in().into_iter().find(|reg| {
        if matches!(reg.as_str(), "s0" | "s3" | "v5") {
            return false;
        }
        match &bindings {
            Some(mesh) => !reg
                .strip_prefix('s')
                .and_then(|n| n.parse().ok())
                .is_some_and(|sgpr| mesh.takes_sgpr(sgpr)),
            None => !matches!(reg.as_str(), "s25" | "s26"),
        }
    }) {
        return Err(format!("unsupported mesh launch register {reg}"));
    }
    em.mesh_bindings = bindings;
    em.resolve_resources()?;
    em.resolve_structured_strides();
    em.emit_body(false)?;
    em.touch(&exports.vertices);
    em.touch(&exports.primitives);
    em.touch(&exports.connectivity);
    let key = r(&exports.connectivity)?;
    let verts = r(&exports.vertices)?;
    let prims = r(&exports.primitives)?;
    em.line(&format!("result.counts = uint2({verts}, {prims});"));
    em.line(&format!("uint conn = {key};"));
    em.line("result.indices = uint3(conn, conn >> 10, conn >> 20) & 1023u;");
    em.line(&format!(
        "uint checkedConn = any(result.indices >= {verts}) ? 0x80000000u : conn;"
    ));
    em.line("result.dropPrimitive = (checkedConn >> 31) != 0;");
    let vertex_regs = distinct_regs(ex.output_sig.iter().filter(|e| e.sysval == 0));
    let prim_regs = distinct_regs(ex.primitive_sig.iter().filter(|e| e.sysval == 0));
    // NGG reserves at least PARAM0 for per-vertex outputs. In a position-only
    // program there is no vertex PARAM instruction; its first primitive is PARAM1.
    let first_primitive = 32 + vertex_regs.len().max(1) as i64;
    write_exports(
        &mut em,
        "primitive",
        &ex.primitive_sig,
        &exports.primitive_exports,
        &prim_regs,
        first_primitive,
    )?;
    // The bounds check can also change a register read by the vertex exports.
    em.touch(&exports.connectivity_without_primitive);
    em.touch(&exports.guarded_connectivity);
    let guarded = r(&exports.guarded_connectivity)?;
    let unguarded = r(&exports.connectivity_without_primitive)?;
    em.line(&format!(
        "{guarded} = lane < {prims} ? checkedConn : {unguarded};"
    ));
    write_exports(
        &mut em,
        "vertex",
        &ex.output_sig,
        &exports.vertex_exports,
        &vertex_regs,
        32,
    )?;
    let mut h = em.resource_decls()?;
    em.push_helpers(&mut h);
    signature(&mut h, "Vertex", &ex.output_sig, interpolation)?;
    signature(&mut h, "Primitive", &ex.primitive_sig, None)?;
    h.push_str("struct MeshLane { uint2 counts; Vertex vertex; Primitive primitive; uint3 indices; bool dropPrimitive; };\n");
    h.push_str("MeshLane evaluate(uint lane, uint group) {\n");
    h.push_str("    MeshLane result = (MeshLane)0;\n    uint exec_lo = 1u, exec_hi = 0u, vcc_lo = 0u, vcc_hi = 0u, scc = 0u, m0 = 0u, nul = 0u;\n");
    for register in ["s3", "v5"] {
        em.regs.insert(register.into());
    }
    writeln!(h, "{}", em.reg_decls()).unwrap();
    writeln!(
        h,
        "    s3 = (lane >> 6) << 24; v5 = group << {group_shift};"
    )
    .unwrap();
    h.push_str(&em.body);
    h.push_str("    return result;\n}\n");
    h.push_str(
        r#"
groupshared uint2 counts;
[outputtopology("triangle")]
[numthreads(128, 1, 1)]
void main(uint lane : SV_GroupIndex, uint3 group : SV_GroupID,
"#,
    );
    writeln!(
        h,
        "          out vertices Vertex vertices[{}], out indices uint3 triangles[128],",
        info.vertices
    )
    .unwrap();
    h.push_str(
        r#"
          out primitives Primitive attributes[128]) {
    MeshLane first = evaluate(lane, group.x);
    if (lane == 0) counts = first.counts;
    GroupMemoryBarrierWithGroupSync();
    uint2 sizes = counts;
"#,
    );
    writeln!(
        h,
        "    if (any(sizes == 0) || any(sizes > uint2({},128))) sizes = 0;",
        info.vertices
    )
    .unwrap();
    h.push_str(
        r#"
    SetMeshOutputCounts(sizes.x, sizes.y);
    if (lane < sizes.y) {
        triangles[lane] = first.dropPrimitive ? uint3(0,0,0) : first.indices;
        first.primitive.cull = first.dropPrimitive;
        attributes[lane] = first.primitive;
    }
    if (lane < sizes.x) vertices[lane] = first.vertex;
"#,
    );
    if info.vertices > 128 {
        h.push_str("    if (lane + 128 < sizes.x) vertices[lane + 128] = evaluate(lane + 128, group.x).vertex;\n");
    }
    h.push_str("}\n");
    Ok(Translated {
        hlsl: h,
        entry: "main",
        profile: "ms_6_5",
        needs_barycentrics: false,
        packed_dot_product: false,
        typed_int64_atomics: false,
        required_wave_size: None,
        portable_compute_group: None,
        ps_input_interp: InputInterp::default(),
    })
}

// Check the actual operand chains that turn the packed launch registers into
// SV_GroupID.x and the logical 0..255 group index. The compiler schedules
// unrelated scalar loads and vector moves among the two chains.
fn validate_launch(body: &[Insn], group_shift: i64, lane_index: Opnd) -> Result<(), String> {
    use Opnd::{ConstInt as I, Literal, Sgpr as S, Vgpr as V};

    fn writes(ins: &Insn, reg: Opnd) -> bool {
        if ins.mnemonic == "v_nop" {
            return false;
        }
        let width = match ins.enc {
            Enc::Smem => 1u16 << (ins.op & 7),
            Enc::Mubuf => Emitter::mubuf_dwords(&ins.mnemonic).unwrap_or(1) as u16,
            _ if ins.mnemonic.ends_with("_b64") => 2,
            _ => 1,
        };
        ins.dst.iter().any(|dst| match (*dst, reg) {
            (S(first), S(n)) | (V(first), V(n)) => n >= first && u16::from(n - first) < width,
            _ => false,
        })
    }

    fn def(code: &[Insn], at: usize, reg: Opnd) -> Option<(usize, &Insn)> {
        code[..at]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, op)| writes(op, reg))
    }

    let mut group_x = false;
    let mut group_index = false;
    for (i, op) in body.iter().enumerate() {
        if op.enc == Enc::Sopp && !matches!(op.mnemonic.as_str(), "s_inst_prefetch" | "s_waitcnt") {
            break;
        }
        match (op.mnemonic.as_str(), op.src.as_slice()) {
            ("s_lshr_b32", [packed @ S(_), I(shift)]) if *shift == group_shift => {
                if let Some((j, read)) = def(body, i, *packed) {
                    group_x |= read.mnemonic == "v_readfirstlane_b32"
                        && read.src == [V(5)]
                        && def(body, j, V(5)).is_none();
                }
            }
            ("v_lshl_add_u32", [wave @ S(_), I(6), lane @ V(_)]) if op.dst == [lane_index] => {
                let Some((bfe_idx, bfe)) = def(body, i, *wave) else {
                    continue;
                };
                let Some((hi_idx, hi)) = def(body, i, *lane) else {
                    continue;
                };
                let [I(-1), low @ V(_), ..] = hi.src.as_slice() else {
                    continue;
                };
                let Some((_, lo)) = def(body, hi_idx, *low) else {
                    continue;
                };
                group_index = bfe.mnemonic == "s_bfe_u32"
                    && bfe.src == [S(3), Literal(0x0004_0018)]
                    && def(body, bfe_idx, S(3)).is_none()
                    && hi.mnemonic == "v_mbcnt_hi_u32_b32"
                    && lo.mnemonic == "v_mbcnt_lo_u32_b32"
                    && lo.src.starts_with(&[I(-1), I(0)])
                    && !body[i + 1..].iter().any(|next| writes(next, lane_index));
            }
            _ => {}
        }
        if group_x && group_index {
            return Ok(());
        }
    }
    Err("unrecognized mesh launch group or wave/lane register calculation".into())
}

/// The logical 64-lane wave is split across desktop invocations. A first-lane
/// broadcast can be scalarized only when its source is already uniform.
fn validate_lane_ops(body: &[Insn]) -> Result<(), String> {
    let mut uniform = BTreeSet::from([5u8]); // the packed dispatch group identifier
    for ins in body {
        let m = ins.mnemonic.as_str();
        if m == "v_nop" {
            continue;
        }
        let uniform_source = |source: &Opnd| match source {
            Opnd::Vgpr(v) => uniform.contains(v),
            Opnd::Sgpr(_) | Opnd::ConstInt(_) | Opnd::ConstF32(_) | Opnd::Literal(_) => true,
            _ => false,
        };
        if m == "v_readfirstlane_b32" && !ins.src.first().is_some_and(uniform_source) {
            return Err(format!(
                "mesh broadcast of a lane-dependent value at {:#x}",
                ins.offset
            ));
        }
        if matches!(
            ins.enc,
            Enc::Ds | Enc::Exp | Enc::Flat | Enc::Mimg | Enc::Mtbuf
        ) || (ins.enc == Enc::Mubuf && !m.starts_with("buffer_load"))
            || (ins.enc == Enc::Vop1 && m.contains("lane") && m != "v_readfirstlane_b32")
            || (ins.enc == Enc::Sopp
                && !matches!(
                    m,
                    "s_nop"
                        | "s_waitcnt"
                        | "s_inst_prefetch"
                        | "s_cbranch_scc1"
                        | "s_cbranch_execz"
                ))
            || (matches!(ins.enc, Enc::Vop2 | Enc::Vop3) && m.contains("lane"))
            || ins.aux.sdwa.is_some_and(|mode| {
                // An unsigned high-word conversion only reads the current lane.
                m != "v_cvt_f32_u32" || mode & !0xff != 0x0005_0600
            })
            || ins.src.iter().any(|op| matches!(op, Opnd::Raw(_)))
        {
            return Err(format!(
                "unsupported mesh operation {m} at {:#x}",
                ins.offset
            ));
        }
        if let Some(Opnd::Vgpr(dst)) = ins.dst.first() {
            // The observed scalarized view loads and 24-bit products have
            // group-uniform addresses/operands at their broadcast sites.
            let same = match m {
                "v_mov_b32" | "v_mul_u32_u24" => ins.src.iter().all(uniform_source),
                "buffer_load_format_x" => ins.src.iter().all(uniform_source),
                _ => false,
            };
            uniform.remove(dst);
            if same {
                uniform.insert(*dst);
            }
            let words = match m {
                "buffer_load_dwordx3" => 3,
                "buffer_load_dwordx2" => 2,
                _ => 1,
            };
            for extra in 1..words {
                if let Some(v) = dst.checked_add(extra) {
                    uniform.remove(&v);
                }
            }
        }
    }
    Ok(())
}

fn signature(
    h: &mut String,
    name: &str,
    elements: &[SigElement],
    interp: Option<&InputInterp>,
) -> Result<(), String> {
    writeln!(h, "struct {name} {{").unwrap();
    for (i, e) in elements.iter().enumerate() {
        if !matches!(e.comp_type, 1..=3)
            || !matches!(e.sysval, 0 | 1)
            || e.mask == 0
            || e.mask > 15
            || !e
                .name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(format!("unsupported mesh output signature element {e:?}"));
        }
        let sem = semantic(e);
        let modifier = interp
            .and_then(|mods| mods.get(&sem))
            .unwrap_or(if e.comp_type == 3 {
                ""
            } else {
                "nointerpolation"
            });
        writeln!(h, "    {modifier} {} attr{i} : {sem};", sig_type(e).0).unwrap();
    }
    if name == "Primitive" {
        h.push_str("    bool cull : SV_CullPrimitive;\n");
    }
    h.push_str("};\n");
    Ok(())
}

fn write_exports(
    em: &mut Emitter<'_>,
    field: &str,
    elements: &[SigElement],
    exports: &[Insn],
    registers: &[u32],
    first_param: i64,
) -> Result<(), String> {
    for (i, e) in elements.iter().enumerate() {
        let target = if e.sysval == 1 {
            12
        } else {
            first_param
                + registers
                    .iter()
                    .position(|r| *r == e.register)
                    .ok_or("unmapped mesh parameter")? as i64
        };
        let export = exports
            .iter()
            .find(|ins| ins.aux.imm == target)
            .ok_or_else(|| {
                format!(
                    "missing mesh export for {}{} (target {target})",
                    e.name, e.index
                )
            })?;
        if export.aux.imm2 & 1 != 0 || (e.mask & !export.aux.mask) != 0 {
            return Err(format!("unsupported packed or masked mesh export {target}"));
        }
        let mut values = Vec::new();
        for ch in 0..4 {
            if e.mask & (1 << ch) == 0 {
                continue;
            }
            let op = export.src.get(ch).ok_or("missing mesh export operand")?;
            em.touch(op);
            let value = r(op)?;
            values.push(match e.comp_type {
                3 => format!("asfloat({value})"),
                2 => format!("asint({value})"),
                _ => value,
            });
        }
        em.line(&format!(
            "result.{field}.attr{i} = {}({});",
            sig_type(e).0,
            values.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_requires_the_packed_group_and_wave64_lane_definitions() {
        let words: [u32; 10] = [
            0xd7650000, 0x000100c1, // v0 = mbcnt_lo(-1, 0)
            0x7e000505, // s0 = readfirstlane(v5)
            0x9381ff03, 0x00040018, // s1 = s3[27:24]
            0xd7660000, 0x000200c1, // v0 = mbcnt_hi(-1, v0)
            0x900a8800, // s10 = s0 >> 8
            0xd7460001, 0x04010c01, // v1 = (s1 << 6) + v0
        ];
        let check = |words: &[u32]| {
            let bytes = words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>();
            validate_launch(&disassemble(&bytes), 8, Opnd::Vgpr(1))
        };
        assert!(check(&words).is_ok());
        let mut wrong_wave = words;
        wrong_wave[4] = 0x00040017;
        assert!(check(&wrong_wave).is_err());
        // An s_load_dwordx4 into s[0:3] clobbers both intermediate values.
        let mut clobbered = words.to_vec();
        clobbered.splice(7..7, [0xf408000d, 0xfa000000]);
        assert!(check(&clobbered).is_err());
        let mut wrong_group = words;
        wrong_group[7] = 0x900a8700;
        assert!(check(&wrong_group).is_err());
    }

    #[test]
    fn only_uniform_values_can_bypass_a_wave_broadcast() {
        let validate = |words: &[u32]| {
            let bytes = words
                .iter()
                .flat_map(|w| w.to_le_bytes())
                .collect::<Vec<_>>();
            validate_lane_ops(&disassemble(&bytes))
        };
        // Entry v5 holds the same packed group identifier in every lane.
        assert!(validate(&[0x7e000505]).is_ok());
        // Counting preceding lanes overwrites v5 with a lane-dependent value.
        assert!(validate(&[0xd7650005, 0x000100c1, 0x7e000505]).is_err());
        // A new constant in v5 is safe to broadcast.
        assert!(validate(&[0x7e0a0281, 0x7e000505]).is_ok());
        // Shared memory would require synchronization among logical Xbox waves.
        assert!(validate(&[0xd8380100, 0x00020100]).is_err());
    }
}
