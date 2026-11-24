//! Translate only a small synthetic compute program supplied as native words.
use tuxbox_xshader::{ShaderExtract, Stage, hlsl};
fn main() {
    let words: Vec<u32> = std::env::args().skip(1)
        .map(|s| u32::from_str_radix(&s, 16).expect("hex native word")).collect();
    assert!(!words.is_empty() && words.len() <= 256 && words.last() == Some(&0xbf810000));
    let isa: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let ex = ShaderExtract { flat_inputs: Vec::new(), flat_outputs: Vec::new(),
        stage: Stage::Compute, shader_model: (6, 0), required_features: None,
        bitcode_stripped: true, fspx_config: None, ps09_pixel_inputs: None,
        fssm: Vec::new(), entry: Some("main".into()), isa_data: isa.clone(),
        isa, isa_offset_in_fssm: 0, psv_threads: None, isa_preamble: [0, 1, 1, 1],
        has_input_sig: false, has_output_sig: false, has_psv0: false, has_root_sig: false,
        input_sig: Vec::new(), output_sig: Vec::new(), resources: Vec::new(),
        gs_info: None, mesh_info: None, primitive_sig: Vec::new(), root_sig: None,
    };
    match hlsl::translate(&ex) {
        Ok(result) => print!("{}", result.hlsl),
        Err(reason) => { eprintln!("{reason}"); std::process::exit(65); }
    }
}
