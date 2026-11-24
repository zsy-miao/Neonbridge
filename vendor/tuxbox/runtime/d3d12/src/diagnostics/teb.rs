//! Opt-in loader checks, run once at device creation (the title is loaded and
//! patched by then):
//! - TUXBOX_TEB_CHECK=1: allocate TLS slots, store markers and read them back
//!   the way rewritten guest code does: gs:[0x1480 + 8*slot] for slots under
//!   64, the expansion array at gs:[0x1780] for 64 and up, and gs:[0x58] (the
//!   static TLS array). Logs whether those offsets hold on this Windows build.
//! - TUXBOX_DUMP_RVAS=rva[:len],...: the title image's bytes at those RVAs
//!   (default 16 bytes each), to confirm patched instructions at run time.
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{TLS_OUT_OF_INDEXES, TlsAlloc, TlsFree, TlsSetValue};

pub(crate) fn run() {
    if std::env::var_os("TUXBOX_TEB_CHECK").is_some_and(|v| v == "1") {
        teb_check();
    }
    if let Ok(list) = std::env::var("TUXBOX_DUMP_RVAS") {
        dump(&list);
    }
}

fn gs(offset: usize) -> usize {
    let teb: usize;
    unsafe { core::arch::asm!("mov {}, gs:[0x30]", out(reg) teb, options(nostack, readonly, preserves_flags)) };
    unsafe { ((teb + offset) as *const usize).read_volatile() }
}

fn teb_check() {
    // Fill past 64 slots so one lands in the expansion array; free them after.
    let mut slots = Vec::new();
    let mut results = Vec::new();
    for _ in 0..80 {
        let slot = unsafe { TlsAlloc() };
        if slot == TLS_OUT_OF_INDEXES {
            break;
        }
        slots.push(slot);
        if slot != slots[0] && slot < 64 && slots.len() > 1 && slot != 63 {
            continue; // check the first low slot, 63, and the first expansion slot
        }
        let marker = 0x7ec0_0000_0000_0000usize | slot as usize;
        if unsafe { TlsSetValue(slot, Some(marker as *const _)) }.is_err() {
            continue;
        }
        let read = if slot < 64 {
            gs(0x1480 + 8 * slot as usize)
        } else {
            let expansion = gs(0x1780);
            if expansion == 0 { 0 } else { unsafe { ((expansion + 8 * (slot as usize - 64)) as *const usize).read_volatile() } }
        };
        results.push(format!("slot {slot} via {} {}", if slot < 64 { "gs:[0x1480+8*slot]" } else { "gs:[0x1780][slot-64]" },
            if read == marker { "ok".to_string() } else { format!("MISMATCH (read {read:#x}, stored {marker:#x})") }));
        if slot >= 64 {
            break;
        }
    }
    for slot in slots {
        let _ = unsafe { TlsFree(slot) };
    }
    let array = gs(0x58);
    super::log_line(format_args!("teb check: {}; static TLS array gs:[0x58] = {array:#x} (first cell {:#x})",
        if results.is_empty() { "no TLS slot available".into() } else { results.join(", ") },
        if array != 0 { unsafe { (array as *const usize).read_volatile() } } else { 0 }));
}

fn dump(list: &str) {
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else { return };
    let base = module.0 as usize;
    for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let (rva, len) = item.split_once(':').unwrap_or((item, "16"));
        let parse = |s: &str| {
            let s = s.trim();
            s.strip_prefix("0x").map_or_else(|| s.parse::<usize>().ok(), |h| usize::from_str_radix(h, 16).ok())
        };
        let (Some(rva), Some(len)) = (parse(rva), parse(len)) else {
            super::log_line(format_args!("byte dump: cannot read {item:?} (rva[:len], hex with 0x)"));
            continue;
        };
        let len = len.min(256);
        let bytes = super::crash_detail::bytes(base + rva, len);
        super::log_line(format_args!("byte dump: game+{rva:#x} ({len}): {bytes}"));
    }
}
