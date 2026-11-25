//! Frosty reads a 16-byte result and hashes each 56-byte resource description.
//! Slot 25 passes the result in RCX and the receiver in RDX (title RVA 0x57945e9).
use super::Device;
use core::slice;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B5G5R5A1_UNORM, DXGI_FORMAT_R16_UINT,
};
use windows::core::Interface;

mod trace;

const _: () = assert!(
    size_of::<D3D12_RESOURCE_DESC>() == 56 && size_of::<D3D12_RESOURCE_ALLOCATION_INFO>() == 16
);

// DCC is selected by the console. The host driver chooses compression for its
// own image, so allocation and copy-footprint queries must match image creation.
pub(super) const DCC_FLAGS: i32 = 0x00c0_0000;

fn host_description(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    use super::format::HOST_FLAGS;
    // Console-only flag bits (including DCC) are dropped; the host driver
    // chooses its own compression for the image it creates.
    super::depth::primary(desc).or_else(|| {
        (desc.Flags.0 & !HOST_FLAGS != 0)
            .then(|| D3D12_RESOURCE_DESC { Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & HOST_FLAGS), ..*desc })
    })
}

// Preserve the packed render/sampling format. A raw 16-bit storage view can
// share those exact bits even on adapters without typed storage for BGRA5551.
pub(super) fn packed_uav_formats(format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> &'static [DXGI_FORMAT] {
    if format == DXGI_FORMAT_B5G5R5A1_UNORM && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
        &[DXGI_FORMAT_R16_UINT]
    } else {
        &[]
    }
}

pub(super) fn packed_requirements(
    device: &ID3D12Device, visible: u32, desc: &D3D12_RESOURCE_DESC1,
) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    let formats = packed_uav_formats(desc.Format, desc.Flags);
    if formats.is_empty() { return None; }
    let native = D3D12_RESOURCE_DESC1 { Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & 0x1ff), ..*desc };
    let count = formats.len() as u32;
    let entries = formats.as_ptr();
    let api: ID3D12Device12 = device.cast().ok()?;
    Some(unsafe { api.GetResourceAllocationInfo3(visible, 1, &native, Some(&count), Some(&entries), None) })
}

// Like GetResourceAllocationInfo, this Xbox vtable entry places the struct
// return pointer before the device. GTA queries default, upload and readback.
pub(super) unsafe extern "system" fn custom_heap_properties(
    out: *mut D3D12_HEAP_PROPERTIES,
    device: &Device,
    node_mask: u32,
    heap_type: D3D12_HEAP_TYPE,
) -> *mut D3D12_HEAP_PROPERTIES {
    if !out.is_null() {
        unsafe { out.write(device.native.GetCustomHeapProperties(node_mask, heap_type)) };
    }
    out
}

pub(super) unsafe extern "system" fn info(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let result = unsafe { info_impl(out, device, visible, count, descs) };
    if trace::enabled() && !result.is_null() && !descs.is_null() && count != 0 && visible <= 1 {
        let answer = unsafe { &*result };
        if trace::matches(answer.SizeInBytes) {
            let items = unsafe { slice::from_raw_parts(descs, count as usize) };
            trace::report("info", items, answer);
        }
    }
    result
}

unsafe fn info_impl(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return out;
    };
    let invalid = D3D12_RESOURCE_ALLOCATION_INFO {
        SizeInBytes: u64::MAX,
        Alignment: 0,
    };
    if descs.is_null() || count == 0 || visible > 1 {
        *out = invalid;
        return out;
    }
    let descs = unsafe { slice::from_raw_parts(descs, count as usize) };
    if descs.iter().any(|d| super::placement::console_layout(d.Layout.0)
        || !packed_uav_formats(d.Format, d.Flags).is_empty()) {
        // The guest will allocate these bytes itself and pass the address to
        // CreatePlacedResourceX. Account for the Xbox planes and batch padding.
        let mut bytes = 0u64;
        let mut alignment = 256;
        for desc in descs {
            let layout = if let Some(pitch) = super::placement::nv12_pitch(desc) {
                let y = pitch * u64::from(desc.Height);
                D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: y + y / 2, Alignment: 256 }
            } else if super::placement::console_layout(desc.Layout.0) {
                match super::placement::color_plane(desc) {
                    Ok(plane) => D3D12_RESOURCE_ALLOCATION_INFO {
                        SizeInBytes: plane.size,
                        Alignment: plane.alignment.max(desc.Alignment),
                    },
                    Err(_) => invalid,
                }
            } else if !packed_uav_formats(desc.Format, desc.Flags).is_empty() {
                let native = D3D12_RESOURCE_DESC1 {
                    Dimension: desc.Dimension, Alignment: desc.Alignment, Width: desc.Width, Height: desc.Height,
                    DepthOrArraySize: desc.DepthOrArraySize, MipLevels: desc.MipLevels, Format: desc.Format,
                    SampleDesc: desc.SampleDesc, Layout: desc.Layout, Flags: desc.Flags, ..Default::default()
                };
                packed_requirements(&device.native, visible, &native).unwrap_or(invalid)
            } else {
                let mut item = invalid;
                unsafe { info(&mut item, device, visible, 1, desc) };
                item
            };
            if layout.Alignment == 0 || layout.SizeInBytes == u64::MAX {
                static BAD_LAYOUT: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&BAD_LAYOUT, || format!("Xbox layout {:#x} dim={} {}x{}x{} format={} flags={:#x} alignment={} mips={} samples={}", desc.Layout.0, desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Flags.0, desc.Alignment, desc.MipLevels, desc.SampleDesc.Count));
                *out = invalid;
                return out;
            }
            let Some(end) = bytes.checked_next_multiple_of(layout.Alignment)
                .and_then(|offset| offset.checked_add(layout.SizeInBytes)) else {
                *out = invalid;
                return out;
            };
            bytes = end;
            alignment = alignment.max(layout.Alignment);
        }
        *out = D3D12_RESOURCE_ALLOCATION_INFO {
            SizeInBytes: bytes.checked_next_multiple_of(alignment).unwrap_or(u64::MAX), Alignment: alignment,
        };
        return out;
    }
    let supported = descs.iter().all(|d| {
        matches!(
            d.Dimension,
            D3D12_RESOURCE_DIMENSION_BUFFER
                | D3D12_RESOURCE_DIMENSION_TEXTURE1D
                | D3D12_RESOURCE_DIMENSION_TEXTURE2D
                | D3D12_RESOURCE_DIMENSION_TEXTURE3D
        ) && matches!(
            d.Layout,
            D3D12_TEXTURE_LAYOUT_UNKNOWN | D3D12_TEXTURE_LAYOUT_ROW_MAJOR
        ) && d.Width != 0
            && d.Height != 0
            && d.DepthOrArraySize != 0
            && d.SampleDesc.Count != 0
    });
    *out = if supported {
        // Let the same adapter that creates the placed resource determine its
        // requirements, including padding between descriptors in a batch.
        if descs.iter().any(|d| host_description(d).is_some()) {
            let mut host = Vec::new();
            if host.try_reserve_exact(descs.len()).is_err() {
                *out = invalid;
                return out;
            }
            host.extend(descs.iter().map(|d| host_description(d).unwrap_or(*d)));
            unsafe { device.native.GetResourceAllocationInfo(visible, &host) }
        } else {
            unsafe { device.native.GetResourceAllocationInfo(visible, descs) }
        }
    } else {
        invalid
    };
    if out.SizeInBytes == u64::MAX {
        let d = descs[0];
        static INVALID: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&INVALID, || format!("allocation requires dim={} {}x{}x{} mips={} format={} layout={:#x} flags={:#x} align={}", d.Dimension.0,d.Width,d.Height,d.DepthOrArraySize,d.MipLevels,d.Format.0,d.Layout.0,d.Flags.0,d.Alignment));
    }
    out
}

// Device8 extends each description with a sampler-feedback mip region and may
// return a placement offset, alignment and size for each individual resource.
pub(super) unsafe extern "system" fn info2(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC1,
    details: *mut D3D12_RESOURCE_ALLOCATION_INFO1,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let Some(result) = (unsafe { out.as_mut() }) else { return out };
    let invalid = D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: u64::MAX, Alignment: 0 };
    *result = invalid;
    if descs.is_null() || count == 0 || visible > 1 { return out; }
    let inputs = unsafe { slice::from_raw_parts(descs, count as usize) };
    let mut offset = 0u64;
    let mut alignment = 1;
    let mut placements = Vec::new();
    if !details.is_null() && placements.try_reserve_exact(inputs.len()).is_err() { return out; }
    for d in inputs {
        let requirement = if d.Layout.0 >= 0x100 {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height,
                DepthOrArraySize: d.DepthOrArraySize, MipLevels: d.MipLevels, Format: d.Format,
                SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
            };
            let mut item = invalid;
            unsafe { info(&mut item, device, visible, 1, &desc) };
            item
        } else if let Some(requirement) = packed_requirements(&device.native, visible, d) {
            requirement
        } else if let Ok(host) = device.native.cast::<ID3D12Device8>() {
            // Image creation keeps only the desktop bits, DCC included, so the
            // requirements must be queried with the same flags.
            let mut native = *d;
            native.Flags.0 &= super::format::HOST_FLAGS;
            let guest = D3D12_RESOURCE_DESC {
                Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height,
                DepthOrArraySize: d.DepthOrArraySize, MipLevels: d.MipLevels, Format: d.Format,
                SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
            };
            if let Some(primary) = super::depth::primary(&guest) {
                native.Format = primary.Format;
                native.Flags = primary.Flags;
            }
            unsafe { host.GetResourceAllocationInfo2(visible, 1, &native, None) }
        } else { return out };
        if trace::matches(requirement.SizeInBytes) {
            let guest = D3D12_RESOURCE_DESC {
                Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height,
                DepthOrArraySize: d.DepthOrArraySize, MipLevels: d.MipLevels, Format: d.Format,
                SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
            };
            trace::report("info2-item", core::slice::from_ref(&guest), &requirement);
        }
        if requirement.Alignment == 0 || requirement.SizeInBytes == u64::MAX {
            crate::diagnostics::recording_error(format_args!("allocation info2 dim={} {}x{} format={} flags={:#x} layout={:#x} alignment={}", d.Dimension.0, d.Width, d.Height, d.Format.0, d.Flags.0, d.Layout.0, d.Alignment));
            return out;
        }
        let Some(start) = offset.checked_next_multiple_of(requirement.Alignment) else { return out };
        let Some(end) = start.checked_add(requirement.SizeInBytes) else { return out };
        if !details.is_null() {
            placements.push(D3D12_RESOURCE_ALLOCATION_INFO1 {
                Offset: start, Alignment: requirement.Alignment, SizeInBytes: requirement.SizeInBytes,
            });
        }
        offset = end;
        alignment = alignment.max(requirement.Alignment);
    }
    let Some(total) = offset.checked_next_multiple_of(alignment) else { return out };
    *result = D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: total, Alignment: alignment };
    if !placements.is_empty() {
        unsafe { core::ptr::copy_nonoverlapping(placements.as_ptr(), details, placements.len()) };
    }
    out
}

// The shipping texture upload routine (RVA 0x57e12a1) supplies independent
// arrays with strides 32, 4 and 8 for layout, row count and unpadded row size.
pub(super) unsafe extern "system" fn footprints(
    device: &Device,
    desc: *const D3D12_RESOURCE_DESC,
    first: u32,
    count: u32,
    base: u64,
    layouts: *mut D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    rows: *mut u32,
    row_bytes: *mut u64,
    total: *mut u64,
) {
    const _: () = assert!(size_of::<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>() == 32);
    let native = unsafe { desc.as_ref() }.and_then(host_description);
    let desc = native.as_ref().map_or(desc, core::ptr::from_ref);
    // All four output arrays are optional. Their storage is owned by the guest.
    unsafe {
        (device.native.vtable().GetCopyableFootprints)(
            device.native.as_raw(),
            desc,
            first,
            count,
            base,
            layouts,
            rows,
            row_bytes,
            total,
        );
    }
}
