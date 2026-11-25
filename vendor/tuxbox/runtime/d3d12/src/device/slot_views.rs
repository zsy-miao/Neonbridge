//! Buffer view addresses per descriptor heap slot, so a translated shader
//! that computes a heap index can read the title GPU address of the view
//! there. Runtime half of VItoriaBr b62bbc0 (SLOT_VIEWS; GPL-3.0, credit to
//! its authors), keyed and maintained exactly like view_formats.
//!
//! A stage that declares `ByteAddressBuffer tb_slot_views : register(t3,
//! space32766)` reads 16 bytes per slot of the bound shader-visible
//! CBV/SRV/UAV heap, indexed by the host slot (after the index map), like
//! tb_view_formats: u32 address low, u32 address high (the title GPU VA of
//! the view's first element), u32 byte length, u32 element stride. Only
//! buffer SRVs and UAVs get a nonzero entry; other views and null or cleared
//! slots are zero.
//!
//! TUXBOX_SLOT_VIEWS=0 leaves the binding out of every root signature.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::Result;

/// The shader's binding: `register(t3, space32766)` (t0 index map, t1
/// desc_table, t2 view_formats).
pub(crate) const REGISTER: u32 = 3;
pub(crate) const SPACE: u32 = super::desc_table::SPACE;

/// address lo, address hi, byte length, element stride.
pub(crate) type Entry = [u32; 4];

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_SLOT_VIEWS") != Some("0")
}

/// The translated stage declares `tb_slot_views` (a PSV0 SRV binding at t3,
/// space32766).
pub(crate) fn declared(code: &[u8]) -> bool {
    enabled()
        && super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
            matches!(resource, 3..=5) && space == SPACE && (lower..=upper).contains(&REGISTER)
        })
}

/// The entry of a buffer view over title address `base` (0: unknown, no entry).
pub(crate) fn entry(base: u64, format: DXGI_FORMAT, structure_stride: u32, raw: bool, first: u64, elements: u32) -> Entry {
    let stride = if structure_stride != 0 { Some(structure_stride) }
        else if raw { Some(4) }
        else { super::views::element_stride(format.0, 0) };
    let Some(stride) = stride.filter(|_| base != 0) else { return [0; 4] };
    let address = base.wrapping_add(first.wrapping_mul(u64::from(stride)));
    let length = u64::from(elements) * u64::from(stride);
    // A structured view made raw from below its start carries the byte bias
    // in bits 24-31 of the stride word (raw_views::bias_on).
    let bias = if structure_stride != 0 && !raw { super::raw_views::bias(first, elements, structure_stride).unwrap_or(0) } else { 0 };
    [address as u32, (address >> 32) as u32, length.min(u64::from(u32::MAX)) as u32, stride | bias << 24]
}

pub(crate) fn srv_entry(base: u64, desc: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>) -> Entry {
    match desc {
        Some(v) if v.ViewDimension == D3D12_SRV_DIMENSION_BUFFER => {
            let b = unsafe { v.Anonymous.Buffer };
            entry(base, v.Format, b.StructureByteStride, b.Flags.contains(D3D12_BUFFER_SRV_FLAG_RAW), b.FirstElement, b.NumElements)
        }
        _ => [0; 4],
    }
}

/// A UAV with a counter stays structured (raw_views::uav), so its slot
/// carries no raw bias.
pub(crate) fn counted(mut entry: Entry, counter: bool) -> Entry {
    if counter {
        entry[3] &= 0x00ff_ffff;
    }
    entry
}

pub(crate) fn uav_entry(base: u64, desc: Option<&D3D12_UNORDERED_ACCESS_VIEW_DESC>) -> Entry {
    match desc {
        Some(v) if v.ViewDimension == D3D12_UAV_DIMENSION_BUFFER => {
            let b = unsafe { v.Anonymous.Buffer };
            entry(base, v.Format, b.StructureByteStride, b.Flags.contains(D3D12_BUFFER_UAV_FLAG_RAW), b.FirstElement, b.NumElements)
        }
        _ => [0; 4],
    }
}

/// The title GPU address of a resource proxy, without registering one
/// (a native buffer counts once the title has asked for its address).
pub(crate) fn title_base(resource: Option<&super::Resource>) -> u64 {
    let Some(resource) = resource.filter(|_| enabled()) else { return 0 };
    if let super::Backing::Addressed(address, _) = &resource.state.backing { return address.base as u64; }
    if let Some(placed) = resource.state.placed() { return placed.address; }
    resource.state.texture_address.get().map_or(0, |a| a.base as u64)
}

/// A shader-visible heap's table.
struct Table {
    end: usize,
    stride: usize,
    buffer: ID3D12Resource,
    words: *mut [u32; 4],
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
    /// Nonzero entries by host CPU descriptor handle.
    codes: BTreeMap<usize, Entry>,
    /// Shader-visible heap tables by CPU start.
    tables: BTreeMap<usize, Table>,
}

impl State {
    fn set(&mut self, handle: usize, code: Entry) {
        if code == [0; 4] {
            self.codes.remove(&handle);
        } else {
            self.codes.insert(handle, code);
        }
        if let Some((&start, table)) = self.tables.range(..=handle).next_back()
            && let Some(slot) = table.slot(start, handle)
        {
            // In bounds: slot < count, and the buffer holds count entries.
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

/// The element stride of a recorded buffer view whose range holds title
/// address `va` (root_strides); 0 when none does. The newest match wins.
pub(crate) fn stride_at(va: u64) -> u32 {
    use std::collections::HashMap;
    static CACHE: Mutex<Option<HashMap<u64, u32>>> = Mutex::new(None);
    if va == 0 || !ANY.load(Ordering::Relaxed) {
        return 0;
    }
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cache.get_or_insert_with(HashMap::new);
    if let Some(&stride) = cache.get(&va) {
        return stride;
    }
    let stride = state().codes.values().rev()
        .find(|[lo, hi, length, _]| {
            let start = u64::from(*lo) | u64::from(*hi) << 32;
            start <= va && va < start + u64::from((*length).max(1))
        })
        .map_or(0, |e| e[3]);
    if cache.len() >= 1 << 16 {
        cache.clear();
    }
    cache.insert(va, stride);
    stride
}

/// The entry of the view at `handle` (title address, length, stride), if any.
pub(crate) fn get(handle: usize) -> Option<Entry> {
    state().codes.get(&handle).copied()
}

/// A view was created at `handle`.
pub(crate) fn write(handle: usize, code: Entry) {
    // Its V# for shaders reading descriptor words as data (slot_words); a
    // texture's T# is written after this by the view's own call.
    super::slot_words::write(handle, super::slot_words::buffer(code, code[3] != 4));
    if !enabled() || (code == [0; 4] && !ANY.load(Ordering::Relaxed)) {
        return;
    }
    if code != [0; 4] {
        ANY.store(true, Ordering::Relaxed);
    }
    state().set(handle, code);
}

/// `count` descriptors were copied from `src` to `dst`.
pub(crate) fn copy(dst: usize, src: usize, count: usize, stride: usize) {
    super::slot_words::copy(dst, src, count, stride);
    if !enabled() || !ANY.load(Ordering::Relaxed) || count == 0 || dst == src {
        return;
    }
    let mut state = state();
    let codes: Vec<Entry> = (0..count).map(|i| state.codes.get(&(src + i * stride)).copied().unwrap_or([0; 4])).collect();
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
    let bytes = u64::from(count.max(1)) * 16;
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
    let words = mapped.cast::<[u32; 4]>();
    unsafe { std::ptr::write_bytes(words, 0, count.max(1) as usize) };
    for (&handle, &code) in state.codes.range(start..end) {
        if let Some(slot) = slot(start, end, stride, handle) {
            unsafe { words.add(slot).write(code) };
        }
    }
    state.tables.insert(start, Table { end, stride, buffer: buffer.clone(), words });
    static FIRST: std::sync::Once = std::sync::Once::new();
    crate::diagnostics::log_once(&FIRST, || format!(
        "slot view address table bound for a {count}-slot shader-visible heap (t3 space32766; TUXBOX_SLOT_VIEWS=0 leaves it out)"));
    let address = unsafe { buffer.GetGPUVirtualAddress() };
    Ok((buffer, address))
}

/// Forgets the entries and table of a released heap's handles (called with
/// view_formats::HeapGuard's ranges).
pub(crate) fn forget(ranges: &[std::ops::Range<usize>]) {
    super::slot_words::forget(ranges);
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
