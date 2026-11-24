//! Xbox graphics-list query slots 28–30 and queue clock slots 16–17.
use super::{List, Queue, parent, recording};
use crate::device::{queries::range, resource_proxy};
use core::ffi::c_void;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{HRESULT, Interface};

pub(super) unsafe extern "system" fn query<const BEGIN: bool>(
    list: &List,
    object: *mut c_void,
    kind: D3D12_QUERY_TYPE,
    index: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let heap = unsafe { range(object, parent(&list.parent), kind, index, 1) };
    let Some((heap, _)) = heap else {
        state.error = E_INVALIDARG;
        return;
    };
    // Occlusion and stream-output queries exist only on direct lists, and a
    // copy list takes timestamps alone; the core runtime refuses the list.
    let list_kind = unsafe { list.native.GetType() };
    let supported = match list_kind {
        D3D12_COMMAND_LIST_TYPE_DIRECT => true,
        D3D12_COMMAND_LIST_TYPE_COMPUTE => matches!(kind, D3D12_QUERY_TYPE_TIMESTAMP | D3D12_QUERY_TYPE_PIPELINE_STATISTICS),
        _ => kind == D3D12_QUERY_TYPE_TIMESTAMP,
    };
    if !supported {
        static DROPPED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&DROPPED, || format!(
            "dropped query type {} on a type {} list", kind.0, list_kind.0));
        return;
    }
    // The core runtime refuses a nested Begin or an End without a Begin.
    let open = state.open_queries.iter().position(|(h, k, i)| h == heap && *k == kind && *i == index);
    if BEGIN {
        if kind == D3D12_QUERY_TYPE_TIMESTAMP {
            state.error = E_INVALIDARG;
            return;
        }
        if open.is_some() {
            return;
        }
        if state.open_queries.try_reserve(1).is_err() {
            state.error = windows::Win32::Foundation::E_OUTOFMEMORY;
            return;
        }
        unsafe { list.native.BeginQuery(heap, kind, index) };
        state.open_queries.push((heap.clone(), kind, index));
    } else if kind == D3D12_QUERY_TYPE_TIMESTAMP {
        unsafe { list.native.EndQuery(heap, kind, index) };
    } else if let Some(at) = open {
        state.open_queries.swap_remove(at);
        unsafe { list.native.EndQuery(heap, kind, index) };
    } else {
        static UNMATCHED: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&UNMATCHED, || format!("dropped EndQuery type {} index {index} with no Begin in this list", kind.0));
    }
}

pub(super) unsafe extern "system" fn resolve(
    list: &List,
    object: *mut c_void,
    kind: D3D12_QUERY_TYPE,
    start: u32,
    count: u32,
    destination: *mut c_void,
    offset: u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let owner = parent(&list.parent);
    let heap = unsafe { range(object, owner, kind, start, count) };
    let dst = unsafe { resource_proxy(destination, owner) };
    let (Some((heap, stride)), Some(dst)) = (heap, dst) else {
        state.error = E_INVALIDARG;
        return;
    };
    unsafe { crate::device::note_shadow_write(owner, windows::core::Interface::as_raw(&dst.native)) };
    let Some(offset) = super::compute::buffer_offset(dst, offset, u64::from(count) * stride) else {
        state.error = E_INVALIDARG;
        return;
    };
    if !offset.is_multiple_of(8) { state.error = E_INVALIDARG; return; }
    unsafe {
        list.native
            .ResolveQueryData(heap, kind, start, count, &dst.native, offset)
    };
}

// A Series timestamp packet writes the queue clock directly to title memory.
// Desktop D3D12 exposes the same clock through a timestamp query and resolve.
// Write64BitValueBottomOfPipeX(address, value, flush, flags): flags bit 0 or
// 1 writes the GPU clock, otherwise the 64-bit value (leonida-ce, rebox).
pub(super) unsafe extern "system" fn timestamp_at(list: &List, address: u64, value: u64, _flush: u32, mode: u32) {
    const SLOTS: u32 = 256;
    let mut state = recording(list);
    if !state.ensure_open(&list.native) { return; }
    let result = (|| -> windows::core::Result<()> {
        if address == 0 || !address.is_multiple_of(4) { return Err(E_INVALIDARG.into()); }
        if mode & 3 == 0 && super::xargs_strict() { return Err(E_INVALIDARG.into()); }
        if mode & 3 == 0 {
            super::memory_wait::note_write(list, &mut state, "64-bit value write", address, value);
            let (resource, gpu) = unsafe { crate::device::command_buffer_address(parent(&list.parent), address, 8)? };
            if let Some(buffer) = &resource { super::raytracing::retain(&state, &[buffer])?; }
            for (dest, word) in [(gpu, value as u32), (gpu + 4, (value >> 32) as u32)] {
                let write = D3D12_WRITEBUFFERIMMEDIATE_PARAMETER { Dest: dest, Value: word };
                state.prefix.push(&list.native, super::memory_wait::Packet::Marker(write))?;
            }
            return Ok(());
        }
        // A timestamp to memory that is not GPU memory (or not 8-byte aligned)
        // was refused, which poisoned the list; it is skipped and logged.
        let (resource, gpu) = if address.is_multiple_of(8) {
            unsafe { crate::device::command_buffer_address(parent(&list.parent), address, 8)? }
        } else { (None, 0) };
        let Some(resource) = resource else {
            static SKIPPED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            if SKIPPED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 4 {
                crate::diagnostics::log_line(format_args!("timestamp write to {address:#x} (mode {mode:#x}) is not GPU memory; skipped"));
            }
            return Ok(());
        };
        let offset = gpu.checked_sub(unsafe { resource.GetGPUVirtualAddress() }).ok_or(E_INVALIDARG)?;
        super::raytracing::retain(&state, &[&resource])?;
        if state.timestamp_page.as_ref().is_none_or(|(_, next)| *next >= SLOTS) {
            let mut device: Option<ID3D12Device> = None;
            unsafe { list.native.GetDevice(&mut device)? };
            let mut heap: Option<ID3D12QueryHeap> = None;
            unsafe { device.ok_or(E_INVALIDARG)?.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC {
                Type: if list.native.GetType() == D3D12_COMMAND_LIST_TYPE_COPY {
                    // A copy list ends timestamps only in a copy-queue heap.
                    D3D12_QUERY_HEAP_TYPE_COPY_QUEUE_TIMESTAMP
                } else {
                    D3D12_QUERY_HEAP_TYPE_TIMESTAMP
                },
                Count: SLOTS, NodeMask: 0,
            }, &mut heap)? };
            let heap = heap.ok_or(E_INVALIDARG)?;
            let mut keep = state.resources.lock().unwrap_or_else(|e| e.into_inner());
            keep.try_reserve(1).map_err(|_| windows::Win32::Foundation::E_OUTOFMEMORY)?;
            keep.push(heap.clone().into());
            drop(keep);
            state.timestamp_page = Some((heap, 0));
        }
        let (heap, index) = state.timestamp_page.as_mut().unwrap();
        let packet = super::memory_wait::Packet::Timestamp(heap.clone(), *index, resource, offset);
        *index += 1;
        state.prefix.push(&list.native, packet)
    })();
    if let Err(error) = result { state.error = error.code(); }
}

pub(super) unsafe extern "system" fn frequency(queue: &Queue, hz: *mut u64) -> HRESULT {
    if hz.is_null() {
        return E_INVALIDARG;
    }
    let host = &queue.native;
    unsafe { (host.vtable().GetTimestampFrequency)(host.as_raw(), hz) }
}

pub(super) unsafe extern "system" fn calibrate(
    queue: &Queue,
    gpu: *mut u64,
    cpu: *mut u64,
) -> HRESULT {
    if gpu.is_null() || cpu.is_null() {
        return E_INVALIDARG;
    }
    let host = &queue.native;
    unsafe { (host.vtable().GetClockCalibration)(host.as_raw(), gpu, cpu) }
}
