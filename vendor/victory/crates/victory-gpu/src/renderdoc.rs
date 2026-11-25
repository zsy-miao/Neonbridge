//! Diagnostics: one frame of the title captured with RenderDoc.
//!
//! `VICTORY_RENDERDOC=<full path of renderdoc.dll>` loads RenderDoc into
//! this process before the host device is made (its documented
//! in-application use: it hooks the graphics interfaces created after it
//! is loaded). `VICTORY_RENDERDOC_AT=<n>` (150 by default) then asks it for
//! the frame `n` presents after the first frame of 500 draws or more, and
//! `VICTORY_RENDERDOC_OUT` names the capture files (by default
//! `victory_rdc\frame` under the temporary folder).
//!
//! Nothing is loaded unless the variable gives the full path of a file: a
//! bare name is refused, so neither the current folder nor the title's is
//! ever searched for it.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// RenderDoc's in-application interface: a table of function pointers
/// (`RENDERDOC_API_1_1_2` of its `renderdoc_app.h`), by address.
struct Api(usize);

/// `void SetCaptureFilePathTemplate(const char *)`.
const SET_CAPTURE_FILE_PATH_TEMPLATE: usize = 11;
/// `uint32_t GetNumCaptures(void)`.
const GET_NUM_CAPTURES: usize = 13;
/// `void TriggerCapture(void)`: the next presented frame is captured.
const TRIGGER_CAPTURE: usize = 15;
/// `eRENDERDOC_API_Version_1_1_2`.
const VERSION_1_1_2: i32 = 10102;

impl Api {
    /// The table's entry `index` as a function of type `F`.
    ///
    /// # Safety
    /// `index` is an entry of the 1.1.2 table and `F` its signature.
    unsafe fn entry<F: Copy>(&self, index: usize) -> F {
        let table = self.0 as *const usize;
        // SAFETY: by the caller; the table lives as long as the module,
        // which is never unloaded.
        unsafe { std::mem::transmute_copy::<usize, F>(&*table.add(index)) }
    }
}

static API: OnceLock<Option<Api>> = OnceLock::new();

/// Whether RenderDoc was loaded into this run ([`load`]): the host device
/// is then RenderDoc's, and what a driver keeps for its own device (the
/// pipeline library's blob) is neither read nor written by this run.
pub(crate) fn loaded() -> bool {
    API.get().is_some_and(Option::is_some)
}

/// Loads RenderDoc when `VICTORY_RENDERDOC` names its DLL by full path.
/// Called before the host device is made.
pub(crate) fn load() {
    API.get_or_init(|| {
        let path = std::path::PathBuf::from(std::env::var_os("VICTORY_RENDERDOC")?);
        if !path.is_absolute() || !path.is_file() {
            tracing::warn!(
                path = %path.display(),
                "VICTORY_RENDERDOC: not the full path of a file; RenderDoc is not loaded"
            );
            return None;
        }
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::System::LibraryLoader::{
            GetProcAddress, LOAD_WITH_ALTERED_SEARCH_PATH, LoadLibraryExW,
        };
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: a NUL-terminated full path, named by the user.
        let module =
            unsafe { LoadLibraryExW(wide.as_ptr(), std::ptr::null_mut(), LOAD_WITH_ALTERED_SEARCH_PATH) };
        if module.is_null() {
            tracing::warn!(path = %path.display(), "VICTORY_RENDERDOC: the DLL does not load");
            return None;
        }
        // SAFETY: a loaded module and a NUL-terminated name.
        let get = unsafe { GetProcAddress(module, c"RENDERDOC_GetAPI".as_ptr().cast()) }?;
        // SAFETY: the export's documented signature,
        // `int RENDERDOC_GetAPI(RENDERDOC_Version, void **)`.
        let get = unsafe {
            std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                unsafe extern "C" fn(i32, *mut *mut c_void) -> i32,
            >(get)
        };
        let mut table: *mut c_void = std::ptr::null_mut();
        // SAFETY: a valid out pointer.
        if unsafe { get(VERSION_1_1_2, &mut table) } != 1 || table.is_null() {
            tracing::warn!("VICTORY_RENDERDOC: the DLL does not give its interface 1.1.2");
            return None;
        }
        let api = Api(table as usize);
        let out = std::env::var_os("VICTORY_RENDERDOC_OUT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("victory_rdc").join("frame"));
        if let Some(folder) = out.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        if let Ok(name) = std::ffi::CString::new(out.to_string_lossy().as_bytes()) {
            // SAFETY: the entry's signature; a NUL-terminated string it copies.
            unsafe {
                api.entry::<unsafe extern "C" fn(*const std::ffi::c_char)>(SET_CAPTURE_FILE_PATH_TEMPLATE)(
                    name.as_ptr(),
                );
            }
        }
        tracing::info!(
            path = %path.display(),
            out = %out.display(),
            at = at(),
            "RenderDoc loaded: it captures one frame of the world"
        );
        Some(api)
    });
}

/// How many presents after the first world frame the capture is asked for.
fn at() -> u64 {
    static AT: OnceLock<u64> = OnceLock::new();
    *AT.get_or_init(|| {
        std::env::var("VICTORY_RENDERDOC_AT")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(150)
    })
}

/// At the present of `frame`, with `drawn` draws run on the host so far.
pub(crate) fn frame_drew(frame: u64, drawn: u64) {
    let Some(api) = API.get().and_then(|a| a.as_ref()) else {
        return;
    };
    static LAST: AtomicU64 = AtomicU64::new(0);
    static WORLD: AtomicU64 = AtomicU64::new(u64::MAX);
    let before = LAST.swap(drawn, Relaxed);
    if drawn.saturating_sub(before) >= 500 {
        let _ = WORLD.compare_exchange(u64::MAX, frame, Relaxed, Relaxed);
    }
    let world = WORLD.load(Relaxed);
    if world == u64::MAX {
        return;
    }
    if frame == world + at() {
        // SAFETY: the entry's signature.
        unsafe { api.entry::<unsafe extern "C" fn()>(TRIGGER_CAPTURE)() };
        tracing::info!(frame, "RenderDoc is asked for the next frame");
    } else if frame == world + at() + 8 {
        // SAFETY: the entry's signature.
        let captures = unsafe { api.entry::<unsafe extern "C" fn() -> u32>(GET_NUM_CAPTURES)() };
        tracing::info!(captures, "RenderDoc captures made so far");
    }
}
