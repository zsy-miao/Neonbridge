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
/// The documented "no such user" error. Copied from ztrx victory-hle
/// xuser.rs:55 (GPL-3.0, credit to its authors).
const USER_NOT_FOUND: i32 = 0x8924_5104_u32 as i32;
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
    Monitor,
    /// A handle or small value an async call hands back (TUXBOX_STORE_OFFLINE,
    /// TUXBOX_XNET_SLOTS).
    Handle(usize),
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
/// TUXBOX_XASYNC_EARLY_CALLBACK=0 reads the callback and queue after publishing the status.
unsafe fn complete(block: *mut c_void, status: i32, size: usize, payload: Payload) -> i32 {
    if block.is_null() {
        return INVALID;
    }
    // SAFETY: the guest keeps its XAsyncBlock alive until completion; its
    // third word is the completion routine.
    // The queue handle (its first word) is read with it, for the same reason:
    // a call begun on the block after the status publishes may set another
    // queue (VIctory d01b170).
    let read = || unsafe { (block.cast::<usize>().read(), block.cast::<usize>().add(2).read()) };
    let early = callback_at_completion().then(read);
    OPERATIONS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(block as usize, Operation { status, size, payload });
    let (queue, callback) = early.unwrap_or_else(read);
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
    // TUXBOX_XUSER_ZTRX (=0 NOT_FOUND as before): a miss is USER_NOT_FOUND.
    // Copied from ztrx victory-hle xuser.rs:55,141-143 (GPL-3.0, credit to its authors).
    if id != LOCAL_ID { return if switch("TUXBOX_XUSER_ZTRX") { USER_NOT_FOUND } else { NOT_FOUND }; }
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
    // TUXBOX_XUSER_ZTRX (=0 NOT_FOUND as before): a miss is USER_NOT_FOUND.
    // Copied from ztrx victory-hle xuser.rs:55,147-149 (GPL-3.0, credit to its authors).
    if id != USER_ID { return if switch("TUXBOX_XUSER_ZTRX") { USER_NOT_FOUND } else { NOT_FOUND }; }
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
    // TUXBOX_PACKAGE_PROGRESS32 (default on, =0 the 24-byte 1/1 answer below):
    // the 32-byte { total, installed, launch: u64; launchable, completed: bool }
    // with the package folder's real size. Copied from ztrx victory-hle
    // xpackage.rs:50-70,104-126 (GPL-3.0, credit to its authors); the package
    // folder is our executable's folder, as the leader's main image path is.
    if switch("TUXBOX_PACKAGE_PROGRESS32") {
        let bytes = package_bytes();
        unsafe {
            out.write_bytes(0, 32);
            out.cast::<u64>().write_unaligned(bytes);
            out.add(8).cast::<u64>().write_unaligned(bytes);
            out.add(16).cast::<u64>().write_unaligned(bytes);
            out.add(24).write(1);
            out.add(25).write(1);
        }
        return;
    }
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

/// Default-on switches: unset or anything but `0` is on.
fn switch(name: &str) -> bool {
    std::env::var(name).map_or(true, |v| v.trim() != "0")
}

/// A slot's old answer with its switch off: the probe's log and E_NOTIMPL.
unsafe fn probe_slot<const K: usize, const N: usize>(this: &Object, a: usize, b: usize, c: usize, d: usize, e: usize) -> i32 {
    unsafe { super::probe::method::<K, N>(this, a, b, c, d, e) }
}

/// Bytes in the package folder, counted once. Copied from ztrx victory-hle
/// xpackage.rs:50-70 (GPL-3.0, credit to its authors); the folder is our
/// executable's, not the leader's runtime::main_image.
fn package_bytes() -> u64 {
    fn size(dir: &std::path::Path) -> u64 {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .flatten()
            .map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => size(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            })
            .sum()
    }
    static BYTES: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *BYTES.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        exe.as_deref().and_then(std::path::Path::parent).map_or(0, size)
    })
}

/// The documented "could not resolve an identifier for the package".
const NO_PACKAGE: i32 = 0x8924_5208_u32 as i32;
const E_INVALIDARG: i32 = 0x8007_0057_u32 as i32;

/// Our installation's id as package_identifier hands it out (32 bytes at most).
fn is_ours(id: *const i8) -> bool {
    let mut ours = package_name();
    ours.truncate(32);
    unsafe { text(id) }.is_some_and(|id| id == ours)
}

// TUXBOX_PACKAGE_CHUNKS (default on, =0 the probe's E_NOTIMPL for slots 12,
// 14, 16-19): every chunk installed, nothing to download, other package ids
// NO_PACKAGE. Copied from ztrx victory-hle xpackage.rs:144-224 (GPL-3.0,
// credit to its authors); ours compares against package_name(), the id our
// slot 3 reports, and the install result is our monitor handle.

/// XPackageFindChunkAvailability (slot 12): 0, ready.
// shortcut (leader): every selector counts as installed.
pub(super) unsafe extern "system" fn package_find_chunk_availability(
    this: &Object, id: *const i8, count: u32, selectors: *const c_void, out: *mut u32,
) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 12>(this, id as usize, count as usize, selectors as usize, out as usize, 0) };
    }
    if !is_ours(id) { return NO_PACKAGE; }
    if out.is_null() { return POINTER; }
    unsafe { out.write(0) };
    OK
}
/// XPackageChangeChunkInstallOrder (slot 14).
pub(super) unsafe extern "system" fn package_change_chunk_order(
    this: &Object, id: *const i8, count: u32, selectors: *const c_void,
) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 14>(this, id as usize, count as usize, selectors as usize, 0, 0) };
    }
    if is_ours(id) { OK } else { NO_PACKAGE }
}
/// XPackageInstallChunksAsync (slot 16): done at once, the result is a monitor.
pub(super) unsafe extern "system" fn package_install_chunks_async(
    this: &Object, id: *const i8, count: u32, selectors: *const c_void, interval: u32,
    suppress: bool, block: *mut c_void,
) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 16>(this, id as usize, count as usize, selectors as usize, interval as usize, suppress as usize) };
    }
    if is_ours(id) {
        unsafe { complete(block, OK, size_of::<usize>(), Payload::Monitor) }
    } else {
        unsafe { complete(block, NO_PACKAGE, 0, Payload::Empty) }
    }
}
/// XPackageInstallChunksResult (slot 17).
pub(super) unsafe extern "system" fn package_install_chunks_result(this: &Object, block: *mut c_void, out: *mut *const u8) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 17>(this, block as usize, out as usize, 0, 0, 0) };
    }
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(Payload::Monitor) => { unsafe { out.write(&raw const MONITOR) }; OK }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
/// XPackageEstimateDownloadSize (slot 18): nothing to download.
pub(super) unsafe extern "system" fn package_estimate_download(
    this: &Object, id: *const i8, count: u32, selectors: *const c_void, bytes: *mut u64, confirm: *mut bool,
) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 18>(this, id as usize, count as usize, selectors as usize, bytes as usize, confirm as usize) };
    }
    if !is_ours(id) { return NO_PACKAGE; }
    if !confirm.is_null() { unsafe { confirm.write(false) }; }
    if bytes.is_null() { return POINTER; }
    unsafe { bytes.write(0) };
    OK
}
/// XPackageUninstallChunks (slot 19).
// shortcut (leader): accepted, nothing removed; the chunks stay ready.
pub(super) unsafe extern "system" fn package_uninstall_chunks(
    this: &Object, id: *const i8, count: u32, selectors: *const c_void,
) -> i32 {
    if !switch("TUXBOX_PACKAGE_CHUNKS") {
        return unsafe { probe_slot::<2, 19>(this, id as usize, count as usize, selectors as usize, 0, 0) };
    }
    if is_ours(id) { OK } else { NO_PACKAGE }
}

// TUXBOX_PACKAGE_MOUNT_SLOTS (default on, =0 the probe's E_NOTIMPL for slots
// 25, 26, 34, 37, 38): no other package is installed, so none mounts. Copied
// from ztrx victory-hle xpackage.rs:226-258 (GPL-3.0, credit to its authors).

/// XPackageGetMountPath (slot 25): mount handles only come from mounting
/// another package, and there is none.
pub(super) unsafe extern "system" fn package_mount_path(this: &Object, mount: *const c_void, size: usize, buffer: *mut u8) -> i32 {
    if !switch("TUXBOX_PACKAGE_MOUNT_SLOTS") {
        return unsafe { probe_slot::<2, 25>(this, mount as usize, size, buffer as usize, 0, 0) };
    }
    E_INVALIDARG
}
/// XPackageCloseMountHandle (slot 26).
pub(super) unsafe extern "system" fn package_close_mount(this: &Object, mount: *const c_void) {
    if !switch("TUXBOX_PACKAGE_MOUNT_SLOTS") {
        unsafe { probe_slot::<2, 26>(this, mount as usize, 0, 0, 0, 0) };
    }
}
/// XPackageUninstallPackage (slot 34): only other packages are uninstalled
/// this way, and none is installed.
pub(super) unsafe extern "system" fn package_uninstall(this: &Object, id: *const i8) -> bool {
    if !switch("TUXBOX_PACKAGE_MOUNT_SLOTS") {
        return unsafe { probe_slot::<2, 34>(this, id as usize, 0, 0, 0, 0) } != 0;
    }
    super::report(format_args!("package uninstall refused: {:?}", unsafe { text(id) }));
    false
}
/// XPackageMountWithUiAsync (slot 37): finishes with NO_PACKAGE.
pub(super) unsafe extern "system" fn package_mount_async(this: &Object, id: *const i8, block: *mut c_void) -> i32 {
    if !switch("TUXBOX_PACKAGE_MOUNT_SLOTS") {
        return unsafe { probe_slot::<2, 37>(this, id as usize, block as usize, 0, 0, 0) };
    }
    super::report(format_args!("package mount: {:?} not installed", unsafe { text(id) }));
    unsafe { complete(block, NO_PACKAGE, 0, Payload::Empty) }
}
/// XPackageMountWithUiResult (slot 38).
pub(super) unsafe extern "system" fn package_mount_result(this: &Object, block: *mut c_void, out: *mut *const c_void) -> i32 {
    if !switch("TUXBOX_PACKAGE_MOUNT_SLOTS") {
        return unsafe { probe_slot::<2, 38>(this, block as usize, out as usize, 0, 0, 0) };
    }
    match take(block) {
        Ok(_) => INVALID,
        Err(status) => status,
    }
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
    let path = storage_title_path();
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

/// The path titles get. TUXBOX_PLS_DRIVE (default on, =0 the host folder as
/// before): the root of a drive letter over the storage folder, as ztrx
/// victory-hle storage.rs:44-59 with files.rs:50-64 mount (GPL-3.0, credit to
/// its authors): the highest letter D..Z free in GetLogicalDrives, or the one
/// already mapped to this folder. Difference: we have no file-path rewrite
/// table, so the letter is a real per-session DOS device (DefineDosDeviceW);
/// it is removed at exit (release_storage_drive), and a letter a crashed run
/// left on this folder is reused and owned. Only letters free in both
/// GetLogicalDrives and QueryDosDeviceW are taken. If that fails the host folder is answered, logged once.
/// Saves keep using the host folder (save_root is unchanged).
fn storage_title_path() -> String {
    if !switch("TUXBOX_PLS_DRIVE") {
        return storage_path();
    }
    static DRIVE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    DRIVE.get_or_init(|| {
        let folder = storage_path();
        let folder = folder.trim_end_matches('\\').to_owned();
        let letter = storage_drive(&folder);
        match &letter {
            Some(l) => super::report(format_args!("persistent local storage: drive {l} over {folder}")),
            None => super::report(format_args!("persistent local storage: no drive letter for {folder}, answering the host path")),
        }
        letter
    }).clone().unwrap_or_else(storage_path)
}

fn storage_drive(folder: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{DefineDosDeviceW, GetLogicalDrives, QueryDosDeviceW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let target = format!("\\??\\{folder}");
    let drives = unsafe { GetLogicalDrives() };
    // Already mapped to this folder (an earlier run, possibly one that
    // crashed): reuse it and take ownership, so our exit removes it.
    for letter in b'D'..=b'Z' {
        if drives & (1 << (letter - b'A')) == 0 { continue; }
        let name = wide(&format!("{}:", letter as char));
        let mut buffer = [0u16; 1024];
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        if n == 0 { continue; }
        let first = buffer[..n].split(|&c| c == 0).next().unwrap_or(&[]);
        if String::from_utf16_lossy(first).eq_ignore_ascii_case(&target) {
            own_drive(letter, String::from_utf16_lossy(first));
            return Some(format!("{}:\\", letter as char));
        }
    }
    // Never a letter in use: clear in GetLogicalDrives and unknown to
    // QueryDosDeviceW (ERROR_FILE_NOT_FOUND); none free, the host path.
    let free = |l: &u8| {
        if drives & (1 << (l - b'A')) != 0 { return false; }
        let name = wide(&format!("{}:", *l as char));
        let mut buffer = [0u16; 1024];
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) };
        n == 0 && unsafe { windows_sys::Win32::Foundation::GetLastError() } == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND
    };
    let letter = (b'D'..=b'Z').rev().find(free)?;
    let name = wide(&format!("{}:", letter as char));
    let path = wide(folder);
    if unsafe { DefineDosDeviceW(0, name.as_ptr(), path.as_ptr()) } == 0 { return None; }
    own_drive(letter, target);
    Some(format!("{}:\\", letter as char))
}

/// The letter this process defined (or took over from a dead run) and its
/// exact target, removed at exit: the C runtime's atexit for a normal exit,
/// and lifecycle's end for a PLM close (it ends the process directly).
static OWNED_DRIVE: Mutex<Option<(u8, String)>> = Mutex::new(None);
fn own_drive(letter: u8, target: String) {
    unsafe extern "C" { fn atexit(f: extern "C" fn()) -> i32; }
    extern "C" fn at_exit() { release_storage_drive(); }
    let mut owned = OWNED_DRIVE.lock().unwrap_or_else(|e| e.into_inner());
    if owned.replace((letter, target)).is_none() {
        unsafe { atexit(at_exit) };
    }
}
/// Removes the letter we own, exactly the definition we made (raw target,
/// exact match), so a letter someone else redefined is left alone.
pub(super) fn release_storage_drive() {
    use windows_sys::Win32::Storage::FileSystem::{DDD_EXACT_MATCH_ON_REMOVE, DDD_RAW_TARGET_PATH, DDD_REMOVE_DEFINITION, DefineDosDeviceW};
    let Some((letter, target)) = OWNED_DRIVE.lock().unwrap_or_else(|e| e.into_inner()).take() else { return };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let name = wide(&format!("{}:", letter as char));
    let target = wide(&target);
    let flags = DDD_REMOVE_DEFINITION | DDD_EXACT_MATCH_ON_REMOVE | DDD_RAW_TARGET_PATH;
    unsafe { DefineDosDeviceW(flags, name.as_ptr(), target.as_ptr()) };
}

// TUXBOX_PLS_SPACE (default on, =0 the probe's E_NOTIMPL for slots 3, 5, 6,
// 7). Copied from ztrx victory-hle storage.rs:61-68,90-160 (GPL-3.0, credit
// to its authors). Differences: the quota is the manifest's
// PersistentLocalStorage/SizeMB read from MicrosoftGame.Config beside the
// executable, else 0 as the leader's Title::from_manifest defaults; our saves
// live inside the storage folder, so GameSave and SaveFiles are not counted
// as used.

/// XPersistentLocalStorageGetPathSize (slot 3), matching slot 4's path.
pub(super) unsafe extern "system" fn storage_path_size(this: &Object, out: *mut usize) -> i32 {
    if !switch("TUXBOX_PLS_SPACE") {
        return unsafe { probe_slot::<3, 3>(this, out as usize, 0, 0, 0, 0) };
    }
    if out.is_null() { return POINTER; }
    unsafe { out.write(storage_title_path().len() + 1) };
    OK
}

fn storage_quota() -> u64 {
    let config = std::env::current_exe().ok()
        .and_then(|exe| std::fs::read_to_string(exe.with_file_name("MicrosoftGame.Config")).ok())
        .unwrap_or_default();
    let mb = config.split_once("<PersistentLocalStorage")
        .and_then(|(_, rest)| rest.split_once("</PersistentLocalStorage>"))
        .and_then(|(body, _)| body.split_once("<SizeMB>"))
        .and_then(|(_, rest)| rest.split_once("</SizeMB>"))
        .and_then(|(mb, _)| mb.trim().parse::<u64>().ok());
    mb.unwrap_or(0) << 20
}

/// { available_free (writable now), total_free (left in the quota), used, total (the quota) }.
fn storage_space() -> [u64; 4] {
    let path = storage_path();
    let root = std::path::Path::new(&path);
    let used: u64 = std::fs::read_dir(root).into_iter().flatten().flatten()
        .filter(|e| !matches!(e.file_name().to_str(), Some("GameSave" | "SaveFiles")))
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => tree_bytes(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum();
    let total = storage_quota();
    let free = total.saturating_sub(used);
    [disk_free(root).map_or(free, |d| free.min(d)), free, used, total]
}

/// XPersistentLocalStorageGetSpaceInfo (slot 5).
pub(super) unsafe extern "system" fn storage_space_info(this: &Object, out: *mut [u64; 4]) -> i32 {
    if !switch("TUXBOX_PLS_SPACE") {
        return unsafe { probe_slot::<3, 5>(this, out as usize, 0, 0, 0, 0) };
    }
    if out.is_null() { return POINTER; }
    unsafe { out.write_unaligned(storage_space()) };
    OK
}
/// XPersistentLocalStoragePromptUserForSpaceAsync (slot 6): nobody to ask,
/// granted when it fits, else E_ABORT. The leader checks the quota's rest
/// (total_free); ours checks what is writable now (capped by free disk).
pub(super) unsafe extern "system" fn storage_prompt_for_space(this: &Object, bytes: u64, block: *mut c_void) -> i32 {
    if !switch("TUXBOX_PLS_SPACE") {
        return unsafe { probe_slot::<3, 6>(this, bytes as usize, block as usize, 0, 0, 0) };
    }
    let status = if bytes <= storage_space()[0] { OK } else { 0x8000_4004u32 as i32 };
    unsafe { complete(block, status, 0, Payload::Empty) }
}
/// XPersistentLocalStoragePromptUserForSpaceResult (slot 7).
pub(super) unsafe extern "system" fn storage_prompt_result(this: &Object, block: *mut c_void) -> i32 {
    if !switch("TUXBOX_PLS_SPACE") {
        return unsafe { probe_slot::<3, 7>(this, block as usize, 0, 0, 0, 0) };
    }
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
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
/// One update writes at most 16 MiB (a blob, or all its blobs together), else
/// E_GS_UPDATE_TOO_BIG, as VIctory-fxceless xgamesave.rs MAX_UPDATE (GPL-3.0,
/// credit). TUXBOX_SAVE_UPDATE_LIMIT=0 lifts it.
const UPDATE_TOO_BIG: i32 = 0x8083_0005u32 as i32;
const MAX_UPDATE: usize = 16 << 20;
fn update_limit() -> bool {
    std::env::var_os("TUXBOX_SAVE_UPDATE_LIMIT").is_none_or(|v| v != "0")
}
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

/// TUXBOX_SAVE_NAME_CHECK (default on, =0 any name as before): E_GS_INVALID_CONTAINER_NAME.
const INVALID_CONTAINER_NAME: i32 = 0x8083_0001u32 as i32;
fn name_check() -> bool {
    switch("TUXBOX_SAVE_NAME_CHECK")
}
/// Copied from ztrx victory-hle xgamesave.rs:121-135 (GPL-3.0, credit to its
/// authors). Documented: the path part (up to the last `/`) letters, digits,
/// `_` and `/`; the file part letters, digits, `_`, `.` and `-`, not empty,
/// not ending in `.`, no `..`; 256 characters at most.
fn valid_container(name: &str) -> bool {
    let (path, file) = name.rsplit_once('/').unwrap_or(("", name));
    name.len() <= 256
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'/')
        && !file.is_empty()
        && !file.ends_with('.')
        && !file.contains("..")
        && file
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

fn delete_container(name: &str) -> i32 {
    // TUXBOX_SAVE_SAFE_DELETE (default on, =0 the plain remove_dir_all below):
    // renamed away (one step), then removed; a missing container is S_OK.
    // Copied from ztrx victory-hle xgamesave.rs:487-505 (GPL-3.0, credit to its
    // authors). Differences: `.gone` is appended to the encoded folder name
    // (the leader's with_extension would replace a `.` suffix of ours), the
    // rename goes through our rename_retry, and our cross-process save lock
    // stands in for the leader's saves_lock.
    if switch("TUXBOX_SAVE_SAFE_DELETE") {
        let _saves = match lock_saves() { Ok(l) => l, Err(busy) => return busy };
        let dir = container_dir(name);
        if !dir.exists() {
            return OK;
        }
        let mut gone = dir.clone().into_os_string();
        gone.push(".gone");
        let gone = std::path::PathBuf::from(gone);
        let _ = std::fs::remove_dir_all(&gone);
        if let Err(error) = rename_retry(&dir, &gone) {
            super::report(format_args!("game save: deleting container {name:?} failed: {error}"));
            return io_status(&error);
        }
        let _ = std::fs::remove_dir_all(&gone);
        return OK;
    }
    match std::fs::remove_dir_all(container_dir(name)) {
        Ok(()) => OK,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => OK,
        Err(error) => {
            super::report(format_args!("game save: deleting container {name:?} failed: {error}"));
            io_status(&error)
        }
    }
}
/// XGameSaveDeleteContainer(provider, name) (slot 10).
pub(super) unsafe extern "system" fn save_delete(_: &Object, _provider: *const u8, name: *const i8) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    super::report(format_args!("game save: delete container {name:?}"));
    // TUXBOX_SAVE_NAME_CHECK: ztrx victory-hle xgamesave.rs:660-675 (GPL-3.0, credit to its authors).
    if name_check() && !valid_container(&name) { return INVALID_CONTAINER_NAME; }
    delete_container(&name)
}
/// XGameSaveDeleteContainerAsync(provider, name, async) (slot 11).
pub(super) unsafe extern "system" fn save_delete_async(_: &Object, _provider: *const u8, name: *const i8, block: *mut c_void) -> i32 {
    let Some(name) = (unsafe { text(name) }) else { return POINTER };
    super::report(format_args!("game save: delete container {name:?} (async)"));
    // TUXBOX_SAVE_NAME_CHECK: the block finishes with the error, as ztrx
    // victory-hle xgamesave.rs:660-675 delete_async (GPL-3.0, credit to its authors).
    let status = if name_check() && !valid_container(&name) { INVALID_CONTAINER_NAME } else { delete_container(&name) };
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
    // TUXBOX_SAVE_NAME_CHECK: ztrx victory-hle xgamesave.rs:680-692 (GPL-3.0, credit to its authors).
    if name_check() && !valid_container(&name) { return INVALID_CONTAINER_NAME; }
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
    // TUXBOX_SAVE_NAME_CHECK: ztrx victory-hle xgamesave.rs:708-723 (GPL-3.0, credit to its authors).
    if name_check() && !valid_container(&name) { return INVALID_CONTAINER_NAME; }
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
    let _shared = match lock_saves_shared() { Ok(l) => l, Err(busy) => return busy };
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
    let _shared = lock_saves_shared()?;
    // TUXBOX_SAVE_BLOB_NOT_FOUND (default on, =0 the check below): any read of
    // a container never written (no committed blobs folder) is BLOB_NOT_FOUND,
    // named reads included. Copied from ztrx victory-hle xgamesave.rs:507-526
    // (GPL-3.0, credit to its authors), where no current version is BLOB_NOT_FOUND.
    if switch("TUXBOX_SAVE_BLOB_NOT_FOUND") && !container_dir(container).join("blobs").is_dir() {
        return Err(BLOB_NOT_FOUND);
    }
    // Reading every blob of a container never written is BLOB_NOT_FOUND, not
    // an empty S_OK, as VIctory-fxceless xgamesave.rs (GPL-3.0, credit) says.
    // TUXBOX_SAVE_MISSING_NOT_FOUND=0 returns no blobs again.
    if names.is_none() && !container_dir(container).exists()
        && std::env::var_os("TUXBOX_SAVE_MISSING_NOT_FOUND").is_none_or(|v| v != "0")
    {
        return Err(BLOB_NOT_FOUND);
    }
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
        Ok(Payload::Blobs(blobs)) => {
            let status = unsafe { fill_blobs(&blobs, size, out, count) };
            // A too-small buffer keeps the result for the title's retry with a
            // bigger one, as VIctory-fxceless xgamesave.rs (GPL-3.0, credit)
            // does; before, take() dropped it and the retry failed.
            // TUXBOX_SAVE_KEEP_RESULT=0 drops it again.
            if status == BUFFER_TOO_SMALL && std::env::var_os("TUXBOX_SAVE_KEEP_RESULT").is_none_or(|v| v != "0") {
                let needed = blobs_size(&blobs);
                OPERATIONS.lock().unwrap_or_else(|e| e.into_inner())
                    .insert(block as usize, Operation { status: OK, size: needed, payload: Payload::Blobs(blobs) });
            }
            status
        }
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
    if bytes > MAX_UPDATE && update_limit() { return UPDATE_TOO_BIG; }
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
    if pending.writes.iter().map(|(_, data)| data.len()).sum::<usize>() > MAX_UPDATE && update_limit() {
        return Some(UPDATE_TOO_BIG);
    }
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
    // TUXBOX_SAVE_QUOTA (default on, =0 the whole folder tree as before): for
    // the containers' folder, only committed blob bytes count, not staging,
    // display or lock files. Copied from ztrx victory-hle xgamesave.rs:393-412
    // (GPL-3.0, credit to its authors); the leader's used_bytes counts its blob
    // files, ours sums list_blobs, which skips staging files. SaveFiles (slot
    // 32) has no blobs and keeps the tree count.
    let blob_only = root == save_root() && switch("TUXBOX_SAVE_QUOTA");
    let root = std::path::Path::new(root);
    let used = if blob_only {
        list_containers().iter().flat_map(|c| list_blobs(c)).map(|b| b.1).sum()
    } else {
        tree_bytes(root)
    };
    let left = (SAVE_QUOTA as u64).saturating_sub(used);
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
const SAVE_BUSY_WIN32: i32 = 0x8007_00AAu32 as i32;
const NO_ACCESS: i32 = 0x8083_0002u32 as i32;
/// Save I/O failures answer with the E_GS codes the title handles: a full disk
/// is OUT_OF_LOCAL_STORAGE, anything else (and the save lock held past its
/// retry) NO_ACCESS, as VIctory-fxceless xgamesave.rs io_error (GPL-3.0,
/// credit). TUXBOX_SAVE_GS_ERRORS=0 returns E_INVALIDARG and ERROR_BUSY again.
fn gs_errors() -> bool {
    std::env::var_os("TUXBOX_SAVE_GS_ERRORS").is_none_or(|v| v != "0")
}
fn io_status(error: &std::io::Error) -> i32 {
    if !gs_errors() {
        INVALID
    } else if error.kind() == std::io::ErrorKind::StorageFull {
        OUT_OF_LOCAL_STORAGE
    } else {
        NO_ACCESS
    }
}
fn save_busy() -> i32 {
    if gs_errors() { NO_ACCESS } else { SAVE_BUSY_WIN32 }
}

fn redirected(path: &std::path::Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_attributes() & 0x400 != 0)
}

/// Exclusive LockFileEx on `<save root>/.lock`, released when the file closes.
/// Any failure that is not a grant (lock or sharing violation, IO_PENDING, or
/// anything else) is busy, retried briefly before giving up.
fn lock_saves() -> Result<std::fs::File, i32> {
    lock_save_root(true)
}

/// The same lock held shared, by readers: a listing or read sees another
/// process's publish whole or not at all (Xbox2Windows 5550ec2b, re-derived).
/// None when TUXBOX_SAVE_READ_LOCK=0 or saves are not atomic (reads unlocked
/// as before); Err(busy) when a publish held it the whole retry.
fn lock_saves_shared() -> Result<Option<std::fs::File>, i32> {
    if !atomic_saves() || std::env::var("TUXBOX_SAVE_READ_LOCK").is_ok_and(|v| v.trim() == "0") {
        return Ok(None);
    }
    lock_save_root(false).map(Some)
}

fn lock_save_root(exclusive: bool) -> Result<std::fs::File, i32> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY};
    let path = std::path::Path::new(&save_root()).join(".lock");
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path)
        .map_err(|_| save_busy())?;
    for _ in 0..100 {
        let mut overlapped: windows_sys::Win32::System::IO::OVERLAPPED = unsafe { core::mem::zeroed() };
        let granted = unsafe {
            LockFileEx(file.as_raw_handle() as _, if exclusive { LOCKFILE_EXCLUSIVE_LOCK } else { 0 } | LOCKFILE_FAIL_IMMEDIATELY, 0, 1, 0, &mut overlapped)
        };
        if granted != 0 {
            return Ok(file);
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    Err(save_busy())
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
            io_status(&error)
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
            io_status(&error)
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
    _: &Object, block: *mut c_void, title: *const i8, description: *const i8, default: *const i8, _scope: u32, max: u32,
) -> i32 {
    let title = unsafe { text(title).unwrap_or_default() };
    if std::env::var("TUXBOX_TEXT_ENTRY_CANCEL").is_ok_and(|v| v.trim() == "1") {
        super::report(format_args!("text entry \"{title}\": cancelled (TUXBOX_TEXT_ENTRY_CANCEL=1)"));
        return unsafe { complete(block, ABORT, 0, Payload::Empty) };
    }
    let default = unsafe { text(default) }.unwrap_or_default();
    let clip = move |answer: String| if max > 0 { answer.chars().take(max as usize).collect() } else { answer };
    // TUXBOX_TEXT_ENTRY overrides: the answer without asking. Otherwise a host
    // dialog asks the player, as the console's keyboard does (fxceless 9d94a1c,
    // ztrx xgameui.rs:147); TUXBOX_TEXT_ENTRY_DIALOG=0 answers the default text.
    let asked = match std::env::var("TUXBOX_TEXT_ENTRY") {
        Ok(answer) => Some(answer),
        Err(_) if std::env::var("TUXBOX_TEXT_ENTRY_DIALOG").is_ok_and(|v| v.trim() == "0") => Some(default.clone()),
        Err(_) => None,
    };
    if let Some(answer) = asked {
        let answer = clip(answer);
        super::report(format_args!("text entry \"{title}\": answered \"{answer}\" (TUXBOX_TEXT_ENTRY sets the answer)"));
        let size = answer.len() + 1;
        return unsafe { complete(block, OK, size, Payload::Text(answer)) };
    }
    let description = unsafe { text(description) }.unwrap_or_default();
    // Modal, so on a thread of its own: the call is async and the title's
    // thread keeps running; the block lives until completion.
    if block.is_null() { return INVALID; }
    // Pending until the player answers: XAsyncGetStatus polls it meanwhile.
    OPERATIONS.lock().unwrap_or_else(|e| e.into_inner())
        .insert(block as usize, Operation { status: E_PENDING, size: 0, payload: Payload::Empty });
    let block = block as usize;
    std::thread::spawn(move || {
        let block = block as *mut c_void;
        match crate::text_entry::ask(&title, &description, &default, max) {
            Some(answer) => {
                let answer = clip(answer);
                super::report(format_args!("text entry \"{title}\": typed \"{answer}\""));
                let size = answer.len() + 1;
                unsafe { complete(block, OK, size, Payload::Text(answer)) };
            }
            None => {
                super::report(format_args!("text entry \"{title}\": cancelled in the dialog"));
                unsafe { complete(block, ABORT, 0, Payload::Empty) };
            }
        }
    });
    OK
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
// TUXBOX_STORE_OFFLINE (=0 every handle valid, as before): only the licence
// handle slots 20 and 71 give out for the game's own package is valid. Copied
// from ztrx victory-hle xstore.rs:161-163 (GPL-3.0, credit to its authors).
pub(super) extern "system" fn store_is_license_valid(_: &Object, license: *mut c_void) -> bool {
    if !switch("TUXBOX_STORE_OFFLINE") { return true; }
    license as usize == store_handle(&STORE_VALID)
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
                // TUXBOX_STORE_OFFLINE (=0 an empty skuStoreId as before): the
                // SKU is "<store id>/0010", the store's first. Copied from ztrx
                // victory-hle xstore.rs:180-199 (GPL-3.0, credit to its authors);
                // the store id is read from MicrosoftGame.Config, not the
                // leader's runtime::title().
                if switch("TUXBOX_STORE_OFFLINE") {
                    let id = format!("{}/0010", store_id());
                    let len = id.len().min(17);
                    core::ptr::copy_nonoverlapping(id.as_ptr(), out, len);
                }
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

// ---------------------------------------------------------------------------
// TUXBOX_STORE_OFFLINE (default on, =0 the probe's E_NOTIMPL for every slot
// below, and slot 22 / the game licence as before). Copied from ztrx
// victory-hle xstore.rs:46,52-300 with its table 335-415 (GPL-3.0, credit to
// its authors). Our XStore table is served for IXStoreImpl6 b09d803c, the
// leader's IID_2026, and our implemented slots (3, 4, 22, 28, 29) sit where
// its do, so its slot numbers are used as they are. Only slots that were
// probe-only in ours are ported: ours keeps 30-32 and 55-57 (empty lists)
// and 79-81 (past the leader's table). Differences: results go through our
// OPERATIONS map (complete/take), not the leader's xasync, and the package
// id is ours (is_ours / package_identifier, as XPackage slot 3 reports it).

/// The documented "failed to communicate with the store network".
const STORE_NETWORK_ERROR: i32 = 0x8924_5301_u32 as i32;
const E_ILLEGAL_METHOD_CALL: i32 = 0x8000_000E_u32 as i32;
static STORE_QUERY: u8 = 0;
static STORE_VALID: u8 = 0;
static STORE_INVALID: u8 = 0;
fn store_handle(of: &'static u8) -> usize {
    core::ptr::from_ref(of) as usize
}
fn store_on() -> bool {
    switch("TUXBOX_STORE_OFFLINE")
}

/// The manifest's StoreId (MicrosoftGame.Config beside the executable), else
/// empty as the leader's Title defaults.
fn store_id() -> &'static str {
    static ID: LazyLock<String> = LazyLock::new(|| {
        let config = std::env::current_exe().ok()
            .and_then(|exe| std::fs::read_to_string(exe.with_file_name("MicrosoftGame.Config")).ok())
            .unwrap_or_default();
        config.split_once("<StoreId>")
            .and_then(|(_, rest)| rest.split_once('<'))
            .map(|(id, _)| id.trim().to_owned())
            .unwrap_or_default()
    });
    &ID
}

/// Calls that need the store service: complete with NETWORK_ERROR. AT is
/// the 1-based argument (after `this`) holding the XAsyncBlock.
pub(super) unsafe extern "system" fn store_offline<const N: usize, const AT: usize>(
    this: &Object, a: usize, b: usize, c: usize, d: usize, e: usize, f: usize,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, N>(this, a, b, c, d, e) }; }
    let block = [a, b, c, d, e, f][AT - 1] as *mut c_void;
    super::report(format_args!("store call (slot {N}): offline"));
    unsafe { complete(block, STORE_NETWORK_ERROR, 0, Payload::Empty) }
}
/// The result, size, string, count and array slots of those calls: their status.
pub(super) unsafe extern "system" fn store_status<const N: usize>(
    this: &Object, block: *mut c_void, b: usize, c: usize, d: usize, e: usize,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, N>(this, block as usize, b, c, d, e) }; }
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}
/// The user owns no add-ons (slot 9): an empty query.
pub(super) unsafe extern "system" fn store_entitled_async(
    this: &Object, context: usize, kinds: usize, per_page: usize, block: *mut c_void,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 9>(this, context, kinds, per_page, block as usize, 0) }; }
    unsafe { complete(block, OK, 8, Payload::Handle(store_handle(&STORE_QUERY))) }
}
/// Query and licence results (6, 10, 18, 21, 72): the handle, or the failure.
pub(super) unsafe extern "system" fn store_handle_result<const N: usize>(
    this: &Object, block: *mut c_void, out: *mut usize,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, N>(this, block as usize, out as usize, 0, 0, 0) }; }
    match take(block) {
        Ok(Payload::Handle(h)) => {
            if out.is_null() { return POINTER; }
            unsafe { out.write(h) };
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
/// Empty, so the callback never runs (slot 15).
pub(super) unsafe extern "system" fn store_enumerate_products(
    this: &Object, a: usize, b: usize, c: usize, d: usize, e: usize,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 15>(this, a, b, c, d, e) }; }
    OK
}
/// Slot 16: no more pages. Off, the probe's E_NOTIMPL read as a bool, as before.
pub(super) unsafe extern "system" fn store_has_more_pages(this: &Object, query: usize) -> bool {
    if !store_on() { return unsafe { probe_slot::<11, 16>(this, query, 0, 0, 0, 0) } != 0; }
    false
}
/// Slot 17: there is no next page to ask for.
pub(super) unsafe extern "system" fn store_next_page_async(this: &Object, query: usize, block: *mut c_void) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 17>(this, query, block as usize, 0, 0, 0) }; }
    unsafe { complete(block, E_ILLEGAL_METHOD_CALL, 0, Payload::Empty) }
}
/// Product query and licence handles hold nothing (19, 23).
pub(super) unsafe extern "system" fn store_close_handle<const N: usize>(this: &Object, handle: usize) {
    if !store_on() { unsafe { probe_slot::<11, N>(this, handle, 0, 0, 0, 0) }; }
}
/// Slot 20: only the game's own package is installed, so only its licence is valid.
pub(super) unsafe extern "system" fn store_license_for_package_async(
    this: &Object, context: usize, package: *const i8, block: *mut c_void,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 20>(this, context, package as usize, block as usize, 0, 0) }; }
    let license = if is_ours(package) { &STORE_VALID } else { &STORE_INVALID };
    unsafe { complete(block, OK, 8, Payload::Handle(store_handle(license))) }
}
/// Slot 71: the user owns no durables, so their licences are not valid.
pub(super) unsafe extern "system" fn store_license_for_durables_async(
    this: &Object, context: usize, store: *const i8, block: *mut c_void,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 71>(this, context, store as usize, block as usize, 0, 0) }; }
    super::report(format_args!("store: durable {} not owned", unsafe { text(store) }.unwrap_or_default()));
    unsafe { complete(block, OK, 8, Payload::Handle(store_handle(&STORE_INVALID))) }
}
/// Slot 65: the game's own store id maps to its installation; nothing else is installed.
pub(super) unsafe extern "system" fn store_package_identifier(
    this: &Object, store: *const i8, size: usize, buffer: *mut u8,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, 65>(this, store as usize, size, buffer as usize, 0, 0) }; }
    match unsafe { text(store) } {
        Some(id) if !store_id().is_empty() && id == store_id() => unsafe { package_identifier(this, size, buffer) },
        _ => NO_PACKAGE,
    }
}
/// Slots 66 and 68: licences never change offline, so the callbacks never run.
pub(super) unsafe extern "system" fn store_register<const N: usize>(
    this: &Object, context: usize, queue: usize, user: usize, callback: usize, token: *mut u64,
) -> i32 {
    if !store_on() { return unsafe { probe_slot::<11, N>(this, context, queue, user, callback, token as usize) }; }
    if token.is_null() { return POINTER; }
    unsafe { token.write(next_token()) };
    OK
}
/// Slots 67 and 69. Off, the probe's E_NOTIMPL read as a bool, as before.
pub(super) unsafe extern "system" fn store_unregister<const N: usize>(this: &Object, context: usize, token: u64, wait: bool) -> bool {
    if !store_on() { return unsafe { probe_slot::<11, N>(this, context, token as usize, wait as usize, 0, 0) } != 0; }
    true
}

// ---------------------------------------------------------------------------
// TUXBOX_XNET_SLOTS (default on, =0 the probe's E_NOTIMPL for slots 3-5,
// 8-10, 14, 17). Copied from ztrx victory-hle xnetworking.rs:35-160 (GPL-3.0,
// credit to its authors); our table is chosen by class 37e56907 and its hint
// slots (15, 16) sit where the leader's do, so the numbers match. Results
// go through our OPERATIONS map.

/// `HRESULT_FROM_WIN32(ERROR_NETWORK_UNREACHABLE)`: what needs the service.
const NET_UNREACHABLE: i32 = 0x8007_04CF_u32 as i32;
/// The documented default for the preferred local UDP multiplayer port.
const UDP_PORT: u16 = 3074;
fn net_on() -> bool {
    switch("TUXBOX_XNET_SLOTS")
}
pub(super) unsafe extern "system" fn net_udp_port(this: &Object, out: *mut u16) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 3>(this, out as usize, 0, 0, 0, 0) }; }
    if out.is_null() { return POINTER; }
    unsafe { out.write(UDP_PORT) };
    OK
}
pub(super) unsafe extern "system" fn net_udp_port_async(this: &Object, block: *mut c_void) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 4>(this, block as usize, 0, 0, 0, 0) }; }
    unsafe { complete(block, OK, 2, Payload::Handle(UDP_PORT as usize)) }
}
pub(super) unsafe extern "system" fn net_udp_port_result(this: &Object, block: *mut c_void, out: *mut u16) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 5>(this, block as usize, out as usize, 0, 0, 0) }; }
    match take(block) {
        Ok(Payload::Handle(port)) => {
            if out.is_null() { return POINTER; }
            unsafe { out.write(port as u16) };
            OK
        }
        Ok(_) => INVALID,
        Err(status) => status,
    }
}
/// The allow list comes from the service: unreachable offline.
pub(super) unsafe extern "system" fn net_security_info_async(this: &Object, url: *const i8, block: *mut c_void) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 8>(this, url as usize, block as usize, 0, 0, 0) }; }
    super::report(format_args!("security information for {}: offline", unsafe { text(url) }.unwrap_or_default()));
    unsafe { complete(block, NET_UNREACHABLE, 0, Payload::Empty) }
}
pub(super) unsafe extern "system" fn net_security_info_size(this: &Object, block: *mut c_void, out: *mut usize) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 9>(this, block as usize, out as usize, 0, 0, 0) }; }
    unsafe { async_result_size(this, block, out) }
}
pub(super) unsafe extern "system" fn net_security_info_result(
    this: &Object, block: *mut c_void, size: usize, used: usize, buffer: usize, info: usize,
) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 10>(this, block as usize, size, used, buffer, info) }; }
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}
/// No security information can exist offline, so no certificate passes.
pub(super) unsafe extern "system" fn net_verify_certificate(this: &Object, request: usize, info: usize) -> i32 {
    if !net_on() { return unsafe { probe_slot::<4, 14>(this, request, info, 0, 0, 0) }; }
    NET_UNREACHABLE
}
/// Slot 17: the hint never changes. Off, the probe's E_NOTIMPL read as a bool.
pub(super) unsafe extern "system" fn net_unregister(this: &Object, token: u64, wait: bool) -> bool {
    if !net_on() { return unsafe { probe_slot::<4, 17>(this, token as usize, wait as usize, 0, 0, 0) } != 0; }
    true
}

// ---------------------------------------------------------------------------
// TUXBOX_XUSER_ZTRX (default on, =0 the probe's E_NOTIMPL for slots 15,
// 21-25 and NOT_FOUND for find-by-id misses). Copied from ztrx victory-hle
// xuser.rs:200-312 (GPL-3.0, credit to its authors); our XUser table is
// chosen by class 01acd177 and its slots 3-14, 19, 20, 33, 34 sit where the
// leader's do, so the numbers match. Slot 15 answers as our gamertag
// interface does (name "TuxBox"), not the leader's "Player".

/// The documented "failed to get token": tokens come from the service.
const FAILED_TO_GET_TOKEN: i32 = 0x8924_5111_u32 as i32;
fn xuser_on() -> bool {
    switch("TUXBOX_XUSER_ZTRX")
}
/// Slot 15, the older gamertag slot.
pub(super) unsafe extern "system" fn user_gamertag_old(
    this: &Object, user: *mut c_void, component: u32, size: usize, buffer: *mut u8, used: *mut usize,
) -> i32 {
    if !xuser_on() {
        return unsafe { probe_slot::<0, 15>(this, user as usize, component as usize, size, buffer as usize, used as usize) };
    }
    unsafe { user_gamertag(this, user, component, size, buffer, used) }
}
/// Slot 21: every privilege is granted, so there is nothing to resolve.
pub(super) unsafe extern "system" fn user_resolve_privilege_async(
    this: &Object, user: usize, options: u32, privilege: u32, block: *mut c_void,
) -> i32 {
    if !xuser_on() { return unsafe { probe_slot::<0, 21>(this, user, options as usize, privilege as usize, block as usize, 0) }; }
    unsafe { complete(block, OK, 0, Payload::Empty) }
}
/// Slots 22 and 25: the call's status.
pub(super) unsafe extern "system" fn user_status_result<const N: usize>(
    this: &Object, block: *mut c_void, b: usize, c: usize, d: usize, e: usize,
) -> i32 {
    if !xuser_on() { return unsafe { probe_slot::<0, N>(this, block as usize, b, c, d, e) }; }
    match take(block) {
        Ok(_) => OK,
        Err(status) => status,
    }
}
/// Slot 23: token and signature come from the service.
#[allow(clippy::too_many_arguments)]
pub(super) unsafe extern "system" fn user_token_async(
    this: &Object, user: usize, options: u32, method: usize, url: *const i8, header_count: usize,
    _headers: usize, _body_size: usize, _body: usize, block: *mut c_void,
) -> i32 {
    if !xuser_on() { return unsafe { probe_slot::<0, 23>(this, user, options as usize, method, url as usize, header_count) }; }
    super::report(format_args!("token and signature for {}: offline", unsafe { text(url) }.unwrap_or_default()));
    unsafe { complete(block, FAILED_TO_GET_TOKEN, 0, Payload::Empty) }
}
/// Slot 24.
pub(super) unsafe extern "system" fn user_token_size(this: &Object, block: *mut c_void, out: *mut usize) -> i32 {
    if !xuser_on() { return unsafe { probe_slot::<0, 24>(this, block as usize, out as usize, 0, 0, 0) }; }
    unsafe { async_result_size(this, block, out) }
}

// ---------------------------------------------------------------------------
// TUXBOX_PROFILE_CARD_OK (default on, =0 cancelled with E_ABORT as before):
// the profile card has nothing to show and completes with S_OK. Copied from
// ztrx victory-hle xgameui.rs:38-47 (GPL-3.0, credit to its authors); its
// slot 7 (0x38) is ours, and slot 8 stays ui_empty_result.
pub(super) unsafe extern "system" fn ui_profile_card_async(this: &Object, block: *mut c_void, _user: usize, xuid: u64) -> i32 {
    if !switch("TUXBOX_PROFILE_CARD_OK") { return unsafe { ui_cancelled::<7>(this, block) }; }
    super::report(format_args!("profile card for {xuid:#x}: nothing to show"));
    unsafe { complete(block, OK, 0, Payload::Empty) }
}

// ---------------------------------------------------------------------------
// TUXBOX_XSYSTEM_ZTRX (default on, =0 the probe's answer and class gating
// as before): the analytics class b884675d (its interface id the class id)
// slot 3, and XSystem slot 5. Copied from ztrx victory-hle xsystem.rs:40,
// 68-70,82-125 (GPL-3.0, credit to its authors). Difference: the form
// follows TUXBOX_CONSOLE as xmem's console_mode reads it (s / series s /
// lockhart = Series S), since we have no victory_gpu::console.

#[repr(C)]
#[derive(Clone, Copy)]
struct OsVersion {
    major: u16,
    minor: u16,
    build: u16,
    revision: u16,
}
#[repr(C)]
pub(super) struct AnalyticsInfo {
    os: OsVersion,
    hosting_os: OsVersion,
    family: [u8; 64],
    form: [u8; 64],
}
const ANALYTICS_OS: OsVersion = OsVersion { major: 10, minor: 0, build: 22000, revision: 0 };
fn text64(s: &str) -> [u8; 64] {
    let mut out = [0; 64];
    out[..s.len()].copy_from_slice(s.as_bytes());
    out
}
/// A struct this large comes back through the caller's buffer, whose
/// address is also the return value.
pub(super) unsafe extern "system" fn system_analytics_info(this: &Object, out: *mut AnalyticsInfo) -> *mut AnalyticsInfo {
    if !switch("TUXBOX_XSYSTEM_ZTRX") {
        return unsafe { probe_slot::<7, 3>(this, out as usize, 0, 0, 0, 0) } as isize as *mut AnalyticsInfo;
    }
    let series_s = matches!(
        std::env::var("TUXBOX_CONSOLE").unwrap_or_default().trim().to_ascii_lowercase().as_str(),
        "s" | "series s" | "lockhart"
    );
    let info = AnalyticsInfo {
        os: ANALYTICS_OS,
        hosting_os: ANALYTICS_OS,
        family: text64("Xbox"),
        form: text64(if series_s { "Xbox Series S" } else { "Xbox Series X" }),
    };
    if !out.is_null() { unsafe { out.write(info) } };
    out
}
/// XSystem slot 5, the device id this title sees (base64 of 32 bytes), as
/// the leader's put_string (xgameruntime.rs:145-160) writes it.
pub(super) unsafe extern "system" fn system_device_id(_: &Object, size: usize, buffer: *mut u8, used: *mut usize) -> i32 {
    const DEVICE_ID: &str = "VkljdG9yeSBvZmZsaW5lIGRldmljZSBpZGVudGl0eSE=";
    let len = DEVICE_ID.len() + 1;
    if buffer.is_null() || size < len { return INSUFFICIENT_BUFFER; }
    unsafe {
        buffer.copy_from_nonoverlapping(DEVICE_ID.as_ptr(), DEVICE_ID.len());
        buffer.add(DEVICE_ID.len()).write(0);
        if !used.is_null() { used.write(len) };
    }
    OK
}
/// XSystem slot 5 with the switch on, else the old unsupported answer.
pub(super) fn xsystem_ztrx() -> bool {
    switch("TUXBOX_XSYSTEM_ZTRX")
}
