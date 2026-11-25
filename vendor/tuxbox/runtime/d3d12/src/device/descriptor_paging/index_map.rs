//! Translate absolute guest indices for shaders using ResourceDescriptorHeap.
//! Entries never move once published. Ordinary root tables can share contiguous
//! entries or keep their own aliases when the guest table overlaps other ranges.
use super::{Arena, FreeSpans, RetiredViews, Tables};
use crate::device::{Device, blas::upload_buffer};
use std::{collections::BTreeMap, ops::Range};
use windows::Win32::Foundation::{E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Interface, Result};

enum Upload {
    Native {
        native: ID3D12Resource,
        address: usize,
    },
    #[cfg(test)]
    Mock(Vec<u32>),
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
        Ok(Self::Native {
            native,
            address: data.addr(),
        })
    }

    fn set(&mut self, logical: usize, resident: u32) {
        match self {
            Self::Native { address, .. } => {
                // CPU writes are serialized by the arena lock. The resource stays
                // mapped for its lifetime; no CPU read from write-combined memory.
                unsafe {
                    (*address as *mut u32)
                        .add(1 + logical)
                        .write_volatile(resident)
                };
            }
            #[cfg(test)]
            Self::Mock(words) => words[1 + logical] = resident,
        }
    }

    fn native_resource(&self) -> &ID3D12Resource {
        match self {
            Self::Native { native, .. } => native,
            #[cfg(test)]
            Self::Mock(_) => unreachable!("mock upload has no ID3D12Resource"),
        }
    }
}

impl Drop for Upload {
    fn drop(&mut self) {
        match self {
            Self::Native { native, .. } => unsafe { native.Unmap(0, None) },
            #[cfg(test)]
            Self::Mock(_) => {}
        }
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

    fn missing_in(&self, range: Range<u32>) -> u32 {
        self.slots[range.start as usize..range.end as usize]
            .iter()
            .filter(|&&slot| slot == u32::MAX)
            .count() as u32
    }

    fn copy(
        &mut self,
        arena: &Arena,
        range: Range<u32>,
        free: &mut FreeSpans,
        used: &mut u32,
        retired: &mut RetiredViews,
    ) {
        let mut run = None;
        for i in range.clone() {
            let mut slot = self.slots[i as usize];
            if slot == u32::MAX {
                slot = free.alloc(1).unwrap_or_else(|| {
                    let next = *used;
                    debug_assert!(next < arena.limit);
                    *used += 1;
                    next
                });
                self.assign(i as usize, slot);
            }
            match run {
                Some((first, physical)) if slot == physical + (i - first) => {}
                previous => {
                    if let Some((first, physical)) = previous {
                        arena.copy(first, physical, i - first, retired);
                    }
                    run = Some((i, slot));
                }
            }
        }
        if let Some((first, physical)) = run {
            arena.copy(first, physical, range.end - first, retired);
        }
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
    pub(super) fn is_resident(&self) -> bool {
        self.resident.is_some()
    }

    pub(super) fn set_exhausted(&mut self) {
        self.exhausted = true;
    }

    pub(super) fn missing_in_range(&self, range: Range<u32>) -> u32 {
        self.resident
            .as_ref()
            .map_or(0, |resident| resident.missing_in(range))
    }

    pub(super) fn publish_resident(
        &mut self,
        arena: &Arena,
        range: Range<u32>,
        free: &mut FreeSpans,
        used: &mut u32,
        retired: &mut RetiredViews,
    ) {
        if let Some(resident) = &mut self.resident {
            resident.copy(arena, range, free, used, retired);
        }
    }

    pub(super) fn record_written(&mut self, mut range: Range<u32>) {
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
        let buffer = Upload::new(&device.native, 0)?.native_resource().clone();
        *identity = Some(buffer.clone());
        return Ok(buffer);
    };
    let mut retired = RetiredViews::default();
    let result = (|| -> Result<ID3D12Resource> {
        let mut tables = arena.tables.lock().unwrap_or_else(|e| e.into_inner());
        if tables.direct.exhausted {
            return Err(E_OUTOFMEMORY.into());
        }
        if let Some(resident) = &tables.direct.resident {
            return Ok(resident.upload.native_resource().clone());
        }

        let mut slots = Vec::new();
        slots
            .try_reserve_exact(arena.count as usize)
            .map_err(|_| E_OUTOFMEMORY)?;
        slots.resize(arena.count as usize, u32::MAX);
        let resident = Resident {
            upload: Upload::new(&device.native, arena.count)?,
            slots,
        };
        activate_with_resident(arena, &mut tables, resident, &mut retired)?;
        Ok(tables
            .direct
            .resident
            .as_ref()
            .expect("activated resident map")
            .upload
            .native_resource()
            .clone())
    })();
    drop(retired);
    result
}

fn activate_with_resident(
    arena: &Arena,
    tables: &mut Tables,
    mut resident: Resident,
    retired: &mut RetiredViews,
) -> Result<()> {
    // Adopt longest ranges first so shorter overlapping aliases that add zero
    // new slots stay unpinned and reclaimable.
    let mut keys: Vec<(u32, u32)> = tables.ranges.keys().copied().collect();
    keys.sort_by_key(|&(first, count)| (std::cmp::Reverse(count), first));
    let mut adopted_keys = Vec::new();
    for key in keys {
        let (first, count) = key;
        let entry = tables.ranges.get_mut(&key).expect("range key exists");
        let mut adopted = false;
        for i in 0..count {
            if resident.slots[(first + i) as usize] == u32::MAX {
                resident.assign((first + i) as usize, entry.slot + i);
                adopted = true;
            }
        }
        if adopted && !entry.direct_pinned {
            entry.direct_pinned = true;
            adopted_keys.push(key);
        }
    }
    let missing: u32 = tables
        .direct
        .written
        .iter()
        .map(|(&first, &end)| resident.missing_in(first..end))
        .sum();
    if missing > 0
        && let Err(error) = tables.ensure_direct_capacity(arena, missing, retired)
    {
        for key in adopted_keys {
            if let Some(entry) = tables.ranges.get_mut(&key) {
                entry.direct_pinned = false;
            }
        }
        return Err(error);
    }
    let Tables {
        direct,
        free,
        used,
        ..
    } = tables;
    for (&first, &end) in &direct.written {
        resident.copy(arena, first..end, free, used, retired);
    }
    direct.resident = Some(resident);
    direct.written.clear();
    Ok(())
}

#[cfg(test)]
pub(super) fn activate_for_test(
    arena: &Arena,
    tables: &mut Tables,
    retired: &mut RetiredViews,
) -> Result<()> {
    let slots = vec![u32::MAX; arena.count as usize];
    let resident = Resident {
        upload: Upload::Mock(vec![0; 1 + arena.count as usize]),
        slots,
    };
    activate_with_resident(arena, tables, resident, retired)
}
