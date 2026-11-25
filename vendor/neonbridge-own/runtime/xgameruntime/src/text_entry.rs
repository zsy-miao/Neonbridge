//! Host text-entry dialog for XGameUiShowTextEntryAsync, copied from ztrx
//! 4e17478 crates/victory-hle/src/modules/xgameui.rs:147 (GPL-3, credit to
//! its authors).

pub(crate) use dialog::ask;

mod dialog {
    use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows_sys::Win32::UI::WindowsAndMessaging::{self as wm, DLGTEMPLATE};

    const EDIT: i32 = 100;
    // Edit control messages (the controls headers).
    const EM_SETSEL: u32 = 0xb1;
    const EM_LIMITTEXT: u32 = 0xc5;

    /// The dialog's state, behind its user data while it runs.
    struct Entry {
        max: u32,
        typed: Option<String>,
    }

    /// The description, an edit box with the default text, OK and Cancel,
    /// from an in-memory template (dialog units, the system font); Enter
    /// accepts, Esc cancels.
    pub(crate) fn ask(title: &str, description: &str, default: &str, max: u32) -> Option<String> {
        let mut t: Vec<u16> = Vec::new();
        fn dword(t: &mut Vec<u16>, v: u32) {
            t.extend([v as u16, (v >> 16) as u16]);
        }
        fn text(t: &mut Vec<u16>, s: &str) {
            t.extend(s.encode_utf16().filter(|&c| c != 0));
            t.push(0);
        }
        let style = wm::WS_POPUP | wm::WS_CAPTION | wm::WS_SYSMENU;
        let flags = (wm::DS_MODALFRAME | wm::DS_CENTER | wm::DS_SETFOREGROUND) as u32;
        dword(&mut t, style | flags);
        dword(&mut t, wm::WS_EX_TOPMOST);
        t.extend([4, 0, 0, 240, 62, 0, 0]); // items, x, y, cx, cy, no menu, default class
        text(&mut t, title);
        let mut item = |style: u32, rect: [u16; 4], id: u16, class: u16, caption: &str| {
            if t.len() % 2 == 1 {
                t.push(0); // items start on a DWORD
            }
            dword(&mut t, wm::WS_CHILD | wm::WS_VISIBLE | style);
            dword(&mut t, 0);
            t.extend(rect);
            t.extend([id, 0xffff, class]);
            text(&mut t, caption);
            t.push(0); // no creation data
        };
        let (button, edit, label) = (0x80, 0x81, 0x82);
        let tab = wm::WS_TABSTOP;
        item(0, [7, 7, 226, 10], 0xffff, label, description);
        let edit_style = wm::WS_BORDER | tab | wm::ES_AUTOHSCROLL as u32;
        item(edit_style, [7, 20, 226, 12], EDIT as u16, edit, default);
        let ok = wm::BS_DEFPUSHBUTTON as u32 | tab;
        item(ok, [129, 40, 50, 14], wm::IDOK as u16, button, "OK");
        let cancel = wm::BS_PUSHBUTTON as u32 | tab;
        item(cancel, [183, 40, 50, 14], wm::IDCANCEL as u16, button, "Cancel");
        // The template must start on a DWORD.
        let template: Vec<u32> = t
            .chunks(2)
            .map(|c| u32::from(c[0]) | u32::from(c.get(1).copied().unwrap_or(0)) << 16)
            .collect();
        let mut entry = Entry { max, typed: None };
        // SAFETY: a complete template and our dialog procedure; `entry`
        // outlives the modal dialog.
        unsafe {
            wm::DialogBoxIndirectParamW(
                GetModuleHandleW(std::ptr::null()),
                template.as_ptr().cast::<DLGTEMPLATE>(),
                std::ptr::null_mut(),
                Some(procedure),
                (&raw mut entry) as LPARAM,
            );
        }
        entry.typed
    }

    extern "system" fn procedure(dialog: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> isize {
        // SAFETY: our dialog; its user data is the `Entry` from `ask`,
        // stored at WM_INITDIALOG.
        unsafe {
            let edit = wm::GetDlgItem(dialog, EDIT);
            match message {
                wm::WM_INITDIALOG => {
                    wm::SetWindowLongPtrW(dialog, wm::GWLP_USERDATA, lparam);
                    let entry = &*(lparam as *const Entry);
                    wm::SendMessageW(edit, EM_LIMITTEXT, entry.max as WPARAM, 0);
                    wm::SendMessageW(edit, EM_SETSEL, 0, -1);
                    SetFocus(edit);
                    0 // focus set here
                }
                wm::WM_COMMAND => {
                    let entry = &mut *(wm::GetWindowLongPtrW(dialog, wm::GWLP_USERDATA) as *mut Entry);
                    match (wparam & 0xffff) as i32 {
                        wm::IDOK => {
                            let mut buffer = vec![0u16; wm::GetWindowTextLengthW(edit) as usize + 1];
                            let n = wm::GetWindowTextW(edit, buffer.as_mut_ptr(), buffer.len() as i32);
                            entry.typed = Some(String::from_utf16_lossy(&buffer[..n.max(0) as usize]));
                            wm::EndDialog(dialog, 1);
                        }
                        wm::IDCANCEL => {
                            wm::EndDialog(dialog, 0);
                        }
                        _ => return 0,
                    }
                    1
                }
                _ => 0,
            }
        }
    }
}
