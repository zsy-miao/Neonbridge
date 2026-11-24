//! Typed-buffer view formats for translated shaders (Research request,
//! confirmed with the shader translator).
//!
//! A translated stage that declares
//! `ByteAddressBuffer tb_view_formats : register(t2, space32766)` reads one
//! dword per slot of the bound shader-visible CBV/SRV/UAV heap, indexed by the
//! host slot (after the index map). The dword is `class | dxgi_format << 8`:
//! class 0 is float or unknown (the translator keeps its current path), 1 is
//! UINT and 2 is SINT. Only typed buffer SRVs and UAVs get a nonzero word.
//!
//! Codes follow descriptors: every view creation sets (or clears) the code of
//! its host CPU handle, every descriptor copy carries the codes along, and a
//! handle inside a registered shader-visible heap also writes the heap's
//! table, a persistently mapped upload buffer the root SRV points at. Like the
//! descriptors themselves, a slot the title rewrites while the GPU reads it
//! is the title's race.
//!
//! TUXBOX_TYPED_FMT_TABLE=0 leaves the binding out of every root signature.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Result;

/// The shader's binding: `register(t2, space32766)`.
pub(crate) const REGISTER: u32 = 2;
pub(crate) const SPACE: u32 = super::desc_table::SPACE;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_TYPED_FMT_TABLE") != Some("0")
}

/// The translated stage declares `tb_view_formats` (a PSV0 SRV binding at t2,
/// space32766).
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && (lower..=upper).contains(&REGISTER)
        })
}

const UINT: u32 = 1;
const SINT: u32 = 2;

fn class(format: DXGI_FORMAT) -> u32 {
    match format {
        DXGI_FORMAT_R32G32B32A32_UINT | DXGI_FORMAT_R32G32B32_UINT | DXGI_FORMAT_R16G16B16A16_UINT
        | DXGI_FORMAT_R32G32_UINT | DXGI_FORMAT_R10G10B10A2_UINT | DXGI_FORMAT_R8G8B8A8_UINT
        | DXGI_FORMAT_R16G16_UINT | DXGI_FORMAT_R32_UINT | DXGI_FORMAT_R8G8_UINT
        | DXGI_FORMAT_R16_UINT | DXGI_FORMAT_R8_UINT => UINT,
        DXGI_FORMAT_R32G32B32A32_SINT | DXGI_FORMAT_R32G32B32_SINT | DXGI_FORMAT_R16G16B16A16_SINT
        | DXGI_FORMAT_R32G32_SINT | DXGI_FORMAT_R8G8B8A8_SINT | DXGI_FORMAT_R16G16_SINT
        | DXGI_FORMAT_R32_SINT | DXGI_FORMAT_R8G8_SINT | DXGI_FORMAT_R16_SINT
        | DXGI_FORMAT_R8_SINT => SINT,
        _ => 0,
    }
}

/// The word for a view: nonzero only for a typed buffer view.
pub(crate) fn code(buffer: bool, format: DXGI_FORMAT) -> u32 {
    if !buffer || format == DXGI_FORMAT_UNKNOWN {
        return 0;
    }
    class(format) | (format.0 as u32) << 8
}

pub(crate) fn srv_code(desc: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>) -> u32 {
    desc.map_or(0, |v| code(v.ViewDimension == D3D12_SRV_DIMENSION_BUFFER, v.Format))
}

pub(crate) fn uav_code(desc: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>) -> u32 {
    desc.map_or(0, |v| code(v.ViewDimension == D3D12_UAV_DIMENSION_BUFFER, v.Format))
}

/// A shader-visible heap's table.
struct Table {
    end: usize,
    stride: usize,
    buffer: ID3D12Resource,
    words: *mut u32,
}

// The mapped pointer stays valid while `buffer` lives; writes go through the lock.
unsafe impl Send for Table {}

impl Table {
    fn slot(&self, start: usize, handle: usize) -> Option<usize> {
        slot(start, self.end, self.stride, handle)
    }
}

fn slot(start: usize, end: usize, stride: usize, handle: usize) -> Option<usize> {
    let offset = handle.checked_sub(start)?;
    (handle < end && offset.is_multiple_of(stride)).then_some(offset / stride)
}

#[derive(Default)]
struct State {
    /// Nonzero codes by host CPU descriptor handle.
    codes: BTreeMap<usize, u32>,
    /// Shader-visible heap tables by CPU start.
    tables: BTreeMap<usize, Table>,
}

impl State {
    fn set(&mut self, handle: usize, code: u32) {
        if code == 0 {
            self.codes.remove(&handle);
        } else {
            self.codes.insert(handle, code);
        }
        if let Some((&start, table)) = self.tables.range(..=handle).next_back()
            && let Some(slot) = table.slot(start, handle)
        {
            // In bounds: slot < count, and the buffer holds count dwords.
            unsafe { table.words.add(slot).write_volatile(code) };
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State { codes: BTreeMap::new(), tables: BTreeMap::new() });
/// Any nonzero code was ever stored: copies before that skip the lock.
static ANY: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A view was created at `handle`.
pub(crate) fn write(handle: usize, code: u32) {
    if !enabled() || (code == 0 && !ANY.load(Ordering::Relaxed)) {
        return;
    }
    if code != 0 {
        ANY.store(true, Ordering::Relaxed);
    }
    state().set(handle, code);
}

/// `count` descriptors were copied from `src` to `dst`.
pub(crate) fn copy(dst: usize, src: usize, count: usize, stride: usize) {
    if !enabled() || !ANY.load(Ordering::Relaxed) || count == 0 || dst == src {
        return;
    }
    let mut state = state();
    let codes: Vec<u32> = (0..count).map(|i| state.codes.get(&(src + i * stride)).copied().unwrap_or(0)).collect();
    for (i, code) in codes.into_iter().enumerate() {
        state.set(dst + i * stride, code);
    }
}

/// The GPU address of the table for the shader-visible heap at `start`,
/// created on first use from the codes already stored for its slots.
pub(crate) fn address(device: &ID3D12Device, start: usize, count: u32, stride: usize) -> Result<(ID3D12Resource, u64)> {
    let mut state = state();
    let end = start + count.max(1) as usize * stride;
    if let Some(table) = state.tables.get(&start).filter(|t| t.end == end && t.stride == stride) {
        return Ok((table.buffer.clone(), unsafe { table.buffer.GetGPUVirtualAddress() }));
    }
    let bytes = u64::from(count.max(1)) * 4;
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_UPLOAD, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    };
    let mut buffer = None::<ID3D12Resource>;
    unsafe {
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc,
            D3D12_RESOURCE_STATE_GENERIC_READ, None, &mut buffer)?;
    }
    let buffer = buffer.ok_or(E_POINTER)?;
    let mut mapped = std::ptr::null_mut();
    unsafe { buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut mapped))? };
    let words = mapped.cast::<u32>();
    unsafe { std::ptr::write_bytes(words, 0, count.max(1) as usize) };
    for (&handle, &code) in state.codes.range(start..end) {
        if let Some(slot) = slot(start, end, stride, handle) {
            unsafe { words.add(slot).write(code) };
        }
    }
    state.tables.insert(start, Table { end, stride, buffer: buffer.clone(), words });
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "typed view format table bound for a {count}-slot shader-visible heap (t2 space32766; TUXBOX_TYPED_FMT_TABLE=0 leaves it out)"));
    let address = unsafe { buffer.GetGPUVirtualAddress() };
    Ok((buffer, address))
}

/// Forgets the codes and table of a released heap's handles.
pub(crate) struct HeapGuard {
    ranges: Vec<std::ops::Range<usize>>,
}

impl HeapGuard {
    pub(crate) fn new(ranges: Vec<std::ops::Range<usize>>) -> Self {
        let guard = Self { ranges };
        guard.forget();
        guard
    }

    fn forget(&self) {
        if !enabled() {
            return;
        }
        let mut state = state();
        for range in &self.ranges {
            let keys: Vec<usize> = state.codes.range(range.clone()).map(|(&k, _)| k).collect();
            for key in keys {
                state.codes.remove(&key);
            }
            // Recorded lists hold their own reference to the buffer.
            state.tables.remove(&range.start);
        }
    }
}

impl Drop for HeapGuard {
    fn drop(&mut self) {
        self.forget();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_typed_buffer_views_carry_a_class_and_format() {
        assert_eq!(code(true, DXGI_FORMAT_R32_UINT), 1 | 42 << 8);
        assert_eq!(code(true, DXGI_FORMAT_R16G16_SINT), 2 | 38 << 8);
        assert_eq!(code(true, DXGI_FORMAT_R32G32B32A32_FLOAT), 2 << 8);
        assert_eq!(code(true, DXGI_FORMAT_UNKNOWN), 0);
        assert_eq!(code(false, DXGI_FORMAT_R32_UINT), 0);
    }

    #[test]
    fn a_table_slot_needs_an_aligned_handle_inside_the_heap() {
        let end = 0x1000 + 4 * 32;
        assert_eq!(slot(0x1000, end, 32, 0x1000 + 3 * 32), Some(3));
        assert_eq!(slot(0x1000, end, 32, end), None);
        assert_eq!(slot(0x1000, end, 32, 0x1001), None);
        assert_eq!(slot(0x1000, end, 32, 0x0fff), None);
    }
}
