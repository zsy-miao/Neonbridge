//! Geometry, pixel packing and bar animation for the progress panel. Pure
//! functions with no platform calls.

pub(super) const BASE_WIDTH: u32 = 380;
pub(super) const BASE_HEIGHT: u32 = 110;
const MARGIN: u32 = 16;

/// Whole-number scale so the panel reads the same at 1080p and 4K.
pub(super) fn scale_for(height: u32) -> u32 {
    (height / 1080).clamp(1, 3)
}

pub(super) fn panel_size(scale: u32) -> (u32, u32) {
    (BASE_WIDTH * scale, BASE_HEIGHT * scale)
}

/// Buffer-to-texture copies need each row to start on a 256-byte boundary.
pub(super) fn row_pitch(width: u32) -> u32 {
    (width * 4).next_multiple_of(256)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Corner {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Corner {
    /// `TUXBOX_PROGRESS_OVERLAY_POS`: `tl`, `tr`, `bl` or `br`. The top right
    /// is the default, away from a MangoHud panel in the top left.
    pub(super) fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("tl" | "top-left") => Self::TopLeft,
            Some("bl" | "bottom-left") => Self::BottomLeft,
            Some("br" | "bottom-right") => Self::BottomRight,
            _ => Self::TopRight,
        }
    }
}

/// Top-left pixel of the panel, or `None` if it does not fit the screen.
pub(super) fn origin(corner: Corner, panel: (u32, u32), screen: (u32, u32), scale: u32) -> Option<(u32, u32)> {
    let margin = MARGIN * scale;
    let (width, height) = panel;
    let free_x = screen.0.checked_sub(width + 2 * margin)?;
    let free_y = screen.1.checked_sub(height + 2 * margin)?;
    let x = match corner {
        Corner::TopLeft | Corner::BottomLeft => margin,
        Corner::TopRight | Corner::BottomRight => margin + free_x,
    };
    let y = match corner {
        Corner::TopLeft | Corner::TopRight => margin,
        Corner::BottomLeft | Corner::BottomRight => margin + free_y,
    };
    Some((x, y))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PixelFormat {
    Rgba8,
    Rgb10A2,
}

/// Pack an 8-bit colour into one pixel of the swap chain's format, opaque.
pub(super) fn pack(format: PixelFormat, [r, g, b]: [u8; 3]) -> u32 {
    match format {
        PixelFormat::Rgba8 => u32::from(r) | u32::from(g) << 8 | u32::from(b) << 16 | 0xff << 24,
        PixelFormat::Rgb10A2 => {
            let wide = |c: u8| (u32::from(c) << 2) | (u32::from(c) >> 6);
            wide(r) | wide(g) << 10 | wide(b) << 20 | 3 << 30
        }
    }
}

/// The part of a bar of `track` pixels that a marching block covers at step
/// `phase` of `period`. The block enters on the left and leaves on the right.
pub(super) fn marching_block(track: u32, phase: u32, period: u32) -> (u32, u32) {
    let block = (track / 4).max(1);
    let travel = track + block;
    let head = (u64::from(phase % period.max(1)) * u64::from(travel) / u64::from(period.max(1))) as u32;
    let end = head.min(track);
    let start = head.saturating_sub(block).min(track);
    (start, end)
}

#[cfg(test)]
mod tests {
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
}
