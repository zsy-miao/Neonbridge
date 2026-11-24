// Synthetic compute DS_SWIZZLE_B32, shared with the translator regression.
use super::{ShaderExtract, Stage};

/// ds_swizzle_b32 v0, v1 offset:<offset>; s_waitcnt lgkmcnt(0); s_endpgm
/// in one 64-lane group. As in LLVM's encoding, the source occupies the ADDR field.
pub fn shader(offset: u32, word0_extra: u32) -> ShaderExtract {
    let words = [0xd8d4_0000 | offset | word0_extra, 0x0000_0001, 0xbf8c_c07f, 0xbf81_0000];
    let isa: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
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
