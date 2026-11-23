//! Signature queries used for raster-stage linkage and pixel shader outputs.

use crate::SigElement;
use crate::signature::signature;

const OUTPUTS: &[&[u8; 4]] = &[b"OSG1", b"OSG5", b"OSGN"];
const INPUTS: &[&[u8; 4]] = &[b"ISG1", b"ISGN"];

fn register_order(code: &[u8], tags: &[&[u8; 4]]) -> Option<Vec<SigElement>> {
    let mut elements = signature(code, tags)?;
    elements.sort_by_key(|element| (element.register, element.mask.trailing_zeros()));
    Some(elements)
}

/// Returns the producer's output parameters in register and component order.
pub fn raster_outputs(code: &[u8]) -> Option<Vec<SigElement>> {
    register_order(code, OUTPUTS)
}

/// Returns indexed pixel input names in register and component order.
pub fn pixel_input_order(code: &[u8]) -> Vec<String> {
    register_order(code, INPUTS)
        .unwrap_or_default()
        .into_iter()
        .map(|element| format!("{}{}", element.name, element.index))
        .collect()
}

/// Tests for a scalar unsigned integer in the x lane of SV_StencilRef0.
/// See the [HLSL stencil reference semantic](https://learn.microsoft.com/en-us/windows/win32/direct3dhlsl/sv-stencilref).
pub fn exports_stencil_reference(code: &[u8]) -> bool {
    const UINT32: u32 = 1;
    const STENCIL_REF: u32 = 69;
    signature(code, OUTPUTS).is_some_and(|elements| {
        elements.iter().any(|element| {
            element.sysval == STENCIL_REF
                && element.index == 0
                && element.comp_type == UINT32
                && element.mask == 1
        })
    })
}

/// Tests whether a semantic present at both stages moves registers or needs
/// components the producer did not allocate.
pub fn has_packing_mismatch(producer: &[u8], pixel: &[u8]) -> bool {
    let Some(outputs) = signature(producer, OUTPUTS) else {
        return false;
    };
    let Some(inputs) = signature(pixel, INPUTS) else {
        return false;
    };
    inputs.iter().any(|input| {
        outputs.iter().any(|output| {
            input.index == output.index
                && input.name.eq_ignore_ascii_case(&output.name)
                && (input.register != output.register || input.mask & !output.mask != 0)
        })
    })
}

#[cfg(test)]
mod tests {
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
}
