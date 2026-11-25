use super::{early_return_distance, is_recoverable};

const SCARLETT_TILE_HEAP: [u8; 9] = [0xcc, 0x48, 0x8b, 0x1f, 0xe9, 0xf0, 0xfd, 0xff, 0xff];

#[test]
fn run_walk_end_iterator_assert_is_recoverable_on_exact_bytes() {
    let site = [0xcc, 0xe9, 0xa5, 0xfd, 0xff, 0xff, 0xcc, 0x48, 0x8b];
    assert!(is_recoverable(0x4ffd859, &site));
    assert!(!is_recoverable(0x4ffd85a, &site));
    let mut other = site;
    other[2] = 0xa6;
    assert!(!is_recoverable(0x4ffd859, &other));
}

#[test]
fn run_table_walk_assert_returns_early_only_on_exact_bytes() {
    let site = [0xcc, 0xeb, 0xc5, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc, 0xcc];
    assert_eq!(early_return_distance(0x51d251e, &site), Some(0x10));
    assert_eq!(early_return_distance(0x51d251f, &site), None);
    let mut other = site;
    other[2] = 0xc6;
    assert_eq!(early_return_distance(0x51d251e, &other), None);
}

#[test]
fn only_the_scarlett_tile_heap_assert_is_recoverable() {
    assert!(is_recoverable(0x4ffd879, &SCARLETT_TILE_HEAP));
    let mut other = SCARLETT_TILE_HEAP;
    other[4] = 0xeb;
    assert!(!is_recoverable(0x4ffd879, &other));
    assert!(!is_recoverable(0x4ffd868, &SCARLETT_TILE_HEAP));
}
