//! Translate absolute guest indices for shaders using ResourceDescriptorHeap.
//! Entries never move once published. Ordinary root tables can share contiguous
//! entries or keep their own aliases when the guest table overlaps other ranges.
use super::{Arena, Tables};
use crate::device::{Device, blas::upload_buffer};
use std::{collections::BTreeMap, ops::Range};
use windows::Win32::Foundation::{E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, Result};

struct Upload {
    native: ID3D12Resource,
    address: usize,
}

impl Upload {
    fn new(device: &ID3D12Device, count: u32) -> Result<Self> {
        let native = upload_buffer(device, (1 + u64::from(count)) * 4)?;
        let mut data = std::ptr::null_mut();
        unsafe { native.Map(0, Some(&D3D12_RANGE::default()), Some(&mut data))? };
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { data.cast::<u32>().write_volatile(u32::from(count != 0)) };
        Ok(Self {
            native,
            address: data.addr(),
        })
    }

    fn set(&mut self, logical: usize, resident: u32) {
        // CPU writes are serialized by the arena lock. The resource stays
        // mapped for its lifetime; no CPU read from write-combined memory.
        unsafe {
            (self.address as *mut u32)
                .add(1 + logical)
                .write_volatile(resident)
        };
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        unsafe { self.native.Unmap(0, None) };
    }
}

struct Resident {
    upload: Upload,
    slots: Vec<u32>,
}

impl Arena {
    pub(crate) fn check_index_capacity(&self) -> Result<()> {
        if self.tables.lock().unwrap_or_else(|e| e.into_inner()).direct.exhausted {
            return Err(E_OUTOFMEMORY.into());
        }
        Ok(())
    }
}

impl Resident {
    fn assign(&mut self, i: usize, slot: u32) {
        self.slots[i] = slot;
        self.upload.set(i, slot);
    }

    fn copy(&mut self, arena: &Arena, range: Range<u32>, used: &mut u32) -> Result<()> {
        let missing = self.slots[range.start as usize..range.end as usize]
            .iter()
            .filter(|&&slot| slot == u32::MAX)
            .count() as u32;
        if missing > arena.limit - *used {
            return Err(E_OUTOFMEMORY.into());
        }
        let mut run = None;
        for i in range.clone() {
            let mut slot = self.slots[i as usize];
            if slot == u32::MAX {
                slot = *used;
                *used += 1;
                self.assign(i as usize, slot);
            }
            match run {
                Some((first, physical)) if slot == physical + (i - first) => {}
                previous => {
                    if let Some((first, physical)) = previous {
                        arena.copy(first, physical, i - first);
                    }
                    run = Some((i, slot));
                }
            }
        }
        if let Some((first, physical)) = run {
            arena.copy(first, physical, range.end - first);
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Direct {
    // Remember descriptors written before a shader first indexes the heap.
    written: BTreeMap<u32, u32>,
    resident: Option<Resident>,
    exhausted: bool,
}

impl Direct {
    pub(super) fn publish(&mut self, arena: &Arena, mut range: Range<u32>, used: &mut u32) {
        if let Some(resident) = &mut self.resident {
            self.exhausted |= resident.copy(arena, range, used).is_err();
            return;
        }
        if let Some((&lo, &hi)) = self.written.range(..=range.start).next_back() {
            if range.end <= hi {
                return;
            }
            if range.start <= hi {
                range.start = lo;
                self.written.remove(&lo);
            }
        }
        while let Some((&lo, &hi)) = self.written.range(range.start..=range.end).next() {
            self.written.remove(&lo);
            range.end = range.end.max(hi);
        }
        self.written.insert(range.start, range.end);
    }

    pub(super) fn contiguous(&self, first: u32, count: u32) -> Option<u32> {
        let slots = &self.resident.as_ref()?.slots[first as usize..(first + count) as usize];
        let (&start, rest) = slots.split_first()?;
        (start != u32::MAX
            && rest
                .iter()
                .enumerate()
                .all(|(i, &s)| start.checked_add(i as u32 + 1) == Some(s)))
        .then_some(start)
    }
}

/// # Safety
/// The owner must be the controlling unknown of the live guest device.
pub(crate) unsafe fn index_map(owner: &IUnknown, arena: Option<&Arena>) -> Result<ID3D12Resource> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    let Some(arena) = arena else {
        let mut identity = device
            .state
            .descriptor_arenas
            .identity_map
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if let Some(buffer) = identity.as_ref() {
            return Ok(buffer.clone());
        }
        let buffer = Upload::new(&device.native, 0)?.native.clone();
        *identity = Some(buffer.clone());
        return Ok(buffer);
    };
    let mut tables = arena.tables.lock().unwrap_or_else(|e| e.into_inner());
    let Tables {
        direct,
        used,
        ranges,
    } = &mut *tables;
    if direct.exhausted {
        return Err(E_OUTOFMEMORY.into());
    }
    if let Some(resident) = &direct.resident {
        return Ok(resident.upload.native.clone());
    }

    let mut slots = Vec::new();
    slots
        .try_reserve_exact(arena.count as usize)
        .map_err(|_| E_OUTOFMEMORY)?;
    slots.resize(arena.count as usize, u32::MAX);
    let mut resident = Resident {
        upload: Upload::new(&device.native, arena.count)?,
        slots,
    };
    for (&(first, count), &physical) in ranges.iter() {
        for i in 0..count {
            if resident.slots[(first + i) as usize] == u32::MAX {
                resident.assign((first + i) as usize, physical + i);
            }
        }
    }
    // Reserve against a local cursor so an unsuccessful activation consumes no
    // heap space. The next attempt can safely overwrite those unreferenced slots.
    let mut next = *used;
    for (&first, &end) in &direct.written {
        resident.copy(arena, first..end, &mut next)?;
    }
    let buffer = resident.upload.native.clone();
    direct.resident = Some(resident);
    direct.written.clear();
    *used = next;
    Ok(buffer)
}
