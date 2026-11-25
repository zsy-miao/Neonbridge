//! Command allocators and command lists.
//!
//! The title's inlined code writes packets into a list only while its put
//! pointer is below a limit, and calls the list's method otherwise. Our
//! lists keep the pointer and every limit at zero, so every command reaches
//! the table as a call.
//!
//! Each list records onto a host command list as its methods are called
//! (decision 0007); draw state lives in `crate::draw`. Methods not
//! written yet log once with their caller, so a run lists what to do next.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

#[cfg(windows)]
use std::sync::Arc;
use std::mem::offset_of;

use crate::object::{
    self, ALLOCATOR, E_INVALIDARG, HRESULT, Iid, LIST, Object, S_OK, Table, This, guid,
};
use crate::queue::Written;
use crate::resource::PAGEABLE_IIDS;

const ALLOCATOR_IIDS: [Iid; 4] = [
    guid(
        0x6102dee4,
        0xaf59,
        0x4b09,
        [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

/// The list interfaces up to `ID3D12GraphicsCommandList6` (the table has
/// all their slots): those whose ids are the same as on PC, the console's
/// own List5 and List6 (Grand Theft Auto VI asks for List6), then the
/// device child's.
const LIST_IIDS: [Iid; 10] = [
    guid(
        0x7116d91c,
        0xe7e4,
        0x47ce,
        [0xb8, 0xc6, 0xec, 0x81, 0x68, 0xf4, 0x37, 0xe5],
    ),
    guid(
        0x5b160d0f,
        0xac1b,
        0x4185,
        [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55],
    ),
    guid(
        0x553103fb,
        0x1fe7,
        0x4557,
        [0xbb, 0x38, 0x94, 0x6d, 0x7d, 0x0e, 0x7c, 0xa7],
    ),
    guid(
        0x38c3e585,
        0xff17,
        0x412c,
        [0x91, 0x50, 0x4f, 0xc6, 0xf9, 0xd7, 0x2a, 0x28],
    ),
    guid(
        0x6fda83a7,
        0xb84c,
        0x4e38,
        [0x9a, 0xc8, 0xc7, 0xbd, 0x22, 0x01, 0x6b, 0x3d],
    ),
    guid(
        0x8754318e,
        0xd3a9,
        0x4541,
        [0x98, 0xcf, 0x64, 0x5b, 0x50, 0xdc, 0x48, 0x74],
    ),
    guid(
        0x55050859,
        0x4024,
        0x474c,
        [0x98, 0xcf, 0x64, 0x5b, 0x51, 0xdc, 0x48, 0x74],
    ),
    guid(
        0xc3827890,
        0x4024,
        0x474c,
        [0x98, 0xcf, 0x64, 0x5b, 0x51, 0xdc, 0x48, 0x74],
    ),
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
struct Allocator {
    object: Object,
    kind: u32,
}

/// The layout the title's inlined code reads.
#[repr(C)]
struct List {
    object: Object,
    kind: u32,
    /// The put pointer, then the plain, draw, API and unbiased API limits:
    /// all zero, so every inline check fails.
    put: [u64; 5],
    reserved: [u32; 64],
    /// State the inline code keeps for building packets; unused by us.
    graphics_root_headers: [u32; 128],
    compute_root_headers: [u32; 128],
    pipeline_shadow: [u32; 8],
    /// Newer titles' lists are larger: Grand Theft Auto VI's state flush
    /// (title+0x32786) copies a pipeline's 32 bytes to +0x544..+0x564 and
    /// tests +0x548, so its layout runs past 0x560. Its size is unknown;
    /// this keeps our fields clear of whatever the title writes there.
    newer: [u32; 256],
    /// Ours, after the console layout: memory handed out by
    /// [`allocate_upload`], kept until the list is reset; the host list the
    /// calls record into (decision 0007), and the draw state the title set.
    uploads: std::sync::Mutex<Vec<Vec<u8>>>,
    /// Replaced at a reset while a submission of it is still to run; titles
    /// use a list on one thread at a time.
    #[cfg(windows)]
    host: std::cell::UnsafeCell<Option<Arc<crate::host::HostList>>>,
    /// The recording before its last wait that followed work, cut there so
    /// that work runs first.
    #[cfg(windows)]
    parts: std::cell::UnsafeCell<Vec<Arc<crate::host::HostList>>>,
    #[cfg(windows)]
    state: std::sync::Mutex<crate::draw::State>,
}
const _: () = assert!(offset_of!(List, put) == 0x18);
const _: () = assert!(offset_of!(List, pipeline_shadow) + 32 == 0x560);

/// Runs `f` with the host list and draw state of a title list, if there
/// is a host GPU.
#[cfg(windows)]
fn with_state(this: This, f: impl FnOnce(&crate::host::HostList, &mut crate::draw::State)) {
    let Some(list) = host_list(this) else { return };
    // SAFETY: the title passes one of our lists.
    let state = unsafe { &(*this.cast::<List>()).state };
    f(list, &mut state.lock().unwrap_or_else(|e| e.into_inner()));
}

/// The host list behind a title list, if there is a host GPU.
#[cfg(windows)]
pub(crate) fn host_list<'a>(this: This) -> Option<&'a crate::host::HostList> {
    if this.is_null() {
        return None;
    }
    // SAFETY: the title passes one of our lists.
    unsafe { (*(*this.cast::<List>()).host.get()).as_deref() }
}

/// A title list's recording, submitted.
#[cfg(windows)]
pub(crate) struct Submitted(Arc<crate::host::HostList>);

// SAFETY: host lists are free-threaded objects, recorded by one thread and,
// once closed, run by one.
#[cfg(windows)]
unsafe impl Send for Submitted {}

#[cfg(windows)]
impl Submitted {
    /// The host list itself, to keep past the submission's return.
    pub(crate) fn shared(&self) -> Arc<crate::host::HostList> {
        self.0.clone()
    }
}

#[cfg(windows)]
impl std::ops::Deref for Submitted {
    type Target = crate::host::HostList;
    fn deref(&self) -> &crate::host::HostList {
        &self.0
    }
}

/// What a submission of a title list runs, however the title goes on to use
/// the list.
#[cfg(windows)]
pub(crate) fn submitted(this: This) -> Vec<Submitted> {
    if this.is_null() {
        return Vec::new();
    }
    // SAFETY: the title passes one of our lists, closed.
    let (parts, host) = unsafe {
        let list = &*this.cast::<List>();
        (&*list.parts.get(), &*list.host.get())
    };
    parts.iter().chain(host).cloned().map(Submitted).collect()
}

/// Cuts a list's recording before a wait, if work precedes it: the rest goes
/// into another host list, with the state the title set.
#[cfg(windows)]
fn cut(this: This) {
    if !host_list(this).is_some_and(|l| l.worked()) {
        return;
    }
    let Some(next) = spare() else { return };
    // SAFETY: one of our lists; the title uses it on one thread at a time,
    // and no reference to the host list is in use here.
    let (host, parts, state) = unsafe {
        let list = &*this.cast::<List>();
        (&mut *list.host.get(), &mut *list.parts.get(), &list.state)
    };
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    state.end_hoist();
    if let Some(old) = host.replace(next) {
        old.close();
        parts.push(old);
    }
    if let Some(next) = host {
        state.continue_in(next);
    }
}

/// Before an indirect draw (`crate::indirect::hoisting`): unless the clamps
/// of the list's draws already go to a list before it and nothing since may
/// have written arguments, the recording is cut here; a list for the clamps
/// of the draws to come follows what was recorded, and the recording goes
/// on in another list after it, with the state the title set.
#[cfg(windows)]
fn hoist(this: This) {
    if !crate::indirect::hoisting() {
        return;
    }
    // SAFETY: one of our lists; the title uses it on one thread at a time,
    // and no reference to the host list is in use here.
    let (host, parts, state) = unsafe {
        let list = &*this.cast::<List>();
        (&mut *list.host.get(), &mut *list.parts.get(), &list.state)
    };
    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
    let Some(current) = host.as_ref() else { return };
    if state.hoists_into(current) {
        return;
    }
    state.end_hoist();
    // A query that runs ends with the list it began in: its draws stay in
    // one list, their clamps before each (modes 5 and 6 cut all the same,
    // to find faults).
    if current.open_queries() > 0 {
        crate::indirect::CUTS[1].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if crate::indirect::hoist_mode() < 5 {
            return;
        }
    }
    crate::indirect::CUTS[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let Some(next) = spare() else { return };
    // What was recorded keeps its place; with no work in it, it holds the
    // clamps itself (the state set in it does them no harm).
    let clamps = if current.worked() || crate::indirect::hoist_mode() == 5 {
        let Some(clamps) = spare() else { return };
        if let Some(old) = host.replace(next) {
            old.close();
            parts.push(old);
        }
        clamps
    } else {
        let Some(old) = host.replace(next) else { return };
        old
    };
    parts.push(clamps.clone());
    if let Some(next) = host {
        state.continue_in(next);
        state.hoist(clamps, next);
    }
}

/// Host lists the title's lists were parted from while a submission of them
/// was still to run, reused once it has.
#[cfg(windows)]
static PARTED: std::sync::Mutex<Vec<Submitted>> = std::sync::Mutex::new(Vec::new());

/// A host list to record into, open: one that has run, or a new one.
#[cfg(windows)]
fn spare() -> Option<Arc<crate::host::HostList>> {
    let mut parted = PARTED.lock().unwrap_or_else(|e| e.into_inner());
    let spare = parted
        .iter()
        .position(|l| Arc::strong_count(&l.0) == 1)
        .map(|i| parted.swap_remove(i).0);
    drop(parted);
    if let Some(list) = spare {
        list.release_kept();
        list.reset();
        return Some(list);
    }
    let made = crate::host::host()?.new_list();
    match made {
        #[allow(clippy::arc_with_non_send_sync)] // see `Submitted`
        Ok(list) => Some(Arc::new(list)),
        Err(error) => {
            tracing::warn!(%error, "no host command list");
            None
        }
    }
}

#[cfg(windows)]
impl Drop for List {
    fn drop(&mut self) {
        // What a submission still to run needs of the list.
        if let Some(host) = self.host.get_mut()
            && Arc::strong_count(host) > 1
        {
            host.keep_until_run(Box::new((
                std::mem::take(&mut self.uploads),
                std::mem::take(&mut self.state),
            )));
        }
    }
}

pub(crate) extern "system" fn create_allocator(
    _: This,
    kind: u32,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    let allocator = Box::new(Allocator {
        object: Object::new(&ALLOCATOR_FUNCTIONS),
        kind,
    });
    object::hand_out(ALLOCATOR, allocator, iid, &ALLOCATOR_IIDS, out)
}

/// Lists start open, as in D3D12.
pub(crate) extern "system" fn create_list(
    _: This,
    _nodes: u32,
    kind: u32,
    allocator: This,
    _pipeline: This,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    new_list(kind, allocator, iid, out)
}

/// The console's list creation: type, flags, node mask and barrier batch
/// size in a description.
#[repr(C)]
#[derive(Debug)]
pub(crate) struct ListDesc {
    kind: u32,
    flags: u32,
    nodes: u32,
    barrier_batch: u32,
}

pub(crate) extern "system" fn create_list_x(
    _: This,
    desc: *const ListDesc,
    allocator: This,
    _pipeline: This,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    tracing::debug!(?desc, "console command list");
    new_list(desc.kind, allocator, iid, out)
}

fn new_list(kind: u32, allocator: This, iid: *const Iid, out: *mut This) -> HRESULT {
    if allocator.is_null() {
        return E_INVALIDARG;
    }
    tracing::debug!(kind, "command list");
    let list = Box::new(List {
        object: if crate::device::newer() {
            Object::new(&NEWER_LIST_FUNCTIONS)
        } else {
            Object::new(&LIST_FUNCTIONS)
        },
        kind,
        put: [0; 5],
        reserved: [0; 64],
        graphics_root_headers: [0; 128],
        compute_root_headers: [0; 128],
        pipeline_shadow: [0; 8],
        newer: [0; 256],
        uploads: Default::default(),
        #[cfg(windows)]
        #[allow(clippy::arc_with_non_send_sync)] // see `Submitted`
        host: crate::host::host()
            .and_then(|h| match h.new_list() {
                Ok(list) => Some(Arc::new(list)),
                Err(e) => {
                    tracing::warn!(error = %e, "no host command list");
                    None
                }
            })
            .into(),
        #[cfg(windows)]
        parts: Default::default(),
        #[cfg(windows)]
        state: Default::default(),
    });
    object::hand_out(LIST, list, iid, &LIST_IIDS, out)
}

extern "system" fn allocator_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    let found = object::is(iid, &ALLOCATOR_IIDS).then_some(this);
    object::answer(ALLOCATOR, iid, found, out)
}
extern "system" fn allocator_release(this: This) -> u32 {
    object::release_boxed::<Allocator>(this)
}
/// Nothing is recorded, so there is nothing to free.
extern "system" fn allocator_reset(_: This) -> HRESULT {
    S_OK
}

extern "system" fn list_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(LIST, iid, object::is(iid, &LIST_IIDS).then_some(this), out)
}
extern "system" fn list_release(this: This) -> u32 {
    object::release_boxed::<List>(this)
}
extern "system" fn list_type(this: This) -> u32 {
    // SAFETY: `this` is one of our lists.
    unsafe { (*this.cast::<List>()).kind }
}
extern "system" fn close(this: This) -> HRESULT {
    #[cfg(windows)]
    if let Some(host) = host_list(this) {
        // SAFETY: the title passes one of our lists.
        let state = unsafe { &(*this.cast::<List>()).state };
        state.lock().unwrap_or_else(|e| e.into_inner()).end_hoist();
        host.close();
    }
    S_OK
}
extern "system" fn reset(this: This, allocator: This, pipeline: This) -> HRESULT {
    if allocator.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the title passes one of our lists.
    let uploads = unsafe { &(*this.cast::<List>()).uploads };
    let mut uploads = uploads.lock().unwrap_or_else(|e| e.into_inner());
    #[cfg(windows)]
    {
        // SAFETY: as above; the title uses a list on one thread at a time.
        let (host, state) = unsafe {
            let list = &*this.cast::<List>();
            (&mut *list.host.get(), &list.state)
        };
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        state.end_hoist();
        // SAFETY: as above.
        let parts = unsafe { &mut *(*this.cast::<List>()).parts.get() };
        {
            let mut parted = PARTED.lock().unwrap_or_else(|e| e.into_inner());
            parted.extend(parts.drain(..).map(Submitted));
        }
        if let Some(old) = host.take_if(|h| Arc::strong_count(h) > 1) {
            // A submission of it is still to run, with the memory and the
            // descriptors it was recorded with.
            old.keep_until_run(Box::new((std::mem::take(&mut *uploads), std::mem::take(&mut *state))));
            PARTED.lock().unwrap_or_else(|e| e.into_inner()).push(Submitted(old));
            *host = spare();
            if host.is_none() {
                return crate::object::E_OUTOFMEMORY;
            }
        } else if let Some(host) = host {
            host.reset();
        }
        state.reset(pipeline);
    }
    uploads.clear();
    drop(uploads);
    #[cfg(not(windows))]
    let _ = (this, pipeline);
    S_OK
}

/// A resource barrier as titles pass it: type, flags, then a resource and
/// either a subresource and two states (transition) or a second resource
/// (aliasing). The same layout as the host's.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Barrier {
    kind: u32,
    flags: u32,
    first: This,
    second: u64,
    last: u64,
}

/// The barrier on host resources, or `None` if it names a resource the host
/// GPU does not have (buffers so far).
#[cfg(windows)]
fn host_barrier(list: &crate::host::HostList, b: &Barrier) -> Option<Barrier> {
    use windows::core::Interface;
    let host = |r: This| crate::resource::host_resource(r).map(|h| h.as_raw().cast_const().cast());
    match b.kind {
        0 => {
            // A split barrier's first half: its second half transitions.
            const BEGIN_ONLY: u32 = 1;
            if b.flags & BEGIN_ONLY != 0 {
                return None;
            }
            // shortcut: whole resources, from the state the list tracked
            // (`HostList::transition`); subresources are not told apart.
            let wanted = crate::host::texture_state(b.last as u32);
            let (texture, before) = list.transition(b.first, wanted)?;
            Some(Barrier {
                kind: 0,
                flags: 0,
                first: texture.as_raw().cast_const().cast(),
                second: 0xffff_ffff | (u64::from(before) << 32),
                last: u64::from(crate::host::legal_state(texture, wanted)),
            })
        }
        1 => Some(Barrier {
            first: if b.first.is_null() {
                b.first
            } else {
                host(b.first)?
            },
            second: if b.second == 0 {
                0
            } else {
                host(b.second as This)? as u64
            },
            ..*b
        }),
        2 => Some(Barrier {
            first: if b.first.is_null() {
                b.first
            } else {
                host(b.first)?
            },
            ..*b
        }),
        _ => None,
    }
}

/// An unordered access barrier on every resource: earlier shader writes
/// finish before later work reads or writes.
const UAV_BARRIER: Barrier = Barrier {
    kind: 2,
    flags: 0,
    first: std::ptr::null(),
    second: 0,
    last: 0,
};

/// Returns the console's barrier "actions" (none).
extern "system" fn barrier(this: This, count: u32, barriers: *const Barrier) -> u32 {
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && !barriers.is_null()
    {
        // SAFETY: the title passes `count` barriers.
        let barriers = unsafe { std::slice::from_raw_parts(barriers, count as usize) };
        let mut host: Vec<Barrier> = barriers
            .iter()
            .filter_map(|b| host_barrier(&list, b))
            .collect();
        // Buffers have host copies, not resources the barrier can name:
        // a barrier on one waits for every shader write before it.
        if barriers.iter().any(|b| {
            b.kind != 1 && !b.first.is_null() && crate::resource::buffer_bytes(b.first).is_some()
        }) {
            host.push(UAV_BARRIER);
        }
        if !host.is_empty() {
            use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_BARRIER;
            const _: () = assert!(size_of::<Barrier>() == size_of::<D3D12_RESOURCE_BARRIER>());
            // SAFETY: the same layout; borrowed resource pointers, never
            // released through this slice.
            let host = unsafe {
                std::slice::from_raw_parts(
                    host.as_ptr().cast::<D3D12_RESOURCE_BARRIER>(),
                    host.len(),
                )
            };
            // SAFETY: an open list and live resources.
            unsafe { list.list.ResourceBarrier(host) };
        }
    }
    0
}

/// Moves a title texture's host copy to `state` in the list, if needed.
#[cfg(windows)]
pub(crate) fn ensure_state(list: &crate::host::HostList, resource: This, state: u32) {
    use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES as State;
    if !resource.is_null() {
        crate::resource::note_written(resource, state);
    }
    if let Some((texture, before)) = list.transition(resource, state) {
        let state = crate::host::legal_state(texture, state);
        let barrier = crate::host::transition(texture, State(before as i32), State(state as i32));
        // SAFETY: an open list and a live texture.
        unsafe { list.list.ResourceBarrier(&[barrier]) };
    }
}

#[cfg(windows)]
const COPY_DEST: u32 = 0x400;
#[cfg(windows)]
const COPY_SOURCE: u32 = 0x800;

/// `len` bytes at `offset` of a title buffer, if they are inside it.
#[cfg(windows)]
fn buffer_range(buffer: This, offset: u64, len: u64) -> Option<*mut u8> {
    let (data, size) = crate::resource::buffer_bytes(buffer)?;
    let end = offset.checked_add(len)?;
    if end > size {
        tracing::warn!(offset, len, size, "copy outside its buffer");
        return None;
    }
    // SAFETY: inside the buffer, checked above.
    Some(unsafe { data.add(offset as usize) })
}

/// Buffer to buffer: both are host memory, copied at submission.
extern "system" fn copy_buffer_region(
    this: This,
    dst: This,
    dst_offset: u64,
    src: This,
    src_offset: u64,
    len: u64,
) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_write();
        list.count_work();
    }
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && let Some(to) = buffer_range(dst, dst_offset, len)
        && let Some(from) = buffer_range(src, src_offset, len)
    {
        list.defer_copy(to, from, len as usize);
    }
    #[cfg(not(windows))]
    let _ = (this, dst, dst_offset, src, src_offset, len);
}

extern "system" fn copy_resource(this: This, dst: This, src: This) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_write();
        list.count_work();
    }
    #[cfg(windows)]
    {
        let Some(list) = host_list(this) else {
            return;
        };
        if let (Some(to), Some(from)) = (
            crate::resource::host_resource(dst),
            crate::resource::host_resource(src),
        ) {
            // shortcut: not between textures without their largest mips.
            if crate::resource::skipped_mips(dst) + crate::resource::skipped_mips(src) > 0 {
                return;
            }
            if crate::texture::title_written(from) {
                crate::texture::upload(&list, src, from);
            }
            ensure_state(&list, dst, COPY_DEST);
            ensure_state(&list, src, COPY_SOURCE);
            crate::texture::host_written(dst);
            #[cfg(feature = "diagnostics")]
            list.note_op(|| format!("copy resource {src:?} to {dst:?}"));
            // SAFETY: an open list and live textures.
            unsafe { list.list.CopyResource(to, from) };
        } else if let (Some((to, a)), Some((from, b))) = (
            crate::resource::buffer_bytes(dst),
            crate::resource::buffer_bytes(src),
        ) {
            list.defer_copy(to, from, a.min(b) as usize);
        }
    }
    #[cfg(not(windows))]
    let _ = (this, dst, src);
}

extern "system" fn copy_resource_x(this: This, dst: This, src: This, _flags: u32) {
    copy_resource(this, dst, src);
}

/// A copy location as titles pass it, laid out as the host's: a resource,
/// then a subresource index (type 0) or a placed footprint (type 1).
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct CopyLocation {
    resource: This,
    kind: u32,
    footprint: crate::resource::PlacedFootprint,
}

/// Size of the source box, or of the selected mip/array/plane subresource
/// when the caller asks D3D12 to copy the whole subresource.
#[cfg(windows)]
fn readback_extent(
    host: &crate::host::Host,
    texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource,
    subresource: u32,
    source_box: Option<&[u32; 6]>,
) -> Option<[u32; 3]> {
    if let Some(&[left, top, front, right, bottom, back]) = source_box {
        return Some([
            right.checked_sub(left)?,
            bottom.checked_sub(top)?,
            back.checked_sub(front)?,
        ]);
    }
    use windows::Win32::Graphics::Direct3D12::D3D12_PLACED_SUBRESOURCE_FOOTPRINT;
    let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
    let mut total = 0;
    // SAFETY: the texture is live and both outputs hold one subresource's result.
    unsafe {
        host.device.GetCopyableFootprints(
            &texture.GetDesc(),
            subresource,
            1,
            0,
            Some(&mut layout),
            None,
            None,
            Some(&mut total),
        );
    }
    let fp = layout.Footprint;
    (total != u64::MAX).then_some([fp.Width, fp.Height, fp.Depth])
}

/// Texture to texture on the GPU, buffer to texture through an upload
/// buffer filled at submission, and texture to buffer through a readback
/// buffer copied into title memory once the submission completes.
extern "system" fn copy_texture_region(
    this: This,
    dst: *const CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const CopyLocation,
    source_box: *const [u32; 6],
) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_work();
    }
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Direct3D12::{D3D12_BOX, D3D12_TEXTURE_COPY_LOCATION};
        use windows::core::Interface;
        const FOOTPRINT: u32 = 1;
        // SAFETY: the title passes both locations (null is tolerated).
        let (Some(list), Some(&dst), Some(&src)) =
            (host_list(this), unsafe { dst.as_ref() }, unsafe {
                src.as_ref()
            })
        else {
            return;
        };
        const _: () =
            assert!(size_of::<CopyLocation>() == size_of::<D3D12_TEXTURE_COPY_LOCATION>());
        let host_box = (!source_box.is_null()).then(|| source_box.cast::<D3D12_BOX>());
        if dst.kind == FOOTPRINT {
            if src.kind == FOOTPRINT {
                return;
            }
            let (Some(host), Some(from_tex)) = (
                crate::host::host(),
                crate::resource::host_resource(src.resource),
            ) else {
                return;
            };
            let Some((lines, row_bytes, total)) = dst.footprint.layout() else {
                return;
            };
            let host_format = crate::resource::pc_format(dst.footprint.format);
            if crate::resource::element(host_format)
                != crate::resource::element(dst.footprint.format)
            {
                // Console formats such as 118..=120 (3-byte D16S8) map to
                // 4-byte PC substitutes (45..=47): raw readback would mis-size
                // the staging buffer and write the wrong texel layout.
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    tracing::warn!(
                        format = dst.footprint.format,
                        host_format,
                        "texture-to-buffer readback skipped: host format has a different texel size",
                    );
                });
                return;
            }
            if total == 0 || lines == 0 || row_bytes == 0 {
                return;
            }
            // For a subresource location the first DWORD of the footprint union is its index.
            let Some([width, height, depth]) = readback_extent(
                host,
                from_tex,
                src.footprint.offset as u32,
                // SAFETY: the optional source box is read during this call.
                unsafe { source_box.as_ref() },
            ) else {
                return;
            };
            let footprint = dst.footprint;
            let Some((element, block)) = crate::resource::element(footprint.format) else {
                return;
            };
            // A BC mip's reported extent can include its final partial block.
            // Measure the footprint and the copied area in the same storage units.
            let rows_per_slice = u64::from(footprint.height).div_ceil(block);
            let row_in_slice = u64::from(y) / block;
            let first_row = u64::from(z) * rows_per_slice + row_in_slice;
            let column_bytes = (u64::from(x) / block) * element;
            let copy_rows = u64::from(height).div_ceil(block);
            let copy_bytes = u64::from(width).div_ceil(block) * element;
            if copy_bytes == 0
                || copy_rows == 0
                || depth == 0
                || column_bytes + copy_bytes > row_bytes
                || row_in_slice + copy_rows > rows_per_slice
                || u64::from(z) + u64::from(depth) > u64::from(footprint.depth)
            {
                return;
            }
            let dst_pitch = u64::from(dst.footprint.row_pitch);
            if lines > 1 && dst_pitch < row_bytes {
                return;
            }
            let Some(target) = buffer_range(dst.resource, dst.footprint.offset, total) else {
                return;
            };
            let Ok(row_bytes_u32) = u32::try_from(row_bytes) else {
                return;
            };
            // D3D12 requires placed footprints' row pitch to be 256-byte aligned.
            let Some(host_pitch) = dst
                .footprint
                .row_pitch
                .max(row_bytes_u32)
                .checked_next_multiple_of(256)
            else {
                return;
            };
            let Some(staging_bytes) = u64::from(host_pitch)
                .checked_mul(lines - 1)
                .and_then(|n| n.checked_add(row_bytes))
            else {
                return;
            };
            let Some((buffer, staging)) = list.readback(host, staging_bytes) else {
                return;
            };
            if crate::resource::skipped_mips(src.resource) > 0 {
                return;
            }
            crate::texture::upload(&list, src.resource, from_tex);
            ensure_state(&list, src.resource, COPY_SOURCE);
            let mut to_location = dst;
            to_location.resource = buffer.as_raw().cast_const().cast();
            to_location.footprint.offset = 0;
            to_location.footprint.format = host_format;
            to_location.footprint.row_pitch = host_pitch;
            let mut from_location = src;
            from_location.resource = from_tex.as_raw().cast_const().cast();
            #[cfg(feature = "diagnostics")]
            list.note_op(|| {
                // SAFETY: a live texture; the box is the title's or null.
                let (d, b) = unsafe { (from_tex.GetDesc(), host_box.map(|b| *b)) };
                format!(
                    "readback to {:?} footprint {:?} at {x},{y},{z} from {:?} {}x{} f{} sub {} box {b:?}",
                    dst.resource,
                    dst.footprint,
                    src.resource,
                    d.Width,
                    d.Height,
                    d.Format.0,
                    src.kind,
                )
            });
            // SAFETY: an open list; both locations in the host's layout, naming
            // live resources; the box is the title's (same layout).
            unsafe {
                list.list.CopyTextureRegion(
                    (&raw const to_location).cast(),
                    x,
                    y,
                    z,
                    (&raw const from_location).cast(),
                    host_box,
                )
            };
            // The GPU only populated the copied rectangle in staging. Preserve
            // the rest of each destination row and the gaps between slices.
            for slice in 0..u64::from(depth) {
                let row = first_row + slice * rows_per_slice;
                let dst_offset = (row * dst_pitch + column_bytes) as usize;
                let src_offset = (row * u64::from(host_pitch) + column_bytes) as usize;
                list.defer_readback(
                    target.wrapping_add(dst_offset),
                    staging.wrapping_add(src_offset),
                    dst_pitch as usize,
                    host_pitch as usize,
                    copy_rows as usize,
                    copy_bytes as usize,
                );
            }
            return;
        }
        let Some(to) = crate::resource::host_resource(dst.resource) else {
            return;
        };
        let mut from = src;
        let _upload; // keeps the source alive while this call records it
        #[cfg(feature = "diagnostics")]
        let mut read = None;
        if src.kind == FOOTPRINT {
            let Some(bytes) = src.footprint.bytes() else {
                return;
            };
            let Some(data) = buffer_range(src.resource, src.footprint.offset, bytes) else {
                return;
            };
            let Some((buffer, at, staging)) = list.upload(bytes) else {
                return;
            };
            // The list owns this mapped D3D12 upload memory. Wine can report
            // its address as MEM_FREE, so use the host-upload path.
            list.defer_fill(staging, data, bytes as usize);
            from.resource = buffer.as_raw().cast_const().cast();
            from.footprint.offset = at;
            #[cfg(feature = "diagnostics")]
            {
                // SAFETY: a live buffer.
                read = Some((unsafe { buffer.GetGPUVirtualAddress() } + at, bytes));
            }
            _upload = buffer;
        } else {
            let Some(texture) = crate::resource::host_resource(src.resource) else {
                return;
            };
            if crate::resource::skipped_mips(src.resource) > 0 {
                return;
            }
            if crate::texture::title_written(texture) {
                crate::texture::upload(&list, src.resource, texture);
            }
            ensure_state(&list, src.resource, COPY_SOURCE);
            from.resource = texture.as_raw().cast_const().cast();
        }
        // shortcut: as in `copy_resource`.
        if crate::resource::skipped_mips(dst.resource) > 0 {
            return;
        }
        ensure_state(&list, dst.resource, COPY_DEST);
        crate::texture::host_written(dst.resource);
        let mut to_location = dst;
        to_location.resource = to.as_raw().cast_const().cast();
        #[cfg(feature = "diagnostics")]
        {
            let text = || {
                // SAFETY: a live texture; the box is the title's or null.
                let (d, b) = unsafe { (to.GetDesc(), host_box.map(|b| *b)) };
                format!(
                    "copy to {:?} {}x{} f{} sub/kind {} at {x},{y},{z} from {:?} kind {} footprint {:?} box {b:?}",
                    dst.resource,
                    d.Width,
                    d.Height,
                    d.Format.0,
                    dst.kind,
                    src.resource,
                    src.kind,
                    (src.kind == FOOTPRINT).then_some(src.footprint)
                )
            };
            match read {
                Some(range) => list.note_read(text, range),
                None => list.note_op(text),
            }
        }
        // SAFETY: an open list; both locations in the host's layout, naming
        // live resources; the box is the title's (same layout).
        unsafe {
            list.list.CopyTextureRegion(
                (&raw const to_location).cast(),
                x,
                y,
                z,
                (&raw const from).cast(),
                host_box,
            )
        };
    }
    #[cfg(not(windows))]
    let _ = (this, dst, x, y, z, src, source_box);
}

#[allow(clippy::too_many_arguments)] // the method's own arguments
extern "system" fn copy_texture_region_x(
    this: This,
    dst: *const CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const CopyLocation,
    source_box: *const [u32; 6],
    _flags: u32,
) {
    copy_texture_region(this, dst, x, y, z, src, source_box);
}

extern "system" fn resolve(
    this: This,
    dst: This,
    dst_sub: u32,
    src: This,
    src_sub: u32,
    format: u32,
) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_work();
    }
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && let (Some(to), Some(from)) = (
            crate::resource::host_resource(dst),
            crate::resource::host_resource(src),
        )
    {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Direct3D12::{
            D3D12_RESOLVE_MODE_AVERAGE, ID3D12GraphicsCommandList1,
        };
        use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
        use windows::core::Interface;
        const RESOLVE_DEST: u32 = 0x1000;
        const RESOLVE_SOURCE: u32 = 0x2000;
        ensure_state(&list, dst, RESOLVE_DEST);
        ensure_state(&list, src, RESOLVE_SOURCE);
        crate::texture::host_written(dst);
        #[cfg(feature = "diagnostics")]
        list.note_op(|| format!("resolve {src:?} to {dst:?} format {format}"));
        // SAFETY: an open list and live textures; the region is inside both.
        unsafe {
            let (a, b) = (to.GetDesc(), from.GetDesc());
            let format = DXGI_FORMAT(format as i32);
            if (a.Width, a.Height) == (b.Width, b.Height) {
                list.list
                    .ResolveSubresource(to, dst_sub, from, src_sub, format);
            } else if let Ok(list1) = list.list.cast::<ID3D12GraphicsCommandList1>() {
                // The console clips a larger source to the destination.
                let rect = RECT {
                    left: 0,
                    top: 0,
                    right: a.Width.min(b.Width) as i32,
                    bottom: a.Height.min(b.Height) as i32,
                };
                list1.ResolveSubresourceRegion(
                    to,
                    dst_sub,
                    0,
                    0,
                    from,
                    src_sub,
                    Some(&rect),
                    format,
                    D3D12_RESOLVE_MODE_AVERAGE,
                );
            }
        }
    }
    #[cfg(not(windows))]
    let _ = (this, dst, dst_sub, src, src_sub, format);
}

#[repr(C)]
pub(crate) struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

extern "system" fn clear_rtv(
    this: This,
    handle: u64,
    color: *const [f32; 4],
    count: u32,
    rects: *const Rect,
) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_work();
    }
    #[cfg(windows)]
    {
        let (Some(list), Some(record)) = (host_list(this), crate::descriptor::rtv_record(handle))
        else {
            return;
        };
        let (Some(texture), Some(host)) = (
            crate::resource::host_resource(record.resource),
            crate::host::host(),
        ) else {
            return;
        };
        // SAFETY: the title passes four floats (null is tolerated).
        let Some(color) = (unsafe { color.as_ref() }) else {
            return;
        };
        let rects = (count > 0 && !rects.is_null()).then(|| {
            // SAFETY: the title passes `count` rectangles, laid out as the host's.
            unsafe { std::slice::from_raw_parts(rects.cast(), count as usize) }
        });
        const RENDER_TARGET: u32 = 0x4;
        let rtv = host.rtv(texture, record.desc().as_ref());
        // SAFETY: an open list, a fresh view of a live texture.
        unsafe {
            if let Some((_, before)) = list.transition(record.resource, RENDER_TARGET) {
                use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_STATES as State;
                list.list.ResourceBarrier(&[crate::host::transition(
                    texture,
                    State(before as i32),
                    State(RENDER_TARGET as i32),
                )]);
            }
            list.list.ClearRenderTargetView(rtv, color, rects);
        }
    }
    #[cfg(not(windows))]
    let _ = (this, handle, color, count, rects);
}

/// `ClearDepthStencilView(handle, flags, depth, stencil, count, rects)`;
/// flags: 1 depth, 2 stencil.
extern "system" fn clear_dsv(
    this: This,
    handle: u64,
    flags: u32,
    depth: f32,
    stencil: u8,
    count: u32,
    rects: *const Rect,
) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_work();
    }
    #[cfg(windows)]
    {
        use windows::Win32::Graphics::Direct3D12::D3D12_CLEAR_FLAGS;
        let (Some(list), Some(record)) = (host_list(this), crate::descriptor::dsv_record(handle))
        else {
            return;
        };
        let (Some(texture), Some(host)) = (
            crate::resource::host_resource(record.resource),
            crate::host::host(),
        ) else {
            return;
        };
        let rects = (count > 0 && !rects.is_null()).then(|| {
            // SAFETY: the title passes `count` rectangles, laid out as the host's.
            unsafe { std::slice::from_raw_parts(rects.cast(), count as usize) }
        });
        const DEPTH_WRITE: u32 = 0x10;
        ensure_state(&list, record.resource, DEPTH_WRITE);
        let dsv = host.dsv(texture, record.depth_desc().as_ref());
        // SAFETY: an open list, a fresh view of a live texture.
        unsafe {
            list.list
                .ClearDepthStencilView(dsv, D3D12_CLEAR_FLAGS(flags as i32 & 3), depth, stencil, rects)
        };
    }
    #[cfg(not(windows))]
    let _ = (this, handle, flags, depth, stencil, count, rects);
}

/// `CopyMemoryToMemoryX(destination, source, size, flags)`: GPU addresses
/// are title memory, copied at submission like buffer copies (if mapped).
extern "system" fn copy_memory(this: This, dst: u64, src: u64, len: u64, _flags: u32) {
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.count_write();
        list.count_work();
    }
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && dst != 0
        && src != 0
    {
        let len = len as usize;
        list.defer(move || {
            if crate::texture::committed(src as *const u8, len)
                && crate::texture::committed(dst as *const u8, len)
            {
                // SAFETY: both ranges are committed title memory.
                unsafe { std::ptr::copy(src as *const u8, dst as *mut u8, len) };
            }
        });
    }
    #[cfg(not(windows))]
    let _ = (this, dst, src, len);
}

/// `DriverHintX(feature, value)`: no hints for the host.
extern "system" fn driver_hint(_: This, _feature: u64, _value: u64) -> u64 {
    0
}

/// `DispatchX(x, y, z, flags)`: a dispatch with the GPU's initiator flags,
/// of which the host needs to know if `x` counts threads (as the
/// ray-tracing builder's does); another picks 32-lane waves, as the
/// program's own flags do.
extern "system" fn dispatch_x(this: This, x: u32, y: u32, z: u32, flags: u32) {
    const USE_THREAD_DIMENSIONS: u32 = 0x20;
    #[cfg(windows)]
    if flags & USE_THREAD_DIMENSIONS != 0 {
        state::dispatch_threads(this, x, y, z);
    } else {
        state::dispatch(this, x, y, z);
    }
    #[cfg(not(windows))]
    {
        let _ = (this, x, y, z, flags);
        count_dispatch();
    }
}

/// `BeginQuery(heap, type, index)`: recorded into the host list, in order
/// with the draws, for the types the host has (not timestamps, which only
/// end, nor a query running already).
extern "system" fn begin_query(this: This, heap: This, kind: u32, index: u32) {
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && let Some(host) = crate::query::host(heap, kind, index, 1)
        && list.begin_query(&host.heap, kind, index)
    {
        host.began(index);
    }
    #[cfg(not(windows))]
    let _ = (this, heap, kind, index);
}

/// `EndQuery(heap, type, index)`: takes a timestamp, or ends a query begun
/// in the same list; an end without its begin is dropped, so the query
/// has no host result.
extern "system" fn end_query(this: This, heap: This, kind: u32, index: u32) {
    #[cfg(windows)]
    if let Some(list) = host_list(this)
        && let Some(host) = crate::query::host(heap, kind, index, 1)
        && let Some(ended) = list.end_query(&host.heap, kind, index)
    {
        host.ended(index, ended);
    }
    #[cfg(not(windows))]
    let _ = (this, heap, kind, index);
}

/// `ResolveQueryData(heap, type, first, count, buffer, offset)`: writes
/// each query's result into the buffer once the list has run. Queries that
/// ran on the host GPU give its results ([`host_resolve`]); the others,
/// and every query of types the host lacks, stand-in values: timestamps
/// (type 2) the GPU clock then, occlusion counts (0) a million samples
/// passed and binary occlusion (1) visible, so nothing is culled by a
/// query that did not run.
/// shortcut: the console's other query types and result layouts are not
/// known; they get zeros, 16 bytes per query.
extern "system" fn resolve_queries(
    this: This,
    heap: This,
    kind: u32,
    first: u32,
    count: u32,
    buffer: This,
    offset: u64,
) {
    use crate::query::{BINARY_OCCLUSION, OCCLUSION, PIPELINE_STATISTICS, TIMESTAMP};
    let Some((data, size)) = crate::resource::buffer_bytes(buffer) else {
        return;
    };
    let (what, bytes) = match kind {
        OCCLUSION => (Written::Value(1 << 20), 8),
        BINARY_OCCLUSION => (Written::Value(1), 8),
        TIMESTAMP => (Written::GpuTimestamp, 8),
        PIPELINE_STATISTICS => (Written::Value(0), 88),
        _ => (Written::Value(0), 16),
    };
    let Some(end) = offset.checked_add(u64::from(count) * bytes) else {
        return;
    };
    if end > size {
        return;
    }
    // SAFETY: inside the buffer, checked above.
    let at = unsafe { data.add(offset as usize) };
    #[cfg(windows)]
    let measured = host_resolve(this, heap, kind, first, count, at, bytes);
    #[cfg(not(windows))]
    let (measured, _) = (Vec::<bool>::new(), (heap, first));
    for i in 0..u64::from(count) {
        if measured.get(i as usize) == Some(&true) {
            continue;
        }
        // Wider results are written as 8-byte pieces.
        for piece in 0..bytes / 8 {
            // SAFETY: inside the buffer, checked above.
            let at = unsafe { at.add((i * bytes + piece * 8) as usize) };
            let what = if piece == 0 { what } else { Written::Value(0) };
            defer_write(this, at as u64, what, 8);
        }
    }
}

/// Resolves on the host GPU the queries of `[first, first + count)` whose
/// last run there was of type `kind`, into title memory at `at` (`bytes`
/// per query, inside a buffer): which ones it resolves.
///
/// Occlusion and statistics results go into the buffer's host copy, where
/// later commands read them, and back into title memory after the
/// submission, as shader writes do ([`crate::shadow::Shadow::gpu_writes`]).
/// Timestamps go through read-back memory and are put on the title's GPU
/// clock after the submission (`Written::HostTimestamp`).
#[cfg(windows)]
fn host_resolve(this: This, heap: This, kind: u32, first: u32, count: u32, at: *mut u8, bytes: u64) -> Vec<bool> {
    use windows::Win32::Graphics::Direct3D12::*;
    let (Some(list), Some(host)) = (host_list(this), crate::query::host(heap, kind, first, count)) else {
        return Vec::new();
    };
    let measured: Vec<bool> = (first..first + count).map(|i| host.measured(i, kind)).collect();
    // Runs of measured queries: (first index from `first`, count).
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for (i, _) in measured.iter().enumerate().filter(|(_, m)| **m) {
        match runs.last_mut() {
            Some((from, n)) if *from + *n == i as u32 => *n += 1,
            _ => runs.push((i as u32, 1)),
        }
    }
    if runs.is_empty() {
        return Vec::new();
    }
    let len = u64::from(count) * bytes;
    let host_kind = D3D12_QUERY_TYPE(kind as i32);
    if kind == crate::query::TIMESTAMP {
        let Some((back, data)) = crate::host::host().and_then(|h| list.readback(h, len)) else {
            return Vec::new();
        };
        for &(from, n) in &runs {
            // SAFETY: an open list, a live heap, a read-back buffer of `len`
            // bytes.
            unsafe { list.list.ResolveQueryData(&host.heap, host_kind, first + from, n, &back, u64::from(from) * 8) };
        }
        for (i, _) in measured.iter().enumerate().filter(|(_, m)| **m) {
            // SAFETY: both inside their buffers (`i < count`).
            let (to, raw) = unsafe { (at.add(i * 8), data.add(i * 8)) };
            list.defer_write(to, Written::HostTimestamp(raw as usize), 8);
        }
        return measured;
    }
    let address = at as u64;
    // The host resolves to 8-byte aligned offsets only.
    let Some(shadow) = crate::shadow::find(address, len).filter(|s| s.offset(address).is_multiple_of(8)) else {
        return Vec::new();
    };
    // Title memory first (the stand-in values and the bytes between
    // results), then the results over it.
    shadow.refresh(&list, address, len);
    shadow.gpu_writes(&list, address, len);
    let to = shadow.offset(address);
    #[cfg(feature = "diagnostics")]
    list.note_op(|| format!("resolve {count} queries of type {kind} to {address:#x}"));
    // SAFETY: an open list, a live heap and buffer copy, kept until the list
    // is reset; the copy is promoted to a copy destination and left for
    // what reads it next.
    unsafe {
        for &(from, n) in &runs {
            let offset = to + u64::from(from) * bytes;
            list.list.ResolveQueryData(&host.heap, host_kind, first + from, n, &shadow.buffer, offset);
        }
        list.list.ResourceBarrier(&[crate::host::transition(
            &shadow.buffer,
            D3D12_RESOURCE_STATE_COPY_DEST,
            D3D12_RESOURCE_STATE_COMMON,
        )]);
    }
    measured
}

/// IUnknown 3, object 4, device child 1, allocator 2.
static ALLOCATOR_FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<ALLOCATOR, 24>());
    f[0] = allocator_query as *const ();
    f[2] = allocator_release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    f[8] = allocator_reset as *const (); // 0x40
    f[9] = object::ignore as *const (); // 0x48 allocation callback
    Table(f)
};

extern "system" fn set_pipeline(this: This, pipeline: This) {
    crate::pipeline::bound(pipeline);
    #[cfg(windows)]
    with_state(this, |_, state| state.set_pipeline(pipeline));
    #[cfg(not(windows))]
    let _ = this;
}

/// Pipeline syncs: waits for earlier work and cache flushes, at the top or
/// bottom of the pipe `(flags, control, address, size)`. Host lists order
/// their own work and every submission is waited for, so there is nothing
/// to do.
extern "system" fn sync(_: This, _flags: u32, _control: u32, _address: u64, _size: u64) {}

/// Reset with console flags `(allocator, pipeline, flags)`; the flags only
/// tune the console's command memory.
extern "system" fn reset_x(this: This, allocator: This, pipeline: This, _flags: u32) -> HRESULT {
    reset(this, allocator, pipeline)
}

/// Newer lists' 0x4a0 `(size, alignment) -> pointer`: memory the list
/// owns until it is reset, which the CPU writes and the GPU reads at the
/// same address; null on failure. Grand Theft Auto VI's BVH builder puts
/// its instance descriptions there (and wrote through our old
/// `E_NOTIMPL` as a pointer).
extern "system" fn allocate_upload(this: This, size: u64, alignment: u64) -> *mut u8 {
    let align = usize::try_from(alignment).unwrap_or(0).max(16);
    let Some(len) = usize::try_from(size).ok().and_then(|s| s.checked_add(align)) else {
        return std::ptr::null_mut();
    };
    if this.is_null() || !align.is_power_of_two() {
        return std::ptr::null_mut();
    }
    let mut block = vec![0u8; len];
    let start = block.as_mut_ptr();
    // SAFETY: the offset is below `align`, so `size` bytes fit after it.
    let at = unsafe { start.add(start.align_offset(align)) };
    // SAFETY: the title passes one of our lists.
    let uploads = unsafe { &(*this.cast::<List>()).uploads };
    // Moving the vector keeps its heap block where it is.
    uploads.lock().unwrap_or_else(|e| e.into_inner()).push(block);
    at
}

/// Draw state the host has no use for (root signatures: ours come with
/// the pipeline; depth bounds, depth order and shading rate hints): any
/// arguments, nothing returned.
extern "system" fn ignored_state(_: This) {}

extern "system" fn depth_bounds(this: This, min: f32, max: f32) {
    #[cfg(windows)]
    with_state(this, |list, s| s.set_depth_bounds(list, min, max));
    #[cfg(not(windows))]
    let _ = (this, min, max);
}

/// The title's draw state (decision 0007, step 3), kept for its draws or
/// set on the host list at once. Without a host GPU these do nothing.
#[cfg(windows)]
mod state {
    use super::{This, with_state};
    use crate::draw::{IndexView, VertexView};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Direct3D12::D3D12_VIEWPORT;

    /// `count` items at `items`, or none.
    fn items<'a, T>(items: *const T, count: u32) -> &'a [T] {
        if items.is_null() || count == 0 {
            return &[];
        }
        // SAFETY: the title passes `count` items.
        unsafe { std::slice::from_raw_parts(items, count as usize) }
    }

    pub(super) extern "system" fn topology(this: This, topology: u32) {
        with_state(this, |list, s| s.set_topology(list, topology));
    }
    pub(super) extern "system" fn viewports(
        this: This,
        count: u32,
        viewports: *const D3D12_VIEWPORT,
    ) {
        with_state(this, |list, s| s.set_viewports(list, items(viewports, count)));
    }
    pub(super) extern "system" fn scissors(this: This, count: u32, rects: *const RECT) {
        with_state(this, |list, s| s.set_scissors(list, items(rects, count)));
    }
    pub(super) extern "system" fn blend_factor(this: This, factor: *const [f32; 4]) {
        // SAFETY: the title passes four floats or null.
        with_state(this, |list, s| s.set_blend_factor(list, unsafe { factor.as_ref() }));
    }
    pub(super) extern "system" fn stencil_ref(this: This, reference: u32) {
        with_state(this, |list, s| s.set_stencil_ref(list, reference));
    }
    pub(super) extern "system" fn heaps(this: This, count: u32, heaps: *const This) {
        with_state(this, |_, s| s.set_heaps(items(heaps, count)));
    }
    pub(super) extern "system" fn compute_table(this: This, slot: u32, handle: u64) {
        with_state(this, |_, s| s.set_table(true, slot, handle));
    }
    pub(super) extern "system" fn graphics_table(this: This, slot: u32, handle: u64) {
        with_state(this, |_, s| s.set_table(false, slot, handle));
    }
    pub(super) extern "system" fn compute_constant(this: This, slot: u32, value: u32, offset: u32) {
        with_state(this, |_, s| s.set_constants(true, slot, &[value], offset));
    }
    pub(super) extern "system" fn graphics_constant(
        this: This,
        slot: u32,
        value: u32,
        offset: u32,
    ) {
        with_state(this, |_, s| s.set_constants(false, slot, &[value], offset));
    }
    pub(super) extern "system" fn compute_constants(
        this: This,
        slot: u32,
        count: u32,
        values: *const u32,
        offset: u32,
    ) {
        with_state(this, |_, s| {
            s.set_constants(true, slot, items(values, count), offset)
        });
    }
    pub(super) extern "system" fn graphics_constants(
        this: This,
        slot: u32,
        count: u32,
        values: *const u32,
        offset: u32,
    ) {
        with_state(this, |_, s| {
            s.set_constants(false, slot, items(values, count), offset)
        });
    }
    /// Root constant buffer, shader resource and unordered access views.
    pub(super) extern "system" fn compute_view(this: This, slot: u32, address: u64) {
        with_state(this, |_, s| s.set_view(true, slot, address));
    }
    pub(super) extern "system" fn graphics_view(this: This, slot: u32, address: u64) {
        with_state(this, |_, s| s.set_view(false, slot, address));
    }
    pub(super) extern "system" fn index_buffer(this: This, view: *const IndexView) {
        // SAFETY: the title passes a view or null.
        with_state(this, |_, s| s.set_index_buffer(unsafe { view.as_ref() }));
    }
    pub(super) extern "system" fn vertex_buffers(
        this: This,
        start: u32,
        count: u32,
        views: *const VertexView,
    ) {
        with_state(this, |_, s| {
            if views.is_null() {
                s.clear_vertex_buffers(start, count);
            } else {
                s.set_vertex_buffers(start, items(views, count));
            }
        });
    }
    /// `(count, handles, one range, depth)`: with one range, the handles
    /// follow the first at the render target view size (128 bytes).
    pub(super) extern "system" fn targets(
        this: This,
        count: u32,
        handles: *const u64,
        one_range: i32,
        depth: *const u64,
    ) {
        let handles: Vec<u64> = if one_range != 0 && !handles.is_null() {
            // SAFETY: the title passes the first handle.
            let first = unsafe { *handles };
            (0..u64::from(count)).map(|i| first + 128 * i).collect()
        } else {
            items(handles, count).to_vec()
        };
        // SAFETY: the title passes a handle or null.
        let depth = unsafe { depth.as_ref() }.copied().unwrap_or(0);
        with_state(this, |list, s| s.set_targets(list, &handles, depth));
    }
    pub(super) extern "system" fn draw(
        this: This,
        vertices: u32,
        instances: u32,
        first: u32,
        first_instance: u32,
    ) {
        super::count_draw();
        with_state(this, |list, s| {
            s.draw(list, vertices, instances, first, first_instance)
        });
    }
    pub(super) extern "system" fn dispatch(this: This, x: u32, y: u32, z: u32) {
        super::count_dispatch();
        with_state(this, |list, s| s.dispatch(list, x, y, z, false));
    }
    /// A dispatch whose `x` counts threads.
    pub(super) extern "system" fn dispatch_threads(this: This, x: u32, y: u32, z: u32) {
        super::count_dispatch();
        with_state(this, |list, s| s.dispatch(list, x, y, z, true));
    }
    /// The groups a mesh dispatch of `x` × `y` × `z` groups runs: the
    /// title's mesh programs read only the x group id, so `x` (y and z
    /// other than 1 are logged once); none if any dimension is empty.
    pub(super) fn mesh_groups(x: u32, y: u32, z: u32) -> Option<u32> {
        if x == 0 || y == 0 || z == 0 {
            return None;
        }
        if y != 1 || z != 1 {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!(x, y, z, "a mesh dispatch with y or z groups"));
        }
        Some(x)
    }
    /// `DispatchMesh(x, y, z)` (the PC method of List6).
    pub(super) extern "system" fn dispatch_mesh(this: This, x: u32, y: u32, z: u32) {
        super::count_draw();
        if let Some(x) = mesh_groups(x, y, z) {
            with_state(this, |list, s| s.dispatch_mesh(list, x));
        }
    }
    /// `DispatchMeshX(x, flags, indirect arguments, payload)`. With an
    /// indirect address the group counts are in GPU memory there, run as
    /// indirect execution of one mesh dispatch (behind the same switch).
    /// Inferred, not documented: the arguments are those of a PC indirect
    /// mesh dispatch (D3D12_DISPATCH_MESH_ARGUMENTS, three 32-bit group
    /// counts, as the title's mesh dispatch signatures have) and `x` is then
    /// unused; `flags` and `payload` are unknown.
    pub(super) extern "system" fn dispatch_mesh_x(
        this: This,
        x: u32,
        _flags: u64,
        indirect: u64,
        _payload: u64,
    ) {
        const MESH: u32 = 10;
        const STRIDE: u32 = 12;
        super::count_draw();
        if indirect == 0 {
            if x != 0 {
                with_state(this, |list, s| s.dispatch_mesh(list, x));
            }
            return;
        }
        let mut ran = false;
        if super::indirect_on() {
            super::hoist(this);
            with_state(this, |list, s| {
                ran = s.execute_indirect(list, (MESH, STRIDE, 1), indirect, None);
            });
        }
        if !ran {
            if let Some(list) = super::host_list(this) {
                list.missed_draw();
            }
            super::INDIRECT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    pub(super) extern "system" fn draw_indexed(
        this: This,
        indices: u32,
        instances: u32,
        first: u32,
        base: i32,
        first_instance: u32,
    ) {
        super::count_draw();
        with_state(this, |list, s| {
            s.draw_indexed(list, indices, instances, first, base, first_instance)
        });
    }
}

/// Without a host GPU every draw state call is ignored.
#[cfg(not(windows))]
mod state {
    pub(super) use super::dispatch;
    pub(super) use super::draw;
    pub(super) use super::draw as draw_indexed;
    pub(super) use super::draw as dispatch_mesh;
    pub(super) use super::draw as dispatch_mesh_x;
    pub(super) use super::ignored_state as blend_factor;
    pub(super) use super::ignored_state as compute_constant;
    pub(super) use super::ignored_state as compute_constants;
    pub(super) use super::ignored_state as compute_table;
    pub(super) use super::ignored_state as compute_view;
    pub(super) use super::ignored_state as graphics_constant;
    pub(super) use super::ignored_state as graphics_constants;
    pub(super) use super::ignored_state as graphics_table;
    pub(super) use super::ignored_state as graphics_view;
    pub(super) use super::ignored_state as heaps;
    pub(super) use super::ignored_state as index_buffer;
    pub(super) use super::ignored_state as scissors;
    pub(super) use super::ignored_state as stencil_ref;
    pub(super) use super::ignored_state as targets;
    pub(super) use super::ignored_state as topology;
    pub(super) use super::ignored_state as vertex_buffers;
    pub(super) use super::ignored_state as viewports;
}

fn count_draw() {
    DRAWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Draws of the kinds the host does not run yet (indirect, mesh), and
/// dispatches, counted per frame for the log.
/// shortcut: dropped until their shaders translate (decision 0007).
#[cfg(not(windows))]
extern "system" fn draw(_: This) {
    count_draw();
}

/// Whether the title's indirect draws and dispatches run on the host:
/// always now that their arguments are clamped and budgeted
/// (`crate::indirect`) and pipelines are made off the render thread.
#[cfg(windows)]
fn indirect_on() -> bool {
    true
}

/// `ExecuteIndirect(signature, max count, arguments, offset, count buffer,
/// offset)`. Grand Theft Auto VI's signatures (title+0x2479466, 2026-11-24)
/// hold one argument each: draw (stride 16), indexed draw (20), dispatch
/// (12), mesh dispatch (12), and the console's single-pass dispatch, type
/// 105 (stride 12: the three group counts; its flags live in the
/// signature, the title passes none), run as a plain dispatch. They run
/// through the host's indirect execution (`State::execute_indirect`).
/// shortcut: signatures that change root arguments are dropped (logged
/// once per signature).
extern "system" fn execute_indirect(
    this: This,
    signature: This,
    max: u32,
    arguments: This,
    offset: u64,
    count: This,
    count_offset: u64,
) {
    count_draw();
    let Some((stride, args)) = crate::signature::layout(signature) else {
        return;
    };
    let address = crate::resource::buffer_bytes(arguments).map(|(p, _)| p as u64 + offset);
    let counter = crate::resource::buffer_bytes(count).map(|(p, _)| p as u64 + count_offset);
    let mut ran = false;
    #[cfg(windows)]
    if indirect_on()
        && let ([[kind, ..]], Some(address)) = (args, address)
        && (count.is_null() || counter.is_some())
    {
        const DISPATCH: u32 = 2;
        const DISPATCH_SINGLE: u32 = 105;
        let kind = if *kind == DISPATCH_SINGLE { DISPATCH } else { *kind };
        if kind != DISPATCH {
            hoist(this);
        }
        with_state(this, |list, s| {
            ran = s.execute_indirect(list, (kind, stride, max), address, counter);
        });
    } else {
        #[cfg(all(windows, feature = "diagnostics"))]
        crate::pipeline::note_why(format!(
            "indirect signature {args:?}: arguments in a buffer {}, count in a buffer {}",
            address.is_some(),
            count.is_null() || counter.is_some()
        ));
    }
    if ran {
        return;
    }
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.missed_draw();
    }
    INDIRECT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    static SEEN: std::sync::Mutex<Vec<usize>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&(signature as usize)) {
        seen.push(signature as usize);
        tracing::info!(
            stride,
            ?args,
            max,
            address = ?address.map(|a| format!("{a:#x}")),
            counted = !count.is_null(),
            "indirect execution dropped (first time for this signature)"
        );
    }
}

/// Indirect executions dropped since the last call.
pub(crate) fn take_indirect() -> u64 {
    INDIRECT.swap(0, std::sync::atomic::Ordering::Relaxed)
}
static INDIRECT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn count_dispatch() {
    DISPATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}
#[cfg(not(windows))]
extern "system" fn dispatch(_: This) {
    count_dispatch();
}
static DRAWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static DISPATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Draws (any kind, mesh and indirect included) and compute dispatches
/// recorded since the last call.
pub(crate) fn take_counts() -> (u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (DRAWS.swap(0, Relaxed), DISPATCHES.swap(0, Relaxed))
}

/// A GPU wait for a memory value `(address, comparison, reference, ...)`.
/// Another queue or the CPU writes the value: the submission waits for it
/// before its list runs (`queue::execute`).
extern "system" fn wait_value(this: This, address: u64, comparison: u32, reference: u64) {
    note_wait(this, address, comparison, reference, 8);
}
extern "system" fn wait_value32(this: This, address: u64, comparison: u32, reference: u32) {
    note_wait(this, address, comparison, u64::from(reference), 4);
}
/// Keeps the wait, waited for when the list is submitted.
fn note_wait(this: This, address: u64, comparison: u32, reference: u64, bytes: usize) {
    #[cfg(windows)]
    cut(this);
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.note_wait(crate::memwait::Wait {
            address: address as usize,
            comparison,
            reference,
            bytes,
        });
    }
    let _ = (this, address, comparison, reference, bytes);
}

/// The enhanced barriers `(group count, groups)`: buffer, texture and
/// global barriers with layouts instead of states. Every host use of a
/// title texture (clears, copies) moves its host copy to the state it needs
/// first, so only their ordering matters: one barrier on every shader
/// write (the title's buffer barriers order compute passes).
extern "system" fn barrier_groups(this: This, count: u32, _groups: *const u8) {
    #[cfg(windows)]
    if count > 0
        && let Some(list) = host_list(this)
    {
        use windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_BARRIER;
        // SAFETY: the same layout; no resource named.
        let barrier = unsafe { std::mem::transmute::<Barrier, D3D12_RESOURCE_BARRIER>(UAV_BARRIER) };
        // SAFETY: an open list.
        unsafe { list.list.ResourceBarrier(&[barrier]) };
    }
    #[cfg(not(windows))]
    let _ = (this, count);
}

/// The bottom-of-pipe value writes `(address, value, flush, flags)`, 32 and
/// 64 bits: done when the list has run, as the GPU would. Flag 1 writes the
/// GPU timestamp instead (64-bit form), flag 2 the system timestamp.
extern "system" fn write32(this: This, address: u64, value: u32, _flush: u32, _flags: u32) {
    defer_write(this, address, Written::Value(u64::from(value)), 4);
}
extern "system" fn write64(this: This, address: u64, value: u64, _flush: u32, flags: u32) {
    const GPU_TIMESTAMP: u32 = 1;
    const SYSTEM_TIMESTAMP: u32 = 2;
    let what = match flags {
        GPU_TIMESTAMP => Written::GpuTimestamp,
        SYSTEM_TIMESTAMP => Written::SystemTimestamp,
        _ => Written::Value(value),
    };
    defer_write(this, address, what, 8);
}

fn defer_write(this: This, address: u64, what: Written, bytes: usize) {
    if address == 0 {
        return;
    }
    #[cfg(windows)]
    if let Some(list) = host_list(this) {
        list.defer_write(address as *mut u8, what, bytes);
        return;
    }
    let _ = this;
    // Without a host GPU nothing runs later: write now.
    what.write(address as *mut u8, bytes);
}

/// The list methods by their slot in older titles' lists (the 2020-era
/// layout): IUnknown 3, object 4, device child 1, list 1, graphics list 96
/// (59 as on PC, then console ones), then versions 1-6 (6 + 1 + 1 + 11 + 3
/// + 13): 140 slots.
const METHODS: [(usize, *const ()); 52] = [
    (0, list_query as *const ()),
    (2, list_release as *const ()),
    (7, object::parent_device as *const ()),      // 0x38
    (8, list_type as *const ()),                  // 0x40
    (9, close as *const ()),                      // 0x48
    (10, reset as *const ()),                     // 0x50
    (12, state::draw as *const ()),               // 0x60 instanced
    (13, state::draw_indexed as *const ()),       // 0x68 indexed
    (14, state::dispatch as *const ()),           // 0x70
    (15, copy_buffer_region as *const ()),        // 0x78
    (16, copy_texture_region as *const ()),       // 0x80
    (17, copy_resource as *const ()),             // 0x88
    (19, resolve as *const ()),                   // 0x98
    (20, state::topology as *const ()),           // 0xa0
    (21, state::viewports as *const ()),          // 0xa8
    (22, state::scissors as *const ()),           // 0xb0
    (23, state::blend_factor as *const ()),       // 0xb8
    (24, state::stencil_ref as *const ()),        // 0xc0
    (25, set_pipeline as *const ()),              // 0xc8
    (26, barrier as *const ()),                   // 0xd0
    (32, state::heaps as *const ()),              // 0x100
    (33, ignored_state as *const ()),             // 0x108 compute root signature
    (34, ignored_state as *const ()),             // 0x110 graphics root signature
    (35, state::compute_table as *const ()),      // 0x118
    (36, state::graphics_table as *const ()),     // 0x120
    (37, state::compute_constant as *const ()),   // 0x128
    (38, state::graphics_constant as *const ()),  // 0x130
    (39, state::compute_constants as *const ()),  // 0x138
    (40, state::graphics_constants as *const ()), // 0x140
    (41, state::compute_view as *const ()),       // 0x148 CBV
    (42, state::graphics_view as *const ()),      // 0x150 CBV
    (43, state::compute_view as *const ()),       // 0x158 SRV
    (44, state::graphics_view as *const ()),      // 0x160 SRV
    (45, state::compute_view as *const ()),       // 0x168 UAV
    (46, state::graphics_view as *const ()),      // 0x170 UAV
    (47, state::index_buffer as *const ()),       // 0x178
    (48, state::vertex_buffers as *const ()),     // 0x180
    (50, state::targets as *const ()),            // 0x190
    (51, clear_dsv as *const ()),                 // 0x198
    (52, clear_rtv as *const ()),                 // 0x1a0
    (59, execute_indirect as *const ()),          // 0x1d8 indirect
    (60, copy_memory as *const ()),               // 0x1e0
    (68, depth_bounds as *const ()),              // 0x220
    (69, write32 as *const ()),                   // 0x228
    (70, write64 as *const ()),                   // 0x230
    (71, wait_value32 as *const ()),              // 0x238, 32 bits
    (72, wait_value as *const ()),                // 0x240, 64 bits
    (73, ignored_state as *const ()),             // 0x248 pixel shader depth order
    (83, copy_texture_region_x as *const ()),     // 0x298
    (84, copy_resource_x as *const ()),           // 0x2a0
    (124, ignored_state as *const ()),            // 0x3e0 shading rate
    (127, state::dispatch_mesh as *const ()),     // 0x3f8 mesh dispatch
];
/// The rest of [`METHODS`], kept apart only to stay readable.
const MORE_METHODS: [(usize, *const ()); 9] = [
    (28, begin_query as *const ()),            // 0xe0
    (29, end_query as *const ()),              // 0xe8
    (30, resolve_queries as *const ()),        // 0xf0
    (78, dispatch_x as *const ()),             // 0x270
    (98, state::dispatch_mesh_x as *const ()), // 0x310
    (99, driver_hint as *const ()),            // 0x318
    // Pipeline syncs at the top and bottom of the pipe, 0x408 and 0x410.
    (129, sync as *const ()),
    (130, sync as *const ()),
    (137, reset_x as *const ()), // 0x448
];

/// Newer titles' lists have two more methods in the console part of the
/// base list, after the clears (0x198/0x1a0 as before) and before
/// ExecuteIndirect, which moved from 0x1d8 to 0x1e8: every slot from 0x1d8
/// on is two later (taken here as starting exactly at 0x1d8). Inferred from
/// Grand Theft Auto VI's call shapes: ExecuteIndirect 0x1e8, bottom-of-pipe
/// writes 0x238/0x240, waits 0x248/0x250, depth bounds 0x230, shading rate
/// 0x3f0/0x400, syncs 0x418/0x420, ResetX 0x458 (asserts on failure).
const fn newer_slot(slot: usize) -> usize {
    if slot >= 59 { slot + 2 } else { slot }
}

/// Methods only newer titles' lists have. 0x490: the enhanced barrier
/// `(group count, groups)`, with 16-byte groups `(type, count, barriers)`
/// and 40-byte buffer barriers (title+0x95e0 builds them). 0x470 `(u64)`,
/// called from Grand Theft Auto VI's state flush with its result unused.
/// shortcut: 0x470's purpose is unknown; ignored until a title shows it
/// matters.
const NEWER_METHODS: [(usize, *const ()); 3] = [
    (142, ignored_state as *const ()),
    (146, barrier_groups as *const ()),
    (148, allocate_upload as *const ()),
];

const fn list_table(newer: bool) -> [*const (); 160] {
    let mut f = object::d3d12_object(object::missing_table::<LIST, 160>());
    let mut i = 0;
    while i < METHODS.len() + MORE_METHODS.len() {
        let (slot, method) = if i < METHODS.len() {
            METHODS[i]
        } else {
            MORE_METHODS[i - METHODS.len()]
        };
        f[if newer { newer_slot(slot) } else { slot }] = method;
        i += 1;
    }
    let mut i = 0;
    while newer && i < NEWER_METHODS.len() {
        f[NEWER_METHODS[i].0] = NEWER_METHODS[i].1;
        i += 1;
    }
    f
}

/// Every slot up to the limit exists and logs, for the methods newer
/// titles add.
static LIST_FUNCTIONS: Table<160> = Table(list_table(false));
static NEWER_LIST_FUNCTIONS: Table<160> = Table(list_table(true));

#[cfg(all(test, windows))]
pub(crate) fn has_newer_table(list: This) -> bool {
    // SAFETY: one of our lists; its first field is the table. The device
    // tests can switch layouts, so use this list's table.
    let table = unsafe { *list.cast::<*const *const ()>() };
    table == NEWER_LIST_FUNCTIONS.0.as_ptr()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::slot;

    /// Mesh programs read only the x group id, so a mesh dispatch runs its
    /// x groups; one with no groups in any dimension runs nothing.
    #[cfg(windows)]
    #[test]
    fn mesh_dispatches_run_x_groups_unless_empty() {
        use state::mesh_groups;
        assert_eq!(mesh_groups(5, 1, 1), Some(5));
        assert_eq!(mesh_groups(0, 1, 1), None);
        assert_eq!(mesh_groups(5, 0, 1), None);
        assert_eq!(mesh_groups(5, 1, 0), None);
        assert_eq!(mesh_groups(5, 2, 3), Some(5));
        assert_eq!(mesh_groups(5, u32::MAX, u32::MAX), Some(5));
    }

    /// An indirect mesh dispatch the host does not run (indirect execution
    /// off, as in tests) is counted with the other indirect work dropped.
    #[cfg(windows)]
    #[test]
    fn indirect_mesh_dispatches_not_run_are_counted() {
        let null = std::ptr::null();
        let mut allocator = null;
        assert_eq!(
            create_allocator(null, 0, &ALLOCATOR_IIDS[0], &mut allocator),
            S_OK
        );
        let mut list = null;
        let hr = create_list(null, 0, 0, allocator, null, &LIST_IIDS[1], &mut list);
        assert_eq!(hr, S_OK);
        let arguments = [7u32, 1, 1];
        let before = INDIRECT.load(std::sync::atomic::Ordering::Relaxed);
        state::dispatch_mesh_x(list, 0, 0, arguments.as_ptr() as u64, 0);
        assert!(INDIRECT.load(std::sync::atomic::Ordering::Relaxed) > before);
        assert_eq!(close(list), S_OK);
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(list), 0);
        assert_eq!(release(allocator), 0);
    }

    #[test]
    fn lists_are_cut_at_waits_that_follow_work() {
        if crate::host::host().is_none() {
            return;
        }
        let null = std::ptr::null();
        let (mut allocator, mut list) = (null, null);
        assert_eq!(create_allocator(null, 0, &ALLOCATOR_IIDS[0], &mut allocator), S_OK);
        assert_eq!(new_list(0, allocator, &LIST_IIDS[0], &mut list), S_OK);
        let memory = [0u64; 2];
        let at = |i: usize| (&raw const memory[i]) as u64;
        let value = |i: usize| {
            // SAFETY: inside the array.
            unsafe { (at(i) as *const u64).read_volatile() }
        };
        for _ in 0..2 {
            // SAFETY: inside the array.
            unsafe { (at(0) as *mut [u64; 2]).write_volatile([0; 2]) };
            state::stencil_ref(list, 3);
            // No work yet: not cut.
            wait_value(list, at(0), 7, 0);
            assert_eq!(submitted(list).len(), 1);
            host_list(list).unwrap().count_work();
            write64(list, at(0), 1, 0, 0);
            wait_value(list, at(1), 7, 4);
            assert_eq!(submitted(list).len(), 2);
            assert_eq!(close(list), S_OK);
            let parts = submitted(list);
            assert!(parts[0].unmet().is_empty() && parts[1].unmet().len() == 1);
            crate::host::host().unwrap().execute(&[&*parts[0]]);
            assert_eq!(value(0), 1);
            // Reset while the second part is still to run.
            assert_eq!(reset(list, allocator, null), S_OK);
            assert_eq!(submitted(list).len(), 1);
            // SAFETY: inside the array.
            unsafe { (at(1) as *mut u64).write_volatile(4) };
            assert!(parts[1].unmet().is_empty());
            crate::host::host().unwrap().execute(&[&*parts[1]]);
        }
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(list), 0);
        assert_eq!(release(allocator), 0);
    }

    #[test]
    fn every_inline_command_falls_back_to_the_table() {
        let null = std::ptr::null();
        let mut allocator = null;
        assert_eq!(
            create_allocator(null, 0, &ALLOCATOR_IIDS[0], &mut allocator),
            S_OK
        );
        let mut list = null;
        let hr = create_list(null, 0, 0, allocator, null, &LIST_IIDS[1], &mut list);
        assert_eq!(hr, S_OK);
        // What the title's inline draw checks: put < draw limit.
        // SAFETY: the list is 0x560 bytes.
        let (put, draw_limit, api_limit) = unsafe {
            let at = |o: usize| list.cast::<u8>().add(o).cast::<u64>().read();
            (at(0x18), at(0x28), at(0x30))
        };
        assert!(put >= draw_limit && put >= api_limit);
        // The console's ID3D12GraphicsCommandList6, as Grand Theft Auto VI asks.
        let mut same = null;
        assert_eq!(list_query(list, &LIST_IIDS[7], &mut same), S_OK);
        assert_eq!(same, list);
        assert_eq!(slot::<extern "system" fn(This) -> u32>(list, 0x10)(list), 1);
        assert_eq!(slot::<extern "system" fn(This) -> u32>(list, 0x40)(list), 0);
        assert_eq!(
            slot::<extern "system" fn(This) -> HRESULT>(list, 0x48)(list),
            S_OK
        );
        // List-owned upload memory: aligned, writable, gone at reset.
        let p = allocate_upload(list, 40, 0x100);
        assert!(!p.is_null() && (p as usize).is_multiple_of(0x100));
        // SAFETY: 40 bytes were handed out.
        unsafe { p.write_bytes(0xab, 40) };
        assert!(allocate_upload(list, 8, 0x30).is_null(), "not a power of two");
        assert_eq!(reset(list, allocator, null), S_OK);
        // SAFETY: the list is ours.
        assert!(unsafe { &(*list.cast::<List>()).uploads }.lock().unwrap().is_empty());
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        assert_eq!(release(list), 0);
        assert_eq!(release(allocator), 0);
    }

    /// An offset near `u64::MAX` must not overflow and write outside the
    /// query destination buffer.
    #[test]
    fn resolve_queries_rejects_overflowing_offset() {
        let null = std::ptr::null();
        let mut allocator = null;
        assert_eq!(
            create_allocator(null, 0, &ALLOCATOR_IIDS[0], &mut allocator),
            S_OK
        );
        let mut list = null;
        assert_eq!(
            create_list(null, 0, 0, allocator, null, &LIST_IIDS[1], &mut list),
            S_OK
        );
        let buffer = crate::resource::test_buffer(64);
        // Used to overflow `offset + count * 8` (panic in debug, wrap in release).
        resolve_queries(list, null, 0, 0, 2, buffer, u64::MAX - 8);
        let release = |o: This| slot::<extern "system" fn(This) -> u32>(o, 0x10)(o);
        for object in [buffer, list, allocator] {
            assert_eq!(release(object), 0);
        }
    }

}
