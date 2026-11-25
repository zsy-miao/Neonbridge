use super::*;

fn region(index: u32, offset: u64, span: u64, pitch: u32) -> Region {
    Region {
        index,
        offset,
        span,
        pitch,
    }
}

#[test]
fn one_plane_matches_its_row_pitch() {
    let regions = [region(0, 0, 8_294_400, 7680)];
    assert!(matches!(
        plan(0, 8_294_400, 8_294_400, &regions),
        Some(Plan::Whole)
    ));
}

#[test]
fn mip0_is_not_the_whole_chain() {
    let regions = [region(0, 0, 1000, 256), region(1, 1024, 500, 256)];
    assert!(matches!(plan(0, 1000, 1524, &regions), Some(Plan::One(0))));
}

#[test]
fn packed_chain_uses_driver_offsets() {
    let regions = [region(0, 0, 2048, 256), region(1, 2048, 1024, 256)];
    assert!(matches!(plan(0, 3072, 3072, &regions), Some(Plan::Whole)));
}

#[test]
fn second_subresource_can_be_addressed_alone() {
    let regions = [region(0, 0, 2048, 256), region(1, 2048, 1024, 256)];
    assert!(matches!(
        plan(2048, 1024, 3072, &regions),
        Some(Plan::One(1))
    ));
}

#[test]
fn height_padded_single_subresource_uploads_its_footprint() {
    let regions = [region(0, 0, 0x1000, 1024)];
    assert!(matches!(plan(0, 0x2000, 0x1000, &regions), Some(Plan::One(0))));
    assert!(plan(0, 0x2001, 0x1000, &regions).is_none());
}

#[test]
fn oversized_guest_buffer_is_rejected() {
    let regions = [region(0, 0, 1000, 256)];
    assert!(plan(0, 50_000, 1000, &regions).is_none());
}
