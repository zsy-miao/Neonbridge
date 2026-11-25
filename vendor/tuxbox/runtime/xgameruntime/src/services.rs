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
    Button(u32),
    Text(String),
}

struct Operation {
    status: i32,
    size: usize,
    payload: Payload,
}

static OPERATIONS: LazyLock<Mutex<HashMap<usize, Operation>>> = LazyLock::new(Default::default);

type Completion = unsafe extern "system" fn(*mut c_void);

/// The completion routine a finished operation calls, read from the block
/// before its status is published (VIctory d01b170, GPL-3, credit to its
/// authors). Once the status reads complete, a title polling the block may
/// begin another call on it from another thread, so a routine read when the
/// completion runs could be that next call's, fired for this one.
struct Delivery {
    block: usize,
    callback: usize,
}

unsafe extern "system" fn deliver(record: *mut c_void, _canceled: bool) {
    // SAFETY: boxed in `complete` and consumed exactly once.
    let Delivery { block, callback } = *unsafe { Box::from_raw(record.cast::<Delivery>()) };
    if callback != 0 {
        let callback: Completion = unsafe { core::mem::transmute(callback) };
        unsafe { callback(block as *mut c_void) };
    }
}

fn callback_at_completion() -> bool {
    static ON: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_XASYNC_EARLY_CALLBACK").map_or(true, |v| v.trim() != "0"));
    *ON
}

/// Record a completed operation and run the block's callback on its queue.
/// TUXBOX_XASYNC_EARLY_CALLBACK=0 reads the callback after publishing the status.
unsafe fn complete(block: *mut c_void, status: i32, size: usize, payload: Payload) -> i32 {
    if block.is_null() {
        return INVALID;
    }
    // SAFETY: the guest keeps its XAsyncBlock alive until completion; its
    // third word is the completion routine.
    let read = || unsafe { block.cast::<usize>().add(2).read() };
    let early = callback_at_completion().then(read);
    OPERATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(block as usize, Operation { status, size, payload });
    let callback = early.unwrap_or_else(read);
    // SAFETY: a nonnull XAsyncBlock begins with its queue handle.
    let queue = unsafe { block.cast::<usize>().read() };
    let record = Box::into_raw(Box::new(Delivery { block: block as usize, callback })).cast::<c_void>();
    if unsafe { queue::submit_completion(queue, record, deliver) } != OK {
        // No usable queue: deliver synchronously, as the console does when the
        // completion port is immediate.
        unsafe { deliver(record, false) };
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
unsafe extern "system" fn user_add_result_impl(_: &Object, block: *mut c_void, out: *mut *mut c_void) -> i32 {
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
unsafe extern "system" fn user_id_impl(_: &Object, user: *mut c_void, out: *mut u64) -> i32 {
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
unsafe extern "system" fn user_is_guest_impl(_: &Object, user: *mut c_void, out: *mut bool) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(false) };
    OK
}
/// XUserState::SignedIn is 0.
unsafe extern "system" fn user_state_impl(_: &Object, user: *mut c_void, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    if !is_user(user) { return INVALID; }
    unsafe { out.write(0) };
    OK
}
/// XUserCheckPrivilege: every privilege is granted offline.
unsafe extern "system" fn user_check_privilege_impl(
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
unsafe extern "system" fn user_gamertag_impl(
    _: &Object, user: *mut c_void, component: u32, size: usize, buffer: *mut u8, used: *mut usize,
) -> i32 {
    // XUserGamertagComponent: 0 classic, 1 modern, 2 modern suffix, 3 unique
    // modern. The name has no suffix, so the suffix is empty and the unique
    // modern form is the name itself. TUXBOX_GAMERTAG_SUFFIX=0 answers the
    // name for every component (old behaviour).
    let suffix = component == 2 && std::env::var("TUXBOX_GAMERTAG_SUFFIX").map_or(true, |v| v.trim() != "0");
    let name: &[u8] = if suffix { b"\0" } else { b"TuxBox\0" };
    if !is_user(user) { return INVALID; }
    if !used.is_null() { unsafe { used.write(name.len()) } };
    if buffer.is_null() { return POINTER; }
    if size < name.len() { return INSUFFICIENT_BUFFER; }
    unsafe { core::ptr::copy_nonoverlapping(name.as_ptr(), buffer, name.len()) };
    OK
}

/// XUserGetAgeGroup: Adult (3).
unsafe extern "system" fn user_age_group_impl(_: &Object, user: *mut c_void, out: *mut u32) -> i32 {
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
    if size < needed {
        // The console answers a short buffer with E_FAIL (the PC runtime with
        // ERROR_INSUFFICIENT_BUFFER; TUXBOX_STORAGE_PATH_PC=1 answers that).
        // VIctory 376b3cf.
        return if std::env::var("TUXBOX_STORAGE_PATH_PC").is_ok_and(|v| v.trim() == "1") { INSUFFICIENT_BUFFER } else { 0x80004005u32 as i32 };
    }
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
unsafe extern "system" fn network_hint_impl(_: &Object, out: *mut u8) -> i32 {
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
const OUT_OF_LOCAL_STORAGE: i32 = 0x8083_0003u32 as i32;
const QUOTA_EXCEEDED: i32 = 0x8083_0006u32 as i32;

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
        .filter(|e| !is_staging(&e.file_name().to_string_lossy()))
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
    unsafe { out.write(remaining_quota(&save_root())) };
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
        Ok(Payload::Quota) => { unsafe { out.write(remaining_quota(&save_root())) }; OK }
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
    // The staging suffix is reserved: a blob named like one would be hidden.
    if atomic_saves() && is_staging(&encode(&name)) { return INVALID; }
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

fn tree_bytes(path: &std::path::Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries.flatten().map(|entry| match entry.metadata() {
        Ok(meta) if meta.is_dir() => tree_bytes(&entry.path()),
        Ok(meta) => meta.len(),
        Err(_) => 0,
    }).sum()
}

/// Space for an update, checked as the console does before anything is written.
/// Over the 256 MB title quota (counting the blobs it replaces or deletes as
/// freed) is E_GS_QUOTA_EXCEEDED; within the quota but over the host disk's
/// free space is E_GS_OUT_OF_LOCAL_STORAGE, so the title does not tell the
/// player to delete saves. The disk check credits nothing: each new blob is
/// written before the one it replaces is renamed away. TUXBOX_SAVE_SPACE_CHECK=0
/// skips both (VIctory f0ee35e, re-derived).
fn space_check(pending: &Update, blobs: &std::path::Path) -> Option<i32> {
    if std::env::var("TUXBOX_SAVE_SPACE_CHECK").is_ok_and(|v| v.trim() == "0") {
        return None;
    }
    let size = |name: &String| std::fs::metadata(blobs.join(encode(name))).map_or(0, |m| m.len());
    let incoming: u64 = pending.writes.iter().map(|(_, data)| data.len() as u64).sum();
    let mut replaced: Vec<&String> = pending.writes.iter().map(|(name, _)| name).chain(&pending.deletes).collect();
    replaced.sort();
    replaced.dedup();
    let freed: u64 = replaced.into_iter().map(size).sum();
    let used = tree_bytes(std::path::Path::new(&save_root()));
    if used.saturating_sub(freed) + incoming > SAVE_QUOTA as u64 {
        return Some(QUOTA_EXCEEDED);
    }
    let free = disk_free(blobs)?;
    // A small margin for the display name and file system bookkeeping.
    (incoming + (64 << 10) > free).then_some(OUT_OF_LOCAL_STORAGE)
}

/// Free bytes on the host disk holding `path` (its nearest existing ancestor).
fn disk_free(path: &std::path::Path) -> Option<u64> {
    let mut probe = path.to_path_buf();
    while !probe.exists() {
        if !probe.pop() { return None; }
    }
    let wide: Vec<u16> = std::os::windows::ffi::OsStrExt::encode_wide(probe.as_os_str()).chain(Some(0)).collect();
    let mut free = 0u64;
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, core::ptr::null_mut(), core::ptr::null_mut())
    };
    (ok != 0).then_some(free)
}

/// What XGameSaveGetRemainingQuota answers: the 256 MiB title quota minus
/// what the saves already use, capped by the host disk's free space (ztrx
/// xgamesave.rs:393). A fixed 256 MiB told a title with saves on disk it had
/// room it did not. TUXBOX_SAVE_QUOTA_FIXED=1 answers 256 MiB as before.
/// `root` is the containers' folder, or SaveFiles' for slot 32.
fn remaining_quota(root: &str) -> i64 {
    if std::env::var("TUXBOX_SAVE_QUOTA_FIXED").is_ok_and(|v| v.trim() == "1") {
        return SAVE_QUOTA;
    }
    let root = std::path::Path::new(root);
    let left = (SAVE_QUOTA as u64).saturating_sub(tree_bytes(root));
    disk_free(root).map_or(left, |free| left.min(free)) as i64
}

/// Apply a pending update. Atomic (default): every write is staged first in a
/// file of its own, then deletes and renames are published together under the
/// cross-process save lock, each rename retried on sharing errors, so a crash
/// or a second process never sees half an update; a container or blob folder
/// redirected through a reparse point is refused. TUXBOX_GAMESAVE_ATOMIC=0 keeps
/// the per-blob rename (Xbox2Windows e1416e9e..a43e9f8f, re-derived).
fn submit(update: usize) -> i32 {
    if atomic_saves() { submit_atomic(update) } else { submit_direct(update) }
}

fn atomic_saves() -> bool {
    std::env::var("TUXBOX_GAMESAVE_ATOMIC").map_or(true, |v| v.trim() != "0")
}

/// Staging files carry `.partial` (matched case-insensitively) and are never
/// listed as blobs.
fn is_staging(file: &str) -> bool {
    file.to_ascii_lowercase().contains(".partial")
}

/// HRESULT_FROM_WIN32(ERROR_BUSY): the save lock is held elsewhere.
const SAVE_BUSY: i32 = 0x8007_00AAu32 as i32;

fn redirected(path: &std::path::Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// Exclusive LockFileEx on `<save root>/.lock`, released when the file closes.
/// Any failure that is not a grant (lock or sharing violation, IO_PENDING, or
/// anything else) is busy, retried briefly before giving up.
fn lock_saves() -> Result<std::fs::File, i32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    let path = std::path::Path::new(&save_root()).join(".lock");
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)
        .map_err(|_| SAVE_BUSY)?;
    for _ in 0..100 {
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { core::mem::zeroed() };
        let granted = unsafe {
            LockFileEx(file.as_raw_handle() as _, LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlapped)
        };
        if granted != 0 {
            return Ok(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(SAVE_BUSY)
}

/// Rename over the target, retried while Windows reports a sharing, lock or
/// access error (a scanner or reader holding the file).
fn rename_retry(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if matches!(e.raw_os_error(), Some(5 | 32 | 33)) && attempt < 20 => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10 * attempt));
            }
            other => return other,
        }
    }
}

fn submit_atomic(update: usize) -> i32 {
    use std::io::Write as _;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    // UPDATES stays held to the end, serialising every publish in this process.
    let updates = UPDATES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pending) = updates.get(&update) else { return INVALID };
    let dir = container_dir(&pending.container);
    let blobs = dir.join("blobs");
    if redirected(&dir) || redirected(&blobs) {
        super::report(format_args!("game save: container {:?} is redirected; refused", pending.container));
        return INVALID;
    }
    if let Some(status) = space_check(pending, &blobs) {
        super::report(format_args!("game save: container {:?} refused: {status:#x}", pending.container));
        return status;
    }
    if let Err(error) = std::fs::create_dir_all(&blobs) {
        super::report(format_args!("game save: saving container {:?} failed: {error}", pending.container));
        return INVALID;
    }
    let stage = |target: &std::path::Path, data: &[u8]| -> std::io::Result<std::path::PathBuf> {
        let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let staged = target.with_file_name(format!("{name}.partial-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::Relaxed)));
        let mut file = std::fs::File::create(&staged)?;
        file.write_all(data)?;
        file.sync_all()?;
        Ok(staged)
    };
    // 1. Stage every write (and the display name) before anything is published.
    let mut staged: Vec<(std::path::PathBuf, std::path::PathBuf)> = Vec::new();
    let mut targets: Vec<(std::path::PathBuf, &[u8])> = pending.writes.iter()
        .map(|(name, data)| (blobs.join(encode(name)), data.as_slice())).collect();
    if let Some(display) = &pending.display {
        targets.push((dir.join("display"), display.as_bytes()));
    }
    for (target, data) in targets {
        match stage(&target, data) {
            Ok(file) => staged.push((file, target)),
            Err(error) => {
                for (file, _) in &staged { let _ = std::fs::remove_file(file); }
                super::report(format_args!("game save: staging container {:?} failed: {error}", pending.container));
                return INVALID;
            }
        }
    }
    // 2. Publish deletes and renames together under the cross-process lock.
    let lock = match lock_saves() {
        Ok(lock) => lock,
        Err(status) => {
            for (file, _) in &staged { let _ = std::fs::remove_file(file); }
            super::report(format_args!("game save: container {:?} busy (save lock held)", pending.container));
            return status;
        }
    };
    let result = (|| -> std::io::Result<()> {
        for name in &pending.deletes {
            match std::fs::remove_file(blobs.join(encode(name))) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        for (file, target) in &staged {
            rename_retry(file, target)?;
        }
        Ok(())
    })();
    drop(lock);
    for (file, _) in &staged {
        let _ = std::fs::remove_file(file); // left over only after a failed rename
    }
    match result {
        Ok(()) => {
            super::report(format_args!("game save: saved container {:?} ({} blobs written, {} deleted)",
                pending.container, pending.writes.len(), pending.deletes.len()));
            OK
        }
        Err(error) => {
            super::report(format_args!("game save: publishing container {:?} failed: {error}", pending.container));
            INVALID
        }
    }
}

/// The per-blob path (TUXBOX_GAMESAVE_ATOMIC=0): each blob is written beside its
/// target and renamed over it, so a crash mid-save keeps the previous blob.
fn submit_direct(update: usize) -> i32 {
    let updates = UPDATES.lock().unwrap_or_else(|e| e.into_inner());
    let Some(pending) = updates.get(&update) else { return INVALID };
    let dir = container_dir(&pending.container);
    let blobs = dir.join("blobs");
    if let Some(status) = space_check(pending, &blobs) {
        super::report(format_args!("game save: container {:?} refused: {status:#x}", pending.container));
        return status;
    }
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
    // The async block is checked before anything is committed, so a refused
    // block never leaves a published update without a completion.
    if atomic_saves() && block.is_null() { return INVALID; }
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
    unsafe { out.write(remaining_quota(&format!("{}SaveFiles", storage_path()))) };
    OK
}

// ---------------------------------------------------------------------------
// XGameUi (class dfcd4649). There is no system UI offline: a message dialog
// answers its default button, an error dialog is dismissed, text entry answers
// TUXBOX_TEXT_ENTRY or the title's default text (TUXBOX_TEXT_ENTRY_CANCEL=1
// cancels it instead), and every other dialog (profile card, achievements,
// player picker, invites, web authentication, state share) completes as
// cancelled by the user (E_ABORT), which titles handle as a closed dialog.
// TUXBOX_RUNTIME_SERVICES without XGameUi withholds the class (old behaviour).

const ABORT: i32 = 0x80004004u32 as i32;

/// 3: XGameUiShowMessageDialogAsync(async, title, content, first, second, third, default, cancel).
#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "system" fn ui_message_dialog_async(
    _: &Object, block: *mut c_void, title: *const i8, content: *const i8,
    _first: *const i8, _second: *const i8, _third: *const i8, default: u32, _cancel: u32,
) -> i32 {
    let (title, content) = unsafe { (text(title).unwrap_or_default(), text(content).unwrap_or_default()) };
    super::report(format_args!("game dialog: \"{title}\": \"{content}\" answered with button {default}"));
    unsafe { complete(block, OK, 4, Payload::Button(default)) }
}
/// 4: XGameUiShowMessageDialogResult(async, button*).
pub(super) unsafe extern "system" fn ui_message_dialog_result(_: &Object, block: *mut c_void, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::Button(b)) => { unsafe { out.write(b) }; OK }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
/// 14: XGameUiShowErrorDialogAsync(async, errorCode, context).
pub(super) unsafe extern "system" fn ui_error_dialog_async(_: &Object, block: *mut c_void, code: i32, context: *const i8) -> i32 {
    let context = unsafe { text(context).unwrap_or_default() };
    super::report(format_args!("game error dialog: {:#010x} \"{context}\" dismissed", code as u32));
    unsafe { complete(block, OK, 0, Payload::Empty) }
}
/// Result slots whose outputs a cancelled dialog never fills (15 and the
/// cancelled dialogs' results): the operation's status.
pub(super) unsafe extern "system" fn ui_empty_result(_: &Object, block: *mut c_void) -> i32 {
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}
/// The other dialogs' Async slots: the XAsyncBlock comes first; they complete
/// as cancelled by the user.
pub(super) unsafe extern "system" fn ui_cancelled<const N: usize>(_: &Object, block: *mut c_void) -> i32 {
    super::report(format_args!("game UI slot {N}: no system UI offline, completed as cancelled (E_ABORT)"));
    unsafe { complete(block, ABORT, 0, Payload::Empty) }
}
/// 12 / 18 / 25: result-size and count slots (async, out*): the size of a
/// text answer, otherwise the operation's failure.
pub(super) unsafe extern "system" fn ui_result_size(_: &Object, block: *mut c_void, out: *mut u32) -> i32 {
    let operations = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(operation) = operations.get(&(block as usize)) else { return INVALID };
    if operation.status < 0 {
        return operation.status;
    }
    if out.is_null() { return POINTER; }
    // XGameUiShowTextEntryResultSize writes a uint32_t.
    unsafe { out.write(operation.size as u32) };
    OK
}
/// 16: XGameUiSetNotificationPositionHint(position): nothing to place.
pub(super) extern "system" fn ui_notification_hint(_: &Object, _position: u32) {}

/// 17: XGameUiShowTextEntryAsync(async, title, description, defaultText, inputScope, maxTextLength).
pub(super) unsafe extern "system" fn ui_text_entry_async(
    _: &Object, block: *mut c_void, title: *const i8, _description: *const i8, default: *const i8, _scope: u32, max: u32,
) -> i32 {
    let title = unsafe { text(title).unwrap_or_default() };
    if std::env::var("TUXBOX_TEXT_ENTRY_CANCEL").is_ok_and(|v| v.trim() == "1") {
        super::report(format_args!("text entry \"{title}\": cancelled (TUXBOX_TEXT_ENTRY_CANCEL=1)"));
        return unsafe { complete(block, ABORT, 0, Payload::Empty) };
    }
    let mut answer = std::env::var("TUXBOX_TEXT_ENTRY").ok().unwrap_or_else(|| unsafe { text(default) }.unwrap_or_default());
    if max > 0 {
        answer = answer.chars().take(max as usize).collect();
    }
    super::report(format_args!("text entry \"{title}\": answered \"{answer}\" (TUXBOX_TEXT_ENTRY sets the answer)"));
    let size = answer.len() + 1;
    unsafe { complete(block, OK, size, Payload::Text(answer)) }
}
/// 19: XGameUiShowTextEntryResult(async, bufferSize, buffer, bufferUsed*).
pub(super) unsafe extern "system" fn ui_text_entry_result(_: &Object, block: *mut c_void, size: u32, out: *mut u8, used: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    {
        let operations = OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(op) = operations.get(&(block as usize))
            && op.status >= 0
            && (size as usize) < op.size
        {
            return INSUFFICIENT_BUFFER;
        }
    }
    match take(block) {
        Ok(Payload::Text(answer)) => {
            unsafe {
                core::ptr::copy_nonoverlapping(answer.as_ptr(), out, answer.len());
                out.add(answer.len()).write(0);
                if !used.is_null() { used.write(answer.len() as u32 + 1) };
            }
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}

// ---------------------------------------------------------------------------
// XAccessibility (class 3e241977): closed captions off, high contrast off,
// speech-to-text display accepted and dropped, and a silent speech
// synthesizer: it creates handles and every stream is a short silent WAV, so
// narration code runs without a voice. TUXBOX_SPEECH=refuse makes
// XSpeechSynthesizerCreate fail instead (no synthesizer).

static SPEECH_HANDLES: LazyLock<Mutex<std::collections::HashSet<usize>>> = LazyLock::new(Default::default);

fn silent_wav() -> &'static [u8] {
    static WAV: LazyLock<Vec<u8>> = LazyLock::new(|| {
        // 100 ms of 16 kHz mono 16-bit PCM silence.
        let (rate, samples) = (16_000u32, 1_600u32);
        let data = samples * 2;
        let mut w = Vec::with_capacity(44 + data as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 2).to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data.to_le_bytes());
        w.resize(44 + data as usize, 0);
        w
    });
    &WAV
}

fn speech_handle_valid(h: usize) -> bool {
    SPEECH_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).contains(&h)
}

/// 3: XClosedCaptionGetProperties(properties*): three colours, edge, style,
/// font scale (float), enabled (bool): captions off at scale 1.
pub(super) unsafe extern "system" fn caption_properties(_: &Object, out: *mut u8) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe {
        core::ptr::write_bytes(out, 0, 25);
        out.add(20).cast::<f32>().write_unaligned(1.0);
    }
    OK
}
/// 4: XClosedCaptionSetEnabled(enabled).
pub(super) extern "system" fn caption_set_enabled(_: &Object, _enabled: bool) -> i32 { OK }
/// 5: XHighContrastGetMode(mode*): Off.
pub(super) unsafe extern "system" fn high_contrast(_: &Object, out: *mut u32) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(0) };
    OK
}
/// 6, 7, 18-20: speech-to-text display calls; there is no display to show them.
pub(super) extern "system" fn speech_to_text_ignored(_: &Object) -> i32 { OK }
/// 17: XSpeechToTextBeginHypothesisString(speaker, content, type, hypothesisId*).
pub(super) unsafe extern "system" fn speech_to_text_begin(_: &Object, _speaker: *const i8, _content: *const i8, _kind: u32, id: *mut u32) -> i32 {
    static NEXT_ID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
    if !id.is_null() {
        unsafe { id.write(NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) };
    }
    OK
}
/// 8: XSpeechSynthesizerEnumerateInstalledVoices(context, callback): no voices.
pub(super) extern "system" fn speech_enumerate_voices(_: &Object, _context: *mut c_void, _callback: *const c_void) -> i32 { OK }
/// 9: XSpeechSynthesizerCreate(handle*).
pub(super) unsafe extern "system" fn speech_create(_: &Object, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    if std::env::var("TUXBOX_SPEECH").is_ok_and(|v| v.trim().eq_ignore_ascii_case("refuse")) {
        unsafe { out.write(0) };
        return 0x80004005u32 as i32; // E_FAIL
    }
    let h = new_handle();
    SPEECH_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).insert(h);
    super::report(format_args!("speech synthesizer: silent synthesizer created (TUXBOX_SPEECH=refuse fails it)"));
    unsafe { out.write(h) };
    OK
}
/// 10 / 14: close a synthesizer or a stream.
pub(super) extern "system" fn speech_close(_: &Object, handle: usize) -> i32 {
    SPEECH_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).remove(&handle);
    OK
}
/// 11: XSpeechSynthesizerSetDefaultVoice(handle); 12: SetCustomVoice(handle, voiceId).
pub(super) extern "system" fn speech_set_voice(_: &Object, handle: usize) -> i32 {
    if speech_handle_valid(handle) { OK } else { INVALID }
}
/// 13 / 21: CreateStreamFromText / FromSsml(handle, text, stream*).
pub(super) unsafe extern "system" fn speech_create_stream(_: &Object, handle: usize, _text: *const i8, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    if !speech_handle_valid(handle) { return INVALID; }
    let h = new_handle();
    SPEECH_HANDLES.lock().unwrap_or_else(|e| e.into_inner()).insert(h);
    unsafe { out.write(h) };
    OK
}
/// 15: XSpeechSynthesizerGetStreamDataSize(stream, size*).
pub(super) unsafe extern "system" fn speech_stream_size(_: &Object, stream: usize, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    if !speech_handle_valid(stream) { return INVALID; }
    unsafe { out.write(silent_wav().len()) };
    OK
}
/// 16: XSpeechSynthesizerGetStreamData(stream, bufferSize, buffer, bufferUsed*).
pub(super) unsafe extern "system" fn speech_stream_data(_: &Object, stream: usize, size: usize, out: *mut u8, used: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    if !speech_handle_valid(stream) { return INVALID; }
    let wav = silent_wav();
    if size < wav.len() { return INSUFFICIENT_BUFFER; }
    unsafe {
        core::ptr::copy_nonoverlapping(wav.as_ptr(), out, wav.len());
        if !used.is_null() { used.write(wav.len()) };
    }
    OK
}

// ---------------------------------------------------------------------------
// XGame launch slots. XLaunchNewGame ends the title and starts another one:
// the request (exe path, then arguments) goes to
// %LOCALAPPDATA%\TuxBox\relaunch\<exe stem>.txt and the process exits; the
// launcher starts the requested title in its place. TUXBOX_RELAUNCH=0 logs the
// request and keeps running (old behaviour).

fn relaunch_request_path() -> Option<std::path::PathBuf> {
    let stem = std::env::current_exe().ok()?.file_stem()?.to_string_lossy().into_owned();
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::Path::new(&base).join("TuxBox").join("relaunch").join(format!("{stem}.txt")))
}

/// 4: void XLaunchNewGame(exePath, args, defaultUser).
pub(super) unsafe extern "system" fn game_launch_new(_: &Object, exe: *const i8, args: *const i8, _user: *mut c_void) {
    let (exe, args) = unsafe { (text(exe).unwrap_or_default(), text(args).unwrap_or_default()) };
    if std::env::var("TUXBOX_RELAUNCH").is_ok_and(|v| v.trim() == "0") {
        super::report(format_args!("XLaunchNewGame(\"{exe}\", \"{args}\") ignored (TUXBOX_RELAUNCH=0)"));
        return;
    }
    let Some(path) = relaunch_request_path() else {
        super::report(format_args!("XLaunchNewGame(\"{exe}\", \"{args}\") ignored: no LOCALAPPDATA"));
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(&path, format!("{exe}\n{args}\n")) {
        super::report(format_args!("XLaunchNewGame(\"{exe}\", \"{args}\") ignored: {e}"));
        return;
    }
    eprintln!("TuxBox xgameruntime: XLaunchNewGame(\"{exe}\", \"{args}\"): exiting for the launcher to relaunch (TUXBOX_RELAUNCH=0 ignores it)");
    std::process::exit(0);
}
/// 5: XLaunchRestartOnCrash(args, reserved): accepted; nothing restarts the title.
pub(super) unsafe extern "system" fn game_restart_on_crash(_: &Object, args: *const i8, _reserved: u32) -> i32 {
    let args = unsafe { text(args).unwrap_or_default() };
    super::report(format_args!("XLaunchRestartOnCrash(\"{args}\") accepted"));
    OK
}

// ---------------------------------------------------------------------------
// TUXBOX_TRACE_API=1 (Xbox2Windows dce4ce6d/8d6c5477/9731fae2): one line per
// traced provider call with its HRESULT, who answered (every provider here is
// the stand-in), and which declared outputs changed. Only the outputs in each
// call's contract are read, each bounded by its declared width.

fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("TUXBOX_TRACE_API").is_ok_and(|v| v.trim() == "1"))
}

/// (output name, pointer, width in bytes); a null pointer is not read.
type Contract<'a> = &'a [(&'a str, *const u8, usize)];

unsafe fn traced(api: &str, outs: Contract<'_>, call: impl FnOnce() -> i32) -> i32 {
    if !trace_on() {
        return call();
    }
    let read = |p: *const u8, w: usize| (!p.is_null() && w != 0).then(|| unsafe { std::slice::from_raw_parts(p, w) }.to_vec());
    let before: Vec<Option<Vec<u8>>> = outs.iter().map(|&(_, p, w)| read(p, w)).collect();
    let hr = call();
    let mut changed = Vec::new();
    for (&(name, p, w), old) in outs.iter().zip(before) {
        if let (Some(old), Some(new)) = (old, read(p, w)) {
            if old != new {
                let hex: String = new.iter().map(|b| format!("{b:02x}")).collect();
                changed.push(format!("{name}={hex}"));
            }
        }
    }
    eprintln!("TuxBox api: {api} -> {:#010x} (stand-in) changed [{}]", hr as u32, changed.join(", "));
    hr
}

pub(super) unsafe extern "system" fn user_add_result(o: &Object, block: *mut c_void, out: *mut *mut c_void) -> i32 {
    unsafe { traced("XUserAddResult", &[("user", out.cast(), 8)], || user_add_result_impl(o, block, out)) }
}
pub(super) unsafe extern "system" fn user_id(o: &Object, user: *mut c_void, out: *mut u64) -> i32 {
    unsafe { traced("XUserGetId", &[("id", out.cast(), 8)], || user_id_impl(o, user, out)) }
}
pub(super) unsafe extern "system" fn user_is_guest(o: &Object, user: *mut c_void, out: *mut bool) -> i32 {
    unsafe { traced("XUserGetIsGuest", &[("guest", out.cast(), 1)], || user_is_guest_impl(o, user, out)) }
}
pub(super) unsafe extern "system" fn user_state(o: &Object, user: *mut c_void, out: *mut u32) -> i32 {
    unsafe { traced("XUserGetState", &[("state", out.cast(), 4)], || user_state_impl(o, user, out)) }
}
pub(super) unsafe extern "system" fn user_check_privilege(
    o: &Object, user: *mut c_void, options: u32, privilege: u32, has: *mut bool, reason: *mut u32,
) -> i32 {
    unsafe {
        traced("XUserCheckPrivilege", &[("has", has.cast(), 1), ("deny_reason", reason.cast(), 4)],
            || user_check_privilege_impl(o, user, options, privilege, has, reason))
    }
}
pub(super) unsafe extern "system" fn user_gamertag(
    o: &Object, user: *mut c_void, component: u32, size: usize, buffer: *mut u8, used: *mut usize,
) -> i32 {
    unsafe {
        traced("XUserGetGamertag", &[("gamertag", buffer.cast_const(), size.min(64)), ("used", used.cast(), 8)],
            || user_gamertag_impl(o, user, component, size, buffer, used))
    }
}
pub(super) unsafe extern "system" fn user_age_group(o: &Object, user: *mut c_void, out: *mut u32) -> i32 {
    unsafe { traced("XUserGetAgeGroup", &[("age_group", out.cast(), 4)], || user_age_group_impl(o, user, out)) }
}
pub(super) unsafe extern "system" fn network_hint(o: &Object, out: *mut u8) -> i32 {
    unsafe { traced("XNetworkingGetConnectivityHint", &[("hint", out.cast_const(), 16)], || network_hint_impl(o, out)) }
}

// ---------------------------------------------------------------------------
// XStore (class 0dd112ac, IXStoreImpl6 b09d803c). Offline stand-in: the game
// license is active (not a trial, not a disc license), there are no add-on
// licenses and no package updates. Ownership is answered, never purchased.
// TUXBOX_STORE=0 withholds the class (the title then sees E_NOINTERFACE).

static STORE_CONTEXT: u8 = 0;

/// XStoreCreateContext(user, context*).
pub(super) unsafe extern "system" fn store_create_context(_: &Object, _user: *mut c_void, out: *mut *mut c_void) -> i32 {
    if out.is_null() { return POINTER; }
    unsafe { out.write(core::ptr::from_ref(&STORE_CONTEXT).cast_mut().cast()) };
    super::report(format_args!("XStore context created (offline: licensed, no add-ons)"));
    OK
}
/// XStoreCloseContextHandle(context).
pub(super) extern "system" fn store_close_context(_: &Object, _context: *mut c_void) {}
/// XStoreIsLicenseValid(license): every license handle is valid.
pub(super) extern "system" fn store_is_license_valid(_: &Object, _license: *mut c_void) -> bool {
    true
}
/// XStoreQueryGameLicenseAsync(context, async).
pub(super) unsafe extern "system" fn store_game_license_async(_: &Object, _context: *mut c_void, block: *mut c_void) -> i32 {
    unsafe { complete(block, OK, STORE_LICENSE_SIZE, Payload::Empty) }
}
/// XStoreGameLicense: skuStoreId[18], isActive, isTrialOwnedByThisUser,
/// isDiscLicense, isTrial (bool), trialTimeRemainingInSeconds (u32 at 24),
/// trialUniqueId[64] (28), expirationDate (time_t at 96) = 104 bytes.
const STORE_LICENSE_SIZE: usize = 104;
/// XStoreQueryGameLicenseResult(async, license*).
pub(super) unsafe extern "system" fn store_game_license_result(_: &Object, block: *mut c_void, out: *mut u8) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(_) => {
            unsafe {
                out.write_bytes(0, STORE_LICENSE_SIZE);
                out.add(18).write(1); // isActive
            }
            OK
        }
        Err(status) => status,
    }
}
/// XStoreQueryAddOnLicensesAsync / XStoreQueryGameAndDlcPackageUpdatesAsync(context, async).
pub(super) unsafe extern "system" fn store_empty_async(_: &Object, _context: *mut c_void, block: *mut c_void) -> i32 {
    unsafe { complete(block, OK, 0, Payload::Empty) }
}
/// ...ResultCount(async, count*): none. The operation stays for the result call.
pub(super) unsafe extern "system" fn store_zero_count(_: &Object, _block: *mut c_void, count: *mut u32) -> i32 {
    if count.is_null() { return POINTER; }
    unsafe { count.write(0) };
    OK
}
/// ...Result(async, count, items*): an empty list.
pub(super) extern "system" fn store_empty_list_result(_: &Object, block: *mut c_void, _count: u32, _items: *mut c_void) -> i32 {
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}
/// XStoreQueryPackageUpdatesAsync(context, packageIdentifiers, count, async): none.
pub(super) unsafe extern "system" fn store_package_updates_async(
    _: &Object, _context: *mut c_void, _ids: *const *const u8, _count: usize, block: *mut c_void,
) -> i32 {
    unsafe { complete(block, OK, 0, Payload::Empty) }
}
