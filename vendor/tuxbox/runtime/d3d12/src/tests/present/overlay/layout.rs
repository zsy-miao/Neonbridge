use super::*;

#[test]
fn scale_follows_the_vertical_resolution() {
    assert_eq!(scale_for(720), 1);
    assert_eq!(scale_for(1080), 1);
    assert_eq!(scale_for(1440), 1);
    assert_eq!(scale_for(2160), 2);
    assert_eq!(scale_for(4320), 3);
    assert_eq!(scale_for(100_000), 3);
}

#[test]
fn rows_start_on_256_byte_boundaries() {
    assert_eq!(row_pitch(380), 1536);
    assert_eq!(row_pitch(64), 256);
    assert_eq!(row_pitch(65), 512);
    assert_eq!(row_pitch(760) % 256, 0);
}

#[test]
fn panel_sits_in_the_requested_corner() {
    let panel = panel_size(1);
    let screen = (1920, 1080);
    assert_eq!(origin(Corner::TopLeft, panel, screen, 1), Some((16, 16)));
    assert_eq!(origin(Corner::TopRight, panel, screen, 1), Some((1920 - 380 - 16, 16)));
    assert_eq!(origin(Corner::BottomLeft, panel, screen, 1), Some((16, 1080 - BASE_HEIGHT - 16)));
    assert_eq!(origin(Corner::BottomRight, panel, screen, 1), Some((1920 - 380 - 16, 1080 - BASE_HEIGHT - 16)));
}

#[test]
fn panel_that_does_not_fit_is_skipped() {
    assert_eq!(origin(Corner::TopRight, panel_size(1), (400, 1080), 1), None);
    assert_eq!(origin(Corner::TopRight, panel_size(1), (1920, 100), 1), None);
    assert!(origin(Corner::TopRight, panel_size(1), (380 + 32, BASE_HEIGHT + 32), 1).is_some());
}

#[test]
fn corner_names_parse() {
    assert_eq!(Corner::parse(None), Corner::TopRight);
    assert_eq!(Corner::parse(Some("tl")), Corner::TopLeft);
    assert_eq!(Corner::parse(Some("bl")), Corner::BottomLeft);
    assert_eq!(Corner::parse(Some("br")), Corner::BottomRight);
    assert_eq!(Corner::parse(Some("whatever")), Corner::TopRight);
}

#[test]
fn pixels_pack_into_both_swap_chain_formats() {
    assert_eq!(pack(PixelFormat::Rgba8, [0x11, 0x22, 0x33]), 0xff33_2211);
    let white = pack(PixelFormat::Rgb10A2, [255, 255, 255]);
    assert_eq!(white, 0x3ff | 0x3ff << 10 | 0x3ff << 20 | 3 << 30);
    assert_eq!(pack(PixelFormat::Rgb10A2, [0, 0, 0]), 3 << 30);
}

#[test]
fn marching_block_enters_crosses_and_leaves() {
    let (track, period) = (100, 24);
    assert_eq!(marching_block(track, 0, period), (0, 0));
    let (start, end) = marching_block(track, 12, period);
    assert!(start < end && end <= track);
    for phase in 0..period {
        let (s, e) = marching_block(track, phase, period);
        assert!(s <= e && e <= track, "phase {phase}: {s}..{e}");
    }
    assert_eq!(marching_block(track, 23, period).1, track);
}
