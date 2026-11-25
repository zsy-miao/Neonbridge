use super::{Emitter, Res};
use crate::{ShaderExtract, Stage, rdna2::disassemble};

/// Build a synthetic extract from little-endian instruction words.
fn compute_extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute,
        shader_model: (6, 0),
        required_features: None,
        bitcode_stripped: true,
        fspx_config: None,
        ps09_pixel_inputs: None,
        fssm: Vec::new(),
        entry: Some("main".into()),
        isa_data: isa.clone(),
        isa,
        isa_offset_in_fssm: 0,
        psv_threads: None,
        isa_preamble: [0, 64, 1, 1],
        has_input_sig: false,
        has_output_sig: false,
        has_psv0: false,
        has_root_sig: false,
        input_sig: Vec::new(),
        output_sig: Vec::new(),
        resources: Vec::new(),
        gs_info: None,
        mesh_info: None,
        primitive_sig: Vec::new(),
        root_sig: None,
    }
}

/// Supply the verified sequential atomic with one typed 2D UAV.
fn fixture() -> ShaderExtract {
    let mut ex = compute_extract(&[0xf05c1308, 0x00000301, 0xbf810000]);
    ex.stage = Stage::Pixel;
    ex.entry = Some("synthetic_visibility".into());
    ex.resources.push(crate::psv::PsvResource {
        res_type: 6, space: 0, lower: 0, upper: 0,
    });
    ex
}

/// Bind the synthetic instruction to its uint UAV for emitter checks.
fn bind(ex: &ShaderExtract) -> Emitter<'_> {
    let mut em = Emitter::new(ex, disassemble(&ex.isa));
    em.res.insert(0, vec![Res::Uav(0, 1)]);
    em.uav.insert(0, 1);
    em.uav_uint.insert(0);
    em
}

#[test]
/// Keep the existing sequential encoding and generated atomic stable.
fn verified_atomic_does_not_depend_on_shader_name() {
    let ex = fixture();
    let mut em = bind(&ex);
    em.resolve_wide_image_atomics().unwrap();
    assert!(em.wide_visibility_uavs.contains(&0));
    let op = em.insns[0].clone();
    assert_eq!(op.mnemonic, "image_atomic_umax");
    em.emit_mimg(0, &op).unwrap();
    assert!(em.body.contains("InterlockedMax(uav0[uint2(v1, v2)], (uint64_t(v4) << 32) | v3);"), "{}", em.body);
    println!("BEGIN_ATOMIC\n{}\nEND_ATOMIC", em.body);
}

#[test]
/// Reject unverified resource widths, mixed accesses, and D16 atomics.
fn unverified_width_and_mixed_accesses_stay_rejected() {
    let mut ex = fixture();
    ex.resources[0].res_type = 7;
    let mut em = bind(&ex);
    em.resolve_wide_image_atomics().unwrap();
    assert!(em.wide_visibility_uavs.is_empty());
    let op = em.insns[0].clone();
    assert!(em.emit_mimg(0, &op).is_err());

    let ex = fixture();
    let mut em = bind(&ex);
    let mut other = em.insns[0].clone();
    other.raw[0] = 0xf05c1108;
    em.insns.push(other);
    em.res.insert(2, vec![Res::Uav(0,1)]);
    assert!(em.resolve_wide_image_atomics().is_err());

    let mut em = bind(&ex);
    em.insns[0].raw[1] |= 0x80000000;
    em.resolve_wide_image_atomics().unwrap();
    assert!(em.wide_visibility_uavs.is_empty());
}

/// Supply nonconsecutive coordinates and distinct data VGPRs for GPU readback.
fn nsa_fixture() -> ShaderExtract {
    let mut ex = fixture();
    let words: [u32; 4] = [0xf05c130a, 0x00000301, 5, 0xbf810000];
    ex.isa = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ex.isa_data = ex.isa.clone();
    ex
}

#[test]
/// Prove NSA uses encoded Y=v5 rather than the sequential Y=v2.
fn verified_nsa_atomic_uses_the_encoded_second_coordinate() {
    for stage in [Stage::Pixel, Stage::Compute] {
        let mut ex = nsa_fixture();
        ex.stage = stage;
        let mut em = bind(&ex);
        em.resolve_wide_image_atomics().unwrap();
        assert!(em.wide_visibility_uavs.contains(&0));
        let op = em.insns[0].clone();
        assert_eq!(super::wide_atomic_coordinates(&op).unwrap(), [1, 5]);
        em.emit_mimg(0, &op).unwrap();
        assert!(em.body.contains("InterlockedMax(uav0[uint2(v1, v5)], (uint64_t(v4) << 32) | v3);"), "{}", em.body);
        assert!(!em.body.contains("uint2(v1, v2)"));
        if stage == Stage::Pixel {
            println!("BEGIN_ATOMIC\n{}\nEND_ATOMIC", em.body);
        }
    }
}

#[test]
/// Refuse returning, packed, differently shaped, and nonzero-padded NSA encodings.
fn unverified_nsa_atomic_forms_stay_rejected() {
    for (word, bits) in [
        (0, 1 << 13), // GLC returns the previous value.
        (1, 1 << 30), // A16 changes coordinates.
        (1, 1 << 31), // D16 changes data width.
        (0, 1 << 8),  // DMASK=2.
        (0, 1 << 3),  // DIM=0.
        (0, 1 << 18), // Different atomic opcode.
        (2, 1 << 8),  // Unused extension bytes remain outside this proof.
    ] {
        let mut ex = nsa_fixture();
        let offset = word * 4;
        let value = u32::from_le_bytes(ex.isa[offset..offset + 4].try_into().unwrap()) ^ bits;
        ex.isa[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        let mut em = bind(&ex);
        em.resolve_wide_image_atomics().unwrap();
        assert!(em.wide_visibility_uavs.is_empty(), "word {word}, bit {bits:#x}");
        let op = em.insns[0].clone();
        assert!(em.emit_mimg(0, &op).is_err(), "word {word}, bit {bits:#x}");
    }
    let ex = nsa_fixture();
    let mut em = bind(&ex);
    em.insns[0].src[1] = crate::rdna2::Opnd::Vgpr(2);
    em.resolve_wide_image_atomics().unwrap();
    assert!(em.wide_visibility_uavs.is_empty());
    let op = em.insns[0].clone();
    assert!(em.emit_mimg(0, &op).is_err());

    let mut ex = nsa_fixture();
    ex.stage = Stage::Vertex;
    let mut em = bind(&ex);
    em.resolve_wide_image_atomics().unwrap();
    let op = em.insns[0].clone();
    assert!(em.emit_mimg(0, &op).is_err());
}

#[test]
/// Preserve the stage, single typed resource, and uniform-width requirements for NSA.
fn nsa_atomic_binding_and_stage_guards_stay_rejected() {
    let mut ex = nsa_fixture();
    ex.stage = Stage::Vertex;
    let mut em = bind(&ex);
    em.resolve_wide_image_atomics().unwrap();
    let op = em.insns[0].clone();
    assert!(em.emit_mimg(0, &op).is_err());

    for resource_type in [6, 7] {
        let mut ex = nsa_fixture();
        ex.resources.push(crate::psv::PsvResource {
            res_type: resource_type, space: 0, lower: 0, upper: 0,
        });
        let mut em = bind(&ex);
        em.resolve_wide_image_atomics().unwrap();
        assert!(em.wide_visibility_uavs.is_empty());
        let op = em.insns[0].clone();
        assert!(em.emit_mimg(0, &op).is_err());
    }
    let ex = nsa_fixture();
    let mut em = bind(&ex);
    let mut narrow = em.insns[0].clone();
    narrow.raw[0] = 0xf05c110a;
    em.insns.push(narrow);
    em.res.insert(2, vec![Res::Uav(0, 1)]);
    assert!(em.resolve_wide_image_atomics().is_err());
}

#[test]
/// Keep the packed sequential LOAD emission and its raw flag gate unchanged.
fn packed_wide_load_stays_sequential() {
    let ex = fixture();
    let mut em = bind(&ex);
    let mut op = em.insns[0].clone();
    op.raw[0] = 0xf0001308;
    op.mnemonic = "image_load".into();
    super::packed_root::ps09_raster::emit_wide_image(&mut em, &op, "wide_u10").unwrap();
    assert!(em.body.contains("wide_u10[uint2(v1, v2)]"));
    assert!(em.body.contains("v3 = (uint)val0; v4 = (uint)(val0 >> 32);"));
    op.raw = vec![0xf000130a, 0x301, 5];
    assert!(super::packed_root::ps09_raster::emit_wide_image(&mut em, &op, "wide_u10").is_err());
}
