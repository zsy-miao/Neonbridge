//! Display settings: the config the Settings page edits, the environment the runtime reads
//! (`XCL_WINDOW_MODE`, `XCL_WINDOW_SIZE`, `XCL_FPS_CAP`, `XCL_VSYNC`, `XCL_MONITOR`), and the
//! monitors and their modes (EnumDisplayMonitors / EnumDisplaySettingsW).

use serde::{Deserialize, Serialize};

pub const MODES: [&str; 3] = ["windowed", "borderless", "fullscreen"];
pub const MODE_LABELS: [&str; 3] = ["Windowed", "Borderless", "Fullscreen"];

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(default)]
pub struct DisplayCfg {
    /// "windowed" | "borderless" | "fullscreen"
    pub mode: String,
    /// "WxH", or empty for the game's own default (windowed) / the monitor's size (fullscreen)
    pub resolution: String,
    /// 30, 60 or 0 (unlimited)
    pub fps_cap: u32,
    pub vsync: bool,
    /// index of the monitor, in EnumDisplayMonitors order
    pub monitor: u32,
}

impl Default for DisplayCfg {
    fn default() -> Self {
        DisplayCfg { mode: "windowed".into(), resolution: String::new(), fps_cap: 0, vsync: true, monitor: 0 }
    }
}

pub fn parse_res(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.trim().to_ascii_lowercase().split_once('x').map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The environment variables the runtime reads. `monitors`: how many monitors there are (a
/// stored index past them is left out).
pub fn env(d: &DisplayCfg, monitors: usize) -> Vec<(&'static str, String)> {
    let mode = if MODES.contains(&d.mode.as_str()) { d.mode.as_str() } else { "windowed" };
    let mut v = vec![("XCL_WINDOW_MODE", mode.to_string())];
    if let Some((w, h)) = parse_res(&d.resolution) {
        v.push(("XCL_WINDOW_SIZE", format!("{w}x{h}")));
    }
    v.push(("XCL_FPS_CAP", if matches!(d.fps_cap, 30 | 60) { d.fps_cap } else { 0 }.to_string()));
    v.push(("XCL_VSYNC", if d.vsync { "1" } else { "0" }.to_string()));
    if (d.monitor as usize) < monitors {
        v.push(("XCL_MONITOR", d.monitor.to_string()));
    }
    v
}

/// The resolutions to offer from a monitor's raw modes (width, height): at least 800x600,
/// each once, largest first.
pub fn unique_modes(raw: impl IntoIterator<Item = (u32, u32)>) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = raw.into_iter().filter(|&(w, h)| w >= 800 && h >= 600).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.dedup();
    v
}

#[derive(Clone, Debug)]
pub struct Monitor {
    pub primary: bool,
    pub size: (u32, u32),
    pub modes: Vec<(u32, u32)>,
}

impl Monitor {
    pub fn label(&self, i: usize) -> String {
        format!("{}: {} x {}{}", i + 1, self.size.0, self.size.1, if self.primary { " (primary)" } else { "" })
    }
}

#[cfg(windows)]
pub fn monitors() -> Vec<Monitor> {
    use windows_sys::Win32::Foundation::{LPARAM, RECT};
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, HDC, HMONITOR, MONITORINFOEXW};

    unsafe extern "system" fn each(mon: HMONITOR, _dc: HDC, _rc: *mut RECT, data: LPARAM) -> i32 {
        let out = &mut *(data as *mut Vec<Monitor>);
        let mut mi: MONITORINFOEXW = std::mem::zeroed();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(mon, &mut mi as *mut MONITORINFOEXW as *mut _) == 0 {
            return 1;
        }
        let r = mi.monitorInfo.rcMonitor;
        let mut raw = Vec::new();
        let mut i = 0;
        loop {
            let mut dm: DEVMODEW = std::mem::zeroed();
            dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsW(mi.szDevice.as_ptr(), i, &mut dm) == 0 {
                break;
            }
            raw.push((dm.dmPelsWidth, dm.dmPelsHeight));
            i += 1;
        }
        out.push(Monitor { primary: mi.monitorInfo.dwFlags & 1 != 0, size: ((r.right - r.left) as u32, (r.bottom - r.top) as u32), modes: unique_modes(raw) });
        1
    }
    let mut v: Vec<Monitor> = Vec::new();
    unsafe {
        EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(each), &mut v as *mut Vec<Monitor> as LPARAM);
    }
    v
}

#[cfg(not(windows))]
pub fn monitors() -> Vec<Monitor> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_for_the_default_config() {
        let e = env(&DisplayCfg::default(), 2);
        assert_eq!(e, vec![("XCL_WINDOW_MODE", "windowed".into()), ("XCL_FPS_CAP", "0".into()), ("XCL_VSYNC", "1".into()), ("XCL_MONITOR", "0".into())]);
    }

    #[test]
    fn env_for_a_chosen_setup() {
        let d = DisplayCfg { mode: "fullscreen".into(), resolution: "2560 x 1440".into(), fps_cap: 60, vsync: false, monitor: 1 };
        let e = env(&d, 2);
        assert_eq!(
            e,
            vec![("XCL_WINDOW_MODE", "fullscreen".into()), ("XCL_WINDOW_SIZE", "2560x1440".into()), ("XCL_FPS_CAP", "60".into()), ("XCL_VSYNC", "0".into()), ("XCL_MONITOR", "1".into())]
        );
        // a monitor that is gone, a mode nobody knows, an odd cap
        let d = DisplayCfg { mode: "tall".into(), resolution: "auto".into(), fps_cap: 45, vsync: true, monitor: 5 };
        assert_eq!(env(&d, 1), vec![("XCL_WINDOW_MODE", "windowed".into()), ("XCL_FPS_CAP", "0".into()), ("XCL_VSYNC", "1".into())]);
    }

    #[test]
    fn modes_are_unique_and_largest_first() {
        let m = unique_modes([(640, 480), (1920, 1080), (1280, 720), (1920, 1080), (2560, 1440), (800, 600)]);
        assert_eq!(m, vec![(2560, 1440), (1920, 1080), (1280, 720), (800, 600)]);
    }

    #[test]
    fn resolutions_parse_and_label() {
        assert_eq!(parse_res("1920x1080"), Some((1920, 1080)));
        assert_eq!(parse_res(" 1920 x 1080 "), Some((1920, 1080)));
        assert_eq!(parse_res(""), None);
    }

    #[test]
    fn an_old_config_loads_with_display_defaults() {
        let d: DisplayCfg = serde_json::from_str("{\"fps_cap\":30}").unwrap();
        assert_eq!(d, DisplayCfg { fps_cap: 30, ..DisplayCfg::default() });
    }

    #[test]
    fn this_machine_lists_a_monitor() {
        #[cfg(windows)]
        {
            let m = monitors();
            // headless CI may have none; when there are some, each has modes
            for mon in &m {
                assert!(mon.size.0 > 0);
            }
        }
    }
}
