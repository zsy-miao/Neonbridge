//! `VICTORY_MEM2_GPU_ONLY_NOVIEW=1` (off by default): page mappings into the
//! title's GPU-only reservations are not host views (MEMORY.md §3.3 4).
//!
//! The title's texture streamer maps each streamed page twice: once into
//! its CPU pool view (`XMemMapPhysicalPagesScatter`), once at the tiled
//! resource's address (`CopyPageMappingsBatchX`), which lies in a
//! reservation the console's CPU cannot touch (protection 0x801). The
//! runtime made a host view for both, 3-4 system calls and a TLB shootdown
//! a page. With the switch, `victory-hle` xmem records the second as a
//! translation instead: destination page → the page's bytes in a view of
//! the whole section that never moves ([`title_memory_translated`]).
//!
//! Every read of title memory the runtime makes through the kernel or the
//! mapping record translates (`crate::host::read`, `crate::regions::copy`,
//! the texture refresh); a write of the runtime's own goes through the
//! table too (`crate::host::write_mapped`). Anything else that touches such
//! a page, the title's code included (which the console would not allow),
//! faults: the handler here then maps the page as a view after all, as
//! without the switch ([`materialize`]), tells the mapping record, and lets
//! the access run again. A kernel call about to write title memory
//! (`crate::watch::writing`) has its pages made views first the same way.

use std::sync::RwLock;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed};

use victory_core::mem2::{PAGE, PageTable};

/// A destination page shown through the table.
struct Entry {
    /// The page's first byte in the section's own view (unmoving while the
    /// entry lives: the runtime keeps the section until it removes it).
    src: usize,
    /// The section and the page's offset in it, to map it as a view.
    section: usize,
    offset: u64,
    /// 0: no view; 1: being mapped; 2: mapped as a view ([`materialize`]).
    state: AtomicU8,
}

static TABLE: RwLock<PageTable<Entry>> = RwLock::new(PageTable::new());
/// Entries in [`TABLE`], read without its lock to pass by at once.
static PAGES: AtomicUsize = AtomicUsize::new(0);

/// Since the start: destination pages recorded instead of mapped, of those
/// made views later after all, and bytes read and written through the table.
pub(crate) static COUNTS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

thread_local! {
    /// This thread holds [`TABLE`]'s lock (its fault handler must not take
    /// it again).
    static HOLDING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the switch is on (read once).
pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        let on = victory_core::mem2::switch("VICTORY_MEM2_GPU_ONLY_NOVIEW");
        if on {
            tracing::warn!("VICTORY_MEM2_GPU_ONLY_NOVIEW: page mappings into GPU-only reservations are translated, not mapped");
        }
        on
    })
}

/// Whether any page of `[at, at + len)` is shown through the table.
#[inline]
pub(crate) fn any(at: usize, len: usize) -> bool {
    PAGES.load(Relaxed) != 0 && len != 0 && table().any_in(at, len)
}

/// Bytes of `[at, at + len)` shown through the table.
pub(crate) fn bytes_in(at: usize, len: usize) -> usize {
    if PAGES.load(Relaxed) == 0 || len == 0 {
        return 0;
    }
    table().pieces(at, len).iter().filter(|p| p.page.is_some()).map(|p| p.len).sum()
}

/// Pages shown through the table now.
pub(crate) fn live() -> usize {
    PAGES.load(Relaxed)
}

struct Held(std::sync::RwLockReadGuard<'static, PageTable<Entry>>);

impl std::ops::Deref for Held {
    type Target = PageTable<Entry>;
    fn deref(&self) -> &PageTable<Entry> {
        &self.0
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        HOLDING.with(|h| h.set(false));
    }
}

fn table() -> Held {
    let g = TABLE.read().unwrap_or_else(|e| e.into_inner());
    HOLDING.with(|h| h.set(true));
    Held(g)
}

/// The runtime shows the page at `src` (in a view of section `section`, at
/// `offset` in it) at destination page `at`, without a view there. Called
/// under the runtime's page lock, after what `at` showed was taken away.
pub fn title_memory_translated(at: usize, src: usize, section: usize, offset: u64) {
    install();
    let entry = Entry { src, section, offset, state: AtomicU8::new(0) };
    let old = TABLE.write().unwrap_or_else(|e| e.into_inner()).insert(at, entry);
    if old.is_none() {
        PAGES.fetch_add(1, Relaxed);
    }
    COUNTS[0].fetch_add(1, Relaxed);
}

/// The runtime takes away what the table shows in `[at, at + len)`: the
/// entries go, and a page made a view meanwhile is a placeholder again.
/// Returns the pages removed. Called under the runtime's page lock and its
/// hold on the bytes, before it lets go of the sections.
pub fn title_memory_untranslated(at: usize, len: usize) -> usize {
    use windows_sys::Win32::System::Memory::{MEM_PRESERVE_PLACEHOLDER, MEMORY_MAPPED_VIEW_ADDRESS, UnmapViewOfFile2};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    if PAGES.load(Relaxed) == 0 {
        return 0;
    }
    let gone = TABLE.write().unwrap_or_else(|e| e.into_inner()).remove_range(at, len);
    for (page, e) in &gone {
        if e.state.load(Relaxed) == 2 {
            // SAFETY: the view [`materialize`] mapped there.
            unsafe {
                UnmapViewOfFile2(
                    GetCurrentProcess(),
                    MEMORY_MAPPED_VIEW_ADDRESS { Value: *page as *mut _ },
                    MEM_PRESERVE_PLACEHOLDER,
                )
            };
        }
    }
    PAGES.fetch_sub(gone.len(), Relaxed);
    gone.len()
}

/// Copies `[at, at + len)` of title memory to `dst`: the pages the table
/// shows from their sections, the rest with `plain(at, dst, n)`. True if
/// every piece was read (`plain`'s answers).
pub(crate) fn read(at: usize, dst: *mut u8, len: usize, mut plain: impl FnMut(usize, *mut u8, usize) -> bool) -> bool {
    let pieces = table().pieces(at, len);
    let mut ok = true;
    for p in pieces {
        let to = dst.wrapping_add(p.at - at);
        let done = p.page.is_some_and(|page| {
            let t = table();
            let Some((_, e)) = t.get(page) else {
                return false;
            };
            // SAFETY: the entry's page lies in a committed view of its
            // section that stays while the table holds the entry (held);
            // the destination is the caller's `len` bytes.
            unsafe { std::ptr::copy_nonoverlapping((e.src + (p.at - page)) as *const u8, to, p.len) };
            true
        });
        if done {
            COUNTS[2].fetch_add(p.len as u64, Relaxed);
        } else {
            ok &= plain(p.at, to, p.len);
        }
    }
    ok
}

/// Copies `len` bytes from `src` into title memory `[at, at + len)`: the
/// pages the table shows into their sections (the same physical pages, as
/// a write through a view would), the rest with `plain(at, src, n)`.
pub(crate) fn write(at: usize, src: *const u8, len: usize, mut plain: impl FnMut(usize, *const u8, usize)) {
    let pieces = table().pieces(at, len);
    for p in pieces {
        let from = src.wrapping_add(p.at - at);
        let done = p.page.is_some_and(|page| {
            let t = table();
            let Some((_, e)) = t.get(page) else {
                return false;
            };
            // SAFETY: as in [`read`]; the source is the caller's.
            unsafe { std::ptr::copy_nonoverlapping(from, (e.src + (p.at - page)) as *mut u8, p.len) };
            true
        });
        if done {
            COUNTS[3].fetch_add(p.len as u64, Relaxed);
        } else {
            plain(p.at, from, p.len);
        }
    }
}

/// After the mapping record forgot the views of `[at, at + len)`
/// (`crate::title_memory_remapped`): the pages of it the table shows that
/// are views are views still.
pub(crate) fn remapped(at: usize, len: usize) {
    if PAGES.load(Relaxed) == 0 || len == 0 {
        return;
    }
    let views: Vec<usize> = table().iter_in(at, len).filter(|(_, e)| e.state.load(Relaxed) == 2).map(|(p, _)| p).collect();
    for page in views {
        crate::regions::mapped(page, PAGE);
    }
}

/// Makes the pages of `[at, at + len)` the table shows host views, as the
/// runtime maps them without the switch: before something reads or writes
/// them in place or through the kernel.
pub(crate) fn materialize(at: usize, len: usize) {
    if PAGES.load(Relaxed) == 0 || len == 0 || HOLDING.with(|h| h.get()) {
        return;
    }
    let pages: Vec<usize> = table().iter_in(at, len).map(|(p, _)| p).collect();
    for page in pages {
        let made = {
            let t = table();
            t.get(page).is_some_and(|(page, e)| materialize_one(page, e))
        };
        if made {
            // A view now, for the mapping record (its lock is never taken
            // with the table's held).
            crate::regions::mapped(page, PAGE);
        }
    }
}

/// Maps `e`'s page as a view at `page`, once (another thread doing it is
/// waited for). True when the page is a view.
fn materialize_one(page: usize, e: &Entry) -> bool {
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_PRESERVE_PLACEHOLDER, MEM_RELEASE, MEM_REPLACE_PLACEHOLDER, MEMORY_MAPPED_VIEW_ADDRESS,
        MapViewOfFile3, PAGE_READWRITE, UnmapViewOfFile2, VirtualAlloc, VirtualFree,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    loop {
        match e.state.compare_exchange(0, 1, Relaxed, Relaxed) {
            Ok(_) => break,
            Err(2) => return true,
            Err(_) => std::thread::yield_now(),
        }
    }
    // SAFETY: `page` is a page of a placeholder no view shows (the table's
    // entry stands for it): split it out, then map the section's page in
    // its place; both act on placeholders only.
    let ok = unsafe {
        VirtualFree(page as *mut _, PAGE, MEM_RELEASE | MEM_PRESERVE_PLACEHOLDER);
        let view = MapViewOfFile3(
            e.section as _,
            GetCurrentProcess(),
            page as *const _,
            e.offset,
            PAGE,
            MEM_REPLACE_PLACEHOLDER,
            PAGE_READWRITE,
            std::ptr::null_mut(),
            0,
        );
        if view.Value as usize != page {
            false
        } else if VirtualAlloc(page as *const _, PAGE, MEM_COMMIT, PAGE_READWRITE).is_null() {
            UnmapViewOfFile2(GetCurrentProcess(), MEMORY_MAPPED_VIEW_ADDRESS { Value: page as *mut _ }, MEM_PRESERVE_PLACEHOLDER);
            false
        } else {
            true
        }
    };
    e.state.store(if ok { 2 } else { 0 }, Relaxed);
    if ok {
        COUNTS[1].fetch_add(1, Relaxed);
    } else {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            tracing::warn!(
                page = format_args!("{page:#x}"),
                error = %std::io::Error::last_os_error(),
                "VICTORY_MEM2_GPU_ONLY_NOVIEW: a translated page could not be mapped as a view"
            )
        });
    }
    ok
}

/// Registers the handler that maps a translated page when something
/// touches it in place, once, ahead of the crash reporter's.
fn install() {
    use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
    use windows_sys::Win32::System::Diagnostics::Debug::{
        AddVectoredExceptionHandler, EXCEPTION_CONTINUE_EXECUTION, EXCEPTION_CONTINUE_SEARCH, EXCEPTION_POINTERS,
    };
    unsafe extern "system" fn on_fault(info: *mut EXCEPTION_POINTERS) -> i32 {
        // SAFETY: the system passes the exception being dispatched.
        let r = unsafe { &*(*info).ExceptionRecord };
        if r.ExceptionCode != EXCEPTION_ACCESS_VIOLATION as NTSTATUS
            || r.NumberParameters < 2
            || PAGES.load(Relaxed) == 0
            || HOLDING.with(|h| h.get())
        {
            return EXCEPTION_CONTINUE_SEARCH;
        }
        let at = r.ExceptionInformation[1];
        let made = {
            let t = table();
            t.get(at).and_then(|(page, e)| materialize_one(page, e).then_some(page))
        };
        match made {
            Some(page) => {
                // A view now, for the mapping record: copies read it in
                // place, the guards take its writes as they do any view's.
                crate::regions::mapped(page, PAGE);
                EXCEPTION_CONTINUE_EXECUTION
            }
            None => EXCEPTION_CONTINUE_SEARCH,
        }
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    // SAFETY: the handler touches only the table and maps placeholders.
    ONCE.call_once(|| unsafe {
        AddVectoredExceptionHandler(1, Some(on_fault));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Destination pages far from anything mapped: the table is only read
    /// for them, never touched in place.
    const AT: usize = 0x7f30_0000_0000;

    #[test]
    fn reads_and_writes_go_through_the_table_and_the_rest_through_plain() {
        let mut page1 = vec![0u8; PAGE];
        let mut page2 = vec![0u8; PAGE];
        for (i, b) in page1.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        page2.fill(0xab);
        // Pages 1 and 2 of four shown through the table.
        title_memory_translated(AT + PAGE, page1.as_mut_ptr() as usize, 0, 0);
        title_memory_translated(AT + 2 * PAGE, page2.as_mut_ptr() as usize, 0, 0);
        assert!(any(AT, PAGE + 1));
        assert!(!any(AT, PAGE));
        assert_eq!(bytes_in(AT + 100, 4 * PAGE), 2 * PAGE);
        // A read from inside page 0 to inside page 3.
        let (from, len) = (AT + PAGE - 10, 2 * PAGE + 20);
        let mut out = vec![0u8; len];
        let mut plain = Vec::new();
        let ok = read(from, out.as_mut_ptr(), len, |at, d, n| {
            plain.push((at, n));
            // SAFETY: inside `out`.
            unsafe { d.write_bytes(0xee, n) };
            true
        });
        assert!(ok);
        assert_eq!(plain, vec![(from, 10), (AT + 3 * PAGE, 10)]);
        assert_eq!(&out[..10], &[0xee; 10]);
        assert_eq!(&out[10..10 + PAGE], &page1[..]);
        assert!(out[10 + PAGE..10 + 2 * PAGE].iter().all(|&b| b == 0xab));
        assert_eq!(&out[10 + 2 * PAGE..], &[0xee; 10]);
        // A plain piece that fails fails the read.
        assert!(!read(AT, out.as_mut_ptr(), PAGE + 1, |_, _, _| false));
        // The kernel-path read of the runtime translates too.
        let mut two = [0u8; 2];
        assert!(crate::host::read_into((AT + PAGE + 5) as *const u8, &mut two));
        assert_eq!(two, [5, 6]);
        // Writes land in the pages, the rest goes to plain.
        let data = vec![0x11u8; 30];
        let mut wrote = Vec::new();
        write(AT + 2 * PAGE - 15, data.as_ptr(), 30, |at, _, n| wrote.push((at, n)));
        assert_eq!(wrote, Vec::<(usize, usize)>::new());
        assert!(page1[PAGE - 15..].iter().all(|&b| b == 0x11));
        assert!(page2[..15].iter().all(|&b| b == 0x11) && page2[15] == 0xab);
        write(AT + 3 * PAGE - 1, data.as_ptr(), 2, |at, _, n| wrote.push((at, n)));
        assert_eq!(wrote, vec![(AT + 3 * PAGE, 1)]);
        assert_eq!(page2[PAGE - 1], 0x11);
        // Untranslated: gone, and the read is all plain.
        assert_eq!(title_memory_untranslated(AT, 4 * PAGE), 2);
        assert!(!any(AT, 4 * PAGE));
        let mut plain = 0;
        assert!(read(AT, out.as_mut_ptr(), PAGE, |_, _, n| {
            plain += n;
            true
        }));
        assert_eq!(plain, PAGE);
        assert_eq!(title_memory_untranslated(AT, 4 * PAGE), 0);
    }
}
