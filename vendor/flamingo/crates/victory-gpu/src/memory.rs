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
//! there are at most [`copies`] of them (4 MiB windows mostly: 4 GiB at
//! most). Every dispatch of such a shader brings all numbered copies up to
//! date and, if it can store, declares all of them written (the bytes are
//! compared on the CPU afterwards, as for any unordered access view): a
//! dirty-page mask written by the shader would bound both.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use victory_shader::emit::{MEMORY_FAULTS, MEMORY_PAGES};
use windows::Win32::Graphics::Direct3D12::*;

use crate::descriptor::host::HostHeap;
use crate::host::{HostList, host};
use crate::shadow::Shadow;

/// Copies that can be numbered (the run of heap slots is one longer). A
/// listed frame of the opening cutscene names about 50 windows.
/// Flamingo: at most this many numbered copies (`VICTORY_MEMORY_COPIES`,
/// 1023 by default, at most 16383). With the structure builder on, 1023 ran
/// out within two minutes (run 122) and 4095 did not (run 123); without it,
/// 4095 took the frame from 95 to 105-110 ms and broke the picture in three
/// runs of three (126-128: every copy is brought up to date for each list).
fn copies() -> u32 {
    static N: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *N.get_or_init(|| {
        std::env::var("VICTORY_MEMORY_COPIES")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .map_or(1023, |n| n.clamp(1, 16383))
    })
}
/// Frames before a retired copy's place is given to another copy.
const REUSE_FRAMES: u64 = 4;
/// A window of memory outside buffers (`shadow.rs`).
const CELL: u64 = 4 << 20;
/// A directory page.
const PAGE: u64 = 1 << 16;

struct Memory {
    /// Buffer, its mapping, its GPU address.
    directory: (ID3D12Resource, *mut u32, u64),
    faults: (ID3D12Resource, *mut u32, u64),
    /// Copy `n` is `copies[n - 1]`.
    copies: Vec<Arc<Shadow>>,
    /// Flamingo: per copy, whether it is still the copy of its memory
    /// ([`Memory::forget_stale`]); a copy that is not keeps its place, and
    /// its view, until its number is given out again.
    live: Vec<bool>,
    /// Flamingo: per copy's place, how many copies it has had (a heap's
    /// view of the place is made again when it changed).
    generation: Vec<u32>,
    /// Flamingo: places whose copy stopped being its memory's, and the
    /// frame it did: given out again [`REUSE_FRAMES`] frames later, when no
    /// list in the air uses the old copy's view.
    free: std::collections::VecDeque<(usize, u64)>,
    /// Flamingo: places given a copy since [`ready`] last looked.
    assigned: Vec<usize>,
    /// Flamingo: the addresses the dispatch being made ready names.
    named: Vec<u64>,
    /// Per host view heap (by address): the first slot of its run, and per
    /// copy's place the generation of the view there (0: none yet).
    heaps: HashMap<usize, (u32, Vec<u32>)>,
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
            faults: make(u64::from(MEMORY_FAULTS), "faults")?,
            copies: Vec::new(),
            live: Vec::new(),
            generation: Vec::new(),
            free: Default::default(),
            assigned: Vec::new(),
            named: Vec::new(),
            heaps: HashMap::new(),
            frame: u64::MAX,
            refused: Default::default(),
        })
    }

    /// Gives the page of `address` its copy, if it has none and the memory
    /// is the title's.
    fn map(&mut self, address: u64) {
        let page = address / PAGE;
        // The last entry stays empty: addresses past the directory read it.
        if page >= u64::from(MEMORY_PAGES) - 1 {
            return;
        }
        // SAFETY: inside the directory's mapping.
        let entry = unsafe { self.directory.1.add(page as usize * 2) };
        // SAFETY: as above.
        if unsafe { entry.read() } != 0 {
            return;
        }
        let start = page * PAGE;
        // A shader's pointer, or a constant taken for an address, may be
        // anything: only memory the title has.
        if self.refused.contains(&page) {
            return;
        }
        if !crate::texture::committed(start as *const u8, PAGE as usize) {
            self.refused.insert(page);
            return;
        }
        // The copy a root view at that address would get; a buffer that
        // ends inside the page is passed over for what holds all of it.
        let whole = |s: &Arc<Shadow>| s.offset_in(start).is_some_and(|_| s.left(start) >= PAGE);
        let Some(shadow) = crate::shadow::find(start, 1)
            .filter(whole)
            .or_else(|| crate::shadow::find(start, PAGE).filter(whole))
        else {
            return;
        };
        // SAFETY: a live buffer.
        let flags = unsafe { shadow.buffer.GetDesc() }.Flags;
        if flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0 {
            return;
        }
        let reusable = self
            .free
            .front()
            .filter(|&&(_, at)| self.frame >= at + REUSE_FRAMES)
            .map(|&(n, _)| n);
        let n = match self.copies.iter().zip(&self.live).position(|(c, &live)| live && Arc::ptr_eq(c, &shadow)) {
            Some(n) => n,
            None if reusable.is_some() || self.copies.len() < copies() as usize => {
                let n = match reusable {
                    Some(n) => {
                        self.free.pop_front();
                        self.copies[n] = shadow.clone();
                        n
                    }
                    None => {
                        self.copies.push(shadow.clone());
                        self.live.push(false);
                        self.generation.push(0);
                        self.copies.len() - 1
                    }
                };
                self.live[n] = true;
                self.generation[n] += 1;
                self.assigned.push(n);
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
                    tracing::warn!(copies = copies(), "title memory by address: no number left for another copy; its pages read zeros")
                });
                return;
            }
        };
        let offset = shadow.offset_in(start).unwrap_or(0);
        // The offset first: a shader that sees the number sees it too.
        // SAFETY: inside the directory's mapping.
        unsafe {
            entry.add(1).write_volatile(offset as u32);
            entry.write_volatile(n as u32 + 1);
        }
    }

    /// Flamingo: copies that are no longer the copy of their memory (the
    /// title made another buffer there, or the memory was taken out of the
    /// tables) are retired: their directory entries are emptied, so the next
    /// touch faults and maps the copy that holds the memory now, they are
    /// neither brought up to date nor written back any more, and their place
    /// is given out again later. They stayed numbered for the rest of the
    /// run: run 116 ran out of numbers (1023), and what shaders stored
    /// through such a copy was written back over the memory's new contents.
    fn forget_stale(&mut self) {
        for n in 0..self.copies.len() {
            if !self.live[n] {
                continue;
            }
            let (first, len) = self.copies[n].span();
            if crate::shadow::find(first, 1).is_some_and(|c| Arc::ptr_eq(&c, &self.copies[n])) {
                continue;
            }
            self.live[n] = false;
            self.free.push_back((n, self.frame));
            let last = ((first + len.max(1) - 1) / PAGE).min(u64::from(MEMORY_PAGES) - 2);
            for page in first / PAGE..=last {
                // SAFETY: inside the directory's mapping.
                let entry = unsafe { self.directory.1.add(page as usize * 2) };
                // SAFETY: as above; a shader that reads the number 0 reads
                // no offset.
                unsafe {
                    if entry.read_volatile() == n as u32 + 1 {
                        entry.write_volatile(0);
                    }
                }
            }
        }
    }

    /// The pages shaders found empty since the last frame.
    fn take_faults(&mut self) {
        let mut pages = Vec::new();
        for i in 1..MEMORY_FAULTS as usize {
            // SAFETY: inside the faults' mapping; the GPU may be writing,
            // a number missed now is written again next frame.
            let at = unsafe { self.faults.1.add(i) };
            // SAFETY: as above.
            let page = unsafe { at.read_volatile() };
            if page != 0 {
                pages.push(page);
                // SAFETY: as above.
                unsafe { at.write_volatile(0) };
            }
        }
        if !pages.is_empty() {
            // Each is an access that read 0 or was dropped.
            static SAID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
            if SAID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 50 {
                tracing::warn!(
                    pages = pages.len(),
                    first = format_args!("{:#x}", u64::from(pages[0]) * PAGE),
                    "title memory by address: shaders touched pages that had no copy (those accesses read 0 or were dropped); the copies are made now"
                );
            }
        }
        for page in pages {
            self.map(u64::from(page) * PAGE);
        }
    }

    /// What a dispatch names: the copy around `address` and the 4 MiB
    /// after it.
    fn name(&mut self, address: u64) {
        self.map(address);
        self.map((address & !(CELL - 1)) + CELL);
        self.named.push(address);
        self.named.push((address & !(CELL - 1)) + CELL);
    }

    /// Flamingo: the place of the copy the directory gives `address`, if
    /// it has one.
    fn place_of(&self, address: u64) -> Option<usize> {
        let page = address / PAGE;
        if page >= u64::from(MEMORY_PAGES) - 1 {
            return None;
        }
        // SAFETY: inside the directory's mapping.
        let n = unsafe { self.directory.1.add(page as usize * 2).read_volatile() };
        (n != 0).then(|| n as usize - 1)
    }
}

/// Flamingo: whether a dispatch that can store by address declares every
/// numbered copy written (the default), or only the copies of the memory it
/// names, its root views and the addresses in its root constants
/// (`VICTORY_BVH_WRITE_ALL=0`). Every copy, compared on the CPU after each
/// list that stores, is slow: 0.9 fps with 4095 copies (run 124, no crash
/// in 400 s). The named copies alone made 3.0 fps, but the title then read
/// a wild pointer 98 s into the builder (run 125): what it reads back must
/// be all of what the stages wrote, or none of it (`VICTORY_BVH_QUARANTINE`).
fn write_all() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_BVH_WRITE_ALL").is_none_or(|v| v != "0"))
}

/// Before a draw whose shader reads title memory by address: what its
/// root arguments name gets its copies (`addresses`: its root views;
/// `constants`: its root constants, see the module), every numbered copy
/// has its view in `heap`'s run and is brought up to date when `list` is
/// submitted (and, with `writes`, copied back after). None if the host
/// cannot hold it; the draw is then dropped.
pub(crate) fn ready(
    list: &HostList,
    heap: &HostHeap,
    addresses: &[u64],
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
        m.forget_stale();
        m.take_faults();
    }
    m.named.clear();
    for &address in addresses {
        m.name(address);
    }
    // A title address in two constants (low dword, then bits 32 to 39), or
    // in one in 256-byte units. A constant that only looks like one names
    // memory nothing reads; one that is not the title's memory is refused.
    for (i, &c) in constants.iter().enumerate() {
        if let Some(&high) = constants.get(i + 1)
            && (1..=0xFF).contains(&high)
            && c >= 0x1_0000
        {
            m.name(u64::from(c) | u64::from(high) << 32);
        }
        if c >= 0x0100_0000 {
            m.name(u64::from(c) << 8);
        }
    }
    let key = std::ptr::from_ref(heap) as usize;
    let (base, mut shown) = match m.heaps.remove(&key) {
        Some(run) => run,
        None => {
            // Held for the heap's life: not given back with a list's slots.
            let base = heap.dynamic(copies() + 1)?;
            heap.null_raw_uav_at(base);
            (base, Vec::new())
        }
    };
    shown.resize(m.copies.len(), 0);
    for (n, shadow) in m.copies.iter().enumerate() {
        if shown[n] != m.generation[n] {
            let (_, len) = shadow.span();
            heap.raw_uav_at(base + 1 + n as u32, &shadow.buffer, 0, len);
            shown[n] = m.generation[n];
        }
    }
    m.heaps.insert(key, (base, shown));
    let assigned = std::mem::take(&mut m.assigned);
    let named: Vec<usize> = if writes && !write_all() {
        let mut places: Vec<usize> = m.named.iter().filter_map(|&a| m.place_of(a)).collect();
        places.sort_unstable();
        places.dedup();
        places
    } else {
        Vec::new()
    };
    // Once per list, and for copies numbered since: the builder alone
    // dispatches hundreds of times a frame.
    // shortcut: a copy another list numbers meanwhile is brought up to date
    // by that list's submission, which may run after this one's.
    const READ: usize = usize::MAX - 0x6d;
    const WRITE: usize = usize::MAX - 0x6e;
    const NAMED_WRITE: usize = usize::MAX - 0x10_0000;
    let first_read = list.first_use(READ);
    let first_write = writes && list.first_use(WRITE);
    for (n, shadow) in m.copies.iter().enumerate() {
        // A retired copy is not its memory's any more.
        if !m.live[n] {
            continue;
        }
        let (start, len) = shadow.span();
        let new = assigned.contains(&n);
        if first_read || new {
            shadow.refresh(list, start, len);
        }
        if writes && write_all() && (first_write || new) {
            shadow.gpu_writes(list, start, len);
        }
        // Once per list for each copy named (keys far below the other
        // `first_use` keys near `usize::MAX`).
        if writes && !write_all() && named.binary_search(&n).is_ok() && list.first_use(NAMED_WRITE - n) {
            shadow.gpu_writes(list, start, len);
        }
    }
    if first_read {
        list.keep(&m.directory.0);
        list.keep(&m.faults.0);
    }
    Some(Ready {
        directory: m.directory.2,
        faults: m.faults.2,
        base,
    })
}

/// Evicts cached descriptor-run state for `heap` when its owning `Heap` is
/// dropped so a later `Heap` allocated at the same address cannot reuse stale
/// `(base, made)` slot offsets.
pub(crate) fn forget_heap(heap: &HostHeap) {
    let key = std::ptr::from_ref(heap) as usize;
    if let Some(m) = MEMORY.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        m.heaps.remove(&key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{HeapDesc, create_heap, heap};
    use crate::object::{S_OK, This, guid};

    #[test]
    fn dropping_a_view_heap_evicts_its_memory_heaps_entry() {
        let Some(host) = crate::host::host() else {
            return;
        };
        let iid = guid(
            0x8efb471d,
            0x616c,
            0x4f49,
            [0x90, 0xf7, 0x12, 0x7b, 0xb7, 0x63, 0xfa, 0x51],
        );
        let desc = HeapDesc {
            kind: 0,
            count: 64,
            flags: 1,
            nodes: 1,
        };
        let mut h: This = std::ptr::null();
        assert_eq!(create_heap(std::ptr::null(), &desc, &iid, &mut h), S_OK);
        let list = host.new_list().unwrap();
        let host_heap = heap(h).host().expect("host view heap");
        let key = std::ptr::from_ref(host_heap) as usize;

        assert!(ready(&list, host_heap, &[], &[], false).is_some());
        {
            let guard = MEMORY.lock().unwrap();
            assert!(guard.as_ref().unwrap().heaps.contains_key(&key));
        }

        // Release the title heap (COM vtable slot 2): `Heap::drop` must evict `key`.
        // SAFETY: `h` is a live COM object from `create_heap`.
        let release: extern "system" fn(This) -> u32 =
            unsafe { std::mem::transmute(*(*(h as *const *const *const ())).add(2)) };
        assert_eq!(release(h), 0);

        let guard = MEMORY.lock().unwrap();
        assert!(
            !guard.as_ref().unwrap().heaps.contains_key(&key),
            "MEMORY.heaps retained dropped HostHeap key {key:#x}"
        );
    }
}
