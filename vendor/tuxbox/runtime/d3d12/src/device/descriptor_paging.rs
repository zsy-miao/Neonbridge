//! A writable Xbox descriptor arena can exceed the shader-visible heap limit.
//! Store all native descriptors in a CPU heap and materialize bounded root
//! tables in a shader-visible heap when the command list references them.
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, Weak},
};
use windows::Win32::Foundation::{E_INVALIDARG, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::Result;

use super::title_memory::GuestMemory;
mod index_map;
pub(crate) use index_map::index_map;

#[derive(Default)]
pub(crate) struct Registry {
    arenas: Mutex<BTreeMap<usize, Weak<Arena>>>,
    count: std::sync::atomic::AtomicUsize,
    identity_map: Mutex<Option<ID3D12Resource>>,
}
impl Registry {
    pub(crate) fn find(&self, address: usize) -> Option<Arc<Arena>> {
        if self.count.load(std::sync::atomic::Ordering::Acquire) == 0 {
            return None;
        }
        let entries = self.arenas.lock().unwrap_or_else(|e| e.into_inner());
        let arena = entries.range(..=address).next_back()?.1.upgrade()?;
        (address - arena.base() < arena.count as usize * arena.stride).then_some(arena)
    }
    pub(crate) fn cpu(&self, handle: D3D12_CPU_DESCRIPTOR_HANDLE) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        self.find(handle.ptr)
            .map_or(handle, |arena| arena.cpu(handle.ptr))
    }
}

pub(crate) struct Arena {
    memory: GuestMemory,
    registry: Arc<Registry>,
    imported: super::placement::DescriptorRange<super::views::DescriptorBacking>,
    placed: super::placement::DescriptorRange<super::placement::PlacedView>,
    scenes: super::placement::DescriptorRange<super::blas::SceneView>,
    staging: ID3D12DescriptorHeap,
    visible: ID3D12DescriptorHeap,
    device: ID3D12Device,
    stride: usize,
    count: u32,
    limit: u32,
    // Guest ranges can overlap. Keep stable slots for every table that has
    // been referenced; a CPU descriptor update is published to each alias.
    tables: Mutex<Tables>,
}
#[derive(Default)]
struct Tables {
    used: u32,
    ranges: BTreeMap<(u32, u32), u32>,
    direct: index_map::Direct,
}
impl Drop for Arena {
    fn drop(&mut self) {
        let mut entries = self.registry.arenas.lock().unwrap_or_else(|e| e.into_inner());
        if entries
            .get(&self.base())
            .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self))
        {
            entries.remove(&self.base());
            self.registry
                .count
                .store(entries.len(), std::sync::atomic::Ordering::Release);
        }
    }
}
impl Arena {
    pub(super) fn create(
        owner: &super::Device,
        desc: &D3D12_DESCRIPTOR_HEAP_DESC,
        limit: u32,
    ) -> Result<Arc<Self>> {
        let registry = &owner.state.descriptor_arenas;
        let device = &owner.native;
        let cpu = D3D12_DESCRIPTOR_HEAP_DESC {
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
            ..*desc
        };
        let gpu = D3D12_DESCRIPTOR_HEAP_DESC {
            NumDescriptors: limit,
            ..*desc
        };
        let staging = unsafe { device.CreateDescriptorHeap(&cpu)? };
        let visible = unsafe { device.CreateDescriptorHeap(&gpu)? };
        let stride = unsafe { device.GetDescriptorHandleIncrementSize(desc.Type) } as usize;
        let bytes = stride
            .checked_mul(desc.NumDescriptors as usize)
            .ok_or(E_OUTOFMEMORY)?;
        let memory = GuestMemory::allocate(bytes, 0x0480_0004)?;
        let arena = Arc::new(Self {
            imported: owner.state.imported_views.heap(device, &staging, &cpu, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV).ok_or(E_INVALIDARG)?,
            placed: owner
                .state
                .placements
                .views
                .heap(
                    device,
                    &staging,
                    &cpu,
                    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                )
                .ok_or(E_INVALIDARG)?,
            scenes: owner
                .state
                .blas
                .views
                .heap(
                    device,
                    &staging,
                    &cpu,
                    D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                )
                .ok_or(E_INVALIDARG)?,
            memory,
            registry: registry.clone(),
            staging,
            visible,
            device: device.clone(),
            stride,
            count: desc.NumDescriptors,
            limit,
            tables: Mutex::new(Tables::default()),
        });
        let mut entries = registry.arenas.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(arena.base(), Arc::downgrade(&arena));
        registry
            .count
            .store(entries.len(), std::sync::atomic::Ordering::Release);
        Ok(arena)
    }
    pub(crate) fn base(&self) -> usize {
        self.memory.address
    }
    pub(crate) fn contains(&self, address: u64) -> bool {
        address
            .checked_sub(self.base() as u64)
            .is_some_and(|offset| offset < self.count as u64 * self.stride as u64)
    }
    pub(crate) fn visible(&self) -> &ID3D12DescriptorHeap {
        &self.visible
    }
    pub(crate) fn cpu(&self, address: usize) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        let base = unsafe { self.staging.GetCPUDescriptorHandleForHeapStart() };
        D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: base.ptr + (address - self.base()),
        }
    }
    fn copy(&self, src: u32, dst: u32, count: u32) {
        let from = self.cpu(self.base() + src as usize * self.stride);
        let start = unsafe { self.visible.GetCPUDescriptorHandleForHeapStart() };
        let to = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: start.ptr + dst as usize * self.stride,
        };
        self.imported.copy_to(self.stride, to.ptr, from.ptr, count);
        self.placed.copy_to(self.stride, to.ptr, from.ptr, count);
        self.scenes.copy_to(self.stride, to.ptr, from.ptr, count);
        unsafe {
            self.device.CopyDescriptorsSimple(
                count,
                to,
                from,
                D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            )
        };
    }
    pub(crate) fn publish(&self, address: usize, count: u32) {
        let first = ((address - self.base()) / self.stride) as u32;
        let end = first.saturating_add(count);
        if end > self.count || count == 0 {
            return;
        }
        let mut tables = self.tables.lock().unwrap_or_else(|e| e.into_inner());
        let Tables { direct, used, .. } = &mut *tables;
        direct.publish(self, first..end, used);
        for (&(start, length), &resident) in &tables.ranges {
            let lo = start.max(first);
            let hi = (start + length).min(end);
            if lo < hi {
                self.copy(lo, resident + lo - start, hi - lo);
            }
        }
    }
    pub(crate) fn gpu(&self, address: u64, length: u32) -> Result<D3D12_GPU_DESCRIPTOR_HANDLE> {
        let offset = (address as usize)
            .checked_sub(self.base())
            .ok_or(E_INVALIDARG)?;
        if length == 0 || !offset.is_multiple_of(self.stride) {
            return Err(E_INVALIDARG.into());
        }
        let first = u32::try_from(offset / self.stride).map_err(|_| E_INVALIDARG)?;
        if first.checked_add(length).is_none_or(|end| end > self.count) {
            return Err(E_INVALIDARG.into());
        }
        let mut tables = self.tables.lock().unwrap_or_else(|e| e.into_inner());
        let key = (first, length);
        let resident = if let Some(index) = tables.ranges.get(&key) {
            *index
        } else if let Some(index) = tables.direct.contiguous(first, length) {
            index
        } else {
            let index = tables.used;
            let end = index
                .checked_add(length)
                .filter(|&n| n <= self.limit)
                .ok_or(E_OUTOFMEMORY)?;
            self.copy(first, index, length);
            tables.ranges.insert(key, index);
            tables.used = end;
            index
        };
        Ok(D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: unsafe { self.visible.GetGPUDescriptorHandleForHeapStart() }.ptr
                + resident as u64 * self.stride as u64,
        })
    }
}

// Keep the logical slot and publish descriptor writes after the host call.
pub(super) struct Write {
    arena: Option<Arc<Arena>>,
    address: usize,
}
impl Write {
    pub(super) fn new(registry: &Registry, handle: &mut D3D12_CPU_DESCRIPTOR_HANDLE) -> Self {
        let address = handle.ptr;
        let arena = registry.find(address);
        if let Some(a) = &arena {
            *handle = a.cpu(address);
        }
        Self { arena, address }
    }
}
impl Drop for Write {
    fn drop(&mut self) {
        if let Some(arena) = &self.arena {
            arena.publish(self.address, 1);
        }
    }
}
