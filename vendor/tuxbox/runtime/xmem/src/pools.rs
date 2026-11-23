//! Xbox suballocation pools retain whole 4 MiB blocks for future allocations.
use core::{ffi::c_void, ptr};
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};
use windows_sys::Win32::Foundation::{
    ERROR_BAD_LENGTH, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY, ERROR_NOT_SUPPORTED,
    SetLastError,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
};

const BLOCK: usize = 1 << 22;
const ARENA: usize = 1 << 31;

// Keys are byte offsets and values are rounded sizes within a committed block.
type Block = BTreeMap<usize, usize>;

struct Pool {
    base: usize,
    blocks: Vec<Option<Block>>,
    target: u32,
    commits: u32,
    decommits: u32,
}

impl Pool {
    fn empty_blocks(&self) -> usize {
        self.blocks
            .iter()
            .filter(|b| b.as_ref().is_some_and(Block::is_empty))
            .count()
    }

    fn commit(&mut self) -> Option<usize> {
        if !super::options::can_demote_pages() {
            // Our reserved arena commits ordinary host pages. A strict pool
            // cannot silently satisfy this request with smaller backing pages.
            unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
            return None;
        }
        let index = self
            .blocks
            .iter()
            .position(Option::is_none)
            .unwrap_or(self.blocks.len());
        if index == ARENA / BLOCK {
            unsafe { SetLastError(ERROR_NOT_ENOUGH_MEMORY) };
            return None;
        }
        if self.base == 0 {
            self.base =
                unsafe { VirtualAlloc(ptr::null(), ARENA, MEM_RESERVE, PAGE_READWRITE) }.addr();
            if self.base == 0 {
                return None;
            }
        }
        let address = self.base + index * BLOCK;
        if unsafe { VirtualAlloc(address as *const _, BLOCK, MEM_COMMIT, PAGE_READWRITE) }.is_null()
        {
            return None;
        }
        if index == self.blocks.len() {
            self.blocks.push(Some(Block::new()));
        } else {
            self.blocks[index] = Some(Block::new());
        }
        self.commits = self.commits.wrapping_add(1);
        Some(index)
    }

    fn trim(&mut self) {
        let mut excess = self.empty_blocks().saturating_sub(self.target as usize);
        for (index, block) in self.blocks.iter_mut().enumerate().rev() {
            if excess == 0 {
                break;
            }
            if block.as_ref().is_some_and(Block::is_empty) {
                let address = (self.base + index * BLOCK) as *mut c_void;
                if unsafe { VirtualFree(address, BLOCK, MEM_DECOMMIT) } != 0 {
                    *block = None;
                    self.decommits = self.decommits.wrapping_add(1);
                    excess -= 1;
                }
            }
        }
    }
}

static POOLS: LazyLock<[Mutex<Pool>; 32]> = LazyLock::new(|| {
    std::array::from_fn(|index| {
        Mutex::new(Pool {
            target: if matches!(index & 15, 1..=6 | 10..=12) {
                16
            } else {
                2
            },
            base: 0,
            blocks: Vec::new(),
            commits: 0,
            decommits: 0,
        })
    })
});

// Free template pools on explicit DLL unload. At this point there are no
// in-flight XMem API calls and allocation ownership has returned to the DLL.
pub(super) fn unload() {
    if let Some(pools) = LazyLock::get(&POOLS) {
        for pool in pools {
            let mut pool = pool.lock().unwrap_or_else(|e| e.into_inner());
            if pool.base != 0 {
                unsafe { VirtualFree(pool.base as *mut c_void, 0, MEM_RELEASE) };
                pool.base = 0;
            }
            drop(std::mem::take(&mut pool.blocks));
        }
    }
}

fn pool(kind: u32, large: i32) -> Option<std::sync::MutexGuard<'static, Pool>> {
    if kind >= 16 {
        unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
        return None;
    }
    super::options::initialize_pool();
    let index = kind as usize + if large != 0 { 16 } else { 0 };
    Some(POOLS[index].lock().unwrap_or_else(|e| e.into_inner()))
}

#[unsafe(export_name = "XMemGetAllocationHysteresis")]
extern "system" fn get_hysteresis(kind: u32, large: i32) -> usize {
    let Some(pool) = pool(kind, large) else {
        return usize::MAX;
    };
    unsafe { SetLastError(0) };
    pool.target.wrapping_shl(22) as usize
}

#[unsafe(export_name = "XMemSetAllocationHysteresis")]
extern "system" fn set_hysteresis(kind: u32, large: i32, bytes: usize) -> i32 {
    let Some(mut pool) = pool(kind, large) else {
        return 0;
    };
    pool.target = (bytes.wrapping_add(BLOCK - 1) >> 22) as u32;
    pool.trim();
    unsafe { SetLastError(0) };
    1
}

#[unsafe(export_name = "XMemPreallocateFreeSpace")]
extern "system" fn preallocate(kind: u32, large: i32) -> i32 {
    let Some(mut pool) = pool(kind, large) else {
        return 0;
    };
    for _ in pool.empty_blocks()..pool.target as usize {
        if kind != 0 {
            unsafe { SetLastError(ERROR_NOT_SUPPORTED) };
            return 0;
        }
        if pool.commit().is_none() {
            return 0;
        }
    }
    unsafe { SetLastError(0) };
    1
}

#[repr(C)]
struct Statistics {
    size: u32,
    reserved: u32,
    extent: u64,
    cached: u64,
    commits: u32,
    decommits: u32,
}

#[unsafe(export_name = "XMemGetAllocationStatistics")]
unsafe extern "system" fn statistics(kind: u32, large: i32, out: *mut Statistics) -> i32 {
    if out.is_null() || unsafe { out.cast::<u32>().read_unaligned() } != 32 {
        unsafe { SetLastError(ERROR_BAD_LENGTH) };
        return 0;
    }
    let Some(pool) = pool(kind, large) else {
        return 0;
    };
    // The DWORD following Size is caller-owned padding.
    unsafe {
        (&raw mut (*out).extent).write_unaligned((pool.blocks.len() * BLOCK) as u64);
        (&raw mut (*out).cached).write_unaligned((pool.empty_blocks() * BLOCK) as u64);
        (&raw mut (*out).commits).write_unaligned(pool.commits);
        (&raw mut (*out).decommits).write_unaligned(pool.decommits);
        SetLastError(0);
    }
    1
}

pub(super) fn is_arena(base: usize) -> bool {
    LazyLock::get(&POOLS).is_some_and(|pools| {
        pools.iter().any(|pool| pool.lock().unwrap_or_else(|e| e.into_inner()).base == base)
    })
}

pub(super) fn cpu_attributes(attr: u64) -> bool {
    (attr >> 29) & 15 == 0 && attr & (1 << 33) == 0 && ((attr >> 24) & 31) <= 16
}

pub(super) fn allocate(bytes: usize, attr: u64) -> *mut c_void {
    let exponent = ((attr >> 24) & 31) as u32;
    let large = bytes > 0xf00 || exponent > 12;
    let granularity = if large { 4096 } else { 256 };
    let alignment = granularity.max(1 << exponent);
    let length = bytes.next_multiple_of(granularity);
    let mut pool = pool(0, i32::from(large)).unwrap();
    let base = pool.base;
    for (index, block) in pool.blocks.iter_mut().enumerate() {
        let Some(block) = block else { continue };
        let mut offset = block.last_key_value().map_or(0, |(&start, &size)| {
            (start + size).next_multiple_of(alignment)
        });
        if offset + length > BLOCK {
            offset = 0;
            for (&start, &size) in block.iter() {
                if offset + length <= start {
                    break;
                }
                offset = (start + size).next_multiple_of(alignment);
            }
        }
        if offset + length <= BLOCK {
            block.insert(offset, length);
            return (base + index * BLOCK + offset) as *mut c_void;
        }
    }
    let Some(index) = pool.commit() else {
        return ptr::null_mut();
    };
    pool.blocks[index].as_mut().unwrap().insert(0, length);
    (pool.base + index * BLOCK) as *mut c_void
}

pub(super) fn release(address: usize) -> bool {
    for large in [0, 1] {
        let mut pool = pool(0, large).unwrap();
        let Some(offset) = address.checked_sub(pool.base) else {
            continue;
        };
        if let Some(Some(block)) = pool.blocks.get_mut(offset / BLOCK)
            && block.remove(&(offset % BLOCK)).is_some()
        {
            if block.is_empty() {
                pool.trim();
            }
            return true;
        }
    }
    false
}
