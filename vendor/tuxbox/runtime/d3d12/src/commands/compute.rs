//! Compute commands on the Xbox graphics command list.
use super::{List, parent, recording};
use crate::device;
use core::ffi::c_void;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::{
    D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION, D3D12_RESOURCE_DIMENSION_BUFFER,

};

pub(super) unsafe extern "system" fn bind_root(list: &List, signature: *mut c_void) {
    let mut state = recording(list);
    if !state.can_record_shader(&list.native) {
        return;
    }
    // SAFETY: the guest holds its signature during this call. Only a native
    // interface belonging to this list's device is passed to the driver.
    match unsafe { device::compute::root_proxy(signature, parent(&list.parent)) } {
        Ok(root) => {
            let native = root.map(|r| &r.native);
            state.bindings.compute_root(native, root.map(|r| &r.state));
            state.unresolved_roots[1] = 0;
            unsafe { list.native.SetComputeRootSignature(native) };
        }
        Err(error) => state.error = error,
    }
}

pub(super) unsafe extern "system" fn one_constant<const COMPUTE: bool>(list: &List, index: u32, data: u32, offset: u32) {
    unsafe { constants::<COMPUTE>(list, index, 1, (&raw const data).cast(), offset) };
}

pub(super) unsafe extern "system" fn constants<const COMPUTE: bool>(
    list: &List,
    index: u32,
    count: u32,
    data: *const c_void,
    offset: u32,
) {
    let mut state = recording(list);
    if !state.can_record_for::<COMPUTE>(&list.native) || count == 0 {
        return;
    }
    if data.is_null() || offset.checked_add(count).is_none_or(|end| end > 64) {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    // SAFETY: the caller supplies count readable DWORDs for this command.
    let words = unsafe { core::slice::from_raw_parts(data.cast(), count as usize) };
    if COMPUTE {
        state.bindings.constants(index, offset, words);
        unsafe { list.native.SetComputeRoot32BitConstants(index, count, data, offset); }
    } else {
        state.bindings.graphics_constants(index, offset, words);
        unsafe { list.native.SetGraphicsRoot32BitConstants(index, count, data, offset); }
    }
}

use super::compute_bindings::BufferView;

thread_local! {
    /// The host import root_import resolved last, for a root view table's
    /// raw view (device::root_tables).
    static ROOT_IMPORT: std::cell::RefCell<Option<windows::Win32::Graphics::Direct3D12::ID3D12Resource>> = const { std::cell::RefCell::new(None) };
}

/// root_address: the title address did not resolve (stale clear).
const UNRESOLVED: u64 = u64::MAX;

fn root_buffer<const COMPUTE: bool>(list: &List, index: u32, guest: u64, view: fn(u64) -> BufferView) {
    let mut state = recording(list);
    if state.can_record_for::<COMPUTE>(&list.native) {
        ROOT_IMPORT.with(|b| b.take());
        let Some(mut address) = root_address(list, &mut state, guest) else { return };
        let bit = 1u64.checked_shl(index).unwrap_or(0);
        if address == UNRESOLVED {
            state.unresolved_roots[COMPUTE as usize] |= bit;
            address = 0;
        } else {
            state.unresolved_roots[COMPUTE as usize] &= !bit;
        }
        if address == 0 && let Some(sink) = null_sink(list) {
            address = sink;
        }
        let buffer = ROOT_IMPORT.with(|b| b.take());
        let view = view(address);
        // Root UAV writes set no view marks: flag a shadow copy they land in
        // for a whole write-back (VIctory cd94d35).
        if let (BufferView::Uav(_), Some(buffer)) = (view, buffer.as_ref()) {
            unsafe { device::note_root_uav(parent(&list.parent), windows::core::Interface::as_raw(buffer)) };
        }
        state.bindings.note_guest::<COMPUTE>(index, matches!(view, BufferView::Srv(_) | BufferView::Uav(_)).then_some(guest));
        state.bindings.note_root_va::<COMPUTE>(index, guest);
        state.bindings.bind_buffer::<COMPUTE>(&list.native, index, view, buffer.as_ref());
    }
}

/// test61 lost its device to a GPU page fault at VA 0x0 (DRED): a root view
/// bound as null (title address 0 or uncommitted) is still read by its
/// shader, and on this driver that faults. Such views get the address of a
/// 64 KiB zeroed buffer of their device instead: reads see zeros, writes
/// land there. TUXBOX_NULL_ROOT_SINK=0 binds 0 again.
pub(super) fn null_sink(list: &List) -> Option<u64> {
    use std::collections::HashMap;
    use std::sync::Mutex;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    struct Sink(ID3D12Resource);
    // SAFETY: D3D12 resources are free-threaded; it is never mutated.
    unsafe impl Send for Sink {}
    static SINKS: Mutex<Option<HashMap<usize, Sink>>> = Mutex::new(None);
    if crate::env_toggle!("TUXBOX_NULL_ROOT_SINK") == Some("0") {
        return None;
    }
    let mut device: Option<ID3D12Device> = None;
    unsafe { list.native.GetDevice(&mut device) }.ok()?;
    let device = device?;
    let device = &device;
    let mut sinks = SINKS.lock().unwrap_or_else(|e| e.into_inner());
    let sinks = sinks.get_or_insert_with(HashMap::new);
    let key = windows::core::Interface::as_raw(device) as usize;
    if let Some(sink) = sinks.get(&key) {
        return Some(unsafe { sink.0.GetGPUVirtualAddress() });
    }
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: 0x10000,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer) }.ok()?;
    let buffer = buffer?;
    let va = unsafe { buffer.GetGPUVirtualAddress() };
    crate::diagnostics::log_line(format_args!("null root views bound to a 64 KiB zeroed buffer at {va:#x} instead of GPU VA 0 (TUXBOX_NULL_ROOT_SINK=0 binds 0)"));
    sinks.insert(key, Sink(buffer));
    Some(va)
}

/// A root view's shader reads past the bound address (a CBV up to 64 KiB, an
/// SRV/UAV unbounded), so importing one byte left the rest of the run outside
/// GPU memory: a GPUVM fault or device removal (rebox aadcc49 adopts a 64 KiB
/// run). Import up to 64 KiB of committed memory; an uncommitted address binds
/// null instead of handing the GPU a raw title VA. TUXBOX_ROOT_SPAN=1 restores
/// the one-byte import with pass-through.
fn root_address(list: &List, state: &mut super::Recording, guest: u64) -> Option<u64> {
    use windows::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, VirtualQuery};
    crate::diagnostics::transient_pool::touch(windows::core::Interface::as_raw(&list.native) as usize, guest);
    let _stat = branches::Phase::new(branches::TOTAL);
    if guest == 0 || crate::env_toggle!("TUXBOX_ROOT_SPAN") == Some("1") {
        branches::count(branches::NULL);
        return address(list, state, guest, 1);
    }
    // A page only the GPU writes is still reserved in a lazy title view.
    // 49702da committed only to the end of the guest address's own 64 KiB
    // page, so an unaligned window's second page stayed reserved, the span
    // walk below stopped there and the import fell short of what the shader
    // reads (the GPU fault the note above describes). The whole window is
    // committed (only pages inside lazy views are touched).
    // TUXBOX_ROOT_COMMIT_PAGE=1 commits the first page alone as before.
    let window = if crate::env_toggle!("TUXBOX_ROOT_COMMIT_PAGE") == Some("1") {
        0x10000 - (guest as usize & 0xffff)
    } else {
        0x10000
    };
    {
        let _p = branches::Phase::new(branches::COMMIT);
        device::commit_lazy(guest as usize, window);
    }
    // xcl c1a9b52 (GPL-3.0, idea): a VirtualQuery scans its whole region
    // under the address-space lock (test137: 16 ms per GiB of view, and the
    // GPU page pool is one 5.4 GiB view), and every root view bind made one
    // or more. A window xmem knows committed in one lazy view needs none: it
    // is read-write to the view's end. TUXBOX_COMMIT_BITS=0 (in xmem) walks.
    let known = {
        let _p = branches::Phase::new(branches::STATE);
        device::view_state(guest as usize, 0x10000)
    };
    if known.committed() && known.one_view() {
        branches::count(branches::KNOWN);
        let end = (known.base + known.size) as u64;
        let span = end.saturating_sub(guest).min(0x10000).max(1);
        let _p = branches::Phase::new(branches::IMPORT);
        return root_import(list, state, guest, span);
    }
    // test154: all binds took the walk below (2 ms each) because the 64 KiB
    // window ran past the end of the lazy view holding the address, so the
    // whole-window ask above said "not lazy memory". The walk stops at that
    // allocation's end anyway (AllocationBase changes): ask for the view
    // holding the address and, if every page from the address to the
    // window's end inside that view is known committed, take that span.
    // TUXBOX_ROOT_VIEW_END=0 walks as before.
    if crate::env_toggle!("TUXBOX_ROOT_VIEW_END") != Some("0") {
        let holder = {
            let _p = branches::Phase::new(branches::STATE);
            device::view_state(guest as usize, 1)
        };
        if holder.one_view() {
            let end = ((holder.base + holder.size) as u64).min(guest.saturating_add(0x10000));
            let span = end.saturating_sub(guest).max(1);
            let inside = {
                let _p = branches::Phase::new(branches::STATE);
                device::view_state(guest as usize, span as usize)
            };
            if inside.committed() && inside.one_view() {
                branches::count(branches::KNOWN);
                let _p = branches::Phase::new(branches::IMPORT);
                return root_import(list, state, guest, span);
            }
        }
    }
    branches::count(branches::QUERY);
    let query = branches::Phase::new(branches::WALK);
    if let Some((start, end)) = walk_cache(guest, None) {
        drop(query);
        let span = end.saturating_sub(guest).min(0x10000).max(1);
        let _p = branches::Phase::new(branches::IMPORT);
        return span_import(list, state, guest, span, (start, end));
    }
    let mut region = MEMORY_BASIC_INFORMATION::default();
    if unsafe { VirtualQuery(Some(guest as _), &mut region, size_of_val(&region)) } == 0 || region.State != MEM_COMMIT {
        static UNMAPPED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&UNMAPPED, || format!(
            "root view at uncommitted title address {guest:#x} bound as null (guest RVA {:#x})",
            crate::survey::title_rva().unwrap_or(0)));
        return Some(0);
    }
    // TuxBox 5c73bee (Apache-2.0, idea): the window is the 64 KiB the shader
    // can read. It stopped at the first region boundary, so committed pages
    // past a protection change were left out of a short import; adjoining
    // committed regions now extend it (TUXBOX_ROOT_SPAN_REGIONS=0 one region).
    let mut end = region.BaseAddress as u64 + region.RegionSize as u64;
    while end < guest.saturating_add(0x10000) && crate::env_toggle!("TUXBOX_ROOT_SPAN_REGIONS") != Some("0") {
        let mut next = MEMORY_BASIC_INFORMATION::default();
        if unsafe { VirtualQuery(Some(end as _), &mut next, size_of_val(&next)) } == 0
            || next.State != MEM_COMMIT
            || next.AllocationBase != region.AllocationBase
        {
            break;
        }
        end = next.BaseAddress as u64 + next.RegionSize as u64;
    }
    walk_cache(guest, Some((region.BaseAddress as u64, end)));
    drop(query);
    let span = end.saturating_sub(guest).min(0x10000).max(1);
    let _p = branches::Phase::new(branches::IMPORT);
    span_import(list, state, guest, span, (region.BaseAddress as u64, end))
}

/// test167: 649 walked binds in 10 s spent 1285 ms importing (2 ms each): an
/// import covered only the bind's own 64 KiB, so the next bind a little below
/// or past it in the same committed span missed buffer_at's reuse and imported
/// again. The import covers the 32 MiB-aligned window of the walked span that
/// holds the bind (up to 64 MiB), so nearby binds reuse it; the bind's address
/// is that import's address plus its offset. TUXBOX_ROOT_SPAN_IMPORT=0
/// imports the bind's 64 KiB alone.
fn span_import(list: &List, state: &mut super::Recording, guest: u64, span: u64, (start, end): (u64, u64)) -> Option<u64> {
    const ALIGN: u64 = 32 << 20;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_SPAN_IMPORT") != Some("0")) {
        return root_import(list, state, guest, span);
    }
    let base = (guest & !(ALIGN - 1)).max(start).max(1 << 32);
    let top = end.min(base.saturating_add(2 * ALIGN));
    if base > guest || top < guest.saturating_add(span) {
        return root_import(list, state, guest, span);
    }
    match unsafe { device::command_buffer_address(parent(&list.parent), base, top - base) } {
        Ok((buffer, va)) if va != base || buffer.is_some() => {
            if let Some(buffer) = buffer {
                super::raytracing::retain(state, &[&buffer]).ok()?;
                ROOT_IMPORT.with(|b| b.replace(Some(buffer)));
            }
            Some(va + (guest - base))
        }
        _ => root_import(list, state, guest, span),
    }
}

/// test165: 200 root view binds in 10 s took 653 ms in the VirtualQuery walk
/// (3.3 ms each: a query scans its whole region under the address-space lock,
/// and these regions are GiB views). A walked committed span (region base to
/// the end it reached) is kept for 1 s; a bind inside it takes that end
/// without querying. TUXBOX_ROOT_WALK_CACHE=0 walks every bind as before.
fn walk_cache(guest: u64, store: Option<(u64, u64)>) -> Option<(u64, u64)> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_WALK_CACHE") != Some("0")) {
        return None;
    }
    static SPANS: Mutex<Vec<(u64, u64, Instant)>> = Mutex::new(Vec::new());
    let mut spans = SPANS.lock().unwrap_or_else(|e| e.into_inner());
    let now = Instant::now();
    spans.retain(|s| now.duration_since(s.2) < Duration::from_secs(1));
    if let Some((start, end)) = store {
        if spans.len() >= 256 {
            spans.remove(0);
        }
        spans.push((start, end, now));
        return None;
    }
    spans.iter().rev().find(|s| s.0 <= guest && guest < s.1).map(|s| (s.0, s.1))
}

/// root_address branch counts and time per phase, logged every 10 s
/// (TUXBOX_ROOT_BRANCHES=0 off): test153 spent ~4 ms/frame at
/// compute.rs:71 (root_buffer), this splits it.
mod branches {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    pub(super) const NULL: usize = 0;
    pub(super) const KNOWN: usize = 1;
    pub(super) const QUERY: usize = 2;
    pub(super) const TOTAL: usize = 0;
    pub(super) const COMMIT: usize = 1;
    pub(super) const STATE: usize = 2;
    pub(super) const WALK: usize = 3;
    pub(super) const IMPORT: usize = 4;
    static COUNTS: [AtomicU64; 3] = [const { AtomicU64::new(0) }; 3];
    static NS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
    static NEXT_MS: AtomicU64 = AtomicU64::new(0);

    fn on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOT_BRANCHES") != Some("0"))
    }

    pub(super) fn count(branch: usize) {
        if on() {
            COUNTS[branch].fetch_add(1, Relaxed);
        }
    }

    pub(super) struct Phase(usize, Option<std::time::Instant>);
    impl Phase {
        pub(super) fn new(phase: usize) -> Self {
            Phase(phase, on().then(std::time::Instant::now))
        }
    }
    impl Drop for Phase {
        fn drop(&mut self) {
            let Some(start) = self.1 else { return };
            NS[self.0].fetch_add(start.elapsed().as_nanos() as u64, Relaxed);
            if self.0 == TOTAL {
                report();
            }
        }
    }

    fn report() {
        static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
        let now = START.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64;
        let next = NEXT_MS.load(Relaxed);
        if now < next || NEXT_MS.compare_exchange(next, now + 10_000, Relaxed, Relaxed).is_err() || next == 0 {
            return;
        }
        let c: Vec<u64> = COUNTS.iter().map(|a| a.swap(0, Relaxed)).collect();
        let ms: Vec<f64> = NS.iter().map(|a| a.swap(0, Relaxed) as f64 / 1e6).collect();
        crate::diagnostics::log_line(format_args!(
            "root views last 10 s: {} binds ({:.1} ms): null/span {}, known view {}, VirtualQuery walk {}; ms in commit_lazy {:.1}, view_state {:.1}, VirtualQuery walk {:.1}, import {:.1}; new buffer imports {} (TUXBOX_ROOT_BRANCHES=0 off)",
            c.iter().sum::<u64>(), ms[TOTAL], c[NULL], c[KNOWN], c[QUERY], ms[COMMIT], ms[STATE], ms[WALK], ms[IMPORT], crate::device::new_buffer_imports()));
    }
}

/// Imports a root view's `span` bytes at `guest` (one byte when that fails).
fn root_import(list: &List, state: &mut super::Recording, guest: u64, span: u64) -> Option<u64> {
    let result = unsafe { device::command_buffer_address(parent(&list.parent), guest, span) }
        .or_else(|_| unsafe { device::command_buffer_address(parent(&list.parent), guest, 1) })
        .and_then(|(buffer, va)| {
            if let Some(buffer) = buffer {
                super::raytracing::retain(state, &[&buffer])?;
                ROOT_IMPORT.with(|b| b.replace(Some(buffer)));
            }
            Ok(va)
        });
    match result {
        Ok(va) => Some(va),
        // Xbox2Windows 109e7dc8 (MIT): an unresolvable bind left the previous
        // root view bound, so the draw read a stale buffer. ea4abef bound the
        // zeroed sink and let the list go on; test104/105 then lost the device
        // to a GPU page fault at VA 0, as a shader that reads a 64-bit address
        // out of a zeroed constant buffer dereferences 0. Now the sink is bound
        // and the list's draws/dispatches drop until that root view is bound
        // again or the root signature changes. TUXBOX_ROOT_STALE_CLEAR=sink
        // draws with the sink (ea4abef); =0 refuses the call, keeping the
        // previous binding.
        Err(e) if crate::env_toggle!("TUXBOX_ROOT_STALE_CLEAR") != Some("0") => {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 4 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "root view at title address {guest:#x} did not resolve ({:#010x}); its draws/dispatches drop until it is bound again (guest RVA {:#x}; {n} so far; TUXBOX_ROOT_STALE_CLEAR=sink draws with zeros, =0 refuses)",
                    e.code().0, crate::survey::title_rva().unwrap_or(0)));
            }
            Some(if crate::env_toggle!("TUXBOX_ROOT_STALE_CLEAR") == Some("sink") { 0 } else { UNRESOLVED })
        }
        Err(e) => { { state.error = e.code(); state.error_at = (file!(), line!()); } None }
    }
}

pub(super) unsafe extern "system" fn constant_buffer<const COMPUTE: bool>(list: &List, index: u32, guest: u64) {
    root_buffer::<COMPUTE>(list, index, guest, BufferView::Cbv);
}

pub(super) unsafe extern "system" fn shader_resource<const COMPUTE: bool>(list: &List, index: u32, guest: u64) {
    root_buffer::<COMPUTE>(list, index, guest, BufferView::Srv);
}

pub(super) unsafe extern "system" fn unordered_access<const COMPUTE: bool>(list: &List, index: u32, guest: u64) {
    root_buffer::<COMPUTE>(list, index, guest, BufferView::Uav);
}

pub(super) unsafe extern "system" fn async_compute_budget(list: &List, budget: *const [u8; 24]) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    // The newer list layout may put RSSetDepthBiasX (floats, no pointer) at
    // this slot, so check the record is readable before reading it.
    if !super::accessible(budget as usize, 24, false) {
        static NOT_POINTER: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&NOT_POINTER, || format!("list slot 96 argument {budget:p} is not a readable record; ignored"));
        return;
    }
    // SAFETY: checked readable above.
    let budget = unsafe { &*budget };
    // SetAsyncComputeBudgetInternal passes six zero DWORDs at RVA 0x5765a97.
    // Host queues keep their own scheduling policy for any budget.
    if *budget != [0; 24] {
        static NONZERO: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&NONZERO, || format!("nonzero asynchronous compute budget {budget:02x?} ignored"));
        return;
    }
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || {
        "accepted default asynchronous compute budget (list slot 96)".into()
    });
}

pub(super) unsafe extern "system" fn dispatch(list: &List, x: u32, y: u32, z: u32) {
    let mut state = recording(list);
    if !state.can_record_shader(&list.native) {
        return;
    }
    if x.max(y).max(z) > D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    }
    if let Some(super::compute_bindings::Pipeline::Regular(pso, ..)) = &state.bindings.pipeline {
        crate::diagnostics::display::null_dispatch(windows::core::Interface::as_raw(pso), [x, y, z]);
    }
    // SAFETY: the native list remains open under its recording lock.
    state.compute_dispatch(list, || unsafe {
        list.native.Dispatch(x, y, z);
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!("recorded Dispatch list={list:p} groups=({x},{y},{z})"));
    });
}

pub(super) fn buffer_offset(resource: &device::Resource, offset: u64, bytes: u64) -> Option<u64> {
    let desc = unsafe { resource.native.GetDesc() };
    let (base, length) = resource.state.buffer_window().unwrap_or((0, desc.Width));
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || offset.checked_add(bytes)? > length {
        return None;
    }
    base.checked_add(offset)
}

pub(super) fn address(list: &List, state: &mut super::Recording, guest: u64, bytes: u64) -> Option<u64> {
    if guest == 0 { return Some(0); }
    let result = unsafe { device::command_buffer_address(parent(&list.parent), guest, bytes.max(1)) }
        .and_then(|(buffer, va)| {
            if let Some(buffer) = buffer { super::raytracing::retain(state, &[&buffer])?; }
            Ok(va)
        });
    match result { Ok(va) => Some(va), Err(e) => { { state.error = e.code(); state.error_at = (file!(), line!()); } None } }
}

pub(super) unsafe extern "system" fn copy_buffer(
    list: &List,
    dst: *mut c_void,
    dst_offset: u64,
    src: *mut c_void,
    src_offset: u64,
    bytes: u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let owner = parent(&list.parent);
    // SAFETY: resource validates each guest proxy and its owning device.
    let (Some(dst), Some(src)) = (unsafe { device::resource_proxy(dst, owner) }, unsafe {
        device::resource_proxy(src, owner)
    }) else {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    };
    let (Some(dst_offset), Some(src_offset)) = (buffer_offset(dst, dst_offset, bytes), buffer_offset(src, src_offset, bytes)) else {
        { state.error = E_INVALIDARG; state.error_at = (file!(), line!()); }
        return;
    };
    unsafe { device::note_shadow_write(owner, windows::core::Interface::as_raw(&dst.native)) };
    // SAFETY: both native buffers belong to the list and contain the requested range.
    unsafe {
        list.native
            .CopyBufferRegion(&dst.native, dst_offset, &src.native, src_offset, bytes)
    };
}
