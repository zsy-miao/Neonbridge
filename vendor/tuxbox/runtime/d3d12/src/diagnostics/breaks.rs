//! TUXBOX_BREAK_RVAS=rva,...: persistent register-log breakpoints in the title
//! image (idea from palmetto bc380bbc, re-derived). Each RVA gets an int3; on
//! a hit the registers are logged, the original byte goes back, the thread
//! single-steps over the instruction (trap flag) and the int3 is rewritten,
//! so every pass logs, not only the first. TUXBOX_BREAK_HITS (default 16)
//! caps the hits per site; past it the site stays restored. Installed at
//! device creation, after the title is loaded and patched.
//!
//! Every hit also logs 4 qwords at rax, rsi and rcx when readable (idea from
//! palmetto 6529787f). The list may name the preset `audiolock`: game+0x559465c
//! (the audio thread's call into the title lock routine, rcx = the audio
//! object) and game+0x43ae230 (the per-frame stream tick), plus a dump of the
//! stream job at game+0x9ae9988 (+0x78 CAS word, +0x7c busy, +0x80 pending) at
//! install, on each site's first hit and once after TUXBOX_AUDIO_LOCK_DUMP
//! seconds (default 30), to show whether the audio-lock stall from the
//! leonida-ce notes happens here too.
//!
//! TUXBOX_STOP_INT3=rva,...: a title int3 at one of these RVAs is reported with
//! its registers and left to fault instead of being stepped over (idea from
//! palmetto 45727f42): stepping past a failed null check turns it into a worse
//! fault later.
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use windows::Win32::System::Diagnostics::Debug::{AddVectoredExceptionHandler, CONTEXT, EXCEPTION_POINTERS, FlushInstructionCache};
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
static BASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static PATCH: Mutex<()> = Mutex::new(());

thread_local! {
    /// The site this thread is single-stepping over (0: none).
    static STEPPING: Cell<usize> = const { Cell::new(0) };
}

const BREAKPOINT: u32 = 0x8000_0003;
const SINGLE_STEP: u32 = 0x8000_0004;
const TRAP_FLAG: u32 = 0x100;

const AUDIO_LOCK_CALL: usize = 0x559465c;
const STREAM_TICK: usize = 0x43ae230;
const STREAM_JOB: usize = 0x9ae9988;
static AUDIO_PRESET: AtomicBool = AtomicBool::new(false);

fn parse_rva(item: &str) -> Option<usize> {
    let item = item.trim();
    item.strip_prefix("0x").map_or_else(|| item.parse::<usize>().ok(), |h| usize::from_str_radix(h, 16).ok())
}

/// The stream job words: +0x78 CAS, +0x7c busy, +0x80 pending.
fn stream_job(when: &str) {
    let base = BASE.load(Ordering::Relaxed);
    if base == 0 {
        return;
    }
    let job = base + STREAM_JOB;
    let word = |o: usize| super::crash_detail::read::<u32>(job + o).map_or("?".into(), |v| format!("{v:#x}"));
    super::log_line(format_args!(
        "break audiolock: stream job game+{STREAM_JOB:#x} {when}: +0x78={} +0x7c(busy)={} +0x80(pending)={}",
        word(0x78), word(0x7c), word(0x80)
    ));
}

/// 4 qwords at `address`, or "unreadable" (VirtualQuery-guarded read).
fn qwords(address: u64) -> String {
    match super::crash_detail::read::<[u64; 4]>(address as usize) {
        Some(q) => q.iter().map(|w| format!("{w:#x}")).collect::<Vec<_>>().join(" "),
        None => "unreadable".into(),
    }
}

fn registers(c: &CONTEXT) -> String {
    format!(
        "rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x} rbp={:#x} rsp={:#x} \
         r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x} eflags={:#x} stack [{}] \
         [rax] {} [rsi] {} [rcx] {}",
        c.Rax, c.Rbx, c.Rcx, c.Rdx, c.Rsi, c.Rdi, c.Rbp, c.Rsp,
        c.R8, c.R9, c.R10, c.R11, c.R12, c.R13, c.R14, c.R15, c.EFlags,
        stack_words(c.Rsp as usize), qwords(c.Rax), qwords(c.Rsi), qwords(c.Rcx),
    )
}

/// TUXBOX_STOP_INT3: true when the title int3 at `rva` is to be reported and
/// left to fault; the report is logged here.
pub(crate) fn stop_int3(rva: usize, c: &CONTEXT) -> bool {
    static LIST: OnceLock<Vec<usize>> = OnceLock::new();
    let list = LIST.get_or_init(|| {
        std::env::var("TUXBOX_STOP_INT3").map(|v| v.split(',').filter_map(parse_rva).collect()).unwrap_or_default()
    });
    if !list.contains(&rva) {
        return false;
    }
    if BASE.load(Ordering::Relaxed) == 0 && let Ok(module) = unsafe { GetModuleHandleW(None) } {
        BASE.store(module.0 as usize, Ordering::Relaxed);
    }
    super::log_line(format_args!(
        "stop int3: title trap at game+{rva:#x} thread {} not stepped over (TUXBOX_STOP_INT3): {}",
        unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, registers(c),
    ));
    true
}

pub(crate) fn install(list: &str) {
    let Ok(module) = (unsafe { GetModuleHandleW(None) }) else { return };
    let base = module.0 as usize;
    BASE.store(base, Ordering::Relaxed);
    if let Some(cap) = std::env::var("TUXBOX_BREAK_HITS").ok().and_then(|v| v.trim().parse().ok()) {
        CAP.store(cap, Ordering::Relaxed);
    }
    let mut sites = Vec::new();
    let mut items: Vec<String> = Vec::new();
    for item in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if item.eq_ignore_ascii_case("audiolock") {
            AUDIO_PRESET.store(true, Ordering::Relaxed);
            items.extend([format!("{AUDIO_LOCK_CALL:#x}"), format!("{STREAM_TICK:#x}")]);
        } else {
            items.push(item.to_string());
        }
    }
    if AUDIO_PRESET.load(Ordering::Relaxed) {
        let seconds = std::env::var("TUXBOX_AUDIO_LOCK_DUMP").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(30u64);
        stream_job("at install");
        let _ = std::thread::Builder::new().name("TuxBox audiolock dump".into()).spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(seconds));
            stream_job(&format!("after {seconds} s"));
        });
    }
    for item in items.iter().map(String::as_str) {
        let Some(rva) = parse_rva(item) else {
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
        super::log_line(format_args!("break: set at game+{:#x} (planted 0xcc over original byte {:#04x}, {} hits at most)", site.rva, site.original, CAP.load(Ordering::Relaxed)));
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

/// The first 8 stack words (raw, not an unwind: stale frames can look like
/// return addresses), title-image words as game+RVA. Logged in the hit's one line.
fn stack_words(rsp: usize) -> String {
    let base = BASE.load(Ordering::Relaxed);
    let size = if base == 0 { 0 } else { super::crash_detail::read::<u32>(base + 0x3c)
        .and_then(|nt| super::crash_detail::read::<u32>(base + nt as usize + 24 + 56)).unwrap_or(0) as usize };
    (0..8).map(|i| match super::crash_detail::read::<usize>(rsp + 8 * i) {
        Some(w) if w >= base && w < base + size => format!("game+{:#x}", w - base),
        Some(w) => format!("{w:#x}"),
        None => "?".into(),
    }).collect::<Vec<_>>().join(" ")
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
                super::log_line(format_args!(
                    "break game+{:#x} hit {hit} thread {}: {}{}",
                    site.rva, unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, registers(c),
                    if hit == cap { " (cap reached; site restored)" } else { "" },
                ));
            }
            if hit == 1 && AUDIO_PRESET.load(Ordering::Relaxed) && matches!(site.rva, AUDIO_LOCK_CALL | STREAM_TICK) {
                stream_job(&format!("at first hit of game+{:#x}", site.rva));
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
