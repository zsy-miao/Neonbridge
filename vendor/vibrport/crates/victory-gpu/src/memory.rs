//! Title memory by address, for translated shaders that read and write
//! through raw 40-bit addresses (`victory_shader::emit::SLOT_MEMORY`: the
//! console's ray-tracing structure builder and the programs that walk its
//! structures; the structures themselves lie in plain title memory, in no
//! buffer object).
//!
//! The copies are the ones every other access uses ([`crate::shadow`]: a
//! title buffer's copy, or the 4 MiB window of memory outside buffers), so
//! a root view and a raw address reach the same host bytes. This module
//! keeps what a shader needs to find them:
//!
//! - the **directory**: per 64 KiB page of title address space the number
//!   of the copy that holds the page (0: none) and the page's byte offset
//!   in it; one buffer for all lists, entries only ever go from empty to
//!   filled;
//! - per view heap a run of slots with a raw unordered access view of each
//!   numbered copy (slot 0 of the run: a null view, for pages without a
//!   copy);
//! - the **faults**: the page numbers shaders found empty, read back here
//!   once a frame, so the copies exist from the next frame on. A pointer a
//!   shader takes out of memory cannot be known before it runs; the
//!   addresses a dispatch is given as root views are made ready before it.
//!
//! **A page that is not there when a shader touches it is a lost access**:
//! the read gives 0, the store is dropped, and the fault note only helps
//! the next frame. For the structure builder that is the same as a dropped
//! stage, so before a dispatch the host makes ready everything it can see
//! the dispatch name: the whole copy around each root view, around each
//! pair of root constants that is a title address and around each root
//! constant that is one in 256-byte units (the builder passes structures
//! all three ways), and the 4 MiB after each (a structure may cross a
//! window's end).
//! shortcut: what a stage reaches only through pointers it reads out of
//! memory (InitHeaders' table of structure addresses, LoadInstances'
//! instance records) is not made ready unless something else named it;
//! the fault count in the log says whether that happens. The whole answer
//! is the extent of the title's allocation around an address, which the
//! memory module of the layer knows and this one does not ask yet.
//!
//! shortcut: a copy that is numbered is kept for the rest of the run, and
//! there are at most [`COPIES`] of them (4 MiB windows mostly: 4 GiB at
//! most). Every list with such a shader brings all numbered copies up to
//! date.
//!
//! **Where shaders stored** is told by the shaders: a store by address sets
//! its page's bit behind the fault notes, the host takes the bits when a
//! submission has run ([`take_dirty`]), and a list whose shaders can store
//! declares as written (snapshot before, compare after, as for any
//! unordered access view) the copies its dispatches' written root views
//! name and the pages whose bit was set in the last [`MARK_FRAMES`]
//! frames. Declaring every numbered copy instead was 16 GB snapshotted and
//! compared a frame with the whole builder running (4 to 6 s a frame).
//! Port of fxceless/victory-fork #814.
//! shortcut: the first stores to a page no root view named stay in the
//! host's copy (the programs that walk the structures read them there)
//! and reach title memory only when a later store to the page changes
//! them again; the answer is a snapshot the card keeps, or the title's
//! bytes taken as the snapshot of a page first found written.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use victory_shader::emit::{MEMORY_DIRTY_DWORDS, MEMORY_FAULTS, MEMORY_PAGES};
use windows::Win32::Graphics::Direct3D12::*;

use crate::descriptor::host::HostHeap;
use crate::host::{HostList, host};
use crate::shadow::Shadow;

/// Copies that can be numbered (the run of heap slots is one longer). A
/// listed frame of the opening cutscene names about 50 windows.
const COPIES: u32 = 1023;
/// A window of memory outside buffers (`shadow.rs`).
const CELL: u64 = 4 << 20;
/// A directory page.
const PAGE: u64 = 1 << 16;
/// A page counts as one shaders store to for this many frames after its
/// bit was last found set ([`take_dirty`]): lists recorded while earlier
/// submissions still run declare it too.
const MARK_FRAMES: u64 = 8;

struct Memory {
    /// Buffer, its mapping, its GPU address.
    directory: (ID3D12Resource, *mut u32, u64),
    faults: (ID3D12Resource, *mut u32, u64),
    /// The fault notes of shaders whose stores are not copied back (`ready`
    /// without `writes`): their bits must not make pages count as written.
    quarantine_faults: (ID3D12Resource, *mut u32, u64),
    /// Copy `n` is `copies[n - 1]`.
    copies: Vec<Arc<Shadow>>,
    /// Pages whose bit was found set: the copy's index, and the frame.
    dirty_pages: BTreeMap<u32, (usize, u64)>,
    /// Counts the times a page joined `dirty_pages`.
    dirty_changes: usize,
    /// Per host view heap (by address): the first slot of its run, and how
    /// many copies have their view there.
    heaps: HashMap<usize, (u32, u32)>,
    /// The frame the faults were last read in.
    frame: u64,
    /// Pages asked for this frame that are not the title's memory.
    refused: std::collections::HashSet<u64>,
}

// SAFETY: the mappings are of buffers the struct owns; all access is under
// the mutex.
unsafe impl Send for Memory {}

static MEMORY: Mutex<Option<Memory>> = Mutex::new(None);

/// What a draw binds.
pub(crate) struct Ready {
    pub(crate) directory: u64,
    pub(crate) faults: u64,
    /// The view heap slot of the null view; copy `n`'s view is `n` further.
    pub(crate) base: u32,
}

impl Memory {
    fn new() -> Option<Memory> {
        let host = host()?;
        let make = |dwords: u64, what: &str| {
            let (buffer, data) = host.shared_buffer(dwords * 4)?;
            crate::host::name(&buffer, || format!("title memory by address: {what}"));
            // SAFETY: a fresh mapping of at least that many bytes.
            unsafe { std::ptr::write_bytes(data, 0, (dwords * 4) as usize) };
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            Some((buffer, data.cast::<u32>(), gpu))
        };
        tracing::info!("title memory by address: a shader reads or writes through raw addresses");
        Some(Memory {
            directory: make(u64::from(MEMORY_PAGES) * 2, "directory")?,
            faults: make(u64::from(MEMORY_FAULTS) + u64::from(MEMORY_DIRTY_DWORDS), "faults")?,
            quarantine_faults: make(u64::from(MEMORY_FAULTS) + u64::from(MEMORY_DIRTY_DWORDS), "faults, stores not copied back")?,
            copies: Vec::new(),
            dirty_pages: BTreeMap::new(),
            dirty_changes: 0,
            heaps: HashMap::new(),
            frame: u64::MAX,
            refused: Default::default(),
        })
    }

    /// Gives the page of `address` its copy, if it has none and the memory
    /// is the title's. The copy's index in `copies`, if the page has one.
    fn map(&mut self, address: u64) -> Option<usize> {
        let page = address / PAGE;
        // The last entry stays empty: addresses past the directory read it.
        // The first megabyte is no title memory: a null pointer and what a
        // shader adds to it (the host's own pages are there).
        if !(16..u64::from(MEMORY_PAGES) - 1).contains(&page) {
            return None;
        }
        // SAFETY: inside the directory's mapping.
        let entry = unsafe { self.directory.1.add(page as usize * 2) };
        // SAFETY: as above.
        let held = unsafe { entry.read() };
        if held != 0 {
            return Some(held as usize - 1);
        }
        let start = page * PAGE;
        // A shader's pointer, or a constant taken for an address, may be
        // anything: only memory the title has.
        if self.refused.contains(&page) {
            return None;
        }
        if !crate::texture::committed(start as *const u8, PAGE as usize) {
            self.refused.insert(page);
            return None;
        }
        // The copy a root view at that address would get; a buffer that
        // ends inside the page is passed over for what holds all of it.
        let whole = |s: &Arc<Shadow>| s.offset_in(start).is_some_and(|_| s.left(start) >= PAGE);
        let shadow = crate::shadow::find(start, 1)
            .filter(whole)
            .or_else(|| crate::shadow::find(start, PAGE).filter(whole))?;
        // SAFETY: a live buffer.
        let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
        if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
            return None;
        }
        let n = match self.copies.iter().position(|c| Arc::ptr_eq(c, &shadow)) {
            Some(n) => n,
            None if self.copies.len() < COPIES as usize => {
                self.copies.push(shadow.clone());
                let n = self.copies.len() - 1;
                // Every page the copy holds whole, not only the one asked
                // for: a structure is more than 64 KiB.
                // shortcut: a page of a title buffer inside this one that
                // has a copy of its own is given this copy too.
                let (first, len) = shadow.span();
                let mut at = first.next_multiple_of(PAGE);
                while at + PAGE <= first + len && at / PAGE < u64::from(MEMORY_PAGES) - 1 {
                    // SAFETY: inside the directory's mapping.
                    let other = unsafe { self.directory.1.add((at / PAGE) as usize * 2) };
                    // SAFETY: as above; the offset first, as below.
                    unsafe {
                        if other.read() == 0 {
                            other.add(1).write_volatile((at - first) as u32);
                            other.write_volatile(n as u32 + 1);
                        }
                    }
                    at += PAGE;
                }
                n
            }
            None => {
                static ONCE: std::sync::Once = std::sync::Once::new();
                ONCE.call_once(|| {
                    tracing::warn!(COPIES, "title memory by address: no number left for another copy; its pages read zeros")
                });
                return None;
            }
        };
        let offset = shadow.offset_in(start).unwrap_or(0);
        // The offset first: a shader that sees the number sees it too.
        // SAFETY: inside the directory's mapping.
        unsafe {
            entry.add(1).write_volatile(offset as u32);
            entry.write_volatile(n as u32 + 1);
        }
        Some(n)
    }

    /// The pages shaders found empty since the last frame.
    fn take_faults(&mut self) {
        let mut pages = Vec::new();
        for notes in [self.faults.1, self.quarantine_faults.1] {
            for i in 1..MEMORY_FAULTS as usize {
                // SAFETY: inside the faults' mapping; the GPU may be writing,
                // a number missed now is written again next frame.
                let at = unsafe { notes.add(i) };
                // SAFETY: as above.
                let page = unsafe { at.read_volatile() };
                if page != 0 {
                    pages.push(page);
                    // SAFETY: as above.
                    unsafe { at.write_volatile(0) };
                }
            }
        }
        if !pages.is_empty() {
            // Each is an access that read 0 or was dropped.
            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 50 {
                tracing::warn!(
                    pages = pages.len(),
                    first = format_args!("{:#x}", u64::from(pages[0]) * PAGE),
                    copies = self.copies.len(),
                    written_pages = self.dirty_pages.len(),
                    "title memory by address: shaders touched pages that had no copy (those accesses read 0 or were dropped); the copies are made now"
                );
            }
        }
        for page in pages {
            self.map(u64::from(page) * PAGE);
        }
    }

    /// What a dispatch names: the copy around `address` and the 4 MiB
    /// after it. Their indices go to `named`, if given.
    fn name(&mut self, address: u64, named: Option<&mut Vec<usize>>) {
        let copies = [self.map(address), self.map((address & !(CELL - 1)) + CELL)];
        if let Some(named) = named {
            named.extend(copies.into_iter().flatten());
        }
    }
}

/// `pages` (page, its copy's index; in page order) as runs of title memory
/// (the copy's index, address, bytes), each inside its copy (`spans`: the
/// copies' address and length).
fn coalesce_dirty_runs(pages: impl IntoIterator<Item = (u32, usize)>, spans: &[(u64, u64)]) -> Vec<(usize, u64, u64)> {
    let mut runs: Vec<(usize, u64, u64)> = Vec::new();
    for (page, copy) in pages {
        let Some(&(start, len)) = spans.get(copy) else {
            continue;
        };
        let (from, to) = ((u64::from(page) * PAGE).max(start), ((u64::from(page) + 1) * PAGE).min(start + len));
        if from >= to {
            continue;
        }
        match runs.last_mut() {
            Some((last, _, end)) if *last == copy && from <= *end => *end = (*end).max(to),
            _ => runs.push((copy, from, to)),
        }
    }
    for (_, start, end) in &mut runs {
        *end -= *start;
    }
    runs
}

/// Before a draw whose shader reads title memory by address: what its
/// root arguments name gets its copies (`read_addresses` and
/// `write_addresses`: its root views, the latter those it stores through;
/// `constants`: its root constants, see the module), every numbered copy
/// has its view in `heap`'s run and is brought up to date when `list` is
/// submitted, and with `writes` what the shaders may store to is copied
/// back after (see the module). None if the host cannot hold it; the draw
/// is then dropped.
pub(crate) fn ready(
    list: &HostList,
    heap: &HostHeap,
    read_addresses: &[u64],
    write_addresses: &[u64],
    constants: &[u32],
    writes: bool,
) -> Option<Ready> {
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    if guard.is_none() {
        *guard = Memory::new();
    }
    let m = guard.as_mut()?;
    let frame = crate::frame::current();
    if m.frame != frame {
        m.frame = frame;
        m.refused.clear();
        m.dirty_pages.retain(|_, (_, last)| frame.wrapping_sub(*last) <= MARK_FRAMES);
        m.take_faults();
    }
    // The copies the dispatch stores to through root views.
    let mut named = Vec::new();
    for &address in read_addresses {
        m.name(address, None);
    }
    for &address in write_addresses {
        m.name(address, Some(&mut named));
    }
    // A title address in two constants (low dword, then bits 32 to 39), or
    // in one in 256-byte units. A constant that only looks like one names
    // memory nothing reads; one that is not the title's memory is refused.
    for (i, &c) in constants.iter().enumerate() {
        if let Some(&high) = constants.get(i + 1)
            && (1..=0xFF).contains(&high)
            && c >= 0x1_0000
        {
            m.name(u64::from(c) | u64::from(high) << 32, None);
        }
        if c >= 0x0100_0000 {
            m.name(u64::from(c) << 8, None);
        }
    }
    named.sort_unstable();
    named.dedup();
    let key = std::ptr::from_ref(heap) as usize;
    let (base, made) = match m.heaps.get(&key) {
        Some(&run) => run,
        None => {
            // Held for the heap's life: not given back with a list's slots.
            let base = heap.dynamic(COPIES + 1)?;
            heap.null_raw_uav_at(base);
            (base, 0)
        }
    };
    for (n, shadow) in m.copies.iter().enumerate().skip(made as usize) {
        let (_, len) = shadow.span();
        heap.raw_uav_at(base + 1 + n as u32, &shadow.buffer, 0, len);
    }
    m.heaps.insert(key, (base, m.copies.len() as u32));
    // Each copy once per list, whichever list numbered it (the builder
    // alone dispatches hundreds of times a frame; the title records its
    // lists on several threads, and this list's shaders reach a copy
    // another list numbered through the directory all the same). Copies
    // are only ever added, so those a list has not taken yet are the last.
    const READ: usize = usize::MAX - 0x6d;
    const READ_COPY: usize = usize::MAX - 0x1_0000;
    const WRITE_COPY: usize = usize::MAX - 0x2_0000;
    const WRITE_PAGE: usize = usize::MAX - 0x400_0000;
    const WRITE_PAGES: usize = usize::MAX - 0x1000_0000;
    let first_read = list.first_use(READ);
    for (n, shadow) in m.copies.iter().enumerate().rev() {
        if !list.first_use(READ_COPY - n) {
            break;
        }
        let (start, len) = shadow.span();
        shadow.refresh(list, start, len);
    }
    if writes {
        WRITERS.store(true, std::sync::atomic::Ordering::Relaxed);
        for &n in &named {
            if list.first_use(WRITE_COPY - n) {
                let (start, len) = m.copies[n].span();
                m.copies[n].gpu_writes(list, start, len);
            }
        }
        // The written pages once per list, and again when more have joined
        // them (thousands of pages, hundreds of dispatches a frame).
        if list.first_use(WRITE_PAGES - (m.dirty_changes & 0xFF_FFFF)) {
            let spans: Vec<(u64, u64)> = m.copies.iter().map(|s| s.span()).collect();
            let dirty: Vec<(u32, usize)> = m
                .dirty_pages
                .iter()
                .filter(|&(&page, _)| list.first_use(WRITE_PAGE - page as usize))
                .map(|(&page, &(n, _))| (page, n))
                .collect();
            for (n, start, len) in coalesce_dirty_runs(dirty, &spans) {
                m.copies[n].gpu_writes(list, start, len);
            }
        }
    }
    if first_read {
        list.keep(&m.directory.0);
        list.keep(&m.faults.0);
        list.keep(&m.quarantine_faults.0);
    }
    Some(Ready {
        directory: m.directory.2,
        faults: if writes { m.faults.2 } else { m.quarantine_faults.2 },
        base,
    })
}

/// A list whose shaders' stores by address are copied back was recorded:
/// from then on there are bits to take.
static WRITERS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The pages whose bit is set in `mask` (`dwords` of it), in order; the
/// bits are taken (a dword at once: a bit a shader sets meanwhile stays
/// for the next time or is taken now).
///
/// # Safety
/// `mask` points to `dwords` dwords that are valid and aligned.
unsafe fn drain_dirty_bits(mask: *mut u32, dwords: usize, out: &mut Vec<u32>) {
    use std::sync::atomic::{AtomicU32, Ordering};
    for i in 0..dwords {
        // SAFETY: inside the mask (the caller's word).
        let at = unsafe { mask.add(i) };
        // SAFETY: as above. Most dwords are zero and need no exchange.
        if unsafe { at.read_volatile() } == 0 {
            continue;
        }
        // SAFETY: as above, and aligned.
        let mut word = unsafe { AtomicU32::from_ptr(at) }.swap(0, Ordering::AcqRel);
        while word != 0 {
            out.push(i as u32 * 32 + word.trailing_zeros());
            word &= word - 1;
        }
    }
}

/// After a submission ran: the pages its shaders stored to by address
/// count as written for the lists recorded from now on (see the module).
pub(crate) fn take_dirty() {
    if !WRITERS.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let frame = crate::frame::current();
    let mut guard = MEMORY.lock().unwrap_or_else(|e| e.into_inner());
    let Some(m) = guard.as_mut() else {
        return;
    };
    let mut pages = Vec::new();
    // SAFETY: the faults' buffer holds the mask behind the notes.
    unsafe { drain_dirty_bits(m.faults.1.add(MEMORY_FAULTS as usize), MEMORY_DIRTY_DWORDS as usize, &mut pages) };
    for page in pages {
        // SAFETY: inside the directory's mapping (the mask has no more bits
        // than the directory has entries).
        let copy = unsafe { m.directory.1.add(page as usize * 2).read() } as usize;
        if (1..=m.copies.len()).contains(&copy) && m.dirty_pages.insert(page, (copy - 1, frame)).is_none() {
            m.dirty_changes += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PAGE, coalesce_dirty_runs, drain_dirty_bits};

    /// (Port of the tests of fxceless/victory-fork #814.)
    #[test]
    fn the_mask_gives_its_pages_and_is_left_empty() {
        let mut words = vec![0u32; 8];
        words[0] = 1 | 1 << 31;
        words[5] = 1 << 7;
        let mut pages = Vec::new();
        // SAFETY: eight dwords of our own.
        unsafe { drain_dirty_bits(words.as_mut_ptr(), words.len(), &mut pages) };
        assert_eq!(pages, [0, 31, 5 * 32 + 7]);
        assert!(words.iter().all(|&w| w == 0));
    }

    #[test]
    fn written_pages_join_into_runs_inside_their_copies() {
        // A copy that begins and ends inside pages, and a window.
        let spans = [(0x1000_0000 + 0x4000, 3 * PAGE), (0x2000_0000, 4 << 20)];
        let (a, b) = ((spans[0].0 / PAGE) as u32, (spans[1].0 / PAGE) as u32);
        let runs = coalesce_dirty_runs([(a, 0), (a + 1, 0), (a + 3, 0), (b, 1), (b + 2, 1), (b + 3, 1), (999, 99)], &spans);
        assert_eq!(
            runs,
            [
                (0, spans[0].0, 2 * PAGE - 0x4000),
                (0, u64::from(a + 3) * PAGE, 0x4000),
                (1, u64::from(b) * PAGE, PAGE),
                (1, u64::from(b + 2) * PAGE, 2 * PAGE),
            ]
        );
    }
}
