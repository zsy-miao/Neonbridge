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

/// Tests whether a semantic present at both stages moves registers, needs
/// components the producer did not allocate, or changes component type.
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
                && (input.register != output.register
                    || input.mask & !output.mask != 0
                    || (input.sysval == 0 && input.comp_type != output.comp_type))
        })
    })
}

#[cfg(test)]
#[path = "tests/raster.rs"]
mod tests;
