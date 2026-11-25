//! Creating, mapping and describing resources and heaps.
use super::*;

pub(super) unsafe extern "system" fn map_resource(
    this: &Resource,
    subresource: u32,
    read: *const D3D12_RANGE,
    data: Out,
) -> HRESULT {
    let host = &this.native;
    if let Backing::Placed(texture) = &this.state.backing {
        // The host image is device memory; its contents live at the guest address.
        let Some(address) = texture.guest_pointer(subresource) else { return E_INVALIDARG };
        if !data.is_null() {
            unsafe { data.write(address as *mut c_void) };
        }
        return windows::Win32::Foundation::S_OK;
    }
    let Some((offset, width)) = this.state.buffer_window() else {
        let hr = unsafe { (host.vtable().Map)(host.as_raw(), subresource, read, data) };
        if hr.is_err() {
            let desc = unsafe { host.GetDesc() };
            let kind = match &this.state.backing {
                Backing::Native => "native", Backing::ConsoleLayout(..) => "console-layout",
                Backing::Addressed(..) => "addressed", Backing::Placed(_) => "placed",
                Backing::Depth(_) => "depth", Backing::Translated(_) => "translated",
            };
            let heap = unsafe { host.GetHeapProperties(None, None) }.map(|_| ()).is_ok();
            crate::diagnostics::log_once(&std::sync::Once::new(), || format!(
                "Map failed {:#010x}: {kind} dim={} {}x{}x{} format={} layout={:#x} flags={:#x} subresource={subresource} heap-properties={heap} title RVA {:#x}",
                hr.0, desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Layout.0, desc.Flags.0,
                crate::survey::title_rva().unwrap_or(0)));
        }
        return hr;
    };
    let Some(range) = buffer_map_range(offset, width, unsafe { read.as_ref() }) else {
        return E_INVALIDARG;
    };
    let hr = unsafe { (host.vtable().Map)(host.as_raw(), subresource, &range, data) };
    if hr.is_ok() && !data.is_null() {
        unsafe { data.write((*data).byte_add(offset as usize)) };
    }
    hr
}

pub(super) unsafe extern "system" fn unmap_resource(
    this: &Resource,
    subresource: u32,
    written: *const D3D12_RANGE,
) {
    let host = &this.native;
    if let Some(texture) = this.state.placed() {
        if texture.guest_pointer(subresource).is_some()
            && unsafe { written.as_ref() }.is_none_or(|range| range.End > range.Begin)
            && let Some(parent) = &this.parent
        {
            // SAFETY: resource proxies are created with a retained guest device.
            let device = unsafe { &*parent.as_raw().cast::<Device>() };
            if device.state.device11_queue
                && let Err(error) = device.state.placements.finish_cpu_write(texture)
            {
                crate::diagnostics::recording_error(format_args!("placed texture Unmap upload: {error}"));
            }
        }
        return;
    }
    let window;
    let written = if let Some((offset, width)) = this.state.buffer_window() {
        window = buffer_map_range(offset, width, unsafe { written.as_ref() })
            .unwrap_or_else(|| crate::proxy::fail_call(format_args!("placed buffer Unmap range outside the resource")));
        &window
    } else { written };
    unsafe { (host.vtable().Unmap)(host.as_raw(), subresource, written) }
}

pub(super) fn buffer_map_range(offset: u64, width: u64, range: Option<&D3D12_RANGE>) -> Option<D3D12_RANGE> {
    let (begin, end) = range.map_or((0, width), |r| (r.Begin as u64, r.End as u64));
    if begin >= end {
        return Some(D3D12_RANGE::default());
    }
    if end > width {
        return None;
    }
    Some(D3D12_RANGE { Begin: (offset + begin) as usize, End: (offset + end) as usize })
}

pub(super) unsafe extern "system" fn resource_desc(
    this: &Resource,
    out: *mut D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_DESC {
    unsafe {
        out.write(match &this.state.backing {
            Backing::Depth(pair) => pair.guest,
            Backing::Translated(guest) => *guest,
            Backing::Addressed(_, guest) => *guest,
            Backing::Placed(texture) => texture.desc,
            Backing::Native => this.native.GetDesc(),
            Backing::ConsoleLayout(layout, flags) => D3D12_RESOURCE_DESC {
                Layout: *layout,
                Flags: *flags,
                ..this.native.GetDesc()
            },
        })
    };
    out
}

pub(super) unsafe extern "system" fn heap_properties(
    this: &Resource,
    props: *mut D3D12_HEAP_PROPERTIES,
    flags: *mut D3D12_HEAP_FLAGS,
) -> HRESULT {
    let native = &this.native;
    unsafe { (native.vtable().GetHeapProperties)(native.as_raw(), props, flags) }
}

pub(super) unsafe extern "system" fn gpu_address(this: &Resource) -> u64 {
    if let Backing::Addressed(buffer, _) = &this.state.backing { return buffer.base as u64; }
    if let Some(placed) = this.state.placed() {
        return placed.address;
    }
    let desc = unsafe { this.native.GetDesc() };
    this.state.texture_address.get_or_init(|| {
        let owner = this.parent.as_ref().expect("resource belongs to a device");
        let device = unsafe { &*owner.as_raw().cast::<Device>() };
        if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
            let address = unsafe { this.native.GetGPUVirtualAddress() } as usize;
            views::TextureAddress::at(device, &this.native, address, false, None)
        } else {
            views::TextureAddress::register(device, &this.native, &desc)
        }
    }).base as u64
}

pub(super) unsafe extern "system" fn create_heap(
    this: &Device,
    desc: *const D3D12_DESCRIPTOR_HEAP_DESC,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12DescriptorHeap>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if desc.NumDescriptors == 0 {
        return E_INVALIDARG;
    }
    if !matches!(
        desc.Type,
        D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
            | D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER
            | D3D12_DESCRIPTOR_HEAP_TYPE_RTV
            | D3D12_DESCRIPTOR_HEAP_TYPE_DSV
    ) || !matches!(
        desc.Flags,
        D3D12_DESCRIPTOR_HEAP_FLAG_NONE | D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
    ) || desc.NodeMask > 1
    {
        return E_NOTIMPL;
    }
    if desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && matches!(
            desc.Type,
            D3D12_DESCRIPTOR_HEAP_TYPE_RTV | D3D12_DESCRIPTOR_HEAP_TYPE_DSV
        )
    {
        return E_INVALIDARG;
    }
    let mut limit = if desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        && desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && desc.NumDescriptors > 1_000_000 {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS19::default();
        unsafe { this.native.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS19,
            (&raw mut options).cast(), size_of_val(&options) as u32) }
            .map_or(1_000_000, |_| options.MaxViewDescriptorHeapSize.max(1_000_000))
    } else { desc.NumDescriptors };
    if desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV
        && desc.Flags == D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE
        && let Some(cap) = std::env::var("TUXBOX_DESCRIPTOR_HEAP_LIMIT")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|&n| n > 0)
    {
        limit = limit.min(cap);
    }
    let needs_inline = this.state.device11_queue && desc.Type == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    let (native, arena) = if desc.NumDescriptors > limit || needs_inline {
        match descriptor_paging::Arena::create(this, desc, limit) {
            Ok(arena) => (arena.visible().clone(), Some(arena)),
            Err(error) => return error.code(),
        }
    } else {
        match unsafe { this.native.CreateDescriptorHeap::<ID3D12DescriptorHeap>(desc) } {
            Ok(native) => (native, None),
            Err(error) => return error.code(),
        }
    };
    let resident_desc = D3D12_DESCRIPTOR_HEAP_DESC { NumDescriptors: limit, ..*desc };
    let placed_views = this.state.placements.views.heap(
        &this.native,
        &native,
        &resident_desc,
        D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
    );
    let state = DescriptorHeapState {
        desc: *desc,
        cpu_start: arena.as_ref().map_or_else(|| unsafe { native.GetCPUDescriptorHandleForHeapStart().ptr }, |arena| arena.base()),
        arena,
        placed_views,
        _placed_targets: this.state.placements.targets.heap(&this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_RTV),
        _imported_views: this.state.imported_views.heap(&this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
        _scene_views: this.state.blas.views.heap(
            &this.native, &native, &resident_desc, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
        ),
        _depth_views: this.state.depth_views.heap(
            &this.native,
            &native,
            &resident_desc,
            D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
        ),
    };
    unsafe { wrap_pageable(Ok(native), &HEAP, this, state, out) }
}

pub(crate) fn host_state(state: D3D12_RESOURCE_STATES, texture: bool) -> D3D12_RESOURCE_STATES {
    // Frosty's SetSRVs emits these two tagged read states, including on compute lists.
    let bits = match state.0 {
        0x1200_00c0 => 0xc0,
        0x1200_0040 => 0x40,
        other => other,
    };
    D3D12_RESOURCE_STATES(if texture {
        bits & !BUFFER_READ_BITS
    } else {
        bits
    })
}

pub(super) unsafe extern "system" fn create_resource<const EXTENDED: bool>(
    this: &Device,
    properties: *const D3D12_HEAP_PROPERTIES,
    flags: D3D12_HEAP_FLAGS,
    guest_desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let (Some(heap), Some(desc)) = (unsafe { (properties.as_ref(), guest_desc.as_ref()) }) else {
        return E_INVALIDARG;
    };
    if !matches!(flags, D3D12_HEAP_FLAG_NONE | D3D12_HEAP_FLAG_ALLOW_DISPLAY)
        || !matches!(
            heap.Type,
            D3D12_HEAP_TYPE_DEFAULT | D3D12_HEAP_TYPE_READBACK | D3D12_HEAP_TYPE_UPLOAD
        )
        || !matches!(
            desc.Dimension,
            D3D12_RESOURCE_DIMENSION_BUFFER
                | D3D12_RESOURCE_DIMENSION_TEXTURE1D
                | D3D12_RESOURCE_DIMENSION_TEXTURE2D
                | D3D12_RESOURCE_DIMENSION_TEXTURE3D
        )
    {
        return E_NOTIMPL;
    }
    if flags == D3D12_HEAP_FLAG_ALLOW_DISPLAY
        && (heap.Type != D3D12_HEAP_TYPE_DEFAULT
            || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D)
    {
        return E_INVALIDARG;
    }
    let mut host_desc = *desc;
    let mut host_heap = *heap;
    let linear_readback = heap.Type == D3D12_HEAP_TYPE_READBACK
        && desc.Layout.0 == 0x100
        && desc.Flags.0 == 0x300001
        && desc.MipLevels == 1
        && desc.DepthOrArraySize == 1
        && placement::color_plane(desc).is_ok();
    if linear_readback {
        // Desktop READBACK heaps accept only buffers. A custom L0 heap can
        // host a CPU-readable color image while retaining its render-target view.
        host_heap.Type = D3D12_HEAP_TYPE_CUSTOM;
        host_heap.CPUPageProperty = D3D12_CPU_PAGE_PROPERTY_WRITE_BACK;
        host_heap.MemoryPoolPreference = D3D12_MEMORY_POOL_L0;
        host_desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
        host_desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    }
    if EXTENDED {
        // Slot 76 extends the resource description at byte 52 (desktop padding).
        // The Scarlett upscaler writes DCC options there and a zero qword at 56.
        const _: () = assert!(core::mem::offset_of!(D3D12_RESOURCE_DESC, Flags) == 48);
        let bytes = guest_desc.cast::<u8>();
        let (dcc, extension) = unsafe {
            (
                bytes.add(52).cast::<u32>().read_unaligned(),
                bytes.add(56).cast::<u64>().read_unaligned(),
            )
        };
        if !matches!(dcc, 0 | 1 | 3) || extension != 0 {
            return E_NOTIMPL;
        }
        use allocation::DCC_FLAGS;
        if desc.Layout.0 == 0x101 || desc.Flags.0 & DCC_FLAGS != 0 {
            // A private committed texture exposes its logical contents through
            // D3D12. The host driver chooses its tiling and compression.
            if flags != D3D12_HEAP_FLAG_NONE
                || heap.Type != D3D12_HEAP_TYPE_DEFAULT
                || desc.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
                || desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_CROSS_ADAPTER)
            {
                return E_NOTIMPL;
            }
            if desc.Layout.0 == 0x101 {
                host_desc.Layout = D3D12_TEXTURE_LAYOUT_UNKNOWN;
            }
            host_desc.Flags.0 &= !DCC_FLAGS;
        }
    }
    host_desc.Flags.0 &= format::HOST_FLAGS;
    let primary = depth::primary(desc);
    let initial = format::host_initial_state(desc, initial);
    if primary.is_some()
        && (flags != D3D12_HEAP_FLAG_NONE
            || heap.Type != D3D12_HEAP_TYPE_DEFAULT
            || initial.contains(D3D12_RESOURCE_STATE_UNORDERED_ACCESS))
    {
        return E_NOTIMPL;
    }
    let host_clear = format::host_clear(clear);
    let mut native = None;
    if let Err(error) = unsafe {
        this.native.CreateCommittedResource::<ID3D12Resource>(
            &host_heap,
            flags,
            primary.as_ref().unwrap_or(&host_desc),
            host_state(initial, desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER),
            Some(host_clear.as_ref().map_or(clear, core::ptr::from_ref)),
            &mut native,
        )
    } {
        return error.code();
    }
    if linear_readback {
        return unsafe { depth::wrap::<true>(this, heap, desc, None, initial, native, out) };
    }
    unsafe { depth::wrap::<EXTENDED>(this, heap, desc, primary.as_ref(), initial, native, out) }
}
