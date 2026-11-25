//! First-call trace of command-list methods 59..149, the slots whose layout
//! differs between console list generations (older vs newer, ztrx list.rs
//! +2 from 59 on). Each slot of the three guest list tables goes through a
//! naked thunk that saves the argument registers, logs once per (table, slot)
//! the first call's integer arguments and return address, and jumps to the
//! real method with every register as the title left it, so any signature
//! (float arguments in xmm, stack arguments) passes through unchanged.
//! TUXBOX_SLOT_TRACE=0 stops the logging (the thunk stays, a load and a jump).
use crate::proxy::{Slot, Slots};
use std::sync::atomic::{AtomicU8, Ordering};

pub(super) const END: usize = 149;
const NAMES: [&str; 3] = ["older", "device11", "extended"];

static SEEN: [AtomicU8; 3 * END] = [const { AtomicU8::new(0) }; 3 * END];

fn raw(table: usize) -> &'static Slots<END> {
    match table {
        0 => &super::LIST_RAW,
        1 => &super::DEVICE11_RAW,
        _ => &super::EXTENDED_RAW,
    }
}

/// Called by the thunk: logs the first call, returns the real method.
extern "system" fn first_call(table: usize, slot: usize, list: usize, a: usize, b: usize, ret: usize) -> usize {
    let target = raw(table)[slot] as usize;
    let seen = &SEEN[table * END + slot];
    if seen.load(Ordering::Relaxed) == 0 && seen.swap(1, Ordering::Relaxed) == 0 && crate::env_toggle!("TUXBOX_SLOT_TRACE") != Some("0") {
        // The title is the process image; a return inside it is shown as game+rva.
        let base = unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(None) }.map_or(0, |m| m.0 as usize);
        let image = ret.checked_sub(base).filter(|&r| base != 0 && r < 0x2000_0000)
            .map_or(String::new(), |r| format!(" (game+{r:#x})"));
        crate::diagnostics::log_line(format_args!(
            "list slot {slot} ({:#x}) first call on the {} table: list {list:#x} args {a:#x} {b:#x}, from {ret:#x}{image}",
            slot * 8, NAMES[table]));
    }
    target
}

#[unsafe(naked)]
unsafe extern "system" fn thunk<const T: usize, const N: usize>() {
    core::arch::naked_asm!(
        "push rcx", "push rdx", "push r8", "push r9",
        "sub rsp, 0x78",
        "movdqu [rsp+0x30], xmm0", "movdqu [rsp+0x40], xmm1",
        "movdqu [rsp+0x50], xmm2", "movdqu [rsp+0x60], xmm3",
        "mov r8, [rsp+0x90]",
        "mov r9, [rsp+0x88]",
        "mov rax, [rsp+0x80]", "mov [rsp+0x20], rax",
        "mov rax, [rsp+0x98]", "mov [rsp+0x28], rax",
        "mov ecx, {t}", "mov edx, {n}",
        "call {f}",
        "movdqu xmm0, [rsp+0x30]", "movdqu xmm1, [rsp+0x40]",
        "movdqu xmm2, [rsp+0x50]", "movdqu xmm3, [rsp+0x60]",
        "add rsp, 0x78",
        "pop r9", "pop r8", "pop rdx", "pop rcx",
        "jmp rax",
        t = const T, n = const N, f = sym first_call,
    )
}

macro_rules! traced {
    ($t:literal, $raw:expr, $($n:literal),*) => {{
        let mut slots = $raw;
        $( slots[$n] = thunk::<$t, $n> as Slot; )*
        slots
    }};
}

pub(super) const fn older(raw: Slots<END>) -> Slots<END> { traced!(0, raw, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148) }
pub(super) const fn device11(raw: Slots<END>) -> Slots<END> { traced!(1, raw, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148) }
pub(super) const fn extended(raw: Slots<END>) -> Slots<END> { traced!(2, raw, 59, 60, 61, 62, 63, 64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79, 80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95, 96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127, 128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148) }
