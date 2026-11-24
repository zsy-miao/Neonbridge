//! One-line game picker, ported from OpenScarlett's launcher target row (MIT):
//! one path box that takes the package folder, the exe or MicrosoftGame.Config,
//! with Folder... and Exe... browse buttons and Launch. Shown when the launcher
//! starts without --game; the last pick is remembered. NEONBRIDGE_PICKER=0
//! keeps the old behaviour (print the usage instead).

use std::{
    cell::RefCell,
    fs,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{COLOR_BTNFACE, DEFAULT_GUI_FONT, GetStockObject, HBRUSH},
    System::{
        Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree},
        LibraryLoader::GetModuleHandleW,
    },
    UI::{
        Input::KeyboardAndMouse::SetFocus,
        Controls::Dialogs::{GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY, OFN_PATHMUSTEXIST, OPENFILENAMEW},
        Shell::{BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW, SHBrowseForFolderW, SHGetPathFromIDListW},
        WindowsAndMessaging::*,
    },
};

const ID_PATH: usize = 100;
const ID_FOLDER: usize = 101;
const ID_EXE: usize = 102;
const ID_LAUNCH: usize = 103;

thread_local! {
    static EDIT: RefCell<HWND> = const { RefCell::new(null_mut()) };
    static PICKED: RefCell<Option<(PathBuf, Option<String>)>> = const { RefCell::new(None) };
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn saved() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack/last_game.txt")
}

/// Folder, exe or MicrosoftGame.Config -> (package folder, exe name if one was picked).
fn resolve(text: &str) -> Result<(PathBuf, Option<String>), String> {
    let text = text.trim().trim_matches('"');
    if text.is_empty() {
        return Err("Pick the game folder or its executable.".into());
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

fn edit_text(edit: HWND) -> String {
    let mut buf = vec![0u16; 4096];
    let n = unsafe { GetWindowTextW(edit, buf.as_mut_ptr(), buf.len() as i32) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

fn set_edit(text: &str) {
    let w = wide(text);
    EDIT.with(|e| unsafe { SetWindowTextW(*e.borrow(), w.as_ptr()) });
}

fn browse_folder(owner: HWND) -> Option<String> {
    let title = wide("Select the game folder (holds MicrosoftGame.Config)");
    let mut info: BROWSEINFOW = unsafe { std::mem::zeroed() };
    info.hwndOwner = owner;
    info.lpszTitle = title.as_ptr();
    info.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
    let pidl = unsafe { SHBrowseForFolderW(&info) };
    if pidl.is_null() {
        return None;
    }
    let mut buf = [0u16; 1024];
    let ok = unsafe { SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) };
    unsafe { CoTaskMemFree(pidl as *const _) };
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len]))
}

fn browse_exe(owner: HWND) -> Option<String> {
    let title = wide("Select the Scarlett executable or MicrosoftGame.Config");
    let filter: Vec<u16> = "Executables & Config (*.exe;MicrosoftGame.Config)\0*.exe;MicrosoftGame.Config\0All Files (*.*)\0*.*\0\0"
        .encode_utf16().collect();
    let mut file = vec![0u16; 2048];
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
    ofn.hwndOwner = owner;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let len = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(String::from_utf16_lossy(&file[..len]))
}

fn child(parent: HWND, class: &str, text: &str, style: u32, ex: u32, x: i32, w: i32, id: usize) -> HWND {
    let (class, text) = (wide(class), wide(text));
    let hwnd = unsafe {
        CreateWindowExW(ex, class.as_ptr(), text.as_ptr(), WS_CHILD | WS_VISIBLE | WS_TABSTOP | style,
            x, 10, w, 26, parent, id as _, GetModuleHandleW(null()), null())
    };
    unsafe { SendMessageW(hwnd, WM_SETFONT, GetStockObject(DEFAULT_GUI_FONT) as WPARAM, 1) };
    hwnd
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let edit = child(hwnd, "EDIT", "", ES_AUTOHSCROLL as u32, WS_EX_CLIENTEDGE, 10, 420, ID_PATH);
            EDIT.with(|e| *e.borrow_mut() = edit);
            child(hwnd, "BUTTON", "Folder...", BS_PUSHBUTTON as u32, 0, 438, 74, ID_FOLDER);
            child(hwnd, "BUTTON", "Exe...", BS_PUSHBUTTON as u32, 0, 516, 64, ID_EXE);
            child(hwnd, "BUTTON", "Launch", BS_DEFPUSHBUTTON as u32, 0, 588, 84, ID_LAUNCH);
            if let Ok(last) = fs::read_to_string(saved()) {
                set_edit(last.trim());
            }
            unsafe { SetFocus(edit) };
            0
        }
        WM_COMMAND => {
            match wparam & 0xffff {
                ID_FOLDER => if let Some(p) = browse_folder(hwnd) { set_edit(&p) },
                ID_EXE => if let Some(p) = browse_exe(hwnd) { set_edit(&p) },
                ID_LAUNCH | 1 => {
                    let text = EDIT.with(|e| edit_text(*e.borrow()));
                    match resolve(&text) {
                        Ok(pick) => {
                            let _ = fs::create_dir_all(saved().parent().unwrap_or(Path::new(".")));
                            let _ = fs::write(saved(), text.trim());
                            PICKED.with(|p| *p.borrow_mut() = Some(pick));
                            unsafe { DestroyWindow(hwnd) };
                        }
                        Err(why) => {
                            let (why, title) = (wide(&why), wide("Neonbridge"));
                            unsafe { MessageBoxW(hwnd, why.as_ptr(), title.as_ptr(), MB_OK | MB_ICONWARNING) };
                        }
                    }
                }
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

/// Show the picker; None when the window was closed without launching.
pub fn pick() -> Option<(PathBuf, Option<String>)> {
    unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
    let class = wide("NeonbridgePicker");
    let title = wide("Neonbridge - pick the game");
    let instance = unsafe { GetModuleHandleW(null()) };
    let mut wc: WNDCLASSW = unsafe { std::mem::zeroed() };
    wc.lpfnWndProc = Some(proc);
    wc.hInstance = instance;
    wc.lpszClassName = class.as_ptr();
    wc.hbrBackground = (COLOR_BTNFACE + 1) as usize as HBRUSH;
    wc.hCursor = unsafe { LoadCursorW(null_mut(), IDC_ARROW) };
    unsafe { RegisterClassW(&wc) };
    let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
    let mut rect = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 682, bottom: 46 };
    unsafe { AdjustWindowRect(&mut rect, style, 0) };
    let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
    let (sw, sh) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
    let hwnd = unsafe {
        CreateWindowExW(0, class.as_ptr(), title.as_ptr(), style | WS_VISIBLE,
            (sw - w) / 2, (sh - h) / 2, w, h, null_mut(), null_mut(), instance, null())
    };
    if hwnd.is_null() {
        return None;
    }
    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        if unsafe { IsDialogMessageW(hwnd, &msg) } == 0 {
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
