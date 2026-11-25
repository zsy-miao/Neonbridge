//! Placeholder reservations inside the title's GPU-visible sparse address space.
//! Splitting and replacing change the logical regions, never release their addresses to
//! Windows. The backing reservation returns to UMA after its last region is released.

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::Foundation::{SetLastError, ERROR_INVALID_ADDRESS, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY};
use windows_sys::Win32::System::Memory as mem;

const RESERVE_PLACEHOLDER: u32 = 0x40000;
const REPLACE_PLACEHOLDER: u32 = 0x4000;
const PRESERVE: u32 = mem::MEM_RELEASE | 2;
const COALESCE: u32 = mem::MEM_RELEASE | 1;
const GRAIN: usize = crate::gpu::uma::GRAIN;

struct Backing(usize);

impl Drop for Backing {
    fn drop(&mut self) {
        if let Some(u) = crate::gpu::uma::get() {
            u.sparse_release(self.0);
        }
    }
}

#[derive(Clone)]
struct Region {
    end: usize,
    placeholder: bool,
    backing: Arc<Backing>,
}

static REGIONS: Mutex<BTreeMap<usize, Region>> = Mutex::new(BTreeMap::new());

/// Up to 1 GiB in window 0 when it has room: titles put descriptor heaps there, and shaders
/// address descriptor tables with 32 bits (the retail game's first placeholder begins with its view heap).
fn place(u: &crate::gpu::uma::Uma, len: usize) -> Option<usize> {
    if len <= 1 << 30 {
        if let Some((at, _)) = u.sparse_alloc(len, GRAIN) {
            return Some(at);
        }
    }
    u.sparse_reserve(len)
}

unsafe fn fail(error: u32) -> *mut c_void {
    SetLastError(error);
    std::ptr::null_mut()
}

/// None means this is an ordinary allocation, to be handled by kernelx.
pub(super) unsafe fn alloc(addr: *const c_void, size: usize, flags: u32, protect: u32) -> Option<*mut c_void> {
    if flags & RESERVE_PLACEHOLDER != 0 {
        if flags & (mem::MEM_RESERVE | mem::MEM_COMMIT) != mem::MEM_RESERVE || protect != mem::PAGE_NOACCESS || size == 0 {
            return Some(fail(ERROR_INVALID_PARAMETER));
        }
        if !addr.is_null() {
            return Some(fail(ERROR_INVALID_ADDRESS));
        }
        let Some(len) = size.checked_next_multiple_of(GRAIN) else { return Some(fail(ERROR_NOT_ENOUGH_MEMORY)) };
        let Some(base) = crate::gpu::uma::get().and_then(|u| place(u, len)) else { return Some(fail(ERROR_NOT_ENOUGH_MEMORY)) };
        REGIONS.lock().unwrap().insert(base, Region { end: base + len, placeholder: true, backing: Arc::new(Backing(base)) });
        return Some(base as *mut c_void);
    }
    if flags & REPLACE_PLACEHOLDER == 0 || flags & mem::MEM_RESERVE == 0 {
        return None;
    }
    let base = addr as usize;
    let mut regions = REGIONS.lock().unwrap();
    let Some(r) = regions.get_mut(&base).filter(|r| r.placeholder && base.checked_add(size) == Some(r.end)) else {
        return Some(fail(ERROR_INVALID_ADDRESS));
    };
    if flags & mem::MEM_COMMIT != 0 && super::commit_sparse(base, size, protect).is_null() {
        return Some(std::ptr::null_mut());
    }
    r.placeholder = false;
    Some(base as *mut c_void)
}

/// None means the address is not one of our placeholder regions or replacements.
pub(super) unsafe fn free(addr: *mut c_void, size: usize, flags: u32) -> Option<i32> {
    let base = addr as usize;
    let mut regions = REGIONS.lock().unwrap();
    let (&start, r) = regions.range(..=base).next_back().filter(|(_, r)| base < r.end)?;
    let r = r.clone();
    let fail = || { SetLastError(ERROR_INVALID_PARAMETER); Some(0) };
    if flags == PRESERVE && r.placeholder {
        let Some(end) = base.checked_add(size).filter(|&end| size > 0 && end <= r.end && end.is_multiple_of(GRAIN)) else { return fail() };
        if !base.is_multiple_of(GRAIN) {
            return fail();
        }
        regions.remove(&start);
        if start < base {
            regions.insert(start, Region { end: base, ..r.clone() });
        }
        regions.insert(base, Region { end, ..r.clone() });
        if end < r.end {
            regions.insert(end, r);
        }
        return Some(1);
    }
    if flags == COALESCE {
        let Some(end) = base.checked_add(size).filter(|&end| end > base) else { return fail() };
        let mut next = base;
        let mut keys = Vec::new();
        for (&at, region) in regions.range(base..end) {
            if at != next || !region.placeholder || !Arc::ptr_eq(&r.backing, &region.backing) || region.end > end {
                return fail();
            }
            keys.push(at);
            next = region.end;
        }
        if start != base || next != end {
            return fail();
        }
        for key in keys {
            regions.remove(&key);
        }
        regions.insert(base, Region { end, ..r });
        return Some(1);
    }
    if flags == mem::MEM_DECOMMIT {
        if r.placeholder || (size == 0 && start != base) || base.checked_add(size).is_none_or(|end| end > r.end) {
            return fail();
        }
        let len = if size == 0 { r.end - base } else { size };
        super::super::d3d12x::sparse::decommitted(base, len);
        return Some(mem::VirtualFree(addr, len, mem::MEM_DECOMMIT));
    }
    if start != base || (flags != PRESERVE && flags != mem::MEM_RELEASE) || (flags == mem::MEM_RELEASE && size != 0)
        || (flags == PRESERVE && size != 0 && size != r.end - base)
    {
        return fail();
    }
    if !r.placeholder {
        super::super::d3d12x::sparse::decommitted(base, r.end - base);
        if mem::VirtualFree(addr, r.end - base, mem::MEM_DECOMMIT) == 0 {
            return Some(0);
        }
    }
    if flags == PRESERVE {
        regions.get_mut(&base).unwrap().placeholder = true;
    } else {
        regions.remove(&base);
    }
    Some(1)
}
