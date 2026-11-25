//! Near-field reflection query. The material table, sky and deferred ray queue
//! continue through the original shader after the committed-hit fields are set.
use super::{Emitter, Enc, Insn, Opnd, ShaderExtract, packed_root::matches_program};

pub(super) fn matches(ex: &ShaderExtract) -> bool {
    ex.entry.as_deref() == Some("LumenReflectionHardwareRayTracingCS")
        && matches_program(&ex.isa, 0x3350, 0xc051029694c16e15)
        && ex.resources.iter().map(|r| (r.res_type, r.lower)).eq([
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
            (5, 15),
            (3, 16),
            (3, 17),
            (4, 18),
            (5, 19),
            (8, 0),
            (8, 1),
            (8, 2),
            (8, 3),
            (6, 4),
            (6, 5),
        ])
}
pub(super) fn prepare(ops: &mut Vec<Insn>) {
    // Keep the roughness bit test at 0xe1c and the translated view origin at
    // 0xe24: both are scheduled among the guest's committed-hit unpack.
    ops.retain(|op| !matches!(op.offset,0x062c|0x0650..=0x0e18|0x0e20|0x0e28..=0x12f0));
    let query = ops
        .iter_mut()
        .find(|op| op.offset == 0x0648)
        .expect("matched reflection walker");
    query.enc = Enc::Vop1;
    query.mnemonic = "tb_radiance_query".into();
    query.src = [19, 20, 21, 30, 31, 32, 36, 41].map(Opnd::Vgpr).to_vec();
    query.src.push(Opnd::Sgpr(36));
    query.dst = [36, 37, 38, 39, 40, 41, 46, 48, 49, 50]
        .map(Opnd::Vgpr)
        .to_vec();
    query.dst.push(Opnd::Sgpr(1));
}
pub(super) fn emit(em: &mut Emitter<'_>) {
    em.line(r#"if (exec_lo & 1u) {
        ProbeHit hit = traceProbe(asfloat(uint3(v30,v31,v32)), asfloat(uint3(v19,v20,v21)), asfloat(v36), asfloat(v41), RAY_FLAG_CULL_BACK_FACING_TRIANGLES, s36);
        v36 = hit.instanceID;
        v37 = hit.instanceIndex;
        v38 = hit.geometry;
        v39 = hit.contribution;
        v40 = asuint(hit.distance);
        v50 = asuint(hit.normal.x);
        v49 = asuint(hit.normal.y);
        v48 = asuint(hit.normal.z);
        v41 = 0xffffffffu;
        v46 = 0xffffffffu;
        s1 = uint(hit.incomplete);
    }"#);
}
