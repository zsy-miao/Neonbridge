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
    if BEGIN {
        if kind == D3D12_QUERY_TYPE_TIMESTAMP {
            state.error = E_INVALIDARG;
            return;
        }
        unsafe { list.native.BeginQuery(heap, kind, index) };
    } else {
        unsafe { list.native.EndQuery(heap, kind, index) };
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
pub(super) unsafe extern "system" fn timestamp_at(list: &List, address: u64, arg: u32, flags: u32, mode: u32) {
    const SLOTS: u32 = 256;
    let mut state = recording(list);
    if !state.ensure_open(&list.native) { return; }
    let result = (|| -> windows::core::Result<()> {
        if arg != 0 || flags != 0 || mode != 1 || address == 0 || !address.is_multiple_of(8) { return Err(E_INVALIDARG.into()); }
        let (resource, gpu) = unsafe { crate::device::command_buffer_address(parent(&list.parent), address, 8)? };
        let resource = resource.ok_or(E_INVALIDARG)?;
        let offset = gpu.checked_sub(unsafe { resource.GetGPUVirtualAddress() }).ok_or(E_INVALIDARG)?;
        super::raytracing::retain(&state, &[&resource])?;
        if state.timestamp_page.as_ref().is_none_or(|(_, next)| *next >= SLOTS) {
            let mut device: Option<ID3D12Device> = None;
            unsafe { list.native.GetDevice(&mut device)? };
            let mut heap: Option<ID3D12QueryHeap> = None;
            unsafe { device.ok_or(E_INVALIDARG)?.CreateQueryHeap(&D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP, Count: SLOTS, NodeMask: 0,
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
