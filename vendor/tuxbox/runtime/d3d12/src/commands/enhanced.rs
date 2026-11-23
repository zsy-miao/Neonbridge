//! The Series barrier groups have the desktop layout with guest COM resource pointers.
use super::*;

fn guest_pointer(
    field: &mem::ManuallyDrop<Option<ID3D12Resource>>,
) -> Result<*mut c_void, HRESULT> {
    field.as_ref().map(Interface::as_raw).ok_or(E_INVALIDARG)
}

fn borrow_resource(field: &mut mem::ManuallyDrop<Option<ID3D12Resource>>, native: &ID3D12Resource) {
    // Desktop D3D12 borrows the COM pointer for the duration of the call.
    unsafe {
        ptr::from_mut(field)
            .cast::<*mut c_void>()
            .write(native.as_raw())
    };
}

fn texture_group(api: &ID3D12GraphicsCommandList7, items: &[D3D12_TEXTURE_BARRIER]) {
    if !items.is_empty() {
        unsafe {
            api.Barrier(&[D3D12_BARRIER_GROUP {
                Type: D3D12_BARRIER_TYPE_TEXTURE,
                NumBarriers: items.len() as u32,
                Anonymous: D3D12_BARRIER_GROUP_0 {
                    pTextureBarriers: items.as_ptr(),
                },
            }]);
        }
    }
}

unsafe fn record(
    list: &List,
    native: &ID3D12GraphicsCommandList7,
    groups: &[D3D12_BARRIER_GROUP],
) -> Result<(), HRESULT> {
    let owner = parent(&list.parent);
    for group in groups {
        let count = group.NumBarriers as usize;
        if count == 0 || unsafe { group.Anonymous.pGlobalBarriers.is_null() } {
            return Err(E_INVALIDARG);
        }
        match group.Type {
            D3D12_BARRIER_TYPE_GLOBAL => unsafe { native.Barrier(slice::from_ref(group)) },
            D3D12_BARRIER_TYPE_BUFFER => {
                let mut barriers = Vec::new();
                barriers
                    .try_reserve_exact(count)
                    .map_err(|_| E_OUTOFMEMORY)?;
                for item in unsafe { slice::from_raw_parts(group.Anonymous.pBufferBarriers, count) }
                {
                    let resource =
                        unsafe { device::resource_proxy(guest_pointer(&item.pResource)?, owner) }
                            .ok_or(E_INVALIDARG)?;
                    let mut item = unsafe { ptr::read(item) };
                    if let Some((delta, len)) = resource.state.buffer_window() {
                        let size = if item.Size == u64::MAX && item.Offset == 0 {
                            len
                        } else {
                            item.Size
                        };
                        if size == 0 || item.Offset.checked_add(size).is_none_or(|end| end > len) {
                            return Err(E_INVALIDARG);
                        }
                        let host_end = delta
                            .checked_add(item.Offset)
                            .and_then(|offset| offset.checked_add(size))
                            .ok_or(E_INVALIDARG)?;
                        if host_end > unsafe { resource.native.GetDesc() }.Width {
                            return Err(E_INVALIDARG);
                        }
                        if item.SyncBefore == D3D12_BARRIER_SYNC_SPLIT
                            || item.SyncAfter == D3D12_BARRIER_SYNC_SPLIT
                            || item.AccessBefore == D3D12_BARRIER_ACCESS_NO_ACCESS
                            || item.AccessAfter == D3D12_BARRIER_ACCESS_NO_ACCESS
                        {
                            return Err(E_NOTIMPL);
                        }
                        // Desktop buffer barriers require the whole resource. A
                        // wider memory dependency also orders the guest window.
                        // Splitting or deactivating it could affect a neighbour.
                        item.Offset = 0;
                        item.Size = u64::MAX;
                    }
                    borrow_resource(&mut item.pResource, &resource.native);
                    barriers.push(item);
                }
                unsafe {
                    native.Barrier(&[D3D12_BARRIER_GROUP {
                        Anonymous: D3D12_BARRIER_GROUP_0 {
                            pBufferBarriers: barriers.as_ptr(),
                        },
                        ..*group
                    }]);
                }
            }
            D3D12_BARRIER_TYPE_TEXTURE => {
                let mut barriers = Vec::new();
                barriers
                    .try_reserve_exact(count)
                    .map_err(|_| E_OUTOFMEMORY)?;
                for item in
                    unsafe { slice::from_raw_parts(group.Anonymous.pTextureBarriers, count) }
                {
                    let resource =
                        unsafe { device::resource_proxy(guest_pointer(&item.pResource)?, owner) }
                            .ok_or(E_INVALIDARG)?;
                    if let Some(pair) = resource.state.depth() {
                        // Copies belong between the preceding and following barriers.
                        texture_group(native, &barriers);
                        barriers.clear();
                        if depth::enhanced_handoff(native, &resource.native, pair, item)
                            .map_err(|error| error.code())?
                        {
                            continue;
                        }
                    }
                    let mut item = unsafe { ptr::read(item) };
                    borrow_resource(&mut item.pResource, &resource.native);
                    if resource.state.unbacked_uav() {
                        // The host depth image has no unordered access; see unbacked_uav.
                        item.LayoutBefore = device::format::host_layout(item.LayoutBefore);
                        item.LayoutAfter = device::format::host_layout(item.LayoutAfter);
                        item.AccessBefore &= !D3D12_BARRIER_ACCESS_UNORDERED_ACCESS;
                        item.AccessAfter &= !D3D12_BARRIER_ACCESS_UNORDERED_ACCESS;
                    }
                    if resource.state.placed().is_some() {
                        // Simultaneous-access textures share COMMON across all uses.
                        if item.LayoutBefore != D3D12_BARRIER_LAYOUT_UNDEFINED {
                            item.LayoutBefore = D3D12_BARRIER_LAYOUT_COMMON;
                        }
                        if item.LayoutAfter != D3D12_BARRIER_LAYOUT_UNDEFINED {
                            item.LayoutAfter = D3D12_BARRIER_LAYOUT_COMMON;
                        }
                    }
                    barriers.push(item);
                }
                texture_group(native, &barriers);
            }
            _ => return Err(E_NOTIMPL),
        }
    }
    Ok(())
}

pub(super) unsafe extern "system" fn barrier(
    list: &List,
    count: u32,
    groups: *const D3D12_BARRIER_GROUP,
) {
    let mut state = recording(list);
    if count == 0 || !state.can_record(&list.native) {
        return;
    }
    if groups.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    let Some(native) = list
        .state
        .enhanced_barriers
        .get_or_init(|| enhanced_barriers(&list.native))
    else {
        state.error = E_NOTIMPL;
        return;
    };
    if let Err(error) =
        unsafe { record(list, native, slice::from_raw_parts(groups, count as usize)) }
    {
        crate::diagnostics::recording_error(format_args!(
            "enhanced resource barrier group ({error:?})"
        ));
        state.error = error;
    }
}
