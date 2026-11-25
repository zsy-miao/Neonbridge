//! Standalone GPU-copied validated shadow and fence-guarded retirement control.
//! Not linked into TuxBox or exposed as a title-facing API.
#![allow(dead_code)]
#[path = "../../../blas/validate_traversal.rs"]
mod validate_traversal;

use core::{ffi::c_void, mem::ManuallyDrop, ptr, slice};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_SAMPLE_DESC};
use windows::core::Interface;

struct Shadow {
    buffer: ID3D12Resource,
    upload: ID3D12Resource,
    size: u64,
    fences: Vec<(ID3D12Fence, u64)>,
    recordings: usize,
}

// The control harness serializes access to an opaque handle. Device, fence,
// input span, and output pointer must be valid for each call. It must register
// every GPU use with a private fence before submission and retry release until
// all holds have completed; an untracked use is outside this control's scope.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_create(
    device: *mut c_void,
    data: *const u8,
    len: usize,
    out: *mut *mut c_void,
) -> i32 {
    if out.is_null() {
        return -1;
    }
    unsafe { out.write(ptr::null_mut()) };
    if device.is_null() || data.is_null() || len > 64 * 1024 * 1024 || len < 128 {
        return -1;
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        return -3;
    }
    bytes.extend_from_slice(unsafe { slice::from_raw_parts(data, len) });
    if validate_traversal::validate(&bytes).is_err() {
        return -2;
    }
    let body = &bytes[64..];
    let native = ManuallyDrop::new(unsafe { ID3D12Device::from_raw(device) });
    let mut props = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_UPLOAD,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: body.len() as u64,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let mut buffer = None;
    if unsafe {
        native.CreateCommittedResource(
            &props,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ,
            None,
            &mut buffer,
        )
    }
    .is_err()
    {
        return -3;
    }
    let Some(buffer): Option<ID3D12Resource> = buffer else {
        return -3;
    };
    let mut mapped = ptr::null_mut();
    if unsafe { buffer.GetGPUVirtualAddress() } == 0 {
        return -3;
    }
    if unsafe { buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped)) }.is_err() {
        return -4;
    }
    if mapped.is_null() {
        unsafe { buffer.Unmap(0, Some(&D3D12_RANGE::default())) };
        return -4;
    }
    unsafe {
        ptr::copy_nonoverlapping(body.as_ptr(), mapped.cast::<u8>(), body.len());
        buffer.Unmap(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: body.len(),
            }),
        );
    }
    let upload = buffer;
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    let mut buffer = None;
    if unsafe {
        native.CreateCommittedResource(
            &props,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
            None,
            &mut buffer,
        )
    }
    .is_err()
    {
        return -3;
    }
    let Some(buffer): Option<ID3D12Resource> = buffer else {
        return -3;
    };
    if unsafe { buffer.GetGPUVirtualAddress() } == 0 {
        return -3;
    }
    let result = Box::new(Shadow {
        buffer,
        upload,
        size: body.len() as u64,
        fences: Vec::new(),
        recordings: 0,
    });
    unsafe {
        out.write(Box::into_raw(result).cast());
    }
    0
}

// Replace the staged upload for a later execution of the same recorded list.
// The external submission lock must cover this check and the next submission;
// every prior GPU use must be registered with a private completion fence. A
// different body length is rejected because recorded copies use a fixed size.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_update(handle: *mut c_void, data: *const u8, len: usize) -> i32 {
    if handle.is_null() || data.is_null() {
        return -1;
    }
    let shadow = unsafe { &mut *handle.cast::<Shadow>() };
    if len.checked_sub(64) != Some(shadow.size as usize) {
        return -1;
    }
    if shadow.fences.iter().any(|(fence, value)| {
        let completed = unsafe { fence.GetCompletedValue() };
        completed == u64::MAX || completed < *value
    }) {
        return -5;
    }
    let mut bytes = Vec::new();
    if bytes.try_reserve_exact(len).is_err() {
        return -3;
    }
    bytes.extend_from_slice(unsafe { slice::from_raw_parts(data, len) });
    if validate_traversal::validate(&bytes).is_err() {
        return -2;
    }
    let body = &bytes[64..];
    let mut mapped = ptr::null_mut();
    if unsafe {
        shadow
            .upload
            .Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped))
    }
    .is_err()
    {
        return -4;
    }
    if mapped.is_null() {
        unsafe { shadow.upload.Unmap(0, Some(&D3D12_RANGE::default())) };
        return -4;
    }
    unsafe {
        ptr::copy_nonoverlapping(body.as_ptr(), mapped.cast::<u8>(), body.len());
        shadow.upload.Unmap(
            0,
            Some(&D3D12_RANGE {
                Begin: 0,
                End: body.len(),
            }),
        );
    }
    0
}

// Record a copy from the validated private upload to the default-heap body.
// The caller must serialize writes with all readers, use a list of a compatible
// device/type, and ensure the buffer begins/ends in NON_PIXEL_SHADER_RESOURCE.
// Each replay repeats the GPU copy and both resources must be held until its
// private completion fence signals. No runtime command-list integration exists.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_record_copy(handle: *mut c_void, list: *mut c_void) -> i32 {
    if handle.is_null() || list.is_null() {
        return -1;
    }
    let shadow = unsafe { &mut *handle.cast::<Shadow>() };
    let Some(recordings) = shadow.recordings.checked_add(1) else {
        return -2;
    };
    shadow.recordings = recordings;
    let list = ManuallyDrop::new(unsafe { ID3D12GraphicsCommandList::from_raw(list) });
    for to_copy in [true, false] {
        let mut barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                    pResource: ManuallyDrop::new(Some(shadow.buffer.clone())),
                    Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                    StateBefore: if to_copy {
                        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
                    } else {
                        D3D12_RESOURCE_STATE_COPY_DEST
                    },
                    StateAfter: if to_copy {
                        D3D12_RESOURCE_STATE_COPY_DEST
                    } else {
                        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
                    },
                }),
            },
        };
        unsafe {
            list.ResourceBarrier(slice::from_ref(&barrier));
            ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
            if to_copy {
                list.CopyBufferRegion(&shadow.buffer, 0, &shadow.upload, 0, shadow.size);
            }
        }
    }
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_info(
    handle: *const c_void,
    address: *mut u64,
    size: *mut u64,
) -> i32 {
    if handle.is_null() || address.is_null() || size.is_null() {
        return -1;
    }
    let shadow = unsafe { &*handle.cast::<Shadow>() };
    unsafe {
        address.write(shadow.buffer.GetGPUVirtualAddress());
        size.write(shadow.size);
    }
    0
}

// Borrow the destination and validation-upload resources for bounded GPU
// views, and expose the upload's GPU address. The caller must hold recording
// references and fence leases while commands use either object. Neither
// returned pointer adds a COM reference; callers must not release them.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_gpu_resources(
    handle: *const c_void,
    target: *mut *mut c_void,
    validation: *mut *mut c_void,
    validation_address: *mut u64,
) -> i32 {
    if handle.is_null() || target.is_null() || validation.is_null() || validation_address.is_null()
    {
        return -1;
    }
    let shadow = unsafe { &*handle.cast::<Shadow>() };
    unsafe {
        target.write(shadow.buffer.as_raw());
        validation.write(shadow.upload.as_raw());
        validation_address.write(shadow.upload.GetGPUVirtualAddress());
    }
    0
}

// Keep the resource alive for another command list that records a read. A
// recording reference remains held across all replays, including between
// submissions when every GPU fence is complete.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_retain_recording(handle: *mut c_void) -> i32 {
    if handle.is_null() {
        return -1;
    }
    let shadow = unsafe { &mut *handle.cast::<Shadow>() };
    let Some(recordings) = shadow.recordings.checked_add(1) else {
        return -2;
    };
    shadow.recordings = recordings;
    0
}

// Caller guarantees the associated command list has been reset or destroyed
// and can no longer be replayed. GPU fences still guard already-submitted work.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_release_recording(handle: *mut c_void) -> i32 {
    if handle.is_null() {
        return -1;
    }
    let shadow = unsafe { &mut *handle.cast::<Shadow>() };
    let Some(recordings) = shadow.recordings.checked_sub(1) else {
        return -2;
    };
    shadow.recordings = recordings;
    0
}

// Register before queue execution. Handle operations and submission must be
// externally serialized. The caller must use private, monotonic completion
// fences whose values cannot be signaled or rewound by the guest.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_hold(
    handle: *mut c_void,
    fence: *mut c_void,
    value: u64,
) -> i32 {
    if handle.is_null() || fence.is_null() || value == 0 || value == u64::MAX {
        return -1;
    }
    let shadow = unsafe { &mut *handle.cast::<Shadow>() };
    if shadow.fences.try_reserve(1).is_err() {
        return -2;
    }
    let fence = ManuallyDrop::new(unsafe { ID3D12Fence::from_raw(fence) });
    shadow.fences.push(((*fence).clone(), value));
    0
}

// On incomplete or device-removed fences keep ownership and report pending.
// A successful result consumes the handle; it must not be used again.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn tb_shadow_try_release(handle: *mut c_void) -> i32 {
    if handle.is_null() {
        return -1;
    }
    let shadow = unsafe { &*handle.cast::<Shadow>() };
    if shadow.recordings != 0
        || shadow.fences.iter().any(|(fence, value)| {
            let completed = unsafe { fence.GetCompletedValue() };
            completed == u64::MAX || completed < *value
        })
    {
        return 0;
    }
    unsafe {
        drop(Box::from_raw(handle.cast::<Shadow>()));
    }
    1
}
