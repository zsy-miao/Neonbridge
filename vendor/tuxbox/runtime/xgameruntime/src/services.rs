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

enum Payload {
    Empty,
    User,
    SaveProvider,
    Quota,
    Blobs(Vec<(String, Vec<u8>)>),
    Folder(String),
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
unsafe fn complete(block: *mut c_void, status: i32, size: usize, payload: Payload) -> i32 {
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

fn take(block: *mut c_void) -> Result<Payload, i32> {
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
    // gameinput's one auto A press for the player-selection prompt waits for this.
    // SAFETY: one process-wide flag; the readers only test for its presence.
    unsafe { std::env::set_var("TUXBOX_SIGNED_IN", "1") };
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

/// XPackageEnumeratePackages(kind, scope, context, callback) (slot 39 in the
/// title's GDK; 35 in older tables). Offline there is no add-on content, and the
/// game package itself is not listed for kind Content: report none, succeed.
pub(super) extern "system" fn package_enumerate(_: &Object, kind: u32, scope: u32, _context: *mut c_void, callback: *const c_void) -> i32 {
    super::report(format_args!("called package_enumerate kind {kind} scope {scope}"));
    if callback.is_null() { return POINTER; }
    OK
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

fn save_root() -> String {
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
    super::report(format_args!("game save: provider at {}", save_root()));
    unsafe { out.write(&raw const SAVE_PROVIDER) };
    OK
}
/// XGameSaveInitializeProviderAsync(user, configurationId, syncOnDemand, async).
pub(super) unsafe extern "system" fn save_initialize_async(
    _: &Object, user: *mut c_void, _scid: *const u8, _sync_on_demand: bool, block: *mut c_void,
) -> i32 {
    if !is_user(user) { return INVALID; }
    super::report(format_args!("game save: provider (async) at {}", save_root()));
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

// Containers live under the provider folder, one directory per container:
// `<percent-encoded name>\display` (the display name) and `blobs\<encoded
// blob name>`. A container exists once an update has been submitted for it.
// Slots follow the XGameSave.h declaration order; 4-6, 10, 13-14, 16-18, 20
// and 23-27 match the title's own wrappers (evidence runtime-wrappers.csv).

const BUFFER_TOO_SMALL: i32 = 0x8083_0007u32 as i32;
const BLOB_NOT_FOUND: i32 = 0x8083_0008u32 as i32;
const SAVE_QUOTA: i64 = 256 << 20;

#[repr(C)]
pub(super) struct ContainerInfo {
    name: *const i8,
    display: *const i8,
    blobs: u32,
    total: u64,
    modified: i64,
    needs_sync: bool,
}
#[repr(C)]
pub(super) struct BlobInfo {
    name: *const i8,
    size: u32,
}
#[repr(C)]
pub(super) struct Blob {
    info: BlobInfo,
    data: *mut u8,
}
type ContainerCallback = unsafe extern "system" fn(*const ContainerInfo, *mut c_void) -> bool;
type BlobCallback = unsafe extern "system" fn(*const BlobInfo, *mut c_void) -> bool;

struct Update {
    container: String,
    display: Option<String>,
    writes: Vec<(String, Vec<u8>)>,
    deletes: Vec<String>,
}

/// Open container handles (handle -> container name) and pending updates.
static CONTAINERS: LazyLock<Mutex<HashMap<usize, String>>> = LazyLock::new(Default::default);
static UPDATES: LazyLock<Mutex<HashMap<usize, Update>>> = LazyLock::new(Default::default);

fn new_handle() -> usize {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0x5341_5645_0010);
    NEXT.fetch_add(0x10, std::sync::atomic::Ordering::Relaxed)
}

unsafe fn text(pointer: *const i8) -> Option<String> {
    if pointer.is_null() { return None; }
    Some(unsafe { std::ffi::CStr::from_ptr(pointer) }.to_string_lossy().into_owned())
}

/// A NUL-terminated copy that lives for the process: names handed to title
/// callbacks and blob results must outlive the call.
fn intern(text: &str) -> *const i8 {
    static NAMES: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Default::default);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&pointer) = names.get(text) {
        return pointer as *const i8;
    }
    let pointer = std::ffi::CString::new(text.replace('\0', "")).unwrap_or_default().into_raw();
    names.insert(text.to_owned(), pointer as usize);
    pointer
}

fn encode(name: &str) -> String {
    let mut out = String::new();
    for (index, byte) in name.bytes().enumerate() {
        let plain = byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-' || (byte == b'.' && index != 0);
        if plain { out.push(byte as char) } else { out.push_str(&format!("%{byte:02X}")) }
    }
    out
}

fn decode(name: &str) -> String {
    let bytes = name.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%' && i + 2 < bytes.len())
            .then(|| std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()))
            .flatten();
        match hex {
            Some(byte) => { out.push(byte); i += 3; }
            None => { out.push(bytes[i]); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn container_dir(name: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(save_root()).join(encode(name))
}

fn list_containers() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(save_root()).into_iter().flatten().flatten()
        .filter(|e| e.path().join("blobs").is_dir())
        .map(|e| decode(&e.file_name().to_string_lossy()))
        .collect();
    names.sort();
    names
}

fn list_blobs(container: &str) -> Vec<(String, u64)> {
    let mut blobs: Vec<(String, u64)> = std::fs::read_dir(container_dir(container).join("blobs")).into_iter().flatten().flatten()
        .filter(|e| !e.file_name().to_string_lossy().ends_with(".partial"))
        .filter_map(|e| Some((decode(&e.file_name().to_string_lossy()), e.metadata().ok().filter(|m| m.is_file())?.len())))
        .collect();
    blobs.sort();
    blobs
}

/// Report one stored container to the title; false stops an enumeration.
unsafe fn emit_container(name: &str, context: *mut c_void, callback: Option<ContainerCallback>) -> bool {
    let dir = container_dir(name);
    let blobs = list_blobs(name);
    let display = std::fs::read_to_string(dir.join("display")).unwrap_or_default();
    let modified = std::fs::metadata(dir.join("blobs")).and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as i64);
    let info = ContainerInfo {
        name: intern(name),
        display: intern(&display),
        blobs: blobs.len() as u32,
        total: blobs.iter().map(|b| b.1).sum(),
        modified,
        needs_sync: false,
    };
    match callback {
        Some(callback) => unsafe { callback(&info, context) },
        None => true,
    }
}

/// XGameSaveGetRemainingQuota(provider, quota*) (slot 7).
pub(super) unsafe extern "system" fn save_quota(_: &Object, _provider: *const u8, out: *mut i64) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(SAVE_QUOTA) };
    OK
}
/// XGameSaveGetRemainingQuotaAsync(provider, async) (slot 8).
pub(super) unsafe extern "system" fn save_quota_async(_: &Object, _provider: *const u8, block: *mut c_void) -> i32 {
    unsafe { complete(block, OK, 0, Payload::Quota) }
}
/// XGameSaveGetRemainingQuotaResult(async, quota*) (slot 9).
pub(super) unsafe extern "system" fn save_quota_result(_: &Object, block: *mut c_void, out: *mut i64) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::Quota) => { unsafe { out.write(SAVE_QUOTA) }; OK }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}

fn delete_container(name: &str) -> i32 {
    match std::fs::remove_dir_all(container_dir(name)) {
        Ok(()) => OK,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OK,
        Err(error) => {
            super::report(format_args!("game save: deleting container {name:?} failed: {error}"));
            INVALID
        }
    }
}
/// XGameSaveDeleteContainer(provider, name) (slot 10).
pub(super) unsafe extern "system" fn save_delete(_: &Object, _provider: *const u8, name: *const i8) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    super::report(format_args!("game save: delete container {name:?}"));
    delete_container(&name)
}
/// XGameSaveDeleteContainerAsync(provider, name, async) (slot 11).
pub(super) unsafe extern "system" fn save_delete_async(_: &Object, _provider: *const u8, name: *const i8, block: *mut c_void) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    super::report(format_args!("game save: delete container {name:?} (async)"));
    let status = delete_container(&name);
    unsafe { complete(block, status, 0, Payload::Empty) }
}
/// XGameSaveDeleteContainerResult / XGameSaveSubmitUpdateResult(async) (slots 12, 29).
pub(super) unsafe extern "system" fn save_empty_result(_: &Object, block: *mut c_void) -> i32 {
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}

/// XGameSaveGetContainerInfo(provider, name, context, callback) (slot 13):
/// the callback runs once when the container exists.
pub(super) unsafe extern "system" fn save_container_info(
    _: &Object, _provider: *const u8, name: *const i8, context: *mut c_void, callback: Option<ContainerCallback>,
) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    if container_dir(&name).join("blobs").is_dir() {
        super::report(format_args!("game save: container info for {name:?}: found"));
        unsafe { emit_container(&name, context, callback) };
    } else {
        super::report(format_args!("game save: container info for {name:?}: none saved"));
    }
    OK
}
/// XGameSaveEnumerateContainerInfo(provider, context, callback) (slot 14).
pub(super) unsafe extern "system" fn save_enumerate_containers(
    _: &Object, _provider: *const u8, context: *mut c_void, callback: Option<ContainerCallback>,
) -> i32 {
    for name in list_containers() {
        if !unsafe { emit_container(&name, context, callback) } { break; }
    }
    OK
}
/// XGameSaveEnumerateContainerInfoByName(provider, prefix, context, callback) (slot 15).
pub(super) unsafe extern "system" fn save_enumerate_containers_by_name(
    _: &Object, _provider: *const u8, prefix: *const i8, context: *mut c_void, callback: Option<ContainerCallback>,
) -> i32 {
    let prefix = unsafe { text(prefix) }.unwrap_or_default();
    for name in list_containers().into_iter().filter(|n| n.starts_with(&prefix)) {
        if !unsafe { emit_container(&name, context, callback) } { break; }
    }
    OK
}

/// XGameSaveCreateContainer(provider, name, container*) (slot 16). Nothing is
/// written until an update is submitted.
pub(super) unsafe extern "system" fn save_create_container(_: &Object, _provider: *const u8, name: *const i8, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    let Some(name) = (unsafe { text(name) }).filter(|n| !n.is_empty()) else { return INVALID };
    let handle = new_handle();
    super::report(format_args!("game save: open container {name:?}"));
    CONTAINERS.lock().unwrap_or_else(|e| e.into_inner()).insert(handle, name);
    unsafe { out.write(handle) };
    OK
}
/// XGameSaveCloseContainer(container) (slot 17).
pub(super) extern "system" fn save_close_container(_: &Object, container: usize) {
    CONTAINERS.lock().unwrap_or_else(|e| e.into_inner()).remove(&container);
}

fn container_name(container: usize) -> Option<String> {
    CONTAINERS.lock().unwrap_or_else(|e| e.into_inner()).get(&container).cloned()
}

unsafe fn emit_blobs(container: usize, prefix: &str, context: *mut c_void, callback: Option<BlobCallback>) -> i32 {
    let Some(name) = container_name(container) else { return INVALID };
    let Some(callback) = callback else { return OK };
    for (blob, size) in list_blobs(&name).into_iter().filter(|b| b.0.starts_with(prefix)) {
        let info = BlobInfo { name: intern(&blob), size: size as u32 };
        if !unsafe { callback(&info, context) } { break; }
    }
    OK
}
/// XGameSaveEnumerateBlobInfo(container, context, callback) (slot 18).
pub(super) unsafe extern "system" fn save_enumerate_blobs(_: &Object, container: usize, context: *mut c_void, callback: Option<BlobCallback>) -> i32 {
    unsafe { emit_blobs(container, "", context, callback) }
}
/// XGameSaveEnumerateBlobInfoByName(container, prefix, context, callback) (slot 19).
pub(super) unsafe extern "system" fn save_enumerate_blobs_by_name(
    _: &Object, container: usize, prefix: *const i8, context: *mut c_void, callback: Option<BlobCallback>,
) -> i32 {
    let prefix = unsafe { text(prefix) }.unwrap_or_default();
    unsafe { emit_blobs(container, &prefix, context, callback) }
}

/// Read the named blobs (every blob when `names` is None).
fn read_blobs(container: &str, names: Option<Vec<String>>) -> Result<Vec<(String, Vec<u8>)>, i32> {
    let names = names.unwrap_or_else(|| list_blobs(container).into_iter().map(|b| b.0).collect());
    let dir = container_dir(container).join("blobs");
    names.into_iter().map(|name| {
        match std::fs::read(dir.join(encode(&name))) {
            Ok(data) => Ok((name, data)),
            Err(_) => {
                super::report(format_args!("game save: blob {name:?} not found in {container:?}"));
                Err(BLOB_NOT_FOUND)
            }
        }
    }).collect()
}

fn blobs_size(blobs: &[(String, Vec<u8>)]) -> usize {
    blobs.len() * size_of::<Blob>() + blobs.iter().map(|b| b.1.len()).sum::<usize>()
}

/// Lay the blobs out in the title's buffer: the XGameSaveBlob array, then the
/// data. Names are process-lifetime strings outside the buffer.
unsafe fn fill_blobs(blobs: &[(String, Vec<u8>)], size: usize, out: *mut Blob, count: *mut u32) -> i32 {
    if out.is_null() || count.is_null() { return POINTER; }
    if size < blobs_size(blobs) { return BUFFER_TOO_SMALL; }
    let mut data = unsafe { out.add(blobs.len()) }.cast::<u8>();
    for (index, (name, bytes)) in blobs.iter().enumerate() {
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, bytes.len());
            out.add(index).write(Blob { info: BlobInfo { name: intern(name), size: bytes.len() as u32 }, data });
            data = data.add(bytes.len());
        }
    }
    unsafe { count.write(blobs.len() as u32) };
    OK
}

/// XGameSaveReadBlobData(container, blobNames, countOfBlobs*, blobsSize, blobData) (slot 20).
/// Null names read every blob of the container.
pub(super) unsafe extern "system" fn save_read_blobs(
    _: &Object, container: usize, names: *const *const i8, count: *mut u32, size: usize, out: *mut Blob,
) -> i32 {
    if count.is_null() { return POINTER; }
    let Some(name) = container_name(container) else { return INVALID };
    let names = if names.is_null() { None } else {
        Some((0..unsafe { count.read() } as usize).filter_map(|i| unsafe { text(names.add(i).read()) }).collect())
    };
    match read_blobs(&name, names) {
        Ok(blobs) => {
            let status = unsafe { fill_blobs(&blobs, size, out, count) };
            super::report(format_args!("game save: read {} blobs ({} bytes) from {name:?}: {status:#x}",
                blobs.len(), blobs.iter().map(|b| b.1.len()).sum::<usize>()));
            status
        }
        Err(status) => status,
    }
}
/// XGameSaveReadBlobDataAsync(container, blobNames, countOfBlobs, async) (slot 21).
pub(super) unsafe extern "system" fn save_read_blobs_async(
    _: &Object, container: usize, names: *const *const i8, count: u32, block: *mut c_void,
) -> i32 {
    let Some(name) = container_name(container) else { return INVALID };
    let names = if names.is_null() { None } else {
        Some((0..count as usize).filter_map(|i| unsafe { text(names.add(i).read()) }).collect())
    };
    match read_blobs(&name, names) {
        Ok(blobs) => unsafe { complete(block, OK, blobs_size(&blobs), Payload::Blobs(blobs)) },
        Err(status) => unsafe { complete(block, status, 0, Payload::Empty) },
    }
}
/// XGameSaveReadBlobDataResult(async, blobsSize, blobData, countOfBlobs*) (slot 22).
pub(super) unsafe extern "system" fn save_read_blobs_result(_: &Object, block: *mut c_void, size: usize, out: *mut Blob, count: *mut u32) -> i32 {
    match take(block) {
        Ok(Payload::Blobs(blobs)) => unsafe { fill_blobs(&blobs, size, out, count) },
        Ok(_) => INVALID,
        Err(status) => status,
    }
}

/// XGameSaveCreateUpdate(container, displayName, update*) (slot 23).
pub(super) unsafe extern "system" fn save_create_update(_: &Object, container: usize, display: *const i8, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    let Some(name) = container_name(container) else { return INVALID };
    let handle = new_handle();
    UPDATES.lock().unwrap_or_else(|e| e.into_inner())
        .insert(handle, Update { container: name, display: unsafe { text(display) }, writes: Vec::new(), deletes: Vec::new() });
    unsafe { out.write(handle) };
    OK
}
/// XGameSaveCloseUpdate(update) (slot 24).
pub(super) extern "system" fn save_close_update(_: &Object, update: usize) {
    UPDATES.lock().unwrap_or_else(|e| e.into_inner()).remove(&update);
}
/// XGameSaveSubmitBlobWrite(update, blobName, data, byteCount) (slot 25).
pub(super) unsafe extern "system" fn save_blob_write(_: &Object, update: usize, name: *const i8, data: *const u8, bytes: usize) -> i32 {
    let Some(name) = (unsafe { text(name) }).filter(|n| !n.is_empty()) else { return INVALID };
    if data.is_null() && bytes != 0 { return POINTER; }
    let copy = if bytes == 0 { Vec::new() } else { unsafe { core::slice::from_raw_parts(data, bytes) }.to_vec() };
    match UPDATES.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&update) {
        Some(pending) => { pending.writes.push((name, copy)); OK }
        None => INVALID,
    }
}
/// XGameSaveSubmitBlobDelete(update, blobName) (slot 26).
pub(super) unsafe extern "system" fn save_blob_delete(_: &Object, update: usize, name: *const i8) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    match UPDATES.lock().unwrap_or_else(|e| e.into_inner()).get_mut(&update) {
        Some(pending) => { pending.deletes.push(name); OK }
        None => INVALID,
    }
}

/// Apply a pending update: each blob is written beside its target and renamed
/// over it, so a crash mid-save keeps the previous blob.
fn submit(update: usize) -> i32 {
    let updates = UPDATES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pending) = updates.get(&update) else { return INVALID };
    let dir = container_dir(&pending.container);
    let blobs = dir.join("blobs");
    let result = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&blobs)?;
        for name in &pending.deletes {
            match std::fs::remove_file(blobs.join(encode(name))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        for (name, data) in &pending.writes {
            let target = blobs.join(encode(name));
            let partial = blobs.join(format!("{}.partial", encode(name)));
            std::fs::write(&partial, data)?;
            std::fs::rename(&partial, &target)?;
        }
        if let Some(display) = &pending.display {
            std::fs::write(dir.join("display"), display)?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            super::report(format_args!("game save: saved container {:?} ({} blobs written, {} deleted)",
                pending.container, pending.writes.len(), pending.deletes.len()));
            OK
        }
        Err(error) => {
            super::report(format_args!("game save: saving container {:?} failed: {error}", pending.container));
            INVALID
        }
    }
}
/// XGameSaveSubmitUpdate(update) (slot 27).
pub(super) extern "system" fn save_submit(_: &Object, update: usize) -> i32 {
    submit(update)
}
/// XGameSaveSubmitUpdateAsync(update, async) (slot 28).
pub(super) unsafe extern "system" fn save_submit_async(_: &Object, update: usize, block: *mut c_void) -> i32 {
    let status = submit(update);
    unsafe { complete(block, status, 0, Payload::Empty) }
}

/// XGameSaveFilesGetFolderWithUiAsync(user, configurationId, async) (slot 30):
/// a plain folder beside the containers, with no sign-in UI offline.
pub(super) unsafe extern "system" fn save_files_folder_async(_: &Object, user: *mut c_void, _scid: *const i8, block: *mut c_void) -> i32 {
    if !is_user(user) { return INVALID; }
    let path = format!("{}SaveFiles", storage_path());
    let _ = std::fs::create_dir_all(&path);
    super::report(format_args!("game save: files folder at {path}"));
    unsafe { complete(block, OK, path.len() + 1, Payload::Folder(path)) }
}
/// XGameSaveFilesGetFolderWithUiResult(async, folderSize, folderResult) (slot 31).
pub(super) unsafe extern "system" fn save_files_folder_result(_: &Object, block: *mut c_void, size: usize, out: *mut u8) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::Folder(path)) => {
            if size < path.len() + 1 { return INSUFFICIENT_BUFFER; }
            unsafe {
                core::ptr::copy_nonoverlapping(path.as_ptr(), out, path.len());
                out.add(path.len()).write(0);
            }
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
/// XGameSaveFilesGetRemainingQuota(user, configurationId, quota*) (slot 32).
pub(super) unsafe extern "system" fn save_files_quota(_: &Object, _user: *mut c_void, _scid: *const i8, out: *mut i64) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(SAVE_QUOTA) };
    OK
}
