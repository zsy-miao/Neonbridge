//! Game control channel (TUXBOX_CTL=0 off): `neonbridge ctl <command>` talks to
//! the running game over the local named pipe \\.\pipe\tuxbox-ctl. Remote
//! clients are rejected and nothing listens on the network.
//!
//! One request per connection: a command line in, a text reply out.
//!   status                 fps, frame split, GPU latency, hot threads, audio counters
//!   log [N] [filter...]    the last N (default 40) log lines containing the filter
//!   stack                  every thread's stack now
//!   press BUTTONS [ms]     press pad buttons (A, B, X, Y, Menu, View, Up, Down,
//!                          Left, Right, LB, RB, LS, RS; join with +), default 150 ms
//!   hold BUTTONS / release hold until released
//!   screenshot FILE        the next presented frame as a .bmp
//!   get [NAME] / set NAME VALUE / unset NAME
//!                          process switches (TUXBOX_* environment); switches the
//!                          providers read on every use change at once
//!   stop                   close the game (WM_CLOSE, then exit after 5 s)
use std::io::{Read, Seek, SeekFrom};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WPARAM};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAGS_AND_ATTRIBUTES, FlushFileBuffers, GETFINALPATHNAMEBYHANDLE_FLAGS, GetFinalPathNameByHandleW, ReadFile,
    WriteFile, PIPE_ACCESS_DUPLEX,
};
use windows::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS,
    PIPE_TYPE_BYTE, PIPE_WAIT,
};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
use windows::core::{BOOL, w};

fn enabled() -> bool {
    std::env::var_os("TUXBOX_CTL").is_none_or(|v| v != "0")
}

/// Start the pipe server once (device creation).
pub(crate) fn install() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if !enabled() {
        return;
    }
    ONCE.call_once(|| {
        let _ = std::thread::Builder::new().name("TuxBox ctl".into()).spawn(serve);
    });
}

fn serve() {
    super::log_line(format_args!("ctl: listening on \\\\.\\pipe\\tuxbox-ctl (local only; TUXBOX_CTL=0 turns this off)"));
    loop {
        let pipe = unsafe {
            CreateNamedPipeW(
                w!(r"\\.\pipe\tuxbox-ctl"),
                FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0),
                PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
                1,
                1 << 16,
                1 << 12,
                0,
                None,
            )
        };
        if pipe == INVALID_HANDLE_VALUE {
            super::log_line(format_args!("ctl: pipe could not be created ({}); control channel off", std::io::Error::last_os_error()));
            return;
        }
        let connected = unsafe { ConnectNamedPipe(pipe, None) }.is_ok()
            || std::io::Error::last_os_error().raw_os_error() == Some(535); // ERROR_PIPE_CONNECTED
        if connected {
            let request = read_line(pipe);
            let reply = handle(request.trim());
            let _ = unsafe { WriteFile(pipe, Some(reply.as_bytes()), None, None) };
            let _ = unsafe { FlushFileBuffers(pipe) };
        }
        let _ = unsafe { DisconnectNamedPipe(pipe) };
        let _ = unsafe { CloseHandle(pipe) };
    }
}

fn read_line(pipe: HANDLE) -> String {
    let mut out = Vec::new();
    let mut buffer = [0u8; 512];
    while out.len() < 4096 && !out.contains(&b'\n') {
        let mut n = 0u32;
        if unsafe { ReadFile(pipe, Some(&mut buffer), Some(&mut n), None) }.is_err() || n == 0 {
            break;
        }
        out.extend_from_slice(&buffer[..n as usize]);
    }
    String::from_utf8_lossy(&out).lines().next().unwrap_or("").to_string()
}

/// `where ADDR`: what an address a call or store names is now (palmetto
/// 88307252, re-derived): module+RVA and nearest export, or the memory kind.
fn where_is(arg: Option<&str>) -> String {
    let Some((address, _)) = arg.and_then(super::memwatch::parse_address) else {
        return "usage: where game+0xRVA | 0xVA\n".into();
    };
    let list = super::sampler::modules();
    let export = super::crash_detail::export_near(&list, address).map_or(String::new(), |e| format!("; export {e}"));
    format!("{address:#x} = {}: {}{export}; bytes {}\n", super::memwatch::label(address),
        super::crash_detail::address_kind(&list, address), super::crash_detail::bytes(address, 16))
}

fn handle(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((&command, args)) = words.split_first() else { return help() };
    match command.to_ascii_lowercase().as_str() {
        "status" => status(),
        "where" => where_is(args.first().copied()),
        "log" => log_tail(args),
        "stack" | "stacks" => super::crash::stacks_now().join("\n") + "\n",
        "press" => pad(args.first().copied(), args.get(1).and_then(|v| v.parse().ok()).unwrap_or(150)),
        "hold" => pad(args.first().copied(), u32::MAX),
        "release" => pad(Some("none"), 0),
        "screenshot" => screenshot_request(args.first().copied()),
        "get" => get(args.first().copied()),
        "set" if args.len() >= 2 => {
            unsafe { std::env::set_var(args[0], args[1..].join(" ")) };
            super::log_line(format_args!("ctl: set {}={}", args[0], args[1..].join(" ")));
            format!("{}={} (applies to switches read on every use; ones read at startup keep their value)\n", args[0], args[1..].join(" "))
        }
        "unset" if !args.is_empty() => {
            unsafe { std::env::remove_var(args[0]) };
            format!("{} unset\n", args[0])
        }
        "stop" => stop(),
        _ => help(),
    }
}

fn help() -> String {
    "commands: status | log [N] [filter] | stack | press BUTTONS [ms] | hold BUTTONS | release | screenshot FILE | get [NAME] | set NAME VALUE | unset NAME | stop\n\
     buttons: A B X Y Menu View Up Down Left Right LB RB LS RS (join with +)\n".into()
}

// ---- log access: the launcher points stderr at its log file; read its tail ----

fn log_path() -> Option<std::path::PathBuf> {
    let handle = unsafe { GetStdHandle(STD_ERROR_HANDLE) }.ok()?;
    if handle.is_invalid() {
        return None;
    }
    let mut buffer = [0u16; 1024];
    let n = unsafe { GetFinalPathNameByHandleW(handle, &mut buffer, GETFINALPATHNAMEBYHANDLE_FLAGS(0)) } as usize;
    if n == 0 || n >= buffer.len() {
        return None;
    }
    let path = String::from_utf16_lossy(&buffer[..n]);
    Some(path.trim_start_matches(r"\\?\").into())
}

/// The last `max_bytes` of the log as lines.
fn log_lines(max_bytes: u64) -> Result<Vec<String>, String> {
    let path = log_path().ok_or("stderr is not a log file (start the game with neonbridge)")?;
    let mut file = std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = file.seek(SeekFrom::Start(len.saturating_sub(max_bytes)));
    let mut text = Vec::new();
    let _ = file.read_to_end(&mut text);
    let text = String::from_utf8_lossy(&text);
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if len > max_bytes && !lines.is_empty() {
        lines.remove(0); // partial first line
    }
    Ok(lines)
}

fn log_tail(args: &[&str]) -> String {
    let (count, filter) = match args.first().and_then(|v| v.parse::<usize>().ok()) {
        Some(n) => (n, args[1..].join(" ")),
        None => (40, args.join(" ")),
    };
    match log_lines(8 << 20) {
        Ok(lines) => {
            let picked: Vec<&String> = lines.iter().filter(|l| filter.is_empty() || l.contains(&filter)).collect();
            let start = picked.len().saturating_sub(count);
            picked[start..].iter().map(|l| format!("{l}\n")).collect()
        }
        Err(e) => format!("log: {e}\n"),
    }
}

fn status() -> String {
    let lines = match log_lines(4 << 20) {
        Ok(lines) => lines,
        Err(e) => return format!("status: {e}\n"),
    };
    let last = |key: &str, n: usize| -> Vec<String> {
        let mut found: Vec<String> = lines.iter().rev().filter(|l| l.contains(key)).take(n).cloned().collect();
        found.reverse();
        found
    };
    let mut out = Vec::new();
    for (key, n) in [
        ("completed PresentX", 1), ("frame time", 1), ("hot thread", 2), ("spatial audio last", 1),
        ("TuxBox xapu: last", 1), ("TuxBox chad: last", 1), ("XMA last", 1), ("TuxBox acphal: XMA:", 1),
        ("crash record", 3), ("hang:", 1),
    ] {
        out.extend(last(key, n));
    }
    if out.is_empty() {
        return "status: no heartbeat lines yet\n".into();
    }
    out.iter().map(|l| format!("{l}\n")).collect()
}

// ---- pad ----

fn buttons(text: &str) -> Option<u32> {
    let mut mask = 0;
    for name in text.split('+') {
        mask |= match name.to_ascii_lowercase().as_str() {
            "none" => 0,
            "menu" | "start" => 0x1,
            "view" | "back" | "select" => 0x2,
            "a" => 0x4,
            "b" => 0x8,
            "x" => 0x10,
            "y" => 0x20,
            "up" => 0x40,
            "down" => 0x80,
            "left" => 0x100,
            "right" => 0x200,
            "lb" => 0x400,
            "rb" => 0x800,
            "ls" => 0x1000,
            "rs" => 0x2000,
            _ => return None,
        };
    }
    Some(mask)
}

fn pad(names: Option<&str>, ms: u32) -> String {
    let Some(mask) = names.and_then(buttons) else { return "press: unknown button (A B X Y Menu View Up Down Left Right LB RB LS RS)\n".into() };
    type Inject = unsafe extern "system" fn(u32, u32) -> u32;
    let inject = unsafe {
        let Ok(module) = windows::Win32::System::LibraryLoader::GetModuleHandleW(w!("GameInput.dll")) else {
            return "press: GameInput.dll is not loaded\n".into();
        };
        match windows::Win32::System::LibraryLoader::GetProcAddress(module, windows::core::s!("TuxBoxPadInject")) {
            Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, Inject>(f),
            None => return "press: GameInput.dll has no TuxBoxPadInject (old build)\n".into(),
        }
    };
    let pads = unsafe { inject(mask, ms) };
    super::log_line(format_args!("ctl: pad {mask:#x} for {} ({pads} pad)", if ms == u32::MAX { "hold".into() } else { format!("{ms} ms") }));
    match ms {
        0 => "released\n".into(),
        u32::MAX => format!("holding {mask:#x}\n"),
        _ => format!("pressed {mask:#x} for {ms} ms\n"),
    }
}

// ---- screenshot ----

static SCREENSHOT: Mutex<Option<(String, Option<Result<String, String>>)>> = Mutex::new(None);

fn screenshot_request(file: Option<&str>) -> String {
    let Some(file) = file else { return "screenshot: give a file name\n".into() };
    let path = std::path::absolute(file).map(|p| p.display().to_string()).unwrap_or_else(|_| file.into());
    *SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, None));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
        let mut guard = SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, Some(result))) = guard.as_ref() {
            let reply = match result {
                Ok(text) => format!("{text}\n"),
                Err(e) => format!("screenshot: {e}\n"),
            };
            *guard = None;
            return reply;
        }
    }
    *SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner()) = None;
    "screenshot: no frame was presented within 5 s\n".into()
}

/// The present path samples the next frame when a screenshot is waiting.
pub(crate) fn screenshot_pending() -> bool {
    SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|(_, r)| r.is_none())
}

/// One pixel as 8-bit RGB for the DXGI formats the title presents
/// (16-bit UNORM/UINT read as integers, XR_BIAS as 10:10:10:2 with its bias).
pub(crate) fn rgb(data: &[u8], x: usize, y: usize, pitch: usize, format: i32) -> Option<[u8; 3]> {
    let bytes = match format { 10..=14 => 8, _ => 4 };
    let at = y * pitch + x * bytes;
    let p = data.get(at..at + bytes)?;
    let word = |i: usize| u32::from_le_bytes([p[i], p[i + 1], p[i + 2], p[i + 3]]);
    let half = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
    let unit = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    Some(match format {
        87 | 88 | 90 | 91 | 92 | 93 => [p[2], p[1], p[0]],           // B8G8R8A8 / X8
        24..=26 => {                                                  // R10G10B10A2
            let v = word(0);
            [(v & 0x3ff) >> 2, ((v >> 10) & 0x3ff) >> 2, ((v >> 20) & 0x3ff) >> 2].map(|c| c as u8)
        }
        89 => {                                                       // R10G10B10_XR_BIAS_A2
            let v = word(0);
            [v & 0x3ff, (v >> 10) & 0x3ff, (v >> 20) & 0x3ff].map(|c| unit((c as f32 - 384.0) / 510.0))
        }
        10 => [0, 2, 4].map(|i| unit(half_to_f32(half(i)))),         // R16G16B16A16_FLOAT
        11 | 12 => [0, 2, 4].map(|i| (half(i) >> 8) as u8),          // _UNORM / _UINT as integers
        13 | 14 => [0, 2, 4].map(|i| unit(f32::from(half(i) as i16) / 32767.0)), // _SNORM / _SINT
        _ => [p[0], p[1], p[2]],                                      // R8G8B8A8 and the rest
    })
}

fn half_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = i32::from((h >> 10) & 0x1f);
    let frac = f32::from(h & 0x3ff);
    sign * match exp {
        0 => frac * 2f32.powi(-24),
        31 => if frac == 0.0 { f32::INFINITY } else { f32::NAN },
        _ => (1.0 + frac / 1024.0) * 2f32.powi(exp - 15),
    }
}

/// Write the mapped frame as a top-down 24-bit BMP if a screenshot is waiting.
pub(crate) fn screenshot_frame(data: &[u8], width: usize, height: usize, pitch: usize, format: i32) {
    let path = {
        let guard = SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner());
        match guard.as_ref() {
            Some((path, None)) => path.clone(),
            _ => return,
        }
    };
    let row = (width * 3).next_multiple_of(4);
    let size = 54 + row * height;
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(-(height as i32)).to_le_bytes()); // top-down
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&[0u8; 24]);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b] = rgb(data, x, y, pitch, format).unwrap_or([0, 0, 0]);
            out.extend_from_slice(&[b, g, r]);
        }
        out.resize(out.len() + row - width * 3, 0);
    }
    let result = std::fs::write(&path, &out)
        .map(|()| format!("{path}: {width}x{height}, DXGI format {format}"))
        .map_err(|e| format!("{path}: {e}"));
    super::log_line(format_args!("ctl: screenshot {}", match &result { Ok(t) => t.clone(), Err(e) => e.clone() }));
    *SCREENSHOT.lock().unwrap_or_else(|e| e.into_inner()) = Some((path, Some(result)));
}

// ---- get / stop ----

fn get(name: Option<&str>) -> String {
    match name {
        Some(name) => match std::env::var(name) {
            Ok(v) => format!("{name}={v}\n"),
            Err(_) => format!("{name} is not set\n"),
        },
        None => {
            let mut vars: Vec<String> = std::env::vars()
                .filter(|(k, _)| k.starts_with("TUXBOX_") || k.starts_with("NEONBRIDGE_"))
                .map(|(k, v)| format!("{k}={v}\n"))
                .collect();
            vars.sort();
            if vars.is_empty() { "no TUXBOX_* switches set\n".into() } else { vars.concat() }
        }
    }
}

fn stop() -> String {
    super::log_line(format_args!("ctl: stop requested; closing the game window, exiting in 5 s if it stays open"));
    unsafe extern "system" fn close(window: HWND, pid: LPARAM) -> BOOL {
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(window, Some(&mut owner)) };
        if owner as isize == pid.0 {
            let _ = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) };
        }
        BOOL(1)
    }
    let _ = unsafe { EnumWindows(Some(close), LPARAM(std::process::id() as isize)) };
    let _ = std::thread::Builder::new().name("TuxBox ctl stop".into()).spawn(|| {
        std::thread::sleep(Duration::from_secs(5));
        std::process::exit(0);
    });
    "stopping\n".into()
}
