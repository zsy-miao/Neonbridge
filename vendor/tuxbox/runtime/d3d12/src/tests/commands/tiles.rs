use super::{mapping_flags, ranges, regions};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;

#[test]
fn mapping_flags_allow_only_no_hazard() {
    assert!(mapping_flags(D3D12_TILE_MAPPING_FLAG_NONE).is_ok());
    assert!(mapping_flags(D3D12_TILE_MAPPING_FLAG_NO_HAZARD).is_ok());
    assert_eq!(mapping_flags(D3D12_TILE_MAPPING_FLAGS(0x2)).unwrap_err(), E_NOTIMPL);
}

#[test]
fn one_region_may_omit_its_box() {
    assert!(regions(1, core::ptr::null(), core::ptr::null()).is_ok());
    assert_eq!(regions(0, core::ptr::null(), core::ptr::null()).unwrap_err(), E_INVALIDARG);
    assert_eq!(regions(2, core::ptr::null(), core::ptr::null()).unwrap_err(), E_INVALIDARG);
}

#[test]
fn null_heap_requires_null_ranges() {
    let null_flag = [D3D12_TILE_RANGE_FLAG_NULL];
    let none_flag = [D3D12_TILE_RANGE_FLAG_NONE];
    let offset = [0u32];
    assert!(ranges(1, null_flag.as_ptr(), core::ptr::null(), false).is_ok());
    assert_eq!(
        ranges(1, none_flag.as_ptr(), offset.as_ptr(), false).unwrap_err(),
        E_INVALIDARG
    );
    assert_eq!(ranges(1, core::ptr::null(), core::ptr::null(), false).unwrap_err(), E_INVALIDARG);
    assert!(ranges(1, core::ptr::null(), offset.as_ptr(), true).is_ok());
    let unknown = [D3D12_TILE_RANGE_FLAGS(0x8)];
    assert_eq!(ranges(1, unknown.as_ptr(), offset.as_ptr(), true).unwrap_err(), E_NOTIMPL);
}
