//! The title's FidelityFX upscaler calls answered by AMD's PC FidelityFX
//! loader for D3D12, `amd_fidelityfx_loader_dx12.dll` from the public
//! FidelityFX SDK (2.x, `Kits/FidelityFX/signedbin`), in the folder
//! `VICTORY_FFX_DIR` names.
//!
//! The console's FidelityFX API has the PC API's structure layouts. Its
//! upscaler DLL only carries the fixed 1920x1080 Quality tables: a Series X
//! configuration made to pick it (3840x2160 output) gets "maxUpscaleSize
//! must be 1920x1080" and the title stops. Here the title's descriptions go
//! to the PC loader with the console objects swapped for the host's: the
//! console backend description for the DX12 one on the host device, the
//! command list for the host list behind the title's, and every texture for
//! its host copy, moved to the state the upscaler reads (inputs) or writes
//! (output) on entry. The PC upscaler leaves them in those states.
#![allow(non_snake_case)]
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::ffi::c_void;
use std::sync::OnceLock;

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{Interface, PCSTR, PCWSTR};

use crate::object::This;

pub(crate) mod rgb9e5;
mod simple;

type ReturnCode = u32;
const OK: ReturnCode = 0;
const ERROR: ReturnCode = 1;
const ERROR_PARAMETER: ReturnCode = 6;

/// `ffxApiHeader`: a description's type and the next in its chain.
#[repr(C)]
pub struct Header {
    kind: u64,
    next: *mut Header,
}

const CREATE_UPSCALE: u64 = 0x0001_0000;
const DISPATCH_UPSCALE: u64 = 0x0001_0001;
const QUERY_GET_VERSIONS: u64 = 4;
const QUERY_JITTER_PHASE_COUNT: u64 = 0x0001_0004;
/// The console's backend description; the device is at +0x10, as in the
/// PC's DX12 one.
const CONSOLE_BACKEND: u64 = 0x0100_0002;
const BACKEND_DX12: u64 = 2;

/// `ffxCreateBackendDX12Desc`.
#[repr(C)]
struct BackendDx12 {
    header: Header,
    device: *mut c_void,
}

/// `ffxCreateContextDescUpscale`.
#[repr(C)]
#[derive(Clone, Copy)]
struct CreateUpscale {
    kind: u64,
    next: *mut Header,
    flags: u32,
    max_render: [u32; 2],
    max_upscale: [u32; 2],
    message: *const c_void,
}
const _: () = assert!(size_of::<CreateUpscale>() == 48);

/// `FfxApiResource`: the resource, its description, its state.
#[repr(C)]
#[derive(Clone, Copy)]
struct Resource {
    resource: *mut c_void,
    description: [u32; 8],
    state: u32,
}
const _: () = assert!(size_of::<Resource>() == 48);

/// `ffxDispatchDescUpscale`.
#[repr(C)]
#[derive(Clone, Copy)]
struct DispatchUpscale {
    kind: u64,
    next: *mut Header,
    command_list: *mut c_void,
    /// Color, depth, motion vectors, exposure, reactive, transparency and
    /// composition, output.
    resources: [Resource; 7],
    jitter: [f32; 2],
    motion_vector_scale: [f32; 2],
    render_size: [u32; 2],
    upscale_size: [u32; 2],
    sharpening: bool,
    sharpness: f32,
    frame_time_delta: f32,
    pre_exposure: f32,
    reset: bool,
    camera: [f32; 4],
    flags: u32,
}
const _: () = assert!(size_of::<DispatchUpscale>() == 432);
const OUTPUT: usize = 6;

/// `ffxQueryDescGetVersions`.
#[repr(C)]
#[derive(Clone, Copy)]
struct GetVersions {
    kind: u64,
    next: *mut Header,
    create_kind: u64,
    device: *mut c_void,
    count: *mut u64,
    ids: *mut u64,
    names: *mut *const i8,
}

/// FfxApiResourceState bits and the host states they stand for.
const FFX_UNORDERED_ACCESS: u32 = 1 << 1;
const FFX_COMPUTE_READ: u32 = 1 << 2;
const HOST_UNORDERED_ACCESS: u32 = 0x8;
const HOST_NON_PIXEL_SHADER_RESOURCE: u32 = 0x40;

type Farproc = unsafe extern "system" fn() -> isize;
type Create = unsafe extern "C" fn(*mut *mut c_void, *mut Header, *const c_void) -> ReturnCode;
type Destroy = unsafe extern "C" fn(*mut *mut c_void, *const c_void) -> ReturnCode;
type Configure = unsafe extern "C" fn(*mut *mut c_void, *const Header) -> ReturnCode;
type Query = unsafe extern "C" fn(*mut *mut c_void, *mut Header) -> ReturnCode;
type Dispatch = unsafe extern "C" fn(*mut *mut c_void, *const Header) -> ReturnCode;

struct Api {
    create: Create,
    destroy: Destroy,
    configure: Configure,
    query: Query,
    dispatch: Dispatch,
}

/// Whether the title's FidelityFX calls come here: `VICTORY_FFX_DIR` or
/// `VICTORY_FFX_SIMPLE=1` set.
pub fn enabled() -> bool {
    std::env::var_os("VICTORY_FFX_DIR").is_some() || simple_mode()
}

/// `VICTORY_FFX_SIMPLE=1`: no upscaler, the colour input scaled to the
/// output bilinearly ([`simple`]); `VICTORY_FFX_DIR` is not used then.
fn simple_mode() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var_os("VICTORY_FFX_SIMPLE").is_some_and(|v| v == "1");
        if on {
            tracing::info!("ffx: simple upscale (VICTORY_FFX_SIMPLE=1): bilinear scale of the colour input, no FSR, no history, no jitter");
        }
        on
    })
}

/// The context handed to the title in simple mode: never dereferenced.
const SIMPLE_CONTEXT: usize = 0x5157_0001;

/// AMD's loader, loaded on first use. The upscaler it picks is loaded by
/// full path first, so the loader finds it without a change to the DLL
/// search path.
fn api() -> Option<&'static Api> {
    static API: OnceLock<Option<Api>> = OnceLock::new();
    API.get_or_init(|| {
        let dir = std::path::PathBuf::from(std::env::var_os("VICTORY_FFX_DIR")?);
        let load = |name: &str| {
            let path: Vec<u16> = dir.join(name).as_os_str().encode_wide_nul();
            // SAFETY: a nul-terminated path.
            unsafe { LoadLibraryW(PCWSTR(path.as_ptr())) }.ok()
        };
        let _ = load("amd_fidelityfx_upscaler_dx12.dll");
        let Some(loader) = load("amd_fidelityfx_loader_dx12.dll") else {
            tracing::error!(?dir, "ffx: amd_fidelityfx_loader_dx12.dll not found; set VICTORY_FFX_DIR to the SDK's signedbin");
            return None;
        };
        let get = |name: &[u8]| {
            // SAFETY: a loaded module and a nul-terminated name.
            unsafe { GetProcAddress(loader, PCSTR(name.as_ptr())) }
        };
        // SAFETY: the SDK's exports, of these types (`ffx_api.h`).
        unsafe {
            Some(Api {
                create: std::mem::transmute::<Farproc, Create>(get(b"ffxCreateContext\0")?),
                destroy: std::mem::transmute::<Farproc, Destroy>(get(b"ffxDestroyContext\0")?),
                configure: std::mem::transmute::<Farproc, Configure>(get(b"ffxConfigure\0")?),
                query: std::mem::transmute::<Farproc, Query>(get(b"ffxQuery\0")?),
                dispatch: std::mem::transmute::<Farproc, Dispatch>(get(b"ffxDispatch\0")?),
            })
        }
    })
    .as_ref()
}

trait WideNul {
    fn encode_wide_nul(&self) -> Vec<u16>;
}

impl WideNul for std::ffi::OsStr {
    fn encode_wide_nul(&self) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        self.encode_wide().chain(Some(0)).collect()
    }
}

fn host_device() -> Option<*mut c_void> {
    crate::host::host().map(|h| h.device.as_raw())
}

/// Logs a description type the first time it is seen.
fn note(call: &str, kind: u64) {
    static SEEN: std::sync::Mutex<Vec<(String, u64)>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.iter().any(|(c, k)| c == call && *k == kind) {
        seen.push((call.to_owned(), kind));
        tracing::info!("ffx: {call} with description type {kind:#x}");
    }
}

/// The upscale description in a chain that also holds the console backend
/// description, the one this module stands in for.
///
/// # Safety
///
/// `h` is null or the start of a chain of valid descriptions.
unsafe fn upscale_in(mut h: *const Header) -> Option<CreateUpscale> {
    let (mut upscale, mut backend) = (None, false);
    while !h.is_null() {
        // SAFETY: the caller's chain.
        let kind = unsafe { (*h).kind };
        match kind {
            // SAFETY: a description of this type.
            CREATE_UPSCALE => upscale = Some(unsafe { *h.cast::<CreateUpscale>() }),
            CONSOLE_BACKEND => backend = true,
            _ => note("ffxCreateContext", kind),
        }
        // SAFETY: as above.
        h = unsafe { (*h).next };
    }
    upscale.filter(|_| backend)
}

pub extern "C" fn ffxCreateContext(
    context: *mut *mut c_void,
    desc: *mut Header,
    _memory: *const c_void,
) -> ReturnCode {
    if simple_mode() {
        if context.is_null() {
            return ERROR_PARAMETER;
        }
        // SAFETY: the title's out pointer, checked for null.
        unsafe { *context = SIMPLE_CONTEXT as *mut c_void };
        return OK;
    }
    let (Some(api), Some(device)) = (api(), host_device()) else {
        return ERROR;
    };
    if context.is_null() || desc.is_null() {
        return ERROR_PARAMETER;
    }
    // SAFETY: the title's chain of descriptions.
    let Some(mut upscale) = (unsafe { upscale_in(desc) }) else {
        return ERROR_PARAMETER;
    };
    // The console's version and provider descriptions name console builds;
    // the PC loader picks the newest upscaler the GPU runs.
    let mut dx12 = BackendDx12 {
        header: Header {
            kind: BACKEND_DX12,
            next: std::ptr::null_mut(),
        },
        device,
    };
    upscale.next = (&raw mut dx12).cast();
    // SAFETY: the SDK's call with a chain that outlives it.
    let r = unsafe { (api.create)(context, (&raw mut upscale).cast(), std::ptr::null()) };
    let provider = provider_name(context);
    tracing::info!(
        render = ?upscale.max_render, upscale = ?upscale.max_upscale, flags = upscale.flags, result = r, provider,
        "ffx: upscale context on the host GPU"
    );
    r
}

/// The upscaler version the loader chose for a context, for the log.
fn provider_name(context: *mut *mut c_void) -> String {
    #[repr(C)]
    struct ProviderVersion {
        kind: u64,
        next: *mut Header,
        id: u64,
        name: *const i8,
    }
    let Some(api) = api() else {
        return String::new();
    };
    let mut q = ProviderVersion {
        kind: 6,
        next: std::ptr::null_mut(),
        id: 0,
        name: std::ptr::null(),
    };
    // SAFETY: the context pointer the title passed, checked for null.
    if context.is_null() || unsafe { (*context).is_null() } {
        return String::new();
    }
    // SAFETY: a context the loader created.
    if unsafe { (api.query)(context, (&raw mut q).cast()) } != OK || q.name.is_null() {
        return String::new();
    }
    // SAFETY: the loader's nul-terminated name.
    unsafe { std::ffi::CStr::from_ptr(q.name) }
        .to_string_lossy()
        .into_owned()
}

pub extern "C" fn ffxDestroyContext(
    context: *mut *mut c_void,
    _memory: *const c_void,
) -> ReturnCode {
    if simple_mode() {
        return OK;
    }
    let Some(api) = api() else { return ERROR };
    // SAFETY: a context this module created.
    unsafe { (api.destroy)(context, std::ptr::null()) }
}

pub extern "C" fn ffxConfigure(context: *mut *mut c_void, desc: *const Header) -> ReturnCode {
    if simple_mode() {
        return OK;
    }
    let Some(api) = api() else { return ERROR };
    if !desc.is_null() {
        // SAFETY: the title's description.
        note("ffxConfigure", unsafe { (*desc).kind });
    }
    // SAFETY: passed on as the title made it.
    unsafe { (api.configure)(context, desc) }
}

pub extern "C" fn ffxQuery(context: *mut *mut c_void, desc: *mut Header) -> ReturnCode {
    if simple_mode() {
        return simple_query(desc);
    }
    let (Some(api), Some(device)) = (api(), host_device()) else {
        return ERROR;
    };
    if desc.is_null() {
        return ERROR_PARAMETER;
    }
    // SAFETY: the title's description.
    let kind = unsafe { (*desc).kind };
    note("ffxQuery", kind);
    if kind == QUERY_GET_VERSIONS {
        // It names the device to enumerate for: the host's.
        // SAFETY: a description of this type.
        let mut q = unsafe { *desc.cast::<GetVersions>() };
        q.device = device;
        // SAFETY: the SDK's call; the title's output arrays stay its own.
        return unsafe { (api.query)(context, (&raw mut q).cast()) };
    }
    // SAFETY: a null context pointer or one this module filled.
    let contextless = context.is_null() || unsafe { (*context).is_null() };
    if !contextless {
        // SAFETY: passed on with the PC context.
        let r = unsafe { (api.query)(context, desc) };
        log_query(kind, desc, r);
        return r;
    }
    // Without a context the loader learns the backend from the chain: the
    // DX12 one is added at its end for the call, then taken off.
    let mut dx12 = BackendDx12 {
        header: Header {
            kind: BACKEND_DX12,
            next: std::ptr::null_mut(),
        },
        device,
    };
    let mut last = desc;
    // SAFETY: the title's chain.
    while !unsafe { (*last).next }.is_null() {
        // SAFETY: as above.
        last = unsafe { (*last).next };
    }
    // SAFETY: the chain's end, restored before returning.
    unsafe { (*last).next = (&raw mut dx12).cast() };
    // SAFETY: the title's description with the backend chained for the call.
    let r = unsafe { (api.query)(context, desc) };
    // SAFETY: the chain's end again, as the title made it.
    unsafe { (*last).next = std::ptr::null_mut() };
    log_query(kind, desc, r);
    r
}

/// Simple mode's answers: one jitter phase (the title then keeps the same
/// sub-pixel offset every frame, nothing to accumulate); other queries are
/// logged and answered as done.
fn simple_query(desc: *mut Header) -> ReturnCode {
    if desc.is_null() {
        return ERROR_PARAMETER;
    }
    // SAFETY: the title's description.
    let kind = unsafe { (*desc).kind };
    note("ffxQuery", kind);
    if kind == QUERY_JITTER_PHASE_COUNT {
        // SAFETY: a description of this type (`log_query`'s layout): the
        // output pointer after the header and the two widths.
        let out = unsafe { *desc.cast::<u8>().add(24).cast::<*mut i32>() };
        if !out.is_null() {
            // SAFETY: the title's output.
            unsafe { *out = 1 };
        }
    }
    OK
}

/// A query's result, once per type, with the jitter phase count it gave.
fn log_query(kind: u64, desc: *mut Header, r: ReturnCode) {
    static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if seen.contains(&kind) {
        return;
    }
    seen.push(kind);
    #[repr(C)]
    struct JitterPhaseCount {
        kind: u64,
        next: *mut Header,
        render_width: u32,
        display_width: u32,
        out: *mut i32,
    }
    if kind == QUERY_JITTER_PHASE_COUNT {
        // SAFETY: a description of this type.
        let q = unsafe { &*desc.cast::<JitterPhaseCount>() };
        // SAFETY: the title's output, written by the call when it succeeded.
        let count = (r == OK && !q.out.is_null()).then(|| unsafe { *q.out });
        tracing::info!(
            r,
            q.render_width,
            q.display_width,
            ?count,
            "ffx: jitter phase count"
        );
    } else {
        tracing::info!(r, "ffx: query {kind:#x}");
    }
}

pub extern "C" fn ffxDispatch(context: *mut *mut c_void, desc: *const Header) -> ReturnCode {
    let api = if simple_mode() {
        None
    } else {
        let Some(api) = api() else { return ERROR };
        Some(api)
    };
    if context.is_null() || desc.is_null() {
        return ERROR_PARAMETER;
    }
    // SAFETY: the title's description.
    let kind = unsafe { (*desc).kind };
    if kind != DISPATCH_UPSCALE {
        note("ffxDispatch", kind);
        return ERROR_PARAMETER;
    }
    // SAFETY: a description of this type.
    let mut d = unsafe { *desc.cast::<DispatchUpscale>() };
    let title_list = d.command_list as This;
    let Some(list) = crate::list::host_list(title_list) else {
        return ERROR_PARAMETER;
    };
    let output = d.resources[OUTPUT].resource as This;
    let input = d.resources[0].resource as This;
    let Some(destination) = crate::resource::host_resource(output) else {
        missing_texture(OUTPUT, output);
        return ERROR_PARAMETER;
    };
    let writable = crate::host::legal_state(destination, HOST_UNORDERED_ACCESS) == HOST_UNORDERED_ACCESS
        && (api.is_some() || simple::supports(destination, true));
    let packed_output = if !writable {
        let Some(packed) = crate::resource::rgb9e5_fsr_output(output)
            .filter(|packed| packed.matches(d.upscale_size))
        else {
            static REPORTED: std::sync::Once = std::sync::Once::new();
            REPORTED.call_once(|| {
                tracing::warn!(?output, size = ?d.upscale_size, "ffx: output is not writable and has no matching RGB9E5 fallback");
            });
            return ERROR_PARAMETER;
        };
        Some(packed)
    } else {
        None
    };
    let Some(api) = api else {
        return simple_dispatch(list, title_list, &d, (input, output), destination, packed_output);
    };
    for (i, r) in d.resources.iter_mut().enumerate() {
        if i == OUTPUT && let Some(packed) = packed_output {
            *r = packed.sdk_resource(list);
            continue;
        }
        if r.resource.is_null() {
            continue;
        }
        let title = r.resource as This;
        let Some(texture) = crate::resource::host_resource(title) else {
            missing_texture(i, title);
            r.resource = std::ptr::null_mut();
            continue;
        };
        let (wanted, ffx) = if i == OUTPUT {
            (HOST_UNORDERED_ACCESS, FFX_UNORDERED_ACCESS)
        } else {
            (HOST_NON_PIXEL_SHADER_RESOURCE, FFX_COMPUTE_READ)
        };
        crate::list::ensure_state(list, title, wanted);
        if crate::host::legal_state(texture, wanted) != wanted {
            tracing::warn!(
                slot = i,
                wanted,
                "ffx: the host texture cannot take the upscaler's state"
            );
        }
        // An RGB9E5 texture the host made as RGBA16F (`resource::target_format`)
        // is announced to the SDK as what it really is.
        const FFX_R16G16B16A16_FLOAT: u32 = 4;
        const FFX_R9G9B9E5_SHAREDEXP: u32 = 29;
        const DXGI_R16G16B16A16_FLOAT: i32 = 10;
        // SAFETY: a live host texture.
        let host_format = unsafe { texture.GetDesc() }.Format.0;
        if r.description[1] == FFX_R9G9B9E5_SHAREDEXP && host_format == DXGI_R16G16B16A16_FLOAT {
            r.description[1] = FFX_R16G16B16A16_FLOAT;
        }
        r.resource = texture.as_raw();
        r.state = ffx;
    }
    if d.resources[0].resource.is_null() || d.resources[OUTPUT].resource.is_null() {
        return ERROR_PARAMETER;
    }
    d.command_list = list.list.as_raw();
    // SAFETY: the SDK's call on an open host list with live textures.
    let r = unsafe { (api.dispatch)(context, (&raw const d).cast()) };
    if r == OK {
        if let Some(packed) = packed_output {
            packed.copy_to(list, output, destination);
            static FIRST_PACK: std::sync::Once = std::sync::Once::new();
            FIRST_PACK.call_once(|| {
                tracing::info!(?output, size = ?d.upscale_size, "ffx: recorded RGB9E5 output conversion");
            });
        }
        crate::texture::host_written(output);
        #[cfg(feature = "diagnostics")]
        {
            crate::capture::note_target(input);
            crate::capture::note_target(output);
        }
    }
    // The upscaler bound its own heaps, root signature and pipeline.
    crate::list::forget_host_bindings(title_list);
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        // SAFETY: the destination is a live host texture.
        let format = unsafe { destination.GetDesc() }.Format.0;
        tracing::info!(render = ?d.render_size, upscale = ?d.upscale_size, result = r, ?input, ?output, format, packed = packed_output.is_some(), "ffx: first upscale recorded on the host GPU");
    });
    r
}

/// [`ffxDispatch`] in simple mode: the colour input's render rectangle
/// scaled to the output ([`simple::record`]), packed into RGB9E5 after if
/// the output has that fallback.
fn simple_dispatch(
    list: &crate::host::HostList,
    title_list: This,
    d: &DispatchUpscale,
    (input, output): (This, This),
    destination: &windows::Win32::Graphics::Direct3D12::ID3D12Resource,
    packed: Option<&'static rgb9e5::Output>,
) -> ReturnCode {
    let Some(source) = crate::resource::host_resource(input) else {
        missing_texture(0, input);
        return ERROR_PARAMETER;
    };
    if !simple::supports(source, false) {
        static REPORTED: std::sync::Once = std::sync::Once::new();
        REPORTED.call_once(|| tracing::warn!(?input, "ffx: simple upscale: the colour input cannot be sampled"));
        return ERROR_PARAMETER;
    }
    crate::list::ensure_state(list, input, HOST_NON_PIXEL_SHADER_RESOURCE);
    let target = match packed {
        Some(p) => p.float(list),
        None => {
            crate::list::ensure_state(list, output, HOST_UNORDERED_ACCESS);
            destination
        }
    };
    let recorded = simple::record(list, source, target, d.render_size, d.upscale_size);
    if recorded {
        if let Some(p) = packed {
            p.copy_to(list, output, destination);
        }
        crate::texture::host_written(output);
    }
    crate::list::forget_host_bindings(title_list);
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        tracing::info!(render = ?d.render_size, upscale = ?d.upscale_size, recorded, ?input, ?output, packed = packed.is_some(), "ffx: first simple upscale recorded");
    });
    if recorded { OK } else { ERROR }
}

fn missing_texture(slot: usize, title: This) {
    use std::sync::atomic::{AtomicU8, Ordering};
    static REPORTED: AtomicU8 = AtomicU8::new(0);
    let bit = 1 << slot;
    if REPORTED.fetch_or(bit, Ordering::Relaxed) & bit == 0 {
        let (title_address, desc) = crate::resource::texture_memory(title)
            .map_or((0, None), |(address, desc)| (address as usize, Some(desc)));
        tracing::warn!(slot, ?title, title_address, ?desc, "ffx: an upscale resource without a host texture");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_takes_the_upscale_description_only_beside_the_console_backend() {
        let mut backend = Header {
            kind: CONSOLE_BACKEND,
            next: std::ptr::null_mut(),
        };
        let backend = &raw mut backend;
        // The version description the title chains in between.
        let mut version = Header {
            kind: 0x1_000b,
            next: backend,
        };
        let upscale = CreateUpscale {
            kind: CREATE_UPSCALE,
            next: &raw mut version,
            flags: 41,
            max_render: [2560, 1440],
            max_upscale: [3840, 2160],
            message: std::ptr::null(),
        };
        let chain = (&raw const upscale).cast::<Header>();
        // SAFETY: a chain of our own descriptions, alive for the calls.
        let found = unsafe { upscale_in(chain) }.expect("the upscale description");
        assert_eq!(
            (found.flags, found.max_render, found.max_upscale),
            (41, [2560, 1440], [3840, 2160])
        );
        // A PC backend is someone else's chain: not ours to answer.
        // SAFETY: our own description, still alive.
        unsafe { (*backend).kind = BACKEND_DX12 };
        // SAFETY: as above.
        assert!(unsafe { upscale_in(chain) }.is_none());
        // SAFETY: an empty chain.
        assert!(unsafe { upscale_in(std::ptr::null()) }.is_none());
    }
}
