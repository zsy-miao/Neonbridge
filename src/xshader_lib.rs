//! xshader in-process: the shader compiler's cdylib (tuxbox_xshader.dll, its
//! lib.rs `tuxbox_xshader_translate`), built beside xshader.exe from the same
//! tree, is loaded once and called on the worker threads instead of starting
//! one xshader process per program (about 1 s a program per worker: the
//! 383176f prewarm took 97.7 s with 2 of 1338 programs through dxc).
//! The VIctory prewarm translates in-process the same way (apps/victory/src/
//! translate.rs:340-372, victory_shader::emit). A panic in the translator, or
//! no DLL, falls back to the xshader process. NEONBRIDGE_XSHADER_INPROC=0 off.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

type Translate = unsafe extern "C" fn(*const u8, usize, *const u8, usize, *const u8, usize, i64, *mut *mut u8, *mut usize, *mut [u8; 16]) -> u32;
type Free = unsafe extern "C" fn(*mut u8, usize);

/// The DLL's name beside xshader.exe.
pub const DLL: &str = "tuxbox_xshader.dll";

pub fn enabled() -> bool {
    std::env::var("NEONBRIDGE_XSHADER_INPROC").as_deref() != Ok("0")
}

/// The DLL beside `exe` when it came from the same build (written within
/// five minutes of it): xshader-good.exe, kept from an earlier build, has none.
pub fn beside(exe: &Path) -> Option<PathBuf> {
    let dll = exe.with_file_name(DLL);
    let (a, b) = (std::fs::metadata(exe).ok()?.modified().ok()?, std::fs::metadata(&dll).ok()?.modified().ok()?);
    let apart = a.duration_since(b).or_else(|_| b.duration_since(a)).ok()?;
    (apart < Duration::from_secs(300)).then_some(dll)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn load(dll: &Path) -> Option<(Translate, Free)> {
    static LIB: OnceLock<Option<(usize, usize)>> = OnceLock::new();
    let &(t, f) = LIB.get_or_init(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let h = LoadLibraryW(wide(&dll.to_string_lossy()).as_ptr());
        if h.is_null() {
            return None;
        }
        let t = GetProcAddress(h, b"tuxbox_xshader_translate\0".as_ptr())? as usize;
        let f = GetProcAddress(h, b"tuxbox_xshader_free\0".as_ptr())? as usize;
        Some((t, f))
    }).as_ref()?;
    Some(unsafe { (std::mem::transmute::<usize, Translate>(t), std::mem::transmute::<usize, Free>(f)) })
}

pub enum Outcome {
    /// HLSL (or a ray library) and its DXC profile.
    Done(Vec<u8>, String),
    Unsupported(String),
    Failed(String),
    /// No DLL, a panic or past the time: run the xshader process instead.
    Fallback(String),
    /// Detached at the prewarm's tail: the title's request translates it.
    Deferred,
}

/// Translates on a 64 MiB-stack thread (the translator recurses over long
/// programs); past `timeout` the caller falls back and the thread runs on.
/// `cancel` (the prewarm's tail) detaches a translation still running: the
/// caller gets Deferred at once and the thread's result is dropped.
pub fn translate(dll: &Path, source: Vec<u8>, consumer: Option<Vec<u8>>, order: Option<Vec<u8>>, stencil: Option<u32>, timeout: Duration,
    cancel: Option<&std::sync::atomic::AtomicBool>) -> Outcome {
    let Some((translate, free)) = load(dll) else {
        return Outcome::Fallback(format!("{} did not load", dll.display()));
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("xshader".into()).stack_size(64 << 20).spawn(move || {
        let ptr = |v: &Option<Vec<u8>>| v.as_ref().map_or((std::ptr::null(), 0), |v| (v.as_ptr(), v.len()));
        let ((c, cn), (o, on)) = (ptr(&consumer), ptr(&order));
        let (mut out, mut len, mut profile) = (std::ptr::null_mut::<u8>(), 0usize, [0u8; 16]);
        let status = unsafe {
            translate(source.as_ptr(), source.len(), c, cn, o, on, stencil.map_or(-1, i64::from), &mut out, &mut len, &mut profile)
        };
        let data = if out.is_null() { Vec::new() } else { unsafe { std::slice::from_raw_parts(out, len).to_vec() } };
        unsafe { free(out, len) };
        let profile = String::from_utf8_lossy(&profile[..profile.iter().position(|&b| b == 0).unwrap_or(16)]).into_owned();
        let _ = tx.send((status, data, profile));
    });
    if let Err(e) = spawned {
        return Outcome::Fallback(format!("thread: {e}"));
    }
    let start = std::time::Instant::now();
    let got = loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => break Ok(r),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break Err(()),
            Err(_) if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Acquire)) => return Outcome::Deferred,
            Err(_) if start.elapsed() > timeout => break Err(()),
            Err(_) => {}
        }
    };
    match got {
        Ok((0, data, profile)) => Outcome::Done(data, profile),
        Ok((65, data, _)) => Outcome::Unsupported(String::from_utf8_lossy(&data).trim().to_owned()),
        Ok((1, data, _)) => Outcome::Failed(String::from_utf8_lossy(&data).trim().to_owned()),
        Ok((s, data, _)) => Outcome::Fallback(format!("status {s}: {}", String::from_utf8_lossy(&data).trim())),
        Err(_) => Outcome::Fallback("timed out".into()),
    }
}
