//! CPU page tokens for Frosty's 64 KiB allocation mode.
use super::shared_pages::{self, Section};
use core::{ffi::c_void, mem::size_of, ptr, slice};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use windows_sys::Win32::Foundation::{
    ERROR_BUSY, ERROR_INVALID_ADDRESS, ERROR_INVALID_PARAMETER, ERROR_NOT_ENOUGH_MEMORY,
    ERROR_NOT_SUPPORTED, SetLastError,
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
    // Neonbridge: title placeholder reservations (base -> root). Fresh pages
    // mapped inside one take storage at their VA offset in the root's
    // section, so a VA-contiguous range is one view the host GPU can import.
    roots: BTreeMap<usize, Root>,
    /// Pages whose unmap (to no page) failed with error 87: still mapped
    /// here, unmapped again before each later batch (copy_mappings.rs).
    stuck: BTreeMap<usize, u64>,
}

struct Root {
    bytes: usize,
    section: Arc<Section>,
    bound: HashMap<u64, u64>,
}

/// Neonbridge: record a title placeholder reservation (XMemVirtualAlloc profile 7).
pub(crate) fn register_root(base: usize, bytes: usize) {
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    pool.roots.retain(|&b, r| b + r.bytes <= base || base + bytes <= b);
    // Section import needs a committed section (Section::new_committed);
    // TUXBOX_POOL_SECTION_COMMIT=1 with the import on (off by default).
    let commit = section_import() && std::env::var("TUXBOX_POOL_SECTION_COMMIT").is_ok_and(|v| v.trim() == "1");
    let section = if commit { Section::new_committed(bytes / PAGE) } else { Section::new(bytes / PAGE) };
    if commit {
        static TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let total = TOTAL.fetch_add(bytes as u64, Ordering::Relaxed) + bytes as u64;
        crate::loader::log(&format!("pool section commit: root {base:#x} of {} MiB is a SEC_COMMIT section; {} MiB of roots committed so far (TUXBOX_POOL_SECTION_COMMIT=1 on)", bytes >> 20, total >> 20));
    }
    pool.roots.insert(base, Root { bytes, section: Arc::new(section), bound: HashMap::new() });
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
        return Err(shared_pages::os_error("available_memory:73"));
    }
    // Neonbridge: page tokens are SEC_RESERVE pagefile sections, committed only
    // when a page is first mapped, so a batch is bounded by the commit limit
    // (RAM + pagefile), not by what is free right now. GTA VI asks for 6.5 GB
    // at once on a 16 GB PC; a later commit failure is reported by the map.
    // The title's own budget caps it (title_memory_cap).
    Ok(crate::title_memory_cap().map_or(info.ullTotalPageFile, |cap| info.ullTotalPageFile.min(cap)))
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
        let aliases = page.aliases;
        let previous = { MAPPINGS_CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); &mut *self }.mappings.insert(at, token);
        imports::moved(at, previous, Some(token));
        if aliases > 1 {
            self.note_alias(token, at, aliases);
        }
        self.apertures.entry(base).or_insert((extent, 0)).1 += 1;
        Ok(true)
    }

    /// A physical page now mapped at a second (or later) address, with its
    /// other addresses (re-derived from rebox 7141e58; TUXBOX_ALIAS_LOG=0 off,
    /// 32 lines at most).
    fn note_alias(&self, token: u64, at: usize, aliases: usize) {
        use std::sync::atomic::{AtomicU32, Ordering};
        static LINES: AtomicU32 = AtomicU32::new(0);
        if std::env::var_os("TUXBOX_ALIAS_LOG").is_some_and(|v| v == "0") || LINES.fetch_add(1, Ordering::Relaxed) >= 32 {
            return;
        }
        let others: Vec<String> = self.mappings.iter().filter(|&(&a, &t)| t == token && a != at).take(4).map(|(a, _)| format!("{a:#x}")).collect();
        crate::loader::log(&format!("physical page {token:#x} mapped at {at:#x}, {aliases} addresses now (also {}) from title RVA {:#x}",
            others.join(" "), crate::diagnostics::title_rva()));
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
            let aliases = p.aliases;
            let previous = { MAPPINGS_CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); &mut *self }.mappings.insert(start + i * PAGE, token);
            imports::moved(start + i * PAGE, previous, Some(token));
            if aliases > 1 {
                self.note_alias(token, start + i * PAGE, aliases);
            }
        }
        self.merge_views(start);
        Ok(true)
    }

    fn rebind(&mut self, address: usize, tokens: &[u64]) {
        let Some((&rb, root)) = self.roots.range_mut(..=address).next_back() else { return };
        if address + tokens.len() * PAGE > rb + root.bytes { return; }
        for (i, token) in tokens.iter().enumerate() {
            let offset = (address - rb + i * PAGE) as u64;
            if let Some(owner) = root.bound.get(&offset) && self.pages.contains_key(owner) { continue; }
            let Some(page) = self.pages.get_mut(token) else { continue };
            if page.mapped || page.aliases != 0 || page.has_section_data || !page.saved.is_empty()
                || Arc::ptr_eq(&page.section, &root.section) { continue; }
            page.section = Arc::clone(&root.section);
            page.offset = offset;
            root.bound.insert(offset, *token);
        }
    }

    fn rooted(&self, section: &Arc<Section>) -> bool {
        self.roots.values().any(|r| Arc::ptr_eq(&r.section, section))
    }

    // Neonbridge: native D3D12 imports a CPU range as a heap only when it is a
    // single mapped view. Rejoin neighbouring views that continue the same
    // section at matching offsets, as the console's contiguous GPU ranges do.
    fn merge_views(&mut self, mut start: usize) {
        while let Some(&end) = self.views.get(&start) {
            if self.views.contains_key(&end) && self.join_views(start, end) { continue; }
            break;
        }
        while let Some((&lo, _)) = self.views.range(..start).next_back().filter(|&(_, &hi)| hi == start) {
            if !self.join_views(lo, start) { break; }
            start = lo;
        }
    }

    /// Joins pages just mapped one view each at [address, address + bytes)
    /// with their VA neighbours when they continue the same section at the
    /// next offset (join_range checks the rest). The title maps one page per
    /// call, so without this a GPU range stayed thousands of 64 KiB views
    /// (test39: 73k, 4.6 GiB) that native D3D12 cannot import as one heap,
    /// and every range was shadow-copied instead until device memory ran
    /// out. A join remaps the whole view, so views stop growing at
    /// TUXBOX_XMEM_EAGER_JOIN_PAGES (default 256, 16 MiB) and a range past
    /// that is joined when d3d12 imports it. TUXBOX_XMEM_EAGER_JOIN=0 joins
    /// only when d3d12 asks.
    fn join_neighbours(&mut self, address: usize, bytes: usize) {
        static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("TUXBOX_XMEM_EAGER_JOIN").map_or(true, |v| v.trim() != "0"));
        // Eager joins only make one section's consecutive pages one view,
        // which the section import already covers without a view: skipped
        // while it is on (d3d12 still asks join_range for what it refuses).
        if !*ON || bytes == 0 || section_import() { return; }
        let follows = |pool: &Self, before: usize, after: usize| -> bool {
            let (Some(a), Some(b)) = (pool.mappings.get(&before), pool.mappings.get(&after)) else { return false };
            let (a, b) = (&pool.pages[a], &pool.pages[b]);
            !a.mapped && !b.mapped && Arc::ptr_eq(&a.section, &b.section) && b.offset == a.offset + (after - before) as u64
        };
        let end = address + bytes;
        static CAP: LazyLock<usize> = LazyLock::new(|| std::env::var("TUXBOX_XMEM_EAGER_JOIN_PAGES").ok()
            .and_then(|v| v.trim().parse().ok()).filter(|&n: &usize| n >= 2).unwrap_or(256));
        // The view a neighbour lies in, else the page itself.
        let span = |pool: &Self, at: usize| pool.views.range(..=at).next_back()
            .filter(|&(_, &b)| at < b).map_or((at, at + PAGE), |(&a, &b)| (a, b));
        let mut lo = address;
        if address >= PAGE && follows(self, address - PAGE, address) && end - span(self, address - PAGE).0 <= *CAP * PAGE {
            lo = span(self, address - PAGE).0;
        }
        let mut hi = end;
        if follows(self, end - PAGE, end) && span(self, end).1 - lo <= *CAP * PAGE {
            hi = span(self, end).1;
        }
        if hi - lo <= PAGE || (lo == address && hi == end && bytes == PAGE) { return; }
        install_joining_fault();
        if let Err(e) = self.join_range(lo, hi) {
            crate::loader::log(&format!("eager join {lo:#x}..{hi:#x} failed {e}; the pages stay as they were"));
        }
    }

    fn join_views(&mut self, lo: usize, mid: usize) -> bool {
        let hi = self.views[&mid];
        let (a, b) = (&self.pages[&self.mappings[&lo]], &self.pages[&self.mappings[&mid]]);
        if !Arc::ptr_eq(&a.section, &b.section) || b.offset != a.offset + (mid - lo) as u64
            || (self.aperture(lo) != self.aperture(mid) && !self.rooted(&a.section)) {
            return false;
        }
        let (section, offset) = (a.section.clone(), a.offset);
        let joined = (|| -> Result<(), u32> {
            shared_pages::unmap(lo)?;
            if let Err(e) = shared_pages::unmap(mid) {
                section.map_range(offset, lo, mid - lo)?;
                return Err(e);
            }
            let merged = shared_pages::coalesce_range(lo, hi - lo);
            if let Err(e) = merged.and_then(|_| section.map_range(offset, lo, hi - lo)) {
                if merged.is_ok() { shared_pages::split(lo, mid - lo)?; }
                section.map_range(offset, lo, mid - lo)?;
                section.map_range(offset + (mid - lo) as u64, mid, hi - mid)?;
                return Err(e);
            }
            Ok(())
        })();
        crate::loader::log(&format!("join views {lo:#x}..{mid:#x}..{hi:#x}: {joined:?}"));
        if joined.is_err() { return false; }
        self.views.remove(&mid);
        self.views.insert(lo, hi);
        true
    }

    // Restore section-backed fragments around an unmapped interval. Guest
    // aliases retain their original bytes and file offsets on both sides.
    fn unmap_view_range(&mut self, at: usize, bytes: usize) -> Result<bool, u32> {
        // An empty range changes nothing (it would otherwise split the view
        // holding `at` in two; VIctory 02fcf72).
        if bytes == 0 { return Ok(true); }
        let Some((&lo, &hi)) = self.views.range(..=at).next_back() else { return Ok(false); };
        let end = at + bytes;
        if at >= hi || end > hi { return Ok(false); }
        if Self::unmap_fallback_on() && Self::os_view(at) != Some((lo, hi)) {
            // test78: a recorded view the OS does not have made the unmap at
            // its base fail with error 87; the page is handled as the OS has it.
            self.views.remove(&lo);
            Self::fallback_log(format_args!("recorded view {lo:#x}..{hi:#x} is {:x?} in the OS: record dropped", Self::os_view(at)));
            return Ok(false);
        }
        let (base, extent) = self.aperture(at).ok_or(ERROR_INVALID_ADDRESS)?;
        let first = &self.pages[&self.mappings[&lo]];
        let section = first.section.clone();
        let offset = first.offset;
        // Between this unmap and the fragments' remap the view's surviving
        // pages are briefly gone: a title access there waits and retries.
        let _gap = split_retry::Gap::open(lo, hi);
        if let Err(e) = shared_pages::unmap(lo) {
            // test82: this failure (unmap:256 at a recorded view's base) had no log.
            Self::fallback_log(format_args!("page {at:#x}: unmapping its recorded view {lo:#x}..{hi:#x} to split it failed with error {e}"));
            return Err(e);
        }
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
            // test79: these splits failed with error 87 and refused the page
            // mapping. The view is back as it was; with bytes == PAGE the
            // caller rebuilds it page by page instead (rebuild_view).
            if bytes == PAGE && Self::unmap_fallback_on() {
                Self::fallback_log(format_args!("splitting {at:#x} out of view {lo:#x}..{hi:#x} failed with error {e}: view restored, rebuilt page by page instead"));
                return Ok(false);
            }
            return Err(e);
        }
        self.views.remove(&lo);
        self.views.extend(restored);
        for a in (at..end).step_by(PAGE) {
            let t = { MAPPINGS_CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); &mut *self }.mappings.remove(&a).unwrap();
            imports::moved(a, Some(t), None);
            self.pages.get_mut(&t).unwrap().aliases -= 1;
        }
        let live = &mut self.apertures.get_mut(&base).unwrap().1;
        *live -= bytes / PAGE;
        if *live == 0 { self.apertures.remove(&base); }
        Ok(true)
    }

    /// The OS view holding `at`: [its base, its end), walking regions of the
    /// same allocation.
    fn os_view(at: usize) -> Option<(usize, usize)> {
        let mapped = windows_sys::Win32::System::Memory::MEM_MAPPED;
        let info = shared_pages::region(at).ok()?;
        if info.Type != mapped { return None; }
        let lo = info.AllocationBase.addr();
        let mut hi = lo;
        while let Ok(r) = shared_pages::region(hi) {
            if r.AllocationBase.addr() != lo || r.Type != mapped || r.RegionSize == 0 { break; }
            hi = r.BaseAddress.addr() + r.RegionSize;
        }
        (hi > at).then_some((lo, hi))
    }

    fn unmap_fallback_on() -> bool {
        !std::env::var("TUXBOX_XMEM_UNMAP_FALLBACK").is_ok_and(|v| v.trim() == "0")
    }

    fn fallback_log(what: std::fmt::Arguments) {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 8 || n.is_power_of_two() {
            crate::loader::log(&format!("xmem: {what} ({n} so far; TUXBOX_XMEM_UNMAP_FALLBACK=0 off)"));
        }
    }

    /// test77/78: unmaps failed with error 87 where the OS view holding the
    /// page did not match `views` (a page inside a 2-4 page view with no
    /// record, or a recorded view the OS no longer has). Trusting the OS: the
    /// whole OS view is unmapped to a placeholder and its other pages mapped
    /// again one by one from their tokens, leaving `at` a lone placeholder.
    /// Returns false (nothing changed) when the view is just `at` or a page
    /// of it has no token here.
    fn rebuild_view(&mut self, at: usize) -> Result<bool, u32> {
        if !Self::unmap_fallback_on() { return Ok(false); }
        let Some((lo, hi)) = Self::os_view(at) else { return Ok(false) };
        if (lo, hi) == (at, at + PAGE) { return Ok(false); }
        if !lo.is_multiple_of(PAGE) || !hi.is_multiple_of(PAGE) {
            Self::fallback_log(format_args!("page {at:#x} lies in an unaligned OS view {lo:#x}..{hi:#x}: left"));
            return Ok(false);
        }
        if !(lo..hi).step_by(PAGE).all(|a| self.mappings.get(&a).is_some_and(|t| self.pages.contains_key(t))) {
            Self::fallback_log(format_args!("page {at:#x} lies in a {} KiB OS view at {lo:#x} with pages this pool does not map: left", (hi - lo) >> 10));
            return Ok(false);
        }
        // OS views never overlap, so a recorded view overlapping this one
        // but not equal to it is stale (test79: none of these logged).
        let stale: Vec<(usize, usize)> = self.views.range(..hi).filter(|&(&a, &b)| a < hi && lo < b).map(|(&a, &b)| (a, b)).collect();
        if let Err(e) = shared_pages::unmap(lo) {
            Self::fallback_log(format_args!("page {at:#x}: unmapping its OS view {lo:#x}..{hi:#x} failed with error {e} (recorded views over it {stale:x?})"));
            return Err(e);
        }
        for (a, _) in &stale { self.views.remove(a); }
        let remap = |pool: &Self, a: usize| -> Result<(), u32> {
            shared_pages::isolate_page(a, &shared_pages::region(a)?)?;
            if a != at {
                let page = &pool.pages[&pool.mappings[&a]];
                page.section.map(page.offset, a)?;
            }
            Ok(())
        };
        let mut done = Vec::new();
        for a in (lo..hi).step_by(PAGE) {
            if let Err(e) = remap(self, a) {
                Self::fallback_log(format_args!("page {at:#x}: rebuilding its OS view {lo:#x}..{hi:#x} failed at {a:#x} with error {e}; view mapped back whole"));
                // Back to the one view it was (one section run, as the OS had it).
                for &b in &done { let _ = shared_pages::unmap(b); }
                let _ = shared_pages::coalesce(lo, hi - lo);
                let first = &self.pages[&self.mappings[&lo]];
                first.section.map_range(first.offset, lo, hi - lo)?;
                self.views.insert(lo, hi);
                return Err(e);
            }
            if a != at { done.push(a); }
        }
        Self::fallback_log(format_args!("page {at:#x} lay in a {} KiB OS view at {lo:#x} that did not match the recorded views: view rebuilt page by page without it", (hi - lo) >> 10));
        Ok(true)
    }

    fn unmap_shared_page(&mut self, at: usize) -> Result<(), u32> {
        if self.unmap_view_range(at, PAGE)? { return Ok(()); }
        let (base, extent) = self.aperture(at).ok_or(ERROR_INVALID_ADDRESS)?;
        let token = *self.mappings.get(&at).ok_or(ERROR_INVALID_ADDRESS)?;
        if !self.rebuild_view(at)? {
            shared_pages::unmap(at)?;
        }
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
        let previous = { MAPPINGS_CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); &mut *self }.mappings.remove(&at);
        imports::moved(at, previous, None);
        Ok(())
    }

    /// Diagnostics: pages that are their own 64 KiB OS view (outside `views`),
    /// and whether VA-neighbours among them could share one view. Native D3D12
    /// imports a range as one heap only from a single view; every range it
    /// cannot import is copied through a shadow buffer each submission.
    fn single_view_stats(&self) -> String {
        let in_view = |a: usize| self.views.range(..=a).next_back().is_some_and(|(_, &hi)| a < hi);
        let mut singles: Vec<usize> = self.mappings.iter()
            .filter(|&(&a, t)| !self.pages[t].mapped && !in_view(a)).map(|(&a, _)| a).collect();
        singles.sort_unstable();
        let (mut joinable, mut other_section, mut other_offset) = (0usize, 0usize, 0usize);
        let (mut runs, mut run, mut longest) = (0usize, 0usize, 0usize);
        let mut previous = None;
        for &at in &singles {
            if previous.is_some_and(|p: usize| p + PAGE == at) {
                let (a, b) = (&self.pages[&self.mappings[&(at - PAGE)]], &self.pages[&self.mappings[&at]]);
                if !Arc::ptr_eq(&a.section, &b.section) { other_section += 1 }
                else if b.offset != a.offset + PAGE as u64 { other_offset += 1 }
                else { joinable += 1 }
                run += 1;
            } else {
                runs += 1;
                run = 1;
            }
            longest = longest.max(run);
            previous = Some(at);
        }
        let rooted = singles.iter().filter(|&&a| self.roots.range(..=a).next_back().is_some_and(|(&b, r)| a < b + r.bytes)).count();
        format!("single-page views {} ({} MiB, {} inside a root) in {runs} VA runs, longest {longest} pages; neighbour pairs: {joinable} joinable (same section, next offset), {other_section} in different sections, {other_offset} same section other offset; shared views {}",
            singles.len(), singles.len() * PAGE >> 20, rooted, self.views.len())
    }

    fn map(&mut self, address: usize, bytes: usize, tokens: &[u64]) -> Result<(), u32> {
        if bytes == 0 { return Ok(()); }
        imports::changed(address, bytes, "mapped");
        if tokens.iter().any(|t| !self.pages.contains_key(t)) {
            return Err(ERROR_INVALID_PARAMETER);
        }
        // At most one stats line per 15 s of mapping activity.
        {
            use std::sync::atomic::AtomicU64;
            static START: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
            static NEXT: AtomicU64 = AtomicU64::new(15_000);
            static CALLS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
            CALLS[usize::from(tokens.len() > 1)].fetch_add(1, Ordering::Relaxed);
            let now = START.elapsed().as_millis() as u64;
            if now >= NEXT.load(Ordering::Relaxed) {
                NEXT.store(now + 15_000, Ordering::Relaxed);
                // test177: title-mapped VA grew 1.8 -> 10.8 GB and nothing
                // said whether the title still held those pages. Live pool:
                // pages allocated, of them mapped, addresses mapped (an alias
                // counts again in a VA walk), pages at 2+ addresses.
                let mapped_pages = self.pages.values().filter(|p| p.aliases > 0).count();
                let aliased = self.pages.values().filter(|p| p.aliases > 1).count();
                crate::loader::log(&format!("map calls: {} single-page, {} multi-page; live pool: {} pages allocated ({} MiB), {} mapped, {} addresses mapped ({} MiB), {} pages at 2+ addresses; {}",
                    CALLS[0].load(Ordering::Relaxed), CALLS[1].load(Ordering::Relaxed),
                    self.pages.len(), self.pages.len() * PAGE >> 20, mapped_pages, self.mappings.len(), self.mappings.len() * PAGE >> 20, aliased,
                    self.single_view_stats()));
            }
        }
        self.mappings
            .try_reserve(tokens.len())
            .map_err(|_| ERROR_NOT_ENOUGH_MEMORY)?;
        if tokens.len() >= 16 {
            let f = &self.pages[&tokens[0]];
            crate::loader::log(&format!("map {address:#x} pages={} token0={:#x} offset={:#x} section={:p} region={:?}",
                tokens.len(), tokens[0], f.offset, Arc::as_ptr(&f.section),
                shared_pages::region(address).map(|i| (i.BaseAddress, i.AllocationBase, i.RegionSize, i.State, i.Type))));
        }
        self.rebind(address, tokens);
        if self.map_contiguous(address, tokens)? { return Ok(()); }
        if tokens.len() >= 16 { crate::loader::log(&format!("map {address:#x}: not one contiguous view")); }
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
            self.join_neighbours(address, tokens.len() * PAGE);
            return Ok(());
        }
        // A page whose unmap was deferred (copy_mappings.rs) is freed only
        // once that unmap succeeds.
        self.retry_stuck();
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
            return Err(shared_pages::os_error("map:672"));
        }
        // Committed read-write anew: no guard of section_watch stands there.
        crate::section_watch::remapped(address, bytes);
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
            let previous = { MAPPINGS_CHANGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed); &mut *self }.mappings.insert(at, *token);
            imports::moved(at, previous, Some(*token));
        }
        Ok(())
    }

    fn unmap(&mut self, address: usize, bytes: usize) -> Result<(), u32> {
        if bytes == 0 { return Ok(()); }
        imports::changed(address, bytes, "unmapped");
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
                return Err(shared_pages::os_error("unmap:751"));
            }
        }
        // SAFETY: all pages in this aligned range are registered to this provider.
        if unsafe { VirtualFree(address as _, bytes, MEM_DECOMMIT) } == 0 {
            return Err(shared_pages::os_error("unmap:756"));
        }
        crate::unmap_trace::record(address, bytes, crate::unmap_trace::PHYSICAL_UNMAP);
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

/// # Safety
/// The count points to a writable SIZE_T. The separate token array has space
/// for the requested number of UINT64 entries.
#[unsafe(export_name = "XMemAllocatePhysicalPages")]
pub unsafe extern "system" fn allocate(flags: u64, count: *mut usize, tokens: *mut u64) -> i32 {
    let requested = if count.is_null() { 0 } else { unsafe { count.read_unaligned() } };
    let result = unsafe { allocate_inner(flags, count, tokens) };
    if requests_logged() {
        let got = if count.is_null() { 0 } else { unsafe { count.read_unaligned() } };
        let first = if result != 0 && got != 0 && !tokens.is_null() { unsafe { tokens.read_unaligned() } } else { 0 };
        crate::loader::log(&format!("page request: XMemAllocatePhysicalPages flags {flags:#x} pages {requested:#x} -> {} {got:#x} pages, first token {first:#x}; title RVA {:#x}",
            if result != 0 { "ok" } else { "failed" }, crate::diagnostics::title_rva()));
    }
    result
}

/// TUXBOX_PAGE_REQUESTS=1: each physical-page allocation and mapping request
/// with its outcome and title caller, 256 lines at most (idea from leonida
/// 45c99d97 boot_trace, re-derived).
fn requests_logged() -> bool {
    use std::sync::atomic::{AtomicU32, Ordering};
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var_os("TUXBOX_PAGE_REQUESTS").is_some_and(|v| v == "1"));
    static LINES: AtomicU32 = AtomicU32::new(0);
    *ON && LINES.fetch_add(1, Ordering::Relaxed) < 256
}

unsafe fn allocate_inner(flags: u64, count: *mut usize, tokens: *mut u64) -> i32 {
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
        Err(e) => {
            crate::loader::log(&format!("XMemAllocatePhysicalPages flags={flags:#x} pages={requested:#x} failed {e}: available {:?} relinquished {:#x} held {:#x}",
                available_memory(), relinquished(), pool.pages.len()));
            return failure(e);
        }
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
    let result = unsafe { map_inner(address, count, tokens) };
    if requests_logged() {
        let first = if tokens.is_null() || count == 0 { 0 } else { unsafe { tokens.read_unaligned() } };
        crate::loader::log(&format!("page request: XMemMapPhysicalPages at {address:p} pages {count:#x} first token {first:#x}{} -> {result:p}; title RVA {:#x}",
            if tokens.is_null() { " (unmap)" } else { "" }, crate::diagnostics::title_rva()));
    }
    result
}

unsafe fn map_inner(
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
    let result = unsafe { map_scatter_inner(addresses, count, tokens) };
    if requests_logged() {
        let first = if addresses.is_null() || count == 0 { core::ptr::null_mut() } else { unsafe { addresses.read() } };
        crate::loader::log(&format!("page request: XMemMapPhysicalPagesScatter pages {count:#x} first at {first:p} -> {}; title RVA {:#x}",
            if result != 0 { "ok" } else { "failed" }, crate::diagnostics::title_rva()));
    }
    result
}

unsafe fn map_scatter_inner(
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

/// The split/rejoin gap: while an unmap takes a shared view apart, the pages
/// either side of the unmapped interval are unmapped and mapped again. A
/// title access to them in that window faults; the vectored handler waits
/// for the split to finish and retries the access only inside the view that
/// unmap took apart, only when the page is covered again, and once per
/// thread, page and unmap (an access that faults again is left to the next
/// handler). TUXBOX_XMEM_FAULT_RETRY=0 off.
mod split_retry {
    use std::cell::Cell;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

    static LO: AtomicUsize = AtomicUsize::new(0);
    static HI: AtomicUsize = AtomicUsize::new(0);
    /// Unmap generation; odd while a split is in progress.
    static GEN: AtomicU64 = AtomicU64::new(0);
    static RETRIES: AtomicU64 = AtomicU64::new(0);

    thread_local! {
        /// (page, generation) this thread last retried.
        static LAST: Cell<(usize, u64)> = const { Cell::new((usize::MAX, u64::MAX)) };
    }

    pub(super) fn enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("TUXBOX_XMEM_FAULT_RETRY").map_or(true, |v| v.trim() != "0"))
    }

    pub(super) struct Gap(bool);

    impl Gap {
        pub(super) fn open(lo: usize, hi: usize) -> Self {
            if !enabled() {
                return Self(false);
            }
            install();
            // Even -> odd: readers see the new range only with an odd generation.
            GEN.fetch_add(1, Ordering::AcqRel);
            LO.store(lo, Ordering::Release);
            HI.store(hi, Ordering::Release);
            Self(true)
        }
    }

    impl Drop for Gap {
        fn drop(&mut self) {
            if self.0 {
                GEN.fetch_add(1, Ordering::AcqRel);
            }
        }
    }

    fn install() {
        static HANDLER: std::sync::Once = std::sync::Once::new();
        HANDLER.call_once(|| unsafe {
            windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(1, Some(fault));
        });
    }

    /// Whether a retry is allowed for `page` in unmap `generation` given the
    /// thread's last retry; records it when allowed.
    pub(super) fn first_retry(last: &Cell<(usize, u64)>, page: usize, generation: u64) -> bool {
        if last.get() == (page, generation) {
            return false;
        }
        last.set((page, generation));
        true
    }

    fn covered(at: usize) -> bool {
        use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
        let mut info: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        let n = unsafe { VirtualQuery(at as *const _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
        n != 0 && info.State == MEM_COMMIT && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && info.Protect != 0
    }

    unsafe extern "system" fn fault(info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
        use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
        const CONTINUE_EXECUTION: i32 = -1;
        const CONTINUE_SEARCH: i32 = 0;
        let record = unsafe { &*(*info).ExceptionRecord };
        if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || record.NumberParameters < 2 {
            return CONTINUE_SEARCH;
        }
        let at = record.ExceptionInformation[1];
        let mut generation = GEN.load(Ordering::Acquire);
        let (lo, hi) = (LO.load(Ordering::Acquire), HI.load(Ordering::Acquire));
        // The last split's view only (in progress, or just finished when the
        // fault raced its end).
        if generation == 0 || !(lo <= at && at < hi) {
            return CONTINUE_SEARCH;
        }
        while generation % 2 == 1 {
            std::thread::yield_now();
            let now = GEN.load(Ordering::Acquire);
            if now != generation {
                generation = now;
                break;
            }
        }
        let page = at & !(super::PAGE - 1);
        let unmap = generation | 1;
        if !covered(at) || !LAST.with(|last| first_retry(last, page, unmap)) {
            return CONTINUE_SEARCH;
        }
        let n = RETRIES.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 4 || n.is_power_of_two() {
            crate::loader::log(&format!(
                "xmem: access at {at:#x} during the split of view {lo:#x}..{hi:#x} retried ({n} so far; TUXBOX_XMEM_FAULT_RETRY=0 off)"));
        }
        CONTINUE_EXECUTION
    }

    #[cfg(test)]
    #[test]
    fn a_page_is_retried_once_per_thread_and_unmap() {
        let last = Cell::new((usize::MAX, u64::MAX));
        assert!(first_retry(&last, 0x1_0000, 1));
        assert!(!first_retry(&last, 0x1_0000, 1));
        assert!(first_retry(&last, 0x2_0000, 1));
        assert!(first_retry(&last, 0x2_0000, 3));
    }
}

/// Neonbridge: the range a join is remapping. A title access to it during the
/// join (a few unmaps and one map) waits in the handler below and retries.
static JOINING: [std::sync::atomic::AtomicUsize; 2] = [const { std::sync::atomic::AtomicUsize::new(0) }; 2];
/// Bumped as each join starts and ends.
static JOIN_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

thread_local! {
    /// (page, join generation) this thread last retried after a finished join.
    static JOIN_RETRIED: std::cell::Cell<(usize, u64)> = const { std::cell::Cell::new((usize::MAX, u64::MAX)) };
}

/// Whether a fault at `page` may be retried once more for join generation
/// `generation`: once per thread, page and generation, so a real fault
/// still reaches the next handler.
fn join_retry_once(last: &std::cell::Cell<(usize, u64)>, page: usize, generation: u64) -> bool {
    if last.get() == (page, generation) { return false; }
    last.set((page, generation));
    true
}

#[cfg(test)]
#[test]
fn a_finished_join_retries_a_page_once_per_generation() {
    let last = std::cell::Cell::new((usize::MAX, u64::MAX));
    assert!(join_retry_once(&last, 0x1_0000, 2));
    assert!(!join_retry_once(&last, 0x1_0000, 2));
    assert!(join_retry_once(&last, 0x1_0000, 4));
    assert!(join_retry_once(&last, 0x2_0000, 4));
}

/// Committed memory at `at` that allows the faulting access (write = 1).
fn now_accessible(at: usize, write: bool) -> bool {
    use windows_sys::Win32::System::Memory::{MEMORY_BASIC_INFORMATION, MEM_COMMIT, PAGE_EXECUTE_READWRITE, PAGE_GUARD, PAGE_NOACCESS, PAGE_READWRITE, VirtualQuery};
    let mut info: MEMORY_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
    let found = unsafe { VirtualQuery(at as _, &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) };
    found != 0 && info.State == MEM_COMMIT && info.Protect & (PAGE_NOACCESS | PAGE_GUARD) == 0 && info.Protect != 0
        && (!write || info.Protect & (PAGE_READWRITE | PAGE_EXECUTE_READWRITE) != 0)
}

unsafe extern "system" fn joining_fault(info: *mut windows_sys::Win32::System::Diagnostics::Debug::EXCEPTION_POINTERS) -> i32 {
    use windows_sys::Win32::Foundation::EXCEPTION_ACCESS_VIOLATION;
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let record = unsafe { &*(*info).ExceptionRecord };
    if record.ExceptionCode != EXCEPTION_ACCESS_VIOLATION || record.NumberParameters < 2 {
        return CONTINUE_SEARCH;
    }
    let at = record.ExceptionInformation[1];
    let inside = |at: usize| {
        let (lo, hi) = (JOINING[0].load(Ordering::Acquire), JOINING[1].load(Ordering::Acquire));
        lo != 0 && lo <= at && at < hi
    };
    if !inside(at) {
        // The join may have finished between the fault and this check
        // (test43: a read AV at an address the crash record found mapped).
        // Retry once if the page is back. TUXBOX_JOIN_LATE_RETRY=0 off.
        static LATE: LazyLock<bool> = LazyLock::new(|| std::env::var("TUXBOX_JOIN_LATE_RETRY").map_or(true, |v| v.trim() != "0"));
        let generation = JOIN_GEN.load(Ordering::Acquire);
        let write = record.ExceptionInformation[0] == 1;
        if *LATE && generation != 0 && now_accessible(at, write)
            && JOIN_RETRIED.with(|last| join_retry_once(last, at & !(PAGE - 1), generation))
        {
            return CONTINUE_EXECUTION;
        }
        return CONTINUE_SEARCH;
    }
    while inside(at) {
        std::thread::yield_now();
    }
    CONTINUE_EXECUTION
}

impl Pool {
    /// Neonbridge: make [lo, hi) one OS view when every page in it is mapped
    /// from one section at consecutive offsets (the title maps one page per
    /// call, so a GPU range is thousands of 64 KiB views that native D3D12
    /// cannot import as one heap). Views straddling the ends widen the range.
    fn join_range(&mut self, mut lo: usize, mut hi: usize) -> Result<bool, u32> {
        if let Some((&a, &b)) = self.views.range(..=lo).next_back() && a < lo && lo < b { lo = a; }
        if let Some((&a, &b)) = self.views.range(..hi).next_back() && a < hi && hi < b { hi = b; }
        if lo >= hi || !lo.is_multiple_of(PAGE) || !hi.is_multiple_of(PAGE) { return Ok(false); }
        if self.views.get(&lo) == Some(&hi) { return Ok(true); }
        let first = self.pages.get(self.mappings.get(&lo).ok_or(ERROR_INVALID_ADDRESS)?).ok_or(ERROR_INVALID_ADDRESS)?;
        let (section, offset) = (first.section.clone(), first.offset);
        let aperture = self.aperture(lo);
        if aperture.is_none_or(|(b, n)| hi > b + n) && !self.rooted(&section) { return Ok(false); }
        for at in (lo..hi).step_by(PAGE) {
            let Some(page) = self.mappings.get(&at).and_then(|t| self.pages.get(t)) else { return Ok(false) };
            if page.mapped || !Arc::ptr_eq(&page.section, &section) || page.offset != offset + (at - lo) as u64 {
                return Ok(false);
            }
            if self.aperture(at) != aperture && !self.rooted(&section) { return Ok(false); }
        }
        // test144-148: 84-121 joins a run failed 87. A GPU import only ever
        // joins the range it asks for, so the next import a page further
        // overlaps the last view of the one before (0xfa9c00000..0xfa9c20000,
        // then 0xfa9c10000..), which that import's heap pins: the unmap is
        // refused, the join undone, and the range read through a shadow copy.
        // A join now (a) runs on past `hi` over further pages of the same
        // section run, up to TUXBOX_JOIN_AHEAD_MB (32; 0 off), stopping at a
        // pinned view, so a later import nests in one view; and (b) is not
        // tried over a pinned view (TUXBOX_JOIN_SKIP_PINNED=0 tries).
        let ahead = join_ahead();
        if ahead != 0 {
            let limit = hi.saturating_add(ahead);
            while hi < limit {
                let end = self.views.get(&hi).copied().unwrap_or(hi + PAGE);
                let ok = (hi..end).step_by(PAGE).all(|at| {
                    self.mappings.get(&at).and_then(|t| self.pages.get(t)).is_some_and(|page| {
                        !page.mapped && Arc::ptr_eq(&page.section, &section) && page.offset == offset + (at - lo) as u64
                    }) && (self.aperture(at) == aperture || self.rooted(&section))
                });
                if !ok || end > limit || imports::live_over(hi, end) {
                    break;
                }
                hi = end;
            }
        }
        if std::env::var("TUXBOX_JOIN_SKIP_PINNED").map_or(true, |v| v.trim() != "0") && imports::live_over(lo, hi) {
            static PINNED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = PINNED.fetch_add(1, Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                crate::loader::log(&format!("join range {lo:#x}..{hi:#x}: a GPU heap import pins a view in it, not joined ({n} so far; TUXBOX_JOIN_SKIP_PINNED=0 tries)"));
            }
            return Ok(false);
        }
        // The OS views in the range: shared views as recorded, other pages singly.
        let mut pieces = Vec::new();
        let mut at = lo;
        while at < hi {
            let end = self.views.get(&at).copied().unwrap_or(at + PAGE);
            pieces.push((at, end));
            at = end;
        }
        JOIN_GEN.fetch_add(1, Ordering::AcqRel);
        JOINING[1].store(hi, Ordering::Release);
        JOINING[0].store(lo, Ordering::Release);
        let attempt = || -> Result<(), u32> {
            let mut unmapped = 0;
            let undo = |unmapped: usize| -> Result<(), u32> {
                for &(a, b) in &pieces[..unmapped] { section.map_range(offset + (a - lo) as u64, a, b - a)?; }
                Ok(())
            };
            for &(a, _) in &pieces {
                if let Err(e) = shared_pages::unmap(a) { undo(unmapped)?; return Err(e); }
                unmapped += 1;
            }
            if let Err(e) = shared_pages::coalesce_range(lo, hi - lo) {
                // Placeholders of two reservations never coalesce: re-reserve
                // the range as one placeholder instead.
                let again = if e == 87 { shared_pages::re_reserve(lo, hi, &pieces) } else { Err(e) };
                crate::loader::log(&format!("join range {lo:#x}..{hi:#x}: coalesce Err({e}); re-reserve {again:?}"));
                if let Err(e) = again { undo(unmapped)?; return Err(e); }
            }
            if let Err(e) = section.map_range(offset, lo, hi - lo) {
                for &(a, b) in &pieces[..pieces.len() - 1] { shared_pages::split(a, b - a)?; }
                undo(unmapped)?;
                return Err(e);
            }
            Ok(())
        };
        // xbox-emulator phys_runs.rs break_run (GPL, credited): every other
        // thread is suspended while the views are unmapped and remapped, so
        // none touches or holds the range in between. Only a join that failed
        // 87 unfrozen (its views restored) is retried frozen: freezing every
        // join cost a snapshot and ~100 suspends each, and boot's thousands of
        // growing joins starved the title (test166: 0 presents; test167 with
        // the freeze off presented). TUXBOX_JOIN_FREEZE=0 off.
        let mut joined = attempt();
        // Once GIVE_UP frozen retries in a row have failed 87 too, joins stop
        // freezing: the freeze does not cure it, and each one stops every
        // thread (test171: 88 failed frozen retries, a 9.6 s stall at
        // t=16.2-25.9). TUXBOX_JOIN_FREEZE_GIVEUP=0 keeps retrying.
        static FAILED_FROZEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        const GIVE_UP: u32 = 3;
        let give_up = std::env::var("TUXBOX_JOIN_FREEZE_GIVEUP").map_or(true, |v| v.trim() != "0");
        if joined == Err(87) && join_freeze() && !(give_up && FAILED_FROZEN.load(Ordering::Relaxed) >= GIVE_UP) {
            let frozen = Frozen::new();
            joined = attempt();
            drop(frozen);
            if joined == Err(87) {
                if FAILED_FROZEN.fetch_add(1, Ordering::Relaxed) + 1 == GIVE_UP && give_up {
                    crate::loader::log(&format!("join freeze: {GIVE_UP} frozen retries in a row failed 87; joins no longer freeze (TUXBOX_JOIN_FREEZE_GIVEUP=0 keeps freezing)"));
                }
            } else {
                FAILED_FROZEN.store(0, Ordering::Relaxed);
            }
            crate::loader::log(&format!("join range {lo:#x}..{hi:#x}: failed 87 unfrozen; frozen retry {joined:?} (TUXBOX_JOIN_FREEZE=1 on)"));
        }
        JOINING[0].store(0, Ordering::Release);
        JOIN_GEN.fetch_add(1, Ordering::AcqRel);
        {
            static JOINS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = JOINS.fetch_add(1, Ordering::Relaxed) + 1;
            if joined.is_err() || n <= 16 || n.is_power_of_two() {
                // The OS call that failed (unmap, coalesce, map or commit), so
                // an Err(87) names its step instead of only its code.
                let step = if joined.is_err() { format!(", last failed OS step {:?}{}", shared_pages::take_os_error(), imports::describe(lo, hi)) } else { String::new() };
                crate::loader::log(&format!("join range {lo:#x}..{hi:#x} ({} views): {joined:?}{step} ({n} joins so far)", pieces.len()));
            }
        }
        joined?;
        for (a, _) in pieces { self.views.remove(&a); }
        self.views.insert(lo, hi);
        Ok(true)
    }
}

/// Neonbridge provider ABI (XCRDAPI physical-page destinations): copy `bytes`
/// from `src` into the pages named by `tokens`, starting `offset` bytes into
/// the first. A page mapped by copy gets the bytes at its address, an unmapped
/// page with saved contents in those, otherwise its section (shared by every
/// view of it). Returns 1 on success, 0 for an unknown token or short array,
/// a negative Win32 error on a failed write.
/// Title ranges the D3D12 provider imported as GPU heaps
/// (OpenExistingHeapFromAddress): the GPU keeps the pages it saw at import,
/// so physical pages mapped or unmapped inside one later diverge silently.
/// Each such range is reported loudly once (TUXBOX_IMPORT_REMAP_CHECK=0 off).
fn join_ahead() -> usize {
    static MB: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MB.get_or_init(|| std::env::var("TUXBOX_JOIN_AHEAD_MB").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(32)) << 20
}

mod imports {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// (start, end) -> (live imports, already reported).
    static RANGES: Mutex<BTreeMap<(usize, usize), (u32, bool)>> = Mutex::new(BTreeMap::new());

    fn enabled() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("TUXBOX_IMPORT_REMAP_CHECK").map_or(true, |v| v.trim() != "0"))
    }

    /// Whether a live GPU heap import overlaps [lo, hi): its views are
    /// pinned, and Windows refuses to unmap them (error 87).
    /// Imports whose last pin went, and when (for `describe`).
    static GONE: Mutex<BTreeMap<(usize, usize), std::time::Instant>> = Mutex::new(BTreeMap::new());

    /// test152 diagnostic: the live and recently released GPU imports over
    /// [lo, hi), so a join whose unmap fails 87 names what may still pin it.
    /// TUXBOX_JOIN87_DETAIL=0 off.
    pub(super) fn describe(lo: usize, hi: usize) -> String {
        if std::env::var("TUXBOX_JOIN87_DETAIL").is_ok_and(|v| v.trim() == "0") { return String::new(); }
        let live: Vec<String> = RANGES.lock().unwrap_or_else(|e| e.into_inner()).iter()
            .filter(|(k, _)| k.0 < hi && lo < k.1).take(4).map(|(k, v)| format!("{:#x}..{:#x} x{}", k.0, k.1, v.0)).collect();
        let gone: Vec<String> = GONE.lock().unwrap_or_else(|e| e.into_inner()).iter()
            .filter(|(k, _)| k.0 < hi && lo < k.1).take(4)
            .map(|(k, t)| format!("{:#x}..{:#x} {} ms ago", k.0, k.1, t.elapsed().as_millis())).collect();
        format!("; imports live over it [{}], released [{}] (TUXBOX_JOIN87_DETAIL=0 off)", live.join(", "), gone.join(", "))
    }

    pub(super) fn live_over(lo: usize, hi: usize) -> bool {
        let ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
        ranges.range(..(hi, 0)).any(|(&(a, b), &(n, _))| n != 0 && a < hi && lo < b)
    }

    pub(super) fn note(start: usize, bytes: usize, add: bool) {
        if !enabled() || bytes == 0 {
            return;
        }
        let key = (start, start.saturating_add(bytes));
        let mut ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
        if add {
            ranges.entry(key).or_insert((0, false)).0 += 1;
        } else if let Some(entry) = ranges.get_mut(&key) {
            entry.0 = entry.0.saturating_sub(1);
            if entry.0 == 0 {
                ranges.remove(&key);
                drop(ranges);
                let mut gone = GONE.lock().unwrap_or_else(|e| e.into_inner());
                if gone.len() >= 4096 { gone.clear(); }
                gone.insert(key, std::time::Instant::now());
                drop(gone);
                stale::released(key);
            }
        }
    }

    /// Every page-table change (CopyPageMappingsBatchX included, which does
    /// not pass through `changed`): the page at `at` held `previous`, now `now`.
    pub(super) fn moved(at: usize, previous: Option<u64>, now: Option<u64>) {
        let follow = follow_on();
        if previous == now || !((enabled() && stale::enabled()) || follow) {
            return;
        }
        let range = {
            let ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
            if ranges.is_empty() {
                None
            } else {
                // Ranges are keyed by start; an import is at most a few GiB, so
                // the nearest starts at or below the page are enough.
                ranges.range(..=(at, usize::MAX)).rev().take(64).find(|(k, _)| at < k.1).map(|(k, _)| *k)
            }
        };
        if follow && range.is_some() {
            let page = at & !0xffff;
            let mut pending = PENDING.lock().unwrap_or_else(|e| e.into_inner());
            if pending.last() != Some(&(page, page + 0x1_0000)) && pending.len() < 1 << 16 {
                pending.push((page, page + 0x1_0000));
            }
        }
        if enabled() && stale::enabled() {
            stale::changed(at, range, previous, now);
        }
    }

    /// five-star-recomp 61c095b (GPL-3.0, marquezini): GPU views follow XMem
    /// remaps. Every map, remap or unmap of a page inside an imported range is
    /// queued here and the D3D12 provider retires its imports over it before
    /// its next import or view lookup (TuxBoxTakeImportRemaps), so new views
    /// open the pages the title maps now; before, only a queue-ordered
    /// CopyPageMappingsBatchX retired them. Resources made earlier keep their
    /// frames, the reference's own GPU limit. Queued, not called, so no
    /// provider lock is taken under the page table's.
    /// TUXBOX_GPU_FOLLOW_REMAP=0 queues nothing.
    pub(super) static PENDING: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());

    pub(super) fn follow_on() -> bool {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var("TUXBOX_GPU_FOLLOW_REMAP").map_or(true, |v| v.trim() != "0"))
    }

    /// five-star-recomp fad0b7f (GPL-3.0, StaleGpuPages.cs by marquezini),
    /// ported: per GPU-imported range and page, the token (frame) the imported
    /// heap was made over. Each remap or unmap under it is counted, and so is
    /// a held token mapped again at another address, where GPU writes through
    /// the import now change that memory. The first 16 of each are logged in
    /// full. TUXBOX_STALE_GPU_PAGES=0 off.
    mod stale {
        use std::collections::HashMap;
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicU64, Ordering};

        const DETAILED: u64 = 16;
        const MAX_FRAMES: usize = 1 << 16;

        #[derive(Default)]
        struct Held {
            /// (range, page) -> the token the import holds there (0: none).
            pages: HashMap<((usize, usize), usize), u64>,
            /// held token -> (page, range) it is held at.
            frames: HashMap<u64, (usize, (usize, usize))>,
        }

        static HELD: Mutex<Option<Held>> = Mutex::new(None);
        static REMAPPED: AtomicU64 = AtomicU64::new(0);
        static REUSED: AtomicU64 = AtomicU64::new(0);

        pub(super) fn enabled() -> bool {
            static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
            *ON.get_or_init(|| std::env::var("TUXBOX_STALE_GPU_PAGES").map_or(true, |v| v.trim() != "0"))
        }

        fn say(line: String) {
            let line = format!("TuxBox xmem: {line}
 ");
            unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(line.as_ptr()) };
            if std::env::var_os("TUXBOX_LOG").is_some_and(|v| !v.is_empty()) {
                eprint!("{}", line.trim_end_matches(' '));
            }
        }

        fn total(count: u64) {
            if count > DETAILED && count.is_power_of_two() {
                say(format!("gpu pages: {} remaps under GPU-imported heaps so far; tokens they still hold were mapped again elsewhere {} times (TUXBOX_STALE_GPU_PAGES=0 off)",
                    REMAPPED.load(Ordering::Relaxed), REUSED.load(Ordering::Relaxed)));
            }
        }

        pub(super) fn changed(at: usize, range: Option<(usize, usize)>, previous: Option<u64>, now: Option<u64>) {
            let mut guard = HELD.lock().unwrap_or_else(|e| e.into_inner());
            let held = guard.get_or_insert_with(Default::default);
            if let Some(range) = range {
                // The first change since the import: the token before it is the one it was made over.
                let token = *held.pages.entry((range, at)).or_insert(previous.unwrap_or(0));
                if token != 0 {
                    if now == Some(token) {
                        held.pages.remove(&(range, at));
                        held.frames.remove(&token);
                    } else {
                        if held.frames.len() < MAX_FRAMES {
                            held.frames.entry(token).or_insert((at, range));
                        }
                        let n = REMAPPED.fetch_add(1, Ordering::Relaxed) + 1;
                        if n <= DETAILED {
                            let change = now.map_or("unmapped".to_string(), |t| format!("now token {t:#x}"));
                            say(format!("gpu pages: {at:#x} ({change}): the GPU-imported heap {:#x}..{:#x} still reaches token {token:#x} there ({n} so far)", range.0, range.1));
                        }
                        total(n);
                    }
                }
            }
            if let Some(token) = now
                && let Some(&(page, range)) = held.frames.get(&token)
                && page != at
            {
                let n = REUSED.fetch_add(1, Ordering::Relaxed) + 1;
                if n <= DETAILED {
                    say(format!("gpu pages: token {token:#x} mapped again at {at:#x} while the GPU-imported heap {:#x}..{:#x} still holds it at {page:#x}: GPU writes there change the memory at {at:#x} ({n} so far)", range.0, range.1));
                }
                total(n);
            }
        }

        /// The last import of `range` is gone: what it held no longer matters.
        pub(super) fn released(range: (usize, usize)) {
            let mut guard = HELD.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(held) = guard.as_mut() {
                held.pages.retain(|k, _| k.0 != range);
                held.frames.retain(|_, v| v.1 != range);
            }
        }
    }

    /// The GPU-imported ranges holding `at`, with their live import counts.
    pub(super) fn covering(at: usize) -> Vec<(usize, usize, u32)> {
        let ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
        ranges.iter().filter(|&(&(s, e), _)| s <= at && at < e).map(|(&(s, e), &(n, _))| (s, e, n)).take(4).collect()
    }

    pub(super) fn overlaps(a: (usize, usize), b: (usize, usize)) -> bool {
        a.0 < b.1 && b.0 < a.1
    }

    pub(super) fn changed(address: usize, bytes: usize, what: &str) {
        if !enabled() {
            return;
        }
        let span = (address, address.saturating_add(bytes));
        let mut ranges = RANGES.lock().unwrap_or_else(|e| e.into_inner());
        if ranges.is_empty() {
            return;
        }
        for (&(start, end), entry) in ranges.iter_mut() {
            if !entry.1 && overlaps((start, end), span) {
                entry.1 = true;
                let line = format!(
                    "TuxBox xmem: physical pages {what} at {address:#x}+{bytes:#x} inside GPU-imported range {start:#x}..{end:#x}: the imported heap keeps the old pages (TUXBOX_IMPORT_REMAP_CHECK=0 silences)\n\0");
                unsafe { windows_sys::Win32::System::Diagnostics::Debug::OutputDebugStringA(line.as_ptr()) };
                if std::env::var_os("TUXBOX_LOG").is_some_and(|v| !v.is_empty()) {
                    eprint!("{}", line.trim_end_matches('\0'));
                }
            }
        }
    }

    #[cfg(test)]
    #[test]
    fn ranges_overlap_only_when_they_share_bytes() {
        assert!(overlaps((0x1000, 0x2000), (0x1fff, 0x3000)));
        assert!(!overlaps((0x1000, 0x2000), (0x2000, 0x3000)));
        assert!(!overlaps((0x1000, 0x2000), (0, 0x1000)));
    }
}

/// D3D12 provider ABI: `add` != 0 notes a range imported as a GPU heap, 0
/// drops one such note.
#[unsafe(export_name = "TuxBoxNoteGpuImport")]
extern "system" fn note_gpu_import(start: usize, bytes: usize, add: i32) {
    imports::note(start, bytes, add != 0);
}

/// D3D12 provider ABI: moves up to `max` queued remapped page ranges
/// (start, end pairs) into `out`; returns how many.
#[unsafe(export_name = "TuxBoxTakeImportRemaps")]
unsafe extern "system" fn take_import_remaps(out: *mut [usize; 2], max: usize) -> usize {
    if out.is_null() || max == 0 {
        return 0;
    }
    let mut pending = imports::PENDING.lock().unwrap_or_else(|e| e.into_inner());
    let n = pending.len().min(max);
    for (i, (lo, hi)) in pending.drain(..n).enumerate() {
        unsafe { out.add(i).write([lo, hi]) };
    }
    n
}

#[unsafe(export_name = "TuxBoxWritePhysicalPages")]
unsafe extern "system" fn write_physical_pages(tokens: *const u64, count: usize, offset: usize, src: *const u8, bytes: usize) -> i32 {
    if bytes == 0 {
        return 1;
    }
    if tokens.is_null() || src.is_null() || offset.checked_add(bytes).is_none_or(|end| end.div_ceil(PAGE) > count) {
        return 0;
    }
    let tokens = unsafe { slice::from_raw_parts(tokens, count) };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut done, mut within) = (0usize, offset % PAGE);
    for &token in &tokens[offset / PAGE..] {
        if done == bytes {
            break;
        }
        let chunk = (PAGE - within).min(bytes - done);
        let at = if pool.pages.get(&token).is_some_and(|p| p.mapped) {
            pool.mappings.iter().find(|&(_, &t)| t == token).map(|(&a, _)| a)
        } else { None };
        let Some(page) = pool.pages.get_mut(&token) else { return 0 };
        let from = unsafe { src.add(done) };
        if let Some(at) = at {
            unsafe { ptr::copy_nonoverlapping(from, (at + within) as *mut u8, chunk) };
        } else if !page.saved.is_empty() {
            unsafe { ptr::copy_nonoverlapping(from, page.saved.as_mut_ptr().add(within), chunk) };
        } else {
            if let Err(e) = page.section.write_from(page.offset + within as u64, from, chunk) {
                return -(e as i32);
            }
            page.has_section_data = true;
        }
        done += chunk;
        within = 0;
    }
    i32::from(done == bytes)
}

/// Neonbridge provider ABI (XCRDAPI physical-page sources): the read side of
/// [`write_physical_pages`]; a page never written reads as zeros.
#[unsafe(export_name = "TuxBoxReadPhysicalPages")]
unsafe extern "system" fn read_physical_pages(tokens: *const u64, count: usize, offset: usize, dest: *mut u8, bytes: usize) -> i32 {
    if bytes == 0 {
        return 1;
    }
    if tokens.is_null() || dest.is_null() || offset.checked_add(bytes).is_none_or(|end| end.div_ceil(PAGE) > count) {
        return 0;
    }
    let tokens = unsafe { slice::from_raw_parts(tokens, count) };
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let (mut done, mut within) = (0usize, offset % PAGE);
    for &token in &tokens[offset / PAGE..] {
        if done == bytes {
            break;
        }
        let chunk = (PAGE - within).min(bytes - done);
        let Some(page) = pool.pages.get(&token) else { return 0 };
        let to = unsafe { dest.add(done) };
        let at = if page.mapped { pool.mappings.iter().find(|&(_, &t)| t == token).map(|(&a, _)| a) } else { None };
        if let Some(at) = at {
            unsafe { ptr::copy_nonoverlapping((at + within) as *const u8, to, chunk) };
        } else if !page.saved.is_empty() {
            unsafe { ptr::copy_nonoverlapping(page.saved.as_ptr().add(within), to, chunk) };
        } else if page.has_section_data {
            if let Err(e) = page.section.read_part(page.offset + within as u64, to, chunk) {
                return -(e as i32);
            }
        } else {
            unsafe { to.write_bytes(0, chunk) };
        }
        done += chunk;
        within = 0;
    }
    i32::from(done == bytes)
}

/// Neonbridge provider ABI: make [base, base + bytes) one OS view if its pages
/// allow it, so the host GPU can import it as one heap. Returns 1 when the
/// range is (now) one view, 0 when it cannot be, a negative Win32 error on a
/// failed remap (the previous views are restored).
fn install_joining_fault() {
    static HANDLER: std::sync::Once = std::sync::Once::new();
    HANDLER.call_once(|| unsafe {
        windows_sys::Win32::System::Diagnostics::Debug::AddVectoredExceptionHandler(1, Some(joining_fault));
    });
}

/// Section import on (d3d12 d26cb24 opens the section, xmem answers):
/// TUXBOX_POOL_SECTION_IMPORT=1 on (off by default).
fn section_import() -> bool {
    // Off by default: the import needs SEC_COMMIT sections, and the 6236 MiB
    // GPU pool committed at once is more than the host affords.
    static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("TUXBOX_POOL_SECTION_IMPORT").is_ok_and(|v| v.trim() == "1"));
    *ON
}

/// XPortPC era_gpu_alias::open_mapping / era_title_gpu (41da567, credited):
/// a title range is reachable by the host GPU through one
/// OpenExistingHeapFromFileMapping import of the section its pages live in,
/// at their section offsets, so the views never need joining (join_range's
/// unmap fails 87 in test168 even with every other thread frozen). For the
/// page holding `address`, writes [section handle, section bytes, offset of
/// `address` in the section, bytes from `address` that continue at
/// consecutive offsets of the same section (capped at `want`)] and returns 1;
/// 0 when xmem does not map the page. The handle stays xmem's: an importer
/// keeps its own reference through the heap it opens.
#[unsafe(export_name = "TuxBoxPageSection")]
unsafe extern "system" fn page_section(address: usize, want: usize, out: *mut [u64; 4]) -> u32 {
    // Off until d3d12's section import lands: TUXBOX_POOL_SECTION_IMPORT=1 on.
    if !section_import() || out.is_null() { return 0; }
    let pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let at = address & !(PAGE - 1);
    let Some(page) = pool.mappings.get(&at).and_then(|t| pool.pages.get(t)) else { return 0 };
    let Ok((handle, bytes)) = page.section.raw() else { return 0 };
    let start = page.offset + (address - at) as u64;
    let mut run = PAGE - (address - at);
    let mut next = at + PAGE;
    while run < want {
        let Some(p) = pool.mappings.get(&next).and_then(|t| pool.pages.get(t)) else { break };
        if !Arc::ptr_eq(&p.section, &page.section) || p.offset != page.offset + (next - at) as u64 { break; }
        run += PAGE;
        next += PAGE;
    }
    unsafe { out.write([handle as u64, bytes, start, run.min(want.max(1)) as u64]) };
    // Each section handed out the first time: its size and the running total,
    // which is what the importer makes resident against our commit.
    static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    if !seen.contains(&(handle as u64)) {
        seen.push(handle as u64);
        static TOTAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let total = TOTAL.fetch_add(bytes, Ordering::Relaxed) + bytes;
        crate::loader::log(&format!("pool section import: section {:#x} of {} MiB at {address:#x} handed out; {} section(s), {} MiB total (TUXBOX_POOL_SECTION_IMPORT=1 on)",
            handle as u64, bytes >> 20, seen.len(), total >> 20));
    }
    1
}

#[unsafe(export_name = "TuxBoxJoinPageRange")]
unsafe extern "system" fn join_page_range(base: usize, bytes: usize) -> i32 {
    install_joining_fault();
    let Some(end) = base.checked_add(bytes) else { return 0 };
    let mut pool = POOL.lock().unwrap_or_else(|e| e.into_inner());
    match pool.join_range(base & !(PAGE - 1), end.next_multiple_of(PAGE)) {
        Ok(joined) => i32::from(joined),
        Err(e) => -(e as i32),
    }
}

/// Every title address mapping the same physical page as `at` (`at` too),
/// for device_pages; just `at` when xmem does not map it. Spins on the pool
/// lock rather than blocking: the fault handler can run on a thread inside
/// a map call.
pub(crate) fn aliases(at: usize) -> Vec<usize> {
    for _ in 0..1000 {
        if let Ok(pool) = POOL.try_lock() {
            let Some(&token) = pool.mappings.get(&at) else { return vec![at] };
            // A reverse index, rebuilt when a mapping changed since.
            let changed = MAPPINGS_CHANGED.load(std::sync::atomic::Ordering::Relaxed);
            let mut index = ALIAS_INDEX.lock().unwrap_or_else(|e| e.into_inner());
            if index.0 != changed || index.1.is_empty() {
                index.1.clear();
                for (&a, &t) in &pool.mappings {
                    index.1.entry(t).or_default().push(a);
                }
                index.0 = changed;
            }
            return index.1.get(&token).cloned().unwrap_or_else(|| vec![at]);
        }
        std::thread::yield_now();
    }
    vec![at]
}

static MAPPINGS_CHANGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static ALIAS_INDEX: LazyLock<std::sync::Mutex<(u64, HashMap<u64, Vec<usize>>)>> = LazyLock::new(|| std::sync::Mutex::new((u64::MAX, HashMap::new())));

fn join_freeze() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // Off by default: no frozen retry has ever cured an 87 (test168, test171),
    // and each stops every thread. TUXBOX_JOIN_FREEZE=1 on.
    *ON.get_or_init(|| std::env::var("TUXBOX_JOIN_FREEZE").is_ok_and(|v| v.trim() == "1"))
}

/// Every other thread of the process suspended until drop (xbox-emulator
/// crates/compat/xmem/src/phys.rs Frozen, GPL, credited): each is suspended
/// and its context fetched, which waits for the suspension to land.
/// The process heap is locked first and held until every thread resumes,
/// so no suspended thread holds it while the joining thread allocates.
struct Frozen {
    handles: Vec<(isize, bool)>,
    heap: isize,
}

#[repr(C)]
#[derive(Default)]
struct ThreadEntry {
    size: u32,
    usage: u32,
    tid: u32,
    pid: u32,
    base_priority: i32,
    delta_priority: i32,
    flags: u32,
}

#[repr(C, align(16))]
struct ContextBuf([u8; 0x4d0]);

unsafe extern "system" {
    fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
    fn Thread32First(snap: isize, entry: *mut ThreadEntry) -> i32;
    fn Thread32Next(snap: isize, entry: *mut ThreadEntry) -> i32;
    fn OpenThread(access: u32, inherit: i32, tid: u32) -> isize;
    fn SuspendThread(thread: isize) -> u32;
    fn ResumeThread(thread: isize) -> u32;
    fn GetThreadContext(thread: isize, context: *mut u8) -> i32;
    fn CloseHandle(handle: isize) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn GetCurrentProcessId() -> u32;
    fn GetProcessHeap() -> isize;
    fn HeapLock(heap: isize) -> i32;
    fn HeapUnlock(heap: isize) -> i32;
}

impl Frozen {
    fn new() -> Self {
        let mut handles = Vec::with_capacity(256);
        let mut ctx = Box::new(ContextBuf([0; 0x4d0]));
        let mut ids = Vec::with_capacity(256);
        let heap = unsafe { GetProcessHeap() };
        unsafe { HeapLock(heap) };
        unsafe {
            let snap = CreateToolhelp32Snapshot(4, 0);
            if snap != -1 {
                let (me, pid) = (GetCurrentThreadId(), GetCurrentProcessId());
                let mut e = ThreadEntry { size: size_of::<ThreadEntry>() as u32, ..Default::default() };
                let mut ok = Thread32First(snap, &mut e);
                while ok != 0 {
                    if e.pid == pid && e.tid != me && ids.len() < ids.capacity() { ids.push(e.tid); }
                    ok = Thread32Next(snap, &mut e);
                }
                CloseHandle(snap);
                for &tid in &ids {
                    let h = OpenThread(0x2 | 0x8, 0, tid);
                    if h == 0 { continue; }
                    let suspended = SuspendThread(h) != u32::MAX;
                    if suspended {
                        // CONTEXT_CONTROL | CONTEXT_AMD64 at ContextFlags (+0x30).
                        ctx.0[0x30..0x34].copy_from_slice(&0x10_0001u32.to_le_bytes());
                        GetThreadContext(h, ctx.0.as_mut_ptr());
                    }
                    if handles.len() < handles.capacity() { handles.push((h, suspended)); } else {
                        if suspended { ResumeThread(h); }
                        CloseHandle(h);
                    }
                }
            }
        }
        Frozen { handles, heap }
    }
}

impl Drop for Frozen {
    fn drop(&mut self) {
        for &(h, suspended) in &self.handles {
            unsafe {
                if suspended { ResumeThread(h); }
                CloseHandle(h);
            }
        }
        unsafe { HeapUnlock(self.heap) };
    }
}
