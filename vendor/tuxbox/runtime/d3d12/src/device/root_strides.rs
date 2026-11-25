//! TUXBOX_ROOT_RAW_STRIDE (default on, =0 off): a translated compute stage
//! that declares an SRV at t4 space32766 (xshader's
//! `ByteAddressBuffer tb_root_raw_stride`) gets one root SRV over a table of
//! u32 strides for its guest root raw views, so an indexed raw access (IDXEN)
//! can scale its index by the console descriptor's stride. The word at byte
//! (register + 1024 * uav) * 4 is the stride of the view the title bound at
//! that root SRV (t) or UAV (u) register, from the recorded buffer view whose
//! range holds its address (slot_views::stride_at); 0 when unknown (read as
//! bytes). Tables are kept by content for the process's life.
use std::collections::HashMap;
use std::sync::Mutex;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::core::Interface;

pub(crate) const REGISTER: u32 = 4;
pub(crate) const SPACE: u32 = super::desc_table::SPACE;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_ROOT_RAW_STRIDE") != Some("0")
}

/// The stage declares an SRV binding covering t4 in space32766.
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && (lower..=upper).contains(&REGISTER)
        })
}

/// The table for (key, stride) pairs: its buffer and GPU address.
pub(crate) fn table(owner: &windows::core::IUnknown, keys: &[(u32, u32)]) -> Option<(ID3D12Resource, u64)> {
    static TABLES: Mutex<Option<HashMap<Vec<u32>, (ID3D12Resource, u64)>>> = Mutex::new(None);
    let words = keys.iter().map(|&(key, _)| key as usize + 1).max().unwrap_or(1).min(2048);
    let mut content = vec![0u32; words];
    for &(key, stride) in keys {
        if let Some(word) = content.get_mut(key as usize) {
            *word = stride;
        }
    }
    let mut tables = TABLES.lock().unwrap_or_else(|e| e.into_inner());
    let tables = tables.get_or_insert_with(HashMap::new);
    if let Some(found) = tables.get(&content) {
        return Some(found.clone());
    }
    let device: ID3D12Device = owner.cast().ok()?;
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: (words * 4).max(16) as u64, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let mut buffer = None::<ID3D12Resource>;
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer) }.ok()?;
    let buffer = buffer?;
    let mut mapped = std::ptr::null_mut();
    unsafe { buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped)) }.ok()?;
    unsafe { std::ptr::copy_nonoverlapping(content.as_ptr(), mapped.cast::<u32>(), words) };
    let address = unsafe { buffer.GetGPUVirtualAddress() };
    if tables.len() == 0 {
        crate::diagnostics::log_line(format_args!(
            "root raw strides: first t4 space32766 table, {} root raw views, strides {:?} (TUXBOX_ROOT_RAW_STRIDE=0 off)",
            keys.len(), keys));
    }
    tables.insert(content, (buffer.clone(), address));
    Some((buffer, address))
}
