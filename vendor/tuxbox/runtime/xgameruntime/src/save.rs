//! XGameSave containers, updates and blobs (class 704c3f58), in GDK header order.
//! ProjectScarlett creates, writes, commits, enumerates and reads containers through
//! this table. Quota, delete and the remaining slots follow the header unobserved.
use super::services::{Payload, complete, save_root, take};
use super::{INVALID, OK, Object, POINTER, report, save_store as store};
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};

const INSUFFICIENT_BUFFER: i32 = 0x8007007au32 as i32;
const WRITE_FAULT: i32 = 0x8007001du32 as i32;
const BLOB_NOT_FOUND: i32 = 0x80830008u32 as i32;
const QUOTA: i64 = 256 << 20;

struct Container {
    name: String,
}
struct Update {
    container: String,
    display: String,
    changes: Vec<(String, Option<Vec<u8>>)>,
}

// Handles are table keys, so a stale or foreign handle is rejected.
static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0x5358_0010);
static CONTAINERS: LazyLock<Mutex<HashMap<usize, Container>>> = LazyLock::new(Default::default);
static UPDATES: LazyLock<Mutex<HashMap<usize, Update>>> = LazyLock::new(Default::default);
static READS: LazyLock<Mutex<HashMap<usize, Blobs>>> = LazyLock::new(Default::default);
type Blobs = Vec<(String, Vec<u8>)>;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn handle() -> usize {
    NEXT.fetch_add(0x10, std::sync::atomic::Ordering::Relaxed)
}
fn root() -> PathBuf {
    save_root().into()
}
unsafe fn text(p: *const c_char) -> String {
    if p.is_null() { String::new() } else { unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned() }
}
fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

#[repr(C)]
struct ContainerInfo {
    name: *const c_char,
    display: *const c_char,
    blobs: u32,
    size: u64,
    modified: i64,
    needs_sync: bool,
}
#[repr(C)]
pub(super) struct BlobInfo {
    name: *const c_char,
    size: u32,
}
#[repr(C)]
pub(super) struct Blob {
    info: BlobInfo,
    data: *mut u8,
}
const _: () = assert!(size_of::<ContainerInfo>() == 48 && size_of::<BlobInfo>() == 16 && size_of::<Blob>() == 24);

type ContainerCallback = unsafe extern "system" fn(*const ContainerInfo, *mut c_void) -> bool;
type BlobCallback = unsafe extern "system" fn(*const BlobInfo, *mut c_void) -> bool;

unsafe fn each_container(found: Vec<store::Container>, context: *mut c_void, callback: *mut c_void) -> i32 {
    if callback.is_null() { return POINTER; }
    let callback: ContainerCallback = unsafe { core::mem::transmute(callback) };
    for c in found {
        let (name, display) = (cstring(&c.name), cstring(&c.display));
        let info = ContainerInfo {
            name: name.as_ptr(), display: display.as_ptr(), blobs: c.blobs.len() as u32,
            size: c.blobs.iter().map(|b| u64::from(b.1)).sum(), modified: c.modified, needs_sync: false,
        };
        if !unsafe { callback(&info, context) } { break; }
    }
    OK
}

/// XGameSaveGetContainerInfo(provider, name, context, callback).
pub(super) unsafe extern "system" fn container_info(_: &Object, _p: *const u8, name: *const c_char, context: *mut c_void, callback: *mut c_void) -> i32 {
    let name = unsafe { text(name) };
    let found = store::container(&root(), &name);
    report(format_args!("save: container info {name:?}: {}", found.as_ref().map_or("none saved".into(), |c| format!("{} blobs", c.blobs.len()))));
    unsafe { each_container(found.into_iter().collect(), context, callback) }
}
/// XGameSaveEnumerateContainerInfo(provider, context, callback).
pub(super) unsafe extern "system" fn enumerate(_: &Object, _p: *const u8, context: *mut c_void, callback: *mut c_void) -> i32 {
    let found = store::containers(&root());
    report(format_args!("save: enumerate containers: {}", found.len()));
    unsafe { each_container(found, context, callback) }
}
/// XGameSaveEnumerateContainerInfoByName(provider, prefix, context, callback).
pub(super) unsafe extern "system" fn enumerate_by_name(_: &Object, _p: *const u8, prefix: *const c_char, context: *mut c_void, callback: *mut c_void) -> i32 {
    let prefix = unsafe { text(prefix) };
    let found: Vec<_> = store::containers(&root()).into_iter().filter(|c| c.name.starts_with(&prefix)).collect();
    report(format_args!("save: enumerate containers {prefix:?}: {}", found.len()));
    unsafe { each_container(found, context, callback) }
}
/// XGameSaveGetRemainingQuota(provider, bytes*).
pub(super) unsafe extern "system" fn quota(_: &Object, _p: *const u8, out: *mut i64) -> i32 {
    report(format_args!("save: remaining quota"));
    if out.is_null() { return POINTER; }
    unsafe { out.write(QUOTA) };
    OK
}
pub(super) unsafe extern "system" fn quota_async(_: &Object, _p: *const u8, block: *mut c_void) -> i32 {
    report(format_args!("save: remaining quota (async)"));
    unsafe { complete(block, OK, 8, Payload::Empty) }
}
pub(super) unsafe extern "system" fn quota_result(_: &Object, block: *mut c_void, out: *mut i64) -> i32 {
    if out.is_null() { return POINTER; }
    match take(block) {
        Ok(_) => { unsafe { out.write(QUOTA) }; OK }
        Err(e) => e,
    }
}

fn status(result: std::io::Result<()>, what: std::fmt::Arguments<'_>) -> i32 {
    match result {
        Ok(()) => { report(format_args!("save: {what}: ok")); OK }
        Err(e) => { report(format_args!("save: {what}: failed: {e}")); WRITE_FAULT }
    }
}
pub(super) unsafe extern "system" fn delete(_: &Object, _p: *const u8, name: *const c_char) -> i32 {
    let name = unsafe { text(name) };
    status(store::delete(&root(), &name), format_args!("delete container {name:?}"))
}
pub(super) unsafe extern "system" fn delete_async(o: &Object, p: *const u8, name: *const c_char, block: *mut c_void) -> i32 {
    let result = unsafe { delete(o, p, name) };
    unsafe { complete(block, result, 0, Payload::Empty) }
}
/// Results of async operations that return only a status.
pub(super) unsafe extern "system" fn status_result(_: &Object, block: *mut c_void) -> i32 {
    take(block).map_or_else(|e| e, |_| OK)
}

/// XGameSaveCreateContainer(provider, name, container*).
pub(super) unsafe extern "system" fn create_container(_: &Object, _p: *const u8, name: *const c_char, out: *mut usize) -> i32 {
    let name = unsafe { text(name) };
    if out.is_null() { return POINTER; }
    if name.is_empty() || name.len() > 256 { return INVALID; }
    report(format_args!("save: create container {name:?}"));
    let h = handle();
    lock(&CONTAINERS).insert(h, Container { name });
    unsafe { out.write(h) };
    OK
}
pub(super) extern "system" fn close_container(_: &Object, container: usize) {
    lock(&CONTAINERS).remove(&container);
}
fn container_name(container: usize) -> Option<String> {
    lock(&CONTAINERS).get(&container).map(|c| c.name.clone())
}

unsafe fn each_blob(container: usize, prefix: &str, context: *mut c_void, callback: *mut c_void) -> i32 {
    if callback.is_null() { return POINTER; }
    let Some(name) = container_name(container) else { return INVALID };
    let blobs = store::container(&root(), &name).map_or(Vec::new(), |c| c.blobs);
    report(format_args!("save: enumerate blobs of {name:?} {prefix:?}: {blobs:?}"));
    let callback: BlobCallback = unsafe { core::mem::transmute(callback) };
    for (blob, size) in blobs.iter().filter(|b| b.0.starts_with(prefix)) {
        let blob = cstring(blob);
        if !unsafe { callback(&BlobInfo { name: blob.as_ptr(), size: *size }, context) } { break; }
    }
    OK
}
/// XGameSaveEnumerateBlobInfo(container, context, callback).
pub(super) unsafe extern "system" fn blobs(_: &Object, container: usize, context: *mut c_void, callback: *mut c_void) -> i32 {
    unsafe { each_blob(container, "", context, callback) }
}
pub(super) unsafe extern "system" fn blobs_by_name(_: &Object, container: usize, prefix: *const c_char, context: *mut c_void, callback: *mut c_void) -> i32 {
    unsafe { each_blob(container, &text(prefix), context, callback) }
}

/// Null names select every blob in the container.
unsafe fn load(container: usize, names: *const *const c_char, count: u32) -> Result<Blobs, i32> {
    let name = container_name(container).ok_or(INVALID)?;
    let wanted: Vec<String> = if names.is_null() {
        store::container(&root(), &name).map_or(Vec::new(), |c| c.blobs.into_iter().map(|b| b.0).collect())
    } else {
        (0..count as usize).map(|i| unsafe { text(*names.add(i)) }).collect()
    };
    let loaded: Result<Blobs, i32> = wanted.into_iter()
        .map(|b| store::read(&root(), &name, &b).map(|d| (b, d)).ok_or(BLOB_NOT_FOUND)).collect();
    match &loaded {
        Ok(b) => report(format_args!("save: read {name:?}: {:?}", b.iter().map(|b| (&b.0, b.1.len())).collect::<Vec<_>>())),
        Err(e) => report(format_args!("save: read {name:?}: failed {e:#x}")),
    }
    loaded
}
fn needed(blobs: &Blobs) -> usize {
    blobs.iter().map(|(n, d)| size_of::<Blob>() + n.len() + 1 + d.len()).sum()
}
/// Lay out the records first, then blob data, then the names they point at.
unsafe fn place(blobs: &Blobs, size: usize, out: *mut Blob) -> i32 {
    if size < needed(blobs) { return INSUFFICIENT_BUFFER; }
    if blobs.is_empty() { return OK; }
    if out.is_null() { return POINTER; }
    unsafe {
        let mut at = out.add(blobs.len()).cast::<u8>();
        for (i, (_, data)) in blobs.iter().enumerate() {
            core::ptr::copy_nonoverlapping(data.as_ptr(), at, data.len());
            (&raw mut (*out.add(i)).data).write(at);
            at = at.add(data.len());
        }
        for (i, (name, data)) in blobs.iter().enumerate() {
            core::ptr::copy_nonoverlapping(name.as_ptr(), at, name.len());
            at.add(name.len()).write(0);
            (&raw mut (*out.add(i)).info).write(BlobInfo { name: at.cast(), size: data.len() as u32 });
            at = at.add(name.len() + 1);
        }
    }
    OK
}
/// XGameSaveReadBlobData(container, names, count*, size, blobs).
pub(super) unsafe extern "system" fn read(_: &Object, container: usize, names: *const *const c_char, count: *mut u32, size: usize, out: *mut Blob) -> i32 {
    if count.is_null() { return POINTER; }
    match unsafe { load(container, names, *count) } {
        Ok(blobs) => {
            let result = unsafe { place(&blobs, size, out) };
            if result == OK { unsafe { count.write(blobs.len() as u32) } }
            result
        }
        Err(e) => e,
    }
}
/// XGameSaveReadBlobDataAsync(container, names, count, async).
pub(super) unsafe extern "system" fn read_async(_: &Object, container: usize, names: *const *const c_char, count: u32, block: *mut c_void) -> i32 {
    match unsafe { load(container, names, count) } {
        Ok(blobs) => {
            let size = needed(&blobs);
            lock(&READS).insert(block as usize, blobs);
            unsafe { complete(block, OK, size, Payload::Empty) }
        }
        Err(e) => unsafe { complete(block, e, 0, Payload::Empty) },
    }
}
/// XGameSaveReadBlobDataResult(async, size, blobs, count*).
pub(super) unsafe extern "system" fn read_result(_: &Object, block: *mut c_void, size: usize, out: *mut Blob, count: *mut u32) -> i32 {
    let mut reads = lock(&READS);
    let Some(blobs) = reads.get(&(block as usize)) else { return take(block).map_or_else(|e| e, |_| INVALID) };
    let result = unsafe { place(blobs, size, out) };
    if result == OK {
        if !count.is_null() { unsafe { count.write(blobs.len() as u32) } }
        reads.remove(&(block as usize));
        let _ = take(block);
    }
    result
}

/// XGameSaveCreateUpdate(container, displayName, update*).
pub(super) unsafe extern "system" fn create_update(_: &Object, container: usize, display: *const c_char, out: *mut usize) -> i32 {
    if out.is_null() { return POINTER; }
    let Some(container) = container_name(container) else { return INVALID };
    let display = unsafe { text(display) };
    report(format_args!("save: create update for {container:?} display {display:?}"));
    let h = handle();
    lock(&UPDATES).insert(h, Update { container, display, changes: Vec::new() });
    unsafe { out.write(h) };
    OK
}
pub(super) extern "system" fn close_update(_: &Object, update: usize) {
    lock(&UPDATES).remove(&update);
}
/// XGameSaveSubmitBlobWrite(update, name, data, size).
pub(super) unsafe extern "system" fn blob_write(_: &Object, update: usize, name: *const c_char, data: *const u8, size: usize) -> i32 {
    let name = unsafe { text(name) };
    report(format_args!("save: stage write {name:?} {size} bytes"));
    if name.is_empty() || size > QUOTA as usize || (data.is_null() && size != 0) { return INVALID; }
    let data = if size == 0 { Vec::new() } else { unsafe { core::slice::from_raw_parts(data, size) }.to_vec() };
    match lock(&UPDATES).get_mut(&update) {
        Some(u) => { u.changes.push((name, Some(data))); OK }
        None => INVALID,
    }
}
pub(super) unsafe extern "system" fn blob_delete(_: &Object, update: usize, name: *const c_char) -> i32 {
    let name = unsafe { text(name) };
    report(format_args!("save: stage delete {name:?}"));
    match lock(&UPDATES).get_mut(&update) {
        Some(u) if !name.is_empty() => { u.changes.push((name, None)); OK }
        _ => INVALID,
    }
}
fn commit(update: usize) -> i32 {
    let mut updates = lock(&UPDATES);
    let Some(u) = updates.get_mut(&update) else { return INVALID };
    // The last staged change to a blob wins.
    let mut changes: Vec<(String, Option<Vec<u8>>)> = Vec::new();
    for change in std::mem::take(&mut u.changes) {
        changes.retain(|c| c.0 != change.0);
        changes.push(change);
    }
    status(store::commit(&root(), &u.container, &u.display, &changes), format_args!("commit {} changes to {:?}", changes.len(), u.container))
}
pub(super) extern "system" fn submit(_: &Object, update: usize) -> i32 {
    commit(update)
}
pub(super) unsafe extern "system" fn submit_async(_: &Object, update: usize, block: *mut c_void) -> i32 {
    let result = commit(update);
    unsafe { complete(block, result, 0, Payload::Empty) }
}
