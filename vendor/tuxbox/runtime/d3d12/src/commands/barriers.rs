//! Resource barriers and cache flushes.
use super::*;

pub(super) unsafe extern "system" fn transition(
    list: &List,
    count: u32,
    barriers: *const D3D12_RESOURCE_BARRIER,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) || count == 0 {
        return;
    }
    if barriers.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    // The guest supplies count readable barriers. Read a transition payload
    // only after checking the union tag.
    let guest = unsafe { slice::from_raw_parts(barriers, count as usize) };
    let result = translate_batch(
        guest,
        Vec::new(),
        |barrier, handoffs| {
            if barrier.Type == D3D12_RESOURCE_BARRIER_TYPE_ALIASING
                && barrier.Flags == D3D12_RESOURCE_BARRIER_FLAG_NONE
            {
                let alias = unsafe { &barrier.Anonymous.Aliasing };
                let native = |guest: &Option<ID3D12Resource>| match guest {
                    Some(object) => {
                        unsafe { device::resource(object.as_raw(), parent(&list.parent)) }
                            .map(Interface::as_raw)
                            .ok_or(E_INVALIDARG)
                    }
                    None => Ok(ptr::null_mut()),
                };
                return Ok(RecordedBarrier {
                    aliasing: AliasingBarrier {
                        kind: barrier.Type,
                        flags: barrier.Flags,
                        before: native(&alias.pResourceBefore)?,
                        after: native(&alias.pResourceAfter)?,
                    },
                });
            }
            if barrier.Type == D3D12_RESOURCE_BARRIER_TYPE_UAV
                && barrier.Flags == D3D12_RESOURCE_BARRIER_FLAG_NONE
            {
                // A null UAV orders all UAV accesses. Only the first pointer of
                // this union member is initialized by the guest.
                let guest = unsafe { &barrier.Anonymous.UAV.pResource };
                let resource = match guest.as_ref() {
                    Some(object) => {
                        let r = unsafe {
                            device::resource_proxy(object.as_raw(), parent(&list.parent))
                        }
                        .ok_or(E_INVALIDARG)?;
                        if device::blas::is_structure(r) {
                            // A guest AS pool may contain individually relocated structures.
                            // Order their host writes even when the barrier was recorded first.
                            ptr::null_mut()
                        } else {
                            r.state
                                .depth()
                                .map_or(&r.native, |pair| &pair.image)
                                .as_raw()
                        }
                    }
                    None => ptr::null_mut(),
                };
                return Ok(RecordedBarrier {
                    transition: TransitionBarrier {
                        kind: barrier.Type,
                        flags: barrier.Flags,
                        resource,
                        ..Default::default()
                    },
                });
            }
            if barrier.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION
                || !matches!(
                    barrier.Flags,
                    D3D12_RESOURCE_BARRIER_FLAG_NONE
                        | D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY
                        | D3D12_RESOURCE_BARRIER_FLAG_END_ONLY
                )
            {
                crate::diagnostics::recording_error(format_args!(
                    "resource barrier type {} flags {:#x}",
                    barrier.Type.0, barrier.Flags.0
                ));
                return Err(E_NOTIMPL);
            }
            let mut host = unsafe { ptr::from_ref(barrier).cast::<TransitionBarrier>().read() };
            let resource = unsafe { device::resource_proxy(host.resource, parent(&list.parent)) }
                .ok_or(E_INVALIDARG)?;
            let different = host.before != host.after;
            let needs_dimension = (host.before.0 | host.after.0) & device::BUFFER_READ_BITS != 0;
            let texture = needs_dimension
                && unsafe { resource.native.GetDesc().Dimension }
                    != D3D12_RESOURCE_DIMENSION_BUFFER;
            host.before = device::host_state(host.before, texture);
            host.after = device::host_state(host.after, texture);
            if resource.state.unbacked_uav() {
                // The host depth image has no unordered access; see unbacked_uav.
                for state in [&mut host.before, &mut host.after] {
                    if *state == D3D12_RESOURCE_STATE_UNORDERED_ACCESS {
                        *state = D3D12_RESOURCE_STATE_COMMON;
                    }
                }
            }
            if let Some(pair) = resource.state.depth()
                && (host.before == D3D12_RESOURCE_STATE_UNORDERED_ACCESS
                    || host.after == D3D12_RESOURCE_STATE_UNORDERED_ACCESS)
            {
                if !matches!(
                    host.subresource,
                    0 | 1 | D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES
                ) {
                    return Err(E_INVALIDARG);
                }
                if host.flags != D3D12_RESOURCE_BARRIER_FLAG_BEGIN_ONLY && different {
                    handoffs.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                    let index = unsafe { ptr::from_ref(barrier).offset_from(barriers) } as usize;
                    handoffs.push((index, &resource.native, pair));
                }
                host.resource = ptr::null_mut();
            } else if let Some(placed) = resource.state.placed() {
                if !placement::supported_state(host.before, placed.flags())
                    || !placement::supported_state(host.after, placed.flags())
                {
                    crate::diagnostics::recording_error(format_args!(
                        "placed texture transition {:#x} to {:#x}",
                        host.before.0, host.after.0
                    ));
                    return Err(E_NOTIMPL);
                }
                // Read promotions can accumulate implicitly. An explicit barrier
                // orders writes against other uses within the same batch.
                host.resource = if different
                    && [host.before, host.after].iter().any(|state|
                        matches!(*state, D3D12_RESOURCE_STATE_COPY_DEST | D3D12_RESOURCE_STATE_RENDER_TARGET))
                {
                    resource.native.as_raw()
                } else {
                    ptr::null_mut()
                };
            } else if different && host.before == host.after {
                host.resource = ptr::null_mut();
            } else {
                host.resource = resource.native.as_raw();
            }
            Ok(RecordedBarrier { transition: host })
        },
        |host, handoffs| unsafe {
            let mut handoffs = handoffs.into_iter().peekable();
            let mut first = 0;
            let submit = |run: &[RecordedBarrier]| {
                if !run.is_empty() {
                    (list.native.vtable().ResourceBarrier)(
                        list.native.as_raw(),
                        run.len() as u32,
                        run.as_ptr().cast(),
                    );
                }
            };
            for (index, barrier) in host.iter().enumerate() {
                if barrier.kind == D3D12_RESOURCE_BARRIER_TYPE_TRANSITION
                    && barrier.transition.resource.is_null()
                {
                    submit(&host[first..index]);
                    if let Some((_, resource, pair)) = handoffs.next_if(|(at, _, _)| *at == index) {
                        depth::handoff(&list.native, resource, pair, &barrier.transition);
                    }
                    first = index + 1;
                }
            }
            submit(&host[first..]);
            Ok(())
        },
    );
    if let Err(error) = result {
        state.error = error;
    }
}

pub(super) fn enhanced_barriers(native: &ID3D12GraphicsCommandList) -> Option<ID3D12GraphicsCommandList7> {
    if !matches!(
        unsafe { native.GetType() },
        D3D12_COMMAND_LIST_TYPE_DIRECT | D3D12_COMMAND_LIST_TYPE_COMPUTE
    ) {
        return None;
    }
    let mut device: Option<ID3D12Device> = None;
    unsafe { native.GetDevice(&mut device) }.ok()?;
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
    unsafe {
        device?.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS12,
            ptr::from_mut(&mut options).cast(),
            size_of_val(&options) as u32,
        )
    }
    .ok()?;
    if options.EnhancedBarriersSupported.as_bool() {
        native.cast().ok()
    } else {
        None
    }
}

pub(super) unsafe extern "system" fn flush_texture_cache(list: &List, mask: u32, address: u64, bytes: u64) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    // The mask selects the caches to flush. Both can use the same host
    // global memory barrier, which also covers subranges and empty ranges.
    if !matches!(mask, 0xc0_0000 | 0x3000) {
        crate::diagnostics::recording_error(format_args!(
            "texture cache flush mask {mask:#x} address {address:#x} bytes {bytes:#x}"
        ));
        state.error = E_NOTIMPL;
        return;
    }
    if address.checked_add(bytes).is_none() {
        crate::diagnostics::recording_error(format_args!(
            "overflowing texture cache flush address {address:#x} bytes {bytes:#x}"
        ));
        state.error = E_INVALIDARG;
        return;
    }
    if address != 0 || bytes != 0xff_ffff_ff00 {
        static RANGE_FLUSH: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&RANGE_FLUSH, || {
            format!("texture cache range flush mask {mask:#x} address {address:#x} bytes {bytes:#x}")
        });
    }
    global_cache_barrier(list, &mut state);
}

// The extended list accepts separate cache/engine masks before the VA range.
// A host global barrier covers the full range and every participating stage.
pub(super) unsafe extern "system" fn extended_cache_barrier(
    list: &List, _cache: u32, _engines: u32, address: u64, bytes: u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    if address.checked_add(bytes).is_none() {
        state.error = E_INVALIDARG;
        return;
    }
    global_cache_barrier(list, &mut state);
}

pub(super) fn global_cache_barrier(list: &List, state: &mut Recording) {
    let Some(native) = list
        .state
        .enhanced_barriers
        .get_or_init(|| enhanced_barriers(&list.native))
    else {
        crate::diagnostics::recording_error(format_args!("enhanced global barriers unavailable"));
        state.error = E_NOTIMPL;
        return;
    };
    let barrier = D3D12_GLOBAL_BARRIER {
        SyncBefore: D3D12_BARRIER_SYNC_ALL,
        SyncAfter: D3D12_BARRIER_SYNC_ALL,
        AccessBefore: D3D12_BARRIER_ACCESS_COMMON,
        AccessAfter: D3D12_BARRIER_ACCESS_COMMON,
    };
    let group = D3D12_BARRIER_GROUP {
        Type: D3D12_BARRIER_TYPE_GLOBAL,
        NumBarriers: 1,
        Anonymous: D3D12_BARRIER_GROUP_0 {
            pGlobalBarriers: &barrier,
        },
    };
    // SAFETY: the native open list consumes the stack-backed group during Barrier.
    unsafe { native.Barrier(&[group]) };
}
