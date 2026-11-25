//! `VICTORY_COPY_BACK_ONLY_READBACK=1` (testing, off by default): what
//! shaders write is copied back into title memory only for the buffers the
//! title reads on the CPU.
//!
//! The title (build 1.0.2.1) makes every buffer it reads back on the CPU
//! through one function (its read-back ring, title RVA 0x1d40f90, 22
//! callers: lighting, occlusion, particles, wind, water, autofocus, ...;
//! read statically). A buffer created while that function is on the stack
//! is noted here; with the switch on, shader writes to other memory stay on
//! the card.
//!
//! "Stay on the card" must not mean "lost": the runtime itself reads title
//! memory in several places, and run R2a (the switch on, the world drawn,
//! the picture black) put stale title bytes back over shader results there.
//! The ranges are still declared (flights in the air hold them, so readers
//! wait), and what stays in a host copy is "owned" by it
//! (`victory_core::gpuown`, `crate::shadow`'s `owning`/`owned`): its
//! refreshes leave those bytes alone, and the title's buffer copies run on
//! the CPU, fills, textures and other copies over the same memory get them
//! written into title memory first. A page the title writes over owned
//! bytes gives them to the title and is copied back from then on.
//!
//! shortcut: the function is found on the stack by scanning the stack for
//! a return address into it (the title's unwind data is not registered
//! with the system), so a stale return address left in a caller's frame
//! could tag a buffer that is not read back (it is then copied back as
//! before: harmless).

use std::sync::Mutex;
use std::sync::OnceLock;

/// The title's read-back buffer creator (1.0.2.1).
const CREATOR_RVA: u32 = 0x1d40f90;
/// Stack bytes scanned above the creation call.
const SCAN_BYTES: usize = 16 << 10;

/// `VICTORY_COPY_BACK_ASYNC=1` (off by default; needs the relay,
/// `VICTORY_RELAY`): what shaders wrote into the title's read-back rings of
/// three is copied back on a thread of its own after its flight landed
/// (`crate::host`'s async copy-back), not before the fences behind it.
/// Rings of one (owners made with mode bit 0x8, which read in the same
/// frame) are told apart by [`RingSizes`] and copied back as before.
///
/// [`RingSizes`]: victory_core::readring::RingSizes
pub(crate) fn async_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("VICTORY_COPY_BACK_ASYNC").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::warn!("VICTORY_COPY_BACK_ASYNC: read-back rings are copied back beside the landing of flights");
        }
        on
    })
}

/// Read-back buffers by creation group ([`async_on`]), and their sizes.
static RINGS: Mutex<Option<(victory_core::readring::RingSizes, std::collections::HashMap<u64, u64>)>> = Mutex::new(None);

/// Whether a copy-back into `[start, start + len)` may land late: it lies
/// in one read-back buffer made as one of a ring of two or more.
pub(crate) fn lagged(start: u64, len: u64) -> bool {
    let rings = RINGS.lock().unwrap_or_else(|e| e.into_inner());
    let Some((sizes, lens)) = rings.as_ref() else {
        return false;
    };
    // The buffer holding `start` (copies are per title buffer: its start).
    lens.get(&start).is_some_and(|&l| len <= l) && sizes.lagged(start)
}

/// The size of the read-back buffer at `start`, if it is one.
pub(crate) fn ring_len(start: u64) -> Option<u64> {
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|r| r.1.get(&start).copied())
}

/// Whether the switch is on.
pub(crate) fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        let on = std::env::var("VICTORY_COPY_BACK_ONLY_READBACK").is_ok_and(|v| v.trim() == "1");
        if on {
            tracing::warn!(
                "VICTORY_COPY_BACK_ONLY_READBACK: shader writes are copied back only for the title's read-back buffers"
            );
        }
        on
    })
}

/// `[start, end)` of title memory of the read-back buffers alive (let go
/// in [`untag`]: a buffer made later in the same memory need not be one).
static TAGGED: Mutex<crate::memcopy::RangeSet> = Mutex::new(crate::memcopy::RangeSet::new());

/// The creator's code range, once the title image is known: `None` until
/// then, `Some(None)` if it is not this build.
static CREATOR: OnceLock<Option<(usize, usize)>> = OnceLock::new();

/// The `[begin, end)` of the function at `rva` in the image at `base`, from
/// its exception directory.
///
/// # Safety
/// `base` is a mapped PE image.
unsafe fn function_at(base: usize, rva: u32) -> Option<(usize, usize)> {
    // SAFETY: reading the headers of a mapped image.
    unsafe {
        let nt = base + (base as *const u8).add(0x3c).cast::<u32>().read_unaligned() as usize;
        if (nt as *const u32).read_unaligned() != 0x4550 {
            return None;
        }
        // Optional header at +0x18; PE32+ data directories at +0x70; the
        // exception directory is the fourth.
        let dir = nt + 0x18 + 0x70 + 3 * 8;
        let (at, size) = (
            (dir as *const u32).read_unaligned() as usize,
            ((dir + 4) as *const u32).read_unaligned() as usize,
        );
        let entries = std::slice::from_raw_parts((base + at) as *const [u32; 3], size / 12);
        let i = entries.binary_search_by_key(&rva, |e| e[0]).ok()?;
        Some((base + entries[i][0] as usize, base + entries[i][1] as usize))
    }
}

/// The base of the image holding `address` (0 if none).
fn module_of(address: usize) -> usize {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: looks up the module holding the address, no reference.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut module,
        )
    };
    if found == 0 { 0 } else { module as usize }
}

/// The title image holding `address`, if it holds the creator.
fn creator_from(address: usize) -> Option<(usize, usize)> {
    use windows_sys::Win32::System::LibraryLoader::{
        GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
        GetModuleHandleExW,
    };
    let mut module = std::ptr::null_mut();
    // SAFETY: looks up the module holding the address, no reference.
    let found = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            address as *const u16,
            &mut module,
        )
    };
    if found == 0 {
        return None;
    }
    // SAFETY: a module the system knows is a mapped image.
    unsafe { function_at(module as usize, CREATOR_RVA) }
}

/// Called by a resource creation method: whether the title's read-back
/// creator is on the stack, and where on the stack its return address was
/// found (it names the creator's call, for [`RingSizes`]). Learns the
/// creator's place from the first caller in the title executable.
///
/// [`RingSizes`]: victory_core::readring::RingSizes
fn creator_frame() -> Option<usize> {
    use windows_sys::Win32::System::Diagnostics::Debug::RtlCaptureStackBackTrace;
    use windows_sys::Win32::System::Threading::GetCurrentThreadStackLimits;
    // Also with `crate::rebar` placing title buffers (level 2), which must
    // not place these.
    if !enabled() && crate::rebar::level() < 2 && !async_on() {
        return None;
    }
    let creator = match CREATOR.get() {
        Some(c) => *c,
        None => {
            // The first caller outside this runtime's own image: the
            // title's call of the creation method.
            let mut frames = [std::ptr::null_mut(); 8];
            // SAFETY: room for the frames asked for.
            let n = unsafe { RtlCaptureStackBackTrace(1, frames.len() as u32, frames.as_mut_ptr(), std::ptr::null_mut()) };
            let own = module_of(creator_frame as *const () as usize);
            let frame = frames[..n as usize].iter().map(|&f| f as usize).find(|&f| module_of(f) != own)?;
            let Some(c) = creator_from(frame) else {
                // A caller outside the executable (a title DLL): ask again later.
                return None;
            };
            tracing::warn!(begin = format_args!("{:#x}", c.0), end = format_args!("{:#x}", c.1), "read-back creator found");
            *CREATOR.get_or_init(|| Some(c))
        }
    };
    let (begin, end) = creator?;
    let (mut low, mut high) = (0usize, 0usize);
    // SAFETY: plain out pointers.
    unsafe { GetCurrentThreadStackLimits(&mut low, &mut high) };
    let here = &low as *const usize as usize;
    let top = (here + SCAN_BYTES).min(high);
    let mut at = here & !7;
    while at + 8 <= top {
        // SAFETY: inside this thread's committed stack, above this frame.
        let v = unsafe { (at as *const usize).read() };
        if (begin..end).contains(&v) {
            return Some(at);
        }
        at += 8;
    }
    None
}

/// A buffer was made at `[start, start + len)` (any buffer): with
/// [`async_on`], a read-back buffer is noted with its creation group, and
/// any other buffer ends the group its thread was making. Whether it is a
/// read-back buffer.
pub(crate) fn buffer_made(start: u64, len: u64) -> bool {
    let frame = creator_frame();
    if async_on() {
        use windows_sys::Win32::System::Threading::GetCurrentThreadId;
        // SAFETY: a plain query.
        let thread = u64::from(unsafe { GetCurrentThreadId() });
        let mut rings = RINGS.lock().unwrap_or_else(|e| e.into_inner());
        let (sizes, lens) = rings.get_or_insert_with(Default::default);
        match frame {
            Some(at) => {
                sizes.created((thread, at), start, len);
                lens.insert(start, len);
                let depth = sizes.depth(start);
                if depth == 2 || depth == 1 && lens.len().is_power_of_two() {
                    tracing::info!(start = format_args!("{start:#x}"), len, depth, "read-back ring buffers (VICTORY_COPY_BACK_ASYNC)");
                }
            }
            None => sizes.other(thread),
        }
    }
    frame.is_some()
}

/// Notes a read-back buffer at `[start, start + len)`.
pub(crate) fn tag(start: u64, len: u64) {
    let mut t = TAGGED.lock().unwrap_or_else(|e| e.into_inner());
    t.insert(start, start + len);
    let n = t.len();
    if n.is_power_of_two() {
        tracing::warn!(n, start = format_args!("{start:#x}"), len, "read-back buffers tagged");
    }
}

/// Whether a read-back buffer lies in `[start, start + len)`, switch or
/// not (`crate::rebar` keeps such memory where the CPU reads it fast).
pub(crate) fn tagged_in(start: u64, len: u64) -> bool {
    TAGGED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .overlaps(start, start.saturating_add(len))
}

/// Whether shader writes to `[start, start + len)` are copied back: always
/// with the switch off; with it on, only where a read-back buffer lies.
/// Asked at submission, per declared range (`Shadow::writing`).
pub(crate) fn copy_back(start: u64, len: u64) -> bool {
    if !enabled() {
        return true;
    }
    let back = TAGGED
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .overlaps(start, start.saturating_add(len));
    if back {
        crate::memcopy::add(crate::memcopy::M::BackGoes, 1);
    } else {
        crate::memcopy::add(crate::memcopy::M::BackKept, 1);
        crate::memcopy::add(crate::memcopy::M::BackKeptBytes, len);
    }
    back
}

/// A buffer starting at `start` is let go: if it was a read-back buffer,
/// it is one no more.
pub(crate) fn untag(start: u64) {
    if async_on()
        && let Some(len) = ring_len(start)
    {
        // Its copy-back is done before its memory goes.
        crate::host::async_drain_range(start as usize, (start + len) as usize);
        if let Some((sizes, lens)) = RINGS.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
            sizes.forget(start);
            lens.remove(&start);
        }
    }
    if enabled() {
        TAGGED.lock().unwrap_or_else(|e| e.into_inner()).remove_at(start);
    }
}

/// How many read-back buffers are alive.
pub(crate) fn tagged() -> usize {
    TAGGED.lock().unwrap_or_else(|e| e.into_inner()).len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_test_binary_has_its_own_functions_in_its_table() {
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        // SAFETY: our own executable.
        let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
        let f = function_at as usize - base;
        // SAFETY: our own mapped image.
        let (b, e) = unsafe { function_at(base, f as u32) }.expect("own function in .pdata");
        assert_eq!(b, function_at as usize);
        assert!(e > b);
    }
}
