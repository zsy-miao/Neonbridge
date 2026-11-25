//! The launcher window: quiet near-black greys with small pink and cyan
//! accents. Shown when the launcher starts without --game; the last picks
//! are remembered. NEONBRIDGE_PICKER=0 keeps the old behaviour (print the
//! usage instead).
//!
//! Left, the runtimes as rows (name, version, a dot: green once a run of it
//! presented frames, grey untested) and Mix. Right, the selected runtime's
//! details and its preset. At the bottom, the game, found by itself (the last
//! folder, then Downloads, Desktop and each drive's top-level folders that
//! hold MicrosoftGame.Config, one level deep), with Change (the Explorer
//! folder dialog), Advanced... and Play. Advanced is a window of its own:
//! mix mode (one list per victory crate: which tree supplies it, with
//! vendor/mix-compat.toml's clashes marked; saved mixes and the last good
//! one) and the run's options. The game row resolves a folder, an exe or
//! MicrosoftGame.Config, as OpenScarlett's launcher target row (MIT) does.

use std::{
    cell::{Cell, RefCell},
    fs,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::*,
    System::{
        Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree},
        LibraryLoader::GetModuleHandleW,
    },
    UI::{
        Controls::{DRAWITEMSTRUCT, MEASUREITEMSTRUCT, ODS_COMBOBOXEDIT, ODS_DISABLED, ODS_SELECTED, ODT_BUTTON, ODT_COMBOBOX},
        Input::KeyboardAndMouse::{EnableWindow, GetFocus, IsWindowEnabled, SetFocus, TRACKMOUSEEVENT, TME_LEAVE, TrackMouseEvent, VK_DOWN, VK_RETURN, VK_UP},
        Shell::SHCreateItemFromParsingName,
        WindowsAndMessaging::*,
    },
};
use windows_sys::core::GUID;

#[link(name = "uxtheme")]
unsafe extern "system" {
    fn SetWindowTheme(hwnd: HWND, app: *const u16, idlist: *const u16) -> i32;
}
#[link(name = "dwmapi")]
unsafe extern "system" {
    fn DwmSetWindowAttribute(hwnd: HWND, attribute: u32, value: *const core::ffi::c_void, size: u32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn LoadLibraryW(name: *const u16) -> *mut core::ffi::c_void;
    fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *mut core::ffi::c_void;
    fn GetLogicalDrives() -> u32;
}

const BACKGROUND: COLORREF = rgb(17, 18, 20);
const PANEL: COLORREF = rgb(26, 27, 30);
const ROW_HOT: COLORREF = rgb(31, 32, 36);
const ROW_ON: COLORREF = rgb(36, 37, 42);
const BORDER: COLORREF = rgb(42, 43, 48);
const BUTTON: COLORREF = rgb(42, 43, 48);
const BUTTON_HOT: COLORREF = rgb(52, 53, 59);
const TEXT: COLORREF = rgb(232, 232, 234);
const SUB: COLORREF = rgb(139, 141, 148);
const FAINT: COLORREF = rgb(92, 94, 101);
const PINK: COLORREF = rgb(255, 46, 151);
const CYAN: COLORREF = rgb(0, 229, 255);
const GREEN: COLORREF = rgb(63, 185, 80);

const fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    r as u32 | (g as u32) << 8 | (b as u32) << 16
}

const WIDTH: i32 = 880;
const HEIGHT: i32 = 560;
const HEADER: i32 = 56;
const FOOTER: i32 = 72;
const LEFT: i32 = 280;
const ROW: i32 = 56;
const ROWS_TOP: i32 = HEADER + 40;
const DETAILS: i32 = LEFT + 32;

/// The runtime list's last entry: mix mode.
const MIX: &str = "mix";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn saved() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack/last_game.txt")
}

fn last_pick() -> PathBuf {
    saved().with_file_name("last_pick.txt")
}

/// Folder, exe or MicrosoftGame.Config -> (package folder, exe name if one was picked).
fn resolve(text: &str) -> Result<(PathBuf, Option<String>), String> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return Err("Choose the game folder first.".into());
    }
    let path = Path::new(text);
    if path.is_dir() {
        return Ok((path.to_path_buf(), None));
    }
    if !path.is_file() {
        return Err(format!("{text} does not exist."));
    }
    let parent = path.parent().map(Path::to_path_buf).ok_or("The file has no folder.")?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name.eq_ignore_ascii_case("MicrosoftGame.Config") {
        Ok((parent, None))
    } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
        Ok((parent, Some(name)))
    } else {
        Err("Pick a folder, an .exe or MicrosoftGame.Config.".into())
    }
}

/// The package folder holding MicrosoftGame.Config, if `text` names one.
fn game_folder(text: &str) -> Option<PathBuf> {
    resolve(text).ok().map(|(f, _)| f).filter(|f| f.join("MicrosoftGame.Config").is_file())
}

/// The game, found without asking: the last folder, then the folders one
/// level under Downloads, Desktop and each drive's root that hold
/// MicrosoftGame.Config. Never deeper than that.
fn detect_game() -> Option<PathBuf> {
    if let Some(f) = fs::read_to_string(saved()).ok().and_then(|t| game_folder(&t)) {
        return Some(f);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("USERPROFILE").map(PathBuf::from) {
        roots.extend([home.join("Downloads"), home.join("Desktop")]);
    }
    let drives = unsafe { GetLogicalDrives() };
    for i in 2..26 {
        if drives & (1 << i) != 0 {
            roots.push(PathBuf::from(format!("{}:\\", (b'A' + i as u8) as char)));
        }
    }
    roots.into_iter().find_map(|r| {
        fs::read_dir(r).ok()?.flatten().map(|e| e.path()).find(|p| p.join("MicrosoftGame.Config").is_file())
    })
}

/// "GTA VI 1.0.2.1" from the package's MicrosoftGame.Config.
fn game_label(folder: &Path) -> String {
    let config = fs::read_to_string(folder.join("MicrosoftGame.Config")).unwrap_or_default();
    let attr = |tag: &str, key: &str| -> Option<String> {
        let at = config.find(tag)?;
        let rest = &config[at..];
        let k = rest.find(&format!("{key}=\""))? + key.len() + 2;
        Some(rest[k..].split('"').next()?.to_owned())
    };
    let name = attr("<ShellVisuals", "DefaultDisplayName").unwrap_or_else(|| "Game".into());
    let name = if name == "Grand Theft Auto VI" { "GTA VI".to_owned() } else { name };
    match attr("<Identity", "Version") {
        Some(v) => format!("{name} {v}"),
        None => name,
    }
}

/// The path as the window shows it: the home folder as ~, a long path cut
/// to its last three parts.
fn shortened(path: &Path) -> String {
    let mut p = path.display().to_string();
    if let Some(home) = std::env::var_os("USERPROFILE").map(|h| h.to_string_lossy().into_owned()).filter(|h| !h.is_empty()) {
        if p.to_lowercase().starts_with(&home.to_lowercase()) {
            p = format!("~{}", &p[home.len()..]);
        }
    }
    if p.chars().count() > 40 {
        let parts: Vec<&str> = p.split(['\\', '/']).filter(|s| !s.is_empty()).collect();
        if parts.len() > 3 {
            p = format!("\u{2026}\\{}", parts[parts.len() - 3..].join("\\"));
        }
    }
    p
}

// The Explorer folder dialog (IFileOpenDialog with FOS_PICKFOLDERS), by its
// vtable: windows-sys has the functions but not the COM interfaces.
const CLSID_FILE_OPEN_DIALOG: GUID = GUID::from_u128(0xdc1c5a9c_e88a_4dde_a5a1_60f82a20aef7);
const IID_IFILE_OPEN_DIALOG: GUID = GUID::from_u128(0xd57c7288_d4ad_4768_be02_9d969532d960);
const IID_ISHELL_ITEM: GUID = GUID::from_u128(0x43826d1e_e718_42ee_bc55_a1e261c37bfe);
const FOS_PICKFOLDERS: u32 = 0x20;
const FOS_FORCEFILESYSTEM: u32 = 0x40;
const SIGDN_FILESYSPATH: u32 = 0x8005_8000;

type Raw = *mut core::ffi::c_void;

unsafe fn vtable(obj: Raw, i: usize) -> *const core::ffi::c_void {
    unsafe { *(*(obj as *const *const *const core::ffi::c_void)).add(i) }
}

unsafe fn release(obj: Raw) {
    if !obj.is_null() {
        let f: unsafe extern "system" fn(Raw) -> u32 = unsafe { std::mem::transmute(vtable(obj, 2)) };
        unsafe { f(obj) };
    }
}

/// The Explorer-style folder picker, opening in `start`.
fn pick_folder(owner: HWND, start: Option<&Path>) -> Option<PathBuf> {
    unsafe {
        let mut dialog: Raw = null_mut();
        if CoCreateInstance(&CLSID_FILE_OPEN_DIALOG, null_mut(), CLSCTX_INPROC_SERVER, &IID_IFILE_OPEN_DIALOG, &mut dialog) < 0 || dialog.is_null() {
            return None;
        }
        // IFileDialog: GetOptions 10, SetOptions 9, SetFolder 12, SetTitle 17, GetResult 20; Show 3.
        let get_options: unsafe extern "system" fn(Raw, *mut u32) -> i32 = std::mem::transmute(vtable(dialog, 10));
        let set_options: unsafe extern "system" fn(Raw, u32) -> i32 = std::mem::transmute(vtable(dialog, 9));
        let mut options = 0u32;
        get_options(dialog, &mut options);
        set_options(dialog, options | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
        let title = wide("Choose the game folder (it holds MicrosoftGame.Config)");
        let set_title: unsafe extern "system" fn(Raw, *const u16) -> i32 = std::mem::transmute(vtable(dialog, 17));
        set_title(dialog, title.as_ptr());
        if let Some(start) = start {
            let w = wide(&start.display().to_string());
            let mut item: Raw = null_mut();
            if SHCreateItemFromParsingName(w.as_ptr(), null_mut(), &IID_ISHELL_ITEM, &mut item) >= 0 {
                let set_folder: unsafe extern "system" fn(Raw, Raw) -> i32 = std::mem::transmute(vtable(dialog, 12));
                set_folder(dialog, item);
                release(item);
            }
        }
        let show: unsafe extern "system" fn(Raw, HWND) -> i32 = std::mem::transmute(vtable(dialog, 3));
        let mut picked = None;
        if show(dialog, owner) >= 0 {
            let get_result: unsafe extern "system" fn(Raw, *mut Raw) -> i32 = std::mem::transmute(vtable(dialog, 20));
            let mut item: Raw = null_mut();
            if get_result(dialog, &mut item) >= 0 && !item.is_null() {
                // IShellItem::GetDisplayName is 5.
                let name: unsafe extern "system" fn(Raw, u32, *mut *mut u16) -> i32 = std::mem::transmute(vtable(item, 5));
                let mut text: *mut u16 = null_mut();
                if name(item, SIGDN_FILESYSPATH, &mut text) >= 0 && !text.is_null() {
                    let len = (0..).take_while(|&i| *text.add(i) != 0).count();
                    picked = Some(PathBuf::from(String::from_utf16_lossy(std::slice::from_raw_parts(text, len))));
                    CoTaskMemFree(text as *const _);
                }
                release(item);
            }
        }
        release(dialog);
        picked
    }
}

/// What the picker hands the launcher.
pub struct Pick {
    pub folder: PathBuf,
    pub exe: Option<String>,
    pub runtime: Option<String>,
    pub preset: Option<String>,
    pub mix: Option<crate::mix::Mix>,
    pub sets: Vec<String>,
}

struct Row {
    name: String,
    title: String,
    version: String,
    licence: String,
    upstream: String,
    frames: Option<u64>,
    /// A missing prerequisite: (reason, official download pages). Greyed out.
    missing: Option<(String, Vec<&'static str>)>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Hit {
    None,
    Row(usize),
    Preset(usize),
    Change,
    Advanced,
    Play,
    Install,
}

struct State {
    rows: Vec<Row>,
    sel: usize,
    hover: Hit,
    presets: Vec<String>,
    preset: usize,
    game: Option<PathBuf>,
    /// Where paint() drew each clickable thing, for the mouse.
    targets: Vec<(RECT, Hit)>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    static PICKED: RefCell<Option<Pick>> = const { RefCell::new(None) };
    static FONTS: RefCell<Option<Fonts>> = const { RefCell::new(None) };
    static BRUSHES: RefCell<Option<(HBRUSH, HBRUSH)>> = const { RefCell::new(None) };
    static ADVANCED: Cell<HWND> = const { Cell::new(null_mut()) };
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
    static COMBO_PROC: Cell<isize> = const { Cell::new(0) };
    /// The Diagnostics box (NEONBRIDGE_DIAG=1 for the run).
    static DIAG: Cell<bool> = const { Cell::new(false) };
}

fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> Option<R> {
    STATE.with(|s| s.borrow_mut().as_mut().map(f))
}

struct Fonts {
    word: HFONT,
    title: HFONT,
    body: HFONT,
    strong: HFONT,
    small: HFONT,
}

fn font(face: &str, height: i32, weight: i32) -> HFONT {
    let face = wide(face);
    unsafe { CreateFontW(height, 0, 0, 0, weight, 0, 0, 0, DEFAULT_CHARSET as u32, 0, 0, CLEARTYPE_QUALITY as u32, 0, face.as_ptr()) }
}

fn fonts<R>(f: impl FnOnce(&Fonts) -> R) -> R {
    FONTS.with(|c| {
        let mut c = c.borrow_mut();
        let fonts = c.get_or_insert_with(|| Fonts {
            word: font("Segoe UI Variable Display", 20, 700),
            title: font("Segoe UI Variable Display", 28, 600),
            body: font("Segoe UI Variable Text", 16, 400),
            strong: font("Segoe UI Variable Text", 16, 600),
            small: font("Segoe UI Variable Text", 14, 400),
        });
        f(fonts)
    })
}

/// (window background, panel) brushes.
fn brushes() -> (HBRUSH, HBRUSH) {
    BRUSHES.with(|b| *b.borrow_mut().get_or_insert_with(|| unsafe { (CreateSolidBrush(BACKGROUND), CreateSolidBrush(PANEL)) }))
}

/// uxtheme's unnamed exports (ordinals): SetPreferredAppMode (135) and
/// AllowDarkModeForWindow (133), for dark scrollbars and lists, as Explorer
/// uses them. Missing on old Windows: no-op.
fn dark_export(ordinal: usize) -> *mut core::ffi::c_void {
    let name = wide("uxtheme.dll");
    unsafe {
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() { null_mut() } else { GetProcAddress(module, ordinal as *const u8) }
    }
}

fn dark_app() {
    let f = dark_export(135);
    if !f.is_null() {
        let f: unsafe extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(f) };
        unsafe { f(2) }; // ForceDark
    }
}

fn dark_window(hwnd: HWND) {
    let f = dark_export(133);
    if !f.is_null() {
        let f: unsafe extern "system" fn(HWND, i32) -> i32 = unsafe { std::mem::transmute(f) };
        unsafe { f(hwnd, 1) };
    }
    let on = 1u32;
    unsafe { DwmSetWindowAttribute(hwnd, 20 /* DWMWA_USE_IMMERSIVE_DARK_MODE */, &on as *const u32 as *const _, 4) };
}

// ---------------------------------------------------------------- drawing

fn fill_rect(dc: HDC, r: RECT, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        FillRect(dc, &r, brush);
        DeleteObject(brush);
    }
}

fn rounded(dc: HDC, r: RECT, fill: COLORREF, edge: COLORREF, radius: i32, behind: COLORREF) {
    unsafe {
        fill_rect(dc, r, behind);
        let brush = CreateSolidBrush(fill);
        let pen = CreatePen(PS_SOLID, 1, edge);
        let (ob, op) = (SelectObject(dc, brush), SelectObject(dc, pen));
        RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(brush);
        DeleteObject(pen);
    }
}

fn dot(dc: HDC, cx: i32, cy: i32, color: COLORREF) {
    unsafe {
        let brush = CreateSolidBrush(color);
        let (ob, op) = (SelectObject(dc, brush), SelectObject(dc, GetStockObject(NULL_PEN)));
        Ellipse(dc, cx - 4, cy - 4, cx + 5, cy + 5);
        SelectObject(dc, ob);
        SelectObject(dc, op);
        DeleteObject(brush);
    }
}

fn text(dc: HDC, s: &str, mut r: RECT, color: COLORREF, f: HFONT, flags: u32) {
    let mut w: Vec<u16> = s.encode_utf16().collect();
    unsafe {
        let old = SelectObject(dc, f);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, color);
        DrawTextW(dc, w.as_mut_ptr(), w.len() as i32, &mut r, flags | DT_SINGLELINE | DT_NOPREFIX);
        SelectObject(dc, old);
    }
}

fn width_of(dc: HDC, s: &str, f: HFONT) -> i32 {
    let w: Vec<u16> = s.encode_utf16().collect();
    let mut size: windows_sys::Win32::Foundation::SIZE = unsafe { std::mem::zeroed() };
    unsafe {
        let old = SelectObject(dc, f);
        GetTextExtentPoint32W(dc, w.as_ptr(), w.len() as i32, &mut size);
        SelectObject(dc, old);
    }
    size.cx
}

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT { left: l, top: t, right: r, bottom: b }
}

fn paint(hwnd: HWND) {
    unsafe {
        let mut ps: PAINTSTRUCT = std::mem::zeroed();
        let screen = BeginPaint(hwnd, &mut ps);
        let mut client: RECT = std::mem::zeroed();
        GetClientRect(hwnd, &mut client);
        let (w, h) = (client.right, client.bottom);
        let dc = CreateCompatibleDC(screen);
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old_bitmap = SelectObject(dc, bitmap);
        with_state(|s| fonts(|f| draw(dc, w, h, s, f)));
        BitBlt(screen, 0, 0, w, h, dc, 0, 0, SRCCOPY);
        SelectObject(dc, old_bitmap);
        DeleteObject(bitmap);
        DeleteDC(dc);
        EndPaint(hwnd, &ps);
    }
}

fn draw(dc: HDC, w: i32, h: i32, s: &mut State, f: &Fonts) {
    s.targets.clear();
    fill_rect(dc, rect(0, 0, w, h), BACKGROUND);

    // Header: the wordmark, small, and one 1 px line.
    let neon = width_of(dc, "NEON", f.word);
    text(dc, "NEON", rect(24, 0, 200, HEADER), PINK, f.word, DT_LEFT | DT_VCENTER);
    text(dc, "BRIDGE", rect(24 + neon, 0, 240, HEADER), CYAN, f.word, DT_LEFT | DT_VCENTER);
    fill_rect(dc, rect(0, HEADER, w, HEADER + 1), BORDER);

    // Left: the runtimes.
    let foot = h - FOOTER;
    fill_rect(dc, rect(0, HEADER + 1, LEFT, foot), PANEL);
    fill_rect(dc, rect(LEFT, HEADER + 1, LEFT + 1, foot), BORDER);
    text(dc, "Runtimes", rect(24, HEADER + 12, LEFT, HEADER + 36), SUB, f.small, DT_LEFT | DT_VCENTER);
    for (i, row) in s.rows.iter().enumerate() {
        let r = rect(0, ROWS_TOP + i as i32 * ROW, LEFT, ROWS_TOP + (i as i32 + 1) * ROW);
        if i == s.sel {
            fill_rect(dc, r, ROW_ON);
            fill_rect(dc, rect(0, r.top, 2, r.bottom), PINK);
        } else if s.hover == Hit::Row(i) {
            fill_rect(dc, r, ROW_HOT);
        }
        let grey = row.missing.is_some();
        text(dc, &row.title, rect(24, r.top + 8, LEFT - 40, r.top + 30), if grey { FAINT } else { TEXT }, f.strong, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
        let sub = if row.name == MIX { "Each crate from a tree".to_owned() } else if grey { format!("{} \u{00B7} not installed", row.version) } else { row.version.clone() };
        text(dc, &sub, rect(24, r.top + 30, LEFT - 40, r.top + 50), SUB, f.small, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
        if row.name != MIX {
            dot(dc, LEFT - 24, r.top + ROW / 2, if row.frames.is_some_and(|n| n > 0) { GREEN } else { FAINT });
        }
        s.targets.push((r, Hit::Row(i)));
    }

    // Right: the selected runtime.
    if let Some(row) = s.rows.get(s.sel) {
        let x = DETAILS;
        text(dc, &row.title, rect(x, HEADER + 24, w - 32, HEADER + 64), TEXT, f.title, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
        let lines: Vec<(&str, String)> = if row.name == MIX {
            let app = UI.with(|u| u.borrow().as_ref().and_then(|ui| current_mix(ui).app().map(str::to_owned))).unwrap_or_default();
            vec![
                ("Build", "One victory build from crates of several trees".into()),
                ("App from", if app.is_empty() { "-".into() } else { crate::runtimes::title(&app) }),
                ("Crates", "Picked in Advanced".into()),
            ]
        } else {
            vec![
                ("Version", row.version.clone()),
                ("Licence", row.licence.clone()),
                ("Upstream", row.upstream.clone()),
                ("Last run", match row.frames {
                    None => "Not run yet".into(),
                    Some(0) => "Started, no frames presented".into(),
                    Some(n) => format!("{n} frames presented"),
                }),
            ]
        };
        let mut y = HEADER + 84;
        for (k, v) in &lines {
            text(dc, k, rect(x, y, x + 104, y + 28), SUB, f.body, DT_LEFT | DT_VCENTER);
            text(dc, v, rect(x + 112, y, w - 32, y + 28), TEXT, f.body, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            y += 32;
        }
        // A missing prerequisite: why, and a button to its download pages.
        if let Some((why, pages)) = &row.missing {
            let why = why.strip_prefix("needs ").map_or_else(|| why.clone(), |w| format!("Needs {w}"));
            text(dc, "Can't run", rect(x, y, x + 104, y + 28), SUB, f.body, DT_LEFT | DT_VCENTER);
            text(dc, &why, rect(x + 112, y, w - 32, y + 28), PINK, f.body, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            y += 36;
            if !pages.is_empty() {
                let label = "Install\u{2026}";
                let b = rect(x + 112, y, x + 112 + width_of(dc, label, f.body) + 40, y + 32);
                let hot = s.hover == Hit::Install;
                rounded(dc, b, if hot { BUTTON_HOT } else { BUTTON }, if hot { CYAN } else { BUTTON }, 6, BACKGROUND);
                text(dc, label, b, TEXT, f.body, DT_CENTER | DT_VCENTER);
                s.targets.push((b, Hit::Install));
                text(dc, "Opens the official download page in your browser", rect(b.right + 16, y, w - 32, y + 32), SUB, f.small, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                y += 40;
            }
        }
        y += 16;
        fill_rect(dc, rect(x, y, w - 32, y + 1), BORDER);
        y += 24;
        text(dc, "Preset", rect(x, y, x + 200, y + 20), SUB, f.small, DT_LEFT | DT_VCENTER);
        y += 28;
        // A segmented control: one segment per preset.
        let mut sx = x;
        let seg_h = 32;
        let labels: Vec<String> = s.presets.iter().enumerate().map(|(i, p)| if i == 0 { "Default".to_owned() } else { p.clone() }).collect();
        for (i, label) in labels.iter().enumerate() {
            let sw = width_of(dc, label, f.body) + 32;
            let r = rect(sx, y, sx + sw, y + seg_h);
            let on = i == s.preset;
            fill_rect(dc, r, if on { BUTTON } else if s.hover == Hit::Preset(i) { ROW_HOT } else { BACKGROUND });
            text(dc, label, r, if on { TEXT } else { SUB }, f.body, DT_CENTER | DT_VCENTER);
            if on {
                fill_rect(dc, rect(r.left, r.bottom - 2, r.right, r.bottom), CYAN);
            }
            if i > 0 {
                fill_rect(dc, rect(r.left, r.top, r.left + 1, r.bottom), BORDER);
            }
            s.targets.push((r, Hit::Preset(i)));
            sx += sw;
        }
        unsafe {
            let brush = CreateSolidBrush(BORDER);
            FrameRect(dc, &rect(x, y, sx, y + seg_h), brush);
            DeleteObject(brush);
        }
    }

    // Footer: the game, Advanced... and Play.
    fill_rect(dc, rect(0, foot, w, foot + 1), BORDER);
    let mid = foot + FOOTER / 2;
    let mut gx = 24;
    match &s.game {
        Some(g) => {
            let label = game_label(g);
            text(dc, &label, rect(gx, mid - 12, gx + 300, mid + 12), TEXT, f.strong, DT_LEFT | DT_VCENTER);
            gx += width_of(dc, &label, f.strong) + 12;
            let path = format!("\u{00B7}  {}", shortened(g));
            text(dc, &path, rect(gx, mid - 12, gx + 320, mid + 12), SUB, f.body, DT_LEFT | DT_VCENTER);
            gx += width_of(dc, &path, f.body) + 16;
        }
        None => {
            let label = "No game found";
            text(dc, label, rect(gx, mid - 12, gx + 300, mid + 12), SUB, f.strong, DT_LEFT | DT_VCENTER);
            gx += width_of(dc, label, f.strong) + 16;
        }
    }
    let change = if s.game.is_some() { "Change" } else { "Choose folder\u{2026}" };
    let cw = width_of(dc, change, f.body);
    let cr = rect(gx, mid - 12, gx + cw, mid + 12);
    text(dc, change, cr, if s.hover == Hit::Change { TEXT } else { SUB }, f.body, DT_LEFT | DT_VCENTER);
    fill_rect(dc, rect(cr.left, mid + 9, cr.right, mid + 10), if s.hover == Hit::Change { TEXT } else { FAINT });
    s.targets.push((cr, Hit::Change));

    let play = rect(w - 24 - 128, mid - 20, w - 24, mid + 20);
    let ready = s.game.is_some() && s.rows.get(s.sel).is_none_or(|r| r.missing.is_none());
    let hot = s.hover == Hit::Play && ready;
    rounded(dc, play, if hot { BUTTON_HOT } else { BUTTON }, if hot { PINK } else { BUTTON }, 6, BACKGROUND);
    text(dc, "Play", play, if ready { TEXT } else { FAINT }, f.strong, DT_CENTER | DT_VCENTER);
    s.targets.push((play, Hit::Play));
    let adv = "Advanced\u{2026}";
    let aw = width_of(dc, adv, f.body);
    let ar = rect(play.left - 24 - aw, mid - 12, play.left - 24, mid + 12);
    text(dc, adv, ar, if s.hover == Hit::Advanced { TEXT } else { SUB }, f.body, DT_LEFT | DT_VCENTER);
    s.targets.push((ar, Hit::Advanced));
}

fn hit(x: i32, y: i32) -> Hit {
    with_state(|s| s.targets.iter().find(|(r, _)| x >= r.left && x < r.right && y >= r.top && y < r.bottom).map(|(_, h)| *h).unwrap_or(Hit::None)).unwrap_or(Hit::None)
}

// --------------------------------------------------------- runtime state

fn is_mix() -> bool {
    with_state(|s| s.rows.get(s.sel).is_some_and(|r| r.name == MIX)).unwrap_or(false)
}

/// The runtime whose toml runs: the picked one, or the mix's app tree.
fn running() -> String {
    if is_mix() {
        UI.with(|u| u.borrow().as_ref().and_then(|ui| current_mix(ui).app().map(str::to_owned))).unwrap_or_else(|| "scarlettport".into())
    } else {
        with_state(|s| s.rows.get(s.sel).map(|r| r.name.clone())).flatten().unwrap_or_default()
    }
}

fn refill_presets(want: Option<&str>) {
    let mut names = vec!["(author)".to_owned()];
    names.extend(crate::runtimes::presets(&running()));
    with_state(|s| {
        s.preset = want.and_then(|w| names.iter().position(|n| n == w)).unwrap_or(0);
        s.presets = names;
    });
}

fn rows() -> Vec<Row> {
    let mut names = vec![crate::runtimes::TUXBOX.to_owned()];
    names.extend(crate::runtimes::names());
    let mut rows: Vec<Row> = names
        .into_iter()
        .map(|n| {
            let about = crate::runtimes::about(&n);
            let title = crate::runtimes::title(&n);
            let project = title.split(" (").next().unwrap_or(&title).to_owned();
            let (name_part, version) = match project.rsplit_once(' ') {
                Some((a, v)) if v.len() >= 7 && v.chars().all(|c| c.is_ascii_hexdigit()) => (a.to_owned(), v.to_owned()),
                _ => (project.clone(), "Neonbridge".to_owned()),
            };
            let licence = ["GPL-3.0", "GPL-2.0", "MIT", "Apache-2.0"].iter().find(|l| about.contains(*l)).map_or("-".to_owned(), |l| (*l).to_owned());
            let upstream = fs::read_to_string(crate::runtimes::vendor().join(&n).join("UPSTREAM")).ok().and_then(|u| u.lines().next().map(|l| l.trim().chars().take(12).collect())).unwrap_or_else(|| "-".into());
            Row { frames: crate::runtimes::last_run_frames(&n), missing: crate::runtimes::missing(&n), name: n, title: name_part, version, licence, upstream }
        })
        .collect();
    rows.push(Row { name: MIX.into(), title: "Mix".into(), version: String::new(), licence: String::new(), upstream: String::new(), frames: None, missing: None });
    rows
}

/// The remembered picks: runtime=, preset=, part.<name>=, opt= lines.
fn remembered() -> Vec<(String, String)> {
    fs::read_to_string(last_pick()).unwrap_or_default().lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

/// Install... for the PC GDK: asks first, then fetches Microsoft's GDK
/// release (about 1.4 GB) into %LOCALAPPDATA%\6ixpack\gdk on a thread of
/// its own, and re-checks the runtimes when it is done.
fn install_gdk(hwnd: HWND) {
    let text = wide(&format!(
        "Download the Microsoft PC GDK {} (about 1.4 GB) from github.com/microsoft/GDK into {}?\n\nIt is Microsoft's own release, used only to build Five Star Recomp.",
        crate::runtimes::GDK_VERSION,
        crate::runtimes::gdk_dir().display()
    ));
    let title = wide("Neonbridge");
    // SAFETY: both strings are NUL-terminated and live across the call.
    if unsafe { MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION) } != IDYES {
        return;
    }
    let h = hwnd as isize;
    std::thread::spawn(move || {
        let r = crate::runtimes::fetch_gdk();
        let hwnd = h as HWND;
        match r {
            Ok(()) => message(hwnd, "The PC GDK is installed for Neonbridge.", MB_ICONINFORMATION),
            Err(e) => message(hwnd, &format!("The PC GDK was not installed: {e}"), MB_ICONWARNING),
        }
        // Back on the window's thread: rows are rebuilt by the next refresh.
        unsafe { PostMessageW(hwnd, WM_APP + 7, 0, 0) };
    });
}

fn message(hwnd: HWND, text: &str, icon: u32) {
    let (text, title) = (wide(text), wide("Neonbridge"));
    unsafe { MessageBoxW(hwnd, text.as_ptr(), title.as_ptr(), MB_OK | icon) };
}

fn select(hwnd: HWND, i: usize) {
    let changed = with_state(|s| {
        let ok = i < s.rows.len() && i != s.sel;
        if ok {
            s.sel = i;
        }
        ok
    });
    if changed == Some(true) {
        refill_presets(None);
        if is_mix() {
            UI.with(|u| {
                if let Some(ui) = u.borrow().as_ref() {
                    if !crate::mix::conflicts(&current_mix(ui)).is_empty() {
                        if let Some(app) = current_mix(ui).app().map(str::to_owned) {
                            unify(ui, &app);
                        }
                    }
                    check_mix(ui);
                }
            });
        }
        mix_mode();
        unsafe { InvalidateRect(hwnd, null(), 0) };
    }
}

fn play(hwnd: HWND) {
    let Some(game) = with_state(|s| s.game.clone()).flatten() else {
        change_game(hwnd);
        return;
    };
    let Ok((folder, exe)) = resolve(&game.display().to_string()) else {
        message(hwnd, "The game folder is gone; choose it again.", MB_ICONWARNING);
        return;
    };
    let _ = fs::create_dir_all(saved().parent().unwrap_or(Path::new(".")));
    let _ = fs::write(saved(), game.display().to_string());
    let mix = is_mix();
    let runtime = running();
    let (sel_name, preset) = with_state(|s| (s.rows.get(s.sel).map(|r| r.name.clone()).unwrap_or_default(), (s.preset > 0).then(|| s.presets.get(s.preset).cloned()).flatten())).unwrap_or_default();
    let (parts, sets) = UI.with(|u| {
        let u = u.borrow();
        let Some(ui) = u.as_ref() else { return (None, Vec::new()) };
        let sets: Vec<String> = edit_text(ui.options).lines().map(str::trim).filter(|l| l.contains('=')).map(str::to_owned).collect();
        (Some(current_mix(ui)), sets)
    });
    let mut memory = format!("runtime={sel_name}\n");
    if let Some(p) = &preset {
        memory.push_str(&format!("preset={p}\n"));
    }
    // Part picks are remembered only for a mix run: stale part.* lines made a
    // plain run look like a mix.
    if mix {
        for (k, v) in parts.iter().flat_map(|m| m.parts.iter()) {
            memory.push_str(&format!("part.{k}={v}\n"));
        }
    }
    for s in &sets {
        memory.push_str(&format!("opt={s}\n"));
    }
    if DIAG.get() {
        memory.push_str("diag=1\n");
        // SAFETY: the picker's thread is the launcher's only one here.
        unsafe { std::env::set_var("NEONBRIDGE_DIAG", "1") };
    }
    let _ = fs::write(last_pick(), memory);
    PICKED.with(|p| *p.borrow_mut() = Some(Pick { folder, exe, runtime: Some(runtime), preset, mix: if mix { parts } else { None }, sets }));
    unsafe { DestroyWindow(hwnd) };
}

fn change_game(hwnd: HWND) {
    let start = with_state(|s| s.game.clone()).flatten().or_else(|| std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads")));
    if let Some(p) = pick_folder(hwnd, start.as_deref()) {
        match game_folder(&p.display().to_string()) {
            Some(g) => {
                with_state(|s| s.game = Some(g));
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
            None => message(hwnd, "That folder has no MicrosoftGame.Config. Pick the game's package folder.", MB_ICONWARNING),
        }
    }
}

fn click(hwnd: HWND, h: Hit) {
    match h {
        Hit::Row(i) => select(hwnd, i),
        Hit::Preset(i) => {
            with_state(|s| s.preset = i);
            unsafe { InvalidateRect(hwnd, null(), 0) };
        }
        Hit::Change => change_game(hwnd),
        Hit::Advanced => show_advanced(),
        Hit::Play => {
            if with_state(|s| s.rows.get(s.sel).is_some_and(|r| r.missing.is_some())) != Some(true) {
                play(hwnd);
            }
        }
        Hit::Install => {
            // Only opens the pages: nothing is downloaded or installed here.
            for page in with_state(|s| s.rows.get(s.sel).and_then(|r| r.missing.as_ref().map(|m| m.1.clone()))).flatten().unwrap_or_default() {
                if page == crate::runtimes::GDK_DOWNLOAD {
                    install_gdk(hwnd);
                } else {
                    let _ = crate::runtimes::command("explorer").arg(page).spawn();
                }
            }
        }
        Hit::None => {}
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            dark_window(hwnd);
            0
        }
        WM_PAINT => {
            paint(hwnd);
            0
        }
        // install_gdk finished: re-check which runtimes can run.
        m if m == WM_APP + 7 => {
            let fresh = rows();
            with_state(|s| {
                for (row, new) in s.rows.iter_mut().zip(fresh) {
                    row.missing = new.missing;
                }
            });
            unsafe { InvalidateRect(hwnd, null(), 0) };
            0
        }
        WM_ERASEBKGND => 1,
        WM_MOUSEMOVE => {
            let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
            let h = hit(x, y);
            if with_state(|s| std::mem::replace(&mut s.hover, h) != h) == Some(true) {
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
            let mut t = TRACKMOUSEEVENT { cbSize: size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: hwnd, dwHoverTime: 0 };
            unsafe { TrackMouseEvent(&mut t) };
            0
        }
        WM_MOUSELEAVE => {
            with_state(|s| s.hover = Hit::None);
            unsafe { InvalidateRect(hwnd, null(), 0) };
            0
        }
        WM_SETCURSOR if with_state(|s| s.hover).is_some_and(|h| h != Hit::None) => {
            unsafe { SetCursor(LoadCursorW(null_mut(), IDC_HAND)) };
            1
        }
        WM_LBUTTONUP => {
            let (x, y) = ((lparam & 0xffff) as i16 as i32, ((lparam >> 16) & 0xffff) as i16 as i32);
            click(hwnd, hit(x, y));
            0
        }
        WM_KEYDOWN => {
            let sel = with_state(|s| s.sel).unwrap_or(0);
            match wparam as u16 {
                VK_UP if sel > 0 => select(hwnd, sel - 1),
                VK_DOWN => select(hwnd, sel + 1),
                VK_RETURN => click(hwnd, Hit::Play),
                _ => {}
            }
            0
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

// ------------------------------------------------------- Advanced window

const WM_MOUSELEAVE: u32 = 0x02A3;
const ID_SAVED: usize = 112;
const ID_NAME: usize = 113;
const ID_SAVE: usize = 114;
const ID_OPTS_HELP: usize = 115;
const ID_OPTIONS: usize = 116;
const ID_DONE: usize = 117;
const ID_DIAG: usize = 118;
const ID_STATUS: usize = 901;
const ID_PART: usize = 200;
const CBN_SELCHANGE: usize = 1;
const ADV_WIDTH: i32 = 640;

struct Ui {
    saved: HWND,
    /// Saved mixes in the saved list's order (index 0: none).
    saved_names: Vec<String>,
    name: HWND,
    save: HWND,
    /// (part, its list, the trees in the list's order).
    parts: Vec<(String, HWND, Vec<String>)>,
    options: HWND,
    status: HWND,
}

fn child(parent: HWND, class: &str, text: &str, style: u32, (x, y, w, h): (i32, i32, i32, i32), id: usize) -> HWND {
    let (class_w, text) = (wide(class), wide(text));
    let hwnd = unsafe {
        CreateWindowExW(0, class_w.as_ptr(), text.as_ptr(), WS_CHILD | WS_VISIBLE | style,
            x, y, w, h, parent, id as _, GetModuleHandleW(null()), null())
    };
    let f = fonts(|f| f.body);
    unsafe { SendMessageW(hwnd, WM_SETFONT, f as WPARAM, 1) };
    dark_window(hwnd);
    let theme = wide(if class == "COMBOBOX" { "DarkMode_CFD" } else { "DarkMode_Explorer" });
    unsafe { SetWindowTheme(hwnd, theme.as_ptr(), null()) };
    hwnd
}

fn label(parent: HWND, text: &str, rect: (i32, i32, i32, i32)) {
    child(parent, "STATIC", text, 0, (rect.0, rect.1 + 6, rect.2, rect.3), 0);
}

fn button(parent: HWND, text: &str, rect: (i32, i32, i32, i32), id: usize) -> HWND {
    child(parent, "BUTTON", text, WS_TABSTOP | BS_OWNERDRAW as u32, rect, id)
}

fn edit(parent: HWND, style: u32, rect: (i32, i32, i32, i32), id: usize) -> HWND {
    // Borderless; the window paints its 1 px frame.
    child(parent, "EDIT", "", WS_TABSTOP | style, (rect.0 + 8, rect.1 + 6, rect.2 - 16, rect.3 - 12), id)
}

fn combo(parent: HWND, rect: (i32, i32, i32, i32), id: usize) -> HWND {
    let h = child(parent, "COMBOBOX", "", WS_TABSTOP | WS_VSCROLL | (CBS_DROPDOWNLIST | CBS_OWNERDRAWFIXED | CBS_HASSTRINGS) as u32, (rect.0, rect.1, rect.2, 320), id);
    // Its closed face is painted by combo_proc; the list is owner-drawn.
    let old = unsafe { SetWindowLongPtrW(h, GWLP_WNDPROC, combo_proc as *const () as isize) };
    COMBO_PROC.set(old);
    h
}

fn combo_text(h: HWND, i: isize) -> String {
    if i < 0 {
        return String::new();
    }
    let len = unsafe { SendMessageW(h, CB_GETLBTEXTLEN, i as usize, 0) }.max(0) as usize;
    let mut buf = vec![0u16; len + 1];
    unsafe { SendMessageW(h, CB_GETLBTEXT, i as usize, buf.as_mut_ptr() as LPARAM) };
    String::from_utf16_lossy(&buf[..len])
}

/// A combo's closed face: a flat panel box with a 1 px border (cyan when
/// focused), the picked item and a chevron.
unsafe extern "system" fn combo_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let old: WNDPROC = unsafe { std::mem::transmute(COMBO_PROC.get()) };
    match msg {
        WM_PAINT => unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let on = IsWindowEnabled(hwnd) != 0;
            rounded(dc, r, PANEL, if GetFocus() == hwnd { CYAN } else { BORDER }, 6, BACKGROUND);
            let s = combo_text(hwnd, SendMessageW(hwnd, CB_GETCURSEL, 0, 0));
            let color = if !on || s.starts_with('\u{2717}') { FAINT } else { TEXT };
            fonts(|f| {
                text(dc, &s, rect(r.left + 10, r.top, r.right - 28, r.bottom), color, f.body, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
                text(dc, "\u{2304}", rect(r.right - 26, r.top - 4, r.right - 8, r.bottom), if on { SUB } else { FAINT }, f.body, DT_CENTER | DT_VCENTER);
            });
            EndPaint(hwnd, &ps);
            0
        },
        WM_ERASEBKGND => 1,
        WM_SETFOCUS | WM_KILLFOCUS | WM_ENABLE => unsafe {
            let r = CallWindowProcW(old, hwnd, msg, wparam, lparam);
            InvalidateRect(hwnd, null(), 0);
            r
        },
        _ => unsafe { CallWindowProcW(old, hwnd, msg, wparam, lparam) },
    }
}

fn fill(list: HWND, items: &[String], select: usize) {
    unsafe { SendMessageW(list, CB_RESETCONTENT, 0, 0) };
    for i in items {
        let w = wide(i);
        unsafe { SendMessageW(list, CB_ADDSTRING, 0, w.as_ptr() as LPARAM) };
    }
    unsafe { SendMessageW(list, CB_SETCURSEL, select, 0) };
}

fn selected(list: HWND) -> usize {
    let i = unsafe { SendMessageW(list, CB_GETCURSEL, 0, 0) };
    if i < 0 { 0 } else { i as usize }
}

fn edit_text(edit: HWND) -> String {
    let mut buf = vec![0u16; 8192];
    let n = unsafe { GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn current_mix(ui: &Ui) -> crate::mix::Mix {
    crate::mix::Mix {
        parts: ui.parts.iter().filter_map(|(p, h, trees)| trees.get(selected(*h)).map(|t| (p.clone(), t.clone()))).collect(),
    }
}

/// A tree's title without its parenthesis, for the crate grid.
fn short_title(tree: &str) -> String {
    let t = crate::runtimes::title(tree);
    t.split(" (").next().unwrap_or(&t).to_owned()
}

/// Marks each part's trees that clash with the other picks, per
/// vendor/mix-compat.toml (drawn faint); the selection stays.
fn mark_clashes(ui: &Ui) {
    let mix = current_mix(ui);
    for (p, h, trees) in &ui.parts {
        let titles: Vec<String> = trees
            .iter()
            .map(|t| {
                let title = short_title(t);
                if crate::mix::clash(&mix, p, t).is_some() { format!("\u{2717} {title} (incompatible)") } else { title }
            })
            .collect();
        fill(*h, &titles, selected(*h));
    }
}

fn set_mix(ui: &Ui, mix: &crate::mix::Mix) {
    for (p, h, trees) in &ui.parts {
        if let Some(i) = mix.parts.get(p).and_then(|t| trees.iter().position(|x| x == t)) {
            unsafe { SendMessageW(*h, CB_SETCURSEL, i, 0) };
        }
    }
}

/// Every part from `tree` where it has one: a consistent mix to start from.
fn unify(ui: &Ui, tree: &str) {
    for (_, h, trees) in &ui.parts {
        if let Some(i) = trees.iter().position(|t| t == tree) {
            unsafe { SendMessageW(*h, CB_SETCURSEL, i, 0) };
        }
    }
}

/// Marks the clashes and says under the grid why the mix cannot build
/// (vendor/mix-compat.toml, or a compile that failed before); no pop-ups.
fn check_mix(ui: &Ui) {
    mark_clashes(ui);
    let mix = current_mix(ui);
    let bad = crate::mix::conflicts(&mix);
    let line = if let Some(first) = bad.first() {
        let more = if bad.len() > 1 { format!(" (+{} more)", bad.len() - 1) } else { String::new() };
        format!("Incompatible: {first}{more}")
    } else if let Some(e) = crate::mix::incompatible(&mix) {
        format!("Did not compile before: {}", e.lines().next().unwrap_or(""))
    } else {
        String::new()
    };
    let w = wide(&line);
    unsafe { SetWindowTextW(ui.status, w.as_ptr()) };
}

/// The crate grid works only when the runtime is Mix.
fn mix_mode() {
    let on = is_mix();
    UI.with(|u| {
        if let Some(ui) = u.borrow().as_ref() {
            for h in [ui.saved, ui.name, ui.save] {
                unsafe { EnableWindow(h, on as i32) };
            }
            for (_, h, _) in &ui.parts {
                unsafe { EnableWindow(*h, on as i32) };
            }
        }
    });
}

/// Every option of the runtime that runs, or of every tree in the mix (they
/// mix freely: each is an environment switch or the run's own flag).
fn options_help() -> String {
    let mut trees = vec![running()];
    if is_mix() {
        UI.with(|u| {
            if let Some(ui) = u.borrow().as_ref() {
                trees.extend(current_mix(ui).parts.into_values());
            }
        });
    }
    trees.sort();
    trees.dedup();
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::new();
    for t in trees {
        for o in crate::runtimes::options_of(&t) {
            let key = o.split(['=', ' ']).next().unwrap_or("").to_owned();
            if !seen.contains(&key) {
                out.push_str(&format!("{o}   [{t}]\n"));
                seen.push(key);
            }
        }
    }
    if out.is_empty() { "No options listed for this runtime.".into() } else { out }
}

/// The options list in a dark, scrollable window of its own.
fn options_window(owner: HWND, title: &str, body: &str) {
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        match msg {
            WM_CTLCOLOREDIT | WM_CTLCOLORSTATIC => unsafe {
                SetTextColor(wparam as HDC, TEXT);
                SetBkColor(wparam as HDC, PANEL);
                brushes().1 as LRESULT
            },
            WM_SIZE => unsafe {
                let edit = GetDlgItem(hwnd, 1);
                let (w, h) = ((lparam & 0xffff) as i32, ((lparam >> 16) & 0xffff) as i32);
                SetWindowPos(edit, null_mut(), 16, 16, w - 32, h - 32, SWP_NOZORDER);
                0
            },
            _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        }
    }
    let class = wide("NeonbridgeOptions");
    let instance = unsafe { GetModuleHandleW(null()) };
    let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
    wc.lpfnWndProc = Some(proc);
    wc.hInstance = instance;
    wc.lpszClassName = class.as_ptr();
    wc.hbrBackground = brushes().1;
    wc.hCursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    unsafe { RegisterClassW(&wc) };
    let t = wide(title);
    let mut r: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner, &mut r) };
    let win = unsafe { CreateWindowExW(0, class.as_ptr(), t.as_ptr(), WS_OVERLAPPEDWINDOW | WS_VISIBLE, r.left + 40, r.top + 40, 900, 560, owner, null_mut(), instance, null()) };
    if win.is_null() {
        return;
    }
    dark_window(win);
    let edit = child(win, "EDIT", "", WS_VSCROLL | WS_HSCROLL | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL | ES_AUTOHSCROLL) as u32, (16, 16, 860, 500), 1);
    let text = wide(&body.replace('\n', "\r\n"));
    unsafe { SetWindowTextW(edit, text.as_ptr()) };
    let mut c: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetClientRect(win, &mut c) };
    unsafe { SetWindowPos(edit, null_mut(), 16, 16, c.right - 32, c.bottom - 32, SWP_NOZORDER) };
}

fn show_advanced() {
    let h = ADVANCED.get();
    if !h.is_null() {
        unsafe {
            ShowWindow(h, SW_SHOW);
            SetForegroundWindow(h);
        }
    }
}

fn advanced_command(hwnd: HWND, id: usize, code: usize) {
    match id {
        ID_DIAG => unsafe {
            DIAG.set(!DIAG.get());
            InvalidateRect(GetDlgItem(hwnd, ID_DIAG as i32), null(), 0);
        },
        ID_DONE => unsafe {
            ShowWindow(hwnd, SW_HIDE);
        },
        ID_SAVED if code == CBN_SELCHANGE => UI.with(|u| {
            if let Some(ui) = u.borrow().as_ref() {
                let pick = ui.saved_names.get(selected(ui.saved)).cloned().unwrap_or_default();
                if let Some(m) = crate::mix::load(&pick) {
                    set_mix(ui, &m);
                    check_mix(ui);
                    let w = wide(&pick);
                    unsafe { SetWindowTextW(ui.name, w.as_ptr()) };
                }
            }
        }),
        ID_SAVE => UI.with(|u| {
            if let Some(ui) = u.borrow().as_ref() {
                let name = edit_text(ui.name);
                if name.trim().is_empty() {
                    message(hwnd, "Name the mix first.", MB_ICONWARNING);
                } else if let Err(e) = crate::mix::save(name.trim(), &current_mix(ui)) {
                    message(hwnd, &format!("Saving the mix: {e}"), MB_ICONWARNING);
                }
            }
        }),
        ID_OPTS_HELP => options_window(hwnd, &format!("{}: options", crate::runtimes::title(&running())), &options_help()),
        id if (ID_PART..ID_PART + 64).contains(&id) && code == CBN_SELCHANGE => {
            UI.with(|u| {
                if let Some(ui) = u.borrow().as_ref() {
                    if id == ID_PART && !crate::mix::conflicts(&current_mix(ui)).is_empty() {
                        if let Some(app) = current_mix(ui).app().map(str::to_owned) {
                            unify(ui, &app);
                        }
                    }
                    check_mix(ui);
                }
            });
            if id == ID_PART {
                refill_presets(None);
            }
            // The main window's details name the mix's app tree.
            unsafe { InvalidateRect(GetWindow(hwnd, GW_OWNER), null(), 0) };
        }
        _ => {}
    }
}

fn draw_button(d: &DRAWITEMSTRUCT) {
    let (dc, r) = (d.hDC, d.rcItem);
    if d.CtlID as usize == ID_DIAG {
        // A check box: a 16 px box (cyan tick when on) and its label.
        fill_rect(dc, r, BACKGROUND);
        let on = DIAG.get();
        let cy = (r.top + r.bottom) / 2;
        let bx = rect(r.left, cy - 8, r.left + 16, cy + 8);
        rounded(dc, bx, if on { CYAN } else { PANEL }, if on { CYAN } else { BORDER }, 4, BACKGROUND);
        fonts(|f| {
            if on {
                text(dc, "\u{2713}", bx, BACKGROUND, f.strong, DT_CENTER | DT_VCENTER);
            }
            text(dc, "Diagnostics (DRED breadcrumbs, separate debug build)", rect(r.left + 26, r.top, r.right, r.bottom), TEXT, f.body, DT_LEFT | DT_VCENTER);
        });
        return;
    }
    let down = d.itemState & ODS_SELECTED != 0;
    let off = d.itemState & ODS_DISABLED != 0;
    let mut buf = vec![0u16; 128];
    let n = unsafe { GetWindowTextW(d.hwndItem, buf.as_mut_ptr(), buf.len() as i32) };
    let label = String::from_utf16_lossy(&buf[..n.max(0) as usize]);
    rounded(dc, r, if down { BUTTON_HOT } else { BUTTON }, BUTTON, 6, BACKGROUND);
    fonts(|f| text(dc, &label, r, if off { FAINT } else { TEXT }, f.body, DT_CENTER | DT_VCENTER));
}

fn draw_combo_item(d: &DRAWITEMSTRUCT) {
    let (dc, r) = (d.hDC, d.rcItem);
    let face = d.itemState & ODS_COMBOBOXEDIT != 0;
    let hot = d.itemState & ODS_SELECTED != 0 && !face;
    fill_rect(dc, r, if hot { ROW_ON } else { PANEL });
    if hot {
        fill_rect(dc, rect(r.left, r.top, r.left + 2, r.bottom), PINK);
    }
    if d.itemID == u32::MAX {
        return;
    }
    let s = combo_text(d.hwndItem, d.itemID as isize);
    let color = if d.itemState & ODS_DISABLED != 0 || s.starts_with('\u{2717}') { FAINT } else { TEXT };
    fonts(|f| text(dc, &s, rect(r.left + 10, r.top, r.right - 4, r.bottom), color, f.body, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS));
}

unsafe extern "system" fn advanced_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => unsafe {
            let mut ps: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut ps);
            let mut c: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut c);
            fill_rect(dc, c, BACKGROUND);
            fonts(|f| {
                text(dc, "Mix", rect(24, 16, 300, 40), TEXT, f.strong, DT_LEFT | DT_VCENTER);
                text(dc, "Each victory crate from the tree you pick. Pick Mix in the runtime list to use it.", rect(24, 40, ADV_WIDTH - 24, 60), SUB, f.small, DT_LEFT | DT_VCENTER | DT_END_ELLIPSIS);
            });
            // 1 px frames round the borderless edits.
            UI.with(|u| {
                if let Some(ui) = u.borrow().as_ref() {
                    for e in [ui.name, ui.options] {
                        let mut r: RECT = std::mem::zeroed();
                        GetWindowRect(e, &mut r);
                        let mut pts = [POINT { x: r.left, y: r.top }, POINT { x: r.right, y: r.bottom }];
                        MapWindowPoints(null_mut(), hwnd, pts.as_mut_ptr(), 2);
                        let frame = rect(pts[0].x - 8, pts[0].y - 6, pts[1].x + 8, pts[1].y + 6);
                        rounded(dc, frame, PANEL, if GetFocus() == e { CYAN } else { BORDER }, 6, BACKGROUND);
                    }
                }
            });
            EndPaint(hwnd, &ps);
            0
        },
        WM_ERASEBKGND => 1,
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX => unsafe {
            let dc = wparam as HDC;
            let id = GetDlgCtrlID(lparam as HWND) as usize;
            let field = msg != WM_CTLCOLORSTATIC || id == ID_NAME || id == ID_OPTIONS;
            SetTextColor(dc, if id == ID_STATUS { PINK } else if field { TEXT } else { SUB });
            SetBkColor(dc, if field { PANEL } else { BACKGROUND });
            (if field { brushes().1 } else { brushes().0 }) as LRESULT
        },
        WM_COMMAND => {
            let (id, code) = (wparam & 0xffff, (wparam >> 16) & 0xffff);
            if (id == ID_NAME || id == ID_OPTIONS) && (code == 0x100 || code == 0x200) {
                // EN_SETFOCUS / EN_KILLFOCUS: repaint the frame's focus colour.
                unsafe { InvalidateRect(hwnd, null(), 0) };
            }
            advanced_command(hwnd, id, code);
            0
        }
        WM_MEASUREITEM => {
            let m = unsafe { &mut *(lparam as *mut MEASUREITEMSTRUCT) };
            if m.CtlType == ODT_COMBOBOX {
                m.itemHeight = 26;
            }
            1
        }
        WM_DRAWITEM => {
            let d = unsafe { &*(lparam as *const DRAWITEMSTRUCT) };
            match d.CtlType {
                ODT_BUTTON => draw_button(d),
                ODT_COMBOBOX => draw_combo_item(d),
                _ => {}
            }
            1
        }
        WM_CLOSE => {
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// The Advanced window, made hidden at start (its options and mix are part of
/// every pick).
fn create_advanced(owner: HWND, memory: &[(String, String)]) {
    let class = wide("NeonbridgeAdvanced");
    let instance = unsafe { GetModuleHandleW(null()) };
    let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
    wc.lpfnWndProc = Some(advanced_proc);
    wc.hInstance = instance;
    wc.lpszClassName = class.as_ptr();
    wc.hCursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    unsafe { RegisterClassW(&wc) };
    let parts_n = 1 + crate::mix::crates().len();
    let grid = 120;
    let rows = parts_n.div_ceil(2) as i32;
    let status_y = grid + rows * 40;
    let options_y = status_y + 40;
    let height = options_y + 24 + 96 + 24 + 36 + 24;
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
    let mut r = rect(0, 0, ADV_WIDTH, height);
    unsafe { AdjustWindowRect(&mut r, style, 0) };
    let mut o: RECT = unsafe { std::mem::zeroed() };
    unsafe { GetWindowRect(owner, &mut o) };
    let title = wide("Advanced");
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, o.left + 120, o.top + 40, r.right - r.left, r.bottom - r.top, owner, null_mut(), instance, null()) };
    if hwnd.is_null() {
        return;
    }
    dark_window(hwnd);
    ADVANCED.set(hwnd);

    let saved = combo(hwnd, (24, 72, 192, 0), ID_SAVED);
    let mut saved_names = vec!["Saved mixes".to_owned(), "last-good".to_owned()];
    saved_names.extend(crate::mix::saved_names());
    fill(saved, &saved_names, 0);
    let name = edit(hwnd, ES_AUTOHSCROLL as u32, (232, 72, 176, 32), ID_NAME);
    let save = button(hwnd, "Save mix", (416, 72, 96, 32), ID_SAVE);
    button(hwnd, "Options list", (520, 72, 96, 32), ID_OPTS_HELP);

    let trees = crate::mix::trees();
    let mut part_names = vec![crate::mix::APP.to_owned()];
    part_names.extend(crate::mix::crates());
    let col = (ADV_WIDTH - 48 - 16) / 2;
    let mut parts = Vec::new();
    for (i, p) in part_names.iter().enumerate() {
        let (x, y) = (24 + (i as i32 % 2) * (col + 16), grid + (i as i32 / 2) * 40);
        let shown = if p == crate::mix::APP { "app" } else { p.trim_start_matches("victory-") };
        label(hwnd, shown, (x, y, 80, 20));
        let list = combo(hwnd, (x + 80, y, col - 80, 0), ID_PART + i);
        let have: Vec<String> = trees.iter().filter(|t| crate::mix::has(t, p)).cloned().collect();
        fill(list, &have.iter().map(|t| short_title(t)).collect::<Vec<_>>(), 0);
        parts.push((p.clone(), list, have));
    }
    let status = child(hwnd, "STATIC", "", 0x4000 /* SS_ENDELLIPSIS */, (24, status_y + 4, ADV_WIDTH - 48, 20), ID_STATUS);
    label(hwnd, "Options for this run: KEY=VALUE, one per line", (24, options_y - 6, 400, 20));
    let options = edit(hwnd, WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32, (24, options_y + 24, ADV_WIDTH - 48, 96), ID_OPTIONS);
    let opts: Vec<String> = memory.iter().filter(|(k, _)| k == "opt").map(|(_, v)| v.clone()).collect();
    let w = wide(&opts.join("\r\n"));
    unsafe { SetWindowTextW(options, w.as_ptr()) };
    button(hwnd, "Done", (ADV_WIDTH - 24 - 96, options_y + 24 + 96 + 24, 96, 36), ID_DONE);
    DIAG.set(memory.iter().any(|(k, v)| k == "diag" && v == "1") || crate::runtimes::diag());
    button(hwnd, "Diagnostics", (24, options_y + 24 + 96 + 24, 420, 36), ID_DIAG);

    let ui = Ui { saved, saved_names, name, save, parts, options, status };
    let mix = crate::mix::Mix { parts: memory.iter().filter_map(|(k, v)| k.strip_prefix("part.").map(|p| (p.to_owned(), v.clone()))).collect() };
    let mix = if mix.parts.is_empty() { crate::mix::load("last-good").unwrap_or_default() } else { mix };
    set_mix(&ui, &mix);
    // A remembered mix that no longer fits starts over from its app tree.
    if !crate::mix::conflicts(&current_mix(&ui)).is_empty() {
        if let Some(app) = current_mix(&ui).app().map(str::to_owned) {
            unify(&ui, &app);
        }
    }
    check_mix(&ui);
    UI.with(|u| *u.borrow_mut() = Some(ui));
}

/// Show the launcher window: the game, the runtime (or a mix of victory
/// trees), its preset and options. None when it was closed without Play.
pub fn pick() -> Option<Pick> {
    unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
    dark_app();
    let memory = remembered();
    let get = |k: &str| memory.iter().find(|(m, _)| m == k).map(|(_, v)| v.clone());
    let rows = rows();
    let want = get("runtime").map(|r| crate::runtimes::alias(&r)).unwrap_or_else(|| crate::runtimes::name(None));
    let sel = rows.iter().position(|r| r.name == want).unwrap_or(0);
    STATE.with(|s| *s.borrow_mut() = Some(State { rows, sel, hover: Hit::None, presets: Vec::new(), preset: 0, game: detect_game(), targets: Vec::new() }));

    let class = wide("NeonbridgePicker");
    let title = wide("Neonbridge");
    let instance = unsafe { GetModuleHandleW(null()) };
    let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
    wc.lpfnWndProc = Some(proc);
    wc.hInstance = instance;
    wc.lpszClassName = class.as_ptr();
    wc.hCursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    unsafe { RegisterClassW(&wc) };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut r = rect(0, 0, WIDTH, HEIGHT);
    unsafe { AdjustWindowRect(&mut r, style, 0) };
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let hwnd = unsafe { CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style, (sw - w) / 2, (sh - h) / 3, w, h, null_mut(), null_mut(), instance, null()) };
    if hwnd.is_null() {
        return None;
    }
    create_advanced(hwnd, &memory);
    refill_presets(get("preset").as_deref());
    mix_mode();
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetFocus(hwnd);
    }
    if std::env::var("NEONBRIDGE_PICKER_ADVANCED").is_ok_and(|v| v.trim() == "1") {
        show_advanced();
    }
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        let adv = ADVANCED.get();
        if adv.is_null() || unsafe { IsDialogMessageW(adv, &msg) } == 0 {
            unsafe {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    PICKED.with(|p| p.borrow_mut().take())
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn folder_exe_and_config_resolve_to_the_package_folder() {
        let dir = std::env::temp_dir().join("neonbridge_picker_resolve");
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("Game.exe"), b"").unwrap();
        std::fs::write(dir.join("MicrosoftGame.Config"), b"").unwrap();
        assert_eq!(resolve(dir.to_str().unwrap()).unwrap(), (dir.clone(), None));
        assert_eq!(resolve(dir.join("Game.exe").to_str().unwrap()).unwrap(), (dir.clone(), Some("Game.exe".into())));
        assert_eq!(resolve(&format!("\"{}\"", dir.join("MicrosoftGame.Config").display())).unwrap(), (dir.clone(), None));
        assert!(resolve("").is_err());
    }
}
