use super::{Emitter, Res};
use crate::{ShaderExtract, Stage, rdna2::disassemble};

fn compute_extract(words: &[u32]) -> ShaderExtract {
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    ShaderExtract {
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

fn fixture() -> ShaderExtract {
    let mut ex = compute_extract(&[0xf05c1308, 0x00000301, 0xbf810000]);
    ex.stage = Stage::Pixel;
    ex.entry = Some("synthetic_visibility".into());
    ex.resources.push(crate::psv::PsvResource {
        res_type: 6, space: 0, lower: 0, upper: 0,
    });
    ex
}

fn bind(ex: &ShaderExtract) -> Emitter<'_> {
    let mut em = Emitter::new(ex, disassemble(&ex.isa));
    em.res.insert(0, vec![Res::Uav(0, 1)]);
    em.uav.insert(0, 1);
    em.uav_uint.insert(0);
    em
}

#[test]
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
