use super::*;

#[test]
fn bump_alignment_and_reuse() {
    let key = 0x5151_0000;
    let a = allocate(key, 40, 8).unwrap();
    let b = allocate(key, 56, 16).unwrap();
    assert_eq!(b % 16, 0);
    assert!(b >= a + 40);
    let big = allocate(key, 3 << 20, 16).unwrap();
    assert_eq!(big % 16, 0);
    release(key);
    let again = allocate(key, 40, 8).unwrap();
    // A reset hands pages back, so a later arena reuses committed memory.
    let pool = POOL.lock().unwrap();
    assert!(pool.arenas[&key].chunks.iter().any(|c| c.base == again));
    drop(pool);
    assert!(allocate(key, 8, 3).is_none());
    assert!(allocate(key, 0, 8).is_none());
    release(key);
}
