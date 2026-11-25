//! 6ixpack: with TUXBOX_DRED=1, enable Device Removed Extended Data before the
//! host device exists and log why the device was lost: the removal reason,
//! the last GPU breadcrumbs of each command list and any GPU page fault.
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;

fn enabled() -> bool {
    std::env::var_os("TUXBOX_DRED").is_some_and(|v| v == "1")
}

/// Names a command list for DRED (TUXBOX_DRED=1 only): test73's breadcrumbs
/// showed every list as "", so a fault in our own copies, uploads, readbacks
/// or wait lists could not be told from the title's.
pub(crate) fn name(list: &impl windows::core::Interface, label: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(enabled) {
        return;
    }
    if let Ok(object) = list.cast::<windows::Win32::Graphics::Direct3D12::ID3D12Object>() {
        let _ = unsafe { object.SetName(&windows::core::HSTRING::from(label)) };
    }
}

fn pipeline_tag_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| enabled() && std::env::var_os("TUXBOX_DRED_PIPELINE_TAG").is_none_or(|v| v != "0"))
}

/// TUXBOX_DRED_PIPELINE_TAG (default on with TUXBOX_DRED=1, =0 off): marks a
/// pipeline bind in the list with its key (SetMarker, metadata 0 = a wide
/// string, which DRED 1.2 keeps as breadcrumb context), so a hung list's
/// report names the pipeline its last ops ran (test192: an unnamed title
/// list stopped at a DrawInstanced with no pipeline on record).
pub(crate) fn tag_pipeline(list: &ID3D12GraphicsCommandList, key: Option<&str>) {
    if !pipeline_tag_on() {
        return;
    }
    let text: Vec<u16> = format!("pso {}", key.unwrap_or("?")).encode_utf16().chain(std::iter::once(0)).collect();
    unsafe { list.SetMarker(0, Some(text.as_ptr().cast()), (text.len() * 2) as u32) };
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || "DRED: pipeline binds tagged with their key (TUXBOX_DRED_PIPELINE_TAG=0 off)".to_string());
}

/// TUXBOX_DRED_PIPELINE_TAG: one line per created pipeline naming its key's
/// stage digests (the shader cache keys), and the session folder once, so a
/// tagged hung pipeline leads to its .hlsl/.cso/.ztrx files.
pub(crate) fn log_pipeline(key: &str, digests: &[String]) {
    if !pipeline_tag_on() {
        return;
    }
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || {
        let session = std::env::var("TUXBOX_SHADER_SESSION").unwrap_or_else(|_| "-".into());
        format!("DRED: shader session folder {}", crate::device::shader_cache::root().join(session).display())
    });
    crate::diagnostics::log_line(format_args!("DRED pso {key}: {}", digests.join(" ")));
}

/// The last tagged pipeline at or before each incomplete list's stop.
unsafe fn report_pipeline_tags(data: &ID3D12DeviceRemovedExtendedData) {
    if !pipeline_tag_on() {
        return;
    }
    let Ok(data1) = data.cast::<ID3D12DeviceRemovedExtendedData1>() else { return };
    let Ok(crumbs) = (unsafe { data1.GetAutoBreadcrumbsOutput1() }) else { return };
    let mut node = crumbs.pHeadAutoBreadcrumbNode;
    let mut shown = 0;
    let mut count = 0;
    while !node.is_null() && count < 4096 && shown < 16 {
        let n = unsafe { &*node };
        let done = if n.pLastBreadcrumbValue.is_null() { 0 } else { unsafe { *n.pLastBreadcrumbValue } };
        if done < n.BreadcrumbCount && n.BreadcrumbContextsCount != 0 && !n.pBreadcrumbContexts.is_null() {
            let contexts = unsafe { std::slice::from_raw_parts(n.pBreadcrumbContexts, n.BreadcrumbContextsCount as usize) };
            if let Some(c) = contexts.iter().filter(|c| c.BreadcrumbIndex <= done).last() {
                let text = unsafe { c.pContextString.to_string() }.unwrap_or_default();
                log(&format!("incomplete list stopped at op {done}/{}: last pipeline tagged at op {}: {text}", n.BreadcrumbCount, c.BreadcrumbIndex));
                shown += 1;
            }
        }
        node = n.pNext;
        count += 1;
    }
}
fn log(line: &str) {
    crate::diagnostics::log_once(&std::sync::Once::new(), || format!("DRED: {line}"));
}

/// Call before D3D12CreateDevice.
pub(crate) fn configure() {
    if !enabled() {
        return;
    }
    // test60: D3D12GetDebugInterface refused the settings interface; the
    // runtime also hands it out through D3D12GetInterface.
    let mut settings: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
    let debug = unsafe { D3D12GetDebugInterface(&mut settings) };
    if settings.is_none() {
        let mut other: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
        let got = unsafe { D3D12GetInterface(&CLSID_D3D12DeviceRemovedExtendedData, &mut other) };
        if other.is_none() {
            log(&format!("settings interface unavailable (D3D12GetDebugInterface {:?}, D3D12GetInterface {:?})",
                debug.err().map(|e| e.code()), got.err().map(|e| e.code())));
            return;
        }
        settings = other;
    }
    if let Some(settings) = settings {
        unsafe {
            settings.SetAutoBreadcrumbsEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
            settings.SetPageFaultEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
        }
        log("auto-breadcrumbs and page fault reporting on");
    } else {
        log("settings interface unavailable");
    }
}

/// Watch `device` and report once when it is removed.
pub(crate) fn watch(device: &ID3D12Device) {
    if !enabled() {
        return;
    }
    let Ok(fence) = (unsafe { device.CreateFence::<ID3D12Fence>(0, D3D12_FENCE_FLAG_NONE) }) else { return };
    let device = device.clone();
    let _ = std::thread::Builder::new().name("tuxbox-dred".into()).spawn(move || unsafe {
        // A removed device completes every fence value.
        let Ok(event) = CreateEventW(None, false, false, None) else { return };
        if fence.SetEventOnCompletion(u64::MAX, event).is_ok() && WaitForSingleObject(event, INFINITE) == WAIT_OBJECT_0 {
            report(&device);
        }
        let _ = CloseHandle(event);
    });
}

unsafe fn report(device: &ID3D12Device) {
    let reason = unsafe { device.GetDeviceRemovedReason() };
    log(&format!("device removed: {reason:?}"));
    let Ok(data) = device.cast::<ID3D12DeviceRemovedExtendedData>() else { return };
    let name = |p: *const u8| if p.is_null() { String::new() } else { unsafe { windows::core::PCSTR(p).to_string() }.unwrap_or_default() };
    // TUXBOX_DRED_WIDE_NAMES (default on, =0 off): test192 printed every list
    // as "" though dred::name had named them; SetName stores a wide name, and
    // DRED left the ANSI pointer empty. Fall back to the wide one.
    let wide = std::env::var_os("TUXBOX_DRED_WIDE_NAMES").is_none_or(|v| v != "0");
    let named = |a: *const u8, w: windows::core::PCWSTR| {
        let s = name(a);
        if !s.is_empty() || !wide || w.is_null() { return s; }
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || "DRED: list names read from the wide name (TUXBOX_DRED_WIDE_NAMES=0 off)".to_string());
        unsafe { w.to_string() }.unwrap_or_default()
    };
    if let Ok(crumbs) = unsafe { data.GetAutoBreadcrumbsOutput() } {
        let mut node = crumbs.pHeadAutoBreadcrumbNode;
        let mut count = 0;
        let mut incomplete = 0;
        // test61 printed no ops: every list's breadcrumbs had completed, or
        // none were kept. Count them, and show the tails of the last lists.
        let mut tails: std::collections::VecDeque<String> = std::collections::VecDeque::new();
        // The completed lists just before the first incomplete one (test73:
        // all 35 incomplete lists had done nothing, so the fault was in work
        // DRED counted complete).
        let mut before_first: Option<Vec<String>> = None;
        while !node.is_null() && count < 4096 {
            let n = unsafe { &*node };
            let done = if n.pLastBreadcrumbValue.is_null() { 0 } else { unsafe { *n.pLastBreadcrumbValue } };
            if n.BreadcrumbCount != 0 && !n.pCommandHistory.is_null() {
                let ops = unsafe { std::slice::from_raw_parts(n.pCommandHistory, n.BreadcrumbCount as usize) };
                let tail: Vec<String> = ops[ops.len().saturating_sub(6)..].iter().map(|op| op.0.to_string()).collect();
                tails.push_back(format!("list {:?} queue {:?}: {done}/{} done, last ops [{}]",
                    named(n.pCommandListDebugNameA, n.pCommandListDebugNameW), named(n.pCommandQueueDebugNameA, n.pCommandQueueDebugNameW), n.BreadcrumbCount, tail.join(" ")));
                if tails.len() > 3 {
                    tails.pop_front();
                }
            }
            if done < n.BreadcrumbCount {
                if incomplete == 0 {
                    before_first = Some(tails.iter().rev().skip(1).cloned().collect());
                }
                incomplete += 1;
                if incomplete > 16 {
                    node = n.pNext;
                    count += 1;
                    continue;
                }
                let ops = unsafe { std::slice::from_raw_parts(n.pCommandHistory, n.BreadcrumbCount as usize) };
                let from = (done as usize).saturating_sub(4);
                let to = (done as usize + 4).min(ops.len());
                let window: Vec<String> = ops[from..to].iter().enumerate()
                    .map(|(i, op)| format!("{}{}", if from + i == done as usize { ">" } else { "" }, op.0))
                    .collect();
                log(&format!("list {:?} queue {:?}: {done}/{} ops completed, around [{}]",
                    named(n.pCommandListDebugNameA, n.pCommandListDebugNameW), named(n.pCommandQueueDebugNameA, n.pCommandQueueDebugNameW), n.BreadcrumbCount, window.join(" ")));
            }
            node = n.pNext;
            count += 1;
        }
        log(&format!("breadcrumbs: {count} command lists, {incomplete} not completed (op numbers are D3D12_AUTO_BREADCRUMB_OP)"));
        if incomplete == 0 {
            for line in tails {
                log(&line);
            }
        } else if let Some(lines) = before_first {
            for line in lines.iter().rev() {
                log(&format!("completed before the first incomplete: {line}"));
            }
        }
    } else {
        log("no breadcrumb output");
    }
    unsafe { report_pipeline_tags(&data) };
    if let Ok(fault) = unsafe { data.GetPageFaultAllocationOutput() } {
        // DRED answers with a zeroed record when no page fault caused the
        // removal: test142's "page fault at GPU VA 0x0" was a driver internal
        // error (0x887A0020) with no allocation named, not a fault at 0.
        if fault.PageFaultVA == 0 && fault.pHeadExistingAllocationNode.is_null() && fault.pHeadRecentFreedAllocationNode.is_null() {
            log("no page fault recorded (the removal was not a GPU page fault)");
            return;
        }
        log(&format!("page fault at GPU VA {:#x}", fault.PageFaultVA));
        for (label, mut node) in [("existing", fault.pHeadExistingAllocationNode), ("freed", fault.pHeadRecentFreedAllocationNode)] {
            let mut count = 0;
            while !node.is_null() && count < 16 {
                let n = unsafe { &*node };
                log(&format!("  {label} allocation type {} {:?}", n.AllocationType.0, name(n.ObjectNameA)));
                node = n.pNext;
                count += 1;
            }
        }
    }
}

/// With TUXBOX_D3D12_SDK=<directory holding D3D12Core.dll and
/// d3d12SDKLayers.dll>, create the device through that Agility runtime with
/// the debug layer on, and log each error message it reports. Diagnosis only.
///
/// test88: a failed try left the normal device failing too (0x887A002D). It is
/// tried once per process; on failure the caller makes the normal device. The
/// version is read from the file (TUXBOX_D3D12_SDK_VERSION overrides), never
/// by loading it: a D3D12Core.dll we load ourselves is what the system d3d12
/// finds by name afterwards, which mismatches it.
pub(crate) fn debug_device(adapter: Option<&windows::core::IUnknown>) -> Option<windows::core::Result<ID3D12Device>> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    static TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let path = std::env::var("TUXBOX_D3D12_SDK").ok()?;
    if SELECTED.load(std::sync::atomic::Ordering::Acquire) || TRIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    log(&format!("debug layer: D3D12Core.dll already loaded before the factory: {}", loaded_core()));
    let created = (|| unsafe {
        let mut config: Option<ID3D12SDKConfiguration1> = None;
        D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config)?;
        let config = config.ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
        let dir = std::ffi::CString::new(path.clone()).map_err(|_| windows::Win32::Foundation::E_INVALIDARG)?;
        // The factory must ask for the SDK version the directory's D3D12Core.dll exports.
        let version = sdk_version(&path).ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
        log(&format!("debug layer SDK version {version}"));
        let factory: ID3D12DeviceFactory = config.CreateDeviceFactory(version, windows::core::PCSTR(dir.as_ptr().cast()))?;
        let debug: ID3D12Debug = factory.GetConfigurationInterface(&CLSID_D3D12Debug)?;
        debug.EnableDebugLayer();
        let mut device: Option<ID3D12Device> = None;
        factory.CreateDevice(adapter, D3D_FEATURE_LEVEL_12_0, &mut device)?;
        let device = device.ok_or(windows::Win32::Foundation::E_POINTER)?;
        if let Ok(queue) = device.cast::<ID3D12InfoQueue1>() {
            let mut cookie = 0;
            queue.RegisterMessageCallback(Some(on_message), D3D12_MESSAGE_CALLBACK_FLAG_NONE, std::ptr::null_mut(), &mut cookie)?;
        } else {
            log("debug layer has no InfoQueue1; messages are not captured");
        }
        Ok(device)
    })();
    log(&format!("debug layer device from {path}: {:?}{}", created.as_ref().map(|_| ()),
        if created.is_err() { "; making the normal device instead" } else { "" }));
    Some(created)
}

static SELECTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// test89: the system D3D12Core.dll was already loaded when the factory asked
/// for the SDK's, and CreateDevice failed 0x887A002D. The title exe has no
/// D3D12SDKVersion/D3D12SDKPath exports (it is the dumped console exe), so the
/// SDK is chosen with ID3D12SDKConfiguration::SetSDKVersion, which works only
/// before D3D12Core loads and only with Windows Developer Mode on. On success the
/// normal D3D12CreateDevice makes the debug device. Call first in device creation.
///
/// test91: SetSDKVersion succeeded, then D3D12CreateDevice failed
/// 0x887E0003 (D3D12_ERROR_INVALID_REDIST) and the title died on WARP. The
/// choice is process-wide and cannot be taken back, so it now needs
/// TUXBOX_D3D12_SDK_GLOBAL=1 as well; otherwise only the device factory is
/// tried, whose failure leaves the normal device working.
pub(crate) fn sdk_selected() -> bool {
    SELECTED.load(std::sync::atomic::Ordering::Acquire)
}

pub(crate) fn select_sdk() {
    if std::env::var("TUXBOX_D3D12_SDK_GLOBAL").map_or(true, |v| v.trim() != "1") {
        return;
    }
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    static ONCE: std::sync::Once = std::sync::Once::new();
    let Ok(path) = std::env::var("TUXBOX_D3D12_SDK") else { return };
    ONCE.call_once(|| {
        let core = loaded_core();
        let early = unsafe { GetModuleHandleW(windows::core::w!("D3D12Core.dll")) }.is_err();
        let result = (|| unsafe {
            if !early { return Err(windows::core::Error::new(windows::Win32::Foundation::E_ILLEGAL_METHOD_CALL, "D3D12Core already loaded")); }
            let version = sdk_version(&path).ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
            let mut config: Option<ID3D12SDKConfiguration> = None;
            D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config)?;
            let config = config.ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
            let mut dir = path.trim_end_matches(['\\', '/']).to_string();
            dir.push('\\');
            let dir = std::ffi::CString::new(dir).map_err(|_| windows::Win32::Foundation::E_INVALIDARG)?;
            config.SetSDKVersion(version, windows::core::PCSTR(dir.as_ptr().cast()))?;
            let mut debug: Option<ID3D12Debug> = None;
            D3D12GetDebugInterface(&mut debug)?;
            debug.ok_or(windows::Win32::Foundation::E_NOINTERFACE)?.EnableDebugLayer();
            Ok(())
        })();
        log(&format!("debug layer: SetSDKVersion from {path} (D3D12Core loaded before: {core}): {:?}{}",
            result.as_ref().map(|_| ()),
            if result.is_err() { "; trying the device factory instead (SetSDKVersion needs Developer Mode and no D3D12 call before it)" } else { "" }));
        if result.is_ok() { SELECTED.store(true, std::sync::atomic::Ordering::Release); }
    });
}

/// After SetSDKVersion: capture the debug layer's messages on the device.
pub(crate) fn capture_messages(device: &ID3D12Device) {
    if !SELECTED.load(std::sync::atomic::Ordering::Acquire) { return; }
    let result = device.cast::<ID3D12InfoQueue1>().and_then(|queue| {
        let mut cookie = 0;
        unsafe { queue.RegisterMessageCallback(Some(on_message), D3D12_MESSAGE_CALLBACK_FLAG_NONE, std::ptr::null_mut(), &mut cookie) }
    });
    log(&format!("debug layer: device made through the SDK; message capture {:?}", result.map(|_| ())));
}

fn sdk_version(path: &str) -> Option<u32> {
    std::env::var("TUXBOX_D3D12_SDK_VERSION").ok().and_then(|v| v.trim().parse().ok()).or_else(|| {
        std::fs::read(std::path::Path::new(path).join("D3D12Core.dll")).ok().and_then(|image| pe_export_u32(&image, b"D3D12SDKVersion"))
    })
}

/// Which D3D12Core.dll the process holds, if any.
fn loaded_core() -> String {
    use windows::Win32::System::LibraryLoader::{GetModuleFileNameW, GetModuleHandleW};
    let Ok(module) = (unsafe { GetModuleHandleW(windows::core::w!("D3D12Core.dll")) }) else { return "none".into() };
    let mut name = [0u16; 520];
    let n = unsafe { GetModuleFileNameW(Some(module), &mut name) } as usize;
    String::from_utf16_lossy(&name[..n])
}

/// The u32 a PE image exports under `wanted`, read from the file bytes.
fn pe_export_u32(image: &[u8], wanted: &[u8]) -> Option<u32> {
    let u16_at = |o: usize| image.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |o: usize| image.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let pe = u32_at(0x3c)? as usize;
    if image.get(pe..pe + 4)? != b"PE\0\0" { return None; }
    let sections = u16_at(pe + 6)? as usize;
    let optional = pe + 24;
    let optional_size = u16_at(pe + 20)? as usize;
    let data_dirs = match u16_at(optional)? { 0x20b => optional + 112, 0x10b => optional + 96, _ => return None };
    let export_rva = u32_at(data_dirs)? as usize;
    let table = optional + optional_size;
    let to_file = |rva: usize| -> Option<usize> {
        (0..sections).find_map(|i| {
            let s = table + i * 40;
            let va = u32_at(s + 12)? as usize;
            let size = (u32_at(s + 8)? as usize).max(u32_at(s + 16)? as usize);
            let raw = u32_at(s + 20)? as usize;
            (rva >= va && rva < va + size).then(|| rva - va + raw)
        })
    };
    let dir = to_file(export_rva)?;
    let names = u32_at(dir + 24)? as usize;
    let functions = to_file(u32_at(dir + 28)? as usize)?;
    let name_table = to_file(u32_at(dir + 32)? as usize)?;
    let ordinals = to_file(u32_at(dir + 36)? as usize)?;
    for i in 0..names {
        let at = to_file(u32_at(name_table + i * 4)? as usize)?;
        if image.get(at..at + wanted.len() + 1)? == [wanted, b"\0"].concat().as_slice() {
            let ordinal = u16_at(ordinals + i * 2)? as usize;
            let rva = u32_at(functions + ordinal * 4)? as usize;
            return u32_at(to_file(rva)?);
        }
    }
    None
}

unsafe extern "system" fn on_message(
    _category: D3D12_MESSAGE_CATEGORY,
    severity: D3D12_MESSAGE_SEVERITY,
    id: D3D12_MESSAGE_ID,
    description: windows::core::PCSTR,
    _context: *mut core::ffi::c_void,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    if severity.0 > D3D12_MESSAGE_SEVERITY_WARNING.0 || COUNT.fetch_add(1, Ordering::Relaxed) >= 400 {
        return;
    }
    let text = unsafe { description.to_string() }.unwrap_or_default();
    log(&format!("debug layer severity {} id {}: {text}", severity.0, id.0));
}

/// With TUXBOX_DRED=1, name the first submission after which the device
/// reports removal. A removal for an invalid call is raised synchronously.
pub(crate) fn check(queue: &ID3D12CommandQueue, site: &str) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static REPORTED: AtomicBool = AtomicBool::new(false);
    if !enabled() || REPORTED.load(Ordering::Relaxed) {
        return;
    }
    let mut device: Option<ID3D12Device> = None;
    if unsafe { queue.GetDevice(&mut device) }.is_err() {
        return;
    }
    let Some(device) = device else { return };
    if let Err(error) = unsafe { device.GetDeviceRemovedReason() }
        && !REPORTED.swap(true, Ordering::Relaxed)
    {
        let kind = unsafe { queue.GetDesc() }.Type.0;
        log(&format!("device first seen removed after {site} on queue type {kind}: {:?}", error.code()));
    }
}
