//! Native wave control for the 64-thread persistent Nanite culling workgroup.
//! The group's scalar PC and both words of each mask remain uniform, even
//! when its vector instructions have only one participating lane.
use super::{
    compute_group, disassemble, hi, persistent_lanes, persistent_resources, r, Emitter, Enc, Insn,
    Opnd, ShaderExtract, Stage, Translated,
};
use std::collections::BTreeSet;
use std::fmt::Write;

pub(super) fn applies(ex: &ShaderExtract) -> bool {
    ex.stage == Stage::Compute
        && ex.entry.as_deref() == Some("PersistentClusterCull")
        && compute_group(ex) == [64, 1, 1]
}

// Native wave64 provides the fast path. On narrower hardware the workgroup
// shares its two mask words and the values addressed by READLANE/DPP.
const HELPERS: &str = r#"
groupshared uint tb_exchange[64];
groupshared uint tb_mask[2];
bool tb_bit(uint2 mask, uint lane) { return (((lane < 32 ? mask.x : mask.y) >> (lane & 31)) & 1u) != 0; }
uint2 tb_group_ballot(bool p, uint lane) {
    if (WaveGetLaneCount() == 64) return WaveActiveBallot(p).xy;
    if (lane < 2) tb_mask[lane] = 0;
    GroupMemoryBarrierWithGroupSync();
    if (p) InterlockedOr(tb_mask[lane >> 5], 1u << (lane & 31));
    GroupMemoryBarrierWithGroupSync();
    uint2 value = uint2(tb_mask[0], tb_mask[1]);
    GroupMemoryBarrierWithGroupSync();
    return value;
}
uint tb_group_read(uint v, uint source, uint lane) {
    if (WaveGetLaneCount() == 64) return WaveReadLaneAt(v, source & 63u);
    tb_exchange[lane] = v;
    GroupMemoryBarrierWithGroupSync();
    uint value = tb_exchange[source & 63u];
    GroupMemoryBarrierWithGroupSync();
    return value;
}
#define tb_ballot(p) tb_group_ballot((p), tb_lane)
#define tb_read(v,l) tb_group_read((v), (l), tb_lane)
#define tb_active tb_bit(uint2(exec_lo,exec_hi), tb_lane)
"#;

pub(super) fn translate(ex: &ShaderExtract) -> Result<Translated, String> {
    let mut em = Emitter::new(ex, disassemble(&ex.isa));
    em.resolve_resources()?;
    em.infer_tbuf_types();
    let instances = ex.entry.as_deref() == Some("InstanceCull");
    if instances {
        super::instance_cull_hzb::wave_resources(&mut em)?;
    } else {
        persistent_resources::prepare(&mut em)?;
    }
    let code = em.insns.clone();
    emit_blocks(&mut em, &code)?;
    let [x, y, z] = compute_group(ex);
    let g = 4 + ex
        .root_sig
        .as_ref()
        .ok_or("missing persistent compute root signature")?
        .params
        .len();
    let mut h = em.resource_decls()?;
    em.push_helpers(&mut h);
    if !instances {
        h.push_str(&persistent_lanes::declarations(ex)?);
    }
    h.push_str(HELPERS);
    for register in ["v0", "v1", "v2"] {
        em.regs.insert(register.into());
    }
    for k in 0..3 {
        em.regs.insert(format!("s{}", g + k));
    }
    writeln!(h,"[numthreads({x},{y},{z})]\nvoid main(uint3 gid:SV_GroupID, uint groupIndex:SV_GroupIndex) {{").unwrap();
    h.push_str("    uint tb_lane = WaveGetLaneCount() == 64 ? WaveGetLaneIndex() : groupIndex;\n");
    h.push_str("    uint exec_lo=~0u, exec_hi=~0u, vcc_lo=0u, vcc_hi=0u, scc=0u, m0=0u, nul=0u;\n");
    h.push_str(&em.reg_decls());
    writeln!(
        h,
        "    v0=tb_lane; v1=0; v2=0; s{g}=gid.x; s{}=gid.y; s{}=gid.z;",
        g + 1,
        g + 2
    )
    .unwrap();
    h.push_str(&em.heap_handle_decls());
    h.push_str(&em.body);
    h.push_str("}\n");
    Ok(Translated {
        hlsl: h,
        entry: "main",
        profile: "cs_6_0",
        needs_barycentrics: false,
        packed_dot_product: false,
        typed_int64_atomics: !em.wide_visibility_uavs.is_empty(),
        required_wave_size: None,
        portable_compute_group: None,
        ps_input_interp: Default::default(),
    })
}

fn condition(i: &Insn) -> Option<&'static str> {
    if i.enc != Enc::Sopp {
        return None;
    }
    match i.op {
        2 => Some("true"),
        4 => Some("scc == 0"),
        5 => Some("scc != 0"),
        6 => Some("(vcc_lo | vcc_hi) == 0"),
        7 => Some("(vcc_lo | vcc_hi) != 0"),
        8 => Some("(exec_lo | exec_hi) == 0"),
        9 => Some("(exec_lo | exec_hi) != 0"),
        _ => None,
    }
}

fn target(code: &[Insn], branch: &Insn) -> Result<usize, String> {
    let dest = (branch.offset as i64 + 4 + branch.aux.imm * 4) as usize;
    code.binary_search_by_key(&dest, |op| op.offset)
        .map_err(|_| {
            format!(
                "persistent branch at {:#x} targets invalid {dest:#x}",
                branch.offset
            )
        })
}

fn emit_blocks(em: &mut Emitter<'_>, code: &[Insn]) -> Result<(), String> {
    let mut leaders = BTreeSet::from([0, code.len()]);
    for (i, op) in code.iter().enumerate() {
        if condition(op).is_some() {
            leaders.insert(target(code, op)?);
            leaders.insert(i + 1);
        }
        if op.mnemonic == "s_endpgm" {
            leaders.insert(i + 1);
        }
    }
    let leaders: Vec<_> = leaders.into_iter().collect();
    em.line("uint tb_pc=0;");
    em.line(&format!(
        "[allow_uav_condition] while (tb_pc < {}u) {{",
        code.len()
    ));
    // A switch gives the driver a single structured merge for the wave PC.
    // A long chain of nested SSA joins exhausts the retail render thread's
    // 1 MiB stack during host pipeline compilation for larger instance kernels.
    let dispatch_switch = em.ex.entry.as_deref() == Some("InstanceCull");
    if dispatch_switch {
        em.line("switch (tb_pc) {");
    }
    for block in leaders.windows(2) {
        let (start, end) = (block[0], block[1]);
        let last = &code[end - 1];
        if dispatch_switch {
            em.line(&format!("case {start}u: {{"));
        } else {
            em.line(&format!("if (tb_pc == {start}u) {{"));
        }
        em.drop_heap_handles();
        for (i, op) in code.iter().enumerate().take(end).skip(start) {
            if condition(op).is_some() || op.mnemonic == "s_endpgm" {
                continue;
            }
            for reg in op.dst.iter().chain(&op.src) {
                em.touch(reg);
            }
            em.cur_idx = i;
            emit_op(em, i, op).map_err(|e| format!("{e} [{} @ {:#x}]", op.mnemonic, op.offset))?;
        }
        let next = if last.mnemonic == "s_endpgm" {
            format!("{}u", code.len())
        } else if let Some(test) = condition(last) {
            format!("({test}) ? {}u : {end}u", target(code, last)?)
        } else {
            format!("{end}u")
        };
        let leave = if dispatch_switch { " break;" } else { "" };
        em.line(&format!("tb_pc = {next};{leave} }}"));
    }
    if dispatch_switch {
        em.line("}");
    }
    em.line("}");
    Ok(())
}

fn emit_op(em: &mut Emitter<'_>, idx: usize, op: &Insn) -> Result<(), String> {
    if persistent_lanes::emit(em, op)? {
        return Ok(());
    }
    let m = op.mnemonic.as_str();
    if m == "s_ff1_i32_b64" {
        let lo = op.src[0];
        let high = hi(&lo);
        em.touch(&high);
        return em.ws(
            &op.dst[0],
            &format!(
                "({a} != 0u ? firstbitlow({a}) : ({b} != 0u ? 32u + firstbitlow({b}) : ~0u))",
                a = r(&lo)?,
                b = r(&high)?
            ),
        );
    }
    if m.starts_with("v_cmp_") {
        let condition = if m.ends_with("_f32") {
            em.compare_float(op, op.aux.mask)?
        } else if m.ends_with("_i32") || m.ends_with("_u32") {
            em.int_compare(m, &op.src[0], &op.src[1])?
        } else {
            return Err("unsupported persistent wave comparison type".into());
        };
        let lo = op.dst[0];
        let high = hi(&lo);
        em.line(&format!(
            "uint2 tb_result{idx}=tb_ballot(tb_active && ({condition}));"
        ));
        em.ws(&lo, &format!("tb_result{idx}.x"))?;
        return em.ws(&high, &format!("tb_result{idx}.y"));
    }
    if m == "v_cndmask_b32" {
        if op.aux.mask & !0x1b != 0 || op.aux.sdwa.is_some() {
            return Err("unsupported modified wave selection".into());
        }
        let mask = op.src.get(2).copied().unwrap_or(Opnd::VccLo);
        let high = hi(&mask);
        em.touch(&high);
        em.line(&format!(
            "if (tb_active) {} = tb_bit(uint2({},{}),tb_lane) ? {} : {};",
            r(&op.dst[0])?,
            r(&mask)?,
            r(&high)?,
            Emitter::select_source(op, r(&op.src[1])?, 1),
            Emitter::select_source(op, r(&op.src[0])?, 0)
        ));
        return Ok(());
    }
    if m == "s_waitcnt" {
        // Queue entries are published to other persistent workgroups. A
        // draining vector-memory wait orders those writes and subsequent reads.
        let wait = op.aux.imm as u16;
        if (wait & 0xc00f) == 0 {
            em.line("DeviceMemoryBarrierWithGroupSync();");
        }
        return Ok(());
    }
    match op.enc {
        Enc::Sop1 | Enc::Sop2 | Enc::Sopk | Enc::Sopc | Enc::Sopp | Enc::Smem => {
            em.emit_insn(idx, op, false, None)
        }
        Enc::Vop1 | Enc::Vop2 | Enc::Vop3 | Enc::Vopc | Enc::Mubuf | Enc::Mtbuf | Enc::Mimg => {
            // The general emitter guards lane-local instructions with bit zero.
            // Narrow its predicate to the current lane only for this instruction;
            // the scalar EXEC pair remains live for the next wave operation.
            if op
                .src
                .iter()
                .chain(&op.dst)
                .any(|o| matches!(o, Opnd::ExecLo | Opnd::ExecHi))
                || op
                    .dst
                    .iter()
                    .any(|o| matches!(o, Opnd::VccLo | Opnd::VccHi | Opnd::Sgpr(_)))
            {
                return Err("unrecognized operation consuming a wave mask".into());
            }
            em.line("{ uint saved_lo = exec_lo; exec_lo = tb_active ? 1u : 0u;");
            em.emit_insn(idx, op, false, None)?;
            em.line("exec_lo = saved_lo; }");
            Ok(())
        }
        _ => Err("unsupported persistent instruction encoding".into()),
    }
}
