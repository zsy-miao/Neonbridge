//! PC display options: window mode, output size, frame-rate cap, vsync and monitor, all from
//! environment variables (XCL keeps no state of its own; the launcher passes them).
//!
//! * `XCL_WINDOW_MODE`  `windowed` | `borderless` | `fullscreen` (exclusive, via DXGI)
//! * `XCL_WINDOW_SIZE`  `WxH`: the window's client size (windowed) or the display mode
//!   (fullscreen). The title keeps rendering at its own resolution; the presenter scales
//!   the image into this size, letterboxed.
//! * `XCL_FPS_CAP`      `30` | `60` | `0`: a cap on the title's frame clock; `0` adds none
//!   (the title's own pacing stands)
//! * `XCL_VSYNC`        `1` (present interval 1) | `0` (interval 0, tearing when supported)
//! * `XCL_MONITOR`      index into the monitors (`EnumDisplayMonitors` order)

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WindowMode {
    Windowed,
    Borderless,
    Fullscreen,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Options {
    pub mode: Option<WindowMode>,
    pub size: Option<(u32, u32)>,
    /// `Some(0)`: no cap of XCL's
    pub fps_cap: Option<u32>,
    /// `None`: as before (interval 0, no tearing flag)
    pub vsync: Option<bool>,
    pub monitor: Option<usize>,
}

pub fn parse_mode(s: &str) -> Option<WindowMode> {
    match s.trim().to_ascii_lowercase().as_str() {
        "windowed" | "window" => Some(WindowMode::Windowed),
        "borderless" | "borderless-windowed" => Some(WindowMode::Borderless),
        "fullscreen" | "exclusive" => Some(WindowMode::Fullscreen),
        _ => None,
    }
}

/// `1920x1080` (also `X`, `*`, `,`, spaces around); each side 320..=16384.
pub fn parse_size(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().to_ascii_lowercase();
    let (w, h) = s.split_once(['x', '*', ','])?;
    let (w, h) = (w.trim().parse::<u32>().ok()?, h.trim().parse::<u32>().ok()?);
    ((320..=16384).contains(&w) && (200..=16384).contains(&h)).then_some((w, h))
}

/// the frame rates a console frame interval exists for, and 0 / `unlimited` / `off`: no cap
pub fn parse_fps_cap(s: &str) -> Option<u32> {
    let s = s.trim().to_ascii_lowercase();
    if matches!(s.as_str(), "unlimited" | "uncapped" | "off" | "none") {
        return Some(0);
    }
    let n: u32 = s.parse().ok()?;
    matches!(n, 0 | 15 | 20 | 30 | 40 | 60 | 120).then_some(n)
}

pub fn parse_vsync(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "on" | "true" | "yes" => Some(true),
        "0" | "off" | "false" | "no" => Some(false),
        _ => None,
    }
}

pub fn parse_monitor(s: &str) -> Option<usize> {
    s.trim().parse().ok()
}

/// the options from a variable lookup (the environment in `options`)
pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Options {
    fn pick<T>(get: &impl Fn(&str) -> Option<String>, name: &str, parse: fn(&str) -> Option<T>) -> Option<T> {
        let v = get(name)?;
        if v.trim().is_empty() {
            return None;
        }
        let r = parse(&v);
        if r.is_none() {
            crate::xlog_warn!("display: {name}={v:?} not understood - ignored");
        }
        r
    }
    Options {
        mode: pick(&get, "XCL_WINDOW_MODE", parse_mode),
        size: pick(&get, "XCL_WINDOW_SIZE", parse_size),
        fps_cap: pick(&get, "XCL_FPS_CAP", parse_fps_cap),
        vsync: pick(&get, "XCL_VSYNC", parse_vsync),
        monitor: pick(&get, "XCL_MONITOR", parse_monitor),
    }
}

pub fn options() -> &'static Options {
    static O: OnceLock<Options> = OnceLock::new();
    O.get_or_init(|| from_lookup(|n| std::env::var(n).ok()))
}

/// The largest `src` aspect-correct rectangle centred in `dst`: (x, y, w, h).
pub fn letterbox(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> (u32, u32, u32, u32) {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return (0, 0, dst_w, dst_h);
    }
    let (sw, sh, dw, dh) = (src_w as u64, src_h as u64, dst_w as u64, dst_h as u64);
    let (w, h) = if dw * sh <= dh * sw {
        (dw, ((sh * dw + sw / 2) / sw).clamp(1, dh))
    } else {
        (((sw * dh + sh / 2) / sh).clamp(1, dw), dh)
    };
    (((dw - w) / 2) as u32, ((dh - h) / 2) as u32, w as u32, h as u32)
}

/// The frame interval (us) for a title's request under a cap: the longer of the two. A cap
/// of 0 (or one with no console interval) leaves the title's.
pub fn capped_interval(title_us: u32, cap: u32) -> u32 {
    let cap_us = match cap {
        15 => 66_667,
        20 => 50_000,
        30 => 33_333,
        40 => 25_000,
        60 => 16_667,
        120 => 8_333,
        _ => 0,
    };
    if title_us == 0 {
        0
    } else {
        title_us.max(cap_us)
    }
}

/// Window mode switch by Alt+Enter: windowed <-> borderless (fullscreen leaves for windowed).
pub fn toggled(m: WindowMode) -> WindowMode {
    match m {
        WindowMode::Windowed => WindowMode::Borderless,
        _ => WindowMode::Windowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_option() {
        assert_eq!(parse_mode(" Borderless "), Some(WindowMode::Borderless));
        assert_eq!(parse_mode("fullscreen"), Some(WindowMode::Fullscreen));
        assert_eq!(parse_mode("tall"), None);
        assert_eq!(parse_size("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_size(" 2560 X 1440 "), Some((2560, 1440)));
        assert_eq!(parse_size("1920"), None);
        assert_eq!(parse_size("10x10"), None);
        assert_eq!(parse_fps_cap("30"), Some(30));
        assert_eq!(parse_fps_cap("0"), Some(0));
        assert_eq!(parse_fps_cap("Unlimited"), Some(0));
        assert_eq!(parse_fps_cap("45"), None);
        assert_eq!(parse_vsync("1"), Some(true));
        assert_eq!(parse_vsync("0"), Some(false));
        assert_eq!(parse_vsync("x"), None);
        assert_eq!(parse_monitor("2"), Some(2));
        assert_eq!(parse_monitor("-1"), None);
    }

    #[test]
    fn options_from_a_lookup() {
        let env = |n: &str| match n {
            "XCL_WINDOW_MODE" => Some("fullscreen".to_string()),
            "XCL_WINDOW_SIZE" => Some("1920x1080".to_string()),
            "XCL_FPS_CAP" => Some("60".to_string()),
            "XCL_VSYNC" => Some("junk".to_string()),
            "XCL_MONITOR" => Some("".to_string()),
            _ => None,
        };
        let o = from_lookup(env);
        assert_eq!(o, Options { mode: Some(WindowMode::Fullscreen), size: Some((1920, 1080)), fps_cap: Some(60), vsync: None, monitor: None });
        assert_eq!(from_lookup(|_| None), Options::default());
    }

    #[test]
    fn letterbox_keeps_the_aspect() {
        assert_eq!(letterbox(3840, 2160, 1920, 1080), (0, 0, 1920, 1080));
        // 16:9 into 16:10: bars above and below
        assert_eq!(letterbox(1920, 1080, 1920, 1200), (0, 60, 1920, 1080));
        // 16:9 into 4:3: 1024x576 centred
        assert_eq!(letterbox(1920, 1080, 1024, 768), (0, 96, 1024, 576));
        // 16:9 into ultrawide: bars left and right
        assert_eq!(letterbox(1920, 1080, 3440, 1440), (440, 0, 2560, 1440));
        // 4:3 source in 16:9
        assert_eq!(letterbox(1440, 1080, 1920, 1080), (240, 0, 1440, 1080));
        assert_eq!(letterbox(0, 0, 100, 100), (0, 0, 100, 100));
        for (sw, sh, dw, dh) in [(1920, 1080, 1281, 721), (2560, 1440, 999, 1001), (1280, 720, 7680, 4320)] {
            let (x, y, w, h) = letterbox(sw, sh, dw, dh);
            assert!(x + w <= dw && y + h <= dh && (w == dw || h == dh), "{sw}x{sh} -> {dw}x{dh}: {x},{y} {w}x{h}");
        }
    }

    #[test]
    fn a_cap_lengthens_the_interval_never_shortens_it() {
        assert_eq!(capped_interval(16_667, 30), 33_333);
        assert_eq!(capped_interval(8_333, 60), 16_667);
        assert_eq!(capped_interval(33_333, 60), 33_333);
        assert_eq!(capped_interval(16_667, 0), 16_667);
        assert_eq!(capped_interval(0, 30), 0);
    }

    #[test]
    fn alt_enter_toggles_windowed_and_borderless() {
        assert_eq!(toggled(WindowMode::Windowed), WindowMode::Borderless);
        assert_eq!(toggled(WindowMode::Borderless), WindowMode::Windowed);
        assert_eq!(toggled(WindowMode::Fullscreen), WindowMode::Windowed);
    }
}
