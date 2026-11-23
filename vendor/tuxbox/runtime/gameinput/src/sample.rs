//! Host devices sampled by the Xbox GameInput v0 provider.
use windows_sys::Win32::Foundation::POINT;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, MAPVK_VK_TO_VSC_EX, MapVirtualKeyW,
};
use windows_sys::Win32::UI::Input::XboxController::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowThreadProcessId, SetCursorPos,
};

pub const KEYS: u32 = 0x10;
pub const MOUSE: u32 = 0x20;
pub const PAD: u32 = 0x40000;
pub const PORTS: usize = 4;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Key {
    scan: u32,
    codepoint: u32,
    virtual_key: u8,
    dead: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Mouse {
    pub buttons: u32,
    pub x: i64,
    pub y: i64,
    pub wheel_x: i64,
    pub wheel_y: i64,
}

#[repr(C)]
#[derive(Clone, Copy, Default, PartialEq)]
pub struct Pad {
    buttons: u32,
    lt: f32,
    rt: f32,
    lx: f32,
    ly: f32,
    rx: f32,
    ry: f32,
}

#[derive(Clone, PartialEq)]
pub enum Sample {
    Keys([u8; 32]),
    Mouse(Mouse),
    Pad(Pad),
}

pub fn foreground() -> bool {
    let mut pid = 0;
    // SAFETY: The OS validates the window and writes a DWORD to pid.
    unsafe {
        GetWindowThreadProcessId(GetForegroundWindow(), &mut pid);
        pid == GetCurrentProcessId()
    }
}

fn down(vk: i32) -> bool {
    // SAFETY: GetAsyncKeyState accepts a virtual key number.
    unsafe { GetAsyncKeyState(vk) < 0 }
}

pub fn keyboard() -> Sample {
    let mut keys = [0u8; 32];
    for vk in 8..=254 {
        // Generic modifiers duplicate their left/right key readings.
        if !(0x10..=0x12).contains(&vk) && down(vk) {
            keys[vk as usize / 8] |= 1 << (vk % 8);
        }
    }
    Sample::Keys(keys)
}

pub fn key(vk: u8) -> Key {
    Key {
        // SAFETY: Convert the virtual key according to the current keyboard layout.
        scan: unsafe { MapVirtualKeyW(vk.into(), MAPVK_VK_TO_VSC_EX) },
        codepoint: 0,
        virtual_key: vk,
        dead: false,
    }
}

#[derive(Default)]
pub struct MouseMotion {
    pub previous: Option<(i32, i32)>,
    state: Mouse,
}

impl MouseMotion {
    pub fn sample(&mut self) -> Sample {
        let mut pos = POINT::default();
        // SAFETY: pos is a writable POINT.
        if unsafe { GetCursorPos(&mut pos) } != 0 {
            if let Some((x, y)) = self.previous {
                self.state.x += i64::from(pos.x) - i64::from(x);
                self.state.y += i64::from(pos.y) - i64::from(y);
            }
            self.previous = Some((pos.x, pos.y));
        }
        self.state.buttons = 0;
        for (bit, vk) in [1, 2, 4, 5, 6].into_iter().enumerate() {
            self.state.buttons |= u32::from(down(vk)) << bit;
        }
        Sample::Mouse(self.state)
    }
}

pub fn gamepad(port: usize) -> Option<Pad> {
    let mut state = XINPUT_STATE::default();
    // SAFETY: The scanner passes a port from 0 through 3 and writable state.
    if unsafe { XInputGetState(port as u32, &mut state) } != 0 {
        return None;
    }
    let pad = state.Gamepad;
    let mut buttons = 0;
    for (index, mask) in [
        XINPUT_GAMEPAD_START,
        XINPUT_GAMEPAD_BACK,
        XINPUT_GAMEPAD_A,
        XINPUT_GAMEPAD_B,
        XINPUT_GAMEPAD_X,
        XINPUT_GAMEPAD_Y,
        XINPUT_GAMEPAD_DPAD_UP,
        XINPUT_GAMEPAD_DPAD_DOWN,
        XINPUT_GAMEPAD_DPAD_LEFT,
        XINPUT_GAMEPAD_DPAD_RIGHT,
        XINPUT_GAMEPAD_LEFT_SHOULDER,
        XINPUT_GAMEPAD_RIGHT_SHOULDER,
        XINPUT_GAMEPAD_LEFT_THUMB,
        XINPUT_GAMEPAD_RIGHT_THUMB,
    ]
    .into_iter()
    .enumerate()
    {
        buttons |= u32::from(pad.wButtons & mask != 0) << index;
    }
    let axis = |v: i16| f32::from(v) / if v < 0 { 32768.0 } else { 32767.0 };
    Some(Pad {
        buttons,
        lt: f32::from(pad.bLeftTrigger) / 255.0,
        rt: f32::from(pad.bRightTrigger) / 255.0,
        lx: axis(pad.sThumbLX),
        ly: axis(pad.sThumbLY),
        rx: axis(pad.sThumbRX),
        ry: axis(pad.sThumbRY),
    })
}

#[derive(Default)]
pub struct DesktopPad {
    // The pointer stays at this position while right-button mouse look is active.
    anchor: Option<POINT>,
}

impl DesktopPad {
    pub fn abandon_aim(&mut self) {
        self.anchor = None;
    }

    pub fn merge(&mut self, mut pad: Pad) -> Pad {
        let held = |key: u8| down(i32::from(key));
        for (bit, key) in b"MNZXCVIKJLQE".iter().enumerate() {
            pad.buttons |= u32::from(held(*key)) << bit;
        }
        let aim = down(2);
        pad.buttons |= u32::from(down(13) || down(1) && !aim) << 2;
        pad.buttons |= u32::from(down(27)) << 3;
        if held(b'R') || aim {
            pad.lt = 1.0;
        }
        if held(b'F') || aim && down(1) {
            pad.rt = 1.0;
        }

        let axis = |positive: u8, negative: u8, hardware: f32| {
            (hardware + f32::from(held(positive)) - f32::from(held(negative))).clamp(-1.0, 1.0)
        };
        pad.lx = axis(b'D', b'A', pad.lx);
        pad.ly = axis(b'W', b'S', pad.ly);
        pad.rx = axis(0x27, 0x25, pad.rx);
        pad.ry = axis(0x26, 0x28, pad.ry);

        if aim {
            let mut cursor = POINT::default();
            // SAFETY: Win32 writes a POINT to our stack and accepts signed screen coordinates.
            if unsafe { GetCursorPos(&mut cursor) } != 0 {
                let anchor = self.anchor.get_or_insert(cursor);
                if cursor.x != anchor.x || cursor.y != anchor.y {
                    pad.rx = (pad.rx + (cursor.x - anchor.x) as f32 / 40.0).clamp(-1.0, 1.0);
                    pad.ry = (pad.ry + (anchor.y - cursor.y) as f32 / 40.0).clamp(-1.0, 1.0);
                    unsafe { SetCursorPos(anchor.x, anchor.y) };
                }
            }
        } else if let Some(anchor) = self.anchor.take() {
            unsafe { SetCursorPos(anchor.x, anchor.y) };
        }
        pad
    }
}
