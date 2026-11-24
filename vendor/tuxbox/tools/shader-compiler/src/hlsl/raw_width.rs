// Ported from upstream TuxBox e1d7f5a.
//! Raw buffer transfers stay one dword per HLSL `Load`/`Store`.
//!
//! The MUBUF address is 32-bit (`vaddr + soffset + offset`), so a title can produce an offset
//! that wraps, such as 0xFFFFFFFC. Under vkd3d-proton on NVIDIA, a multi-dword raw load
//! (`Load2`..`Load4`) is lowered through `VK_NV_raw_access_chains`; one 16-byte load at
//! 0xFFFFFFFC lost the device (Xid 109) on an RTX 5080 with Proton Experimental 11.0-20261112.
//! Four 4-byte loads at the same offsets completed and returned zero for the out-of-range word.
//! Merging the per-dword transfers would reintroduce that fault, so this test pins the form.
use super::Emitter;
use crate::rdna2::disassemble;
use crate::{ShaderExtract, Stage};

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

#[test]
fn raw_buffer_transfers_are_emitted_one_dword_at_a_time() {
    // vaddr v2 (OFFEN), vdata v[4:7], srsrc s[8:11], soffset 0.
    let cases = [
        ([0xe038_1000, 0x8002_0402], "buffer_load_dwordx4", "Load"),
        ([0xe078_1000, 0x8002_0402], "buffer_store_dwordx4", "Store"),
    ];
    for (words, mnemonic, op) in cases {
        let ex = compute_extract(&words);
        let insns = disassemble(&ex.isa);
        assert_eq!(insns[0].mnemonic, mnemonic);
        let mut em = Emitter::new(&ex, insns.clone());
        em.raw_transfer_on(&insns[0], "raw_ubuf0".into(), true).unwrap();
        assert_eq!(em.body.matches(&format!("raw_ubuf0.{op}(")).count(), 4, "{mnemonic}:\n{}", em.body);
        for wide in ["2(", "3(", "4("] {
            assert!(!em.body.contains(&format!("{op}{wide}")), "{mnemonic} merged:\n{}", em.body);
        }
    }
}
