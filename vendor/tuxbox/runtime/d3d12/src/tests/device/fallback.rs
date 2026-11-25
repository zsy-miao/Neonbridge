#[test]
fn counts_each_kind_separately() {
    let mut counts = Vec::new();
    assert_eq!(super::bump(&mut counts, "compute"), 1);
    assert_eq!(super::bump(&mut counts, "compute"), 2);
    assert_eq!(super::bump(&mut counts, "raster stream"), 1);
    assert_eq!(super::bump(&mut counts, "compute"), 3);
}
