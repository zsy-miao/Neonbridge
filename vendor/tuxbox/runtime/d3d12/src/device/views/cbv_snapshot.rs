//! Constant buffers over remapped title pages read a copy made at view
//! creation. Ported from five-star-recomp 1ea5bce (GPL-3).
//!
//! An import of title memory keeps the physical pages it opened.
//! CopyPageMappingsBatchX remaps pages under GPU-imported ranges (page_mapping
//! logs it), so a CBV over such a range kept reading the old pages. five-star
//! saw the title's constant-pool pages remapped thousands of times in the
//! story load. CS_InitDepthPyramidWithCpuOcclusion3x3 then read old frames
//! and hit its loop guard. A CBV over a range with a remap under an import
//! now reads the constants as the CPU sees them when the view is made,
//! from an upload chunk. TUXBOX_REMAPPED_CBV_COPY=0 imports as before.
use super::{Device, TextureAddress};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC};
use windows::core::Result;

pub(super) fn on() -> bool {
    crate::env_toggle!("TUXBOX_REMAPPED_CBV_COPY") != Some("0")
}

const CHUNK: u64 = 4 << 20;

struct Chunk {
    owner: Arc<TextureAddress>,
    gpu: u64,
    mapped: usize,
    used: u64,
}

// SAFETY: the mapping stays valid while the resource lives; slices are
// handed out under the lock and never overlap.
unsafe impl Send for Chunk {}

static CURRENT: Mutex<Option<Chunk>> = Mutex::new(None);

fn new_chunk(device: &Device) -> Result<Chunk> {
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: CHUNK, Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
    };
    let mut native: Option<ID3D12Resource> = None;
    unsafe {
        device.native.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut native)?;
    }
    let native = native.ok_or(E_POINTER)?;
    crate::dred::name(&native, "tuxbox remapped CBV copies");
    let mut mapped = core::ptr::null_mut();
    unsafe { native.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 0 }), Some(&mut mapped))? };
    crate::diagnostics::memory::note(crate::diagnostics::memory::Category::ShadowImport, CHUNK);
    let gpu = unsafe { native.GetGPUVirtualAddress() };
    Ok(Chunk { owner: TextureAddress::unlisted(device, &native, 0), gpu, mapped: mapped as usize, used: 0 })
}

/// A copy of `bytes` title bytes at `address` in `size` host bytes (a
/// multiple of 256): the host GPU address and the chunk that holds it.
pub(super) fn copy(device: &Device, address: u64, bytes: u64, size: u64) -> Result<(u64, Arc<TextureAddress>)> {
    use windows::Win32::System::Diagnostics::Debug::ReadProcessMemory;
    use windows::Win32::System::Threading::GetCurrentProcess;
    let size = size.max(256);
    let mut current = CURRENT.lock().unwrap_or_else(|e| e.into_inner());
    if current.as_ref().is_none_or(|c| c.used + size > CHUNK) {
        *current = Some(new_chunk(device)?);
    }
    let chunk = current.as_mut().ok_or(E_POINTER)?;
    let at = chunk.used;
    chunk.used += size;
    let dst = (chunk.mapped + at as usize) as *mut u8;
    unsafe { core::ptr::write_bytes(dst, 0, size as usize) };
    // Fails rather than faults on pages the title decommitted; those read zero.
    let read = unsafe { ReadProcessMemory(GetCurrentProcess(), address as *const _, dst.cast(), bytes.min(size) as usize, None) };
    static COUNT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "CBV {address:#x}+{bytes} over remapped GPU-imported pages reads a copy made now{} ({n} so far; TUXBOX_REMAPPED_CBV_COPY=0 imports)",
            if read.is_err() { " (unreadable: zeros)" } else { "" }));
    }
    Ok((chunk.gpu + at, chunk.owner.clone()))
}
