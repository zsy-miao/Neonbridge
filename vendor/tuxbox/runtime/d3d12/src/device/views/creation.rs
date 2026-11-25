//! Creating views and samplers in descriptor heaps.
use super::*;

pub(in super::super) unsafe extern "system" fn srv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_SHADER_RESOURCE_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    let resource = unsafe { view_resource(device, object, desc, handle) };
    let proxy = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) };
    let mut alias = None;
    let mut byte_view = None;
    let mut adjusted;
    let desc = if let Some(super::super::Backing::Addressed(address, original)) = proxy.map(|p| &p.state.backing)
        && address.buffer_offset != 0
    {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer SRV requires a buffer description")); };
        if view.ViewDimension != D3D12_SRV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer SRV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        match placed_view(device, address, original.Width, adjusted.Format.0,
            b.StructureByteStride, b.FirstElement, b.NumElements, b.Flags.0, D3D12_RESOURCE_FLAG_NONE) {
            PlacedView::Elements(first, native) => {
                b.FirstElement = first;
                alias = native.map(|native| (native, address.clone()));
            }
            PlacedView::Displaced(mode, offset, stride) => byte_view = Some((mode, offset, stride)),
        }
        &adjusted
    } else { desc };
    let translated;
    let substitute = proxy.and_then(|p| p.state.translated());
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::super::format::view_format(substitute.is_some(), view.Format) != view.Format => {
            translated = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: super::super::format::view_format(substitute.is_some(), view.Format), ..*view
            };
            &raw const translated
        }
        None if substitute.is_some_and(super::super::format::is_depth) => {
            fail_call(format_args!("Scarlett depth SRV needs an explicit plane format"))
        }
        _ => desc,
    };
    crate::diagnostics::display::texture_view(handle.ptr, resource, unsafe { desc.as_ref() }.map(|view| view.Format.0));
    let texture = proxy.and_then(|resource| resource.state.placed());
    let scene = device.state.blas.scene_view(proxy, unsafe { desc.as_ref() });
    device.state.blas.views.store(handle.ptr, scene);
    device.state.placements.views.set(handle.ptr, texture, false);
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_srv(device, resource.expect("placed buffer"), unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed SRV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        unsafe { device.native.CreateShaderResourceView(alias.as_ref().map(|(native, _)| native).or(resource), Some(desc), handle) };
    }
    device.state.imported_views.store(handle.ptr, alias.map(|(native, original)| DescriptorBacking::BufferAlias { native, original }));
}

pub(in super::super) unsafe extern "system" fn uav(
    device: &Device,
    object: *mut c_void,
    counter: *mut c_void,
    desc: *const D3D12_UNORDERED_ACCESS_VIEW_DESC,
    mut handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let _write = super::super::descriptor_paging::Write::new(&device.state.descriptor_arenas, &mut handle);
    crate::diagnostics::display::texture_view(handle.ptr, None, None);
    let owner = identity(device).as_raw();
    let proxy = unsafe { super::super::resource_proxy(object, owner) };
    let placed = proxy.and_then(|p| p.state.placed());
    let window = proxy.and_then(|p| p.state.buffer_window());
    let counter_window = unsafe { super::super::resource_proxy(counter, owner) }.and_then(|p| p.state.buffer_window());
    let mut adjusted;
    let mut uav_alias = None;
    let mut byte_view = None;
    let desc = if window.is_some() || counter_window.is_some() {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer UAV requires a buffer description")); };
        if view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer UAV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        if let Some(super::super::Backing::Addressed(address, original)) = proxy.map(|p| &p.state.backing)
            && window.is_some()
        {
            if !original.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
                fail_call(format_args!("placed buffer was not created for unordered access"));
            }
            match placed_view(device, address, original.Width, adjusted.Format.0,
                b.StructureByteStride, b.FirstElement, b.NumElements, b.Flags.0, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
                PlacedView::Elements(first, alias) => {
                    b.FirstElement = first;
                    uav_alias = alias.map(|alias| (alias, address.clone()));
                }
                PlacedView::Displaced(mode, offset, stride) => byte_view = Some((mode, offset, stride)),
            }
        }
        if let Some((prefix, width)) = counter_window {
            if b.CounterOffsetInBytes.checked_add(4).is_none_or(|end| end > width) {
                fail_call(format_args!("placed UAV counter outside resource"));
            }
            b.CounterOffsetInBytes += prefix;
            if !b.CounterOffsetInBytes.is_multiple_of(D3D12_UAV_COUNTER_PLACEMENT_ALIGNMENT.into()) {
                fail_call(format_args!("unaligned placed UAV counter offset {}",b.CounterOffsetInBytes));
            }
        }
        &adjusted
    } else { desc };
    device.state.placements.views.set(handle.ptr, placed, true);
    device.state.blas.views.store(handle.ptr, None);
    device.state.imported_views.store(handle.ptr, uav_alias.as_ref().map(|(alias, original)|
        DescriptorBacking::BufferAlias { native: alias.clone(), original: original.clone() }));
    let buffer = unsafe { view_resource(device, object, desc, handle) };
    let buffer = uav_alias.as_ref().map(|(alias, _)| alias).or(buffer);
    let counter = if counter.is_null() {
        None
    } else {
        if buffer.is_none() {
            fail_call(format_args!("null UAV resource with a counter"));
        }
        Some(
            unsafe { resource(counter, identity(device).as_raw()) }
                .unwrap_or_else(|| fail_call(format_args!("invalid UAV counter resource"))),
        )
    };
    let substitute = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) }.and_then(|r| r.state.translated());
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::super::format::view_format(substitute.is_some(), view.Format) != view.Format => {
            translated = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: super::super::format::view_format(substitute.is_some(), view.Format), ..*view
            };
            &raw const translated
        }
        _ => desc,
    };
    if let Some(guest) = substitute.filter(|g| super::super::format::is_depth(g)) {
        // The host depth image has no writable view yet. Keep the descriptor
        // defined (null: reads return zero, writes are dropped) and report it.
        let Some(view) = (unsafe { desc.as_ref() }) else {
            fail_call(format_args!("UAV on Scarlett depth format {} needs a description", guest.Format.0));
        };
        static DEPTH_UAV: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&DEPTH_UAV, || format!(
            "null UAV for Scarlett depth format={} {}x{}x{} flags={:#x}: view format={} dimension={}",
            guest.Format.0, guest.Width, guest.Height, guest.DepthOrArraySize, guest.Flags.0,
            view.Format.0, view.ViewDimension.0
        ));
        unsafe { device.native.CreateUnorderedAccessView(None, None, Some(view), handle) };
        return;
    }
    if let Some(pair) = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) }
        .and_then(|r| r.state.depth())
    {
        use windows::Win32::Graphics::Dxgi::Common::{
            DXGI_FORMAT_R16_UNORM, DXGI_FORMAT_R32_FLOAT, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS,
        };
        let Some(view) = (unsafe { desc.as_ref() }) else {
            fail_call(format_args!("depth UAV needs a typed plane view"));
        };
        if pair.guest.Format == super::super::format::D16_UNORM_S8_UINT
            && view.ViewDimension == D3D12_UAV_DIMENSION_TEXTURE2D
            && unsafe { view.Anonymous.Texture2D.PlaneSlice } == 1
        {
            // The companion has no stencil. Keep the descriptor defined (null:
            // reads return zero, writes are dropped) and report it.
            static STENCIL_UAV: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&STENCIL_UAV, || format!(
                "null UAV for the Scarlett stencil plane of {}x{}: view format={}",
                pair.guest.Width, pair.guest.Height, view.Format.0
            ));
            let null = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_UAV::default() },
                ..*view
            };
            unsafe { device.native.CreateUnorderedAccessView(None, None, Some(&null), handle) };
            return;
        }
        if counter.is_some()
            || view.ViewDimension != D3D12_UAV_DIMENSION_TEXTURE2D
            // Either the plane format or the float format of the depth plane.
            || !(matches!(view.Format, DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS | DXGI_FORMAT_R32_FLOAT)
                // The companion of a 16-bit plane holds its normalized values as floats.
                || view.Format == DXGI_FORMAT_R16_UNORM && pair.guest.Format == super::super::format::D16_UNORM_S8_UINT)
            || unsafe {
                view.Anonymous.Texture2D.MipSlice != 0 || view.Anonymous.Texture2D.PlaneSlice != 0
            }
        {
            fail_call(format_args!(
                "unsupported depth UAV plane view: format={} dimension={} mip/plane={:?} counter={} on {}x{}x{}",
                view.Format.0, view.ViewDimension.0,
                (view.ViewDimension == D3D12_UAV_DIMENSION_TEXTURE2D)
                    .then(|| unsafe { (view.Anonymous.Texture2D.MipSlice, view.Anonymous.Texture2D.PlaneSlice) }),
                counter.is_some(), pair.guest.Width, pair.guest.Height, pair.guest.DepthOrArraySize
            ));
        }
        let host = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_FLOAT,
            ..*view
        };
        unsafe {
            device
                .native
                .CreateUnorderedAccessView(&pair.image, None, Some(&host), handle)
        };
        return;
    }
    if let Some((mode, offset, stride)) = byte_view {
        mode.create_uav(device, buffer.expect("placed buffer"), counter, unsafe { *desc }, offset, stride, handle)
            .unwrap_or_else(|e| fail_call(format_args!("placed UAV offset={offset} stride={stride}: {:?}", e.code())));
    } else {
        unsafe { device.native.CreateUnorderedAccessView(buffer, counter, Some(desc), handle) };
    }
}

pub(in super::super) unsafe extern "system" fn rtv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_RENDER_TARGET_VIEW_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let resource = unsafe { view_resource(device, object, desc, handle) };
    let proxy = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) };
    crate::diagnostics::display::capture_target(handle.ptr, resource);
    crate::diagnostics::display::rtv(handle.ptr, proxy.map(|p| p.native.as_raw()));
    device.state.placements.targets.set(handle.ptr, proxy.and_then(|p| p.state.placed()));
    let substitute = proxy.is_some_and(|p| p.state.translated().is_some());
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::super::format::view_format(substitute, view.Format) != view.Format => {
            translated = D3D12_RENDER_TARGET_VIEW_DESC { Format: super::super::format::view_format(substitute, view.Format), ..*view };
            &raw const translated
        }
        _ => desc,
    };
    let mut adjusted;
    let desc = if let Some(window) = proxy.and_then(|p| p.state.buffer_window()) {
        let Some(view) = (unsafe { desc.as_ref() }) else { fail_call(format_args!("placed buffer RTV requires a buffer description")); };
        if view.ViewDimension != D3D12_RTV_DIMENSION_BUFFER { fail_call(format_args!("placed buffer RTV has non-buffer dimension")); }
        adjusted = *view;
        let b = unsafe { &mut adjusted.Anonymous.Buffer };
        b.FirstElement = rebase_view(window, adjusted.Format.0, 0, b.FirstElement, b.NumElements);
        &adjusted
    } else { desc };
    unsafe {
        device
            .native
            .CreateRenderTargetView(resource, Some(desc), handle)
    };
}

pub(in super::super) unsafe extern "system" fn dsv(
    device: &Device,
    object: *mut c_void,
    desc: *const D3D12_DEPTH_STENCIL_VIEW_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    let resource = unsafe { view_resource(device, object, desc, handle) };
    let translated;
    let desc = match unsafe { desc.as_ref() } {
        Some(view) if super::super::format::host_format(view.Format) != view.Format => {
            translated = D3D12_DEPTH_STENCIL_VIEW_DESC { Format: super::super::format::host_format(view.Format), ..*view };
            &raw const translated
        }
        None if let Some(guest) = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) }
            .and_then(|p| p.state.translated().or_else(|| p.state.depth().map(|pair| &pair.guest)
                .filter(|guest| guest.Format == super::super::format::D16_UNORM_S8_UINT))) => {
            // A typeless host image needs the typed depth format the guest implied.
            let mut view = D3D12_DEPTH_STENCIL_VIEW_DESC { Format: super::super::format::host_format(guest.Format), ..Default::default() };
            let layers = u32::from(guest.DepthOrArraySize);
            match (guest.Dimension, guest.SampleDesc.Count > 1) {
                (D3D12_RESOURCE_DIMENSION_TEXTURE1D, _) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE1D,
                (D3D12_RESOURCE_DIMENSION_TEXTURE1D, _) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE1DARRAY;
                    view.Anonymous.Texture1DArray = D3D12_TEX1D_ARRAY_DSV { MipSlice: 0, FirstArraySlice: 0, ArraySize: layers };
                }
                (_, false) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D,
                (_, true) if layers == 1 => view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DMS,
                (_, false) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DARRAY;
                    view.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_DSV { MipSlice: 0, FirstArraySlice: 0, ArraySize: layers };
                }
                (_, true) => {
                    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2DMSARRAY;
                    view.Anonymous.Texture2DMSArray = D3D12_TEX2DMS_ARRAY_DSV { FirstArraySlice: 0, ArraySize: layers };
                }
            }
            translated = view;
            &raw const translated
        }
        _ => desc,
    };
    if super::super::stencil::needs_emulation(device) {
        let source = unsafe { super::super::resource_proxy(object, identity(device).as_raw()) };
        let surface = source
            .filter(|p| super::super::stencil::supported_view(&p.native, unsafe { desc.as_ref() }))
            .map(|p| {
                p.state
                    .stencil
                    .get_or_init(|| std::sync::Arc::new(super::super::stencil::Surface::new(&p.native)))
            });
        device.state.depth_views.set(handle.ptr, surface);
    }
    unsafe {
        device
            .native
            .CreateDepthStencilView(resource, Some(desc), handle)
    };
}

pub(in super::super) unsafe extern "system" fn sampler(
    device: &Device,
    desc: *const D3D12_SAMPLER_DESC,
    handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    check_destination(handle);
    if desc.is_null() {
        fail_call(format_args!("null D3D12 sampler description"));
    }
    unsafe { device.native.CreateSampler(desc, handle) };
}

// Xbox extended sampler state separates the minification, magnification and
// mip filters. Border color, LOD range and comparison are ordinary D3D values.
pub(in super::super) unsafe extern "system" fn sampler_x(
    device: &Device, words: *const u32, handle: D3D12_CPU_DESCRIPTOR_HANDLE,
) {
    if words.is_null() { fail_call(format_args!("null extended sampler")); }
    let w = unsafe { core::slice::from_raw_parts(words, 21) };
    let comparison = w[16] != D3D12_COMPARISON_FUNC_NEVER.0 as u32 && w[16] != 0;
    let reduction = if comparison { 1 } else { match w[5] { 0 => 0, 1 => 2, 2 => 3, other => fail_call(format_args!("unsupported sampler reduction {other}")) } };
    let linear = |filter: u32| match filter { 0 | 1 => 0, 2 => 1, other => fail_call(format_args!("unsupported sampler filter {other}")) };
    let bits = if w[1] == 3 && w[2] == 3 { 0x55 } else {
        linear(w[1]) << 4 | linear(w[2]) << 2 | linear(w[3])
    };
    let host = D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER(bits | reduction << 7),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(w[6] as _),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(w[7] as _),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(w[8] as _),
        MinLOD: f32::from_bits(w[9]), MaxLOD: f32::from_bits(w[10]),
        MipLODBias: f32::from_bits(w[11]) + f32::from_bits(w[12]),
        MaxAnisotropy: w[13].clamp(1, 16),
        ComparisonFunc: D3D12_COMPARISON_FUNC(w[16] as _),
        BorderColor: [w[17], w[18], w[19], w[20]].map(f32::from_bits),
    };
    unsafe { sampler(device, &host, handle) };
}
