use crate::{hlsl, ShaderExtract, Stage};

const V4: [u32; 2] = [0x7e08_02ff, 0x1357_2468];
const V5: [u32; 2] = [0x7e0a_02ff, 0x2468_1357];
const ENDPGM: u32 = 0xbf81_0000;

/// Translate `v_mov_b32 v4/v5, literal`, then `body`, then `s_endpgm`.
pub(in crate::hlsl) fn translate(stage: Stage, body: &[u32]) -> Result<String, String> {
    let words: Vec<u32> = V4.iter().chain(&V5).chain(body).chain(&[ENDPGM]).copied().collect();
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ex = ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage, shader_model: (6, 0), required_features: None, bitcode_stripped: true,
        fspx_config: None, ps09_pixel_inputs: None, fssm: Vec::new(), entry: Some("main".into()),
        isa_data: isa.clone(), isa, isa_offset_in_fssm: 0, psv_threads: None, isa_preamble: [0, 1, 1, 1],
        has_input_sig: false, has_output_sig: false, has_psv0: false, has_root_sig: false,
        input_sig: Vec::new(), output_sig: Vec::new(), resources: Vec::new(), gs_info: None,
        mesh_info: None, primitive_sig: Vec::new(), root_sig: None,
    };
    hlsl::translate(&ex).map(|t| t.hlsl)
}

#[test]
fn pair_forms_write_both_destination_words() {
    // LLVM gfx1030: v_floor_f64_e32 v[8:9], v[4:5]
    let hlsl = translate(Stage::Compute, &[0x7e10_3504]).unwrap();
    assert!(hlsl.contains("tb_f64_round(uint2(v4, v5), 1u, true)"), "{hlsl}");
    assert!(hlsl.contains("v8 = t") && hlsl.contains("v9 = t"), "{hlsl}");
    // v_rndne_f64_e64 v[8:9], v[4:5]
    assert!(translate(Stage::Compute, &[0xd599_0008, 0x0201_0104]).unwrap().contains(", 3u, true)"));
    // v_cvt_i32_f64_e32 v8, v[4:5]
    assert!(translate(Stage::Compute, &[0x7e10_0704]).unwrap().contains("tb_f64_to_i32(uint2(v4, v5), true)"));
    // v_cvt_f64_u32_e32 v[8:9], v4
    assert!(translate(Stage::Compute, &[0x7e10_2d04]).unwrap().contains("tb_i32_to_f64(v4, false)"));
}

#[test]
fn denormal_mode_controls_input_flushing() {
    // s_denorm_mode 0, then v_floor_f64_e32 v[8:9], v[4:5]
    let hlsl = translate(Stage::Compute, &[0xbfa5_0000, 0x7e10_3504]).unwrap();
    assert!(hlsl.contains("(tb_fp_mode & 0x40u) != 0u"), "{hlsl}");
}

#[test]
fn modifiers_scalar_sources_and_other_stages_are_refused() {
    // v_floor_f64_e64 v[8:9], -v[4:5]
    assert!(translate(Stage::Compute, &[0xd59a_0008, 0x2201_0104]).is_err());
    // v_floor_f64_e32 v[8:9], s[4:5]
    assert!(translate(Stage::Compute, &[0x7e10_3404]).is_err());
    assert!(translate(Stage::Pixel, &[0x7e10_3504]).is_err());
}
