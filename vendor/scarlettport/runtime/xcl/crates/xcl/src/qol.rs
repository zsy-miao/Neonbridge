//! PC quality-of-life: window-focus behaviour and crash reports.
//!
//! * `XCL_BACKGROUND` = `mute` (default) | `pause` | `run`: what happens when the XCL window
//!   loses focus. `mute` silences the XAudio2 master voice and reports the title "constrained"
//!   (as the Xbox guide does); `pause` additionally stops the audio engines (the title's audio
//!   clock stalls with them); `run` changes nothing. Regaining focus undoes it.
//! * A crash report (`%LOCALAPPDATA%\xcl\crash-reports\<time>.zip`) is written when the process
//!   ends from an unhandled exception, a non-zero exit status or a removed device.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

// ---------------------------------------------------------------------------------------
// Focus behaviour
// ---------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Background {
    Mute,
    Pause,
    Run,
}

impl Background {
    pub fn parse(s: Option<&str>) -> Background {
        match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
            Some("pause") => Background::Pause,
            Some("run") | Some("off") | Some("0") => Background::Run,
            _ => Background::Mute,
        }
    }
}

static MODE: AtomicU8 = AtomicU8::new(u8::MAX);
static FOCUSED: AtomicBool = AtomicBool::new(true);

pub fn background() -> Background {
    match MODE.load(Ordering::Relaxed) {
        0 => Background::Mute,
        1 => Background::Pause,
        2 => Background::Run,
        _ => {
            let m = Background::parse(std::env::var("XCL_BACKGROUND").ok().as_deref());
            MODE.store(m as u8, Ordering::Relaxed);
            m
        }
    }
}

/// What a focus change does: (mute audio, stop engines, constrained flag to report).
pub fn plan(mode: Background, focused: bool) -> (bool, bool, Option<bool>) {
    match mode {
        Background::Run => (false, false, None),
        Background::Mute => (!focused, false, Some(!focused)),
        Background::Pause => (!focused, !focused, Some(!focused)),
    }
}

/// The XCL window gained or lost focus (window thread). Idempotent.
pub fn focus_changed(focused: bool) {
    if FOCUSED.swap(focused, Ordering::AcqRel) == focused {
        return;
    }
    let (mute, stop, constrained) = plan(background(), focused);
    crate::xlog_info!("focus {}: background={:?} mute={mute} stop_audio={stop}", if focused { "regained" } else { "lost" }, background());
    crate::hle::xaudio2::set_background(mute, stop);
    if let Some(c) = constrained {
        crate::hle::kernelx::notify_constrained(c);
    }
}

// ---------------------------------------------------------------------------------------
// Crash report
// ---------------------------------------------------------------------------------------

/// Replace `<drive>:\Users\<name>` (either slash) and the literal profile dir by `%USERPROFILE%`.
pub fn scrub_paths(text: &str, profile: Option<&str>) -> String {
    let mut s = text.to_string();
    if let Some(p) = profile.filter(|p| p.len() > 3) {
        for variant in [p.to_string(), p.replace('\\', "/"), p.replace('\\', "\\\\")] {
            s = replace_ci(&s, &variant, "%USERPROFILE%");
        }
    }
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        // `X:` + sep + `Users` + sep + name
        if i + 2 < b.len() && b[i].is_ascii_alphabetic() && b[i + 1] == b':' && is_sep(b[i + 2]) && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            // `\\` (escaped) separators may double up
            let mut k = i + 2;
            while k < b.len() && is_sep(b[k]) {
                k += 1;
            }
            if !(k + 5 < b.len() && s.is_char_boundary(k + 5) && s[k..k + 5].eq_ignore_ascii_case("users") && is_sep(b[k + 5])) {
                out.push(b[i] as char);
                i += 1;
                continue;
            }
            let mut j = k + 5;
            while j < b.len() && is_sep(b[j]) {
                j += 1;
            }
            let mut e = j;
            while e < b.len() && !is_sep(b[e]) && !matches!(b[e], b'"' | b'\'' | b'<' | b'>' | b'|' | b'*' | b'?' | b' ' | b'\r' | b'\n') {
                e += 1;
            }
            if e > j {
                out.push_str("%USERPROFILE%");
                i = e;
                continue;
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn is_sep(c: u8) -> bool {
    c == b'\\' || c == b'/'
}

fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return hay.to_string();
    }
    let (lh, ln) = (hay.to_ascii_lowercase(), needle.to_ascii_lowercase());
    let mut out = String::new();
    let mut at = 0;
    while let Some(p) = lh[at..].find(&ln) {
        out.push_str(&hay[at..at + p]);
        out.push_str(with);
        at += p + needle.len();
    }
    out.push_str(&hay[at..]);
    out
}

/// The `XCL_*` variables in `vars`, sorted, values scrubbed.
pub fn collect_env<I: IntoIterator<Item = (String, String)>>(vars: I, profile: Option<&str>) -> Vec<(String, String)> {
    let mut v: Vec<_> = vars
        .into_iter()
        .filter(|(k, _)| k.to_ascii_uppercase().starts_with("XCL_"))
        .map(|(k, val)| (k, scrub_paths(&val, profile)))
        .collect();
    v.sort();
    v
}

/// The last `n` lines of `text`.
pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `YYYYMMDD-HHMMSS` (UTC) from unix seconds.
pub fn stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xEDB8_8320 & (!(crc & 1)).wrapping_add(1));
        }
    }
    !crc
}

/// A zip archive (deflate) of `files`.
pub fn make_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in files {
        let comp = miniz_oxide::deflate::compress_to_vec(data, 6);
        let (crc, off) = (crc32(data), out.len() as u32);
        let mut hdr = |v: &mut Vec<u8>, central: bool| {
            v.extend_from_slice(if central { &[0x50, 0x4b, 1, 2] } else { &[0x50, 0x4b, 3, 4] });
            if central {
                v.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            v.extend_from_slice(&20u16.to_le_bytes()); // needed
            v.extend_from_slice(&0x0800u16.to_le_bytes()); // utf-8 names
            v.extend_from_slice(&8u16.to_le_bytes()); // deflate
            v.extend_from_slice(&0u16.to_le_bytes()); // time
            v.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
            v.extend_from_slice(&crc.to_le_bytes());
            v.extend_from_slice(&(comp.len() as u32).to_le_bytes());
            v.extend_from_slice(&(data.len() as u32).to_le_bytes());
            v.extend_from_slice(&(name.len() as u16).to_le_bytes());
            v.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central {
                v.extend_from_slice(&[0; 8]); // comment, disk, internal attrs
                v.extend_from_slice(&0u32.to_le_bytes()); // external attrs
                v.extend_from_slice(&off.to_le_bytes());
            }
            v.extend_from_slice(name.as_bytes());
        };
        hdr(&mut out, false);
        out.extend_from_slice(&comp);
        hdr(&mut central, true);
    }
    let (cd_off, cd_len) = (out.len() as u32, central.len() as u32);
    out.extend_from_slice(&central);
    out.extend_from_slice(&[0x50, 0x4b, 5, 6, 0, 0, 0, 0]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&cd_len.to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

fn reg_string(sub: &str, name: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
    let (sub, name): (Vec<u16>, Vec<u16>) = (sub.encode_utf16().chain([0]).collect(), name.encode_utf16().chain([0]).collect());
    let mut buf = [0u16; 256];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let r = unsafe { RegGetValueW(HKEY_LOCAL_MACHINE, sub.as_ptr(), name.as_ptr(), RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut _, &mut len) };
    (r == 0).then(|| String::from_utf16_lossy(&buf[..(len as usize / 2).saturating_sub(1)]))
}

fn os_build() -> String {
    let k = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    format!(
        "{} build {}.{}",
        reg_string(k, "ProductName").unwrap_or_else(|| "Windows".into()),
        reg_string(k, "CurrentBuildNumber").unwrap_or_else(|| "?".into()),
        reg_string(k, "UBR").unwrap_or_else(|| "?".into())
    )
}

/// Adapter name + user-mode driver version (DXGI).
fn gpu_info() -> String {
    use windows::core::Interface;
    use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIDevice, IDXGIFactory1};
    let mut s = String::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return "DXGI unavailable".into() };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            let name = String::from_utf16_lossy(&d.Description).trim_end_matches('\0').to_string();
            let drv = a
                .CheckInterfaceSupport(&IDXGIDevice::IID)
                .map(|v| format!("{}.{}.{}.{}", (v >> 48) & 0xFFFF, (v >> 32) & 0xFFFF, (v >> 16) & 0xFFFF, v & 0xFFFF))
                .unwrap_or_else(|_| "?".into());
            s.push_str(&format!("{name} | vendor {:#06x} device {:#06x} | driver {drv} | vram {} MiB\n", d.VendorId, d.DeviceId, d.DedicatedVideoMemory >> 20));
        }
    }
    if s.is_empty() { "no DXGI adapters".into() } else { s }
}

pub fn build_id() -> String {
    format!("xcl {} git {} translator {}", env!("CARGO_PKG_VERSION"), env!("XCL_GIT_HASH"), env!("XCL_TRANSLATOR_SOURCE_ID"))
}

static WRITTEN: AtomicBool = AtomicBool::new(false);

/// Write the crash report (once per process); `reason` goes into the report. Never panics.
pub fn write_crash_report(reason: &str) -> Option<std::path::PathBuf> {
    if WRITTEN.swap(true, Ordering::AcqRel) {
        return None;
    }
    std::panic::catch_unwind(|| write_crash_report_inner(reason)).ok().flatten()
}

fn write_crash_report_inner(reason: &str) -> Option<std::path::PathBuf> {
    crate::log::flush();
    let profile = std::env::var("USERPROFILE").ok();
    let p = profile.as_deref();
    let boot = crate::state::boot();
    let log = boot
        .and_then(|b| {
            let mut f = std::fs::File::open(b.logs_dir().join("xcl.log")).ok()?;
            // the tail is enough: read at most the last 16 MiB
            use std::io::{Read, Seek, SeekFrom};
            let len = f.metadata().ok()?.len();
            f.seek(SeekFrom::Start(len.saturating_sub(16 << 20))).ok()?;
            let mut v = Vec::new();
            f.read_to_end(&mut v).ok()?;
            Some(String::from_utf8_lossy(&v).into_owned())
        })
        .unwrap_or_default();
    let log = scrub_paths(&tail_lines(&log, 5000), p);
    let env: String = collect_env(std::env::vars(), p).iter().map(|(k, v)| format!("{k}={v}\n")).collect();
    let system = format!("reason: {}\nbuild: {}\nos: {}\ngpu:\n{}", scrub_paths(reason, p), build_id(), os_build(), gpu_info());
    let zip = make_zip(&[("system.txt", system.as_bytes()), ("env.txt", env.as_bytes()), ("log-tail.txt", log.as_bytes())]);
    let dir = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("xcl").join("crash-reports");
    std::fs::create_dir_all(&dir).ok()?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    let path = dir.join(format!("{}.zip", stamp(secs)));
    std::fs::File::create(&path).ok()?.write_all(&zip).ok()?;
    crate::xlog_error!("crash report written: {}", scrub_paths(&path.to_string_lossy(), p));
    crate::log::flush();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_parse() {
        assert_eq!(Background::parse(None), Background::Mute);
        assert_eq!(Background::parse(Some("PAUSE")), Background::Pause);
        assert_eq!(Background::parse(Some("run")), Background::Run);
        assert_eq!(Background::parse(Some("bogus")), Background::Mute);
    }

    #[test]
    fn focus_plan() {
        assert_eq!(plan(Background::Mute, false), (true, false, Some(true)));
        assert_eq!(plan(Background::Mute, true), (false, false, Some(false)));
        assert_eq!(plan(Background::Pause, false), (true, true, Some(true)));
        assert_eq!(plan(Background::Run, false), (false, false, None));
    }

    #[test]
    fn scrubs_user_paths() {
        let s = scrub_paths(r"open C:\Users\Marwan\AppData\Local\xcl\a.log and d:/users/bob/x", None);
        assert_eq!(s, r"open %USERPROFILE%\AppData\Local\xcl\a.log and %USERPROFILE%/x");
        assert_eq!(scrub_paths(r"E:\Profiles\zed\f", Some(r"E:\Profiles\zed")), r"%USERPROFILE%\f");
        assert_eq!(scrub_paths(r"C:\\Users\\Al\\x", None), r"%USERPROFILE%\\x");
        assert_eq!(scrub_paths(r"C:\Windows\System32", None), r"C:\Windows\System32");
        assert!(!scrub_paths("C:\\Users\\Ünï\\a", None).contains("Ünï"));
    }

    #[test]
    fn env_collection() {
        let vars = vec![
            ("PATH".to_string(), "x".to_string()),
            ("XCL_LOG".to_string(), r"C:\Users\me\l.txt".to_string()),
            ("xcl_background".to_string(), "pause".to_string()),
            ("XCLX".to_string(), "no".to_string()),
        ];
        let e = collect_env(vars, None);
        assert_eq!(e, vec![("XCL_LOG".to_string(), r"%USERPROFILE%\l.txt".to_string()), ("xcl_background".to_string(), "pause".to_string())]);
    }

    #[test]
    fn tail_and_stamp() {
        assert_eq!(tail_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(tail_lines("a", 5), "a");
        assert_eq!(stamp(0), "19700101-000000");
        assert_eq!(stamp(1_700_000_000), "20231114-221320");
    }

    #[test]
    fn zip_roundtrip_shape() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let z = make_zip(&[("a.txt", b"hello hello hello"), ("b.txt", b"")]);
        assert_eq!(&z[..4], &[0x50, 0x4b, 3, 4]);
        assert_eq!(&z[z.len() - 22..z.len() - 18], &[0x50, 0x4b, 5, 6]);
        assert_eq!(u16::from_le_bytes([z[z.len() - 12], z[z.len() - 11]]), 2);
    }
}
