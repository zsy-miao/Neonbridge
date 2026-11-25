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
mod tests {
    use super::*;

    fn fixture(rows: &[[u32; 5]]) -> Vec<u8> {
        let start = 0x160;
        let mut data = vec![0u8; start];
        data[..8].copy_from_slice(b"FSSM\x01\x01\0\0");
        data[0xf0..0xf8].copy_from_slice(&((start - 16) as u64).to_le_bytes());
        data[0xfc..0x100].copy_from_slice(&(rows.len() as u32).to_le_bytes());
        for row in rows {
            data.extend(row.iter().flat_map(|v| v.to_le_bytes()));
        }
        let size = (data.len() - 16) as u32;
        data[8..12].copy_from_slice(&size.to_le_bytes());
        data
    }

    #[test]
    fn reads_reflected_stride_for_structured_bindings_only() {
        let rows = [
            [0x300, 24, 3, 800, 0x404],
            [0x303, 8, 1, 44, 0x30704],
            [0x300, 0, 0, 1, 0x404],
        ];
        let data = fixture(&rows);
        let bindings = [(5, 3), (8, 1), (4, 0)].map(|(res_type, slot)| PsvResource {
            res_type,
            space: 0,
            lower: slot,
            upper: slot,
        });
        assert_eq!(
            stride_words(&data, &bindings, Stage::Compute, 3, false),
            Some(200)
        );
        assert_eq!(
            stride_words(&data, &bindings, Stage::Compute, 1, true),
            Some(11)
        );
        assert_eq!(
            stride_words(&data, &bindings, Stage::Compute, 0, false),
            None
        );
        assert_eq!(
            stride_words(&data, &bindings, Stage::Compute, 1, false),
            None
        );

        let binding = &bindings[..1];
        for row in [
            [0x300, 8, 3, 800, 0x404],
            [0x300, 24, 3, 799, 0x404],
            [0x300, 24, 3, 800, 0x408],
        ] {
            assert_eq!(
                stride_words(&fixture(&[row]), binding, Stage::Compute, 3, false),
                None
            );
        }
        assert_eq!(
            stride_words(&fixture(&[rows[0]; 2]), binding, Stage::Compute, 3, false),
            None
        );
        assert_eq!(
            stride_words(&data[..data.len() - 1], binding, Stage::Compute, 3, false),
            None
        );
        let mut out_of_range = data;
        out_of_range[0xf0..0xf8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(
            stride_words(&out_of_range, binding, Stage::Compute, 3, false),
            None
        );
    }

    #[test]
    fn pixel_srv_stride_includes_unread_fields() {
        // These MainPS inputs only fetch the first word or vector of each record.
        let data = fixture(&[[0x300, 0, 0, 16, 0x804], [0x300, 16, 2, 32, 0x804]]);
        let resources = [0, 2].map(|slot| PsvResource {
            res_type: 5,
            space: 0,
            lower: slot,
            upper: slot,
        });
        assert_eq!(
            stride_words(&data, &resources, Stage::Pixel, 0, false),
            Some(4)
        );
        assert_eq!(
            stride_words(&data, &resources, Stage::Pixel, 2, false),
            Some(8)
        );
        assert_eq!(
            stride_words(&data, &resources, Stage::Compute, 0, false),
            None
        );
        assert_eq!(stride_words(&data, &resources, Stage::Pixel, 0, true), None);
    }

    #[test]
    fn vertex_and_mesh_views_keep_the_complete_record() {
        let resource = [PsvResource { res_type: 5, space: 0, lower: 3, upper: 3 }];
        for (stage, tag, other) in [
            (Stage::Vertex, 0x30b04, Stage::Mesh),
            (Stage::Mesh, 0x91104, Stage::Vertex),
        ] {
            let table = fixture(&[[0x300, 24, 3, 800, tag]]);
            assert_eq!(stride_words(&table, &resource, stage, 3, false), Some(200));
            assert_eq!(stride_words(&table, &resource, other, 3, false), None);
        }
    }
}
