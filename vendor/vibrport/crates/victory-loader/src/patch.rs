//! Debugging aid: writing over a mapped image's code before it runs.

use std::ffi::c_void;

use anyhow::{Result, bail};
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_IMAGE, MEMORY_BASIC_INFORMATION, PAGE_EXECUTE, PAGE_EXECUTE_READ,
    PAGE_EXECUTE_READWRITE, PAGE_EXECUTE_WRITECOPY, PAGE_GUARD, VirtualProtect, VirtualQuery,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

use crate::LoadedImage;

/// What became of [`LoadedImage::write_code`] when it did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeWrite {
    /// In, with the page protection restored and the instruction cache
    /// flushed.
    Done,
    /// Nothing was changed, for this reason.
    Refused(&'static str),
}

impl LoadedImage {
    /// Writes `bytes` over the image's code at `rva`, through a writable
    /// mapping for the moment. The range must lie in one region of the
    /// image's executable pages that share one protection, so the protection
    /// put back is right for every page; otherwise nothing is written. An
    /// error means the bytes went in but the protection could not be restored
    /// or the instruction cache flushed.
    ///
    /// # Safety
    /// No thread runs or reads these bytes while they change, and none has
    /// run them yet: call it before the image's code runs.
    pub unsafe fn write_code(&self, rva: usize, bytes: &[u8]) -> Result<CodeWrite> {
        // SAFETY: the caller's promise.
        unsafe { write_code(self.base(), self.info().size_of_image as usize, rva, bytes) }
    }
}

/// [`LoadedImage::write_code`] for the image of `size` bytes at `base`.
///
/// # Safety
/// As there.
unsafe fn write_code(base: usize, size: usize, rva: usize, bytes: &[u8]) -> Result<CodeWrite> {
    if rva.checked_add(bytes.len()).is_none_or(|end| end > size) {
        return Ok(CodeWrite::Refused("outside the image"));
    }
    let at = base + rva;
    // SAFETY: zeroes are a valid MEMORY_BASIC_INFORMATION.
    let mut region: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: describes our own address space into `region`, of the size given.
    let described = unsafe { VirtualQuery(at as *const c_void, &mut region, size_of_val(&region)) };
    if described == 0 || !in_one_code_region(&region, base, at, bytes.len()) {
        return Ok(CodeWrite::Refused(
            "not inside one executable region of the image",
        ));
    }
    let mut old = 0;
    // SAFETY: the range lies in one region of the mapped image (checked);
    // nothing runs it, as the caller promised.
    if unsafe {
        VirtualProtect(
            at as *const c_void,
            bytes.len(),
            PAGE_EXECUTE_READWRITE,
            &mut old,
        )
    } == 0
    {
        return Ok(CodeWrite::Refused("Windows would not make it writable"));
    }
    // SAFETY: writable now and inside the image; nothing reads it meanwhile.
    unsafe { (at as *mut u8).copy_from_nonoverlapping(bytes.as_ptr(), bytes.len()) };
    // SAFETY: putting back the protection taken above, on the same range.
    if unsafe { VirtualProtect(at as *const c_void, bytes.len(), old, &mut old) } == 0 {
        bail!(
            "written, but its page protection was not restored: {}",
            std::io::Error::last_os_error()
        );
    }
    // SAFETY: flushes a range of our own process.
    if unsafe { FlushInstructionCache(GetCurrentProcess(), at as *const c_void, bytes.len()) } == 0
    {
        bail!(
            "written, but the instruction cache was not flushed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(CodeWrite::Done)
}

/// Whether `len` bytes at `at` lie inside `region` (from `VirtualQuery`:
/// pages with the same state, protection and type) and it is committed,
/// executable code of the image mapped at `base`.
fn in_one_code_region(
    region: &MEMORY_BASIC_INFORMATION,
    base: usize,
    at: usize,
    len: usize,
) -> bool {
    let executable =
        PAGE_EXECUTE | PAGE_EXECUTE_READ | PAGE_EXECUTE_READWRITE | PAGE_EXECUTE_WRITECOPY;
    let start = region.BaseAddress as usize;
    region.State == MEM_COMMIT
        && region.Type == MEM_IMAGE
        && region.AllocationBase as usize == base
        && region.Protect & executable != 0
        && region.Protect & PAGE_GUARD == 0
        && at >= start
        && at
            .checked_add(len)
            .is_some_and(|end| end <= start + region.RegionSize)
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::System::Memory::{
        MEM_PRIVATE, MEM_RELEASE, MEM_RESERVE, PAGE_READONLY, VirtualAlloc, VirtualFree,
    };

    fn region(protect: u32, kind: u32) -> MEMORY_BASIC_INFORMATION {
        MEMORY_BASIC_INFORMATION {
            BaseAddress: 0x1_4000_1000 as *mut c_void,
            AllocationBase: 0x1_4000_0000 as *mut c_void,
            AllocationProtect: 0,
            PartitionId: 0,
            RegionSize: 0x2000,
            State: MEM_COMMIT,
            Protect: protect,
            Type: kind,
        }
    }

    #[test]
    fn a_write_must_lie_in_one_executable_region_of_the_image() {
        let code = region(PAGE_EXECUTE_READ, MEM_IMAGE);
        let base = 0x1_4000_0000;
        assert!(in_one_code_region(&code, base, 0x1_4000_1000, 6));
        assert!(in_one_code_region(&code, base, 0x1_4000_2FFA, 6));
        // Past the region: the next pages may have another protection.
        assert!(!in_one_code_region(&code, base, 0x1_4000_2FFB, 6));
        assert!(!in_one_code_region(&code, base, 0x1_4000_0FFF, 6));
        // Another image, data, private memory.
        assert!(!in_one_code_region(&code, 0x1_5000_0000, 0x1_4000_1000, 6));
        let data = region(PAGE_READONLY, MEM_IMAGE);
        assert!(!in_one_code_region(&data, base, 0x1_4000_1000, 6));
        let private = region(PAGE_EXECUTE_READ, MEM_PRIVATE);
        assert!(!in_one_code_region(&private, base, 0x1_4000_1000, 6));
    }

    #[test]
    fn memory_outside_an_image_is_refused_untouched() {
        // SAFETY: a fresh page of our own, released below.
        let page = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                0x1000,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_EXECUTE_READ,
            )
        };
        assert!(!page.is_null());
        let base = page as usize;
        // SAFETY: nothing runs this page.
        let private = unsafe { write_code(base, 0x1000, 0x10, &[0xC3]) }.ok();
        // SAFETY: as above.
        let outside = unsafe { write_code(base, 0x1000, 0xFFF, &[0xC3, 0xC3]) }.ok();
        // SAFETY: readable (execute-read), still ours.
        let untouched = unsafe { (page as *const u8).add(0x10).read() } == 0;
        // SAFETY: allocated above with VirtualAlloc.
        unsafe { VirtualFree(page, 0, MEM_RELEASE) };
        assert!(matches!(private, Some(CodeWrite::Refused(_))));
        assert!(matches!(outside, Some(CodeWrite::Refused(_))));
        assert!(untouched);
    }
}
