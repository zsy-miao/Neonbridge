//! Windows' timeout detection and recovery (TDR) for the host GPU, logged
//! once when the adapter is chosen (VIctory 42961e8, GPL-3.0, credit to its
//! authors).
//!
//! With TdrLevel 0 a GPU that stops answering is never reset: a hung or very
//! slow frame freezes the whole desktop, and the run never sees a device
//! removal it could report. With 1 the timeout is a blue screen. Both are
//! set by hand (often to keep slow frames from being reset), and a report of
//! "the PC froze" does not say so. Raising TdrDelay is what gives slow
//! frames room.

/// Where Windows keeps the values.
const KEY: &str = r"SYSTEM\CurrentControlSet\Control\GraphicsDrivers";

#[link(name = "advapi32")]
unsafe extern "system" {
    fn RegGetValueW(key: isize, sub_key: *const u16, value: *const u16, flags: u32, kind: *mut u32, data: *mut core::ffi::c_void, size: *mut u32) -> u32;
}

/// A DWORD under [`KEY`]; None when it is not set (Windows' default).
fn dword(name: &str) -> Option<u32> {
    const HKEY_LOCAL_MACHINE: isize = 0x8000_0002u32 as i32 as isize;
    const RRF_RT_REG_DWORD: u32 = 0x10;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (key, name) = (wide(KEY), wide(name));
    let (mut value, mut size) = (0u32, 4u32);
    // SAFETY: NUL-terminated strings that outlive the call; room for a DWORD.
    let status = unsafe {
        RegGetValueW(HKEY_LOCAL_MACHINE, key.as_ptr(), name.as_ptr(), RRF_RT_REG_DWORD, core::ptr::null_mut(),
            (&raw mut value).cast(), &mut size)
    };
    (status == 0).then_some(value)
}

/// Logs TdrLevel, TdrDelay and TdrDdiDelay, and warns when a GPU hang would
/// freeze the PC or stop it with a blue screen.
pub(crate) fn log() {
    let (level, delay, ddi) = (dword("TdrLevel"), dword("TdrDelay"), dword("TdrDdiDelay"));
    let shown = |v: Option<u32>| v.map_or_else(|| "default".to_owned(), |v| v.to_string());
    super::log_line(format_args!("GPU timeout detection: TdrLevel {}, TdrDelay {} s, TdrDdiDelay {} s",
        shown(level), shown(delay), shown(ddi)));
    let warning = match level {
        Some(0) => Some("TdrLevel is 0: Windows never resets a hung GPU, so a GPU hang freezes the whole PC instead of \
            ending the run with a device removal; delete TdrLevel (or set it to 3) and raise TdrDelay for slow frames"),
        Some(1) => Some("TdrLevel is 1: a GPU timeout is a blue screen instead of a device removal; delete TdrLevel \
            (or set it to 3) and raise TdrDelay for slow frames"),
        _ => None,
    };
    if let Some(warning) = warning {
        super::log_line(format_args!("WARNING: {warning}"));
    }
}
