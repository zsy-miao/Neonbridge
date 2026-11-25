use anyxboxx_texture::{bc7, decode_surface};

#[test]
fn surface_rows_preserve_crops_and_block_order() {
    for (width, height) in [(0usize, 5usize), (5, 0), (1, 1), (3, 5), (4, 4), (7, 11), (16, 9)] {
        let columns = width.div_ceil(4);
        let rows = height.div_ceil(4);
        let blocks: Vec<u8> = (0..columns * rows)
            .flat_map(|i| {
                let mut bytes = [i as u8 + 1; bc7::BLOCK_BYTES];
                bytes[0] = 1 << (i % 8);
                bytes
            })
            .collect();
        let expected: Vec<_> = (0..height)
            .flat_map(|y| (0..width).map(move |x| (y, x)))
            .flat_map(|(y, x)| {
                let at = (y / 4 * columns + x / 4) * bc7::BLOCK_BYTES;
                let block = bc7::decode(&blocks[at..at + bc7::BLOCK_BYTES]).unwrap();
                let pixel = (y % 4 * 4 + x % 4) * 4;
                <[u8; 4]>::try_from(&block[pixel..pixel + 4]).unwrap()
            })
            .collect();
        assert_eq!(decode_surface(&blocks, width, height).unwrap(), expected, "{width}×{height}");
    }
    assert!(decode_surface(&[0; 15], 4, 4).is_err());
    assert!(decode_surface(&[0; 1], 0, 4).is_err());
}
