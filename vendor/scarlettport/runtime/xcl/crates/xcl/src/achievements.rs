//! Achievement / event recorder.
//!
//! GDK titles report progress either through `XGameEventWrite` or through Xbox Live's
//! achievements REST service (`POST https://achievements.xboxlive.com/users/xuid(..)/achievements/<scid>`
//! with `{"achievements":[{"id":"7","percentComplete":100}]}`, made over libHttpClient after an
//! `XUserGetTokenAndSignatureAsync` that carries the request body). XCL records both to
//! `<title data folder>\achievements.json` (name, progress, first / last time) and shows a toast
//! for 4 seconds when an achievement reaches 100 (`XCL_TOAST=0` turns the toast off).
//! Achievement names live on the Xbox Live service, not in the package: an achievement is
//! recorded by the id the title sends.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    /// "achievement" or "event"
    pub kind: String,
    /// highest percentComplete seen (0 for an event)
    pub progress: u32,
    /// how many writes named it
    pub count: u64,
    /// unix seconds
    pub first: u64,
    pub last: u64,
    #[serde(default)]
    pub unlocked: bool,
}

#[derive(Serialize, Deserialize, Default, Debug)]
struct File {
    entries: Vec<Entry>,
}

pub struct Recorder {
    path: PathBuf,
    entries: BTreeMap<String, Entry>,
}

impl Recorder {
    pub fn load(path: &Path) -> Recorder {
        let file: File = std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let entries = file.entries.into_iter().map(|e| (format!("{}:{}", e.kind, e.name), e)).collect();
        Recorder { path: path.to_path_buf(), entries }
    }

    pub fn save(&self) -> std::io::Result<()> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let file = File { entries: self.entries.values().cloned().collect() };
        std::fs::write(&self.path, serde_json::to_vec_pretty(&file).map_err(std::io::Error::other)?)
    }

    pub fn get(&self, kind: &str, name: &str) -> Option<&Entry> {
        self.entries.get(&format!("{kind}:{name}"))
    }

    fn touch(&mut self, kind: &str, name: &str, now: u64) -> &mut Entry {
        let e = self.entries.entry(format!("{kind}:{name}")).or_insert_with(|| Entry { name: name.into(), kind: kind.into(), progress: 0, count: 0, first: now, last: now, unlocked: false });
        e.count += 1;
        e.last = now;
        e
    }

    /// An `XGameEventWrite`.
    pub fn record_event(&mut self, name: &str, now: u64) {
        self.touch("event", name, now);
    }

    /// An achievement update; true when this update unlocked it (reached 100 for the first time).
    pub fn record_progress(&mut self, id: &str, percent: u32, now: u64) -> bool {
        let e = self.touch("achievement", id, now);
        e.progress = e.progress.max(percent.min(100));
        if e.progress >= 100 && !e.unlocked {
            e.unlocked = true;
            return true;
        }
        false
    }
}

/// The updates in an achievements REST request: (id, percentComplete). Nothing for a read.
pub fn parse_update(method: &str, url: &str, body: &[u8]) -> Vec<(String, u32)> {
    let lower = url.to_ascii_lowercase();
    if method.eq_ignore_ascii_case("GET") || !lower.contains("achievements.xboxlive.com/") {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(body) else { return Vec::new() };
    let percent = |a: &serde_json::Value| -> Option<u32> {
        if let Some(p) = a.get("percentComplete").and_then(|p| p.as_f64()) {
            return Some(p.clamp(0.0, 100.0) as u32);
        }
        match a.get("progressState").and_then(|s| s.as_str()) {
            Some("Achieved") => Some(100),
            _ => None,
        }
    };
    let id_of = |a: &serde_json::Value| a.get("id").map(|i| i.as_str().map(String::from).unwrap_or_else(|| i.to_string()));
    let mut out = Vec::new();
    if let Some(list) = v.get("achievements").and_then(|a| a.as_array()) {
        for a in list {
            if let (Some(id), Some(p)) = (id_of(a), percent(a)) {
                out.push((id, p));
            }
        }
    } else if let Some(p) = percent(&v) {
        // .../achievements/<scid>/<id> with the percentage alone
        let path = lower.split(['?', '#']).next().unwrap_or("");
        if let Some(id) = path.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty() && !s.starts_with("achievements")) {
            out.push((id.to_string(), p));
        }
    }
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn global() -> &'static Mutex<Recorder> {
    static R: OnceLock<Mutex<Recorder>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Recorder::load(&crate::state::data_dir().join("achievements.json"))))
}

/// The title wrote an event.
pub fn note_event(name: &str) {
    if let Ok(mut r) = global().lock() {
        r.record_event(name, now_secs());
        let _ = r.save();
    }
}

/// The title updated an achievement; logs and toasts an unlock.
pub fn note_progress(id: &str, percent: u32) {
    let unlocked = match global().lock() {
        Ok(mut r) => {
            let u = r.record_progress(id, percent, now_secs());
            let _ = r.save();
            u
        }
        Err(_) => false,
    };
    if unlocked {
        crate::xlog_info!("XCL: achievement unlocked: {id}");
        toast(&format!("Achievement unlocked: {id}"));
    }
}

/// Records the achievement updates of a request the title makes (seen at its token request).
pub fn note_request(method: &str, url: &str, body: &[u8]) {
    for (id, pct) in parse_update(method, url, body) {
        note_progress(&id, pct);
    }
}

/// A small topmost window in the screen's lower right corner for 4 seconds; it takes no focus.
pub fn toast(text: &str) {
    if cfg!(test) || std::env::var("XCL_TOAST").as_deref() == Ok("0") {
        return;
    }
    let text = text.to_string();
    let _ = std::thread::Builder::new().name("xcl-toast".into()).spawn(move || unsafe { win::show(&text) });
}

mod win {
    use std::ffi::c_void;

    use crate::util::wide;

    type Hwnd = *mut c_void;

    #[repr(C)]
    struct WndClassExW {
        cb_size: u32,
        style: u32,
        wnd_proc: unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize,
        cls_extra: i32,
        wnd_extra: i32,
        instance: usize,
        icon: usize,
        cursor: usize,
        background: usize,
        menu_name: *const u16,
        class_name: *const u16,
        icon_small: usize,
    }

    #[repr(C)]
    struct Msg {
        hwnd: Hwnd,
        message: u32,
        wparam: usize,
        lparam: isize,
        time: u32,
        pt: [i32; 2],
    }

    #[link(name = "user32")]
    extern "system" {
        #[link_name = "RegisterClassExW"]
        fn register_class(c: *const WndClassExW) -> u16;
        #[allow(clippy::too_many_arguments)]
        #[link_name = "CreateWindowExW"]
        fn create_window(ex: u32, class: *const u16, title: *const u16, style: u32, x: i32, y: i32, w: i32, h: i32, parent: Hwnd, menu: usize, inst: usize, param: *const c_void) -> Hwnd;
        fn DefWindowProcW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn GetMessageW(m: *mut Msg, h: Hwnd, min: u32, max: u32) -> i32;
        fn TranslateMessage(m: *const Msg) -> i32;
        fn DispatchMessageW(m: *const Msg) -> isize;
        fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn DestroyWindow(h: Hwnd) -> i32;
        fn PostQuitMessage(code: i32);
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn SetTimer(h: Hwnd, id: usize, ms: u32, f: usize) -> usize;
        fn GetSystemMetrics(i: i32) -> i32;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn GetStockObject(i: i32) -> usize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> usize;
    }

    const WM_DESTROY: u32 = 0x2;
    const WM_SETFONT: u32 = 0x30;
    const WM_TIMER: u32 = 0x113;
    const WS_POPUP: u32 = 0x8000_0000;
    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_BORDER: u32 = 0x80_0000;
    const WS_EX_TOPMOST: u32 = 0x8;
    const WS_EX_TOOLWINDOW: u32 = 0x80;
    const WS_EX_NOACTIVATE: u32 = 0x800_0000;
    const SS_CENTER: u32 = 1;
    const SS_CENTERIMAGE: u32 = 0x200;
    const SW_SHOWNOACTIVATE: i32 = 4;
    /// how long the toast stays
    const TOAST_MS: u32 = 4000;

    unsafe extern "system" fn proc_(h: Hwnd, m: u32, w: usize, l: isize) -> isize {
        match m {
            WM_TIMER => {
                DestroyWindow(h);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }

    pub unsafe fn show(text: &str) {
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("XclToast");
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: proc_,
            cls_extra: 0,
            wnd_extra: 0,
            instance: inst,
            icon: 0,
            cursor: 0,
            background: 6, // COLOR_WINDOW + 1
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            icon_small: 0,
        };
        register_class(&wc);
        let (w, h) = (360, 64);
        let x = GetSystemMetrics(0) - w - 24;
        let y = GetSystemMetrics(1) - h - 72;
        let win =  create_window(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class.as_ptr(), wide("XCL").as_ptr(), WS_POPUP | WS_BORDER, x, y, w, h, std::ptr::null_mut(), 0, inst, std::ptr::null());
        if win.is_null() {
            return;
        }
        let label =  create_window(0, wide("STATIC").as_ptr(), wide(text).as_ptr(), WS_CHILD | WS_VISIBLE | SS_CENTER | SS_CENTERIMAGE, 4, 4, w - 8, h - 8, win, 0, inst, std::ptr::null());
        SendMessageW(label, WM_SETFONT, GetStockObject(17), 1);
        ShowWindow(win, SW_SHOWNOACTIVATE);
        SetTimer(win, 1, TOAST_MS, 0);
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xcl_ach_test_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("achievements.json")
    }

    #[test]
    fn progress_unlocks_once_at_100() {
        let path = tmp("unlock");
        let mut r = Recorder::load(&path);
        assert!(!r.record_progress("7", 40, 100));
        assert!(!r.record_progress("7", 20, 110), "progress never goes down");
        assert_eq!(r.get("achievement", "7").unwrap().progress, 40);
        assert!(r.record_progress("7", 100, 120), "reaching 100 unlocks");
        assert!(!r.record_progress("7", 100, 130), "only once");
        let e = r.get("achievement", "7").unwrap();
        assert_eq!((e.first, e.last, e.count, e.unlocked), (100, 130, 4, true));
        assert!(r.record_progress("8", 100, 140), "an update with 100 unlocks at once");
    }

    #[test]
    fn events_are_counted_apart_from_achievements() {
        let path = tmp("events");
        let mut r = Recorder::load(&path);
        r.record_event("MissionPassed", 5);
        r.record_event("MissionPassed", 9);
        r.record_progress("MissionPassed", 10, 11);
        let ev = r.get("event", "MissionPassed").unwrap();
        assert_eq!((ev.count, ev.first, ev.last, ev.progress), (2, 5, 9, 0));
        assert_eq!(r.get("achievement", "MissionPassed").unwrap().progress, 10);
    }

    #[test]
    fn recorder_persists() {
        let path = tmp("persist");
        let mut r = Recorder::load(&path);
        r.record_event("E", 1);
        r.record_progress("3", 100, 2);
        r.save().unwrap();
        let mut again = Recorder::load(&path);
        assert_eq!(again.get("event", "E").unwrap().count, 1);
        assert!(again.get("achievement", "3").unwrap().unlocked);
        assert!(!again.record_progress("3", 100, 9), "an unlock survives a restart");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn parses_xbox_live_update_requests() {
        let url = "https://achievements.xboxlive.com/users/xuid(2533274790395905)/achievements/abc-scid";
        let body = br#"{"action":"progressUpdate","achievements":[{"id":"12","percentComplete":100},{"id":"13","percentComplete":35}]}"#;
        assert_eq!(parse_update("POST", url, body), vec![("12".into(), 100), ("13".into(), 35)]);
        // reads and other hosts are not updates
        assert!(parse_update("GET", url, body).is_empty());
        assert!(parse_update("POST", "https://other.example.com/x", body).is_empty());
        assert!(parse_update("POST", url, b"not json").is_empty());
        // percentage alone, id in the path
        let put = "https://achievements.xboxlive.com/users/xuid(1)/achievements/scid/21";
        assert_eq!(parse_update("PUT", put, br#"{"percentComplete":100}"#), vec![("21".into(), 100)]);
        // numeric ids
        assert_eq!(parse_update("POST", url, br#"{"achievements":[{"id":5,"progressState":"Achieved"}]}"#), vec![("5".into(), 100)]);
    }
}
