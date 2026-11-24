//! TUXBOX_BREAK_RVAS=rva,...: persistent register-log breakpoints in the title
//! image (idea from palmetto bc380bbc, re-derived). Each RVA gets an int3; on
//! a hit the registers are logged, the original byte goes back, the thread
//! single-steps over the instruction (trap flag) and the int3 is rewritten,
//! so every pass logs, not only the first. TUXBOX_BREAK_HITS (default 16)
//! caps the hits per site; past it the site stays restored. Installed at
//! device creation, after the title is loaded and patched.
use std::cell::Cell;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, EXCEPTION_POINTERS, FlushInstructionCache};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, PAGE_PROTECTION_FLAGS, VirtualProtect};
use windows::Win32::System::Threading::GetCurrentProcess;

struct Site {
    rva: usize,
    address: usize,
    original: u8,
    hits: AtomicU32,
}

static SITES: OnceLock<Vec<Site>> = OnceLock::new();
static CAP: AtomicU32 = AtomicU32::new(16);
static PATCH: Mutex<()> = Mutex::new(());

thread_local! {
    /// The site this thread is single-stepping over (0: none).
    static STEPPING: Cell<usize> = const { Cell::new(0) };
}

const BREAKPOINT: u32 = 0x8000_0003;
const SINGLE_STEP: u32 = 0x8000_0004;
const TRAP_FLAG: u32 = 0x100;

pub(crate) fn install(list: &str) {
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else { return };
    let base = module.0 as usize;
    if let Some(cap) = std::env::var("TUXBOX_BREAK_HITS").ok().and_then(|v| v.trim().parse().ok()) {
        CAP.store(cap, Ordering::Relaxed);
    }
    let mut sites = Vec::new();
    for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let parsed = item.strip_prefix("0x").map_or_else(|| item.parse::<usize>().ok(), |h| usize::from_str_radix(h, 16).ok());
        let Some(rva) = parsed else {
            super::log_line(format_args!("break: cannot read {item:?} (hex RVA with 0x)"));
            continue;
        };
        let address = base + rva;
        if super::crash_detail::bytes(address, 1).is_empty() || sites.iter().any(|s: &Site| s.address == address) {
            super::log_line(format_args!("break: game+{rva:#x} skipped (unreadable or repeated)"));
            continue;
        }
        let original = unsafe { (address as *const u8).read_volatile() };
        if original == 0xcc {
            super::log_line(format_args!("break: game+{rva:#x} already holds int3; skipped"));
            continue;
        }
        sites.push(Site { rva, address, original, hits: AtomicU32::new(0) });
    }
    if sites.is_empty() || SITES.set(sites).is_err() {
        return;
    }
    // The handler goes in before any int3 is written.
    if unsafe { AddVectoredExceptionHandler(1, Some(handler)) }.is_null() {
        super::log_line(format_args!("break: AddVectoredExceptionHandler failed; no breakpoints set"));
        return;
    }
    for site in SITES.get().into_iter().flatten() {
        patch(site.address, 0xcc);
        super::log_line(format_args!("break: set at game+{:#x} (original byte {:#04x}, {} hits at most)", site.rva, site.original, CAP.load(Ordering::Relaxed)));
    }
}

fn patch(address: usize, byte: u8) {
    let _guard = PATCH.lock().unwrap_or_else(|e| e.into_inner());
    let mut old = PAGE_PROTECTION_FLAGS::default();
    unsafe {
        if VirtualProtect(address as _, 1, PAGE_EXECUTE_READWRITE, &mut old).is_err() {
            return;
        }
        (address as *mut u8).write_volatile(byte);
        let _ = VirtualProtect(address as _, 1, old, &mut old);
        let _ = FlushInstructionCache(GetCurrentProcess(), Some(address as _), 1);
    }
}

unsafe extern "system" fn handler(info: *mut EXCEPTION_POINTERS) -> i32 {
    const CONTINUE_EXECUTION: i32 = -1;
    const CONTINUE_SEARCH: i32 = 0;
    let Some(info) = (unsafe { info.as_ref() }) else { return CONTINUE_SEARCH };
    let (Some(record), Some(c)) = (unsafe { info.ExceptionRecord.as_ref() }, unsafe { info.ContextRecord.as_mut() }) else {
        return CONTINUE_SEARCH;
    };
    match record.ExceptionCode.0 as u32 {
        BREAKPOINT => {
            let address = record.ExceptionAddress as usize;
            let Some(site) = SITES.get().and_then(|s| s.iter().find(|s| s.address == address)) else { return CONTINUE_SEARCH };
            let hit = site.hits.fetch_add(1, Ordering::Relaxed) + 1;
            let cap = CAP.load(Ordering::Relaxed);
            if hit <= cap {
                let top = super::crash_detail::bytes(c.Rsp as usize, 8);
                super::log_line(format_args!(
                    "break game+{:#x} hit {hit} thread {}: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x} rsp={:#x} \
                     r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x} eflags={:#x} [rsp]={top}{}",
                    site.rva, unsafe { windows::Win32::System::Threading::GetCurrentThreadId() },
                    c.Rax, c.Rbx, c.Rcx, c.Rdx, c.Rsi, c.Rdi, c.Rbp, c.Rsp,
                    c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15, c.EFlags,
                    if hit == cap { " (cap reached; site restored)" } else { "" },
                ));
            }
            patch(address, site.original);
            c.Rip = address as u64;
            if hit < cap {
                c.EFlags |= TRAP_FLAG;
                STEPPING.with(|s| s.set(address));
            }
            CONTINUE_EXECUTION
        }
        SINGLE_STEP => {
            let address = STEPPING.with(|s| s.replace(0));
            if address == 0 {
                return CONTINUE_SEARCH;
            }
            patch(address, 0xcc);
            c.EFlags &= !TRAP_FLAG;
            CONTINUE_EXECUTION
        }
        _ => CONTINUE_SEARCH,
    }
}
