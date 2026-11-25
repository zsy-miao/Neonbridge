//! Offline platform services for a single local console user.
//!
//! Slot numbers follow the GDK header order observed in the title's wrappers
//! (see evidence/matrix-awakens/runtime-wrappers.csv and the runtime probe).
//! Only behaviour a title can observe offline is modelled: one signed-in adult
//! user with every privilege, a fully installed package, a local storage
//! folder, no network connectivity and an SDR display.
use super::{INVALID, OK, Object, POINTER, queue};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{LazyLock, Mutex};

const E_PENDING: i32 = 0x8000000Au32 as i32;
const NOT_FOUND: i32 = 0x80070490u32 as i32;
const INSUFFICIENT_BUFFER: i32 = 0x8007007au32 as i32;
const USER_ID: u64 = 0x0009_0000_0000_1001;
const LOCAL_ID: u64 = 1;

/// The single local user. A user handle is the address of this value.
#[repr(C)]
struct User {
    magic: u64,
}
static USER: User = User { magic: 0x5458_4255_5345_5221 };

fn user_handle() -> *mut c_void {
    core::ptr::from_ref(&USER).cast_mut().cast()
}

fn is_user(handle: *mut c_void) -> bool {
    handle == user_handle()
}

// ---------------------------------------------------------------------------
// Asynchronous operations started by these services. XAsyncBlock layout:
// queue handle, context, completion callback, then internal storage.

#[derive(Clone, Copy)]
pub(super) enum Payload {
    Empty,
    User,
    SaveProvider,
}

struct Operation {
    status: i32,
    size: usize,
    payload: Payload,
}

static OPERATIONS: LazyLock<Mutex<HashMap<usize, Operation>>> = LazyLock::new(Default::default);

type Completion = unsafe extern "system" fn(*mut c_void);

unsafe extern "system" fn deliver(block: *mut c_void, _canceled: bool) {
    // SAFETY: the guest keeps its XAsyncBlock alive until completion.
    let callback = unsafe { block.cast::<usize>().add(2).read() };
    if callback != 0 {
        let callback: Completion = unsafe { core::mem::transmute(callback) };
        unsafe { callback(block) };
    }
}

/// Record a completed operation and run the block's callback on its queue.
pub(super) unsafe fn complete(block: *mut c_void, status: i32, size: usize, payload: Payload) -> i32 {
    if block.is_null() {
        return INVALID;
    }
    OPERATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(block as usize, Operation { status, size, payload });
    // SAFETY: a nonnull XAsyncBlock begins with its queue handle.
    let queue = unsafe { block.cast::<usize>().read() };
    if unsafe { queue::submit_completion(queue, block, deliver) } != OK {
        // No usable queue: deliver synchronously, as the console does when the
        // completion port is immediate.
        unsafe { deliver(block, false) };
    }
    OK
}

/// XAsyncRun(async, work) (threading slot 6): run `work(async)` on the block's
/// work port, then complete with its result.
pub(super) unsafe extern "system" fn async_run(_: &Object, block: *mut c_void, work: *mut c_void) -> i32 {
    if block.is_null() || work.is_null() {
        return INVALID;
    }
    type Work = unsafe extern "system" fn(*mut c_void) -> i32;
    unsafe extern "system" fn run(record: *mut c_void, _canceled: bool) {
        // SAFETY: allocated below and consumed exactly once.
        let record = unsafe { Box::from_raw(record.cast::<(usize, usize)>()) };
        let block = record.0 as *mut c_void;
        let work: Work = unsafe { core::mem::transmute(record.1) };
        let status = unsafe { work(block) };
        unsafe { complete(block, status, 0, Payload::Empty) };
    }
    OPERATIONS.lock().unwrap_or_else(|e| e.into_inner())
        .insert(block as usize, Operation { status: E_PENDING, size: 0, payload: Payload::Empty });
    let queue = unsafe { block.cast::<usize>().read() };
    let record = Box::into_raw(Box::new((block as usize, work as usize)));
    let status = unsafe { queue::submit_port(queue, 0, record.cast(), run, std::time::Duration::ZERO) };
    if status != OK {
        unsafe { drop(Box::from_raw(record)) };
        OPERATIONS.lock().unwrap_or_else(|e| e.into_inner()).remove(&(block as usize));
    }
    status
}

/// XAsyncGetStatus (threading slot 3) for operations started here.
pub(super) unsafe extern "system" fn async_status(_: &Object, block: *mut c_void, wait: bool) -> i32 {
    loop {
        let status = match OPERATIONS.lock().unwrap_or_else(|e| e.into_inner()).get(&(block as usize)) {
            Some(operation) => operation.status,
            None => {
                super::report(format_args!("XAsyncGetStatus for an operation not started by TuxBox"));
                return INVALID;
            }
        };
        if status != E_PENDING || !wait {
            return status;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

/// XAsyncGetResultSize (threading slot 4).
pub(super) unsafe extern "system" fn async_result_size(_: &Object, block: *mut c_void, size: *mut usize) -> i32 {
    if size.is_null() {
        return POINTER;
    }
    let operations = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(operation) = operations.get(&(block as usize)) else { return INVALID };
    unsafe { size.write(operation.size) };
    operation.status
}

/// XAsyncCancel (threading slot 5). Every operation here completes at once.
pub(super) extern "system" fn async_cancel(_: &Object, _block: *mut c_void) {}

pub(super) fn take(block: *mut c_void) -> Result<Payload, i32> {
    let mut operations = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    match operations.get(&(block as usize)) {
        None => Err(INVALID),
        Some(operation) if operation.status == E_PENDING => Err(E_PENDING),
        Some(operation) if operation.status < 0 => Err(operation.status),
        Some(_) => Ok(operations.remove(&(block as usize)).map(|o| o.payload).ok_or(INVALID)?),
    }
}

// ---------------------------------------------------------------------------
// XUser (class 01acd177)

pub(super) unsafe extern "system" fn user_duplicate(_: &Object, user: *mut c_void, out: *mut *mut c_void) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(user_handle()) };
    OK
}
pub(super) extern "system" fn user_close(_: &Object, _user: *mut c_void) {}
pub(super) extern "system" fn user_compare(_: &Object, a: *mut c_void, b: *mut c_void) -> i32 {
    (a as isize).cmp(&(b as isize)) as i32
}
pub(super) unsafe extern "system" fn user_max_users(_: &Object, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(1) };
    OK
}
/// XUserAddAsync: every add request signs in the same local user.
pub(super) unsafe extern "system" fn user_add_async(_: &Object, options: u32, block: *mut c_void) -> i32 {
    super::report(format_args!("XUserAddAsync options {options:#x}: signing in the local user"));
    unsafe { complete(block, OK, 0, Payload::User) }
}
pub(super) unsafe extern "system" fn user_add_result(_: &Object, block: *mut c_void, out: *mut *mut c_void) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::User) => {
            unsafe { out.write(user_handle()) };
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
pub(super) unsafe extern "system" fn user_local_id(_: &Object, user: *mut c_void, out: *mut u64) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(LOCAL_ID) };
    OK
}
pub(super) unsafe extern "system" fn user_find_by_local_id(_: &Object, id: u64, out: *mut *mut c_void) -> i32 {
    if out.is_null() { return POINTER; }
    if id != LOCAL_ID { return NOT_FOUND; }
    unsafe { out.write(user_handle()) };
    OK
}
pub(super) unsafe extern "system" fn user_id(_: &Object, user: *mut c_void, out: *mut u64) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(USER_ID) };
    OK
}
pub(super) unsafe extern "system" fn user_find_by_id(_: &Object, id: u64, out: *mut *mut c_void) -> i32 {
    if out.is_null() { return POINTER; }
    if id != USER_ID { return NOT_FOUND; }
    unsafe { out.write(user_handle()) };
    OK
}
pub(super) unsafe extern "system" fn user_is_guest(_: &Object, user: *mut c_void, out: *mut bool) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(false) };
    OK
}
/// XUserState::SignedIn is 0.
pub(super) unsafe extern "system" fn user_state(_: &Object, user: *mut c_void, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(0) };
    OK
}
/// XUserCheckPrivilege: every privilege is granted offline.
pub(super) unsafe extern "system" fn user_check_privilege(
    _: &Object, user: *mut c_void, _options: u32, _privilege: u32, has: *mut bool, reason: *mut u32,
) -> i32 {
    if has.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { has.write(true) };
    if !reason.is_null() { unsafe { reason.write(0) } };
    OK
}
/// XUserRegisterForChangeEvent: the single user never changes.
pub(super) unsafe extern "system" fn user_register_change(
    _: &Object, _queue: usize, _context: *mut c_void, _callback: *mut c_void, token: *mut u64,
) -> i32 {
    if token.is_null() { return POINTER; }
    unsafe { token.write(next_token()) };
    OK
}
pub(super) extern "system" fn user_unregister_change(_: &Object, _token: u64, _wait: bool) -> bool {
    true
}

fn next_token() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static TOKEN: AtomicU64 = AtomicU64::new(0x100);
    TOKEN.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// XGame (class 973a344e)

fn title_id() -> u32 {
    static ID: LazyLock<u32> = LazyLock::new(|| {
        let config = std::env::current_exe().ok()
            .and_then(|exe| std::fs::read_to_string(exe.with_file_name("MicrosoftGame.Config")).ok())
            .unwrap_or_default();
        config.split_once("<TitleId>")
            .and_then(|(_, rest)| rest.split_once('<'))
            .and_then(|(id, _)| u32::from_str_radix(id.trim(), 16).ok())
            .unwrap_or(0)
    });
    *ID
}

pub(super) unsafe extern "system" fn game_title_id(_: &Object, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(title_id()) };
    OK
}

// ---------------------------------------------------------------------------
// XPackage (class af406016)

fn package_name() -> String {
    let config = std::env::current_exe().ok()
        .and_then(|exe| std::fs::read_to_string(exe.with_file_name("MicrosoftGame.Config")).ok())
        .unwrap_or_default();
    config.split_once("<Identity Name=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or_else(|| format!("{:08X}", title_id()), |(name, _)| name.to_owned())
}

/// XPackageGetCurrentProcessPackageIdentifier(size, buffer).
pub(super) unsafe extern "system" fn package_identifier(_: &Object, size: usize, buffer: *mut u8) -> i32 {
    super::report(format_args!("called package_identifier"));
    if buffer.is_null() { return POINTER; }
    let mut name = package_name().into_bytes();
    name.truncate(32);
    if size <= name.len() { return INSUFFICIENT_BUFFER; }
    unsafe {
        core::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len());
        buffer.add(name.len()).write(0);
    }
    OK
}
/// XPackageIsPackagedProcess.
pub(super) extern "system" fn package_is_packaged(_: &Object) -> bool {
    super::report(format_args!("called package_is_packaged"));
    // TUXBOX_PACKAGED=1 reports a packaged process (bisecting the packaged path).
    std::env::var_os("TUXBOX_PACKAGED").is_some_and(|v| v == "1")
}
// Installation monitors report a complete, launchable package. Their
// notifications run on the queue named at creation.
static MONITOR: u8 = 0;
static MONITOR_QUEUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// XPackageCreateInstallationMonitor(identifier, selectorCount, selectors,
/// minimumUpdateIntervalMs, queue, monitor*).
pub(super) unsafe extern "system" fn package_create_monitor(
    _: &Object, _identifier: *const u8, _count: u32, _selectors: *const c_void, _interval: u32,
    queue: usize, out: *mut *const u8,
) -> i32 {
    MONITOR_QUEUE.store(queue, std::sync::atomic::Ordering::Relaxed);
    super::report(format_args!("called package_create_monitor"));
    if out.is_null() { return POINTER; }
    unsafe { out.write(&raw const MONITOR) };
    OK
}
/// XPackageCloseInstallationMonitorHandle.
pub(super) extern "system" fn package_close_monitor(_: &Object, _monitor: *const u8) { super::report(format_args!("called package_close_monitor")); }
/// XPackageGetInstallationProgress: { installedBytes, totalBytes: u64;
/// launchable, completed: bool }.
pub(super) unsafe extern "system" fn package_progress(_: &Object, _monitor: *const u8, out: *mut u8) {
    super::report(format_args!("called package_progress"));
    if out.is_null() { return; }
    unsafe {
        out.write_bytes(0, 24);
        out.cast::<u64>().write_unaligned(1);
        out.add(8).cast::<u64>().write_unaligned(1);
        out.add(16).write(1);
        out.add(17).write(1);
    }
}
/// XPackageRegisterInstallationProgressChanged(monitor, context, callback, token):
/// the package is already complete, so report that once on the process queue.
pub(super) unsafe extern "system" fn package_register_progress(
    _: &Object, monitor: *const u8, context: *mut c_void, callback: *mut c_void, token: *mut u64,
) -> i32 {
    super::report(format_args!("called package_register_progress"));
    if callback.is_null() || token.is_null() { return POINTER; }
    type Changed = unsafe extern "system" fn(*mut c_void, *const u8);
    unsafe extern "system" fn notify(record: *mut c_void, _canceled: bool) {
        // SAFETY: allocated below and consumed exactly once.
        let record = unsafe { Box::from_raw(record.cast::<(usize, usize, usize)>()) };
        let callback: Changed = unsafe { core::mem::transmute(record.0) };
        super::report(format_args!("delivering installation progress to callback {:#x}", record.0));
        unsafe { callback(record.1 as *mut c_void, record.2 as *const u8) };
        super::report(format_args!("installation progress callback returned"));
    }
    unsafe { token.write(next_token()) };
    let record = Box::into_raw(Box::new((callback as usize, context as usize, monitor as usize)));
    // Deliver after the title has recorded this registration's token.
    let queue = MONITOR_QUEUE.load(std::sync::atomic::Ordering::Relaxed);
    super::report(format_args!("installation progress queued on queue {queue:#x}"));
    if unsafe { queue::submit_completion_after(queue, record.cast(), notify, std::time::Duration::from_millis(100)) } != OK {
        unsafe { drop(Box::from_raw(record)) };
    }
    OK
}

/// XPackageEnumeratePackages(kind, scope, context, callback). Offline there is
/// no installed DLC or related package to report; the title enumerates
/// Content/ThisOnly and an empty, successful enumeration means "none owned".
pub(super) extern "system" fn package_enumerate(
    _: &Object, kind: u32, scope: u32, _context: *mut c_void, callback: *mut c_void,
) -> i32 {
    super::report(format_args!("called package_enumerate kind {kind} scope {scope}"));
    if callback.is_null() { return POINTER; }
    OK
}

/// XUserGetGamertag(user, component, size, buffer, used) on interface cef4fac0.
pub(super) unsafe extern "system" fn user_gamertag(
    _: &Object, user: *mut c_void, _component: u32, size: usize, buffer: *mut u8, used: *mut usize,
) -> i32 {
    const NAME: &[u8] = b"TuxBox\0";
    if !is_user(user) { return INVALID; }
    if !used.is_null() { unsafe { used.write(NAME.len()) } };
    if buffer.is_null() { return POINTER; }
    if size < NAME.len() { return INSUFFICIENT_BUFFER; }
    unsafe { core::ptr::copy_nonoverlapping(NAME.as_ptr(), buffer, NAME.len()) };
    OK
}

/// XUserGetAgeGroup: Adult (3).
pub(super) unsafe extern "system" fn user_age_group(_: &Object, user: *mut c_void, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(3) };
    OK
}

/// XPackageUnregisterInstallationProgressChanged and similar unregistrations.
pub(super) extern "system" fn unregister(_: &Object, _monitor: usize, _token: u64, _wait: bool) -> bool {
    super::report(format_args!("called unregister"));
    true
}

/// XSystemGetConsoleId(size, buffer, used): a stable local identifier.
pub(super) unsafe extern "system" fn system_console_id(_: &Object, size: usize, buffer: *mut u8, used: *mut usize) -> i32 {
    const ID: &[u8] = b"54555842-4F58-4C4F-4341-4C434F4E534F\0";
    let id = &ID[..ID.len()];
    if !used.is_null() { unsafe { used.write(id.len()) } };
    if buffer.is_null() { return POINTER; }
    if size < id.len() { return INSUFFICIENT_BUFFER; }
    unsafe { core::ptr::copy_nonoverlapping(id.as_ptr(), buffer, id.len()) };
    OK
}

/// XSystemGetAppSpecificDeviceId(size, buffer, used). ProjectScarlett passes
/// size 45, XSystemAppSpecificDeviceIdBytes: a base64 SHA-256 (44 chars) and
/// its terminator. A fixed local identifier, like the console id.
pub(super) unsafe extern "system" fn system_app_device_id(_: &Object, size: usize, buffer: *mut u8, used: *mut usize) -> i32 {
    const ID: &[u8] = b"VHV4Qm94TG9jYWxBcHBTcGVjaWZpY0RldmljZUlkMDA=\0";
    if !used.is_null() { unsafe { used.write(ID.len()) } };
    if buffer.is_null() { return POINTER; }
    if size < ID.len() { return INSUFFICIENT_BUFFER; }
    unsafe { core::ptr::copy_nonoverlapping(ID.as_ptr(), buffer, ID.len()) };
    OK
}

// ---------------------------------------------------------------------------
// XPersistentLocalStorage (class f4faf4d4)

fn storage_path() -> String {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\users\\Public".into());
    let path = format!("{base}\\TuxBox\\PersistentLocalStorage\\{:08X}\\", title_id());
    let _ = std::fs::create_dir_all(&path);
    path
}

/// XPersistentLocalStorageGetPath(size, buffer, used).
pub(super) unsafe extern "system" fn storage_get_path(_: &Object, size: usize, buffer: *mut u8, used: *mut usize) -> i32 {
    let path = storage_path();
    let needed = path.len() + 1;
    if !used.is_null() { unsafe { used.write(needed) } };
    if buffer.is_null() { return POINTER; }
    if size < needed { return INSUFFICIENT_BUFFER; }
    unsafe {
        core::ptr::copy_nonoverlapping(path.as_ptr(), buffer, path.len());
        buffer.add(path.len()).write(0);
    }
    super::report(format_args!("persistent local storage at {path}"));
    OK
}

// ---------------------------------------------------------------------------
// XNetworking (class 37e56907) and XDisplay (class 03f0fe74)

/// XNetworkingGetConnectivityHint: initialized, no connectivity.
pub(super) unsafe extern "system" fn network_hint(_: &Object, out: *mut u8) -> i32 {
    if out.is_null() { return POINTER; }
    // { level, cost, ianaInterfaceType: u32; networkInitialized,
    //   approachingDataLimit, overDataLimit, roaming: bool } = 16 bytes.
    unsafe {
        out.write_bytes(0, 16);
        out.cast::<u32>().write_unaligned(1); // XNetworkingConnectivityLevelHint::None
        out.add(12).write(1); // networkInitialized
    }
    OK
}
pub(super) unsafe extern "system" fn network_register(
    _: &Object, _queue: usize, _context: *mut c_void, _callback: *mut c_void, token: *mut u64,
) -> i32 {
    if !token.is_null() { unsafe { token.write(next_token()) } };
    OK
}

/// XDisplayTryEnableHdrMode: XDisplayHdrModeResult::Disabled (2).
pub(super) extern "system" fn display_try_hdr(_: &Object, _preference: u32, _info: *mut c_void) -> u32 {
    2
}

// ---------------------------------------------------------------------------
// XGameSave (class 704c3f58). One provider per process, stored in a folder
// beside the persistent local storage path.

static SAVE_PROVIDER: u8 = 0;

pub(super) fn save_root() -> String {
    let path = format!("{}GameSave\\", storage_path());
    let _ = std::fs::create_dir_all(&path);
    path
}

/// XGameSaveInitializeProvider(user, configurationId, syncOnDemand, provider*).
pub(super) unsafe extern "system" fn save_initialize(
    _: &Object, user: *mut c_void, _scid: *const u8, _sync_on_demand: bool, out: *mut *const u8,
) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    super::report(format_args!("game save provider at {}", save_root()));
    unsafe { out.write(&raw const SAVE_PROVIDER) };
    OK
}
/// XGameSaveInitializeProviderAsync(user, configurationId, syncOnDemand, async).
pub(super) unsafe extern "system" fn save_initialize_async(
    _: &Object, user: *mut c_void, _scid: *const u8, _sync_on_demand: bool, block: *mut c_void,
) -> i32 {
    if !is_user(user) { return INVALID; }
    super::report(format_args!("game save provider (async) at {}", save_root()));
    unsafe { complete(block, OK, 0, Payload::SaveProvider) }
}
/// XGameSaveInitializeProviderResult(async, provider*).
pub(super) unsafe extern "system" fn save_initialize_result(_: &Object, block: *mut c_void, out: *mut *const u8) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::SaveProvider) => {
            unsafe { out.write(&raw const SAVE_PROVIDER) };
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
pub(super) extern "system" fn save_close_provider(_: &Object, _provider: *const u8) {}
