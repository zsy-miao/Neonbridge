//! The runtime half of xshader's literal sampler lookup (xshader 96c7e03).
//!
//! A translated shader whose literal S# words match no static sampler reads
//! `ByteAddressBuffer tb_desc_table : register(t1, space32766)`: word 0 is the
//! mask, then mask + 1 entries of ten dwords {key[8], sampler heap index,
//! 0xA11CA7ED when occupied}. The key is the four S# words followed by four
//! 0xFFFFFFFF. The shader hashes the key with FNV-1a per dword and compares
//! the eight slots from `hash & mask`; a hole does not end the probe and a
//! miss returns index 0 (leonida runtime/src/descriptor_table.h).
//!
//! Two sources fill the table:
//! - samplers the title creates or copies into a shader-visible sampler heap,
//!   keyed by the S# this module encodes for them (Mesa's GFX10 layout);
//! - literal S# words of each translated shader, decoded into host samplers
//!   that this module writes into slots it reserves at the top of every
//!   shader-visible sampler heap (host heaps are created at the D3D12 maximum,
//!   the title's descriptors stay at the bottom).
//!
//! Sampler heaps are never title memory (only CBV/SRV/UAV heaps become
//! descriptor arenas), so no S# words are written into heap slots.
//!
//! The table reaches the GPU through one default-heap buffer that each queue
//! submission refreshes when the table changed. A table that outgrows the
//! buffer gets a new one; the old buffer is retired, not released, because
//! lists recorded against it may still be pending. Doubling bounds the
//! retired bytes by the live buffer's size.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
use windows::core::{Interface, Result};

/// The shader's binding: `register(t1, space32766)`.
pub(crate) const REGISTER: u32 = tuxbox_dxbc::reserved::DESC_TABLE.register;
pub(crate) const SPACE: u32 = tuxbox_dxbc::reserved::DESC_TABLE.space;
const PROBES: u32 = 8;
const OCCUPIED: u32 = 0xA11C_A7ED;
const ENTRY: usize = 10;
/// D3D12_MAX_SHADER_VISIBLE_SAMPLER_HEAP_SIZE.
pub(crate) const SAMPLER_HEAP_SLOTS: u32 = 2048;

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) type Key = [u32; 8];

/// Keys hold canonical S# words, so a title sampler and the shader's literal
/// or computed words agree on border and reserved bits.
pub(crate) fn sampler_key(words: [u32; 4]) -> Key {
    let words = tuxbox_dxbc::inline_sampler::canonical_sampler(words);
    [words[0], words[1], words[2], words[3], u32::MAX, u32::MAX, u32::MAX, u32::MAX]
}

fn hash(key: &Key) -> u32 {
    key.iter().fold(2_166_136_261u32, |h, &k| (h ^ k).wrapping_mul(16_777_619))
}

/// The host copy of the shader-visible table.
struct Table {
    mask: u32,
    entries: Vec<[u32; ENTRY]>,
    used: usize,
    /// Bumped on every change, so a submission uploads only after one.
    version: u64,
}

impl Default for Table {
    fn default() -> Self {
        Self { mask: 15, entries: vec![[0; ENTRY]; 16], used: 0, version: 0 }
    }
}

impl Table {
    fn probe(&self, key: &Key) -> impl Iterator<Item = usize> + '_ {
        let h = hash(key);
        (0..PROBES).map(move |k| (h.wrapping_add(k) & self.mask) as usize)
    }

    fn find(&self, key: &Key) -> Option<usize> {
        self.probe(key).find(|&slot| {
            let e = &self.entries[slot];
            e[9] == OCCUPIED && e[..8] == key[..]
        })
    }

    fn lookup(&self, key: &Key) -> Option<u32> {
        self.find(key).map(|slot| self.entries[slot][8])
    }

    /// Map `key` to `index`. An existing key keeps its index unless `replace`.
    fn insert(&mut self, key: &Key, index: u32, replace: bool) {
        if let Some(slot) = self.find(key) {
            if replace && self.entries[slot][8] != index {
                self.entries[slot][8] = index;
                self.version += 1;
            }
            return;
        }
        loop {
            // Load stays at or under 50%; a full probe window doubles too.
            let hole = self.probe(key).find(|&s| self.entries[s][9] != OCCUPIED);
            if (self.used + 1) * 2 <= self.entries.len()
                && let Some(slot) = hole
            {
                let entry = &mut self.entries[slot];
                entry[..8].copy_from_slice(key);
                entry[8] = index;
                entry[9] = OCCUPIED;
                self.used += 1;
                self.version += 1;
                return;
            }
            self.grow();
        }
    }

    fn erase(&mut self, key: &Key) {
        if let Some(slot) = self.find(key) {
            self.entries[slot] = [0; ENTRY];
            self.used -= 1;
            self.version += 1;
        }
    }

    fn grow(&mut self) {
        let mut size = self.entries.len() * 2;
        'retry: loop {
            let mask = size as u32 - 1;
            let mut entries = vec![[0u32; ENTRY]; size];
            for old in self.entries.iter().filter(|e| e[9] == OCCUPIED) {
                let key: Key = old[..8].try_into().unwrap();
                let h = hash(&key);
                let Some(slot) = (0..PROBES)
                    .map(|k| (h.wrapping_add(k) & mask) as usize)
                    .find(|&s| entries[s][9] != OCCUPIED)
                else {
                    size *= 2;
                    continue 'retry;
                };
                entries[slot] = *old;
            }
            self.mask = mask;
            self.entries = entries;
            self.version += 1;
            return;
        }
    }

    fn bytes(&self) -> u64 {
        4 * (1 + self.entries.len() as u64 * ENTRY as u64)
    }

    fn serialize(&self, out: &mut [u32]) {
        out[0] = self.mask;
        for (chunk, entry) in out[1..].chunks_exact_mut(ENTRY).zip(&self.entries) {
            chunk.copy_from_slice(entry);
        }
    }
}

// ---- S#: SQ_IMG_SAMP_WORD0..3 as Mesa's ac_build_sampler_descriptor fills it for GFX10 ----
//   WORD0  CLAMP_X [2:0], CLAMP_Y [5:3], CLAMP_Z [8:6], MAX_ANISO_RATIO [11:9], DEPTH_COMPARE_FUNC [14:12],
//          FORCE_UNNORMALIZED [15], FILTER_MODE [30:29]
//   WORD1  MIN_LOD [11:0], MAX_LOD [23:12]            (4.8 fixed, clamped to 0..15)
//   WORD2  LOD_BIAS [13:0] (signed 5.8, clamped to -32..31), XY_MAG_FILTER [21:20], XY_MIN_FILTER [23:22],
//          MIP_FILTER [27:26]
//   WORD3  BORDER_COLOR_PTR [11:0], BORDER_COLOR_TYPE [31:30]
// (leonida runtime/src/gfx10_descriptors.h:494-528.) Tuning fields are left zero.

fn field(value: u32, low: u32, width: u32) -> u32 {
    (value & ((1 << width) - 1)) << low
}

fn get(word: u32, low: u32, width: u32) -> u32 {
    (word >> low) & ((1 << width) - 1)
}

fn clamped(value: f32, low: f32, high: f32) -> f32 {
    if value > low { if value < high { value } else { high } } else { low }
}

/// D3D12_TEXTURE_ADDRESS_MODE to SQ_TEX_CLAMP.
fn sq_clamp(mode: D3D12_TEXTURE_ADDRESS_MODE) -> u32 {
    match mode {
        D3D12_TEXTURE_ADDRESS_MODE_MIRROR => 1,
        D3D12_TEXTURE_ADDRESS_MODE_CLAMP => 2,
        D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE => 3,
        D3D12_TEXTURE_ADDRESS_MODE_BORDER => 6,
        _ => 0,
    }
}

/// SQ_TEX_CLAMP to D3D12_TEXTURE_ADDRESS_MODE.
fn d3d_clamp(mode: u32) -> D3D12_TEXTURE_ADDRESS_MODE {
    match mode {
        1 => D3D12_TEXTURE_ADDRESS_MODE_MIRROR,
        2 | 4 => D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        3 | 5 | 7 => D3D12_TEXTURE_ADDRESS_MODE_MIRROR_ONCE,
        6 => D3D12_TEXTURE_ADDRESS_MODE_BORDER,
        _ => D3D12_TEXTURE_ADDRESS_MODE_WRAP,
    }
}

/// The GFX10 S# for a D3D12 sampler description.
pub(crate) fn encode(desc: &D3D12_SAMPLER_DESC) -> [u32; 4] {
    let filter = desc.Filter.0 as u32;
    let reduction = (filter >> 7) & 3;
    let aniso = filter & 0x40 != 0;
    let linear = |bit: u32| (filter >> bit) & 1;
    let (mag, min) = if aniso { (3, 3) } else { (linear(2), linear(4)) };
    let mip = if linear(0) != 0 { 2 } else { 1 };
    let ratio = if aniso { (31 - desc.MaxAnisotropy.clamp(1, 16).leading_zeros()).min(4) } else { 0 };
    let compare = if reduction == 1 { (desc.ComparisonFunc.0 as u32).clamp(1, 8) - 1 } else { 0 };
    let filter_mode = match reduction { 2 => 1, 3 => 2, _ => 0 };
    let border = match desc.BorderColor {
        [0.0, 0.0, 0.0, 0.0] => 0,
        [0.0, 0.0, 0.0, 1.0] => 1,
        [1.0, 1.0, 1.0, 1.0] => 2,
        _ => 3,
    };
    [
        field(sq_clamp(desc.AddressU), 0, 3) | field(sq_clamp(desc.AddressV), 3, 3)
            | field(sq_clamp(desc.AddressW), 6, 3) | field(ratio, 9, 3) | field(compare, 12, 3)
            | field(filter_mode, 29, 2),
        field((clamped(desc.MinLOD, 0.0, 15.0) * 256.0) as u32, 0, 12)
            | field((clamped(desc.MaxLOD, 0.0, 15.0) * 256.0) as u32, 12, 12),
        field((clamped(desc.MipLODBias, -32.0, 31.0) * 256.0) as i32 as u32, 0, 14)
            | field(mag, 20, 2) | field(min, 22, 2) | field(mip, 26, 2),
        field(border, 30, 2),
    ]
}

/// A host sampler for literal S# words. Every bit pattern decodes: fields
/// D3D12 cannot express (unnormalized coordinates, a border colour table
/// entry, the tuning fields) fall back to the nearest ordinary state.
pub(crate) fn decode(w: [u32; 4]) -> D3D12_SAMPLER_DESC {
    let compare = get(w[0], 12, 3);
    let reduction = match get(w[0], 29, 2) { 1 => 2, 2 => 3, _ => u32::from(compare != 0) };
    let (mag, min, mip) = (get(w[2], 20, 2), get(w[2], 22, 2), get(w[2], 26, 2));
    let aniso = mag >= 2 || min >= 2;
    // The mip filter stands with anisotropy too: 0x54 is ANISOTROPIC with a
    // point mip (VIctory 170ab23); features::host_filter falls back to 0x55.
    let filter = if aniso { 0x54 | u32::from(mip == 2) } else { (min & 1) << 4 | (mag & 1) << 2 | u32::from(mip == 2) };
    let min_lod = get(w[1], 0, 12) as f32 / 256.0;
    // MIP_FILTER none samples the base level only.
    let max_lod = if mip == 0 { min_lod } else { get(w[1], 12, 12) as f32 / 256.0 };
    let bias = ((get(w[2], 0, 14) << 18) as i32 >> 18) as f32 / 256.0;
    D3D12_SAMPLER_DESC {
        Filter: D3D12_FILTER((filter | reduction << 7) as i32),
        AddressU: d3d_clamp(get(w[0], 0, 3)),
        AddressV: d3d_clamp(get(w[0], 3, 3)),
        AddressW: d3d_clamp(get(w[0], 6, 3)),
        MipLODBias: bias,
        MaxAnisotropy: if aniso { 1 << get(w[0], 9, 3).clamp(1, 4) } else { 1 },
        ComparisonFunc: D3D12_COMPARISON_FUNC(if compare != 0 { compare as i32 + 1 } else { D3D12_COMPARISON_FUNC_NEVER.0 }),
        BorderColor: match get(w[3], 30, 2) {
            1 => [0.0, 0.0, 0.0, 1.0],
            2 => [1.0; 4],
            _ => [0.0; 4],
        },
        MinLOD: min_lod,
        MaxLOD: max_lod.max(min_lod),
    }
}

/// A shader-visible sampler heap: (host CPU start, the title's descriptor count).
struct VisibleHeap {
    cpu: usize,
    count: u32,
}

#[derive(Default)]
struct Samplers {
    heaps: Vec<VisibleHeap>,
    /// The S# of each sampler written at a CPU handle, in any sampler heap.
    written: HashMap<usize, [u32; 4]>,
    /// Literal S# words and the reserved slot holding their host sampler.
    private: HashMap<[u32; 4], (u32, D3D12_SAMPLER_DESC)>,
    stride: usize,
}

impl Samplers {
    /// The lowest reserved slot so far; reserved slots count down from the top.
    fn next_private(&self) -> Option<u32> {
        let floor = self.heaps.iter().map(|h| h.count).max().unwrap_or(0);
        let slot = SAMPLER_HEAP_SLOTS.checked_sub(1 + self.private.len() as u32)?;
        (slot >= floor).then_some(slot)
    }

    fn visible(&self, handle: usize) -> Option<u32> {
        self.heaps.iter().find_map(|h| {
            let offset = handle.checked_sub(h.cpu)?;
            let index = (offset / self.stride.max(1)) as u32;
            (offset % self.stride.max(1) == 0 && index < h.count).then_some(index)
        })
    }
}

struct Recorder {
    kind: D3D12_COMMAND_LIST_TYPE,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    value: u64,
    // ExecuteCommandLists does not retain resources until completion.
    held: Vec<ID3D12Resource>,
}

#[derive(Default)]
struct Gpu {
    buffer: Option<(ID3D12Resource, u64)>,
    retired: Vec<ID3D12Resource>,
    uploaded: Option<u64>,
    recorders: Vec<Recorder>,
}

#[derive(Default)]
pub(crate) struct DescTable {
    table: Mutex<Table>,
    samplers: Mutex<Samplers>,
    gpu: Mutex<Gpu>,
}

/// Unregisters a sampler heap when the title releases it.
pub(crate) struct HeapGuard {
    owner: Arc<DescTable>,
    cpu: usize,
    end: usize,
}

impl Drop for HeapGuard {
    fn drop(&mut self) {
        let mut samplers = lock(&self.owner.samplers);
        samplers.heaps.retain(|h| h.cpu != self.cpu);
        let (cpu, end) = (self.cpu, self.end);
        samplers.written.retain(|&handle, _| !(cpu..end).contains(&handle));
    }
}

fn buffer(width: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: width, Height: 1, DepthOrArraySize: 1,
        MipLevels: 1, Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, ..Default::default()
    }
}

fn committed(device: &ID3D12Device, kind: D3D12_HEAP_TYPE, width: u64, state: D3D12_RESOURCE_STATES) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES { Type: kind, CreationNodeMask: 1, VisibleNodeMask: 1, ..Default::default() };
    let mut out: Option<ID3D12Resource> = None;
    unsafe { device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &buffer(width), state, None, &mut out)? };
    let out = out.ok_or(E_POINTER)?;
    crate::diagnostics::memory::live_on(&out, crate::diagnostics::memory::Category::DescTable, width);
    Ok(out)
}

impl DescTable {
    /// Register a sampler heap. A shader-visible one has a host heap of
    /// [`SAMPLER_HEAP_SLOTS`] descriptors; its reserved slots are filled.
    pub(crate) fn sampler_heap(self: &Arc<Self>, device: &ID3D12Device, cpu: usize, count: u32, visible: bool) -> HeapGuard {
        let mut samplers = lock(&self.samplers);
        if samplers.stride == 0 {
            samplers.stride = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER) } as usize;
        }
        let stride = samplers.stride;
        let end = cpu + if visible { SAMPLER_HEAP_SLOTS } else { count } as usize * stride;
        // A new heap may reuse a released heap's addresses.
        samplers.written.retain(|&handle, _| !(cpu..end).contains(&handle));
        if visible {
            for (slot, desc) in samplers.private.values().filter(|(slot, _)| *slot >= count) {
                unsafe { device.CreateSampler(desc, D3D12_CPU_DESCRIPTOR_HANDLE { ptr: cpu + *slot as usize * stride }) };
            }
            samplers.heaps.push(VisibleHeap { cpu, count });
        }
        HeapGuard { owner: self.clone(), cpu, end }
    }

    /// The title wrote `words` at `handle`. A shader-visible slot maps the key
    /// to that slot unless the key already has one; the previous sampler at
    /// that slot loses the mapping.
    fn wrote(&self, samplers: &mut Samplers, handle: usize, words: Option<[u32; 4]>) {
        let previous = match words {
            Some(words) => samplers.written.insert(handle, words),
            None => samplers.written.remove(&handle),
        };
        let Some(index) = samplers.visible(handle) else { return };
        let mut table = lock(&self.table);
        if let Some(old) = previous.filter(|old| Some(*old) != words) {
            let key = sampler_key(old);
            if table.lookup(&key) == Some(index) && !samplers.private.contains_key(&old) {
                table.erase(&key);
            }
        }
        if let Some(words) = words {
            table.insert(&sampler_key(words), index, false);
        }
    }

    /// `ID3D12Device::CreateSampler` at `handle`.
    pub(crate) fn created(&self, handle: usize, desc: &D3D12_SAMPLER_DESC) {
        let mut samplers = lock(&self.samplers);
        self.wrote(&mut samplers, handle, Some(encode(desc)));
    }

    /// A sampler descriptor copy of `count` handles.
    pub(crate) fn copied(&self, dst: usize, src: usize, count: u32, stride: usize) {
        let mut samplers = lock(&self.samplers);
        for i in 0..count as usize {
            let words = samplers.written.get(&(src + i * stride)).copied();
            self.wrote(&mut samplers, dst + i * stride, words);
        }
    }

    /// Make each literal S# resolvable: a host sampler in a reserved slot of
    /// every shader-visible sampler heap, and the key mapped to that slot.
    pub(crate) fn literals(&self, device: &ID3D12Device, words: &[[u32; 4]]) {
        let mut samplers = lock(&self.samplers);
        if samplers.stride == 0 {
            samplers.stride = unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER) } as usize;
        }
        for &words in words {
            if samplers.private.contains_key(&words) { continue; }
            let Some(slot) = samplers.next_private() else {
                static FULL: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&FULL, || "literal sampler slots exhausted; tb_desc_table lookups fall back to index 0".into());
                return;
            };
            let mut desc = decode(words);
            desc.Filter = super::features::host_filter(device, desc.Filter);
            for heap in samplers.heaps.iter().filter(|h| h.count <= slot) {
                unsafe { device.CreateSampler(&desc, D3D12_CPU_DESCRIPTOR_HANDLE { ptr: heap.cpu + slot as usize * samplers.stride }) };
            }
            samplers.private.insert(words, (slot, desc));
            lock(&self.table).insert(&sampler_key(words), slot, true);
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!("literal sampler {words:08x?} in reserved sampler slot {slot}"));
        }
    }

    /// The table's GPU address for a draw or dispatch being recorded. The
    /// buffer is current for the table's size; its contents arrive with the
    /// next submission's [`Self::flush`].
    pub(crate) fn address(&self, device: &ID3D12Device) -> Result<u64> {
        let mut gpu = lock(&self.gpu);
        let bytes = lock(&self.table).bytes();
        Ok(unsafe { Self::current(&mut gpu, device, bytes)?.GetGPUVirtualAddress() })
    }

    fn current<'a>(gpu: &'a mut Gpu, device: &ID3D12Device, bytes: u64) -> Result<&'a ID3D12Resource> {
        if gpu.buffer.as_ref().is_none_or(|(_, size)| *size < bytes) {
            // Zero-filled: mask 0 and no occupied entry, so lookups miss.
            let fresh = committed(device, D3D12_HEAP_TYPE_DEFAULT, bytes, D3D12_RESOURCE_STATE_COMMON)?;
            if let Some((old, _)) = gpu.buffer.replace((fresh, bytes)) {
                gpu.retired.push(old);
            }
            gpu.uploaded = None;
        }
        Ok(&gpu.buffer.as_ref().unwrap().0)
    }

    /// Upload a changed table ahead of this submission's lists.
    pub(crate) unsafe fn flush(&self, device: &ID3D12Device, queue: &ID3D12CommandQueue) -> Result<()> {
        let mut gpu = lock(&self.gpu);
        // Nothing reads the table before a draw has asked for its address.
        if gpu.buffer.is_none() {
            return Ok(());
        }
        let (version, bytes) = {
            let table = lock(&self.table);
            (table.version, table.bytes())
        };
        if gpu.uploaded == Some(version) {
            return Ok(());
        }
        let target = Self::current(&mut gpu, device, bytes)?.clone();
        let staging = committed(device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ)?;
        unsafe {
            let mut data = std::ptr::null_mut();
            staging.Map(0, Some(&D3D12_RANGE::default()), Some(&mut data))?;
            if data.is_null() { return Err(E_POINTER.into()); }
            let words = std::slice::from_raw_parts_mut(data.cast::<u32>(), (bytes / 4) as usize);
            let table = lock(&self.table);
            table.serialize(words);
            let version = table.version;
            drop(table);
            staging.Unmap(0, None);
            let kind = queue.GetDesc().Type;
            let ready = gpu.recorders.iter().position(|r| r.kind == kind && r.fence.GetCompletedValue() >= r.value);
            let mut recorder = match ready {
                Some(index) => {
                    let mut r = gpu.recorders.swap_remove(index);
                    r.held.clear();
                    r.allocator.Reset()?;
                    r.list.Reset(&r.allocator, None)?;
                    r
                }
                None => {
                    let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(kind)?;
                    let list: ID3D12GraphicsCommandList = device.CreateCommandList(0, kind, &allocator, None)?;
                    crate::dred::name(&list, "tuxbox descriptor table copy");
                    let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE)?;
                    Recorder { kind, allocator, list, fence, value: 0, held: Vec::new() }
                }
            };
            // The buffer decays to COMMON between submissions and promotes
            // to COPY_DEST here and to a shader resource state when read.
            recorder.list.CopyBufferRegion(&target, 0, &staging, 0, bytes);
            recorder.held.push(staging);
            recorder.held.push(target);
            recorder.list.Close()?;
            queue.ExecuteCommandLists(&[Some(recorder.list.cast()?)]);
            crate::dred::check(queue, "descriptor lookup table upload");
            recorder.value += 1;
            queue.Signal(&recorder.fence, recorder.value)?;
            gpu.recorders.push(recorder);
            gpu.uploaded = Some(version);
        }
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || format!("uploaded descriptor lookup table ({bytes} bytes)"));
        Ok(())
    }
}

/// The translated stage declares `tb_desc_table` (a PSV0 SRV binding at t1, space32766).
pub(crate) fn declared(code: &[u8]) -> bool {
    super::null_kinds::bindings(code).iter().any(|&[resource, space, lower, upper, _]| {
        matches!(resource, 3..=5) && space == SPACE && (lower..=upper).contains(&REGISTER)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_probes_like_the_shader() {
        let mut table = Table::default();
        for i in 0..200u32 {
            table.insert(&sampler_key([i, i * 3, 7, 0]), i + 1, false);
        }
        assert!(table.used * 2 <= table.entries.len());
        let mut words = vec![0; (table.bytes() / 4) as usize];
        table.serialize(&mut words);
        let mask = words[0];
        for i in 0..200u32 {
            let key = sampler_key([i, i * 3, 7, 0]);
            let h = hash(&key);
            let found = (0..PROBES).find_map(|k| {
                let e = 1 + ((h.wrapping_add(k) & mask) as usize) * ENTRY;
                (words[e + 9] == OCCUPIED && words[e..e + 8] == key[..]).then_some(words[e + 8])
            });
            assert_eq!(found, Some(i + 1));
        }
    }

    #[test]
    fn decode_inverts_encode_for_ordinary_samplers() {
        let desc = D3D12_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_WRAP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_BORDER,
            MipLODBias: -1.5,
            MaxAnisotropy: 1,
            ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
            BorderColor: [0.0, 0.0, 0.0, 1.0],
            MinLOD: 0.0,
            MaxLOD: 15.0,
        };
        assert_eq!(encode(&decode(encode(&desc))), encode(&desc));
        let back = decode(encode(&desc));
        assert_eq!(back.Filter, desc.Filter);
        assert_eq!(back.MipLODBias, -1.5);
    }
}
