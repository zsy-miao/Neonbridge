use super::*;
use crate::container::tests::container_with;

// Legacy input and output signatures share a six-word parameter format.
fn shader(tag: &[u8; 4], rows: &[(&str, u32, u32, u32, u32, u8)]) -> Vec<u8> {
    let mut words = vec![rows.len() as u32, 8];
    let mut names = Vec::new();
    for &(name, index, system, comp_type, register, mask) in rows {
        let offset = 8 + rows.len() * 24 + names.len();
        words.extend([
            offset as u32,
            index,
            system,
            comp_type,
            register,
            u32::from(mask),
        ]);
        names.extend_from_slice(name.as_bytes());
        names.push(0);
    }
    let mut body: Vec<_> = words.into_iter().flat_map(u32::to_le_bytes).collect();
    body.extend_from_slice(&names);
    container_with(&[(tag, &body)])
}

#[test]
fn orders_by_register_then_first_component() {
    let rows = [
        ("Alpha", 4, 0, 3, 4, 0b1000),
        ("Tex", 1, 0, 3, 2, 0b0011),
        ("Fog", 0, 0, 3, 4, 0b0001),
    ];
    assert_eq!(
        pixel_input_order(&shader(b"ISGN", &rows)),
        ["Tex1", "Fog0", "Alpha4"]
    );
    let sorted = raster_outputs(&shader(b"OSGN", &rows)).unwrap();
    assert_eq!(
        sorted
            .iter()
            .map(|row| row.name.as_str())
            .collect::<Vec<_>>(),
        ["Tex", "Fog", "Alpha"]
    );
}

#[test]
fn compares_shared_semantics_and_allows_a_subset_of_lanes() {
    let producer = shader(
        b"OSGN",
        &[
            ("Tex", 0, 0, 3, 3, 0b1110),
            ("SV_POSITION", 0, 1, 3, 1, 0b1111),
        ],
    );
    let inputs = [
        ("SV_IsFrontFace", 0, 9, 1, 0, 1),
        ("tEX", 0, 0, 3, 3, 0b1010),
        ("Tex", 1, 0, 3, 7, 1),
    ];
    assert!(!has_packing_mismatch(&producer, &shader(b"ISGN", &inputs)));
    for (register, mask) in [(3, 0b0101), (4, 0b0110)] {
        let input = shader(b"ISGN", &[("TEX", 0, 0, 3, register, mask)]);
        assert!(has_packing_mismatch(&producer, &input));
    }
    assert!(!has_packing_mismatch(&producer, &[]));
    assert!(!has_packing_mismatch(&[], &shader(b"ISGN", &inputs)));
}

#[test]
fn a_component_type_change_is_a_mismatch_except_for_system_values() {
    // Console interpolants are raw dwords: a title can write float and read uint.
    let producer = shader(
        b"OSGN",
        &[("TEXCOORD", 3, 0, 3, 8, 0b1111), ("SV_POSITION", 0, 1, 3, 0, 0b1111)],
    );
    let uint = shader(b"ISGN", &[("TEXCOORD", 3, 0, 1, 8, 0b1111)]);
    assert!(has_packing_mismatch(&producer, &uint));
    let float = shader(b"ISGN", &[("TEXCOORD", 3, 0, 3, 8, 0b1111)]);
    assert!(!has_packing_mismatch(&producer, &float));
    let position = shader(b"ISGN", &[("SV_POSITION", 0, 1, 1, 0, 0b1111)]);
    assert!(!has_packing_mismatch(&producer, &position));
}

#[test]
fn requires_scalar_unsigned_stencil_reference_zero_in_x() {
    let valid = shader(b"OSGN", &[("sv_stencilref", 0, 69, 1, u32::MAX, 1)]);
    assert!(exports_stencil_reference(&valid));
    for (sysval, index, comp_type, mask) in [
        (69, 1, 1, 1),
        (69, 0, 3, 1),
        (69, 0, 1, 2),
        (69, 0, 1, 3),
        (64, 0, 1, 1),
    ] {
        let output = shader(
            b"OSGN",
            &[("SV_StencilRef", index, sysval, comp_type, u32::MAX, mask)],
        );
        assert!(!exports_stencil_reference(&output));
    }
}
