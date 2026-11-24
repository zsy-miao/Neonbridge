//! Screen probe gathering traces twice. The first interval culls back faces;
//! the second uses the distance selected by the guest's surface-cache pass.
use super::{packed_root::matches_program, Emitter, Enc, Insn, Opnd, ShaderExtract};

pub(super) fn matches(ex: &ShaderExtract) -> bool {
    ex.entry.as_deref() == Some("LumenScreenProbeGatherHardwareRayTracingCS")
        && matches_program(&ex.isa, 0x4c2c, 0x6acc31421e92ad0a)
        && ex.resources.iter().map(|r| (r.res_type, r.lower)).eq([
            (2, 0),
            (2, 1),
            (2, 2),
            (2, 3),
            (1, 0),
            (5, 0),
            (5, 1),
            (4, 2),
            (4, 3),
            (3, 4),
            (5, 5),
            (5, 6),
            (5, 7),
            (3, 8),
            (3, 9),
            (3, 10),
            (3, 11),
            (3, 12),
            (3, 13),
            (5, 14),
            (3, 15),
            (5, 16),
            (3, 17),
            (4, 18),
            (5, 19),
            (8, 0),
            (6, 1),
            (6, 2),
            (8, 3),
        ])
}

pub(super) fn matches_far(ex: &ShaderExtract) -> bool {
    ex.entry.as_deref() == Some("LumenScreenProbeGatherHardwareRayTracingCS")
        && matches_program(&ex.isa, 0x244c, 0x393eb10d10540e4f)
        && ex.resources.iter().map(|r| (r.res_type, r.lower)).eq([
            (2, 0),
            (2, 1),
            (2, 2),
            (2, 3),
            (1, 0),
            (5, 0),
            (5, 1),
            (4, 2),
            (4, 3),
            (3, 4),
            (5, 5),
            (3, 6),
            (3, 7),
            (3, 8),
            (3, 9),
            (3, 10),
            (3, 11),
            (5, 12),
            (3, 13),
            (5, 14),
            (3, 15),
            (4, 16),
            (5, 17),
            (8, 0),
            (6, 1),
            (6, 2),
            (8, 3),
        ])
}

pub(super) fn prepare_far(ops: &mut Vec<Insn>) {
    // The previous probe result selects near T and the world origin at 0xb64..0xb8c.
    ops.retain(|i| !matches!(i.offset, 0xb5c | 0xb6c..=0xb7c | 0xb94..=0x18b8));
    let query = ops
        .iter_mut()
        .find(|i| i.offset == 0xb90)
        .expect("matched far-field ISA");
    query.mnemonic = "tb_radiance_query".into();
    query.enc = Enc::Vop1;
    query.src = [39, 40, 41, 35, 33, 34, 36].map(Opnd::Vgpr).to_vec();
    query.src.extend([Opnd::Sgpr(73), Opnd::Sgpr(51)]);
    query.dst = [39, 40, 41, 43, 47, 48, 49, 44].map(Opnd::Vgpr).to_vec();
}

pub(super) fn prepare(ops: &mut Vec<Insn>) {
    // 2b40/44/60/64 produce the second trace limit and a live queue flag.
    ops.retain(|op| !matches!(op.offset, 0x09a4..=0x16e8 | 0x2b2c..=0x2b3c | 0x2b48..=0x2b5c | 0x2b68..=0x2b9c | 0x2ba4..=0x3898));
    for op in ops {
        let (inputs, outputs): (&[u8], &[u8]) = match op.offset {
            0x09a0 => (
                &[35, 36, 37, 38, 39, 40, 41],
                &[45, 46, 47, 48, 52, 53, 54, 55],
            ),
            0x2ba0 => (
                &[35, 36, 37, 38, 39, 40, 42, 46],
                &[49, 50, 51, 53, 56, 57, 59, 60, 64, 65],
            ),
            _ => continue,
        };
        op.mnemonic = "tb_radiance_query".into();
        op.enc = Enc::Vop1;
        op.src = inputs.iter().copied().map(Opnd::Vgpr).collect();
        if op.offset == 0x09a0 {
            op.src.push(Opnd::Sgpr(50));
        }
        op.src.push(Opnd::Sgpr(51));
        op.dst = outputs.iter().copied().map(Opnd::Vgpr).collect();
    }
}

pub(super) fn emit(em: &mut Emitter<'_>, ins: &Insn) -> bool {
    if ins.offset == 0xb90 {
        em.line(r#"if (exec_lo & 1u) {
            ProbeHit hit = traceProbe(asfloat(uint3(v39,v40,v41)), asfloat(uint3(v35,v33,v34)), asfloat(v36), asfloat(s73), RAY_FLAG_NONE, s51);
            v39 = hit.instanceID;
            v40 = hit.instanceIndex;
            v41 = hit.geometry;
            v43 = hit.contribution;
            v47 = asuint(hit.distance);
            v48 = asuint(hit.normal.x);
            v49 = asuint(hit.normal.y);
            v44 = asuint(hit.normal.z);
            vcc_lo = uint(hit.incomplete);
        }"#);
        return true;
    }
    let (near, far, flags, outputs) = match ins.offset {
        0x09a0 => (
            "v41",
            "s50",
            "RAY_FLAG_CULL_BACK_FACING_TRIANGLES",
            r#"
        v45 = hit.instanceID;
        v46 = hit.instanceIndex;
        v47 = hit.geometry;
        v48 = hit.contribution;
        v52 = asuint(hit.distance);
        v54 = asuint(hit.normal.x);
        v55 = asuint(hit.normal.y);
        v53 = asuint(hit.normal.z);"#,
        ),
        0x2ba0 => (
            "v46",
            "v42",
            "RAY_FLAG_NONE",
            r#"
        v49 = hit.instanceID;
        v50 = hit.instanceIndex;
        v51 = hit.geometry;
        v53 = hit.contribution;
        v57 = asuint(hit.distance);
        v60 = asuint(hit.normal.x);
        v59 = asuint(hit.normal.y);
        v56 = asuint(hit.normal.z);
        v64 = 0xffffffffu;
        v65 = 0xffffffffu;"#,
        ),
        _ => return false,
    };
    em.line(&format!(r#"if (exec_lo & 1u) {{
        ProbeHit hit = traceProbe(asfloat(uint3(v35,v36,v37)), asfloat(uint3(v39,v38,v40)), asfloat({near}), asfloat({far}), {flags}, s51);
        {outputs}
        vcc_lo = uint(hit.incomplete);
    }}"#));
    true
}
