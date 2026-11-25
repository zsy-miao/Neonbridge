//! SRV-as-UAV piece 2: the native resource and SRV description behind each
//! CBV/SRV/UAV descriptor slot, kept through descriptor copies like
//! view_formats, so a slot xshader's `.slots` names (shader_cache::slots)
//! can be given a UAV of the same resource at dispatch. Kept only while
//! TUXBOX_SRV_AS_UAV=1. A kept resource stays alive until its slot is
//! overwritten or cleared.
use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::*;

type Kept = (ID3D12Resource, D3D12_SHADER_RESOURCE_VIEW_DESC);

static VIEWS: Mutex<Option<HashMap<usize, Kept>>> = Mutex::new(None);

fn on() -> bool {
    super::shader_cache::slots::enabled()
}

/// An SRV of `resource` was made at `handle`.
pub(crate) fn record(handle: usize, resource: Option<&ID3D12Resource>, desc: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>) {
    if !on() {
        return;
    }
    let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let views = views.get_or_insert_with(Default::default);
    match (resource, desc) {
        (Some(r), Some(d)) => { views.insert(handle, (r.clone(), *d)); }
        _ => { views.remove(&handle); }
    }
}

/// Some other view (or none) was made at `handle`.
pub(crate) fn clear(handle: usize) {
    if !on() {
        return;
    }
    if let Some(views) = VIEWS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        views.remove(&handle);
    }
}

/// `count` descriptors were copied from `src` to `dst`.
pub(crate) fn copy(dst: usize, src: usize, count: usize, stride: usize) {
    if !on() || count == 0 || dst == src {
        return;
    }
    let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let Some(views) = views.as_mut() else { return };
    let kept: Vec<Option<Kept>> = (0..count).map(|i| views.get(&(src + i * stride)).cloned()).collect();
    for (i, k) in kept.into_iter().enumerate() {
        match k {
            Some(k) => { views.insert(dst + i * stride, k); }
            None => { views.remove(&(dst + i * stride)); }
        }
    }
}

/// The resource and SRV description at `handle`, if an SRV was made there.
pub(crate) fn get(handle: usize) -> Option<Kept> {
    VIEWS.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&handle).cloned()
}

/// Host roots made by compute_table::srv_as_uav, with the slots each holds as UAVs.
static TABLES: Mutex<Option<HashMap<usize, std::sync::Arc<[super::shader_cache::slots::Slot]>>>> = Mutex::new(None);

pub(crate) fn note_table(root: &ID3D12RootSignature, slots: std::sync::Arc<[super::shader_cache::slots::Slot]>) {
    // The pipeline holds the root for its life; the key is not reused while it lives.
    TABLES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default)
        .insert(windows::core::Interface::as_raw(root) as usize, slots);
}

/// The UAV slots of a host root made for them.
pub(crate) fn table_slots(root: &ID3D12RootSignature) -> Option<std::sync::Arc<[super::shader_cache::slots::Slot]>> {
    TABLES.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&(windows::core::Interface::as_raw(root) as usize)).cloned()
}

/// The UAV of an SRV's resource over the same elements, mip and slices.
pub(crate) fn uav_desc(srv: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> Option<D3D12_UNORDERED_ACCESS_VIEW_DESC> {
    let mut uav = D3D12_UNORDERED_ACCESS_VIEW_DESC { Format: srv.Format, ..Default::default() };
    unsafe {
        let a = &srv.Anonymous;
        match srv.ViewDimension {
            D3D12_SRV_DIMENSION_BUFFER => {
                let b = a.Buffer;
                uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
                uav.Anonymous.Buffer = D3D12_BUFFER_UAV {
                    FirstElement: b.FirstElement, NumElements: b.NumElements, StructureByteStride: b.StructureByteStride,
                    CounterOffsetInBytes: 0,
                    Flags: if b.Flags.contains(D3D12_BUFFER_SRV_FLAG_RAW) { D3D12_BUFFER_UAV_FLAG_RAW } else { D3D12_BUFFER_UAV_FLAG_NONE },
                };
            }
            D3D12_SRV_DIMENSION_TEXTURE1D => {
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1D;
                uav.Anonymous.Texture1D = D3D12_TEX1D_UAV { MipSlice: a.Texture1D.MostDetailedMip };
            }
            D3D12_SRV_DIMENSION_TEXTURE1DARRAY => {
                let t = a.Texture1DArray;
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE1DARRAY;
                uav.Anonymous.Texture1DArray = D3D12_TEX1D_ARRAY_UAV { MipSlice: t.MostDetailedMip, FirstArraySlice: t.FirstArraySlice, ArraySize: t.ArraySize };
            }
            D3D12_SRV_DIMENSION_TEXTURE2D => {
                let t = a.Texture2D;
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
                uav.Anonymous.Texture2D = D3D12_TEX2D_UAV { MipSlice: t.MostDetailedMip, PlaneSlice: t.PlaneSlice };
            }
            D3D12_SRV_DIMENSION_TEXTURE2DARRAY => {
                let t = a.Texture2DArray;
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
                uav.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                    MipSlice: t.MostDetailedMip, FirstArraySlice: t.FirstArraySlice, ArraySize: t.ArraySize, PlaneSlice: t.PlaneSlice,
                };
            }
            D3D12_SRV_DIMENSION_TEXTURECUBE => {
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
                uav.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV { MipSlice: a.TextureCube.MostDetailedMip, FirstArraySlice: 0, ArraySize: 6, PlaneSlice: 0 };
            }
            D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => {
                let t = a.TextureCubeArray;
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2DARRAY;
                uav.Anonymous.Texture2DArray = D3D12_TEX2D_ARRAY_UAV {
                    MipSlice: t.MostDetailedMip, FirstArraySlice: t.First2DArrayFace, ArraySize: t.NumCubes.saturating_mul(6), PlaneSlice: 0,
                };
            }
            D3D12_SRV_DIMENSION_TEXTURE3D => {
                uav.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE3D;
                uav.Anonymous.Texture3D = D3D12_TEX3D_UAV { MipSlice: a.Texture3D.MostDetailedMip, FirstWSlice: 0, WSize: u32::MAX };
            }
            _ => return None,
        }
    }
    Some(uav)
}

/// SRV-as-UAV piece 4: a private copy of the guest table at `src` (`count`
/// slots) in `heap`'s ring, with each slot of `slots` for root parameter
/// `param` made a UAV of the SRV's resource. The GPU start of the copy;
/// None leaves the table as bound (no ring, or no slot of this table).
pub(crate) fn fill_table(
    heap: &ID3D12DescriptorHeap, src: usize, count: u32, param: u32,
    slots: &[super::shader_cache::slots::Slot],
) -> Option<u64> {
    let mine: Vec<_> = slots.iter().filter(|s| s.param == param && s.slot < count).collect();
    if mine.is_empty() {
        return None;
    }
    let mut device: Option<ID3D12Device> = None;
    unsafe { heap.GetDevice(&mut device) }.ok()?;
    let device = &device?;
    let (cpu, gpu, stride) = super::root_tables::private_slots(heap, count)?;
    unsafe {
        device.CopyDescriptorsSimple(count, D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu }, D3D12_CPU_DESCRIPTOR_HANDLE { ptr: src },
            D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    }
    for slot in mine {
        let at = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu + slot.slot as usize * stride };
        let made = get(src + slot.slot as usize * stride).and_then(|(resource, srv)| {
            let uav = uav_desc(&srv)?;
            let flags = unsafe { resource.GetDesc() }.Flags;
            if (flags & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS).0 == 0 {
                // Piece 5 gives such a resource a UAV-capable twin.
                note_once(windows::core::Interface::as_raw(&resource) as usize, "has no ALLOW_UNORDERED_ACCESS");
                return None;
            }
            unsafe { device.CreateUnorderedAccessView(&resource, None, Some(&uav), at) };
            Some(())
        });
        if made.is_none() {
            // Never an SRV read as a UAV: a null UAV of the slot's dimension.
            let null = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_R8G8B8A8_UNORM,
                ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D, ..Default::default()
            };
            unsafe { device.CreateUnorderedAccessView(None, None, Some(&null), at) };
        }
    }
    Some(gpu)
}

fn note_once(key: usize, what: &str) {
    static SEEN: Mutex<Option<std::collections::HashSet<usize>>> = Mutex::new(None);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(Default::default);
    if seen.len() < 256 && seen.insert(key) {
        crate::diagnostics::log_line(format_args!("srv as uav: resource {key:#x} {what}; its slot is a null UAV until it has a UAV twin"));
    }
}
