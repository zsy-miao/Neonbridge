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
//! load. On by default (user order: fixes ship default-on): both branches
//! become NOPs; TUXBOX_NO_WATCHDOG=0 keeps the watchdog, so an A/B run shows
//! whether a post-load exit or stall is it. The cause it masks is the slow
//! builds themselves.
//!
//! Format 0x0a pitch (leonida-ce e8ca18c79, GPL-3.0, credit to its authors):
//! the title's pitch function at +0x1555c10 handles format & 0xfff values 1-4,
//! 9 and above 0x14, so a CPU staging image of format 0x0a (R32G32B32A32) gets
//! pitch 0. Its generated colour ramp then fills 4 KiB into a 0-byte buffer
//! and overruns the heap's next node: the 117-143 s stop. A stub answers
//! width * 16 for 0x0a and runs the replaced code otherwise.
//! TUXBOX_FMT10_PITCH=0 leaves it alone. The open cause upstream is why the
//! title takes the staging fallback at all (its texture lock at +0x1336320
//! fails).
use core::ffi::c_void;
use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAlloc, VirtualProtect};
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
    if std::env::var_os("TUXBOX_FMT10_PITCH").is_none_or(|v| v != "0") {
        unsafe { fmt10_pitch() };
    }
    if std::env::var_os("TUXBOX_NO_WATCHDOG").is_none_or(|v| v != "0") {
        for (rva, expect) in [(0x221_5c70usize, [0x74u8, 0x04]), (0x221_5bdc, [0x73, 0x2e])] {
            unsafe { patch("stall watchdog off (ScarlettPort bb3a27a, TUXBOX_NO_WATCHDOG=0 off)", rva, &expect, &[0x90, 0x90]) };
        }
        // The fault itself (VIctory hooks.rs WATCHDOG, via ScarlettPort,
        // GPL-3.0, credit to its authors): test145 died on the handler's
        // `mov dword [0], 0x4d8bd421` at +0x23cf683, reached by a path the two
        // branches above do not cover. The call site at +0x2215c27 jumps over
        // the handler call and its fallback `mov [0], 0; int3` to the loop's
        // normal path, and the handler's store becomes an 11-byte NOP.
        unsafe {
            patch("watchdog call skipped (VIctory hooks.rs, TUXBOX_NO_WATCHDOG=0 off)", 0x221_5c27,
                &[0x48, 0x8b, 0x05, 0x1a, 0x07, 0xfa, 0x04], &[0xeb, 0x18, 0x90, 0x90, 0x90, 0x90, 0x90]);
            patch("watchdog fault NOP (VIctory hooks.rs, TUXBOX_NO_WATCHDOG=0 off)", 0x23c_f683,
                &[0xc7, 0x04, 0x25, 0, 0, 0, 0, 0x21, 0xd4, 0x8b, 0x4d], &[0x66, 0x66, 0x2e, 0x0f, 0x1f, 0x84, 0, 0, 0, 0, 0]);
        }
    }
}

/// leonida-ce e8ca18c79 `xs_patch_fmt10_pitch`, base-relative.
unsafe fn fmt10_pitch() {
    const AT: usize = 0x155_5c29;
    const EXPECT: [u8; 10] = [0x83, 0xc0, 0xfd, 0x83, 0xf8, 0x02, 0x72, 0x55, 0x31, 0xd2];
    let base = unsafe { GetModuleHandleW(core::ptr::null()) } as usize;
    if base == 0 {
        return;
    }
    let now = unsafe { core::slice::from_raw_parts((base + AT) as *const u8, EXPECT.len()) };
    if now != EXPECT {
        crate::loader::log(&format!("title patch format 0x0a pitch: bytes at game+{AT:#x} differ ({}), not applied", hex(now)));
        return;
    }
    // A stub page within rel32 reach below the image.
    let mut stub = core::ptr::null_mut::<u8>();
    let mut a = base.saturating_sub(0x100_0000);
    while stub.is_null() && a > base.saturating_sub(0x7f00_0000) && a >= 0x1_0000 {
        stub = unsafe { VirtualAlloc(a as *const c_void, 4096, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE) }.cast();
        a -= 0x1_0000;
    }
    if stub.is_null() {
        crate::loader::log("title patch format 0x0a pitch: no stub page near the image, not applied");
        return;
    }
    let mut code = vec![
        0x83, 0xf8, 0x0a, // cmp eax, 10 (eax = format & 0xfff)
        0x75, 0x06, // jne +6
        0xc1, 0xe2, 0x04, // shl edx, 4 (width * 16)
        0x89, 0xd0, // mov eax, edx
        0xc3, // ret
        0x83, 0xc0, 0xfd, // add eax, -3 (replaced)
        0x83, 0xf8, 0x02, // cmp eax, 2
        0x73, 0x0e, // jae +14
    ];
    for target in [base + 0x155_5c86, base + 0x155_5c31] {
        code.extend_from_slice(&[0xff, 0x25, 0, 0, 0, 0]);
        code.extend_from_slice(&(target as u64).to_le_bytes());
    }
    unsafe {
        core::ptr::copy_nonoverlapping(code.as_ptr(), stub, code.len());
        FlushInstructionCache(GetCurrentProcess(), stub as *const c_void, code.len());
    }
    let rel = (stub as i64 - (base + AT + 5) as i64) as i32;
    let mut with = [0xe9, 0, 0, 0, 0, 0x90, 0x90, 0x90];
    with[1..5].copy_from_slice(&rel.to_le_bytes());
    unsafe { patch("format 0x0a pitch (leonida-ce e8ca18c79, TUXBOX_FMT10_PITCH=0 off)", AT, &EXPECT[..8], &with) };
}
