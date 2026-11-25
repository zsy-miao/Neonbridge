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
    /// `fill`: the slot every entry names until it is set (the null tail).
    fn new(device: &ID3D12Device, count: u32, fill: Option<u32>) -> Result<Self> {
        Self::with_header(device, count, fill, if count == 0 { 0 } else { 0x8000_0000 | (count & 0x7fff_ffff) })
    }

    /// An identity map whose shader clamps a guest index to `slots - 1`:
    /// header 0x4000_0000 | slots, no entries (xshader e5ae009 reads bit 30
    /// with bit 31 clear as a clamped identity heap).
    fn identity(device: &ID3D12Device, slots: u32) -> Result<Self> {
        Self::with_header(device, 0, None, if slots == 0 { 0 } else { 0x4000_0000 | (slots & 0x3fff_ffff) })
    }

    fn with_header(device: &ID3D12Device, count: u32, fill: Option<u32>, header: u32) -> Result<Self> {
        let native = upload_buffer(device, (1 + u64::from(count)) * 4)?;
        let mut data = std::ptr::null_mut();
        unsafe { native.Map(0, Some(&D3D12_RANGE::default()), Some(&mut data))? };
        if data.is_null() {
            return Err(E_POINTER.into());
        }
        // Word 0: 0 = identity, 0x4000_0000 | slots = identity clamped to
        // slots - 1, 0x8000_0000 | count = mapped with `count` entries; the
        // shader clamps indices to count - 1 (xshader 932afdd, e5ae009).
        unsafe { data.cast::<u32>().write_volatile(header) };
        if let Some(slot) = fill {
            for i in 0..count as usize {
                unsafe { data.cast::<u32>().add(1 + i).write_volatile(slot) };
            }
        }
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

/// Merge a write made before the index map exists into the ranges it replays
/// when it is built (inside the arena lock, before it is published; the
/// lazy-mirror replay of VIctory a4e7f1d, already the case here).
fn remember(written: &mut BTreeMap<u32, u32>, mut range: Range<u32>) {
    if let Some((&lo, &hi)) = written.range(..=range.start).next_back() {
        if range.end <= hi {
            return;
        }
        if range.start <= hi {
            range.start = lo;
            written.remove(&lo);
        }
    }
    while let Some((&lo, &hi)) = written.range(range.start..=range.end).next() {
        written.remove(&lo);
        range.end = range.end.max(hi);
    }
    written.insert(range.start, range.end);
}

impl Direct {
    pub(super) fn publish(&mut self, arena: &Arena, range: Range<u32>, used: &mut u32) {
        if let Some(resident) = &mut self.resident {
            self.exhausted |= resident.copy(arena, range, used).is_err();
            return;
        }
        remember(&mut self.written, range);
    }

    /// Whether the index map names any slot of the range `first..first+count`
    /// resident at `physical` (such a range is never freed, retire.rs).
    pub(super) fn references(&self, first: u32, count: u32, physical: u32) -> bool {
        self.resident.as_ref().is_some_and(|resident| {
            (0..count).any(|i| resident.slots.get((first + i) as usize) == Some(&(physical + i)))
        })
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
/// `slots`: an unpaged heap's descriptor count, which its identity map
/// clamps guest indices to (0 or TUXBOX_IDENTITY_CLAMP=0: unclamped).
pub(crate) unsafe fn index_map(owner: &IUnknown, arena: Option<&Arena>, slots: u32) -> Result<ID3D12Resource> {
    let device = unsafe { &*owner.as_raw().cast::<Device>() };
    let Some(arena) = arena else {
        let mut identity = device
            .state
            .descriptor_arenas
            .identity_map
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        // A stray index past an unpaged heap stays inside it (the read past a
        // heap's end hung NVIDIA, VIctory host.rs:510-590); one map per size.
        static CLAMP: std::sync::LazyLock<bool> = std::sync::LazyLock::new(||
            // Off with xshader's TB_HEAP_CLAMP_ALL=0 too: a shader without the
            // 0x4000_0000 arm would take the header as a mapped heap.
            ["TUXBOX_IDENTITY_CLAMP", "TB_HEAP_CLAMP_ALL"].iter().all(|k| std::env::var(k).map_or(true, |v| v.trim() != "0")));
        let slots = if *CLAMP { slots } else { 0 };
        if let Some(buffer) = identity.get(&slots) {
            return Ok(buffer.clone());
        }
        let buffer = Upload::identity(&device.native, slots)?.native.clone();
        identity.insert(slots, buffer.clone());
        return Ok(buffer);
    };
    let mut tables = arena.tables.lock().unwrap_or_else(|e| e.into_inner());
    let Tables {
        direct,
        used,
        ranges,
        ..
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
    // With the null tail the map holds one entry past the guest heap, the
    // null slot, where a stray index clamps.
    let entries = if arena.null_slot.is_some() { arena.count.saturating_add(1) } else { arena.count };
    let mut resident = Resident {
        upload: Upload::new(&device.native, entries, arena.null_slot)?,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_before_the_map_exists_are_all_kept_for_replay() {
        let mut written = BTreeMap::new();
        remember(&mut written, 10..20);
        remember(&mut written, 30..40);
        remember(&mut written, 12..15); // inside: no change
        remember(&mut written, 18..32); // bridges both
        remember(&mut written, 50..51);
        assert_eq!(written.into_iter().collect::<Vec<_>>(), vec![(10, 40), (50, 51)]);
    }
}
