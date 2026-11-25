//! Placing resources at guest virtual addresses.
use super::*;

pub(in super::super) unsafe extern "system" fn place_virtual(
    device: &Device,
    address: u64,
    desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if address == 0 {
        return E_INVALIDARG;
    }
    if let Err(error) = GPU_APERTURE.wait(address, 1) { return error.code(); }
    let state = super::super::host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER);
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER && clear.is_null() {
        let native = import_or_back(device, address, desc, state, "place_virtual");
        return unsafe {
            wrap_pageable(native, &RESOURCE, device,
                super::super::ResourceState::from(super::super::Backing::ConsoleLayout(desc.Layout, desc.Flags)), out)
        };
    }
    if clear.is_null()
        && let Some(result) = unsafe { super::super::media::place(device, address, desc, state, out) }
    {
        return result;
    }
    let result = device
        .state
        .placements
        .texture(&device.native, address, desc, state, unsafe { clear.as_ref() });
    match result {
        Ok(texture) => {
            let native = texture.native.clone();
            return unsafe {
                wrap_pageable(
                    Ok(native),
                    &RESOURCE,
                    device,
                    super::super::ResourceState::from(super::super::Backing::Placed(texture)),
                    out,
                )
            };
        }
        Err(error) if error.code() != E_NOTIMPL => return error.code(),
        Err(_) => (),
    }
    static REPORTED: std::sync::Once = std::sync::Once::new();
    REPORTED.call_once(|| {
        let message = format!("Project TuxBox: unsupported CreatePlacedResourceX at {address:#x}, dim={} {}x{}x{} mips={} format={} layout={:#x} flags={:#x} state={:#x}\n\0",
            desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.MipLevels, desc.Format.0,
            desc.Layout.0, desc.Flags.0, initial.0);
        unsafe { windows::Win32::System::Diagnostics::Debug::OutputDebugStringA(windows::core::PCSTR(message.as_ptr())) };
    });
    E_NOTIMPL
}

// The layout-based Xbox placement API uses UNDEFINED for a buffer. On the
// legacy host buffer path this starts in COMMON, independent of any texture layout.
pub(in super::super) unsafe extern "system" fn place_virtual_layout(
    device: &Device, address: u64, desc: *const D3D12_RESOURCE_DESC,
    layout: D3D12_BARRIER_LAYOUT, clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID, out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() { return status; }
    let Some(desc) = (unsafe { desc.as_ref() }) else { return E_INVALIDARG };
    if let Err(error) = GPU_APERTURE.wait(address, 1) { return error.code(); }
    if address != 0 && desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER
        && super::super::placement::console_layout(desc.Layout.0) {
        // Placed CPU-visible images promote from COMMON. The uploader handles
        // the guest's pitch or swizzle before a queue reads the texture.
        let state = match layout {
            // UNDEFINED contents need no particular starting layout.
            D3D12_BARRIER_LAYOUT_UNDEFINED | D3D12_BARRIER_LAYOUT_COMMON | D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COMMON | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_UNORDERED_ACCESS
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COMMON | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_UNORDERED_ACCESS => D3D12_RESOURCE_STATE_COMMON,
            D3D12_BARRIER_LAYOUT_SHADER_RESOURCE | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_SHADER_RESOURCE
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_SHADER_RESOURCE => D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE,
            D3D12_BARRIER_LAYOUT_GENERIC_READ | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_GENERIC_READ
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_GENERIC_READ => D3D12_RESOURCE_STATE_ALL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_BARRIER_LAYOUT_RENDER_TARGET => D3D12_RESOURCE_STATE_RENDER_TARGET,
            D3D12_BARRIER_LAYOUT_COPY_SOURCE | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_SOURCE
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_SOURCE => D3D12_RESOURCE_STATE_COPY_SOURCE,
            D3D12_BARRIER_LAYOUT_COPY_DEST | D3D12_BARRIER_LAYOUT_DIRECT_QUEUE_COPY_DEST
            | D3D12_BARRIER_LAYOUT_COMPUTE_QUEUE_COPY_DEST => D3D12_RESOURCE_STATE_COPY_DEST,
            _ => return E_NOTIMPL,
        };
        let texture = match device.state.placements.texture(&device.native, address, desc, state, unsafe { clear.as_ref() }) {
            Ok(texture) => texture,
            Err(error) => return error.code(),
        };
        return unsafe { wrap_pageable(Ok(texture.native.clone()), &RESOURCE, device,
            super::super::ResourceState::from(super::super::Backing::Placed(texture)), out) };
    }
    if address != 0 && desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER && desc.Layout == D3D12_TEXTURE_LAYOUT_UNKNOWN {
        return unsafe { place_gpu_target(device, address, desc, layout, clear, out) };
    }
    if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || !clear.is_null()
        || !matches!(layout, D3D12_BARRIER_LAYOUT_UNDEFINED | D3D12_BARRIER_LAYOUT_COMMON)
        || address == 0 {
        static UNKNOWN_PLACEMENT: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&UNKNOWN_PLACEMENT, || format!("placed resource dim={} {}x{} format={} flags={:#x} layout={:#x} initial={} clear={}", desc.Dimension.0, desc.Width, desc.Height, desc.Format.0, desc.Flags.0, desc.Layout.0, layout.0, !clear.is_null()));
        return E_NOTIMPL;
    }
    let mut host = *desc;
    host.Flags = D3D12_RESOURCE_FLAGS(host.Flags.0 & !0x2_0000);
    // A Series buffer can start between native placement boundaries. Bind a
    // slightly larger host buffer to the same pages and translate offsets at
    // every resource/view entry point. The guest still sees its original VA.
    let probe = D3D12_RESOURCE_DESC {
        Flags: host.Flags | D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER | D3D12_RESOURCE_FLAGS(0x400), ..host
    };
    let (base, delta) = if !address.is_multiple_of(65536) {
        let info = unsafe { device.native.GetResourceAllocationInfo(0, &[probe]) };
        let alignment = if info.SizeInBytes != u64::MAX && info.Alignment.is_power_of_two() {
            info.Alignment
        } else { 65536 };
        let delta = address % alignment;
        (address - delta, delta)
    } else { (address, 0) };
    let Some(width) = host.Width.checked_add(delta) else { return E_INVALIDARG };
    host.Width = width;
    let (native, heap, delta) = match import_with_commit(device, base, &host, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout") {
        Ok((native, heap)) => (native, heap, delta),
        Err(error) => {
            // If the widened import is unavailable, keep the old direct
            // import as a fallback. A real import must never be registered
            // as the zero-address device-local stand-in.
            if base != address || host.Flags != desc.Flags {
                if let Ok((native, heap)) = import_with_commit(device, address, desc, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout") {
                    let buffer = super::super::views::TextureAddress::with_heap(
                        device, &native, address as usize, false, None, None, None, 0, Some(heap),
                    );
                    return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                        super::super::ResourceState::from(super::super::Backing::Addressed(buffer, *desc)), out) };
                }
            }
            let native = match buffer_standin(device, address, desc, D3D12_RESOURCE_STATE_COMMON, "place_virtual_layout", error.clone()) {
                Ok(native) => native,
                Err(fallback) => {
                    crate::diagnostics::log(&format!(
                        "placed buffer at {address:#x} bytes={} native offset={delta:#x}: import {:?} stand-in {:?}",
                        desc.Width, error.code(), fallback.code()));
                    return fallback.code();
                }
            };
            let buffer = super::super::views::TextureAddress::at(device, &native, 0, false, None);
            return unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
                super::super::ResourceState::from(super::super::Backing::Addressed(buffer, *desc)), out) };
        }
    };
    let buffer = super::super::views::TextureAddress::with_heap(
        device, &native, address as usize, false, None, None, None, delta, Some(heap),
    );
    unsafe { wrap_pageable(Ok(native), &RESOURCE, device,
        super::super::ResourceState::from(super::super::Backing::Addressed(buffer, *desc)), out) }
}
