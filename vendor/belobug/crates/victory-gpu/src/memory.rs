//! Title memory by address, for shaders that follow pointers: Grand Theft
//! Auto VI's rays walk an acceleration structure the title builds itself,
//! with 40-bit pointers from constants and from memory.
//!
//! Every host view heap ends with some slots of ours: a map, then raw views
//! of memory (`victory_shader`, `Lower::memory`). The map holds two dwords
//! per MiB of address space: which view shows it, and the byte in it where
//! the MiB starts. Memory the host uses in place is a view of its window;
//! the rest is the sparse copy of its 2 GiB.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use crate::host::host;

/// Slots of ours at the end of a view heap: the map, then views.
pub(crate) const SLOTS: u32 = 1024;
/// The address space shown: 40 bits.
const MIBS: usize = 1 << 20;
/// A sparse copy's part of it.
const CHUNK: u64 = 2 << 30;

struct View {
    /// None: reads zeros.
    buffer: Option<ID3D12Resource>,
    bytes: u64,
}

struct Map {
    buffer: ID3D12Resource,
    entries: usize,
    views: Vec<View>,
    /// Sparse copies, by number.
    chunks: HashMap<u64, u32>,
    /// Windows of memory in place, by start: the view and the length.
    windows: std::collections::BTreeMap<u64, (u32, u64)>,
}
// SAFETY: free-threaded objects and a persistent mapping.
unsafe impl Send for Map {}

static MAP: Mutex<Option<Map>> = Mutex::new(None);
/// Views made so far.
static VIEWS: AtomicU64 = AtomicU64::new(0);

impl Map {
    fn new() -> Option<Map> {
        let (buffer, data) = host()?.upload_buffer((MIBS * 8) as u64)?;
        crate::host::name(&buffer, || "the map of title memory".into());
        for mib in 0..MIBS {
            // SAFETY: inside the buffer. The first view is the null one.
            unsafe { data.cast::<[u32; 2]>().add(mib).write([1, 0]) };
        }
        VIEWS.store(1, Ordering::Release);
        Some(Map {
            buffer,
            entries: data as usize,
            views: vec![View { buffer: None, bytes: 0 }],
            chunks: HashMap::new(),
            windows: Default::default(),
        })
    }

    /// Shows the MiBs starting in `[from, to)` through view number `view`
    /// of memory from `start` on.
    fn show(&mut self, from: u64, to: u64, view: u32, start: u64) {
        let (first, last) = (from.div_ceil(1 << 20), to.div_ceil(1 << 20));
        for mib in first..last.min(MIBS as u64) {
            let within = (mib << 20).wrapping_sub(start) as u32;
            // SAFETY: inside the buffer.
            unsafe {
                (self.entries as *mut [u32; 2])
                    .add(mib as usize)
                    .write_volatile([view + 1, within]);
            }
        }
    }

    fn add(&mut self, buffer: ID3D12Resource, bytes: u64) -> Option<u32> {
        if self.views.len() as u32 + 2 > SLOTS {
            static ONCE: std::sync::Once = std::sync::Once::new();
            ONCE.call_once(|| tracing::warn!("too many views of title memory by address"));
            return None;
        }
        self.views.push(View { buffer: Some(buffer), bytes });
        VIEWS.store(self.views.len() as u64, Ordering::Release);
        Some(self.views.len() as u32 - 1)
    }
}

/// Makes the memory around `address` reachable, shown in place by the
/// window `[start, start + len)` of `buffer` if that is given.
pub(crate) fn reach(address: u64, window: Option<(&ID3D12Resource, u64, u64)>) {
    if address >> 40 != 0 || address < 1 << 16 {
        return;
    }
    let mut guard = MAP.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Map::new();
    }
    let Some(map) = guard.as_mut() else { return };
    let chunk = address / CHUNK;
    if !map.chunks.contains_key(&chunk)
        && let Some(copy) = crate::shadow::chunk(address)
        && let Some(view) = map.add(copy.buffer.clone(), CHUNK)
    {
        map.chunks.insert(chunk, view);
        map.show(chunk * CHUNK, (chunk + 1) * CHUNK, view, chunk * CHUNK);
        let over: Vec<(u64, u32, u64)> = map
            .windows
            .range(..(chunk + 1) * CHUNK)
            .filter(|(s, w)| *s + w.1 > chunk * CHUNK)
            .map(|(&s, &(v, len))| (s, v, len))
            .collect();
        for (start, view, len) in over {
            map.show(start, start + len, view, start);
        }
    }
    if let Some((buffer, start, len)) = window
        && !map.windows.contains_key(&start)
        && let Some(view) = map.add(buffer.clone(), len)
    {
        map.windows.insert(start, (view, len));
        map.show(start, start + len, view, start);
    }
}

/// The title's memory in `[start, end)` is another's now: windows there are
/// no longer its.
pub(crate) fn remapped(start: u64, end: u64) {
    let mut guard = MAP.lock().unwrap_or_else(|e| e.into_inner());
    let Some(map) = guard.as_mut() else { return };
    let over: Vec<(u64, u64)> = map
        .windows
        .range(..end)
        .filter(|(s, w)| *s + w.1 > start)
        .map(|(&s, &(_, len))| (s, len))
        .collect();
    for (from, len) in over {
        map.windows.remove(&from);
        let mut at = from;
        while at < from + len {
            let chunk = at / CHUNK;
            let next = ((chunk + 1) * CHUNK).min(from + len);
            match map.chunks.get(&chunk).copied() {
                Some(view) => map.show(at, next, view, chunk * CHUNK),
                None => map.show(at, next, 0, 0),
            }
            at = next;
        }
    }
}

/// Makes the views in the slots from `first` on that a heap that has seen
/// `seen` of them misses; how many there are.
pub(crate) fn fill(
    device: &ID3D12Device,
    seen: u64,
    slot: impl Fn(u32) -> D3D12_CPU_DESCRIPTOR_HANDLE,
) -> u64 {
    if VIEWS.load(Ordering::Acquire) + 1 == seen {
        return seen;
    }
    let mut guard = MAP.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Map::new();
    }
    let Some(map) = guard.as_ref() else { return seen };
    if seen == 0 {
        let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_SRV {
                    FirstElement: 0,
                    NumElements: (MIBS * 2) as u32,
                    StructureByteStride: 0,
                    Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a live buffer and a slot of the heap.
        unsafe { device.CreateShaderResourceView(&map.buffer, Some(&desc), slot(0)) };
    }
    for (k, view) in map.views.iter().enumerate().skip(seen.saturating_sub(1) as usize) {
        let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: DXGI_FORMAT_R32_TYPELESS,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                Buffer: D3D12_BUFFER_UAV {
                    FirstElement: 0,
                    NumElements: (view.bytes / 4).max(1) as u32,
                    StructureByteStride: 0,
                    CounterOffsetInBytes: 0,
                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                },
            },
        };
        // SAFETY: a live buffer (or none) and a slot of the heap.
        unsafe {
            device.CreateUnorderedAccessView(
                view.buffer.as_ref(),
                None::<&ID3D12Resource>,
                Some(&desc),
                slot(1 + k as u32),
            )
        };
    }
    map.views.len() as u64 + 1
}
