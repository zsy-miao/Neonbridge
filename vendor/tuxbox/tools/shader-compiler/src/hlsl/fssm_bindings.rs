//! Native structured-buffer strides in the FSSM 1.1 resource binding table.
use crate::{meta::PsvResource, ShaderExtract, Stage};

pub(super) fn structured_words(ex: &ShaderExtract, slot: u32, uav: bool) -> Result<u32, String> {
    if ex.stage != Stage::Compute {
        return Err("FSSM compute binding table requires a compute stage".into());
    }
    stride_words(&ex.fssm, &ex.resources, ex.stage, slot, uav).ok_or_else(|| {
        format!(
            "invalid FSSM structured-buffer stride for {}{slot}",
            if uav { 'u' } else { 't' }
        )
    })
}

pub(super) fn stride_words(
    fssm: &[u8],
    resources: &[PsvResource],
    stage: Stage,
    slot: u32,
    uav: bool,
) -> Option<u32> {
    let (psv_type, class, descriptor) = match (stage, uav) {
        (Stage::Compute, false) => (5, 0x300, 0x404),
        (Stage::Compute, true) => (8, 0x303, 0x30704),
        (Stage::Pixel, false) => (5, 0x300, 0x804),
        (Stage::Vertex, false) => (5, 0x300, 0x30b04),
        (Stage::Mesh, false) => (5, 0x300, 0x91104),
        _ => return None,
    };
    // Raw buffers and images carry a size here as well. The independent PSV
    // binding determines whether this field describes a structured element.
    if !resources
        .iter()
        .any(|p| (p.res_type, p.space, p.lower, p.upper) == (psv_type, 0, slot, slot))
        || fssm.get(..8)? != b"FSSM\x01\x01\0\0"
    {
        return None;
    }
    let body = fssm.get(16..)?;
    let declared = u32::from_le_bytes(*fssm.get(8..)?.first_chunk()?);
    if usize::try_from(declared).ok()? != body.len() {
        return None;
    }
    // Offsets are relative to the end of the sixteen-byte file header.
    let start = u64::from_le_bytes(*fssm.get(0xf0..)?.first_chunk()?);
    let count = u32::from_le_bytes(*fssm.get(0xfc..)?.first_chunk()?);
    let bytes = usize::try_from(count).ok()?.checked_mul(20)?;
    let start = usize::try_from(start).ok()?;
    if start < 0xf0 || start % 4 != 0 {
        return None;
    }
    let table = body.get(start..)?.get(..bytes)?;
    let mut found = None;
    let (records, _) = table.as_chunks::<20>();
    for record in records {
        let (words, _) = record.as_chunks::<4>();
        if u32::from_le_bytes(words[0]) != class || u32::from_le_bytes(words[2]) != slot {
            continue;
        }
        let stride = u32::from_le_bytes(words[3]);
        // In this ABI buffer descriptor slots are eight dwords apart. The
        // fourth word holds element bytes, including padding after the last field.
        if found.is_some()
            || u32::from_le_bytes(words[1]) != slot.checked_mul(8)?
            || u32::from_le_bytes(words[4]) != descriptor
            || !(4..=2048).contains(&stride)
            || stride % 4 != 0
        {
            return None;
        }
        found = Some(stride / 4);
    }
    found
}

#[cfg(test)]
#[path = "../tests/hlsl/fssm_bindings.rs"]
mod tests;
