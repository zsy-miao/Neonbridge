//! Console descriptor words per descriptor heap slot, for translated shaders
//! that read T#/V# words as data (SALU shifts and masks on descriptor
//! dwords: about 125 programs, xshader's "descriptors read as data"). Host
//! heap slots hold host views, so the words such a shader loads were never
//! the console's; this table gives it the GFX10 words a console driver would
//! have written for the same view. Kept like slot_views (same keys, copies
//! and heap lifetime).
//!
//! A stage that declares `ByteAddressBuffer tb_slot_words : register(t6,
//! space32766)` reads 32 bytes per slot of the bound shader-visible
//! CBV/SRV/UAV heap, indexed by the host slot (after the index map):
//! - buffer views: a GFX10 V# in dwords 0-3 (base lo; base hi[15:0] |
//!   stride<<16; num_records in bytes for raw/structured and elements for
//!   typed; dst_sel XYZW, 32_FLOAT, raw OOB select, resource level), 4-7 zero;
//! - texture views: a GFX10 T# in dwords 0-7 (base >> 8, format where known,
//!   width-1, height-1, depth or last array slice, base/last level, type,
//!   dst_sel from the view's component mapping); swizzle mode, pitch and
//!   metadata fields are zero (the host image has no console tiling);
//! - anything else, or an unknown address: zero.
//!
//! TUXBOX_SLOT_WORDS=0 leaves the binding out of every root signature.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Result;

/// `register(t6, space32766)` (t0 index map, t1 desc_table, t2
/// view_formats, t3 slot_views, t4 view marks / root strides, t5 bvh_raw).
pub(crate) const REGISTER: u32 = 6;
pub(crate) const SPACE: u32 = super::desc_table::SPACE;

pub(crate) type Words = [u32; 8];

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_SLOT_WORDS") != Some("0")
}

/// The translated stage declares `tb_slot_words`.
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && (lower..=upper).contains(&REGISTER)
        })
}

// GFX10 SQ_SEL values.
const SEL_0: u32 = 0;
const SEL_1: u32 = 1;
const SEL_X: u32 = 4;
const SEL_Y: u32 = 5;
const SEL_Z: u32 = 6;
const SEL_W: u32 = 7;
/// GFX10 FORMAT_32_FLOAT (buffer and image format tables share the low values).
const FORMAT_32_FLOAT: u32 = 22;

/// A buffer V# from a slot_views entry (address lo, hi, byte length, stride).
pub(crate) fn buffer(entry: [u32; 4], typed: bool) -> Words {
    let [lo, hi, length, stride] = entry;
    if lo == 0 && hi == 0 {
        return [0; 8];
    }
    let records = if typed && stride != 0 { length / stride } else { length };
    let sel = SEL_X | SEL_Y << 3 | SEL_Z << 6 | SEL_W << 9;
    // OOB_SELECT 3 (raw) for byte-addressed views, 0 (structured) otherwise; resource level 1.
    let oob = if typed { 0 } else { 3 };
    let word3 = sel | FORMAT_32_FLOAT << 12 | 1 << 24 | oob << 28;
    [lo, (hi & 0xffff) | (stride & 0x3fff) << 16, records, word3, 0, 0, 0, 0]
}

/// GFX10 image formats for the DXGI formats whose numbers are certain (the
/// shared low part of the table); 0 (invalid) for the rest.
fn image_format(format: DXGI_FORMAT) -> u32 {
    match format.0 {
        61 => 1,  // R8_UNORM: 8_UNORM
        63 => 2,  // R8_SNORM
        62 => 5,  // R8_UINT
        64 => 6,  // R8_SINT
        56 => 7,  // R16_UNORM
        58 => 8,  // R16_SNORM
        57 => 11, // R16_UINT
        59 => 12, // R16_SINT
        54 => 13, // R16_FLOAT
        49 => 14, // R8G8_UNORM: 8_8_UNORM
        51 => 15, // R8G8_SNORM
        50 => 18, // R8G8_UINT
        52 => 19, // R8G8_SINT
        42 => 20, // R32_UINT
        43 => 21, // R32_SINT
        41 => 22, // R32_FLOAT
        _ => 0,
    }
}

fn component(mapping: u32, i: u32) -> u32 {
    match (mapping >> (3 * i)) & 7 {
        0 => SEL_X,
        1 => SEL_Y,
        2 => SEL_Z,
        3 => SEL_W,
        4 => SEL_0,
        _ => SEL_1,
    }
}

/// An image T# for a texture SRV/UAV over title address `base`.
#[allow(clippy::too_many_arguments)]
fn image(base: u64, format: DXGI_FORMAT, resource: &D3D12_RESOURCE_DESC, kind: u32, levels: (u32, u32), slices: u32, mapping: u32) -> Words {
    if base == 0 || resource.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        return [0; 8];
    }
    let width = (resource.Width.max(1) - 1) as u32;
    let height = resource.Height.max(1) - 1;
    let (base_level, last_level) = levels;
    let sel = component(mapping, 0) | component(mapping, 1) << 3 | component(mapping, 2) << 6 | component(mapping, 3) << 9;
    [
        (base >> 8) as u32,
        ((base >> 40) as u32 & 0xff) | image_format(format) << 20 | (width & 3) << 30,
        (width >> 2) & 0x3fff | (height & 0xffff) << 14 | 1 << 31,
        sel | (base_level & 0xf) << 12 | (last_level & 0xf) << 16 | (kind & 0xf) << 28,
        slices.max(1) - 1 & 0x1fff,
        0,
        0,
        0,
    ]
}

/// GFX10 SQ_RSRC_IMG types.
const IMG_1D: u32 = 8;
const IMG_2D: u32 = 9;
const IMG_3D: u32 = 10;
const IMG_CUBE: u32 = 11;
const IMG_1D_ARRAY: u32 = 12;
const IMG_2D_ARRAY: u32 = 13;
const IMG_2D_MSAA: u32 = 14;
const IMG_2D_MSAA_ARRAY: u32 = 15;

fn last(most: u32, levels: u32, all: u16) -> (u32, u32) {
    let count = if levels == u32::MAX || levels == 0 { u32::from(all).saturating_sub(most).max(1) } else { levels };
    (most, most + count - 1)
}

/// The T# of a texture SRV, or zero for buffers and unknown views.
pub(crate) fn srv(base: u64, view: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>, resource: Option<&D3D12_RESOURCE_DESC>) -> Words {
    let (Some(v), Some(r)) = (view, resource) else { return [0; 8] };
    let mips = r.MipLevels;
    let depth = r.DepthOrArraySize as u32;
    let (kind, levels, slices) = unsafe {
        match v.ViewDimension {
            D3D12_SRV_DIMENSION_TEXTURE1D => { let t = v.Anonymous.Texture1D; (IMG_1D, last(t.MostDetailedMip, t.MipLevels, mips), 1) }
            D3D12_SRV_DIMENSION_TEXTURE1DARRAY => { let t = v.Anonymous.Texture1DArray; (IMG_1D_ARRAY, last(t.MostDetailedMip, t.MipLevels, mips), t.FirstArraySlice + t.ArraySize) }
            D3D12_SRV_DIMENSION_TEXTURE2D => { let t = v.Anonymous.Texture2D; (IMG_2D, last(t.MostDetailedMip, t.MipLevels, mips), 1) }
            D3D12_SRV_DIMENSION_TEXTURE2DARRAY => { let t = v.Anonymous.Texture2DArray; (IMG_2D_ARRAY, last(t.MostDetailedMip, t.MipLevels, mips), t.FirstArraySlice + t.ArraySize) }
            D3D12_SRV_DIMENSION_TEXTURE2DMS => (IMG_2D_MSAA, (0, 0), 1),
            D3D12_SRV_DIMENSION_TEXTURE2DMSARRAY => { let t = v.Anonymous.Texture2DMSArray; (IMG_2D_MSAA_ARRAY, (0, 0), t.FirstArraySlice + t.ArraySize) }
            D3D12_SRV_DIMENSION_TEXTURE3D => { let t = v.Anonymous.Texture3D; (IMG_3D, last(t.MostDetailedMip, t.MipLevels, mips), depth) }
            D3D12_SRV_DIMENSION_TEXTURECUBE => { let t = v.Anonymous.TextureCube; (IMG_CUBE, last(t.MostDetailedMip, t.MipLevels, mips), 6) }
            D3D12_SRV_DIMENSION_TEXTURECUBEARRAY => { let t = v.Anonymous.TextureCubeArray; (IMG_CUBE, last(t.MostDetailedMip, t.MipLevels, mips), 6 * t.NumCubes) }
            _ => return [0; 8],
        }
    };
    let format = if v.Format == DXGI_FORMAT_UNKNOWN { r.Format } else { v.Format };
    image(base, format, r, kind, levels, slices, v.Shader4ComponentMapping)
}

/// The T# of a texture UAV (identity swizzle, one level).
pub(crate) fn uav(base: u64, view: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>, resource: Option<&D3D12_RESOURCE_DESC>) -> Words {
    let (Some(v), Some(r)) = (view, resource) else { return [0; 8] };
    let identity = 0 | 1 << 3 | 2 << 6 | 3 << 9;
    let depth = r.DepthOrArraySize as u32;
    let (kind, level, slices) = unsafe {
        match v.ViewDimension {
            D3D12_UAV_DIMENSION_TEXTURE1D => (IMG_1D, v.Anonymous.Texture1D.MipSlice, 1),
            D3D12_UAV_DIMENSION_TEXTURE1DARRAY => { let t = v.Anonymous.Texture1DArray; (IMG_1D_ARRAY, t.MipSlice, t.FirstArraySlice + t.ArraySize) }
            D3D12_UAV_DIMENSION_TEXTURE2D => (IMG_2D, v.Anonymous.Texture2D.MipSlice, 1),
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => { let t = v.Anonymous.Texture2DArray; (IMG_2D_ARRAY, t.MipSlice, t.FirstArraySlice + t.ArraySize) }
            D3D12_UAV_DIMENSION_TEXTURE3D => (IMG_3D, v.Anonymous.Texture3D.MipSlice, depth),
            _ => return [0; 8],
        }
    };
    let format = if v.Format == DXGI_FORMAT_UNKNOWN { r.Format } else { v.Format };
    image(base, format, r, kind, (level, level), slices, identity)
}

/// A shader-visible heap's table.
struct Table {
    end: usize,
    stride: usize,
    buffer: ID3D12Resource,
    words: *mut Words,
}

// The mapped pointer stays valid while `buffer` lives; writes go through the lock.
unsafe impl Send for Table {}

fn slot(start: usize, end: usize, stride: usize, handle: usize) -> Option<usize> {
    let offset = handle.checked_sub(start)?;
    (handle < end && offset.is_multiple_of(stride)).then_some(offset / stride)
}

#[derive(Default)]
struct State {
    /// Nonzero words by host CPU descriptor handle.
    codes: BTreeMap<usize, Words>,
    /// Shader-visible heap tables by CPU start.
    tables: BTreeMap<usize, Table>,
}

impl State {
    fn set(&mut self, handle: usize, code: Words) {
        if code == [0; 8] {
            self.codes.remove(&handle);
        } else {
            self.codes.insert(handle, code);
        }
        if let Some((&start, table)) = self.tables.range(..=handle).next_back()
            && let Some(slot) = slot(start, table.end, table.stride, handle)
        {
            // In bounds: slot < count, and the buffer holds count entries.
            unsafe { table.words.add(slot).write_volatile(code) };
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State { codes: BTreeMap::new(), tables: BTreeMap::new() });
/// Any nonzero words were ever stored: copies before that skip the lock.
static ANY: AtomicBool = AtomicBool::new(false);

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// A view was created at `handle`.
pub(crate) fn write(handle: usize, code: Words) {
    if !enabled() || (code == [0; 8] && !ANY.load(Ordering::Relaxed)) {
        return;
    }
    if code != [0; 8] {
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
    let codes: Vec<Words> = (0..count).map(|i| state.codes.get(&(src + i * stride)).copied().unwrap_or([0; 8])).collect();
    for (i, code) in codes.into_iter().enumerate() {
        state.set(dst + i * stride, code);
    }
}

/// The GPU address of the table for the shader-visible heap at `start`,
/// created on first use from the words already stored for its slots.
pub(crate) fn address(device: &ID3D12Device, start: usize, count: u32, stride: usize) -> Result<(ID3D12Resource, u64)> {
    let mut state = state();
    let end = start + count.max(1) as usize * stride;
    if let Some(table) = state.tables.get(&start).filter(|t| t.end == end && t.stride == stride) {
        return Ok((table.buffer.clone(), unsafe { table.buffer.GetGPUVirtualAddress() }));
    }
    let bytes = u64::from(count.max(1)) * size_of::<Words>() as u64;
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
    let words = mapped.cast::<Words>();
    unsafe { std::ptr::write_bytes(words, 0, count.max(1) as usize) };
    for (&handle, &code) in state.codes.range(start..end) {
        if let Some(slot) = slot(start, end, stride, handle) {
            unsafe { words.add(slot).write(code) };
        }
    }
    state.tables.insert(start, Table { end, stride, buffer: buffer.clone(), words });
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "slot descriptor words table bound for a {count}-slot shader-visible heap (t6 space32766; TUXBOX_SLOT_WORDS=0 leaves it out)"));
    let address = unsafe { buffer.GetGPUVirtualAddress() };
    Ok((buffer, address))
}

/// Forgets the words and table of a released heap's handles.
pub(crate) fn forget(ranges: &[std::ops::Range<usize>]) {
    if !enabled() {
        return;
    }
    let mut state = state();
    for range in ranges {
        let keys: Vec<usize> = state.codes.range(range.clone()).map(|(&k, _)| k).collect();
        for key in keys {
            state.codes.remove(&key);
        }
        state.tables.remove(&range.start);
    }
}
