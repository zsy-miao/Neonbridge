//! The title's crowded thread groups get more processors to run on.
//!
//! The title pins its threads as on the console: sixteen logical
//! processors of eight like cores. Measured on a 14700KF (eight fast cores
//! as logical processors 0-15, twelve efficiency cores as 16-27): eleven
//! of its threads, the ones that record lists and run its jobs, share
//! logical processors 8-12, which are two and a half fast cores; a
//! two-second sample found those five 70 to 107 % busy while fast core 7
//! and the efficiency cores carried no pinned thread at all. The profile
//! of run 236 shows what that costs: code of the recording threads that
//! cannot wait for anything (a load of an atomic, a step in a tree) was
//! on a processor in only 58 to 72 of 100 looks, where the same kind of
//! code on the layer's unpinned submit thread was in 90 to 97. The frame
//! waits about 50 ms for those threads before the submit thread's 80.
//!
//! With `VICTORY_SPREAD=1` a group with more threads than processors may
//! also run on the processors no pinned thread of the title uses: the
//! slow cores by default (`VICTORY_SPREAD_SPARE=<hex mask>`: those
//! instead). The title cannot tell which processor a thread is on (it
//! imports neither `GetCurrentProcessorNumber` nor uses `rdpid`), and a
//! wider set is a place to run, not an order: the scheduler still prefers
//! the fast cores. `VICTORY_SPREAD=alt`: twenty seconds with, twenty
//! without, in turn, each change said in the log, to measure it in one
//! scene. Off by default.

use std::collections::HashMap;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
use windows_sys::Win32::System::SystemInformation::GROUP_AFFINITY;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, GetProcessAffinityMask, GetThreadGroupAffinity, OpenThread, SetThreadGroupAffinity, THREAD_QUERY_INFORMATION,
    THREAD_SET_INFORMATION,
};

/// 0 off, 1 on, 2 in turn.
fn mode() -> u8 {
    static MODE: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_SPREAD").as_deref() {
        Ok("1") => 1,
        Ok("alt") => 2,
        _ => 0,
    })
}

/// A pinned thread of the title: the mask the title gave it and the one it
/// has from us.
struct Pinned {
    original: usize,
    wide: usize,
}

/// The mask for a group of `threads` threads the title pinned to `mask`:
/// with `spare` added if they are more than its processors.
fn widened(mask: usize, threads: usize, spare: usize) -> usize {
    if threads > mask.count_ones() as usize { mask | spare } else { mask }
}

/// Whether `mask` is a pinning of the title's: some of the console's
/// sixteen logical processors, not all the processors there are.
fn console_pinning(mask: usize, all: usize) -> bool {
    mask != 0 && mask != all && mask & !0xFFFF == 0
}

/// The ids of this process's threads.
fn threads() -> Vec<u32> {
    let mut ids = Vec::new();
    // SAFETY: a snapshot handle, closed below; the entry says its own size.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
        if snapshot == INVALID_HANDLE_VALUE {
            return ids;
        }
        let process = GetCurrentProcessId();
        let mut entry: THREADENTRY32 = std::mem::zeroed();
        entry.dwSize = size_of::<THREADENTRY32>() as u32;
        let mut more = Thread32First(snapshot, &mut entry) != 0;
        while more {
            if entry.th32OwnerProcessID == process {
                ids.push(entry.th32ThreadID);
            }
            more = Thread32Next(snapshot, &mut entry) != 0;
        }
        CloseHandle(snapshot);
    }
    ids
}

/// A thread's mask in processor group 0, if it can be asked.
fn mask_of(thread: HANDLE) -> Option<usize> {
    // SAFETY: plain data filled in by the query.
    let mut affinity: GROUP_AFFINITY = unsafe { std::mem::zeroed() };
    // SAFETY: an open thread handle and a valid out pointer.
    (unsafe { GetThreadGroupAffinity(thread, &mut affinity) } != 0 && affinity.Group == 0).then_some(affinity.Mask)
}

/// One look at all threads: the title's pinned ones are noted, and each
/// gets its wide mask (`wide`) or the title's own back.
fn pass(pinned: &mut HashMap<u32, Pinned>, all: usize, spare: usize, wide: bool, said: &mut String) {
    let ids = threads();
    pinned.retain(|id, _| ids.contains(id));
    let mut open: Vec<(u32, HANDLE, usize)> = Vec::new();
    for id in ids {
        // SAFETY: a thread id of this process; the handle is closed below.
        let handle = unsafe { OpenThread(THREAD_QUERY_INFORMATION | THREAD_SET_INFORMATION, 0, id) };
        if handle.is_null() {
            continue;
        }
        match mask_of(handle) {
            Some(mask) => {
                let ours = pinned.get(&id).is_some_and(|p| mask == p.wide || mask == p.original);
                if !ours {
                    if console_pinning(mask, all) {
                        // New, or the title pinned it again.
                        pinned.insert(id, Pinned { original: mask, wide: mask });
                    } else {
                        // Not pinned (an id used again by another thread).
                        pinned.remove(&id);
                    }
                }
                open.push((id, handle, mask));
            }
            // SAFETY: the handle opened above.
            None => unsafe {
                CloseHandle(handle);
            },
        }
    }
    let mut groups: HashMap<usize, usize> = HashMap::new();
    for p in pinned.values() {
        *groups.entry(p.original).or_default() += 1;
    }
    for p in pinned.values_mut() {
        p.wide = widened(p.original, groups[&p.original], spare);
    }
    for (id, handle, mask) in open {
        if let Some(p) = pinned.get(&id) {
            let want = if wide { p.wide } else { p.original };
            if mask != want {
                let affinity = GROUP_AFFINITY { Mask: want, Group: 0, Reserved: [0; 3] };
                // SAFETY: an open thread handle and a mask of this system's processors.
                if unsafe { SetThreadGroupAffinity(handle, &affinity, std::ptr::null_mut()) } == 0 {
                    tracing::debug!(id, want, error = %std::io::Error::last_os_error(), "a thread's processors could not be set");
                }
            }
        }
        // SAFETY: the handle opened above.
        unsafe { CloseHandle(handle) };
    }
    let mut table: Vec<(usize, usize)> = groups.into_iter().collect();
    table.sort();
    let text = table
        .iter()
        .map(|&(mask, threads)| {
            let to = widened(mask, threads, spare);
            if to == mask {
                format!("{threads} on {mask:#x} ({} processors)", mask.count_ones())
            } else {
                format!("{threads} on {mask:#x} ({} processors) -> {to:#x} ({})", mask.count_ones(), to.count_ones())
            }
        })
        .collect::<Vec<_>>()
        .join("; ");
    if text != *said {
        tracing::info!("spread: the title's pinned threads by its mask: {text}");
        *said = text;
    }
}

/// Starts the thread that widens the title's crowded groups, if asked for.
pub(crate) fn start() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    if mode() == 0 {
        return;
    }
    ONCE.call_once(|| {
        let made = std::thread::Builder::new().name("victory-spread".into()).spawn(|| {
            let (mut all, mut system) = (0usize, 0usize);
            // SAFETY: our own process and valid out pointers.
            if unsafe { GetProcessAffinityMask(GetCurrentProcess(), &mut all, &mut system) } == 0 || all == 0 {
                tracing::warn!("spread: the process's processors could not be asked; nothing is widened");
                return;
            }
            let spare = match std::env::var("VICTORY_SPREAD_SPARE").ok().and_then(|v| usize::from_str_radix(v.trim_start_matches("0x"), 16).ok()) {
                Some(mask) => mask & all,
                None => crate::framelog::slow_processors() & all,
            };
            if spare == 0 {
                tracing::info!("spread: no spare processors (all cores are alike and VICTORY_SPREAD_SPARE names none); nothing is widened");
                return;
            }
            tracing::info!(
                "spread: the title's thread groups with more threads than processors may also run on processors {spare:#x} (VICTORY_SPREAD=1; alt: in turn)"
            );
            let epoch = std::time::Instant::now();
            let mut pinned = HashMap::new();
            let mut said = String::new();
            let mut last = None;
            loop {
                let wide = mode() == 1 || (epoch.elapsed().as_secs() / 20) % 2 == 1;
                if mode() == 2 && last != Some(wide) {
                    tracing::info!(
                        "the title's crowded thread groups run on more processors from now (VICTORY_SPREAD=alt): {}",
                        if wide { "ON" } else { "off, as usual" }
                    );
                }
                last = Some(wide);
                pass(&mut pinned, all, spare, wide, &mut said);
                // Often enough to catch threads the title starts and to turn with the
                // twenty-second phases; a pass is a snapshot and a query a thread.
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });
        if let Err(error) = made {
            tracing::warn!(%error, "the spread thread could not be started");
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_crowded_groups_are_widened() {
        let spare = 0x0FFF_0000;
        // eleven threads on five processors: crowded
        assert_eq!(widened(0x1F00, 11, spare), 0x0FFF_1F00);
        // eight on eight, three on six, one on one: left alone
        assert_eq!(widened(0xFF, 8, spare), 0xFF);
        assert_eq!(widened(0x3F00, 3, spare), 0x3F00);
        assert_eq!(widened(0x2000, 1, spare), 0x2000);
        assert_eq!(widened(0x3FFF, 11, spare), 0x3FFF);
        assert_eq!(widened(0x3FFF, 15, spare), 0x0FFF_3FFF);
    }

    #[test]
    fn a_pinning_of_the_title_is_within_the_console_s_sixteen() {
        let all = 0x0FFF_FFFF;
        assert!(console_pinning(0x1F00, all));
        assert!(console_pinning(0xFFFF, all));
        assert!(!console_pinning(all, all), "not pinned");
        assert!(!console_pinning(0x0FFF_1F00, all), "already wide: not the title's");
        assert!(!console_pinning(0, all));
        // a PC with sixteen processors: all of them is not a pinning
        assert!(!console_pinning(0xFFFF, 0xFFFF));
    }

    #[test]
    fn this_process_has_threads_and_they_have_masks() {
        let ids = threads();
        assert!(!ids.is_empty());
        // SAFETY: a thread id of this process.
        let handle = unsafe { OpenThread(THREAD_QUERY_INFORMATION | THREAD_SET_INFORMATION, 0, ids[0]) };
        assert!(!handle.is_null());
        assert!(mask_of(handle).is_some_and(|m| m != 0));
        // SAFETY: the handle opened above.
        unsafe { CloseHandle(handle) };
    }
}
