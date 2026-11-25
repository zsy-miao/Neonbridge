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

/// PSV0 signature elements of `code` with their interpolation modes:
/// (inputs, outputs), each (semantic name, semantic index, kind, interpolation).
fn psv_interp(code: &[u8]) -> Option<(Vec<(String, u32, u8, u8)>, Vec<(String, u32, u8, u8)>)> {
    let dxbc = crate::Dxbc::parse(code).ok()?;
    let c = dxbc.chunk(b"PSV0")?;
    let psv = code.get(c.offset..c.offset + c.size)?;
    let word = |at: usize| psv.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let info = word(0)? as usize;
    if info < 36 {
        return None;
    }
    let (inputs, outputs) = (*psv.get(4 + 28)? as usize, *psv.get(4 + 29)? as usize);
    let mut at = 4 + info;
    let resources = word(at)? as usize;
    at += 4;
    if resources != 0 {
        at += 4 + resources * word(at)? as usize;
    }
    let strings_len = word(at)? as usize;
    let strings = psv.get(at + 4..at + 4 + strings_len)?;
    at += 4 + strings_len;
    let index_count = word(at)? as usize;
    let indexes = at + 4;
    at += 4 + 4 * index_count;
    let stride = word(at)? as usize;
    at += 4;
    if stride < 14 {
        return None;
    }
    let mut read = |n: usize| -> Option<Vec<(String, u32, u8, u8)>> {
        let mut out = Vec::new();
        for _ in 0..n {
            let e = psv.get(at..at + stride)?;
            let name_at = u32::from_le_bytes([e[0], e[1], e[2], e[3]]) as usize;
            let index_at = u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as usize;
            let tail = strings.get(name_at..)?;
            let name = String::from_utf8_lossy(&tail[..tail.iter().position(|&b| b == 0)?]).into_owned();
            let index = word(indexes + 4 * index_at).unwrap_or(0);
            out.push((name, index, e[11], e[13]));
            at += stride;
        }
        Some(out)
    };
    let ins = read(inputs)?;
    let outs = read(outputs)?;
    Some((ins, outs))
}

/// TUXBOX_LINK_INTERP (default on, =0 registers and masks only): whether a
/// semantic both stages declare is interpolated differently, a producer
/// output linear where the pixel shader declares it nointerpolation to read
/// its vertices (test185: 29 of 31 refused PS+MS pairs, MS TEXCOORD linear,
/// PS constant). The host refuses such a pair (E_INVALIDARG); the producer
/// linked to the pixel shader takes its modes.
pub fn has_interp_mismatch(producer: &[u8], pixel: &[u8]) -> bool {
    if std::env::var("TUXBOX_LINK_INTERP").is_ok_and(|v| v.trim() == "0") {
        return false;
    }
    let (Some((_, outputs)), Some((inputs, _))) = (psv_interp(producer), psv_interp(pixel)) else {
        return false;
    };
    inputs.iter().filter(|i| i.2 == 0).any(|input| {
        outputs.iter().any(|output| {
            output.2 == 0 && output.1 == input.1 && output.0.eq_ignore_ascii_case(&input.0) && output.3 != input.3
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
