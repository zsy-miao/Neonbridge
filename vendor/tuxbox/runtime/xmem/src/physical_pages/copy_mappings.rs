//! Queue-ordered aliases of a registered, section-backed 64 KiB page pool.
use super::{PAGE, POOL, Pool, shared_pages};
use std::{collections::HashMap, ops::Range, slice};
use windows_sys::Win32::Foundation::{
    ERROR_INVALID_ADDRESS, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SUPPORTED,
};
use windows_sys::Win32::System::Memory::{MEM_RESERVE, PAGE_NOACCESS};

#[repr(C)]
struct Batch {
    destination: usize,
    count: u32,
    ranges: *const PageRange,
}

#[repr(C)]
struct PageRange {
    mode: u32,
    source: u32,
    count: u32,
}

const _: () = assert!(size_of::<Batch>() == 24 && size_of::<PageRange>() == 12);

unsafe fn array<'a, T>(pointer: *const T, count: u32) -> Result<&'a [T], u32> {
    if count == 0 {
        return Ok(&[]);
    }
    if pointer.is_null() || !pointer.is_aligned() {
        return Err(ERROR_INVALID_PARAMETER);
    }
    // The queue owns the validated native arrays through this synchronous call.
    Ok(unsafe { slice::from_raw_parts(pointer, count as usize) })
}

#[derive(Clone, Copy)]
struct Change {
    at: usize,
    old: Option<u64>,
    new: Option<u64>,
}

impl Pool {
    fn apply_alias(&mut self, at: usize, token: Option<u64>) -> Result<(), u32> {
        let previous = self.mappings.get(&at).copied();
        if previous == token {
            return Ok(());
        }
        if previous.is_some() {
            self.unmap_shared_page(at)?;
        }
        if let Some(token) = token {
            let map = |pool: &mut Pool, token| match pool.map_shared_page(at, token)? {
                true => Ok(()),
                false => Err(ERROR_INVALID_ADDRESS),
            };
            if let Err(error) = map(self, token) {
                if let Some(previous) = previous {
                    map(self, previous)?;
                }
                return Err(error);
            }
        }
        Ok(())
    }

    fn update_aliases(&mut self, updates: &[Change]) -> Result<(), u32> {
        self.gpu_aliases
            .try_reserve(updates.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        self.mappings
            .try_reserve(updates.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        let mut tokens = Vec::new();
        tokens
            .try_reserve(updates.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        let mut done = 0;
        let apply = (|| {
            while let Some(first) = updates.get(done) {
                // A linear source allocation can be represented with one OS
                // view. Repeated tokens and discontiguous sources use separate
                // views, so writes remain visible through every alias.
                if first.old.is_none() && first.new.is_some() {
                    tokens.clear();
                    let mut at = first.at;
                    let region = shared_pages::region(at)?;
                    let region_end = region.BaseAddress.addr() + region.RegionSize;
                    for entry in &updates[done..] {
                        if entry.old.is_some() || entry.at != at || at >= region_end {
                            break;
                        }
                        let Some(token) = entry.new else { break };
                        let page = &self.pages[&token];
                        if let Some(prior) = tokens.last() {
                            let prev = &self.pages[prior];
                            if !std::sync::Arc::ptr_eq(&prev.section, &page.section)
                                || prev.offset + PAGE as u64 != page.offset
                            {
                                break;
                            }
                        }
                        tokens.push(token);
                        at += PAGE;
                    }
                    if tokens.len() > 1 && self.map_contiguous(first.at, &tokens)? {
                        done += tokens.len();
                        continue;
                    }
                }
                self.apply_alias(first.at, first.new)?;
                done += 1;
            }
            Ok(())
        })();
        if let Err(error) = apply {
            for entry in updates[..done].iter().rev() {
                self.apply_alias(entry.at, entry.old)?;
            }
            return Err(error);
        }
        for entry in updates {
            if entry.new.is_some() {
                self.gpu_aliases.insert(entry.at);
            } else {
                self.gpu_aliases.remove(&entry.at);
            }
        }
        Ok(())
    }

    unsafe fn plan_batch(
        &self,
        batches: &[Batch],
        source: usize,
        pages: u32,
    ) -> Result<Vec<Change>, u32> {
        let source_end = source
            .checked_add(pages as usize * PAGE)
            .ok_or(ERROR_INVALID_PARAMETER)?;
        let mut targets = HashMap::new();
        for batch in batches {
            let mut at = batch.destination;
            for range in unsafe { array(batch.ranges, batch.count)? } {
                if range.mode == 3 {
                    continue;
                } // This mode consumes no destination pages.
                if range.mode > 2 || at == 0 || !at.is_multiple_of(PAGE) {
                    return Err(ERROR_INVALID_PARAMETER);
                }
                if range.count == 0 {
                    continue;
                }
                let bytes = range.count as usize * PAGE;
                let end = at.checked_add(bytes).ok_or(ERROR_INVALID_PARAMETER)?;
                if at < source_end && source < end {
                    return Err(ERROR_INVALID_PARAMETER);
                }
                if range.mode != 2 {
                    let n = if range.mode == 0 { range.count } else { 1 };
                    if source == 0
                        || !source.is_multiple_of(PAGE)
                        || range.source.checked_add(n).is_none_or(|last| last > pages)
                    {
                        return Err(ERROR_INVALID_PARAMETER);
                    }
                }
                targets
                    .try_reserve(range.count as usize)
                    .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                for i in 0..range.count {
                    let token = match range.mode {
                        0 | 1 => {
                            let index = if range.mode == 0 {
                                range.source + i
                            } else {
                                range.source
                            };
                            let entry = source + index as usize * PAGE;
                            let token = *self.mappings.get(&entry).ok_or(ERROR_INVALID_ADDRESS)?;
                            if self.pages[&token].mapped {
                                return Err(ERROR_NOT_SUPPORTED);
                            }
                            Some(token)
                        }
                        _ => None,
                    };
                    // Ordered overlapping batch entries leave the final token at the address.
                    targets.insert(at + i as usize * PAGE, token);
                }
                at = end;
            }
        }
        let mut updates = Vec::new();
        updates
            .try_reserve(targets.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        for (at, new) in targets {
            updates.push(Change {
                at,
                old: self.mappings.get(&at).copied(),
                new,
            });
        }
        updates.sort_unstable_by_key(|e| e.at);

        let mut reserved: Range<usize> = 0..0;
        for entry in &updates {
            if entry.old.is_some() {
                if !self.gpu_aliases.contains(&entry.at) && entry.old != entry.new {
                    return Err(ERROR_INVALID_ADDRESS);
                }
            } else if !reserved.contains(&entry.at) {
                let area = shared_pages::region(entry.at)?;
                if area.State != MEM_RESERVE || area.AllocationProtect != PAGE_NOACCESS {
                    return Err(ERROR_INVALID_ADDRESS);
                }
                let base = area.BaseAddress.addr();
                reserved = base..base + area.RegionSize;
            }
        }
        updates.retain(|e| e.old != e.new);
        Ok(updates)
    }
}

// Private provider ABI. The queue passes its validated page-pool VA, synchronizes
// outstanding GPU uses, and retains the mapping inputs until this returns.
#[unsafe(export_name = "TuxBoxCopyPageMappings")]
unsafe extern "system" fn copy_page_mappings(
    count: u32,
    batches: *const Batch,
    source: usize,
    pages: u32,
) -> i32 {
    let result = unsafe {
        array(batches, count).and_then(|batches| {
            let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
            let updates = pool.plan_batch(batches, source, pages)?;
            pool.update_aliases(&updates)
        })
    };
    match result {
        Ok(()) => 0,
        Err(error) => (0x8007_0000 | (error & 0xffff)) as i32,
    }
}

// A desktop heap imported from a CPU mapping pins the old section pages.
// Reapplying its existing token is safe; a real replacement needs a new heap.
#[unsafe(export_name = "TuxBoxValidatePinnedPageMappings")]
unsafe extern "system" fn validate_pinned_page_mappings(
    count: u32,
    batches: *const Batch,
    source: usize,
    pages: u32,
    start: usize,
    end: usize,
) -> i32 {
    if start >= end {
        return windows_sys::Win32::Foundation::E_INVALIDARG;
    }
    let plan = unsafe {
        array(batches, count).and_then(|batches| {
            POOL.lock()
                .unwrap_or_else(|e| e.into_inner())
                .plan_batch(batches, source, pages)
        })
    };
    match plan {
        Ok(updates)
            if updates
                .iter()
                .any(|entry| entry.at < end && start < entry.at + PAGE) =>
        {
            windows_sys::Win32::Foundation::E_NOTIMPL
        }
        Ok(_) => 0,
        Err(error) => (0x8007_0000 | (error & 0xffff)) as i32,
    }
}
