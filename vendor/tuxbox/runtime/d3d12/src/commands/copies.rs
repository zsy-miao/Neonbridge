//! Texture and resource copies.
use super::*;

pub(super) unsafe fn copy_location<'a>(
    location: *const CopyLocation,
    device: *mut c_void,
) -> Option<(CopyLocation, Option<&'a Arc<Texture>>)> {
    let location = unsafe { location.as_ref() }?;
    if !matches!(
        location.kind,
        D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX | D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT
    ) {
        return None;
    }
    let resource = unsafe { device::resource_proxy(location.resource, device) }?;
    let mut host = CopyLocation { resource: resource.native.as_raw(), ..*location };
    if let Some((base, length)) = resource.state.buffer_window() {
        if host.kind != D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT { return None; }
        let placed = unsafe { &mut host.data.PlacedFootprint };
        let f = &placed.Footprint;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: if f.Depth > 1 { D3D12_RESOURCE_DIMENSION_TEXTURE3D } else { D3D12_RESOURCE_DIMENSION_TEXTURE2D },
            Width: f.Width.into(), Height: f.Height, DepthOrArraySize: f.Depth.try_into().ok()?,
            MipLevels: 1, Format: f.Format, SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        let mut owner: Option<ID3D12Device> = None;
        let (mut rows, mut row_bytes) = (0, 0);
        unsafe {
            resource.native.GetDevice(&mut owner).ok()?;
            owner?.GetCopyableFootprints(&desc, 0, 1, 0, None, Some(&mut rows), Some(&mut row_bytes), None);
        }
        let pitch = u64::from(f.RowPitch);
        let lines = u64::from(rows).checked_mul(u64::from(f.Depth))?;
        let bytes = lines.checked_sub(1)?.checked_mul(pitch)?.checked_add(row_bytes)?;
        if row_bytes > pitch || placed.Offset.checked_add(bytes)? > length { return None; }
        placed.Offset = placed.Offset.checked_add(base)?;
    }
    Some((host, resource.state.placed()))
}

pub(super) unsafe extern "system" fn copy_texture(
    list: &List,
    dst: *const CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const CopyLocation,
    region: *const D3D12_BOX,
    flags: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if flags != 0 {
        crate::diagnostics::recording_error(format_args!("CopyTextureRegion flags {flags:#x}"));
        state.error = E_NOTIMPL;
        return;
    }
    let device = parent(&list.parent);
    let (Some((dst, destination)), Some((src, source))) =
        (unsafe { copy_location(dst, device) }, unsafe {
            copy_location(src, device)
        })
    else {
        state.error = E_INVALIDARG;
        return;
    };
    if let Err(error) = state.prepare_copy(destination, source) {
        state.error = error;
        return;
    }
    crate::diagnostics::display::copy(dst.resource);
    unsafe {
        (list.native.vtable().CopyTextureRegion)(
            list.native.as_raw(),
            ptr::from_ref(&dst).cast(),
            x,
            y,
            z,
            ptr::from_ref(&src).cast(),
            region,
        );
    }
}

// Frosty's pool allocator and its buffer and texture copy routines all use
// slot 84 with destination, source and zero options.
pub(super) unsafe extern "system" fn copy_resource(
    list: &List,
    dst: *mut c_void,
    src: *mut c_void,
    options: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if options != 0 {
        crate::diagnostics::recording_error(format_args!(
            "slot 84 CopyResource options {options:#x}"
        ));
        state.error = E_NOTIMPL;
        return;
    }
    let owner = parent(&list.parent);
    // SAFETY: each guest argument is a live COM object or null. Only resource
    // proxies owned by the list's device yield borrowed native interfaces.
    let (Some(dst), Some(src)) = (unsafe { device::resource_proxy(dst, owner) }, unsafe {
        device::resource_proxy(src, owner)
    }) else {
        state.error = E_INVALIDARG;
        return;
    };
    crate::diagnostics::display::copy(dst.native.as_raw());
    let (d, s) = unsafe { (dst.native.GetDesc(), src.native.GetDesc()) };
    if dst.state.buffer_window().is_some() || src.state.buffer_window().is_some() {
        let (to, size) = dst.state.buffer_window().unwrap_or((0, d.Width));
        let (from, other) = src.state.buffer_window().unwrap_or((0, s.Width));
        if d.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER || s.Dimension != d.Dimension || size != other
            || (dst.native == src.native && to < from + size && from < to + size) {
            state.error = E_INVALIDARG;
            return;
        }
        unsafe { list.native.CopyBufferRegion(&dst.native, to, &src.native, from, size) };
        return;
    }
    if dst.native == src.native
        || d.Dimension != s.Dimension
        || d.DepthOrArraySize != s.DepthOrArraySize
        || d.MipLevels != s.MipLevels
        || d.SampleDesc != s.SampleDesc
        // D3D12 also allows reinterpret copies between some differently sized
        // block-compressed and uncompressed textures. Leave those to the driver.
        || (d.Format == s.Format && (d.Width != s.Width || d.Height != s.Height))
    {
        state.error = E_INVALIDARG;
        return;
    }
    if let Err(error) = state.prepare_copy(dst.state.placed(), src.state.placed()) {
        state.error = error;
        return;
    }
    // SAFETY: both native resources belong to this open command list's device.
    // The guest supplies their copy states via barriers and owns their lifetimes.
    unsafe { list.native.CopyResource(&dst.native, &src.native) };
    static FIRST_COPY: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST_COPY, || {
        format!(
            "recorded CopyResource (dimension {}, {} x {} x {}, {} mips)",
            s.Dimension.0, s.Width, s.Height, s.DepthOrArraySize, s.MipLevels
        )
    });
}
