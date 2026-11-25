//! Far-field Lumen reflection permutations share the world-ray, geometry,
//! normal and budget contract. The guest keeps its material and sky paths.
use super::{packed_root::matches_program, Emitter, Enc, Insn, Opnd, ShaderExtract};

pub(super) fn matches(ex: &ShaderExtract) -> bool {
    if ex.entry.as_deref() != Some("LumenReflectionHardwareRayTracingCS")
        || !ex.resources.iter().map(|r| (r.res_type, r.lower)).eq([
            (2, 0),
            (2, 1),
            (2, 2),
            (2, 3),
            (1, 0),
            (1, 1),
            (5, 0),
            (5, 1),
            (4, 2),
            (4, 3),
            (3, 4),
            (3, 5),
            (5, 6),
            (3, 7),
            (3, 8),
            (3, 9),
            (3, 10),
            (3, 11),
            (3, 12),
            (3, 13),
            (3, 14),
            (4, 15),
            (5, 16),
            (8, 0),
            (8, 1),
            (8, 2),
            (8, 3),
            (6, 4),
            (6, 5),
        ])
    {
        return false;
    }
    matches_program(&ex.isa, 0x1ffc, 0xa69916f28c52697f)
}

pub(super) fn prepare(ops: &mut Vec<Insn>) {
    let bvh = ops
        .iter()
        .find(|op| op.mnemonic == "image_bvh64_intersect_ray")
        .unwrap();
    let mut src = bvh.src[3..9].to_vec();
    src.extend([Opnd::Vgpr(26), bvh.src[2].clone(), Opnd::Sgpr(54)]);
    ops.retain(|op| {
        if op.offset < 0x5e0 {
            return !(op.mnemonic == "s_load_dwordx4" && op.aux.imm == 480
                || matches!(op.mnemonic.as_str(), "flat_load_dword" | "ds_write_b32"));
        }
        op.offset == 0x5e0 || op.offset == 0xda4 || op.offset > 0x1258
    });
    let query = ops.iter_mut().find(|op| op.offset == 0x5e0).unwrap();
    query.enc = Enc::Vop1;
    query.mnemonic = "tb_reflection_far_query".into();
    query.src = src;
    query.dst = [29, 30, 31, 34, 38, 39, 40, 36].map(Opnd::Vgpr).to_vec();
    query.dst.push(Opnd::VccLo);
}

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) {
    let [Opnd::Vgpr(x), Opnd::Vgpr(y), Opnd::Vgpr(z), Opnd::Vgpr(dx), Opnd::Vgpr(dy), Opnd::Vgpr(dz), Opnd::Vgpr(near), Opnd::Vgpr(far), Opnd::Sgpr(budget)] =
        ins.src.as_slice()
    else {
        unreachable!()
    };
    em.line(&format!("if (exec_lo & 1u) {{ ProbeHit hit = traceProbe(asfloat(uint3(v{x},v{y},v{z})), asfloat(uint3(v{dx},v{dy},v{dz})), asfloat(v{near}), asfloat(v{far}), RAY_FLAG_CULL_BACK_FACING_TRIANGLES, s{budget});"));
    for (dst, value) in ins.dst.iter().zip([
        "hit.instanceID",
        "hit.instanceIndex",
        "hit.geometry",
        "hit.contribution",
        "asuint(hit.distance)",
        "asuint(hit.normal.x)",
        "asuint(hit.normal.y)",
        "asuint(hit.normal.z)",
    ]) {
        if let Opnd::Vgpr(v) = dst {
            em.line(&format!("v{v} = {value};"));
        }
    }
    em.line("vcc_lo = uint(hit.incomplete); }");
}
