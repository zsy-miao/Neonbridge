//! Input registers and binding metadata consumed by the gameplay HLSL emitter.

/// The depth block may run ordinary early tests speculatively. Only an explicit
/// DEPTH_BEFORE_SHADER request allows those tests to suppress pixel UAV writes.
pub(crate) fn pixel_depth_before_shader(fssm: &[u8]) -> bool {
    // Colour-only early-Z, with optional KILL_ENABLE and EXEC_ON_NOOP.
    pixel_depth_control(fssm).is_some_and(|bits| bits & !0x440 == 0x1010)
}

fn pixel_depth_control(fssm: &[u8]) -> Option<u32> {
    if fssm.get(..8)? != b"FSSM\x01\x01\0\0" {
        return None;
    }
    let image = fssm.get(16..)?;
    if u64::from_le_bytes(*image.first_chunk()?) != 6 {
        return None;
    }
    let offset = u64::from_le_bytes(*image.get(0x30..)?.first_chunk()?);
    let descriptor = image.get(usize::try_from(offset).ok()?..)?;
    Some(u32::from_le_bytes(*descriptor.get(0x18c..)?.first_chunk()?))
}

/// The PS configuration in FSSM 1.1 carries SPI_PS_INPUT_ENA at 0x248.
/// RDNA2 packs the front-face and ancillary words after the barycentrics,
/// line stipple and position components, skipping disabled inputs.
#[derive(Clone, Copy)]
pub(crate) enum PixelSystemInput {
    FrontFace = 12,
    Ancillary = 13,
}

pub(crate) fn pixel_system_vgpr(fssm: &[u8], input: PixelSystemInput) -> Option<usize> {
    let enabled = ps_input_enable(fssm)?;
    let bit = input as usize;
    (enabled & (1 << bit) != 0).then(|| packed_pixel_vgpr(enabled, bit))
}

fn ps_input_enable(fssm: &[u8]) -> Option<u32> {
    if fssm.get(..8)? != b"FSSM\x01\x01\0\0" {
        return None;
    }
    let enabled = u32::from_le_bytes(*fssm.get(0x248..)?.first_chunk()?);
    (enabled >> 16 == 0).then_some(enabled)
}

fn packed_pixel_vgpr(enabled: u32, bit: usize) -> usize {
    const WIDTHS: [usize; 13] = [2, 2, 2, 3, 2, 2, 2, 1, 1, 1, 1, 1, 1];
    WIDTHS[..bit].iter().enumerate()
        .filter(|(i, _)| enabled & (1 << i) != 0)
        .map(|(_, width)| width)
        .sum()
}

/// Position components follow the enabled barycentric/stipple registers.
/// A disabled component occupies no VGPR, including when W follows X/Y or Z.
pub(crate) fn position_vgprs(fssm: &[u8], read: u8) -> Option<[Option<usize>; 4]> {
    let enabled = ps_input_enable(fssm)?;
    if read == 0 || (enabled >> 8) & 15 != u32::from(read) {
        return None;
    }
    Some(std::array::from_fn(|i| {
        (read & (1 << i) != 0).then(|| packed_pixel_vgpr(enabled, 8 + i))
    }))
}

#[cfg(test)]
#[path = "tests/meta/pixel_inputs.rs"]
mod pixel_inputs;

pub use tuxbox_dxbc::{has_packing_mismatch, parse_signature, pixel_input_order, SigElement};

pub use crate::psv::{parse_compute_threads, parse_mesh_info, parse_psv0, parse_psv0_gs, GsInfo, MeshInfo, PsvResource};

pub use crate::rootsig::{parse_rts0, RootParamKind, RootParam, RootRange, RootSig};
