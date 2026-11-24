//! Compact Xbox DMA list used by the media renderer to split decoded NV12.
use super::{
    CopyLocation, List, ListData, TransitionBarrier, device, parent, recording, translate_batch,
};
use crate::device::media::{Plane, alias, native, observe};
use crate::proxy::{Slots, child_slots};
use std::{ptr, slice, sync::Arc};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::{HRESULT, Interface, Result};

pub(super) struct Recording {
    allocator: device::MediaResources,
    pending: Option<Arc<Plane>>,
}
impl Recording {
    pub(super) fn new(allocator: &device::Allocator) -> Self {
        Self {
            allocator: allocator.state.clone(),
            pending: None,
        }
    }
    fn retain(&self, resources: &[&ID3D12Resource]) -> Result<()> {
        // D3D12 permits list Reset while the GPU is using the old recording.
        // The recording allocator must outlive execution and is not reset
        // until its fence completes. It owns all decoder references until then.
        let mut held = self.allocator.lock().unwrap_or_else(|e| e.into_inner());
        for &r in resources {
            let item: ID3D12Pageable = r.clone().into();
            if !held.contains(&item) {
                held.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
                held.push(item);
            }
        }
        Ok(())
    }
}

pub(super) static SLOTS: Slots<34> = {
    let mut s = child_slots::<ID3D12GraphicsCommandList, ListData, 34>();
    s[8] = crate::slot!(super::get_type);
    s[9] = crate::slot!(super::close);
    s[10] = crate::slot!(super::reset);
    s[11] = crate::slot!(super::compute::copy_buffer);
    // Slot 12 stays the NV12 plane split. It is not a general texture copy.
    s[12] = crate::slot!(copy);
    s[13] = crate::slot!(super::standard::copy_whole_resource);
    s[14] = crate::slot!(super::standard::copy_tiles);
    s[15] = crate::slot!(barriers);
    s[16] = crate::slot!(super::queries::query::<true>);
    s[17] = crate::slot!(super::queries::query::<false>);
    s[18] = crate::slot!(super::queries::resolve);
    s[19] = crate::slot!(super::standard::set_marker);
    s[20] = crate::slot!(super::standard::begin_event);
    s[21] = crate::slot!(super::standard::end_event);
    s[27] = crate::slot!(super::memory_wait::wait_value64);
    s
};

fn copy_state(state: D3D12_RESOURCE_STATES) -> std::result::Result<D3D12_RESOURCE_STATES, HRESULT> {
    if matches!(
        state,
        D3D12_RESOURCE_STATE_COMMON
            | D3D12_RESOURCE_STATE_COPY_SOURCE
            | D3D12_RESOURCE_STATE_COPY_DEST
    ) {
        Ok(state)
    } else if state.0
        & !(D3D12_RESOURCE_STATE_COPY_SOURCE
            | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
            | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE)
            .0
        == 0
    {
        // Shader reads take place on the graphics queue. The copy queue
        // hands these surfaces back in COMMON, where read promotion applies.
        Ok(D3D12_RESOURCE_STATE_COMMON)
    } else {
        Err(E_NOTIMPL)
    }
}

unsafe extern "system" fn barriers(list: &List, count: u32, input: *const D3D12_RESOURCE_BARRIER) {
    let mut state = recording(list);
    if !state.can_record(&list.native) || count == 0 {
        return;
    }
    if input.is_null() {
        state.error = E_POINTER;
        return;
    }
    let Some(media) = state.media.as_mut() else {
        state.error = E_INVALIDARG;
        return;
    };
    let guest = unsafe { slice::from_raw_parts(input, count as usize) };
    let result = translate_batch(
        guest,
        (),
        |b, ()| {
            if b.Type != D3D12_RESOURCE_BARRIER_TYPE_TRANSITION
                || b.Flags != D3D12_RESOURCE_BARRIER_FLAG_NONE
            {
                return Err(E_NOTIMPL);
            }
            // SAFETY: The active union variant was checked above.
            let mut t = unsafe { ptr::from_ref(b).cast::<TransitionBarrier>().read() };
            let resource =
                unsafe { native(t.resource, parent(&list.parent)) }.map_err(|e| e.code())?;
            media.retain(&[&resource]).map_err(|e| e.code())?;
            t.before = copy_state(t.before)?;
            t.after = copy_state(t.after)?;
            t.resource = resource.as_raw();
            Ok(t)
        },
        |host, ()| {
            for t in host.iter().filter(|t| t.before != t.after) {
                // SAFETY: TransitionBarrier has the desktop layout and the
                // recording allocator owns the resource throughout execution.
                unsafe {
                    (list.native.vtable().ResourceBarrier)(
                        list.native.as_raw(),
                        1,
                        ptr::from_ref(t).cast(),
                    )
                };
            }
            Ok(())
        },
    );
    if let Err(e) = result {
        state.error = e;
    }
}

unsafe extern "system" fn copy(
    list: &List,
    dst: *const CopyLocation,
    x: u32,
    y: u32,
    z: u32,
    src: *const CopyLocation,
    region: *const D3D12_BOX,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let (Some(dst), Some(src)) = (unsafe { dst.as_ref() }, unsafe { src.as_ref() }) else {
        state.error = E_POINTER;
        return;
    };
    let Some(media) = state.media.as_mut() else {
        state.error = E_INVALIDARG;
        return;
    };
    if let Err(error) = unsafe { record_copy(list, media, dst, [x, y, z], src, region.as_ref()) } {
        state.error = error.code();
    }
}

unsafe fn record_copy(
    list: &List,
    media: &mut Recording,
    dst: &CopyLocation,
    at: [u32; 3],
    src: &CopyLocation,
    region: Option<&D3D12_BOX>,
) -> Result<()> {
    if dst.kind != D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX
        || src.kind != D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX
    {
        return Err(E_NOTIMPL.into());
    }
    if unsafe { dst.data.SubresourceIndex } != 0 || unsafe { src.data.SubresourceIndex } != 0 {
        return Err(E_INVALIDARG.into());
    }
    let owner = parent(&list.parent);
    let destination = unsafe { native(dst.resource, owner) }?;
    let dd = unsafe { destination.GetDesc() };
    let mut from = *src;
    let (source, width, height) =
        if let Some((address, desc)) = unsafe { alias(src.resource, owner) } {
            let plane = media.pending.take().ok_or(E_INVALIDARG)?;
            if address != plane.address
                || desc.Width != plane.pitch / 2
                || desc.Height != plane.height / 2
                || dd.Format != DXGI_FORMAT_R8G8_UNORM
            {
                return Err(E_INVALIDARG.into());
            }
            let sd = unsafe { plane.source.GetDesc() };
            from.data.SubresourceIndex = 1;
            (plane.source.clone(), sd.Width / 2, sd.Height / 2)
        } else {
            let source = unsafe { native(src.resource, owner) }?;
            let sd = unsafe { source.GetDesc() };
            if sd.Format != DXGI_FORMAT_NV12 || dd.Format != DXGI_FORMAT_R8_UNORM {
                return Err(E_NOTIMPL.into());
            }
            media.pending = Some(unsafe { observe(owner, source.clone()) }?);
            (source, sd.Width, sd.Height)
        };
    let width = u32::try_from(width).map_err(|_| E_INVALIDARG)?;
    let rect = region.copied().unwrap_or(D3D12_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: width,
        bottom: height,
        back: 1,
    });
    if rect.left >= rect.right
        || rect.right > width
        || rect.top >= rect.bottom
        || rect.bottom > height
        || rect.front != 0
        || rect.back != 1
        || at[2] != 0
        || u64::from(at[0]) + u64::from(rect.right - rect.left) > dd.Width
        || at[1]
            .checked_add(rect.bottom - rect.top)
            .is_none_or(|end| end > dd.Height)
        || dd.Dimension != D3D12_RESOURCE_DIMENSION_TEXTURE2D
        || dd.SampleDesc.Count != 1
    {
        return Err(E_INVALIDARG.into());
    }
    media.retain(&[&source, &destination])?;
    from.resource = source.as_raw();
    let to = CopyLocation {
        resource: destination.as_raw(),
        ..*dst
    };
    unsafe {
        (list.native.vtable().CopyTextureRegion)(
            list.native.as_raw(),
            ptr::from_ref(&to).cast(),
            at[0],
            at[1],
            at[2],
            ptr::from_ref(&from).cast(),
            &rect,
        )
    };
    Ok(())
}
