//! CPU page tokens for Frosty's 64 KiB allocation mode.
use super::shared_pages::{self, Section};
use core::{ffi::c_void, mem::size_of, ptr, slice};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use windows_sys::Win32::Foundation::{
    ERROR_BUSY, ERROR_INVALID_ADDRESS, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY,
    ERROR_NOT_SUPPORTED, GetLastError, SetLastError,
};
use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_DECOMMIT, MEM_RESERVE, MEMORY_BASIC_INFORMATION, PAGE_READWRITE, VirtualAlloc,
    VirtualFree, VirtualQuery,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

const PAGE: usize = shared_pages::PAGE;
const FLAGS: u64 = 0x2040_0000;
mod copy_mappings;

struct Page {
    section: Arc<Section>,
    offset: u64,
    aliases: usize,
    has_section_data: bool,
    mapped: bool,
    saved: Vec<u8>,
}

#[derive(Default)]
struct Pool {
    next: u64,
    pages: HashMap<u64, Page>,
    mappings: HashMap<usize, u64>,
    gpu_aliases: HashSet<usize>,
    // Contiguous token batches share one OS view until a partial unmap.
    views: BTreeMap<usize, usize>,
    // Only split apertures with live views are retained. An empty original
    // reservation is coalesced and may be released directly by the guest.
    apertures: BTreeMap<usize, (usize, usize)>,
}

static POOL: LazyLock<Mutex<Pool>> = LazyLock::new(|| Mutex::new(Pool::default()));

static RETURNED_TO_SYSTEM: AtomicU64 = AtomicU64::new(0);

fn available_memory() -> Result<u64, u32> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut info = MEMORYSTATUSEX {
        dwLength: size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    if unsafe { GlobalMemoryStatusEx(&mut info) } == 0 {
        return Err(unsafe { GetLastError() });
    }
    Ok(info.ullAvailPhys.min(info.ullAvailPageFile))
}

pub(super) fn relinquished() -> u64 {
    RETURNED_TO_SYSTEM.load(Ordering::Acquire)
}

/// 64 KiB guest page tokens draw on sixteen ordinary host pages each.
#[unsafe(export_name = "XMemGetFreePageStatistics")]
unsafe extern "system" fn free_page_statistics(output: *mut [u64; 7]) -> i32 {
    if output.is_null() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let available = match available_memory() {
        Ok(bytes) => bytes.saturating_sub(relinquished()),
        Err(error) => return failure(error),
    };
    let host_pages =
        (available >> 12).saturating_sub(pool.pages.len() as u64 * (PAGE >> 12) as u64);
    // One title memory node, regular pages only. The console repeats the node
    // counters in the second set on a single-pool system. TuxBox owns no
    // standby page list or physically contiguous large-page pool.
    unsafe { output.write_unaligned([0, host_pages, 0, 0, host_pages, 0, 0]) };
    1
}

pub(super) fn relinquish(bytes: u64) -> Result<(), u32> {
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let returned = relinquished()
        .checked_add(bytes)
        .ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
    let available = available_memory()?;
    if returned > available.saturating_sub(pool.pages.len() as u64 * PAGE as u64) {
        return Err(ERROR_NOT_ENOUGH_MEMORY);
    }
    RETURNED_TO_SYSTEM.store(returned, Ordering::Release);
    Ok(())
}

fn failure(code: u32) -> i32 {
    // SAFETY: this sets the calling thread's last-error code.
    unsafe { SetLastError(code) };
    0
}

impl Pool {
    fn allocate(&mut self, count: usize) -> Result<u64, u32> {
        let available = available_memory()?.saturating_sub(relinquished());
        let max_pages = (available / PAGE as u64).saturating_sub(self.pages.len() as u64);
        if count as u64 > max_pages {
            return Err(ERROR_NOT_ENOUGH_MEMORY);
        }
        let last = self
            .next
            .checked_add(count as u64)
            .filter(|&n| n <= u64::MAX / super::partitions::TITLE_PAGE_STRIDE)
            .ok_or(ERROR_NOT_ENOUGH_MEMORY)?;
        self.pages
            .try_reserve(count)
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        let first = self.next + 1;
        let section = Arc::new(Section::new(count));
        for token in first..=last {
            self.pages.insert(
                token * super::partitions::TITLE_PAGE_STRIDE,
                Page {
                    mapped: false,
                    aliases: 0,
                    saved: Vec::new(),
                    offset: (token - first) * PAGE as u64,
                    section: Arc::clone(&section),
                    has_section_data: false,
                },
            );
        }
        self.next = last;
        Ok(first)
    }

    fn validate_unmapped(&self, tokens: &[u64]) -> Result<(), u32> {
        let mut seen = HashSet::new();
        seen.try_reserve(tokens.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        for token in tokens {
            let page = self.pages.get(token).ok_or(ERROR_INVALID_PARAMETER)?;
            if page.mapped || page.aliases != 0 {
                return Err(ERROR_BUSY);
            }
            if !seen.insert(*token) {
                return Err(ERROR_INVALID_PARAMETER);
            }
        }
        Ok(())
    }

    fn aperture(&self, address: usize) -> Option<(usize, usize)> {
        let (&base, &(bytes, _)) = self.apertures.range(..=address).next_back()?;
        (address - base < bytes).then_some((base, bytes))
    }

    // Try the placeholder operations themselves. This avoids keeping records
    // for empty apertures which games can release with plain VirtualFree.
    fn map_shared_page(&mut self, at: usize, token: u64) -> Result<bool, u32> {
        let existing = self.aperture(at);
        let info = shared_pages::region(at)?;
        if info.State != MEM_RESERVE
            || info.AllocationProtect != windows_sys::Win32::System::Memory::PAGE_NOACCESS
        {
            return if existing.is_some() {
                Err(ERROR_INVALID_ADDRESS)
            } else {
                Ok(false)
            };
        }
        let (base, extent) = existing.unwrap_or((
            info.AllocationBase.addr(),
            info.BaseAddress.addr() + info.RegionSize - info.AllocationBase.addr(),
        ));
        let page = self.pages.get_mut(&token).ok_or(ERROR_INVALID_PARAMETER)?;
        if page.mapped {
            return Err(ERROR_BUSY);
        }
        if let Err(e) = shared_pages::isolate_page(at, &info) {
            return if existing.is_some() {
                Err(e)
            } else {
                Ok(false)
            };
        }
        if let Err(e) = page.section.map(page.offset, at) {
            if existing.is_some() {
                return Err(e);
            }
            // With a full 64 KiB ordinary reservation there is no split to
            // distinguish its type; replacement reports failure without writes.
            if extent == PAGE {
                return Ok(false);
            }
            shared_pages::coalesce(base, extent)?;
            return Err(e);
        }
        if !page.saved.is_empty() {
            unsafe {
                ptr::copy_nonoverlapping(page.saved.as_ptr(), at as *mut u8, PAGE);
            }
            page.saved = Vec::new();
        }
        page.has_section_data = true;
        page.aliases += 1;
        self.mappings.insert(at, token);
        self.apertures.entry(base).or_insert((extent, 0)).1 += 1;
        Ok(true)
    }

    fn map_contiguous(&mut self, start: usize, tokens: &[u64]) -> Result<bool, u32> {
        if tokens.len() < 2 { return Ok(false); }
        let first = &self.pages[&tokens[0]];
        let section = first.section.clone();
        let offset = first.offset;
        if !tokens.iter().enumerate().all(|(i, t)| {
            let p = &self.pages[t];
            Arc::ptr_eq(&p.section, &section) && p.offset == offset + (i * PAGE) as u64
                && !p.mapped && p.saved.is_empty()
        }) { return Ok(false); }
        let bytes = tokens.len() * PAGE;
        let info = shared_pages::region(start)?;
        if info.State != MEM_RESERVE || info.AllocationProtect != windows_sys::Win32::System::Memory::PAGE_NOACCESS
            || start + bytes > info.BaseAddress.addr() + info.RegionSize { return Ok(false); }
        let base = info.AllocationBase.addr();
        let (base, extent) = self.aperture(start).unwrap_or((base, info.BaseAddress.addr() + info.RegionSize - base));
        if shared_pages::isolate_range(start, bytes, &info).is_err() { return Ok(false); }
        if let Err(e) = section.map_range(offset, start, bytes) {
            // A plain PAGE_NOACCESS reserve cannot be replaced by a section.
            if self.aperture(start).is_none() && start == base && bytes == extent { return Ok(false); }
            if self.aperture(start).is_none() { shared_pages::coalesce(base, extent)?; }
            return Err(e);
        }
        self.views.insert(start, start + bytes);
        self.apertures.entry(base).or_insert((extent, 0)).1 += tokens.len();
        for (i, &token) in tokens.iter().enumerate() {
            let p = self.pages.get_mut(&token).unwrap();
            p.aliases += 1;
            p.has_section_data = true;
            self.mappings.insert(start + i * PAGE, token);
        }
        Ok(true)
    }

    // Restore section-backed fragments around an unmapped interval. Guest
    // aliases retain their original bytes and file offsets on both sides.
    fn unmap_view_range(&mut self, at: usize, bytes: usize) -> Result<bool, u32> {
        let Some((&lo, &hi)) = self.views.range(..=at).next_back() else { return Ok(false); };
        let end = at + bytes;
        if at >= hi || end > hi { return Ok(false); }
        let (base, extent) = self.aperture(at).ok_or(ERROR_INVALID_ADDRESS)?;
        let first = &self.pages[&self.mappings[&lo]];
        let section = first.section.clone();
        let offset = first.offset;
        shared_pages::unmap(lo)?;
        let mut restored = Vec::new();
        let resize = (|| {
            for (a, b) in [(lo, at), (end, hi)] {
                if a == b { continue; }
                shared_pages::isolate_range(a, b - a, &shared_pages::region(a)?)?;
                section.map_range(offset + (a - lo) as u64, a, b - a)?;
                restored.push((a, b));
            }
            if self.apertures[&base].1 == bytes / PAGE {
                shared_pages::coalesce(base, extent)?;
            }
            Ok(())
        })();
        if let Err(e) = resize {
            for (a, _) in restored { shared_pages::unmap(a)?; }
            shared_pages::coalesce(lo, hi - lo)?;
            section.map_range(offset, lo, hi - lo)?;
            return Err(e);
        }
        self.views.remove(&lo);
        self.views.extend(restored);
        for a in (at..end).step_by(PAGE) {
            let t = self.mappings.remove(&a).unwrap();
            self.pages.get_mut(&t).unwrap().aliases -= 1;
        }
        let live = &mut self.apertures.get_mut(&base).unwrap().1;
        *live -= bytes / PAGE;
        if *live == 0 { self.apertures.remove(&base); }
        Ok(true)
    }

    fn unmap_shared_page(&mut self, at: usize) -> Result<(), u32> {
        if self.unmap_view_range(at, PAGE)? { return Ok(()); }
        let (base, extent) = self.aperture(at).ok_or(ERROR_INVALID_ADDRESS)?;
        let token = *self.mappings.get(&at).ok_or(ERROR_INVALID_ADDRESS)?;
        shared_pages::unmap(at)?;
        if self.apertures[&base].1 == 1 {
            if let Err(error) = shared_pages::coalesce(base, extent) {
                // The failed coalesce left the original placeholder intact.
                // Restore the caller's view before reporting the failure.
                let page = &self.pages[&token];
                page.section.map(page.offset, at)?;
                return Err(error);
            }
            self.apertures.remove(&base);
        } else {
            self.apertures.get_mut(&base).unwrap().1 -= 1;
        }
        self.pages.get_mut(&token).unwrap().aliases -= 1;
        self.mappings.remove(&at);
        Ok(())
    }

    fn map(&mut self, address: usize, bytes: usize, tokens: &[u64]) -> Result<(), u32> {
        if tokens.iter().any(|t| !self.pages.contains_key(t)) {
            return Err(ERROR_INVALID_PARAMETER);
        }
        self.mappings
            .try_reserve(tokens.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        if self.map_contiguous(address, tokens)? { return Ok(()); }
        if self.map_shared_page(address, tokens[0])? {
            let (base, extent) = self.aperture(address).unwrap();
            for (i, &token) in tokens.iter().enumerate().skip(1) {
                let at = address + i * PAGE;
                let result = if at - base < extent {
                    self.map_shared_page(at, token)
                } else {
                    Err(ERROR_INVALID_ADDRESS)
                };
                if !matches!(result, Ok(true)) {
                    for prior in (0..i).rev() {
                        self.unmap_shared_page(address + prior * PAGE)?;
                    }
                    return Err(result.err().unwrap_or(ERROR_INVALID_ADDRESS));
                }
            }
            return Ok(());
        }
        self.validate_unmapped(tokens)?;
        let mut info = MEMORY_BASIC_INFORMATION::default();
        let mut cursor = address;
        let mut reservation = ptr::null_mut::<c_void>();
        while cursor < address + bytes {
            // SAFETY: VirtualQuery writes only the specified local structure.
            if unsafe {
                VirtualQuery(
                    cursor as _,
                    &mut info,
                    size_of::<MEMORY_BASIC_INFORMATION>(),
                )
            } == 0
                || info.State != MEM_RESERVE
                || (!reservation.is_null() && info.AllocationBase != reservation)
            {
                return Err(ERROR_INVALID_ADDRESS);
            }
            // Newly reserved and previously decommitted pages can form separate
            // VirtualQuery regions within the same reservation.
            reservation = info.AllocationBase;
            cursor = (info.BaseAddress as usize)
                .checked_add(info.RegionSize)
                .filter(|&end| end > cursor)
                .ok_or(ERROR_INVALID_ADDRESS)?;
        }
        if (0..tokens.len()).any(|i| self.mappings.contains_key(&(address + i * PAGE))) {
            return Err(ERROR_BUSY);
        }
        // SAFETY: caller owns the reserved range exclusively during this operation.
        if unsafe { VirtualAlloc(address as _, bytes, MEM_COMMIT, PAGE_READWRITE) }.is_null() {
            return Err(unsafe { GetLastError() });
        }
        for (i, &token) in tokens.iter().enumerate() {
            let p = &self.pages[&token];
            if p.has_section_data && p.saved.is_empty()
                && let Err(e) = p.section.read_into(p.offset, (address + i * PAGE) as *mut u8)
            {
                unsafe { VirtualFree(address as _, bytes, MEM_DECOMMIT); }
                return Err(e);
            }
        }
        for (i, token) in tokens.iter().enumerate() {
            let at = address + i * PAGE;
            let page = self
                .pages
                .get_mut(token)
                .expect("validated under pool lock");
            if !page.saved.is_empty() {
                // SAFETY: saved holds one full page and VirtualAlloc committed its destination.
                unsafe { ptr::copy_nonoverlapping(page.saved.as_ptr(), at as *mut u8, PAGE) };
                page.saved = Vec::new();
            }
            page.mapped = true;
            self.mappings.insert(at, *token);
        }
        Ok(())
    }

    fn unmap(&mut self, address: usize, bytes: usize) -> Result<(), u32> {
        if self.unmap_view_range(address, bytes)? { return Ok(()); }
        if let Some((base, extent)) = self.aperture(address) {
            if address + bytes > base + extent {
                return Err(ERROR_INVALID_ADDRESS);
            }
            let mut entries = Vec::new();
            entries
                .try_reserve_exact(bytes / PAGE)
                .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
            for at in (address..address + bytes).step_by(PAGE) {
                entries.push((at, *self.mappings.get(&at).ok_or(ERROR_INVALID_ADDRESS)?));
            }
            for (i, &(at, _)) in entries.iter().enumerate() {
                if let Err(e) = self.unmap_shared_page(at) {
                    for &(prior, token) in entries[..i].iter().rev() {
                        self.map_shared_page(prior, token)?;
                    }
                    return Err(e);
                }
            }
            return Ok(());
        }
        // Take a snapshot of every mapped page before changing the reservation.
        // Allocation or read failure leaves every mapping in place for a retry.
        for at in (address..address + bytes).step_by(PAGE) {
            let token = self.mappings.get(&at).ok_or(ERROR_INVALID_ADDRESS)?;
            let page = self.pages.get_mut(token).expect("mapping owns a live page");
            if !page.mapped {
                // A legacy range cannot span into another reservation's section views.
                return Err(ERROR_INVALID_ADDRESS);
            }
            if page.saved.is_empty() {
                page.saved
                    .try_reserve_exact(PAGE)
                    .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
                page.saved.resize(PAGE, 0);
            }
            // SAFETY: Windows checks the guest source range. The owned buffer holds PAGE bytes.
            if unsafe {
                ReadProcessMemory(
                    GetCurrentProcess(),
                    at as _,
                    page.saved.as_mut_ptr().cast(),
                    PAGE,
                    ptr::null_mut(),
                )
            } == 0
            {
                return Err(unsafe { GetLastError() });
            }
        }
        // SAFETY: all pages in this aligned range are registered to this provider.
        if unsafe { VirtualFree(address as _, bytes, MEM_DECOMMIT) } == 0 {
            return Err(unsafe { GetLastError() });
        }
        for at in (address..address + bytes).step_by(PAGE) {
            let token = self
                .mappings
                .remove(&at)
                .expect("validated under pool lock");
            self.pages
                .get_mut(&token)
                .expect("mapping owns a live page")
                .mapped = false;
        }
        Ok(())
    }
}

/// TuxBox extension for xcrdapi: write `len` bytes across the 64 KiB physical
/// pages named by `tokens`, starting `offset` bytes into the first page. This
/// is a DirectStorage request whose destination is physical pages (Xbox
/// request option bit 21). Returns 1 on success, 0 with the last error set.
///
/// # Safety
/// `tokens` holds enough readable entries to cover `offset + len` bytes, and
/// `data` holds `len` readable bytes.
#[unsafe(export_name = "TuxBoxWritePhysicalPages")]
pub unsafe extern "system" fn write_pages(tokens: *const u64, count: usize, offset: usize, data: *const u8, len: usize) -> i32 {
    if len == 0 {
        return 1;
    }
    if tokens.is_null() || data.is_null() || offset.checked_add(len).is_none_or(|end| end.div_ceil(PAGE) > count) {
        return failure(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller supplies `count` entries.
    let tokens = unsafe { slice::from_raw_parts(tokens, count) };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut index, mut within, mut done) = (offset / PAGE, offset % PAGE, 0usize);
    while done < len {
        let chunk = (PAGE - within).min(len - done);
        let token = tokens[index];
        // A page mapped into a legacy reservation holds its live bytes in that
        // private committed copy, not in its section. Write there, or the next
        // unmap saves the stale copy over this write.
        let legacy = match pool.pages.get(&token) {
            Some(page) if page.mapped => pool.mappings.iter().find(|&(_, t)| *t == token).map(|(&at, _)| at),
            Some(_) => None,
            None => return failure(ERROR_INVALID_ADDRESS),
        };
        let page = pool.pages.get_mut(&token).expect("checked above");
        // SAFETY: `data` has `len` bytes; `chunk` stays inside one page.
        let src = unsafe { data.add(done) };
        if let Some(at) = legacy {
            // SAFETY: the legacy mapping committed this page read-write.
            unsafe { ptr::copy_nonoverlapping(src, (at + within) as *mut u8, chunk) };
        } else if !page.saved.is_empty() {
            unsafe { ptr::copy_nonoverlapping(src, page.saved.as_mut_ptr().add(within), chunk) };
        } else {
            if let Err(e) = page.section.write_at(page.offset + within as u64, src, chunk) {
                return failure(e);
            }
            page.has_section_data = true;
        }
        done += chunk;
        index += 1;
        within = 0;
    }
    1
}

/// # Safety
/// The count points to a writable SIZE_T. The separate token array has space
/// for the requested number of UINT64 entries.
#[unsafe(export_name = "XMemAllocatePhysicalPages")]
pub unsafe extern "system" fn allocate(flags: u64, count: *mut usize, tokens: *mut u64) -> i32 {
    if count.is_null() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller owns the count for the duration of the call.
    let requested = unsafe { count.read_unaligned() };
    unsafe { count.write_unaligned(0) };
    // Scarlett callers using these pool selectors supply one token for each
    // 64 KiB chunk. The selected pools share the host pagefile backing.
    if !matches!(flags, FLAGS | 0x180 | 0x280 | 0x380) {
        let message = format!("TuxBox: unsupported XMemAllocatePhysicalPages flags={flags:#x} pages={requested}\0");
        unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(message.as_ptr()) };
        return failure(ERROR_NOT_SUPPORTED);
    }
    if requested == 0 {
        return 1;
    }
    if tokens.is_null() || requested > isize::MAX as usize / size_of::<u64>() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let first = match pool.allocate(requested) {
        Ok(first) => first,
        Err(e) => return failure(e),
    };
    for i in 0..requested {
        // SAFETY: caller provided requested entries.
        unsafe {
            tokens
                .add(i)
                .write_unaligned((first + i as u64) * super::partitions::TITLE_PAGE_STRIDE)
        };
    }
    unsafe { count.write_unaligned(requested) };
    1
}

/// # Safety
/// A nonempty token array must contain count readable, aligned entries.
#[unsafe(export_name = "XMemFreePhysicalPages")]
pub unsafe extern "system" fn free(count: usize, tokens: *const u64) -> i32 {
    if count == 0 {
        return 1;
    }
    if tokens.is_null() || !tokens.is_aligned() || count > isize::MAX as usize / size_of::<u64>() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: the caller supplies the initialized array.
    let tokens = unsafe { slice::from_raw_parts(tokens, count) };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let mut owners = HashSet::new();
    if owners.try_reserve(count).is_err() {
        return failure(ERROR_NOT_ENOUGH_MEMORY);
    }
    let mut live = 0;
    for &token in tokens {
        let Some(page) = pool.pages.get(&token) else {
            return failure(ERROR_INVALID_PARAMETER);
        };
        if !owners.insert(token) {
            return failure(ERROR_INVALID_PARAMETER);
        }
        // A plain VirtualAlloc reservation is caller-managed. An actual
        // XMem AWE aperture can be automatically unmapped on physical free.
        if page.mapped {
            return failure(ERROR_BUSY);
        }
        live += page.aliases;
    }
    if live > 0 {
        let mut mapped = Vec::new();
        if mapped.try_reserve_exact(live).is_err() {
            return failure(ERROR_NOT_ENOUGH_MEMORY);
        }
        mapped.extend(
            pool.mappings
                .iter()
                .filter(|(_, token)| owners.contains(token))
                .map(|(&at, &token)| (at, token)),
        );
        // Releasing a linear pool should require one OS unmap per view.
        // Adjacent guest addresses may belong to different section views.
        mapped.sort_unstable_by_key(|&(address, _)| address);
        let mut first = 0;
        while first < mapped.len() {
            let start = mapped[first].0;
            let mut last = first + 1;
            let result = if let Some((_, &limit)) = pool.views.range(..=start).next_back()
                && start < limit
            {
                while last < mapped.len() && mapped[last].0 < limit
                    && mapped[last].0 == mapped[last - 1].0 + PAGE {
                    last += 1;
                }
                pool.unmap_view_range(start, (last - first) * PAGE)
                    .and_then(|done| if done { Ok(()) } else { Err(ERROR_INVALID_ADDRESS) })
            } else {
                pool.unmap_shared_page(start)
            };
            if let Err(error) = result {
                for &(prior, token) in mapped[..first].iter().rev() {
                    if !matches!(pool.map_shared_page(prior, token), Ok(true)) {
                        return failure(ERROR_INVALID_ADDRESS);
                    }
                }
                return failure(error);
            }
            first = last;
        }
    }
    if live != 0 {
        let Pool { gpu_aliases, mappings, .. } = &mut *pool;
        gpu_aliases.retain(|address| mappings.contains_key(address));
    }
    for token in tokens {
        pool.pages.remove(token);
    }
    1
}

/// # Safety
/// The caller owns the reservation, serializes access to the range while this
/// call runs, and unmaps before releasing or independently decommitting it.
/// A supplied token array contains count aligned entries outside that range.
#[unsafe(export_name = "XMemMapPhysicalPages")]
pub unsafe extern "system" fn map(
    address: *mut c_void,
    count: usize,
    tokens: *const u64,
) -> *mut c_void {
    let at = address as usize;
    if at == 0 {
        failure(ERROR_INVALID_PARAMETER);
        return ptr::null_mut();
    }
    if count == 0 {
        return address;
    }
    super::diagnostics::watch("XMemMapPhysicalPages", at, count.saturating_mul(PAGE), || format!(
        "{}", if tokens.is_null() { "unmap" } else { "map" }));
    let Some(bytes) = count
        .checked_mul(PAGE)
        .filter(|&n| at.checked_add(n).is_some())
    else {
        failure(ERROR_INVALID_PARAMETER);
        return ptr::null_mut();
    };
    if !at.is_multiple_of(PAGE) || !tokens.is_aligned() {
        failure(ERROR_INVALID_PARAMETER);
        return ptr::null_mut();
    }
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let result = if tokens.is_null() {
        pool.unmap(at, bytes)
    } else {
        // SAFETY: PAGE is larger than sizeof(u64), and the checked range fits usize.
        // The caller supplies a separate initialized token array.
        pool.map(at, bytes, unsafe { slice::from_raw_parts(tokens, count) })
    };
    match result {
        Ok(()) => {
            if !pool.gpu_aliases.is_empty() {
                for at in (at..at + bytes).step_by(PAGE) { pool.gpu_aliases.remove(&at); }
            }
            address
        },
        Err(e) => {
            failure(e);
            ptr::null_mut()
        }
    }
}

/// Map or unmap individually addressed 64 KiB pages owned by this provider.
///
/// # Safety
/// The caller supplies a nonnull array of `count` aligned page addresses and,
/// if nonnull, `count` aligned page tokens. Arrays must remain readable outside
/// the mapped ranges. The caller must own and serialize access to each range.
#[unsafe(export_name = "XMemMapPhysicalPagesScatter")]
pub unsafe extern "system" fn map_scatter(
    addresses: *const *mut c_void,
    count: usize,
    tokens: *const u64,
) -> i32 {
    if addresses.is_null() || !addresses.is_aligned() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    if count == 0 {
        return 1;
    }
    if count > isize::MAX as usize / size_of::<usize>() || !tokens.is_aligned() {
        return failure(ERROR_INVALID_PARAMETER);
    }
    // SAFETY: The caller supplies separate, initialized arrays of count items.
    let input = unsafe { slice::from_raw_parts(addresses, count) };
    for &page in input {
        super::diagnostics::watch("XMemMapPhysicalPagesScatter", page as usize, PAGE, || format!(
            "{}", if tokens.is_null() { "unmap" } else { "map" }));
    }
    let failed_entry = |at: usize, old: Option<u64>, new: Option<u64>, error| {
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| {
            let line = format!("TuxBox xmem: scatter mapping failed at {at:#x}, old PFN {old:?}, new PFN {new:?}, {count} entries, error {error}\n\0");
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(line.as_ptr()) };
        });
        failure(error)
    };
    let tokens = if tokens.is_null() {
        None
    } else {
        Some(unsafe { slice::from_raw_parts(tokens, count) })
    };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let mut unique = HashSet::new();
    let mut changes = Vec::new();
    if unique.try_reserve(count).is_err() || changes.try_reserve_exact(count).is_err() {
        return failure(ERROR_NOT_ENOUGH_MEMORY);
    }
    for (index, &address) in input.iter().enumerate() {
        let at = address as usize;
        if at == 0
            || !at.is_multiple_of(PAGE)
            || at.checked_add(PAGE).is_none()
            || !unique.insert(at)
        {
            return failure(ERROR_INVALID_PARAMETER);
        }
        let old = pool.mappings.get(&at).copied();
        // NtMapUserPhysicalPagesScatter interprets each zero PFN independently.
        let new = tokens.map(|values| values[index]).filter(|&pfn| pfn != 0);
        if new.is_some_and(|pfn| !pool.pages.contains_key(&pfn)) {
            return failed_entry(at, old, new, ERROR_INVALID_PARAMETER);
        }
        if old == new {
            if old.is_none() && !shared_pages::region(at).is_ok_and(|r| r.State == MEM_RESERVE) {
                return failed_entry(at, old, new, ERROR_INVALID_ADDRESS);
            }
            continue;
        }
        if pool.gpu_aliases.contains(&at) {
            // The GPU aperture is synchronized and checked for live heap imports
            // by its queue. Scatter updates the separate CPU aperture.
            return failed_entry(at, old, new, ERROR_BUSY);
        }
        changes.push((at, old, new));
    }

    // Detach the whole previous set first so a batch can swap two exclusive
    // ordinary-reservation pages without depending on array order. Preserve
    // their contents using the same snapshot/section paths as a normal unmap.
    let restore = |pool: &mut Pool, done: usize| -> Result<(), u32> {
        for &(at, old, _) in changes[..done].iter().rev() {
            if let Some(old) = old {
                pool.map(at, PAGE, core::slice::from_ref(&old))?;
            }
        }
        Ok(())
    };
    for (index, &(at, old, new)) in changes.iter().enumerate() {
        if old.is_some()
            && let Err(error) = pool.unmap(at, PAGE)
        {
            return failed_entry(
                at,
                old,
                new,
                restore(&mut pool, index).err().unwrap_or(error),
            );
        }
    }
    for (index, &(at, old, new)) in changes.iter().enumerate() {
        if let Some(new) = new
            && let Err(error) = pool.map(at, PAGE, core::slice::from_ref(&new))
        {
            for &(prior, _, new) in changes[..index].iter().rev() {
                if new.is_some()
                    && let Err(rollback) = pool.unmap(prior, PAGE)
                {
                    return failure(rollback);
                }
            }
            return failed_entry(
                at,
                old,
                Some(new),
                restore(&mut pool, changes.len()).err().unwrap_or(error),
            );
        }
    }
    1
}
