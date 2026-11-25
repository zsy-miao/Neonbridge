//! Frosty supplies { type, count, node mask } at device slot 39.
use super::{Device, Out, check_output, wrap_pageable};
use crate::proxy::{Proxy, Slots, child_slots, unwrap};
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT};

type Heap = Proxy<ID3D12QueryHeap, D3D12_QUERY_HEAP_DESC>;
static HEAP: Slots<8> = child_slots::<ID3D12QueryHeap, D3D12_QUERY_HEAP_DESC, 8>();

pub(super) unsafe extern "system" fn create_heap(
    device: &Device,
    desc: *const D3D12_QUERY_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12QueryHeap>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.Count == 0 {
        return E_INVALIDARG;
    }
    if desc.NodeMask > 1
        || !matches!(
            desc.Type,
            D3D12_QUERY_HEAP_TYPE_TIMESTAMP
                | D3D12_QUERY_HEAP_TYPE_OCCLUSION
                | D3D12_QUERY_HEAP_TYPE_PIPELINE_STATISTICS
        )
    {
        return E_NOTIMPL;
    }
    let mut heap: Option<ID3D12QueryHeap> = None;
    if let Err(error) = unsafe { device.native.CreateQueryHeap(desc, &mut heap) } {
        return error.code();
    }
    unsafe {
        wrap_pageable(
            heap.ok_or_else(|| E_POINTER.into()),
            &HEAP,
            device,
            *desc,
            out,
        )
    }
}

pub(super) unsafe fn native<'a>(
    object: *mut c_void,
    device: *mut c_void,
) -> Option<&'a ID3D12QueryHeap> {
    let heap: &Heap = unsafe { unwrap(object, device, &HEAP) }?;
    Some(&heap.native)
}

/// Returns the native heap and result stride after checking owner, type and range.
pub(crate) unsafe fn range<'a>(
    object: *mut c_void,
    device: *mut c_void,
    kind: D3D12_QUERY_TYPE,
    start: u32,
    count: u32,
) -> Option<(&'a ID3D12QueryHeap, u64)> {
    let heap: &Heap = unsafe { unwrap(object, device, &HEAP) }?;
    if count == 0 || start.checked_add(count)? > heap.state.Count {
        return None;
    }
    let stride = match (heap.state.Type, kind) {
        (D3D12_QUERY_HEAP_TYPE_TIMESTAMP, D3D12_QUERY_TYPE_TIMESTAMP)
        | (
            D3D12_QUERY_HEAP_TYPE_OCCLUSION,
            D3D12_QUERY_TYPE_OCCLUSION | D3D12_QUERY_TYPE_BINARY_OCCLUSION,
        ) => 8,
        (D3D12_QUERY_HEAP_TYPE_PIPELINE_STATISTICS, D3D12_QUERY_TYPE_PIPELINE_STATISTICS) => {
            size_of::<D3D12_QUERY_DATA_PIPELINE_STATISTICS>() as u64
        }
        _ => return None,
    };
    Some((&heap.native, stride))
}
