//! Auto-press A: the runtimes wait on the title's start screens ("press A") and
//! read the keyboard only while their window has focus (GetAsyncKeyState; Enter
//! is A in every runtime's keyboard map). So unattended runs don't stall there,
//! the launcher focuses the game's window and holds Enter every few seconds
//! for the whole run. Presses alternate between a tap (start screens, prompts)
//! and a long hold (cutscenes skip on a held A).
//!
//! NEONBRIDGE_AUTO_A=0 turns it off; NEONBRIDGE_AUTO_A_EVERY=<s> (default 8),
//! NEONBRIDGE_AUTO_A_FOR=<s> (default 0 = the whole run), NEONBRIDGE_AUTO_A_HOLD_MS=<ms>
//! (default 1500: long enough to be seen by a title running at 1 fps),
//! NEONBRIDGE_AUTO_A_LONG_MS=<ms> (default 6000: the cutscene-skip hold).

use std::time::{Duration, Instant};

fn num(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

pub fn start(pid: u32) {
    if std::env::var("NEONBRIDGE_AUTO_A").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let every = Duration::from_secs(num("NEONBRIDGE_AUTO_A_EVERY", 8).max(2));
    let secs = num("NEONBRIDGE_AUTO_A_FOR", 0);
    let until = (secs > 0).then(|| Instant::now() + Duration::from_secs(secs));
    let hold = Duration::from_millis(num("NEONBRIDGE_AUTO_A_HOLD_MS", 1500));
    let long = Duration::from_millis(num("NEONBRIDGE_AUTO_A_LONG_MS", 6000));
    println!("[launch] auto-press A: Enter every {}s, tap then {} ms hold for cutscenes (NEONBRIDGE_AUTO_A=0 turns it off)", every.as_secs(), long.as_millis());
    std::thread::spawn(move || {
        let mut n = 0u64;
        while until.is_none_or(|u| Instant::now() < u) {
            std::thread::sleep(every);
            let Some(h) = window(pid) else { continue };
            n += 1;
            press(h, if n % 2 == 0 { long } else { hold });
        }
    });
}

fn window(pid: u32) -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    struct Ctx { pid: u32, best: HWND, area: i64 }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> windows_sys::core::BOOL {
        let c = unsafe { &mut *(l as *mut Ctx) };
        let mut p = 0u32;
        unsafe { GetWindowThreadProcessId(h, &mut p) };
        if p == c.pid && unsafe { IsWindowVisible(h) } != 0 {
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetClientRect(h, &mut r) };
            let area = i64::from(r.right - r.left) * i64::from(r.bottom - r.top);
            if area > c.area && r.right - r.left >= 320 {
                c.area = area;
                c.best = h;
            }
        }
        1
    }
    let mut c = Ctx { pid, best: std::ptr::null_mut(), area: 0 };
    unsafe { EnumWindows(Some(each), &mut c as *mut Ctx as LPARAM) };
    (!c.best.is_null()).then_some(c.best)
}

fn key(vk: u16, up: bool) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::*;
    let mut i: INPUT = unsafe { std::mem::zeroed() };
    i.r#type = INPUT_KEYBOARD;
    i.Anonymous.ki = KEYBDINPUT { wVk: vk, wScan: 0, dwFlags: if up { KEYEVENTF_KEYUP } else { 0 }, time: 0, dwExtraInfo: 0 };
    unsafe { SendInput(1, &i, size_of::<INPUT>() as i32) };
}

fn press(h: windows_sys::Win32::Foundation::HWND, hold: Duration) {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{VK_MENU, VK_RETURN};
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    unsafe {
        if GetForegroundWindow() != h {
            if IsIconic(h) != 0 {
                ShowWindow(h, SW_RESTORE);
            }
            // A tapped Alt lets a background process take the foreground.
            key(VK_MENU, false);
            key(VK_MENU, true);
            SetForegroundWindow(h);
            std::thread::sleep(Duration::from_millis(200));
            if GetForegroundWindow() != h {
                return;
            }
        }
    }
    key(VK_RETURN, false);
    std::thread::sleep(hold);
    key(VK_RETURN, true);
}
