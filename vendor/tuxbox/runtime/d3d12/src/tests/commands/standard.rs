use super::*;
use windows::core::BOOL;

#[test]
fn tile_flags_accept_one_direction_and_reject_unknown_bits() {
    assert!(tile_copy_flags(D3D12_TILE_COPY_FLAG_NONE).is_ok());
    assert!(tile_copy_flags(
        D3D12_TILE_COPY_FLAG_NO_HAZARD | D3D12_TILE_COPY_FLAG_LINEAR_BUFFER_TO_SWIZZLED_TILED_RESOURCE
    ).is_ok());
    assert_eq!(
        tile_copy_flags(D3D12_TILE_COPY_FLAGS(0x8)).unwrap_err(),
        E_NOTIMPL
    );
    assert_eq!(
        tile_copy_flags(
            D3D12_TILE_COPY_FLAG_LINEAR_BUFFER_TO_SWIZZLED_TILED_RESOURCE
                | D3D12_TILE_COPY_FLAG_SWIZZLED_TILED_RESOURCE_TO_LINEAR_BUFFER
        ).unwrap_err(),
        E_INVALIDARG
    );
}

#[test]
fn tile_region_uses_the_box_or_the_tile_count() {
    let box_region = D3D12_TILE_REGION_SIZE {
        NumTiles: 0,
        UseBox: BOOL::from(true),
        Width: 2,
        Height: 3,
        Depth: 4,
    };
    assert_eq!(tile_count(&box_region).unwrap(), 24);
    let counted = D3D12_TILE_REGION_SIZE { NumTiles: 5, UseBox: BOOL::from(false), ..box_region };
    assert_eq!(tile_count(&counted).unwrap(), 5);
    let empty = D3D12_TILE_REGION_SIZE { NumTiles: 0, UseBox: BOOL::from(false), ..box_region };
    assert_eq!(tile_count(&empty).unwrap_err(), E_INVALIDARG);
    let flat = D3D12_TILE_REGION_SIZE { Width: 0, ..box_region };
    assert_eq!(tile_count(&flat).unwrap_err(), E_INVALIDARG);
}

#[test]
fn predication_accepts_only_the_two_desktop_operations() {
    assert!(predication_op(D3D12_PREDICATION_OP_EQUAL_ZERO).is_ok());
    assert!(predication_op(D3D12_PREDICATION_OP_NOT_EQUAL_ZERO).is_ok());
    assert_eq!(predication_op(D3D12_PREDICATION_OP(2)).unwrap_err(), E_INVALIDARG);
}

#[test]
fn stream_output_stays_inside_four_slots() {
    assert!(stream_output_range(0, 4).is_ok());
    assert!(stream_output_range(4, 0).is_ok());
    assert_eq!(stream_output_range(3, 2).unwrap_err(), E_INVALIDARG);
    assert_eq!(stream_output_range(5, 0).unwrap_err(), E_INVALIDARG);
}

#[test]
fn marker_rejects_a_sized_null_payload() {
    assert!(event_data(core::ptr::null(), 0).is_ok());
    assert!(event_data(core::ptr::from_ref(&1u8).cast(), 1).is_ok());
    assert_eq!(event_data(core::ptr::null(), 4).unwrap_err(), E_INVALIDARG);
}

#[test]
fn resolve_accepts_unknown_and_shared_formats_only() {
    assert!(resolve_format(DXGI_FORMAT_UNKNOWN));
    assert!(resolve_format(DXGI_FORMAT_B4G4R4A4_UNORM));
    assert!(!resolve_format(DXGI_FORMAT(DXGI_FORMAT_B4G4R4A4_UNORM.0 + 1)));
}
