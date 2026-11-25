use tuxbox_xshader::psv::{portable_pixel_interp, portable_pixel_interface};

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn fixture(mode: u8, register: u8, mask: u8, semantic_index: u32) -> Vec<u8> {
    // Minimal structural DXIL marker; this fixture checks metadata only,
    // not the validity of the dummy LLVM bitcode.
    let mut dxil = words(&[0x60, 7, u32::from_le_bytes(*b"DXIL"), 0x100, 16, 4]);
    dxil.extend(b"BC\xc0\xde");
    let mut isgn = words(&[1, 8, 32, 0, 0, 3, 1, u32::from(mask)]);
    isgn.extend(b"TEXCOORD\0");
    let mut psv = words(&[36]);
    let mut runtime = [0u8; 36];
    runtime[28] = 1;
    runtime[31] = 2;
    psv.extend(runtime);
    psv.extend(words(&[0, 12]));
    psv.extend(b"\0TEXCOORD\0\0\0");
    psv.extend(words(&[1, semantic_index, 16, 1, 0]));
    psv.extend([1, register, 0x41, 0, 3, mode, 0, 0]);
    let parts = [(b"DXIL", dxil), (b"ISGN", isgn), (b"PSV0", psv)];
    let mut result = vec![0; 44];
    result[..4].copy_from_slice(b"DXBC");
    result[20..24].copy_from_slice(&1u32.to_le_bytes());
    result[28..32].copy_from_slice(&3u32.to_le_bytes());
    for (i, (tag, body)) in parts.iter().enumerate() {
        let offset = result.len() as u32;
        result[32+i*4..36+i*4].copy_from_slice(&offset.to_le_bytes());
        result.extend(*tag);
        result.extend((body.len() as u32).to_le_bytes());
        result.extend(body);
        while result.len() % 4 != 0 { result.push(0); }
    }
    let size = result.len() as u32;
    result[24..28].copy_from_slice(&size.to_le_bytes());
    result
}

#[test]
fn metadata_must_match_signature_and_interpolation() {
    for (mode, name) in [(1,"nointerpolation"),(2,"linear"),(3,"centroid"),(4,"noperspective"),(5,"noperspective centroid"),(6,"sample"),(7,"noperspective sample")] {
        assert_eq!(portable_pixel_interp(&fixture(mode,1,1,0)), Some(vec![("TEXCOORD0".to_string(), name.to_string())]));
    }
    for bad in [fixture(0,1,1,0), fixture(9,1,1,0), fixture(2,2,1,0), fixture(2,1,2,0), fixture(2,1,1,1)] {
        assert!(portable_pixel_interp(&bad).is_none());
    }
    let valid = fixture(2,1,1,0);
    let interface = portable_pixel_interface(&valid).unwrap();
    assert_eq!(interface.inputs.len(), 1);
    assert_eq!(interface.inputs[0].comp_type, 3);
    assert_eq!(interface.inputs[0].name, "TEXCOORD");
    // Legacy signature component type at record word 3. The validated
    // interface must retain the integer distinction, not just qualifiers.
    for kind in 1u32..=3 {
        let mut bytes = fixture(1,1,1,0);
        let container = tuxbox_xshader::Dxbc::parse(&bytes).unwrap();
        let offset = container.chunk(b"ISGN").unwrap().offset + 8 + 12;
        bytes[offset..offset+4].copy_from_slice(&kind.to_le_bytes());
        assert_eq!(portable_pixel_interface(&bytes).unwrap().inputs[0].comp_type, kind);
    }
    for n in 0..valid.len() { assert!(portable_pixel_interp(&valid[..n]).is_none()); }
}

#[test]
fn dxc_generated_portable_consumer_if_supplied() {
    let Some(path) = std::env::var_os("TUXBOX_TEST_PORTABLE_PIXEL") else { return; };
    let bytes = std::fs::read(path).unwrap();
    let interface = portable_pixel_interface(&bytes).expect("DXC-generated pixel metadata");
    let interp = interface.interpolation;
    assert_eq!(interface.inputs.iter().filter(|e|e.sysval==0).map(|e|e.comp_type).collect::<Vec<_>>(), vec![1,3,3]);
    assert_eq!(interp, vec![("TEXCOORD0".into(),"nointerpolation".into()),("TEXCOORD1".into(),"centroid".into()),("TEXCOORD2".into(),"noperspective".into())]);
}
