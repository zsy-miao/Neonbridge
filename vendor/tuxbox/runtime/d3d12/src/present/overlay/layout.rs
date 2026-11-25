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
#[path = "../../tests/present/overlay/layout.rs"]
mod tests;
