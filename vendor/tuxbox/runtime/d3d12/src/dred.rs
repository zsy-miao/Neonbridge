//! 6ixpack: with TUXBOX_DRED=1, enable Device Removed Extended Data before the
//! host device exists and log why the device was lost: the removal reason,
//! the last GPU breadcrumbs of each command list and any GPU page fault.
use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::Interface;
use std::os::windows::ffi::OsStrExt;

fn enabled() -> bool {
    std::env::var_os("TUXBOX_DRED").is_some_and(|v| v == "1")
}

fn log(line: &str) {
    crate::diagnostics::log_once(&std::sync::Once::new(), || format!("DRED: {line}"));
}

/// Call before D3D12CreateDevice.
pub(crate) fn configure() {
    if !enabled() {
        return;
    }
    let mut settings: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
    if unsafe { D3D12GetDebugInterface(&mut settings) }.is_ok()
        && let Some(settings) = settings
    {
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
    if let Ok(crumbs) = unsafe { data.GetAutoBreadcrumbsOutput() } {
        let mut node = crumbs.pHeadAutoBreadcrumbNode;
        let mut count = 0;
        while !node.is_null() && count < 64 {
            let n = unsafe { &*node };
            let done = if n.pLastBreadcrumbValue.is_null() { 0 } else { unsafe { *n.pLastBreadcrumbValue } };
            if done < n.BreadcrumbCount {
                let ops = unsafe { std::slice::from_raw_parts(n.pCommandHistory, n.BreadcrumbCount as usize) };
                let from = (done as usize).saturating_sub(4);
                let to = (done as usize + 4).min(ops.len());
                let window: Vec<String> = ops[from..to].iter().enumerate()
                    .map(|(i, op)| format!("{}{}", if from + i == done as usize { ">" } else { "" }, op.0))
                    .collect();
                log(&format!("list {:?} queue {:?}: {done}/{} ops completed, around [{}]",
                    name(n.pCommandListDebugNameA), name(n.pCommandQueueDebugNameA), n.BreadcrumbCount, window.join(" ")));
            }
            node = n.pNext;
            count += 1;
        }
    }
    if let Ok(fault) = unsafe { data.GetPageFaultAllocationOutput() } {
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
pub(crate) fn debug_device(adapter: Option<&windows::core::IUnknown>) -> Option<windows::core::Result<ID3D12Device>> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    let path = std::env::var("TUXBOX_D3D12_SDK").ok()?;
    let created = (|| unsafe {
        let mut config: Option<ID3D12SDKConfiguration1> = None;
        D3D12GetInterface(&CLSID_D3D12SDKConfiguration, &mut config)?;
        let config = config.ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
        let dir = std::ffi::CString::new(path.clone()).map_err(|_| windows::Win32::Foundation::E_INVALIDARG)?;
        // The factory must ask for the SDK version the directory's D3D12Core.dll exports.
        let version = {
            use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LoadLibraryExW};
            let core: Vec<u16> = std::path::Path::new(&path).join("D3D12Core.dll").as_os_str().encode_wide().chain([0]).collect();
            let module = LoadLibraryExW(windows::core::PCWSTR(core.as_ptr()), None, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR)?;
            let export = GetProcAddress(module, windows::core::s!("D3D12SDKVersion")).ok_or(windows::Win32::Foundation::E_NOINTERFACE)?;
            *(export as *const u32)
        };
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
    log(&format!("debug layer device from {path}: {:?}", created.as_ref().map(|_| ())));
    Some(created)
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
