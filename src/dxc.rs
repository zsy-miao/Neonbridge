//! In-process HLSL -> DXIL with the SDK's dxcompiler.dll, in place of one
//! dxc.exe process per program. Ported from xcl gpu/recomp/dxc.rs:326-375 and
//! :467-501 (ScarlettPort runtime/xcl, GPL-3.0), with credit: dxil.dll is
//! loaded before dxcompiler.dll so the DXIL is signed, and each compile thread
//! makes its own IDxcCompiler3 (the DLLs load once per process).
//!
//! A compile runs on its own thread with a 64 MiB stack (xcl DXC_STACK: LLVM
//! recurses along long dependency chains). The caller waits up to its timeout
//! and is told "timed out" past it, as with the dxc.exe path; an in-process
//! compile cannot be killed, so that thread finishes and its result is dropped.

use std::ffi::c_void;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::{GUID, HRESULT, Interface, PCWSTR};
use windows::Win32::Graphics::Direct3D::Dxc::{CLSID_DxcCompiler, DXC_CP_UTF8, DxcBuffer, IDxcBlob, IDxcCompiler3, IDxcResult};

type CreateInstance = unsafe extern "system" fn(*const GUID, *const GUID, *mut *mut c_void) -> HRESULT;

const DXC_STACK: usize = 64 << 20;

/// Compile threads whose caller gave up on them (past its time): they run on
/// to the end and hold their memory, so the prewarm counts them as jobs.
static ABANDONED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn abandoned() -> usize {
    ABANDONED.load(std::sync::atomic::Ordering::Acquire)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// DxcCreateInstance of the dxcompiler.dll beside `dxc_exe`, loaded once.
fn create_fn(dxc_exe: &Path) -> Result<CreateInstance, String> {
    static LIB: OnceLock<Result<usize, String>> = OnceLock::new();
    let f = LIB.get_or_init(|| unsafe {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        let dir = dxc_exe.parent().ok_or("dxc has no folder")?;
        let (signer, compiler) = (dir.join("dxil.dll"), dir.join("dxcompiler.dll"));
        if LoadLibraryW(wide(&signer.to_string_lossy()).as_ptr()).is_null() {
            return Err(format!("no {}", signer.display()));
        }
        let h = LoadLibraryW(wide(&compiler.to_string_lossy()).as_ptr());
        if h.is_null() {
            return Err(format!("no {}", compiler.display()));
        }
        GetProcAddress(h, b"DxcCreateInstance\0".as_ptr()).map(|f| f as usize)
            .ok_or_else(|| "dxcompiler.dll has no DxcCreateInstance".to_owned())
    });
    f.clone().map(|f| unsafe { std::mem::transmute::<usize, CreateInstance>(f) })
}

fn compiler(create: CreateInstance) -> Result<IDxcCompiler3, String> {
    let mut p = std::ptr::null_mut();
    let hr = unsafe { create(&CLSID_DxcCompiler, &IDxcCompiler3::IID, &mut p) };
    if hr.is_err() || p.is_null() {
        return Err(format!("DxcCreateInstance: {hr:?}"));
    }
    Ok(unsafe { IDxcCompiler3::from_raw(p) })
}

/// A failed compile: dxc's own error text, or a failure to run it.
pub enum Error {
    Compiler(String),
    Other(String),
}

fn compile_here(create: CreateInstance, src: &[u8], args: &[String]) -> Result<Vec<u8>, Error> {
    let c = compiler(create).map_err(Error::Other)?;
    let args: Vec<Vec<u16>> = args.iter().map(|a| wide(a)).collect();
    let ptrs: Vec<PCWSTR> = args.iter().map(|a| PCWSTR(a.as_ptr())).collect();
    let buf = DxcBuffer { Ptr: src.as_ptr() as *const c_void, Size: src.len(), Encoding: DXC_CP_UTF8.0 };
    unsafe {
        let r: IDxcResult = c.Compile(&buf, Some(&ptrs), None).map_err(|e| Error::Other(format!("Compile: {e}")))?;
        let status = r.GetStatus().map_err(|e| Error::Other(e.to_string()))?;
        if status.is_err() {
            let msg = r.GetErrorBuffer().ok().map(|b| {
                let s = std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize());
                String::from_utf8_lossy(s).trim_end_matches('\0').to_string()
            }).unwrap_or_default();
            return Err(Error::Compiler(msg));
        }
        let blob: IDxcBlob = r.GetResult().map_err(|e| Error::Other(e.to_string()))?;
        Ok(std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize()).to_vec())
    }
}

/// Whether compiles go in-process: opt-in, NEONBRIDGE_DXC_INPROC=1. The
/// 0577d56 prewarm on 79e07ee stalled at 216/1338 (0.0/s): no dxc.exe left,
/// ~10 in-process -O0 compiles each over a minute of CPU (dxc.exe -O0 took
/// 0.5-8 s), and the -O1 dxc.exe beside them timed out under that load and
/// fed more -O0 in. Until the in-process slowdown is understood, dxc.exe.
pub fn enabled() -> bool {
    std::env::var("NEONBRIDGE_DXC_INPROC").as_deref() == Ok("1")
}

/// Compiles `src` with dxc command-line `args` (no file names) on a compile
/// thread. Err(Other("timed out")) past `timeout`, or once `give_up` (polled
/// every 100 ms) says so.
/// `low` runs the compile thread at the lowest priority (the -O1 upgrades of dxil_upgrade.rs).
pub fn compile(dxc_exe: &Path, src: Vec<u8>, args: Vec<String>, timeout: Duration, give_up: &dyn Fn() -> bool, low: bool) -> Result<Vec<u8>, Error> {
    let create = create_fn(dxc_exe).map_err(Error::Other)?;
    use std::sync::atomic::{AtomicBool, Ordering};
    let (tx, rx) = std::sync::mpsc::channel();
    let gone = std::sync::Arc::new(AtomicBool::new(false));
    let gone_thread = gone.clone();
    std::thread::Builder::new().name("neonbridge-dxc".into()).stack_size(DXC_STACK).spawn(move || {
        if low {
            use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_LOWEST};
            unsafe { SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_LOWEST) };
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile_here(create, &src, &args)))
            .unwrap_or_else(|_| Err(Error::Other("panic in the compile thread".into())));
        // An abandoned compile is counted until here (swap: once).
        let _ = tx.send(r);
        if gone_thread.swap(false, Ordering::AcqRel) {
            ABANDONED.fetch_sub(1, Ordering::AcqRel);
        }
    }).map_err(|e| Error::Other(format!("compile thread: {e}")))?;
    let start = std::time::Instant::now();
    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Err(Error::Other("the compile thread exited".into())),
            Err(_) if start.elapsed() > timeout || give_up() => {
                // Counted before it is marked, so the thread's decrement never comes first.
                ABANDONED.fetch_add(1, Ordering::AcqRel);
                gone.store(true, Ordering::Release);
                drop(rx);
                return Err(Error::Other("timed out".into()));
            }
            Err(_) => {}
        }
    }
}
