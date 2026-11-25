//! Translate the Scarlett NGG mesh ABI. The program computes one logical
//! invocation per vertex/primitive, then asks the geometry engine to allocate
//! export space. A desktop workgroup can service several such logical lanes.
use super::{
    disassemble, distinct_regs, mesh_bindings, mesh_exports, params, r, semantic, sig_type, Emitter, Param,
    InputInterp, Translated,
};
use crate::{
    meta::SigElement,
    rdna2::{Enc, Insn, Opnd},
    ShaderExtract,
};
use std::{collections::BTreeSet, fmt::Write};

/// A mesh shader may write at most 32 KiB of vertex and primitive output. DXC
/// charges 16 bytes per packed signature row, a vertex row for every vertex,
/// and for every primitive one row of indices plus one per primitive
/// attribute. A 20-element vertex with 128 primitives passed at 96 vertices
/// and failed at 97, which is exactly (32768 - 128 * 16) / (20 * 16).
const MESH_OUTPUT_BYTES: usize = 32768;
const MESH_PRIMITIVES: usize = 128;

/// The most vertices a group may declare for `vertex_rows` output elements and
/// `primitive_rows` primitive attributes, capped at the launch's vertex count.
/// Counting one row per element is an upper bound on DXC's packing (22
/// elements packed into 20 rows), so the result always validates. Zero means
/// no vertex fits.
fn mesh_vertex_budget(vertex_rows: usize, primitive_rows: usize, launch: usize) -> usize {
    let primitive_bytes = MESH_PRIMITIVES * 16 * (1 + primitive_rows);
    let per_vertex = vertex_rows.max(1) * 16;
    (MESH_OUTPUT_BYTES.saturating_sub(primitive_bytes) / per_vertex).min(launch)
}

pub(super) fn translate(
    ex: &ShaderExtract,
    interpolation: Option<&InputInterp>,
    output_order: &[String],
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
    let mut clip = ex.output_sig.iter().filter(|e| e.sysval == 2);
    let clip_mask = match (clip.next(), clip.next()) {
        (None, None) => None,
        (Some(e), None)
            if e.name.eq_ignore_ascii_case("SV_ClipDistance")
                && e.index == 0
                && e.comp_type == 3
                && (1..=15).contains(&e.mask) =>
        {
            Some(e.mask)
        }
        _ => return Err("unsupported mesh clip-distance signature".into()),
    };
    let mut body = disassemble(&ex.isa);
    let exports = mesh_exports::recognize(&body, info.vertices, clip_mask)?;
    let has_primitive_attributes = ex.primitive_sig.iter().any(|element| element.sysval == 0);
    let has_attribute_exports = !exports.primitive_exports.is_empty();
    if has_primitive_attributes != has_attribute_exports {
        return Err("mesh primitive attribute exports do not match PSG1".into());
    }
    body.truncate(exports.start);
    if body.iter().any(|op| op.mnemonic == "v_writelane_b32") {
        super::lower_lane_spills(&mut body, false, false)?;
    }
    let group_shift = if info.vertices == 128 { 7 } else { 8 };
    validate_launch(&body, group_shift, exports.lane_index)?;
    let root = ex.root_sig.as_ref().ok_or("mesh root signature missing")?;
    let bindings = if info.vertices == 128 {
        Some(mesh_bindings::read(ex).ok_or("unverified mesh shader bindings")?)
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
    let mut own_reads = waterfall_reads(&body);
    let mut chosen_samplers = BTreeSet::new();
    if let Some(base) = super::mesh_sampler_heap_base(ex) {
        for (reads, image) in sampler_waterfalls(&body, base) {
            own_reads.extend(reads);
            chosen_samplers.insert(image);
        }
    }
    // Every invocation of a 128-vertex shader evaluates exactly once. Votes
    // can rendezvous across both logical waves at forward branch boundaries.
    let wave_control = info.vertices == 128
        && (body
            .iter()
            .any(|op| matches!(op.mnemonic.as_str(), "s_cbranch_vccz" | "s_cbranch_vccnz"))
            || validate_lane_ops(&body, &own_reads, false).is_err_and(|e| e.starts_with("mesh broadcast")));
    validate_lane_ops(&body, &own_reads, wave_control)?;
    let mut em = Emitter::new(ex, body);
    em.chosen_samplers = chosen_samplers;
    for op in em.insns.iter().flat_map(|ins| ins.dst.iter().chain(&ins.src)) {
        if let Opnd::Temp(n @ super::LANE_TEMP_BASE..) = op {
            em.regs.insert(format!("tb_s{n}"));
        }
    }
    em.wave_cull = wave_control;
    em.own_lane_reads = own_reads;
    em.loop_ranges = loops(&em.insns);
    em.hoist_wave_exchanges = wave_control;
    // Class 5 is the resource-heap base. It is not one of the table, raw, or
    // constant SGPRs `MeshLayout` already accepts.
    let heap = super::mesh_heap_base(ex);
    let samplers = super::mesh_sampler_heap_base(ex);
    let live = em.live_in();
    // The decoder exposes an unused third operand (s0) on two-input VOP3 instructions.
    if let Some(reg) = live.iter().find(|reg| {
        // The launch state fills v0..v8; a higher register is simply not set yet.
        let unset = reg.strip_prefix('v').and_then(|n| n.parse::<u16>().ok()).is_some_and(|n| n > 8);
        // User data ends at s39; any later register only holds what the program put there.
        let unset = unset || reg.strip_prefix('s').and_then(|n| n.parse::<u16>().ok()).is_some_and(|n| n >= 40);
        if unset || matches!(reg.as_str(), "s0" | "s3" | "v5") {
            return false;
        }
        match &bindings {
            Some(mesh) => {
                let sgpr = reg.strip_prefix('s').and_then(|n| n.parse::<u8>().ok());
                !sgpr.is_some_and(|sgpr| mesh.takes_sgpr(sgpr) || heap == Some(sgpr) || samplers == Some(sgpr))
            }
            None => !matches!(reg.as_str(), "s25" | "s26"),
        }
    }) {
        return Err(format!("unsupported mesh launch register {reg}"));
    }
    if let Some(sgpr) = heap.filter(|sgpr| {
        bindings.as_ref().is_some_and(|mesh| !mesh.takes_sgpr(*sgpr))
            && live.iter().any(|reg| reg == &format!("s{sgpr}"))
    }) {
        em.resource_heap_base = Some(sgpr);
        em.compute_heap = true;
    }
    if let Some(sgpr) = samplers.filter(|sgpr| {
        bindings.as_ref().is_some_and(|mesh| !mesh.takes_sgpr(*sgpr))
            && live.iter().any(|reg| reg == &format!("s{sgpr}"))
    }) {
        em.sampler_heap_base = Some(sgpr);
        em.compute_heap = true;
    }
    em.mesh_bindings = bindings;
    em.resolve_resources()?;
    em.resolve_structured_strides();
    if let Some(mesh) = &em.mesh_bindings {
        for &(slot, space, count) in mesh.constants.values() {
            if em
                .binding_spaces
                .insert((2, slot), space)
                .is_some_and(|prior| prior != space)
            {
                return Err(format!("mesh constant b{slot} is used in multiple spaces"));
            }
            em.cb_size.insert(slot, count);
        }
    }
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
    let vertex_regs = params::outputs(ex, &ex.output_sig);
    let prim_regs: Vec<_> = distinct_regs(ex.primitive_sig.iter().filter(|e| e.sysval == 0)).into_iter().map(Param::Full).collect();
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
    // A dropped primitive is also degenerate, so its cull flag can go where the outputs
    // would exceed the 32 KiB a mesh shader may write.
    let elements = ex.output_sig.len() + ex.primitive_sig.len() + 1;
    let cull = elements * 16 * usize::from(info.vertices.max(128)) <= 32768;
    let primitives = cull || !ex.primitive_sig.is_empty();
    // Groups with more vertices than this are dropped by the size guard below.
    let primitive_rows = if primitives { ex.primitive_sig.len() + usize::from(cull) } else { 0 };
    let max_vertices = mesh_vertex_budget(ex.output_sig.len(), primitive_rows, usize::from(info.vertices));
    if max_vertices == 0 {
        return Err("mesh vertex outputs exceed the 32 KiB output limit".into());
    }
    let mut h = em.resource_decls()?;
    if wave_control {
        h.push_str(&super::page_waves::declarations([128, 1, 1], false));
    }
    em.push_helpers(&mut h);
    signature(&mut h, "Vertex", &ex.output_sig, &ex.flat_outputs, interpolation, output_order)?;
    signature(&mut h, "Primitive", &ex.primitive_sig, &[], None, output_order)?;
    if !cull {
        h = h.replace("    bool cull : SV_CullPrimitive;\n", "");
    }
    writeln!(h, "struct MeshLane {{ uint2 counts; Vertex vertex; {}uint3 indices; bool dropPrimitive; }};",
        if primitives { "Primitive primitive; " } else { "" }).unwrap();
    h.push_str("MeshLane evaluate(uint lane, uint group) {\n");
    if wave_control {
        h.push_str("    uint tb_wave_tid = lane;\n");
    }
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
    if let Some(mesh) = &em.mesh_bindings {
        for (&base, &(slot, _, count)) in &mesh.constants {
            for (offset, channel) in super::XYZW.iter().take(count as usize).enumerate() {
                let reg = format!("s{}", usize::from(base) + offset);
                if em.regs.contains(&reg) {
                    writeln!(h, "    {reg} = cb{slot}[0].{channel};").unwrap();
                }
            }
        }
    }
    h.push_str(&em.heap_handle_decls());
    em.heap_sampler_locals(&mut h);
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
    write!(
        h,
        "          out vertices Vertex vertices[{}], out indices uint3 triangles[128]{}) {{\n",
        max_vertices,
        if primitives { ",\n\n          out primitives Primitive attributes[128]" } else { "" }
    )
    .unwrap();
    h.push_str(
        r#"    MeshLane first = evaluate(lane, group.x);
    if (lane == 0) counts = first.counts;
    GroupMemoryBarrierWithGroupSync();
    uint2 sizes = counts;
"#,
    );
    writeln!(
        h,
        "    if (any(sizes == 0) || any(sizes > uint2({},128))) sizes = 0;",
        max_vertices
    )
    .unwrap();
    h.push_str(
        r#"
    SetMeshOutputCounts(sizes.x, sizes.y);
    if (lane < sizes.y) {
        triangles[lane] = first.dropPrimitive ? uint3(0,0,0) : first.indices;
        //PRIMITIVE
    }
    if (lane < sizes.x) vertices[lane] = first.vertex;
"#,
    );
    if max_vertices > 128 {
        h.push_str("    if (lane + 128 < sizes.x) vertices[lane + 128] = evaluate(lane + 128, group.x).vertex;\n");
    }
    let mut store = String::new();
    if cull {
        store.push_str("first.primitive.cull = first.dropPrimitive;\n        ");
    }
    if primitives {
        store.push_str("attributes[lane] = first.primitive;");
    }
    h = h.replace("//PRIMITIVE", &store);
    h.push_str("}\n");
    Ok(Translated {
        hlsl: h,
        entry: "main",
        profile: if em.compute_heap { "ms_6_6" } else { "ms_6_5" },
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
fn validate_lane_ops(body: &[Insn], own: &BTreeSet<usize>, wave_control: bool) -> Result<(), String> {
    // Flat loads read the constant tables that follow the code.
    let program_counter = body.iter().any(|op| op.mnemonic == "s_getpc_b64");
    if wave_control {
        // A rendezvous inside a loop could run a different number of times per wave. A lane
        // leaves a loop by its own condition, so only value exchanges are a problem.
        for &(head, back) in &loops(body) {
            if let Some((_, op)) = body[head..=back].iter().enumerate().map(|(i, op)| (head + i, op)).find(|(i, op)| {
                matches!(op.mnemonic.as_str(), "v_readlane_b32" | "s_ff1_i32_b64" | "s_bcnt1_i32_b64")
                    || (op.mnemonic == "v_readfirstlane_b32" && !own.contains(i))
            }) {
                return Err(format!("mesh wave exchange {} at {:#x} inside a loop", op.mnemonic, op.offset));
            }
        }
    }
    let mut uniform = BTreeSet::from([5u8]); // the packed dispatch group identifier
    for (index, ins) in body.iter().enumerate() {
        let m = ins.mnemonic.as_str();
        if m == "v_nop" {
            continue;
        }
        let uniform_source = |source: &Opnd| match source {
            Opnd::Vgpr(v) => uniform.contains(v),
            Opnd::Sgpr(_) | Opnd::ConstInt(_) | Opnd::ConstF32(_) | Opnd::Literal(_) => true,
            _ => false,
        };
        if !wave_control
            && m == "v_readfirstlane_b32"
            && !own.contains(&index)
            && !ins.src.first().is_some_and(uniform_source)
            && !uniform_reciprocal(body, index)
        {
            return Err(format!(
                "mesh broadcast of a lane-dependent value at {:#x}",
                ins.offset
            ));
        }
        if matches!(ins.enc, Enc::Ds | Enc::Exp)
            || (ins.enc == Enc::Flat && !(m.starts_with("flat_load_dword") && program_counter))
            || (ins.enc == Enc::Mimg && !image_read(m))
            || (ins.enc == Enc::Mtbuf && ins.op >= 4)
            || (ins.enc == Enc::Mubuf && !m.starts_with("buffer_load"))
            || (ins.enc == Enc::Vop1 && m.contains("lane") && m != "v_readfirstlane_b32")
            || (wave_control && !wave_mask_operands(ins))
            || (ins.enc == Enc::Sopp
                && !matches!(
                    m,
                    "s_nop"
                        | "s_waitcnt"
                        | "s_inst_prefetch"
                        | "s_branch"
                        | "s_cbranch_scc0"
                        | "s_cbranch_scc1"
                        | "s_cbranch_execz"
                        | "s_cbranch_execnz"
                )
                && !(wave_control && matches!(m, "s_cbranch_vccz" | "s_cbranch_vccnz")))
            || (matches!(ins.enc, Enc::Vop2 | Enc::Vop3) && m.contains("lane"))
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
                _ if ins.enc == Enc::Mtbuf => ins.op as u8 + 1,
                "buffer_load_dwordx4" => 4,
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

/// Instruction ranges of the program's loops, from the head to the backward branch.
fn loops(body: &[Insn]) -> Vec<(usize, usize)> {
    body.iter()
        .enumerate()
        .filter(|(_, op)| op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_some() && op.aux.imm < 0)
        .filter_map(|(back, op)| {
            let target = (op.offset + 4).checked_add_signed(op.aux.imm as isize * 4)?;
            Some((body.iter().position(|op| op.offset == target)?, back))
        })
        .collect()
}

/// First-lane reads that only select which lanes run the rest of an iteration:
/// `s = readfirstlane(v); cmpx_eq(s, v); ...; exec = andn2_wrexec(left); loop while exec`.
/// A lane runs the rest only when `s` is its own `v`, so it can read its own.
fn waterfall_reads(body: &[Insn]) -> BTreeSet<usize> {
    let mut found = BTreeSet::new();
    for (head, back) in loops(body) {
        let closes = body[back].mnemonic == "s_cbranch_execnz"
            && back > head && body[back - 1].mnemonic == "s_andn2_wrexec_b64";
        if !closes {
            continue;
        }
        for i in head..back {
            let read = &body[i];
            let ([s @ Opnd::Sgpr(_)], [v @ Opnd::Vgpr(_)]) = (read.dst.as_slice(), read.src.as_slice()) else { continue };
            if read.mnemonic != "v_readfirstlane_b32" {
                continue;
            }
            // Only waits, scalar loads and further reads may come before the comparison.
            let compare = body[i + 1..back].iter().find(|op| {
                !(op.mnemonic == "v_readfirstlane_b32" || op.mnemonic == "v_nop"
                    || op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_none())
                    && (op.mnemonic.starts_with("v_cmpx") || op.enc != Enc::Smem || op.src.contains(s))
            });
            if compare.is_some_and(|op| op.mnemonic == "v_cmpx_eq_u32" && op.aux.sdwa.is_none() && op.src.len() >= 2
                && (op.src[..2] == [*s, *v] || op.src[..2] == [*v, *s])) {
                found.insert(i);
            }
        }
    }
    found
}

/// Loops that sample with a sampler picked per lane among sampler-heap entries:
/// four first-lane reads of the descriptor words, two 64-bit comparisons joined into EXEC,
/// the sample, and a loop over the lanes left. Returns the reads and the sample.
fn sampler_waterfalls(body: &[Insn], base: u8) -> Vec<([usize; 4], usize)> {
    use Opnd::{ExecLo, Sgpr as S, Vgpr as V};
    let entry = |at: usize, s: u8| {
        body[..at].iter().rev()
            .find(|op| op.dst.iter().any(|d| matches!(d, S(n) if *n <= s && s < n + match op.enc {
                Enc::Smem => 1u8 << (op.op & 7),
                _ if op.mnemonic.ends_with("_b64") => 2,
                _ => 1,
            })))
            .is_some_and(|op| op.mnemonic == "s_load_dwordx4" && op.src.first() == Some(&S(base)) && op.aux.imm % 16 == 0)
    };
    loops(body).into_iter().filter_map(|(head, back)| {
        let code = body.get(head..back)?;
        let kind = |op: &Insn, name: &str| op.mnemonic == name && op.aux.sdwa.is_none();
        let (S(k), V(n)) = (*code.first()?.dst.first()?, *code[0].src.first()?) else { return None };
        let k8 = k;
        for i in 0..4 {
            let op = code.get(i)?;
            if !kind(op, "v_readfirstlane_b32") || op.dst != [S(k8 + i as u8)] || op.src != [V(n + i as u8)] {
                return None;
            }
            // Every value the lane may hold is a word of a sampler-heap entry.
            let mut defs = body[..head].iter().enumerate().rev().filter(|(_, d)| d.dst.contains(&V(n + i as u8)));
            let (at, first) = defs.next()?;
            let source = |at: usize, d: &Insn| kind(d, "v_mov_b32") && matches!(d.src.as_slice(), [S(s)] if entry(at, *s));
            if !source(at, first) {
                return None;
            }
        }
        let rest: Vec<(usize, &Insn)> = code.iter().enumerate().skip(4)
            .filter(|(_, op)| !(op.enc == Enc::Smem || op.mnemonic == "v_nop"
                || op.enc == Enc::Sopp && Emitter::branch_cond(op.op).is_none()))
            .collect();
        let [(_, low), (_, high), (_, both), (_, save), tail @ ..] = rest.as_slice() else { return None };
        let pair = |op: &Insn, i: u8| kind(op, "v_cmp_eq_u64") && op.src.get(..2) == Some(&[S(k8 + i), V(n + i)]);
        let masks = [*low.dst.first()?, *high.dst.first()?];
        if !pair(low, 0) || !pair(high, 2) || both.mnemonic != "s_and_b64" || both.src != masks
            || save.mnemonic != "s_and_saveexec_b64" || save.src != both.dst {
            return None;
        }
        let (image, left) = match tail {
            [image, (_, left)] => (image, left),
            _ => return None,
        };
        if !image.1.mnemonic.starts_with("image_sample") || image.1.src.last() != Some(&S(k8))
            || left.mnemonic != "s_xor_b64" || left.dst != [ExecLo] || body[back].mnemonic != "s_cbranch_execnz" {
            return None;
        }
        Some(([head, head + 1, head + 2, head + 3], head + image.0))
    }).collect()
}

/// Image reads with an explicit level: no implicit derivatives, no writes.
fn image_read(mnemonic: &str) -> bool {
    matches!(mnemonic, "image_load" | "image_load_mip" | "image_get_resinfo")
        || (mnemonic.starts_with("image_sample") || mnemonic.starts_with("image_gather4"))
            && (mnemonic.contains("_lz") || mnemonic.ends_with("_l") || mnemonic.ends_with("_l_o"))
}

fn wave_mask_operands(ins: &Insn) -> bool {
    if ins
        .src
        .iter()
        .chain(&ins.dst)
        .any(|op| matches!(op, Opnd::ExecHi | Opnd::VccHi))
    {
        return false;
    }
    if ins
        .src
        .iter()
        .any(|op| matches!(op, Opnd::ExecLo | Opnd::VccLo))
    {
        // Each invocation stores one bit of a guest mask. These instructions
        // can use that bit directly. Numeric reads of a mask need both words.
        return matches!(
            ins.mnemonic.as_str(),
            "s_cselect_b64" | "s_mov_b64" | "s_and_b64" | "s_andn2_b64" | "v_cndmask_b32"
                | "s_or_b64" | "s_xor_b64" | "s_orn2_b64" | "s_nand_b64" | "s_nor_b64" | "s_xnor_b64"
                | "s_not_b64" | "s_and_saveexec_b64" | "s_or_saveexec_b64" | "s_andn2_saveexec_b64"
        );
    }
    ins.mnemonic != "s_mov_b32" || ins.dst != [Opnd::ExecLo]
}

// The guest builds a division seed from a scalar divisor through four VALU
// operations. Prove every lane actually produced the same value at the read.
fn uniform_reciprocal(body: &[Insn], index: usize) -> bool {
    let Some(start) = index.checked_sub(5) else {
        return false;
    };
    let [convert, scalar, reciprocal, multiply, to_integer, read] = &body[start..=index] else {
        return false;
    };
    use Opnd::{ConstInt, ExecHi, ExecLo, Literal, Sgpr, Vgpr};
    let ([lane @ Vgpr(_)], [divisor @ Sgpr(_)]) = (convert.dst.as_slice(), convert.src.as_slice())
    else {
        return false;
    };
    if convert.mnemonic != "v_cvt_f32_u32"
        || scalar.mnemonic != "s_sub_i32"
        || scalar.src != [ConstInt(0), *divisor]
        || reciprocal.mnemonic != "v_rcp_iflag_f32"
        || reciprocal.src != [*lane]
        || multiply.mnemonic != "v_mul_legacy_f32"
        || multiply.src != [Literal(0x4f7ffffe), *lane]
        || to_integer.mnemonic != "v_cvt_u32_f32"
        || to_integer.src != [*lane]
        || read.src != [*lane]
        || [reciprocal, multiply, to_integer]
            .iter()
            .any(|op| op.dst != [*lane])
        || [convert, reciprocal, multiply, to_integer, read]
            .iter()
            .any(|op| !matches!(op.enc, Enc::Vop1 | Enc::Vop2) || op.aux.sdwa.is_some())
    {
        return false;
    }
    !body[..index].iter().any(|op| {
        let name = op.mnemonic.as_str();
        let full_exec = name == "s_mov_b64" && op.dst == [ExecLo] && op.src == [ConstInt(-1)];
        (!full_exec && (op.dst.contains(&ExecLo) || op.dst.contains(&ExecHi)))
            || name.starts_with("v_cmpx_")
            || name.contains("_saveexec_")
            || name.contains("_wrexec_")
            || (op.enc == Enc::Sopp
                && Emitter::branch_cond(op.op).is_some()
                && (convert.offset as i64 + 1..=read.offset as i64)
                    .contains(&(op.offset as i64 + 4 + op.aux.imm * 4)))
    })
}

fn signature(
    h: &mut String,
    name: &str,
    elements: &[SigElement],
    flat: &[u32],
    interp: Option<&InputInterp>,
    output_order: &[String],
) -> Result<(), String> {
    writeln!(h, "struct {name} {{").unwrap();
    // A linked program declares its outputs in the pixel consumer's register
    // order, so DXC packs them where the consumer reads them. Field names keep
    // the signature index that the exports write.
    let mut ordered: Vec<_> = elements.iter().enumerate().collect();
    if !output_order.is_empty() {
        ordered.sort_by_cached_key(|(_, e)| {
            let name = semantic(e);
            output_order.iter().position(|s| s.eq_ignore_ascii_case(&name)).unwrap_or(usize::MAX)
        });
    }
    for (i, e) in ordered {
        if !(matches!(e.comp_type, 1..=3) || e.comp_type == 6 && name == "Vertex")
            || !(matches!(e.sysval, 0 | 1) || name == "Vertex" && e.sysval == 2)
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
            .unwrap_or(if e.comp_type == 3 || e.comp_type == 6 && !flat.contains(&e.register) {
                ""
            } else {
                "nointerpolation"
            });
        // A pixel consumer that pulls attributes declares them with the
        // TB_VERTEX_INPUT macro (nointerpolation on SM 6); mesh shaders are SM 6.
        let modifier = if modifier == "TB_VERTEX_INPUT" { "nointerpolation" } else { modifier };
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
    registers: &[Param],
    first_param: i64,
) -> Result<(), String> {
    for (i, e) in elements.iter().enumerate() {
        let param = registers.iter().position(|p| p.registers().contains(&e.register)).filter(|_| e.sysval == 0);
        let target = match e.sysval {
            1 => 12,
            2 => 13,
            _ => first_param + param.ok_or("unmapped mesh parameter")? as i64,
        };
        let Some(export) = exports.iter().find(|ins| ins.aux.imm == target) else {
            // The program leaves the output unset; its value is undefined.
            let last = first_param + registers.len() as i64;
            if e.sysval == 0 && field == "vertex" && exports.iter().all(|ins| ins.aux.imm < last) {
                continue;
            }
            return Err(format!(
                "missing mesh export for {}{} (target {target})",
                e.name, e.index
            ));
        };
        if export.aux.imm2 & 1 != 0 {
            return Err(format!("unsupported packed mesh export {target}"));
        }
        let mut values = Vec::new();
        for ch in 0..4 {
            if e.mask & (1 << ch) == 0 {
                continue;
            }
            // A channel the program never exports has no defined value.
            let (ch, high) = match param {
                Some(p) => registers[p].channel(e.register, ch).ok_or("unmapped mesh parameter")?,
                None => (ch, None),
            };
            let op = if export.aux.mask & (1 << ch) == 0 { &Opnd::ConstInt(0) } else { export.src.get(ch).ok_or("missing mesh export operand")? };
            em.touch(op);
            let value = r(op)?;
            if let Some(high) = high {
                values.push(format!("f16tof32(({value} >> {}) & 0xffffu)", if high { 16 } else { 0 }));
                continue;
            }
            values.push(match e.comp_type {
                3 | 6 => format!("asfloat({value})"),
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
fn fssm_rows(rows: &[[u32; 5]]) -> Vec<u8> {
    let mut program = vec![0; 0x100];
    program[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
    let offset = (program.len() - 16) as u64;
    program[0xe8..0xe8 + 8].copy_from_slice(&offset.to_le_bytes());
    program[0xf8..0xf8 + 4].copy_from_slice(&(rows.len() as u32).to_le_bytes());
    program.extend(rows.iter().flatten().copied().flat_map(u32::to_le_bytes));
    let size = (program.len() - 16) as u32;
    program[8..12].copy_from_slice(&size.to_le_bytes());
    program
}

#[cfg(test)]
pub(super) fn heap_extract(rows: &[[u32; 5]], flags: u32, isa: &[u8]) -> crate::ShaderExtract {
    crate::ShaderExtract {
        stage: crate::Stage::Mesh,
        shader_model: (6, 5),
        required_features: Some(1 << 25),
        entry: None,
        isa: isa.to_vec(),
        isa_data: isa.to_vec(),
        isa_offset_in_fssm: 0,
        isa_preamble: [0; 4],
        fssm: fssm_rows(rows),
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        psv_threads: None,
        flat_inputs: Vec::new(),
        flat_outputs: Vec::new(),
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        primitive_sig: Vec::new(),
        has_input_sig: false,
        has_output_sig: false,
        has_root_sig: true,
        has_psv0: false,
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        root_sig: Some(tuxbox_dxbc::rootsig::RootSig {
            version: 1,
            flags,
            params: Vec::new(),
            num_static_samplers: 0,
            static_samplers: Vec::new(),
        }),
    }
}

#[cfg(test)]
fn assert_class5_mesh_pointer() {
    let row = [5, 0, 1, 0, 0x0e];
    let ex = heap_extract(&[row], 0x400, &[]);
    assert_eq!(super::mesh_heap_base(&ex), Some(22));
    let mut compute = ex.clone();
    compute.stage = crate::Stage::Compute;
    assert_eq!(super::mesh_heap_base(&compute), None);
    assert_eq!(super::mesh_heap_base(&heap_extract(&[row], 0, &[])), None);
    assert_eq!(super::mesh_heap_base(&heap_extract(&[row, row], 0x400, &[])), None);
    let wide = [5, 0, 2, 0, 0x0e];
    assert_eq!(super::mesh_heap_base(&heap_extract(&[wide], 0x400, &[])), None);
}

#[cfg(test)]
fn assert_class5_descriptor_load() {
    // s4 = s4 << 5; s[4:7] = s[22:23][s4]; s8 = buffer[s4][0].
    let words = [0x8f048504, 0xf408010b, 0x08000000, 0xf4200202, 0xfa000000];
    let isa: Vec<u8> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    let ex = heap_extract(&[[5, 0, 1, 0, 0x0e]], 0x400, &isa);
    let insns = disassemble(&ex.isa);
    assert_eq!(
        insns.iter().map(|ins| ins.mnemonic.as_str()).collect::<Vec<_>>(),
        vec!["s_lshl_b32", "s_load_dwordx4", "s_buffer_load_dword"]
    );
    let mut em = Emitter::new(&ex, insns.clone());
    em.resource_heap_base = super::mesh_heap_base(&ex);
    em.compute_heap = true;
    em.resolve_resources().unwrap();
    assert!(matches!(em.res.get(&1).map(Vec::as_slice), Some([super::Res::HeapLoad(4)])));
    assert!(matches!(em.res.get(&2).map(Vec::as_slice), Some([super::Res::HeapBuffer(4)])));
    for (idx, ins) in insns.iter().enumerate() {
        em.emit_insn(idx, ins, false, None).unwrap();
    }
    assert!(em.body.contains("s4 = tb_heap_index(s4 >> 5)"));
    let binds = |body: &str| body.matches("ResourceDescriptorHeap[s4]").count();
    assert_eq!(binds(&em.body), 1);
    em.emit_insn(2, &insns[2], false, None).unwrap();
    assert_eq!(binds(&em.body), 1, "unchanged s4 reuses the heap local");
    em.ws(&Opnd::Sgpr(4), "s4 + 1u").unwrap();
    em.emit_insn(2, &insns[2], false, None).unwrap();
    assert_eq!(binds(&em.body), 2, "a write of s4 binds the descriptor again");
    let mut open = true;
    em.end_guard(&mut open);
    em.emit_insn(2, &insns[2], false, None).unwrap();
    assert_eq!(binds(&em.body), 3, "a skipped guard binds the descriptor again");
    em.drop_heap_handles();
    em.emit_insn(2, &insns[2], false, None).unwrap();
    assert_eq!(binds(&em.body), 4, "a loop rebinds the descriptor");
    assert!(!em.body.contains("NonUniformResourceIndex"));
    let handles = em.heap_handle_decls();
    assert_eq!(handles.matches("ByteAddressBuffer").count(), 1);
    assert!(handles.contains("ByteAddressBuffer tb_heap_raw_4;"));
    let decls = em.resource_decls().unwrap();
    assert!(decls.contains("uint tb_heap_index(uint i) { return i; }"));
    assert!(!decls.contains("space32766"));
}

#[cfg(test)]
#[path = "../tests/hlsl/mesh.rs"]
mod tests;

#[cfg(test)]
#[path = "../tests/hlsl/mesh/output_budget_tests.rs"]
mod output_budget_tests;
