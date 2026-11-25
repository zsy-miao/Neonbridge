//! Queue tile mapping. Both queue generations share these two slots.
//! A rejected mapping poisons the queue. The next submit or present returns
//! that error instead of drawing with the old tiles.

use super::{Queue, parent};
use crate::device;
use core::{ffi::c_void, slice};
use std::sync::atomic::Ordering;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::Diagnostics::Debug::OutputDebugStringA;
use windows::core::{HRESULT, PCSTR};

fn mapping_flags(flags: D3D12_TILE_MAPPING_FLAGS) -> Result<(), HRESULT> {
    if flags.0 & !D3D12_TILE_MAPPING_FLAG_NO_HAZARD.0 != 0 {
        Err(E_NOTIMPL)
    } else {
        Ok(())
    }
}

fn regions(
    count: u32,
    coordinates: *const D3D12_TILED_RESOURCE_COORDINATE,
    sizes: *const D3D12_TILE_REGION_SIZE,
) -> Result<(), HRESULT> {
    if count == 0 {
        return Err(E_INVALIDARG);
    }
    if count > 1 && (coordinates.is_null() || sizes.is_null()) {
        return Err(E_INVALIDARG);
    }
    Ok(())
}

fn ranges(
    count: u32,
    flags: *const D3D12_TILE_RANGE_FLAGS,
    offsets: *const u32,
    heap: bool,
) -> Result<(), HRESULT> {
    if count == 0 {
        return Err(E_INVALIDARG);
    }
    if flags.is_null() {
        return if heap && !offsets.is_null() { Ok(()) } else { Err(E_INVALIDARG) };
    }
    // SAFETY: the guest supplies one flag per range and keeps the array alive.
    let rows = unsafe { slice::from_raw_parts(flags, count as usize) };
    let mut needs_offset = false;
    for flag in rows {
        if flag.0 & !7 != 0 {
            return Err(E_NOTIMPL);
        }
        if flag.0.count_ones() > 1 {
            return Err(E_INVALIDARG);
        }
        if !heap && flag.0 != D3D12_TILE_RANGE_FLAG_NULL.0 {
            return Err(E_INVALIDARG);
        }
        if flag.0 == D3D12_TILE_RANGE_FLAG_NONE.0
            || flag.0 == D3D12_TILE_RANGE_FLAG_REUSE_SINGLE_TILE.0
        {
            needs_offset = true;
        }
    }
    if needs_offset && offsets.is_null() {
        Err(E_INVALIDARG)
    } else {
        Ok(())
    }
}

fn is_reserved(resource: &ID3D12Resource) -> Result<bool, HRESULT> {
    let mut properties = D3D12_HEAP_PROPERTIES::default();
    match unsafe { resource.GetHeapProperties(Some(core::ptr::from_mut(&mut properties)), None) } {
        Err(error) if error.code() == E_INVALIDARG => Ok(true),
        Ok(()) => Ok(false),
        Err(error) => Err(error.code()),
    }
}

fn poison(queue: &Queue, error: HRESULT, detail: &str) {
    let _ = queue.state.placement.error.compare_exchange(
        HRESULT(0).0,
        error.0,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(|| {
        let message = format!("Project TuxBox: tile mapping failed {detail} ({:#010x})\n\0", error.0);
        // SAFETY: the message is NUL-terminated and lives until the call returns.
        unsafe { OutputDebugStringA(PCSTR(message.as_ptr())) };
    });
}

pub(super) unsafe extern "system" fn update(
    queue: &Queue,
    resource: *mut c_void,
    region_count: u32,
    coordinates: *const D3D12_TILED_RESOURCE_COORDINATE,
    sizes: *const D3D12_TILE_REGION_SIZE,
    heap: *mut c_void,
    range_count: u32,
    range_flags: *const D3D12_TILE_RANGE_FLAGS,
    offsets: *const u32,
    counts: *const u32,
    flags: D3D12_TILE_MAPPING_FLAGS,
) {
    let mut submitted = queue.state.submission.lock().unwrap_or_else(|e| e.into_inner());
    if queue.state.placement.error.load(Ordering::Acquire) < 0 {
        return;
    }
    let owner = parent(&queue.parent);
    let outcome = (|| {
        mapping_flags(flags)?;
        regions(region_count, coordinates, sizes)?;
        ranges(range_count, range_flags, offsets, !heap.is_null())?;
        let resource = unsafe { device::resource_proxy(resource, owner) }.ok_or(E_INVALIDARG)?;
        match is_reserved(&resource.native)? {
            true => {}
            false => return Err(E_INVALIDARG),
        }
        let heap = if heap.is_null() {
            None
        } else {
            Some(unsafe { device::memory_heap(heap, owner) }.ok_or(E_INVALIDARG)?)
        };
        unsafe {
            queue.native.UpdateTileMappings(
                &resource.native,
                region_count,
                (!coordinates.is_null()).then_some(coordinates),
                (!sizes.is_null()).then_some(sizes),
                heap,
                range_count,
                (!range_flags.is_null()).then_some(range_flags),
                (!offsets.is_null()).then_some(offsets),
                (!counts.is_null()).then_some(counts),
                flags,
            );
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        poison(queue, error, "UpdateTileMappings");
        return;
    }
    *submitted = true;
}

pub(super) unsafe extern "system" fn copy(
    queue: &Queue,
    destination: *mut c_void,
    destination_coordinate: *const D3D12_TILED_RESOURCE_COORDINATE,
    source: *mut c_void,
    source_coordinate: *const D3D12_TILED_RESOURCE_COORDINATE,
    size: *const D3D12_TILE_REGION_SIZE,
    flags: D3D12_TILE_MAPPING_FLAGS,
) {
    let mut submitted = queue.state.submission.lock().unwrap_or_else(|e| e.into_inner());
    if queue.state.placement.error.load(Ordering::Acquire) < 0 {
        return;
    }
    let owner = parent(&queue.parent);
    let outcome = (|| {
        mapping_flags(flags)?;
        if destination_coordinate.is_null() || source_coordinate.is_null() || size.is_null() {
            return Err(E_INVALIDARG);
        }
        let destination = unsafe { device::resource_proxy(destination, owner) }.ok_or(E_INVALIDARG)?;
        let source = unsafe { device::resource_proxy(source, owner) }.ok_or(E_INVALIDARG)?;
        if !is_reserved(&destination.native)? || !is_reserved(&source.native)? {
            return Err(E_INVALIDARG);
        }
        unsafe {
            queue.native.CopyTileMappings(
                &destination.native,
                destination_coordinate,
                &source.native,
                source_coordinate,
                size,
                flags,
            );
        }
        Ok(())
    })();
    if let Err(error) = outcome {
        poison(queue, error, "CopyTileMappings");
        return;
    }
    *submitted = true;
}

#[cfg(test)]
#[path = "../tests/commands/tiles.rs"]
mod tests;
