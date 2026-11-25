//! Fixes to the title's own code, each applied only when its bytes match the
//! build it was found in (GTA VI 1.0.2.1).
//!
//! Audio lock leak (leonida-ce 62ad4fdf, GPL-3.0, ported): the audio function
//! at game+0x5267720 takes its object's lock (the audio engine, +0x3ced0) and,
//! when the object's count at +0x52c is 0, returns from the branch at
//! +0x5267747 without unlocking; every other path ends in the unlock
//! (+0x4d9dab0), as does its sibling +0x5594650 on the same zero count. Here
//! the count is 0 there, so a job worker keeps the lock, the audio thread
//! blocks on it from its first frame, its per-frame command stream is never
//! drained and overflows at about 100 s. The branch is sent to the unlocking
//! tail (+0x5267c7f: rcx = the object, frame popped, jmp unlock).
//! TUXBOX_AUDIO_LOCK_PATCH=0 leaves it alone.
//!
//! Stall watchdog (ScarlettPort bb3a27a / xcl#505, GPL-3.0, credit to their
//! authors): the title ends itself when its heartbeat is unchanged for 30 s
//! (the branch at +0x2215c70, `74 04`) or an in-flight timestamp is too old
//! (+0x2215bdc, `73 2e`). Slow first pipeline builds can trip it after a
//! load. A DIAGNOSTIC, off by default: TUXBOX_NO_WATCHDOG=1 turns both
//! branches into NOPs, so an A/B run shows whether a post-load exit or stall
//! is this watchdog. The cause it masks is the slow builds themselves.
use core::ffi::c_void;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// Rewrites `expect.len()` bytes at `rva` of the exe to `with` when they are `expect`.
unsafe fn patch(name: &str, rva: usize, expect: &[u8], with: &[u8]) {
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    if base == 0 {
        return;
    }
    let at = (base + rva) as *mut u8;
    // SAFETY: the RVA lies in the exe's mapped .text of the matching build;
    // another build's bytes differ and are left alone.
    let now = unsafe { core::slice::from_raw_parts(at, expect.len()) };
    if now != expect {
        crate::loader::log(&format!("title patch {name}: bytes at game+{rva:#x} differ ({}), not applied", hex(now)));
        return;
    }
    let mut old = 0;
    if unsafe { VirtualProtect(at as *const c_void, with.len(), PAGE_EXECUTE_READWRITE, &mut old) } == 0 {
        crate::loader::log(&format!("title patch {name}: VirtualProtect failed, not applied"));
        return;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(with.as_ptr(), at, with.len());
        VirtualProtect(at as *const c_void, with.len(), old, &mut old);
        FlushInstructionCache(GetCurrentProcess(), at as *const c_void, with.len());
    }
    crate::loader::log(&format!("title patch {name}: applied at game+{rva:#x}"));
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Called from loader::install, before any title code runs.
pub(crate) unsafe fn install() {
    if std::env::var_os("TUXBOX_AUDIO_LOCK_PATCH").is_none_or(|v| v != "0") {
        // je rel32 at +0x5267747 (to +0x5267cfb) becomes je to +0x5267c7f.
        const AT: usize = 0x526_7747;
        const TAIL: usize = 0x526_7c7f;
        let rel = (TAIL as i64 - (AT as i64 + 6)) as i32;
        let mut with = [0x0f, 0x84, 0, 0, 0, 0];
        with[2..].copy_from_slice(&rel.to_le_bytes());
        unsafe { patch("audio lock leak (leonida-ce 62ad4fdf, TUXBOX_AUDIO_LOCK_PATCH=0 off)", AT, &[0x0f, 0x84, 0xae, 0x05, 0x00, 0x00], &with) };
    }
    if std::env::var_os("TUXBOX_NO_WATCHDOG").is_some_and(|v| v == "1") {
        for (rva, expect) in [(0x221_5c70usize, [0x74u8, 0x04]), (0x221_5bdc, [0x73, 0x2e])] {
            unsafe { patch("stall watchdog off (ScarlettPort bb3a27a, TUXBOX_NO_WATCHDOG=1 diagnostic)", rva, &expect, &[0x90, 0x90]) };
        }
    }
}
