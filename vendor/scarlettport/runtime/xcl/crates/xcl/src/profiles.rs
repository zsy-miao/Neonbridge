//! Local XCL profiles: the "who is playing" of XUser.
//!
//! `%LOCALAPPDATA%\XCL\profiles.json` (or `%XCL_DATA_ROOT%`) lists profiles (gamertag, an xuid
//! generated once, an optional picture file). `XUserAddAsync` picks one: `XCL_PROFILE=<gamertag>`
//! skips the picker (the profile is created if new); otherwise a small native window lists the
//! profiles with a text box for a new one. A title that asks for the default user silently
//! (`AddDefaultUserSilently`) gets the last profile used without any window; the very first run
//! and the other add options show the picker (`XCL_PROFILE_PICKER=1` shows it every time,
//! `XCL_PROFILE_PICKER=0` never). The chosen profile answers XUserGetGamertag / GetId / the gamer
//! picture and keys the offline token.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// the longest classic gamertag
pub const MAX_GAMERTAG: usize = 15;
/// XUserAddOptions::AddDefaultUserSilently
const ADD_DEFAULT_SILENTLY: u32 = 1;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Profile {
    pub gamertag: String,
    pub xuid: u64,
    /// a PNG / JPEG file shown as the gamer picture
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub picture: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Debug, Clone, PartialEq, Eq)]
pub struct Store {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// gamertag of the profile used last
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<String>,
}

/// A usable gamertag: trimmed, 1..=15 characters, no control characters.
pub fn clean_gamertag(s: &str) -> Option<String> {
    let t = s.trim();
    let n = t.chars().count();
    if n == 0 || n > MAX_GAMERTAG || t.chars().any(|c| c.is_control()) {
        None
    } else {
        Some(t.to_string())
    }
}

impl Store {
    pub fn load(path: &Path) -> Store {
        std::fs::read(path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    pub fn find(&self, gamertag: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.gamertag.eq_ignore_ascii_case(gamertag))
    }

    /// The profile for `gamertag`, made (with a fresh xuid) when it is new. `seed` varies the
    /// xuid. None for an unusable gamertag.
    pub fn get_or_create(&mut self, gamertag: &str, seed: u64) -> Option<Profile> {
        let tag = clean_gamertag(gamertag)?;
        if let Some(p) = self.find(&tag) {
            return Some(p.clone());
        }
        let mut xuid = make_xuid(&tag, seed);
        while self.profiles.iter().any(|p| p.xuid == xuid) {
            xuid = make_xuid(&tag, xuid);
        }
        let p = Profile { gamertag: tag, xuid, picture: None };
        self.profiles.push(p.clone());
        Some(p)
    }

    /// Makes `p` the profile used last.
    pub fn set_last(&mut self, p: &Profile) {
        self.last = Some(p.gamertag.clone());
    }

    pub fn last_profile(&self) -> Option<&Profile> {
        self.last.as_deref().and_then(|l| self.find(l))
    }
}

/// An Xbox-looking xuid (0x0009_xxxx_xxxx_xxxx) from the gamertag and a seed.
pub fn make_xuid(gamertag: &str, seed: u64) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ seed;
    for b in gamertag.bytes().chain(seed.to_le_bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    0x0009_0000_0000_0000 | (h & 0x0000_FFFF_FFFF_FFFF)
}

pub fn store_path() -> PathBuf {
    crate::launcher::data_root().join("profiles.json")
}

static ACTIVE: Mutex<Option<Profile>> = Mutex::new(None);

/// Tests: forget the chosen profile so the next XUserAddAsync chooses again.
#[cfg(test)]
pub fn reset_active_for_test() {
    if let Ok(mut a) = ACTIVE.lock() {
        *a = None;
    }
}

/// The chosen profile, if the title has added a user.
pub fn active() -> Option<Profile> {
    ACTIVE.lock().ok().and_then(|a| a.clone())
}

/// (gamertag, xuid) of the user: the chosen profile, else the configuration's.
pub fn identity() -> (String, u64) {
    match active() {
        Some(p) => (p.gamertag, p.xuid),
        None => {
            let c = crate::state::config();
            (c.gamertag.clone(), c.user_id)
        }
    }
}

/// the picture file of the chosen profile
pub fn picture_bytes() -> Option<Vec<u8>> {
    let path = active()?.picture?;
    let b = std::fs::read(path).ok()?;
    (b.len() > 8 && (b.starts_with(&[0x89, b'P', b'N', b'G']) || b.starts_with(&[0xFF, 0xD8]))).then_some(b)
}

fn seed() -> u64 {
    let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    t.as_nanos() as u64 ^ ((std::process::id() as u64) << 32)
}

/// Picks the profile for XUserAddAsync (once per process; later calls reuse it).
pub fn choose_for_add(options: u32) -> Profile {
    if let Some(p) = active() {
        return p;
    }
    let path = store_path();
    let mut store = Store::load(&path);
    if store.profiles.is_empty() {
        // keeps what the title ran with before profiles: the configured gamertag and id
        let c = crate::state::config();
        if let Some(tag) = clean_gamertag(&c.gamertag) {
            store.profiles.push(Profile { gamertag: tag, xuid: c.user_id, picture: None });
        }
    }
    let env_profile = std::env::var("XCL_PROFILE").ok().and_then(|s| clean_gamertag(&s));
    let picker = std::env::var("XCL_PROFILE_PICKER").ok();
    let chosen = if let Some(tag) = env_profile {
        store.get_or_create(&tag, seed())
    } else if cfg!(test) || picker.as_deref() == Some("0") {
        store.last_profile().or(store.profiles.first()).cloned()
    } else if picker.as_deref() != Some("1") && options & ADD_DEFAULT_SILENTLY != 0 && store.last_profile().is_some() {
        store.last_profile().cloned()
    } else {
        let names: Vec<String> = store.profiles.iter().map(|p| p.gamertag.clone()).collect();
        let pre = store.last.clone().unwrap_or_default();
        match dialog::pick(&names, &pre) {
            Some((sel, created)) => {
                for n in &created {
                    store.get_or_create(n, seed());
                }
                store.get_or_create(&sel, seed())
            }
            None => store.last_profile().or(store.profiles.first()).cloned(),
        }
    };
    let chosen = chosen.unwrap_or_else(|| {
        let mut s = Store::default();
        s.get_or_create("Player1", 1).unwrap()
    });
    if store.find(&chosen.gamertag).is_none() {
        store.profiles.push(chosen.clone());
    }
    store.set_last(&chosen);
    if let Err(e) = store.save(&path) {
        crate::xlog_info!("profiles: cannot save {}: {e}", path.display());
    }
    crate::xlog_info!("profiles: playing as '{}' (xuid {})", chosen.gamertag, chosen.xuid);
    if let Ok(mut a) = ACTIVE.lock() {
        *a = Some(chosen.clone());
    }
    chosen
}

/// The picker window: a list of gamertags, a text box + "New profile" to add one, OK.
mod dialog {
    use std::cell::RefCell;
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
        fn IsDialogMessageW(h: Hwnd, m: *mut Msg) -> i32;
        fn TranslateMessage(m: *const Msg) -> i32;
        fn DispatchMessageW(m: *const Msg) -> isize;
        fn SendMessageW(h: Hwnd, m: u32, w: usize, l: isize) -> isize;
        fn DestroyWindow(h: Hwnd) -> i32;
        fn PostQuitMessage(code: i32);
        fn ShowWindow(h: Hwnd, cmd: i32) -> i32;
        fn SetForegroundWindow(h: Hwnd) -> i32;
        fn SetFocus(h: Hwnd) -> Hwnd;
        fn GetSystemMetrics(i: i32) -> i32;
        fn GetDlgItem(h: Hwnd, id: i32) -> Hwnd;
        fn GetWindowTextW(h: Hwnd, buf: *mut u16, max: i32) -> i32;
        fn LoadCursorW(inst: usize, name: usize) -> usize;
    }
    #[link(name = "gdi32")]
    extern "system" {
        fn GetStockObject(i: i32) -> usize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetModuleHandleW(name: *const u16) -> usize;
    }

    const WS_CHILD: u32 = 0x4000_0000;
    const WS_VISIBLE: u32 = 0x1000_0000;
    const WS_TABSTOP: u32 = 0x1_0000;
    const WS_VSCROLL: u32 = 0x20_0000;
    const WS_BORDER: u32 = 0x80_0000;
    const WS_CAPTION: u32 = 0xC0_0000;
    const WS_SYSMENU: u32 = 0x8_0000;
    const WS_EX_TOPMOST: u32 = 0x8;
    const WS_EX_CONTROLPARENT: u32 = 0x1_0000;
    const WS_EX_CLIENTEDGE: u32 = 0x200;
    const LBS_NOTIFY: u32 = 1;
    const ES_AUTOHSCROLL: u32 = 0x80;
    const BS_DEFPUSHBUTTON: u32 = 1;
    const WM_DESTROY: u32 = 0x2;
    const WM_CLOSE: u32 = 0x10;
    const WM_SETFONT: u32 = 0x30;
    const WM_COMMAND: u32 = 0x111;
    const EM_LIMITTEXT: u32 = 0xC5;
    const LB_ADDSTRING: u32 = 0x180;
    const LB_SETCURSEL: u32 = 0x186;
    const LB_GETCURSEL: u32 = 0x188;
    const LB_GETTEXT: u32 = 0x189;
    const LB_GETTEXTLEN: u32 = 0x18A;
    const LB_GETCOUNT: u32 = 0x18B;
    const WM_SETTEXT: u32 = 0xC;
    const LBN_DBLCLK: usize = 2;
    const ID_LIST: i32 = 101;
    const ID_EDIT: i32 = 102;
    const ID_NEW: i32 = 103;
    const ID_OK: i32 = 1;
    const DEFAULT_GUI_FONT: i32 = 17;

    struct State {
        created: Vec<String>,
        selected: Option<String>,
    }
    thread_local! {
        static STATE: RefCell<State> = const { RefCell::new(State { created: Vec::new(), selected: None }) };
    }

    unsafe fn child(parent: Hwnd, class: &str, text: &str, style: u32, ex: u32, rect: [i32; 4], id: i32, font: usize) -> Hwnd {
        let h =  create_window(ex, wide(class).as_ptr(), wide(text).as_ptr(), WS_CHILD | WS_VISIBLE | style, rect[0], rect[1], rect[2], rect[3], parent, id as usize, 0, std::ptr::null());
        SendMessageW(h, WM_SETFONT, font, 1);
        h
    }

    unsafe fn text_of(h: Hwnd) -> String {
        let mut b = [0u16; 128];
        let n = GetWindowTextW(h, b.as_mut_ptr(), b.len() as i32);
        String::from_utf16_lossy(&b[..n.max(0) as usize])
    }

    unsafe fn list_names(list: Hwnd) -> Vec<String> {
        let n = SendMessageW(list, LB_GETCOUNT, 0, 0).max(0) as usize;
        (0..n)
            .map(|i| {
                let len = SendMessageW(list, LB_GETTEXTLEN, i, 0).max(0) as usize;
                let mut b = vec![0u16; len + 1];
                SendMessageW(list, LB_GETTEXT, i, b.as_mut_ptr() as isize);
                String::from_utf16_lossy(&b[..len])
            })
            .collect()
    }

    /// Adds the text box's gamertag to the list and selects it; whether there was one.
    unsafe fn add_new(win: Hwnd) -> bool {
        let list = GetDlgItem(win, ID_LIST);
        let edit = GetDlgItem(win, ID_EDIT);
        let Some(tag) = super::clean_gamertag(&text_of(edit)) else { return false };
        let names = list_names(list);
        let idx = match names.iter().position(|n| n.eq_ignore_ascii_case(&tag)) {
            Some(i) => i,
            None => {
                let idx = SendMessageW(list, LB_ADDSTRING, 0, wide(&tag).as_ptr() as isize).max(0) as usize;
                STATE.with(|s| s.borrow_mut().created.push(tag));
                idx
            }
        };
        SendMessageW(list, LB_SETCURSEL, idx, 0);
        SendMessageW(edit, WM_SETTEXT, 0, wide("").as_ptr() as isize);
        true
    }

    unsafe fn finish(win: Hwnd) {
        add_new(win);
        let list = GetDlgItem(win, ID_LIST);
        let names = list_names(list);
        let sel = SendMessageW(list, LB_GETCURSEL, 0, 0);
        let pick = if sel >= 0 { names.get(sel as usize).cloned() } else { names.first().cloned() };
        STATE.with(|s| s.borrow_mut().selected = pick);
        DestroyWindow(win);
    }

    unsafe extern "system" fn proc_(h: Hwnd, m: u32, w: usize, l: isize) -> isize {
        match m {
            WM_COMMAND => {
                let id = (w & 0xFFFF) as i32;
                let note = w >> 16;
                if id == ID_NEW {
                    add_new(h);
                } else if id == ID_OK || (id == ID_LIST && note == LBN_DBLCLK) {
                    finish(h);
                }
                0
            }
            WM_CLOSE => {
                finish(h);
                0
            }
            WM_DESTROY => {
                PostQuitMessage(0);
                0
            }
            _ => DefWindowProcW(h, m, w, l),
        }
    }

    /// Shows the picker (blocks the caller; the window runs on its own thread). Returns the
    /// selected gamertag and the gamertags made in the window.
    pub fn pick(names: &[String], preselect: &str) -> Option<(String, Vec<String>)> {
        let names = names.to_vec();
        let pre = preselect.to_string();
        std::thread::Builder::new().name("xcl-profile-picker".into()).spawn(move || unsafe { run(&names, &pre) }).ok()?.join().ok()?
    }

    unsafe fn run(names: &[String], preselect: &str) -> Option<(String, Vec<String>)> {
        STATE.with(|s| *s.borrow_mut() = State { created: Vec::new(), selected: None });
        let inst = GetModuleHandleW(std::ptr::null());
        let class = wide("XclProfilePicker");
        let wc = WndClassExW {
            cb_size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            wnd_proc: proc_,
            cls_extra: 0,
            wnd_extra: 0,
            instance: inst,
            icon: 0,
            cursor: LoadCursorW(0, 32512),
            background: 16, // COLOR_BTNFACE + 1
            menu_name: std::ptr::null(),
            class_name: class.as_ptr(),
            icon_small: 0,
        };
        register_class(&wc); // already registered on a second picker: fine
        let (w, h) = (340, 330);
        let x = (GetSystemMetrics(0) - w) / 2;
        let y = (GetSystemMetrics(1) - h) / 2;
        let win =  create_window(WS_EX_TOPMOST | WS_EX_CONTROLPARENT, class.as_ptr(), wide("XCL - choose a profile").as_ptr(), WS_CAPTION | WS_SYSMENU | WS_VISIBLE, x, y, w, h, std::ptr::null_mut(), 0, inst, std::ptr::null());
        if win.is_null() {
            return None;
        }
        let font = GetStockObject(DEFAULT_GUI_FONT);
        child(win, "STATIC", "Who is playing?", 0, 0, [12, 10, 300, 18], 0, font);
        let list = child(win, "LISTBOX", "", LBS_NOTIFY | WS_VSCROLL | WS_BORDER | WS_TABSTOP, WS_EX_CLIENTEDGE, [12, 32, 300, 130], ID_LIST, font);
        let mut sel = 0;
        for (i, n) in names.iter().enumerate() {
            SendMessageW(list, LB_ADDSTRING, 0, wide(n).as_ptr() as isize);
            if n.eq_ignore_ascii_case(preselect) {
                sel = i;
            }
        }
        SendMessageW(list, LB_SETCURSEL, sel, 0);
        child(win, "STATIC", "New gamertag (up to 15 characters):", 0, 0, [12, 172, 300, 18], 0, font);
        let edit = child(win, "EDIT", "", ES_AUTOHSCROLL | WS_BORDER | WS_TABSTOP, WS_EX_CLIENTEDGE, [12, 194, 200, 24], ID_EDIT, font);
        SendMessageW(edit, EM_LIMITTEXT, 15, 0);
        child(win, "BUTTON", "New profile...", WS_TABSTOP, 0, [220, 193, 92, 26], ID_NEW, font);
        child(win, "BUTTON", "OK", BS_DEFPUSHBUTTON | WS_TABSTOP, 0, [222, 250, 90, 28], ID_OK, font);
        ShowWindow(win, 5);
        SetForegroundWindow(win);
        SetFocus(list);
        let mut msg: Msg = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if IsDialogMessageW(win, &mut msg) == 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        STATE.with(|s| {
            let s = s.borrow();
            s.selected.clone().map(|sel| (sel, s.created.clone()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("xcl_profiles_test_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d.join("profiles.json")
    }

    #[test]
    fn gamertags_are_validated() {
        assert_eq!(clean_gamertag("  Marwan "), Some("Marwan".into()));
        assert_eq!(clean_gamertag(""), None);
        assert_eq!(clean_gamertag("   "), None);
        assert_eq!(clean_gamertag("ABCDEFGHIJKLMNOP"), None); // 16
        assert_eq!(clean_gamertag("ABCDEFGHIJKLMNO").unwrap().len(), 15);
        assert_eq!(clean_gamertag("a\tb"), None);
    }

    #[test]
    fn create_is_idempotent_and_xuids_are_unique() {
        let mut s = Store::default();
        let a = s.get_or_create("Alice", 1).unwrap();
        let again = s.get_or_create("alice", 99).unwrap();
        assert_eq!(a, again, "same gamertag (any case) is the same profile");
        let b = s.get_or_create("Bob", 1).unwrap();
        assert_ne!(a.xuid, b.xuid);
        assert_eq!(a.xuid >> 48, 0x0009);
        assert_eq!(s.profiles.len(), 2);
        assert!(s.get_or_create("", 1).is_none());
    }

    #[test]
    fn xuid_collision_is_resolved() {
        let mut s = Store::default();
        let a = s.get_or_create("Same", 5).unwrap();
        // a different gamertag forced onto the same xuid is re-rolled
        s.profiles.push(Profile { gamertag: "Other".into(), xuid: make_xuid("Third", 5), picture: None });
        let t = s.get_or_create("Third", 5).unwrap();
        assert_ne!(t.xuid, make_xuid("Third", 5));
        assert_ne!(t.xuid, a.xuid);
    }

    #[test]
    fn store_round_trips_through_the_file() {
        let path = tmp("roundtrip");
        let mut s = Store::default();
        let a = s.get_or_create("Alice", 7).unwrap();
        s.get_or_create("Bob", 7).unwrap();
        s.profiles[1].picture = Some("C:\\pics\\bob.png".into());
        s.set_last(&a);
        s.save(&path).unwrap();
        let loaded = Store::load(&path);
        assert_eq!(loaded, s);
        assert_eq!(loaded.last_profile().unwrap().gamertag, "Alice");
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn missing_or_corrupt_file_is_an_empty_store() {
        let path = tmp("corrupt");
        assert_eq!(Store::load(&path), Store::default());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Store::load(&path).profiles.is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn last_profile_follows_the_set_profile() {
        let mut s = Store::default();
        let a = s.get_or_create("Alice", 1).unwrap();
        let b = s.get_or_create("Bob", 1).unwrap();
        assert!(s.last_profile().is_none());
        s.set_last(&b);
        assert_eq!(s.last_profile(), Some(&b));
        s.set_last(&a);
        assert_eq!(s.last_profile(), Some(&a));
    }
}
