//! XAsync / XAsyncProvider (contract: libHttpClient XAsync.h + XAsyncProvider.h, MIT).
//!
//! `XAsyncBlock` = { queue, context, callback, internal[32] }. While an operation is active
//! (or waiting for its non-empty result to be consumed), its `State` lives in `ACTIVE` keyed
//! by block address + a monotonic generation id stored in `internal`. Once completed with no
//! payload (or after `XAsyncGetResult` consumes the payload), the `State` is removed from
//! `ACTIVE` while a compact completion marker remains in `internal`, preventing leaks on
//! stack-allocated `XAsyncBlock`s.
//!
//! A title that reaches the runtime through the GDK thunks (`api.rs`) gets its task queues and
//! async blocks from the PC's XGameRuntime when Gaming Services is installed; then these
//! functions – and so every XCL async API (`begin_with`) – go to the PC runtime's XAsync
//! object (`use_pc`) instead.

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

use super::taskqueue::{self, Item, PORT_COMPLETION, PORT_WORK};
use crate::util::{E_ABORT, E_INVALIDARG, E_NOT_SUFFICIENT_BUFFER, E_PENDING, HRESULT, S_OK};

pub const OP_BEGIN: u32 = 0;
pub const OP_DO_WORK: u32 = 1;
pub const OP_GET_RESULT: u32 = 2;
pub const OP_CANCEL: u32 = 3;
pub const OP_CLEANUP: u32 = 4;

pub const E_ILLEGAL_METHOD_CALL: HRESULT = 0x8000_000Eu32 as i32;

const BLOCK_MAGIC: u32 = 0x5841_5359; // "XASY"
const FLAG_DONE: u8 = 1 << 0;
const FLAG_CONSUMED: u8 = 1 << 1;

#[repr(C)]
pub struct XAsyncBlock {
    pub queue: *mut c_void,
    pub context: *mut c_void,
    pub callback: usize,
    pub internal: [u8; 32],
}

#[repr(C)]
pub struct XAsyncProviderData {
    pub async_: *mut XAsyncBlock,
    pub buffer_size: usize,
    pub buffer: *mut c_void,
    pub context: *mut c_void,
}

pub type XAsyncProvider = unsafe extern "system" fn(u32, *const XAsyncProviderData) -> HRESULT;

/// The PC runtime's XAsync / XTaskQueue object (0 = XCL's own queues). Its slots: 3 GetStatus,
/// 4 GetResultSize, 5 Cancel, 6 Run, 7 Begin, 9 Schedule, 10 Complete, 11 GetResult.
static PC_ASYNC: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// tests that use XCL's queues and the one that switches to the PC runtime take turns
#[cfg(test)]
pub static ASYNC_TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn use_pc(object: usize) {
    PC_ASYNC.store(object, Ordering::Release);
}

/// (object, slot function) of the PC XAsync object, when in use
unsafe fn pc(slot: usize) -> Option<(usize, usize)> {
    let o = PC_ASYNC.load(Ordering::Acquire);
    (o != 0).then(|| (o, *(*(o as *const *const usize)).add(slot)))
}

/// XTaskQueueSubmitCallback on whichever runtime owns the title's task queues (the PC's
/// XAsync object when in use – its queue handles mean nothing to XCL's – else XCL's).
pub unsafe fn submit_callback(queue: *mut c_void, port: u32, ctx: *mut c_void, cb: usize) -> HRESULT {
    if let Some((o, f)) = pc(18) {
        let f: unsafe extern "system" fn(usize, *mut c_void, u32, *mut c_void, usize) -> HRESULT = std::mem::transmute(f);
        return f(o, queue, port, ctx, cb);
    }
    super::taskqueue::XTaskQueueSubmitCallback(queue, port, ctx, cb)
}

pub type XAsyncWork = unsafe extern "system" fn(*mut XAsyncBlock) -> HRESULT;
type Completion = unsafe extern "system" fn(*mut XAsyncBlock);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Operation in progress; not yet completing.
    Pending,
    /// `XAsyncComplete` claimed completion and is running optional cleanup / finalizing header.
    Completing,
    /// Operation finished; if `result_size > 0`, result has not yet been claimed.
    Done,
    /// `XAsyncGetResult` claimed the result and is running `OP_GET_RESULT` + `OP_CLEANUP`.
    Consuming,
    /// Result has been consumed and cleaned up (or `result_size == 0` / error was cleaned up).
    Consumed,
}

struct Inner {
    status: HRESULT,
    result_size: usize,
    phase: Phase,
}

impl Inner {
    fn is_done(&self) -> bool {
        !matches!(self.phase, Phase::Pending | Phase::Completing)
    }
}

pub struct State {
    id: u64,
    provider: usize,
    context: usize,
    identity: usize,
    /// For XAsyncRun.
    work: usize,
    inner: Mutex<Inner>,
    cv: Condvar,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

fn active_map() -> &'static Mutex<HashMap<usize, Arc<State>>> {
    static ACTIVE: OnceLock<Mutex<HashMap<usize, Arc<State>>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Clone, Copy)]
struct BlockHeader {
    status: HRESULT,
    flags: u8,
    id: u64,
}

unsafe fn write_header(b: *mut XAsyncBlock, h: BlockHeader) {
    let dst = (*b).internal.as_mut_ptr();
    std::ptr::write_unaligned(dst as *mut u32, BLOCK_MAGIC);
    std::ptr::write_unaligned(dst.add(4) as *mut i32, h.status);
    *dst.add(8) = h.flags;
    std::ptr::write_unaligned(dst.add(12) as *mut u64, h.id);
}

unsafe fn read_header(b: *mut XAsyncBlock) -> Option<BlockHeader> {
    if b.is_null() {
        return None;
    }
    let src = (*b).internal.as_ptr();
    if std::ptr::read_unaligned(src as *const u32) != BLOCK_MAGIC {
        return None;
    }
    Some(BlockHeader {
        status: std::ptr::read_unaligned(src.add(4) as *const i32),
        flags: *src.add(8),
        id: std::ptr::read_unaligned(src.add(12) as *const u64),
    })
}

enum Lookup {
    Active(Arc<State>),
    Finished(BlockHeader),
    Invalid,
}

/// Atomically look up an `XAsyncBlock`'s live state or finalized completion header under the
/// `ACTIVE` mutex so completion/removal synchronizes cleanly with concurrent readers.
unsafe fn lookup(b: *mut XAsyncBlock) -> Lookup {
    let map = active_map().lock().unwrap();
    let Some(hdr) = read_header(b) else { return Lookup::Invalid };
    if let Some(s) = map.get(&(b as usize)) {
        if s.id == hdr.id {
            return Lookup::Active(s.clone());
        }
    }
    if hdr.flags & FLAG_DONE != 0 {
        Lookup::Finished(hdr)
    } else {
        Lookup::Invalid
    }
}

unsafe fn register_state(b: *mut XAsyncBlock, s: Arc<State>) {
    let mut map = active_map().lock().unwrap();
    write_header(b, BlockHeader { status: E_PENDING, flags: 0, id: s.id });
    map.insert(b as usize, s);
}

unsafe fn call_provider(s: &State, op: u32, b: *mut XAsyncBlock, buffer: *mut c_void, size: usize) -> HRESULT {
    if s.provider == 0 {
        return S_OK;
    }
    let data = XAsyncProviderData { async_: b, buffer_size: size, buffer, context: s.context as *mut c_void };
    let f: XAsyncProvider = std::mem::transmute(s.provider);
    f(op, &data)
}

unsafe extern "system" fn do_work_cb(ctx: *mut c_void, canceled: bool) {
    let b = ctx as *mut XAsyncBlock;
    let Lookup::Active(s) = lookup(b) else { return };
    if canceled {
        XAsyncComplete(b, E_ABORT, 0);
        return;
    }
    let hr = if s.work != 0 {
        let f: XAsyncWork = std::mem::transmute(s.work);
        f(b)
    } else {
        call_provider(&s, OP_DO_WORK, b, std::ptr::null_mut(), 0)
    };
    if hr != E_PENDING {
        // providers that finish synchronously in DoWork may not call XAsyncComplete
        let pending = s.inner.lock().unwrap().phase == Phase::Pending;
        if pending {
            XAsyncComplete(b, hr, 0);
        }
    }
}

unsafe extern "system" fn completion_cb(ctx: *mut c_void, _canceled: bool) {
    let b = ctx as *mut XAsyncBlock;
    if (*b).callback != 0 {
        let f: Completion = std::mem::transmute((*b).callback);
        f(b);
    }
}

pub unsafe extern "system" fn XAsyncBegin(
    b: *mut XAsyncBlock,
    context: *mut c_void,
    identity: *const c_void,
    _identity_name: *const u8,
    provider: usize,
) -> HRESULT {
    if let Some((o, f)) = pc(7) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, *mut c_void, *const c_void, *const u8, usize) -> HRESULT = std::mem::transmute(f);
        return f(o, b, context, identity, _identity_name, provider);
    }
    if b.is_null() || provider == 0 {
        return E_INVALIDARG;
    }
    let s = Arc::new(State {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        provider,
        context: context as usize,
        identity: identity as usize,
        work: 0,
        inner: Mutex::new(Inner { status: E_PENDING, result_size: 0, phase: Phase::Pending }),
        cv: Condvar::new(),
    });
    register_state(b, s.clone());
    // A failing Begin fails the async call, not XAsyncBegin (libHttpClient): the caller gets
    // the error once, through the block, and not also as this return value.
    let hr = call_provider(&s, OP_BEGIN, b, std::ptr::null_mut(), 0);
    if hr < 0 {
        XAsyncComplete(b, hr, 0);
    }
    S_OK
}

pub unsafe extern "system" fn XAsyncSchedule(b: *mut XAsyncBlock, delay_ms: u32) -> HRESULT {
    if let Some((o, f)) = pc(9) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, u32) -> HRESULT = std::mem::transmute(f);
        return f(o, b, delay_ms);
    }
    if b.is_null() {
        return E_INVALIDARG;
    }
    let Some(q) = taskqueue::queue_from((*b).queue) else {
        return if (*b).queue.is_null() { taskqueue::E_NO_TASK_QUEUE } else { E_INVALIDARG };
    };
    // a terminated queue takes no new work (libHttpClient: XTaskQueueSubmitDelayedCallback)
    if !taskqueue::submit_delayed(&q, PORT_WORK, delay_ms, Item { cb: do_work_cb as usize, ctx: b as usize }) {
        return E_ABORT;
    }
    S_OK
}

pub unsafe extern "system" fn XAsyncComplete(b: *mut XAsyncBlock, result: HRESULT, required: usize) {
    if let Some((o, f)) = pc(10) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, HRESULT, usize) = std::mem::transmute(f);
        return f(o, b, result, required);
    }
    // E_PENDING is not a completion: the provider still has work to do (libHttpClient).
    if result == E_PENDING {
        return;
    }
    let Lookup::Active(s) = lookup(b) else { return };
    // Atomically claim completion while keeping `is_done() == false` until `OP_CLEANUP` and
    // `write_header` finish, so waiters/pollers cannot destroy `b` or race on `FLAG_CONSUMED`.
    let (status, cleanup_now);
    {
        let mut g = s.inner.lock().unwrap();
        if g.phase != Phase::Pending {
            return;
        }
        g.phase = Phase::Completing;
        status = result;
        g.status = status;
        g.result_size = if status >= 0 { required } else { 0 };
        cleanup_now = g.result_size == 0;
    }
    // Snapshot callback/queue before publishing completion so a waiter woken from
    // `XAsyncGetStatus(b, true)` can safely destroy a stack-allocated `XAsyncBlock`.
    let (cb, queue_ptr) = ((*b).callback, (*b).queue);
    if cleanup_now {
        call_provider(&s, OP_CLEANUP, b, std::ptr::null_mut(), 0);
    }
    // Persist the finalized status into `b.internal` under `ACTIVE`, remove `s` if no payload
    // remains, and publish `Done`/`Consumed` (lock order: `ACTIVE` -> `s.inner`) before
    // submitting the completion callback or waking waiters.
    {
        let mut map = active_map().lock().unwrap();
        write_header(b, BlockHeader { status, flags: FLAG_DONE, id: s.id });
        if cleanup_now && map.get(&(b as usize)).is_some_and(|cur| cur.id == s.id) {
            map.remove(&(b as usize));
        }
        let mut g = s.inner.lock().unwrap();
        g.phase = if cleanup_now { Phase::Consumed } else { Phase::Done };
    }
    if cb != 0 {
        if let Some(q) = taskqueue::queue_from(queue_ptr) {
            let _ = taskqueue::submit_now(&q, PORT_COMPLETION, Item { cb: completion_cb as usize, ctx: b as usize });
        }
    }
    s.cv.notify_all();
}

pub unsafe extern "system" fn XAsyncGetStatus(b: *mut XAsyncBlock, wait: bool) -> HRESULT {
    if let Some((o, f)) = pc(3) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, bool) -> HRESULT = std::mem::transmute(f);
        return f(o, b, wait);
    }
    match lookup(b) {
        Lookup::Active(s) => {
            let mut g = s.inner.lock().unwrap();
            if wait {
                while !g.is_done() {
                    g = s.cv.wait(g).unwrap();
                }
            }
            if g.is_done() { g.status } else { E_PENDING }
        }
        Lookup::Finished(h) => h.status,
        Lookup::Invalid => E_INVALIDARG,
    }
}

pub unsafe extern "system" fn XAsyncGetResultSize(b: *mut XAsyncBlock, size: *mut usize) -> HRESULT {
    if let Some((o, f)) = pc(4) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, *mut usize) -> HRESULT = std::mem::transmute(f);
        return f(o, b, size);
    }
    match lookup(b) {
        Lookup::Active(s) => {
            let g = s.inner.lock().unwrap();
            if !g.is_done() {
                return E_PENDING;
            }
            if !size.is_null() {
                *size = g.result_size;
            }
            g.status
        }
        Lookup::Finished(h) => {
            if !size.is_null() && h.status >= 0 {
                *size = 0;
            }
            h.status
        }
        Lookup::Invalid => E_INVALIDARG,
    }
}

pub unsafe extern "system" fn XAsyncGetResult(
    b: *mut XAsyncBlock,
    identity: *const c_void,
    size: usize,
    buffer: *mut c_void,
    used: *mut usize,
) -> HRESULT {
    if let Some((o, f)) = pc(11) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, *const c_void, usize, *mut c_void, *mut usize) -> HRESULT = std::mem::transmute(f);
        return f(o, b, identity, size, buffer, used);
    }
    match lookup(b) {
        Lookup::Invalid => E_INVALIDARG,
        Lookup::Finished(h) => {
            if h.status < 0 {
                return h.status;
            }
            if h.flags & FLAG_CONSUMED != 0 {
                return E_ILLEGAL_METHOD_CALL;
            }
            if !used.is_null() {
                *used = 0;
            }
            S_OK
        }
        Lookup::Active(s) => {
            let (status, need) = {
                let mut g = s.inner.lock().unwrap();
                if !g.is_done() {
                    return E_PENDING;
                }
                if !identity.is_null() && s.identity != 0 && identity as usize != s.identity {
                    return E_INVALIDARG;
                }
                if g.status < 0 {
                    return g.status;
                }
                if g.result_size > 0 {
                    if g.phase != Phase::Done {
                        return E_ILLEGAL_METHOD_CALL;
                    }
                    if size < g.result_size || buffer.is_null() {
                        return E_NOT_SUFFICIENT_BUFFER;
                    }
                    // Atomically claim the single result-consumption / OP_CLEANUP slot.
                    g.phase = Phase::Consuming;
                }
                (g.status, g.result_size)
            };
            if need > 0 {
                let hr = call_provider(&s, OP_GET_RESULT, b, buffer, size);
                if !used.is_null() {
                    *used = need;
                }
                call_provider(&s, OP_CLEANUP, b, std::ptr::null_mut(), 0);
                let mut map = active_map().lock().unwrap();
                write_header(b, BlockHeader { status, flags: FLAG_DONE | FLAG_CONSUMED, id: s.id });
                if map.get(&(b as usize)).is_some_and(|cur| cur.id == s.id) {
                    map.remove(&(b as usize));
                }
                s.inner.lock().unwrap().phase = Phase::Consumed;
                return hr;
            }
            if !used.is_null() {
                *used = 0;
            }
            S_OK
        }
    }
}

pub unsafe extern "system" fn XAsyncCancel(b: *mut XAsyncBlock) {
    if let Some((o, f)) = pc(5) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock) = std::mem::transmute(f);
        return f(o, b);
    }
    let Lookup::Active(s) = lookup(b) else { return };
    if s.inner.lock().unwrap().phase != Phase::Pending {
        return;
    }
    // Cancellation is the provider's to do, as in libHttpClient's XAsyncCancel: one that
    // supports it completes with E_ABORT (now or later), one that does not (XAsyncRun, XCL's
    // own `begin_with`) runs to its normal end. Completing here would run the provider's
    // cleanup while its DoWork may still be running.
    let _ = call_provider(&s, OP_CANCEL, b, std::ptr::null_mut(), 0);
}

pub unsafe extern "system" fn XAsyncRun(b: *mut XAsyncBlock, work: usize) -> HRESULT {
    if let Some((o, f)) = pc(6) {
        let f: unsafe extern "system" fn(usize, *mut XAsyncBlock, usize) -> HRESULT = std::mem::transmute(f);
        return f(o, b, work);
    }
    if b.is_null() || work == 0 {
        return E_INVALIDARG;
    }
    let s = Arc::new(State {
        id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
        provider: 0,
        context: 0,
        identity: 0,
        work,
        inner: Mutex::new(Inner { status: E_PENDING, result_size: 0, phase: Phase::Pending }),
        cv: Condvar::new(),
    });
    register_state(b, s);
    let hr = XAsyncSchedule(b, 0);
    if hr < 0 {
        XAsyncComplete(b, hr, 0);
    }
    S_OK
}

// ---------------------------------------------------------------------------------------
// Helper for our own async APIs: run `work` on the queue, complete with a byte payload
// (with optional relative-to-buffer pointer fixups applied in OP_GET_RESULT).
// ---------------------------------------------------------------------------------------

/// Serialized async result bytes plus byte offsets within `bytes` holding relative `usize`
/// offsets that must be relocated by adding the destination buffer's base address.
pub struct PayloadBuf {
    pub bytes: Vec<u8>,
    pub fixups: Vec<usize>,
}

struct Payload {
    result: Mutex<Option<Result<PayloadBuf, HRESULT>>>,
    work: Mutex<Option<Box<dyn FnOnce() -> Result<PayloadBuf, HRESULT> + Send>>>,
}

unsafe extern "system" fn payload_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
    let d = &*data;
    let p = &*(d.context as *const Payload);
    match op {
        OP_BEGIN => XAsyncSchedule(d.async_, 0),
        OP_DO_WORK => {
            let w = p.work.lock().unwrap().take();
            let r = w.map(|f| f()).unwrap_or(Err(crate::util::E_FAIL));
            let (hr, size) = match &r {
                Ok(pb) => (S_OK, pb.bytes.len()),
                Err(e) => (*e, 0),
            };
            *p.result.lock().unwrap() = Some(r);
            XAsyncComplete(d.async_, hr, size);
            S_OK
        }
        OP_GET_RESULT => {
            if let Some(Ok(pb)) = p.result.lock().unwrap().as_ref() {
                let n = pb.bytes.len().min(d.buffer_size);
                let dst = d.buffer as *mut u8;
                std::ptr::copy_nonoverlapping(pb.bytes.as_ptr(), dst, n);
                let base = dst as usize;
                for &off in &pb.fixups {
                    if off + std::mem::size_of::<usize>() <= n {
                        let slot = dst.add(off) as *mut usize;
                        let rel = std::ptr::read_unaligned(slot);
                        std::ptr::write_unaligned(slot, base.wrapping_add(rel));
                    }
                }
            }
            S_OK
        }
        OP_CLEANUP => {
            drop(Box::from_raw(d.context as *mut Payload));
            S_OK
        }
        _ => S_OK,
    }
}

/// Start an async op whose result is `work()`'s bytes; `identity` ties GetResult calls.
pub unsafe fn begin_with<F>(b: *mut XAsyncBlock, identity: usize, work: F) -> HRESULT
where
    F: FnOnce() -> Result<Vec<u8>, HRESULT> + Send + 'static,
{
    begin_with_fixups(b, identity, move || work().map(|bytes| PayloadBuf { bytes, fixups: Vec::new() }))
}

/// Start an async op whose result buffer contains internal pointers relocated in `OP_GET_RESULT`.
pub unsafe fn begin_with_fixups<F>(b: *mut XAsyncBlock, identity: usize, work: F) -> HRESULT
where
    F: FnOnce() -> Result<PayloadBuf, HRESULT> + Send + 'static,
{
    let p = Box::into_raw(Box::new(Payload { result: Mutex::new(None), work: Mutex::new(Some(Box::new(work))) }));
    let hr = XAsyncBegin(b, p as *mut c_void, identity as *const c_void, std::ptr::null(), payload_provider as usize);
    if hr < 0 {
        // SAFETY: `XAsyncBegin` rejected the call before adopting `p`, so `OP_CLEANUP` will
        // never run; reclaim the `Box<Payload>` allocated above to avoid leaking it.
        drop(Box::from_raw(p));
    }
    hr
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;

    static DONE: AtomicU32 = AtomicU32::new(0);
    unsafe extern "system" fn on_done(_b: *mut XAsyncBlock) {
        DONE.fetch_add(1, Ordering::SeqCst);
    }

    #[test]
    fn begin_with_payload_roundtrip() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            b.callback = on_done as usize;
            let id = 0x1234usize;
            assert_eq!(begin_with(&mut b, id, || Ok(b"hello".to_vec())), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, true), S_OK);
            let mut size = 0usize;
            assert_eq!(XAsyncGetResultSize(&mut b, &mut size), S_OK);
            assert_eq!(size, 5);
            let mut buf = [0u8; 8];
            let mut used = 0usize;
            assert_eq!(XAsyncGetResult(&mut b, id as _, 2, buf.as_mut_ptr() as _, &mut used), E_NOT_SUFFICIENT_BUFFER);
            assert_eq!(XAsyncGetResult(&mut b, id as _, 8, buf.as_mut_ptr() as _, &mut used), S_OK);
            assert_eq!(&buf[..5], b"hello");
            assert_eq!(used, 5);
            // Second read of a consumed result returns E_ILLEGAL_METHOD_CALL, and the active map is clean.
            assert_eq!(XAsyncGetResult(&mut b, id as _, 8, buf.as_mut_ptr() as _, &mut used), E_ILLEGAL_METHOD_CALL);
            assert_eq!(XAsyncGetStatus(&mut b, false), S_OK);
            assert!(!active_map().lock().unwrap().contains_key(&(&mut b as *mut XAsyncBlock as usize)));
            // completion callback is delivered on the (thread pool) completion port
            for _ in 0..200 {
                if DONE.load(Ordering::SeqCst) > 0 {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert!(DONE.load(Ordering::SeqCst) > 0);
        }
    }

    #[test]
    fn begin_with_fixups_relocates_embedded_pointers() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            let id = 0x5678usize;
            assert_eq!(
                begin_with_fixups(&mut b, id, || {
                    let mut bytes = Vec::new();
                    bytes.extend_from_slice(&16usize.to_le_bytes()); // ptr 1 -> +16
                    bytes.extend_from_slice(&20usize.to_le_bytes()); // ptr 2 -> +20
                    bytes.extend_from_slice(b"abc\0def\0");
                    Ok(PayloadBuf { bytes, fixups: vec![0, 8] })
                }),
                S_OK
            );
            assert_eq!(XAsyncGetStatus(&mut b, true), S_OK);
            let mut out = [0u8; 32];
            let mut used = 0usize;
            assert_eq!(XAsyncGetResult(&mut b, id as _, out.len(), out.as_mut_ptr() as _, &mut used), S_OK);
            let p1 = std::ptr::read_unaligned(out.as_ptr() as *const *const u8);
            let p2 = std::ptr::read_unaligned(out.as_ptr().add(8) as *const *const u8);
            assert_eq!(crate::util::from_cstr_ptr(p1).as_deref(), Some("abc"));
            assert_eq!(crate::util::from_cstr_ptr(p2).as_deref(), Some("def"));
        }
    }

    #[test]
    fn errors_propagate_and_release_active_state() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            begin_with(&mut b, 1, || Err(crate::util::E_FAIL));
            assert_eq!(XAsyncGetStatus(&mut b, true), crate::util::E_FAIL);
            assert!(!active_map().lock().unwrap().contains_key(&(&mut b as *mut XAsyncBlock as usize)));
        }
    }

    /// A provider that does nothing on Cancel: the operation goes on and completes normally.
    unsafe extern "system" fn ignores_cancel(_op: u32, _data: *const XAsyncProviderData) -> HRESULT {
        S_OK
    }

    /// A provider that supports Cancel by completing with E_ABORT.
    unsafe extern "system" fn aborts_on_cancel(op: u32, data: *const XAsyncProviderData) -> HRESULT {
        if op == OP_CANCEL {
            XAsyncComplete((*data).async_, E_ABORT, 0);
        }
        S_OK
    }

    #[test]
    fn cancel_is_the_providers_to_do() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            assert_eq!(XAsyncBegin(&mut b, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), ignores_cancel as usize), S_OK);
            XAsyncCancel(&mut b);
            assert_eq!(XAsyncGetStatus(&mut b, false), E_PENDING);
            XAsyncComplete(&mut b, S_OK, 0);
            assert_eq!(XAsyncGetStatus(&mut b, true), S_OK);

            let mut b: XAsyncBlock = std::mem::zeroed();
            assert_eq!(XAsyncBegin(&mut b, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), aborts_on_cancel as usize), S_OK);
            XAsyncCancel(&mut b);
            assert_eq!(XAsyncGetStatus(&mut b, false), E_ABORT);
        }
    }

    /// XCL's own operations ignore Cancel: work already running finishes and its result is
    /// read, instead of the payload being freed under it.
    #[test]
    fn cancel_lets_running_work_finish() {
        use std::sync::atomic::AtomicBool;
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let started = Arc::new(AtomicBool::new(false));
        let release = Arc::new(AtomicBool::new(false));
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            let (s, r) = (started.clone(), release.clone());
            let work = move || {
                s.store(true, Ordering::SeqCst);
                while !r.load(Ordering::SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Ok(b"done".to_vec())
            };
            assert_eq!(begin_with(&mut b, 9, work), S_OK);
            while !started.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            XAsyncCancel(&mut b);
            let pending = XAsyncGetStatus(&mut b, false);
            release.store(true, Ordering::SeqCst);
            assert_eq!(pending, E_PENDING);
            assert_eq!(XAsyncGetStatus(&mut b, true), S_OK);
            let mut buf = [0u8; 4];
            let mut used = 0usize;
            assert_eq!(XAsyncGetResult(&mut b, 9 as _, 4, buf.as_mut_ptr() as _, &mut used), S_OK);
            assert_eq!(&buf, b"done");
        }
    }

    unsafe extern "system" fn run_work(_b: *mut XAsyncBlock) -> HRESULT {
        7
    }

    #[test]
    fn xasync_run() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            assert_eq!(XAsyncRun(&mut b, run_work as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, true), 7);
            assert!(!active_map().lock().unwrap().contains_key(&(&mut b as *mut XAsyncBlock as usize)));
        }
    }

    /// Begin fails (or completes) without scheduling anything; the test completes it.
    unsafe extern "system" fn fails_to_begin(op: u32, _data: *const XAsyncProviderData) -> HRESULT {
        if op == OP_BEGIN { crate::util::E_FAIL } else { S_OK }
    }
    unsafe extern "system" fn begins_without_work(_op: u32, _data: *const XAsyncProviderData) -> HRESULT {
        S_OK
    }

    #[test]
    fn a_failing_begin_fails_the_call_not_xasyncbegin() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            assert_eq!(XAsyncBegin(&mut b, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), fails_to_begin as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, false), crate::util::E_FAIL);
            // an XAsyncRun on a queue handle XCL does not know: failed through the block too
            let mut b: XAsyncBlock = std::mem::zeroed();
            b.queue = 0x10 as *mut c_void;
            assert_eq!(XAsyncRun(&mut b, run_work as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, false), E_INVALIDARG);
        }
    }

    #[test]
    fn completing_with_e_pending_does_not_complete() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut b: XAsyncBlock = std::mem::zeroed();
            assert_eq!(XAsyncBegin(&mut b, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), begins_without_work as usize), S_OK);
            XAsyncComplete(&mut b, E_PENDING, 0);
            assert_eq!(XAsyncGetStatus(&mut b, false), E_PENDING);
            XAsyncComplete(&mut b, 3, 0);
            assert_eq!(XAsyncGetStatus(&mut b, true), 3);
        }
    }

    /// A terminated queue takes no new async work: the call fails with E_ABORT through the
    /// block, and the work never runs.
    #[test]
    fn async_work_on_a_terminated_queue_is_aborted() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let mut q = std::ptr::null_mut();
            assert_eq!(taskqueue::XTaskQueueCreate(taskqueue::MODE_THREAD_POOL, taskqueue::MODE_THREAD_POOL, &mut q), S_OK);
            assert_eq!(taskqueue::XTaskQueueTerminate(q, true, std::ptr::null_mut(), 0), S_OK);
            let mut b: XAsyncBlock = std::mem::zeroed();
            b.queue = q;
            assert_eq!(XAsyncRun(&mut b, run_work as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, true), E_ABORT);
            let mut b: XAsyncBlock = std::mem::zeroed();
            b.queue = q;
            assert_eq!(begin_with(&mut b, 9, || -> Result<Vec<u8>, HRESULT> { panic!("work ran on a terminated queue") }), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b, true), E_ABORT);
            taskqueue::XTaskQueueCloseHandle(q);
        }
    }

    #[test]
    fn terminate_manual_completion_port_with_delayed_async_does_not_deadlock() {
        use std::sync::mpsc;
        use std::time::Duration;
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        static CB_HITS: AtomicU32 = AtomicU32::new(0);
        unsafe extern "system" fn count_cb(_b: *mut XAsyncBlock) {
            CB_HITS.fetch_add(1, Ordering::SeqCst);
        }
        unsafe extern "system" fn delayed_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
            if op == OP_BEGIN {
                return XAsyncSchedule((*data).async_, 60_000);
            }
            S_OK
        }

        // Stress across both Manual/Manual and ThreadPool/Manual queues.
        for iter in 0..50 {
            let work_mode = if iter % 2 == 0 { taskqueue::MODE_MANUAL } else { taskqueue::MODE_THREAD_POOL };
            unsafe {
                let mut q = std::ptr::null_mut();
                assert_eq!(taskqueue::XTaskQueueCreate(work_mode, taskqueue::MODE_MANUAL, &mut q), S_OK);
                let before = CB_HITS.load(Ordering::SeqCst);
                let mut b = Box::new(std::mem::zeroed::<XAsyncBlock>());
                b.queue = q;
                b.callback = count_cb as usize;
                assert_eq!(
                    XAsyncBegin(&mut *b, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), delayed_provider as usize),
                    S_OK
                );

                let q_addr = q as usize;
                let (tx, rx) = mpsc::channel();
                let handle = std::thread::spawn(move || {
                    let hr = taskqueue::XTaskQueueTerminate(q_addr as *mut c_void, true, std::ptr::null_mut(), 0);
                    let _ = tx.send(hr);
                });
                let hr = rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap_or_else(|_| panic!("XTaskQueueTerminate deadlocked on iter {iter}"));
                handle.join().unwrap();
                assert_eq!(hr, S_OK);
                assert_eq!(XAsyncGetStatus(&mut *b, false), E_ABORT);
                assert_eq!(CB_HITS.load(Ordering::SeqCst), before, "completion callback must not run on terminated queue");
                assert!(taskqueue::XTaskQueueIsEmpty(q, PORT_WORK));
                assert!(taskqueue::XTaskQueueIsEmpty(q, PORT_COMPLETION));
                taskqueue::XTaskQueueCloseHandle(q);
            }
        }

        // Also verify already-terminated ThreadPool queue does not invoke completion callback on XAsyncRun/begin_with.
        unsafe {
            let mut q = std::ptr::null_mut();
            assert_eq!(taskqueue::XTaskQueueCreate(taskqueue::MODE_THREAD_POOL, taskqueue::MODE_THREAD_POOL, &mut q), S_OK);
            assert_eq!(taskqueue::XTaskQueueTerminate(q, true, std::ptr::null_mut(), 0), S_OK);
            let before = CB_HITS.load(Ordering::SeqCst);
            let mut b1: XAsyncBlock = std::mem::zeroed();
            b1.queue = q;
            b1.callback = count_cb as usize;
            assert_eq!(XAsyncRun(&mut b1, run_work as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b1, true), E_ABORT);

            let mut b2: XAsyncBlock = std::mem::zeroed();
            b2.queue = q;
            b2.callback = count_cb as usize;
            assert_eq!(begin_with(&mut b2, 77, || Ok(b"never".to_vec())), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b2, true), E_ABORT);

            std::thread::sleep(Duration::from_millis(30));
            assert_eq!(CB_HITS.load(Ordering::SeqCst), before, "completion callback ran on already-terminated queue");
            taskqueue::XTaskQueueCloseHandle(q);
        }
    }

    #[test]
    fn complete_finishes_cleanup_and_header_before_publishing_done() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Barrier;
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct Ctx {
            cleanup_done: AtomicBool,
            cleanup_calls: AtomicU32,
        }
        unsafe extern "system" fn slow_cleanup_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
            let ctx = &*((*data).context as *const Ctx);
            if op == OP_CLEANUP {
                std::thread::yield_now();
                ctx.cleanup_calls.fetch_add(1, Ordering::SeqCst);
                ctx.cleanup_done.store(true, Ordering::SeqCst);
            }
            S_OK
        }

        // Part 1: zero-payload completion must finish OP_CLEANUP and write FLAG_DONE before waiters observe completion.
        for _ in 0..50 {
            let ctx = Arc::new(Ctx { cleanup_done: AtomicBool::new(false), cleanup_calls: AtomicU32::new(0) });
            let b = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<XAsyncBlock>() }));
            unsafe {
                assert_eq!(
                    XAsyncBegin(b, Arc::as_ptr(&ctx) as *mut c_void, std::ptr::null(), std::ptr::null(), slow_cleanup_provider as usize),
                    S_OK
                );
            }
            let barrier = Arc::new(Barrier::new(3));
            let b_addr = b as usize;
            let ctx_poll = ctx.clone();
            let bar_poll = barrier.clone();
            let poller = std::thread::spawn(move || {
                bar_poll.wait();
                loop {
                    let hr = unsafe { XAsyncGetStatus(b_addr as *mut XAsyncBlock, false) };
                    if hr != E_PENDING {
                        assert_eq!(hr, S_OK);
                        assert!(ctx_poll.cleanup_done.load(Ordering::SeqCst), "status published before OP_CLEANUP finished");
                        let hdr = unsafe { read_header(b_addr as *mut XAsyncBlock) }.expect("header missing");
                        assert_ne!(hdr.flags & FLAG_DONE, 0, "status published before FLAG_DONE written");
                        break;
                    }
                    std::thread::yield_now();
                }
            });
            let ctx_wait = ctx.clone();
            let bar_wait = barrier.clone();
            let waiter = std::thread::spawn(move || {
                bar_wait.wait();
                let hr = unsafe { XAsyncGetStatus(b_addr as *mut XAsyncBlock, true) };
                assert_eq!(hr, S_OK);
                assert!(ctx_wait.cleanup_done.load(Ordering::SeqCst), "waiter woke before OP_CLEANUP finished");
            });
            barrier.wait();
            unsafe { XAsyncComplete(b, S_OK, 0) };
            poller.join().unwrap();
            waiter.join().unwrap();
            assert_eq!(ctx.cleanup_calls.load(Ordering::SeqCst), 1);
            assert!(!active_map().lock().unwrap().contains_key(&b_addr));
            unsafe { drop(Box::from_raw(b)) };
        }

        // Part 2: non-empty payload completion must write FLAG_DONE before publishing `done`,
        // so an immediate `XAsyncGetResult` cannot have its `FLAG_CONSUMED` overwritten.
        unsafe extern "system" fn payload_4b_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
            if op == OP_GET_RESULT {
                std::ptr::copy_nonoverlapping(b"xyzw".as_ptr(), (*data).buffer as *mut u8, 4);
            }
            S_OK
        }
        for _ in 0..50 {
            let b = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<XAsyncBlock>() }));
            let id = 0xCAFEusize;
            unsafe {
                assert_eq!(
                    XAsyncBegin(b, std::ptr::null_mut(), id as *const c_void, std::ptr::null(), payload_4b_provider as usize),
                    S_OK
                );
            }
            let barrier = Arc::new(Barrier::new(2));
            let b_addr = b as usize;
            let bar_c = barrier.clone();
            let consumer = std::thread::spawn(move || {
                bar_c.wait();
                let mut buf = [0u8; 4];
                let mut used = 0usize;
                loop {
                    let hr = unsafe { XAsyncGetResult(b_addr as *mut XAsyncBlock, id as *const c_void, 4, buf.as_mut_ptr() as *mut c_void, &mut used) };
                    if hr != E_PENDING {
                        assert_eq!(hr, S_OK);
                        assert_eq!(&buf, b"xyzw");
                        assert_eq!(used, 4);
                        break;
                    }
                    std::thread::yield_now();
                }
            });
            barrier.wait();
            unsafe { XAsyncComplete(b, S_OK, 4) };
            consumer.join().unwrap();
            let mut buf = [0u8; 4];
            let hr2 = unsafe { XAsyncGetResult(b, id as *const c_void, 4, buf.as_mut_ptr() as *mut c_void, std::ptr::null_mut()) };
            assert_eq!(hr2, E_ILLEGAL_METHOD_CALL, "FLAG_CONSUMED was clobbered by XAsyncComplete");
            unsafe { drop(Box::from_raw(b)) };
        }
    }

    #[test]
    fn concurrent_get_result_runs_cleanup_exactly_once() {
        use std::sync::Barrier;
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct Counts {
            get_result: AtomicU32,
            cleanup: AtomicU32,
        }
        unsafe extern "system" fn counting_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
            let c = &*((*data).context as *const Counts);
            match op {
                OP_GET_RESULT => {
                    c.get_result.fetch_add(1, Ordering::SeqCst);
                    std::thread::yield_now();
                    std::ptr::copy_nonoverlapping(b"data".as_ptr(), (*data).buffer as *mut u8, 4);
                    S_OK
                }
                OP_CLEANUP => {
                    c.cleanup.fetch_add(1, Ordering::SeqCst);
                    S_OK
                }
                _ => S_OK,
            }
        }

        // 100 iterations of 4-thread race on a counting provider.
        const THREADS: usize = 4;
        for iter in 0..100 {
            let counts = Arc::new(Counts { get_result: AtomicU32::new(0), cleanup: AtomicU32::new(0) });
            let b = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<XAsyncBlock>() }));
            let id = 0x1000 + iter;
            unsafe {
                assert_eq!(
                    XAsyncBegin(b, Arc::as_ptr(&counts) as *mut c_void, id as *const c_void, std::ptr::null(), counting_provider as usize),
                    S_OK
                );
                XAsyncComplete(b, S_OK, 4);
            }
            let barrier = Arc::new(Barrier::new(THREADS));
            let b_addr = b as usize;
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    let bar = barrier.clone();
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 4];
                        let mut used = 0usize;
                        bar.wait();
                        let hr = unsafe {
                            XAsyncGetResult(b_addr as *mut XAsyncBlock, id as *const c_void, 4, buf.as_mut_ptr() as *mut c_void, &mut used)
                        };
                        (hr, buf, used)
                    })
                })
                .collect();
            let mut ok_count = 0usize;
            let mut illegal_count = 0usize;
            for h in handles {
                let (hr, buf, used) = h.join().unwrap();
                if hr == S_OK {
                    ok_count += 1;
                    assert_eq!(&buf, b"data");
                    assert_eq!(used, 4);
                } else {
                    assert_eq!(hr, E_ILLEGAL_METHOD_CALL, "unexpected hr {hr:#x} on iter {iter}");
                    illegal_count += 1;
                }
            }
            assert_eq!(ok_count, 1, "expected exactly 1 S_OK on iter {iter}");
            assert_eq!(illegal_count, THREADS - 1);
            assert_eq!(counts.get_result.load(Ordering::SeqCst), 1);
            assert_eq!(counts.cleanup.load(Ordering::SeqCst), 1, "OP_CLEANUP ran more than once on iter {iter}");
            assert!(!active_map().lock().unwrap().contains_key(&b_addr));
            unsafe { drop(Box::from_raw(b)) };
        }

        // 50 iterations with real `begin_with` (`Box<Payload>` freed in `OP_CLEANUP`).
        for iter in 0..50 {
            let b = Box::into_raw(Box::new(unsafe { std::mem::zeroed::<XAsyncBlock>() }));
            let id = 0x2000 + iter;
            unsafe {
                assert_eq!(begin_with(b, id, || Ok(b"payload!".to_vec())), S_OK);
                assert_eq!(XAsyncGetStatus(b, true), S_OK);
            }
            let barrier = Arc::new(Barrier::new(THREADS));
            let b_addr = b as usize;
            let handles: Vec<_> = (0..THREADS)
                .map(|_| {
                    let bar = barrier.clone();
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 8];
                        let mut used = 0usize;
                        bar.wait();
                        let hr = unsafe {
                            XAsyncGetResult(b_addr as *mut XAsyncBlock, id as *const c_void, 8, buf.as_mut_ptr() as *mut c_void, &mut used)
                        };
                        (hr, buf, used)
                    })
                })
                .collect();
            let mut ok_count = 0usize;
            for h in handles {
                let (hr, buf, used) = h.join().unwrap();
                if hr == S_OK {
                    ok_count += 1;
                    assert_eq!(&buf, b"payload!");
                    assert_eq!(used, 8);
                } else {
                    assert_eq!(hr, E_ILLEGAL_METHOD_CALL);
                }
            }
            assert_eq!(ok_count, 1);
            unsafe { drop(Box::from_raw(b)) };
        }
    }

    #[test]
    fn begin_with_fixups_drops_payload_on_begin_failure() {
        use std::sync::atomic::AtomicBool;
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct DropFlag(Arc<AtomicBool>);
        impl Drop for DropFlag {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(dropped.clone());
        let hr = unsafe {
            begin_with_fixups(std::ptr::null_mut(), 1, move || {
                let _keep = &flag;
                Ok(PayloadBuf { bytes: vec![1, 2, 3], fixups: Vec::new() })
            })
        };
        assert_eq!(hr, E_INVALIDARG);
        assert!(dropped.load(Ordering::SeqCst), "Payload was leaked when XAsyncBegin returned < 0");
    }

    #[test]
    fn complete_synchronously_inside_begin_zero_and_nonempty_payload() {
        let _g = crate::hle::xgameruntime::xasync::ASYNC_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        struct SyncCtx {
            cleanup_calls: AtomicU32,
            cb_calls: AtomicU32,
            cb_saw_hr: AtomicU32,
            cb_buf: Mutex<Vec<u8>>,
            payload_size: usize,
            return_err_after: bool,
        }

        unsafe extern "system" fn sync_begin_provider(op: u32, data: *const XAsyncProviderData) -> HRESULT {
            let ctx = &*((*data).context as *const SyncCtx);
            match op {
                OP_BEGIN => {
                    XAsyncComplete((*data).async_, S_OK, ctx.payload_size);
                    if ctx.return_err_after { crate::util::E_FAIL } else { S_OK }
                }
                OP_GET_RESULT => {
                    std::ptr::copy_nonoverlapping(b"sync".as_ptr(), (*data).buffer as *mut u8, 4);
                    S_OK
                }
                OP_CLEANUP => {
                    ctx.cleanup_calls.fetch_add(1, Ordering::SeqCst);
                    S_OK
                }
                _ => S_OK,
            }
        }

        unsafe extern "system" fn immediate_zero_cb(b: *mut XAsyncBlock) {
            let ctx = &*((*b).context as *const SyncCtx);
            let hr = XAsyncGetStatus(b, false);
            ctx.cb_saw_hr.store(hr as u32, Ordering::SeqCst);
            ctx.cb_calls.fetch_add(1, Ordering::SeqCst);
        }

        unsafe extern "system" fn immediate_payload_cb(b: *mut XAsyncBlock) {
            let ctx = &*((*b).context as *const SyncCtx);
            let mut size = 0usize;
            assert_eq!(XAsyncGetResultSize(b, &mut size), S_OK);
            assert_eq!(size, 4);
            let mut buf = vec![0u8; size];
            let mut used = 0usize;
            let hr = XAsyncGetResult(b, 0x42 as *const c_void, buf.len(), buf.as_mut_ptr() as *mut c_void, &mut used);
            assert_eq!(used, 4);
            *ctx.cb_buf.lock().unwrap() = buf;
            ctx.cb_saw_hr.store(hr as u32, Ordering::SeqCst);
            ctx.cb_calls.fetch_add(1, Ordering::SeqCst);
        }

        unsafe {
            let mut q = std::ptr::null_mut();
            assert_eq!(taskqueue::XTaskQueueCreate(taskqueue::MODE_IMMEDIATE, taskqueue::MODE_IMMEDIATE, &mut q), S_OK);

            // (a) Zero-size synchronous completion inside OP_BEGIN with immediate completion callback.
            let ctx0 = SyncCtx {
                cleanup_calls: AtomicU32::new(0),
                cb_calls: AtomicU32::new(0),
                cb_saw_hr: AtomicU32::new(u32::MAX),
                cb_buf: Mutex::new(Vec::new()),
                payload_size: 0,
                return_err_after: false,
            };
            let mut b0: XAsyncBlock = std::mem::zeroed();
            b0.queue = q;
            b0.context = &ctx0 as *const SyncCtx as *mut c_void;
            b0.callback = immediate_zero_cb as usize;
            assert_eq!(XAsyncBegin(&mut b0, &ctx0 as *const SyncCtx as *mut c_void, std::ptr::null(), std::ptr::null(), sync_begin_provider as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b0, false), S_OK);
            assert_eq!(ctx0.cleanup_calls.load(Ordering::SeqCst), 1);
            assert_eq!(ctx0.cb_calls.load(Ordering::SeqCst), 1);
            assert_eq!(ctx0.cb_saw_hr.load(Ordering::SeqCst) as i32, S_OK);
            assert!(!active_map().lock().unwrap().contains_key(&(&mut b0 as *mut XAsyncBlock as usize)));

            // (b) Non-empty payload completed inside OP_BEGIN and consumed inline by immediate completion callback.
            let ctx4 = SyncCtx {
                cleanup_calls: AtomicU32::new(0),
                cb_calls: AtomicU32::new(0),
                cb_saw_hr: AtomicU32::new(u32::MAX),
                cb_buf: Mutex::new(Vec::new()),
                payload_size: 4,
                return_err_after: false,
            };
            let mut b4: XAsyncBlock = std::mem::zeroed();
            b4.queue = q;
            b4.context = &ctx4 as *const SyncCtx as *mut c_void;
            b4.callback = immediate_payload_cb as usize;
            assert_eq!(XAsyncBegin(&mut b4, &ctx4 as *const SyncCtx as *mut c_void, 0x42 as *const c_void, std::ptr::null(), sync_begin_provider as usize), S_OK);
            assert_eq!(ctx4.cb_calls.load(Ordering::SeqCst), 1);
            assert_eq!(ctx4.cb_saw_hr.load(Ordering::SeqCst) as i32, S_OK);
            assert_eq!(*ctx4.cb_buf.lock().unwrap(), b"sync");
            assert_eq!(ctx4.cleanup_calls.load(Ordering::SeqCst), 1);
            assert_eq!(XAsyncGetResult(&mut b4, 0x42 as *const c_void, 4, [0u8; 4].as_mut_ptr() as *mut c_void, std::ptr::null_mut()), E_ILLEGAL_METHOD_CALL);
            assert!(!active_map().lock().unwrap().contains_key(&(&mut b4 as *mut XAsyncBlock as usize)));

            // (c) Completing inside OP_BEGIN and then returning an error HRESULT does not double-complete or double-cleanup.
            let ctx_err = SyncCtx {
                cleanup_calls: AtomicU32::new(0),
                cb_calls: AtomicU32::new(0),
                cb_saw_hr: AtomicU32::new(u32::MAX),
                cb_buf: Mutex::new(Vec::new()),
                payload_size: 0,
                return_err_after: true,
            };
            let mut b_err: XAsyncBlock = std::mem::zeroed();
            b_err.queue = q;
            b_err.context = &ctx_err as *const SyncCtx as *mut c_void;
            b_err.callback = immediate_zero_cb as usize;
            assert_eq!(XAsyncBegin(&mut b_err, &ctx_err as *const SyncCtx as *mut c_void, std::ptr::null(), std::ptr::null(), sync_begin_provider as usize), S_OK);
            assert_eq!(XAsyncGetStatus(&mut b_err, false), S_OK);
            assert_eq!(ctx_err.cleanup_calls.load(Ordering::SeqCst), 1);
            assert_eq!(ctx_err.cb_calls.load(Ordering::SeqCst), 1);

            taskqueue::XTaskQueueTerminate(q, true, std::ptr::null_mut(), 0);
            taskqueue::XTaskQueueCloseHandle(q);
        }
    }
}
