//! Raw MUBUF IDXEN is a byte offset.
//!
//! Captured batch-cull loads scale the index to bytes, then add a field
//! displacement with IDXEN and a one-byte descriptor stride. Typed format
//! loads keep their own path and do not use this address.

use super::Emitter;
use crate::rdna2::disassemble;
use crate::{ShaderExtract, Stage};

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

fn address(words: [u32; 2]) -> String {
    let ex = compute_extract(&words);
    let insns = disassemble(&ex.isa);
    let mut em = Emitter::new(&ex, insns.clone());
    if insns[0].aux.mask & 2 != 0 {
        assert!(em.raw_address(&insns[0]).is_err());
        // This address-only fixture does not construct a descriptor.
        // The full descriptor proof is covered separately by raw_width.
        em.raw_byte_index.insert(insns[0].offset);
    }
    em.raw_address(&insns[0]).unwrap();
    em.body
}

#[test]
fn indexed_raw_load_is_a_byte_address() {
    // buffer_load_dword, IDXEN, vaddr v2, soffset 0, offset 4.
    let body = address([0xe030_2004, 0x8002_0402]);
    assert!(
        body.contains("uint raw_addr0 = v2 + 0x0u + 4u;"),
        "{body}"
    );
}

#[test]
fn indexed_and_offset_raw_load_uses_consecutive_vgprs() {
    // IDXEN|OFFEN. v2 is the index and v3 is the byte offset.
    let body = address([0xe030_3008, 0x8002_0402]);
    assert!(
        body.contains("uint raw_addr0 = (v2 + v3) + 0x0u + 8u;"),
        "{body}"
    );
}

#[test]
fn offset_only_raw_load_keeps_the_byte_formula() {
    // OFFEN only, the form already lowered before IDXEN was accepted.
    let body = address([0xe030_1000, 0x8002_0402]);
    assert!(
        body.contains("uint raw_addr0 = v2 + 0x0u + 0u;"),
        "{body}"
    );
}
