//! Lower the inline closest-hit walk. Tile addressing, probe ray generation,
//! card lighting, feedback, sky misses and output packing use the guest ISA.
use super::{
    compute_group, packed_root::matches_program, Emitter, Enc, Insn, Opnd, ShaderExtract, Stage,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Probe {
    Radiance,
    Deferred,
    FarField,
    ScreenGather,
    ScreenGatherFar,
    Reflection,
    ReflectionFar,
}

pub(crate) fn identify(ex: &ShaderExtract) -> Option<Probe> {
    if ex.stage != Stage::Compute
        || compute_group(ex) != [32, 1, 1]
        || !ex
            .resources
            .iter()
            .all(|r| r.space == 0 && r.lower == r.upper)
    {
        return None;
    }
    if super::reflection_far::matches(ex) {
        return Some(Probe::ReflectionFar);
    }
    if super::reflection_probe::matches(ex) {
        return Some(Probe::Reflection);
    }
    if super::screen_gather::matches_far(ex) {
        return Some(Probe::ScreenGatherFar);
    }
    if super::screen_gather::matches(ex) {
        return Some(Probe::ScreenGather);
    }
    if ex.entry.as_deref() != Some("LumenRadianceCacheHardwareRayTracingCS") {
        return None;
    }
    let probe = if matches_program(&ex.isa, 7596, 0x418acad35ebb3a59) {
        Probe::Radiance
    } else if matches_program(&ex.isa, 7780, 0xa69322e96bc18590) {
        Probe::Deferred
    } else if matches_program(&ex.isa, 7668, 0xbbbba26659dfea7c) {
        Probe::FarField
    } else {
        return None;
    };
    let shared = [
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
        (5, 5),
        (3, 6),
        (3, 7),
        (5, 8),
        (3, 9),
        (3, 10),
        (3, 11),
        (3, 12),
        (4, 13),
        (5, 14),
        (8, 0),
        (8, 1),
    ];
    let extra = (probe != Probe::Radiance).then_some((8, 2));
    ex.resources
        .iter()
        .map(|r| (r.res_type, r.lower))
        .eq(shared.into_iter().chain(extra))
        .then_some(probe)
}

pub(super) fn prepare(probe: Probe, ops: &mut Vec<Insn>) {
    // LDS stack initialization and the raw TLAS root belong to the replaced
    // walk. Retain the arithmetic interleaved with those instructions.
    let (scratch, query_pc, last, inputs, outputs): (_, _, _, &[u8], &[u8]) = match probe {
        Probe::ScreenGatherFar => return super::screen_gather::prepare_far(ops),
        Probe::ReflectionFar => return super::reflection_far::prepare(ops),
        Probe::Reflection => return super::reflection_probe::prepare(ops),
        Probe::ScreenGather => return super::screen_gather::prepare(ops),
        Probe::Radiance => (
            [0x324, 0x344, 0x37c, 0x3a0],
            0x554,
            0x11a8,
            &[19, 20, 21, 25, 26, 27, 28, 39],
            &[33, 34, 35, 37, 38, 40, 41, 42],
        ),
        Probe::Deferred => (
            [0x32c, 0x34c, 0x384, 0x3a8],
            0x55c,
            0x11d0,
            &[21, 22, 23, 27, 28, 29, 30, 41],
            &[37, 38, 39, 40, 44, 46, 47, 48, 51, 52],
        ),
        Probe::FarField => (
            [0x37c, 0x520, 0x550, 0x574],
            0x57c,
            0x11e8,
            &[28, 38, 41, 42, 43, 44, 45, 46],
            &[31, 32, 33, 34, 37, 38, 39, 40],
        ),
    };
    ops.retain(|op| !(scratch.contains(&op.offset) || (op.offset > query_pc && op.offset <= last)));
    let query = ops
        .iter_mut()
        .find(|op| op.offset == query_pc)
        .expect("matched probe ISA");
    query.mnemonic = "tb_radiance_query".into();
    query.enc = Enc::Vop1;
    query.src = inputs.iter().copied().map(Opnd::Vgpr).collect();
    query.dst = outputs.iter().copied().map(Opnd::Vgpr).collect();
}

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) {
    if ins.offset == 0x0648 {
        return super::reflection_probe::emit(em);
    }
    if super::screen_gather::emit(em, ins) {
        return;
    }
    if ins.offset == 0x57c {
        em.line(r#"if (exec_lo & 1u) {
        ProbeHit hit = traceProbe(asfloat(uint3(v42, v44, v46)), asfloat(uint3(v41, v43, v45)), asfloat(v28), asfloat(v38));
        v31 = hit.instanceID;
        v32 = hit.instanceIndex;
        v33 = hit.geometry;
        v34 = hit.contribution;
        v38 = asuint(hit.distance);
        v39 = asuint(hit.normal.x);
        v40 = asuint(hit.normal.y);
        v37 = asuint(hit.normal.z);
        vcc_lo = uint(hit.incomplete);
    }"#);
        return;
    }
    if ins.offset == 0x55c {
        em.line(r#"if (exec_lo & 1u) {
        ProbeHit hit = traceProbe(asfloat(uint3(v29, v28, v30)), asfloat(uint3(v22, v21, v23)), asfloat(v41), asfloat(v27));
        v37 = hit.instanceID;
        v38 = hit.instanceIndex;
        v39 = hit.geometry;
        v40 = hit.contribution;
        v44 = asuint(hit.distance);
        v48 = asuint(hit.normal.x);
        v47 = asuint(hit.normal.y);
        v46 = asuint(hit.normal.z);
        // Portable traversal leaves each bookmark invalid for a full retrace.
        v51 = 0xffffffffu;
        v52 = 0xffffffffu;
        vcc_lo = uint(hit.incomplete);
    }"#);
        return;
    }
    em.line(r#"if (exec_lo & 1u) {
        ProbeHit hit = traceProbe(asfloat(uint3(v27, v26, v28)), asfloat(uint3(v20, v19, v21)), asfloat(v39), asfloat(v25));
        v41 = asuint(hit.distance);
        v33 = hit.instanceID;
        v34 = hit.instanceIndex;
        v35 = hit.geometry;
        v37 = hit.contribution;
        v40 = asuint(hit.normal.x);
        v42 = asuint(hit.normal.y);
        v38 = asuint(hit.normal.z);
        vcc_lo = uint(hit.incomplete);
    }"#);
}

pub(super) const DECLARATIONS: &str = include_str!("radiance_probe.hlsl");
