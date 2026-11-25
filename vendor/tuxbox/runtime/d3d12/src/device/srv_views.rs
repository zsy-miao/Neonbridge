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
#[allow(dead_code)]
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
#[allow(dead_code)]
pub(crate) fn table_slots(root: &ID3D12RootSignature) -> Option<std::sync::Arc<[super::shader_cache::slots::Slot]>> {
    TABLES.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(&(windows::core::Interface::as_raw(root) as usize)).cloned()
}
