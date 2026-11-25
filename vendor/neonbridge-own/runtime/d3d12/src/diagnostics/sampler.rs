//! Samples where the busiest threads execute, so a heartbeat names what a
//! thread spends its CPU on: the function under its instruction pointer and
//! the title or provider functions that called it. Diagnosis only;
//! TUXBOX_SAMPLE=0 turns it off.
//!
//! A sampled thread is suspended only for a context read and a copy of the
//! top of its stack. Nothing is allocated or locked while it is suspended: it
//! may hold the heap or loader lock.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_CONTROL_AMD64, GetThreadContext, RtlLookupFunctionEntry};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
};
use windows::Win32::System::Memory::{MEM_COMMIT, MEMORY_BASIC_INFORMATION, PAGE_GUARD, PAGE_NOACCESS, VirtualQuery};
use windows::Win32::System::Threading::{
    GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_INFORMATION,
    THREAD_SUSPEND_RESUME,
};

const WATCHED: usize = 2;
/// The function under the instruction pointer, then up to three callers.
const DEPTH: usize = 4;
/// 16 KiB of stack searched for return addresses into the title's modules.
const STACK_WORDS: usize = 2048;

type Chain = [usize; DEPTH];

pub(crate) struct Module {
    pub(crate) base: usize,
    pub(crate) end: usize,
    pub(crate) name: String,
    /// The title's executable or a module beside it (the staged providers).
    title: bool,
}

static TARGETS: [AtomicU32; WATCHED] = [const { AtomicU32::new(0) }; WATCHED];
static NAMES: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());
static COUNTS: Mutex<Option<HashMap<(u32, Chain), u32>>> = Mutex::new(None);
static MODULES: Mutex<Vec<Module>> = Mutex::new(Vec::new());

fn enabled() -> bool {
    std::env::var_os("TUXBOX_SAMPLE").is_none_or(|v| v != "0")
}

/// Sample these threads until the next call. `threads` is (id, name).
pub(crate) fn watch(threads: &[(u32, String)]) {
    static START: std::sync::Once = std::sync::Once::new();
    if !enabled() {
        return;
    }
    for (i, slot) in TARGETS.iter().enumerate() {
        slot.store(threads.get(i).map_or(0, |t| t.0), Ordering::Relaxed);
    }
    *NAMES.lock().unwrap_or_else(|e| e.into_inner()) = threads.iter().take(WATCHED).cloned().collect();
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("TuxBox sampler".into()).spawn(run);
    });
}

pub(crate) fn modules() -> Vec<Module> {
    let mut list = Vec::new();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_string_lossy().to_lowercase()));
    let exe = std::env::current_exe().ok().map(|p| p.to_string_lossy().to_lowercase());
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, 0) else { return list };
        let mut entry = MODULEENTRY32W { dwSize: size_of::<MODULEENTRY32W>() as u32, ..Default::default() };
        let mut more = Module32FirstW(snap, &mut entry).is_ok();
        while more {
            let len = entry.szExePath.iter().position(|&c| c == 0).unwrap_or(entry.szExePath.len());
            let path = String::from_utf16_lossy(&entry.szExePath[..len]).to_lowercase();
            let (dir, file) = path.rsplit_once('\\').unwrap_or(("", &path));
            let name = if exe.as_deref() == Some(path.as_str()) {
                "game".to_owned()
            } else {
                file.rsplit_once('.').map_or(file, |(stem, _)| stem).to_owned()
            };
            let base = entry.modBaseAddr as usize;
            list.push(Module { base, end: base + entry.modBaseSize as usize, name, title: exe_dir.as_deref() == Some(dir) });
            more = Module32NextW(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    list.sort_unstable_by_key(|m| m.base);
    list
}

pub(crate) fn module_of(list: &[Module], address: usize) -> Option<&Module> {
    let i = list.partition_point(|m| m.base <= address);
    list.get(i.checked_sub(1)?).filter(|m| address < m.end)
}

/// True when the bytes before `address` are a call, so the address can be a
/// return address rather than a stale code pointer.
pub(crate) fn follows_call(module: &Module, address: usize) -> bool {
    if address < module.base + 8 {
        return false;
    }
    // SAFETY: the seven bytes before an address inside a loaded image.
    let b = unsafe { std::slice::from_raw_parts((address - 7) as *const u8, 7) };
    let indirect = |at: usize| b[at] == 0xff && (b[at + 1] >> 3) & 7 == 2;
    b[2] == 0xe8 || indirect(5) || indirect(4) || indirect(3) || indirect(1) || indirect(0)
}

pub(crate) fn function_start(address: usize) -> usize {
    let mut image = 0u64;
    // SAFETY: looks the address up in the loaded images' unwind tables.
    let entry = unsafe { RtlLookupFunctionEntry(address as u64, &mut image, None) };
    if entry.is_null() { address } else { image as usize + unsafe { (*entry).BeginAddress } as usize }
}

#[repr(C, align(16))]
struct Aligned(CONTEXT);

fn run() {
    let own = unsafe { GetCurrentThreadId() };
    let mut handles: Vec<(u32, HANDLE)> = Vec::new();
    let mut context = Box::new(Aligned(CONTEXT::default()));
    let mut stack = vec![0usize; STACK_WORDS];
    let mut list: Vec<Module> = Vec::new();
    let mut refreshed: Option<std::time::Instant> = None;
    let mut audio_modules = 0usize;
    let mut audio_threads: Vec<u32> = Vec::new();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(2));
        if refreshed.is_none_or(|at| at.elapsed().as_secs() >= 2) {
            list = modules();
            refreshed = Some(std::time::Instant::now());
            // Which host audio path the title opened (it has no audio provider of ours in its output).
            let sound: Vec<&str> = list.iter().map(|m| m.name.as_str())
                .filter(|n| ["audio", "mmdevapi", "avrt", "dsound", "winmm", "mfplat", "bink"].iter().any(|k| n.contains(k))).collect();
            audio_threads = audio_thread_ids(&list);
            if sound.len() != audio_modules {
                audio_modules = sound.len();
                super::log_line(format_args!("audio modules loaded: {}", sound.join(", ")));
                if sound.contains(&"xaudio2_9") {
                    super::audio::install();
                }
                if sound.contains(&"audioses") {
                    super::audio::install_wasapi();
                }
            }
            let mut shared = MODULES.lock().unwrap_or_else(|e| e.into_inner());
            *shared = list.iter().map(|m| Module { base: m.base, end: m.end, name: m.name.clone(), title: m.title }).collect();
        }
        let mut targets: Vec<u32> = TARGETS.iter().map(|t| t.load(Ordering::Relaxed)).filter(|&t| t != 0 && t != own).collect();
        // Threads started inside the host audio modules are sampled too: they
        // show whether the audio engine waits on the title.
        for &tid in &audio_threads {
            if !targets.contains(&tid) {
                targets.push(tid);
            }
        }
        handles.retain(|(tid, handle)| {
            let keep = targets.contains(tid);
            if !keep {
                let _ = unsafe { CloseHandle(*handle) };
            }
            keep
        });
        for &tid in &targets {
            if !handles.iter().any(|h| h.0 == tid) {
                let access = THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT | THREAD_QUERY_INFORMATION;
                match unsafe { OpenThread(access, false, tid) } {
                    Ok(handle) => handles.push((tid, handle)),
                    Err(_) => continue,
                }
            }
            let handle = handles.iter().find(|h| h.0 == tid).map(|h| h.1).unwrap();
            let mut words = 0usize;
            context.0.ContextFlags = CONTEXT_CONTROL_AMD64;
            // SAFETY: between suspend and resume only system calls and a copy
            // into buffers allocated above.
            let captured = unsafe {
                if SuspendThread(handle) == u32::MAX {
                    false
                } else {
                    let ok = GetThreadContext(handle, &mut context.0).is_ok();
                    let rsp = context.0.Rsp as usize;
                    let mut info = MEMORY_BASIC_INFORMATION::default();
                    if ok && rsp.is_multiple_of(8)
                        && VirtualQuery(Some(rsp as _), &mut info, size_of::<MEMORY_BASIC_INFORMATION>()) != 0
                        && info.State == MEM_COMMIT
                        && (info.Protect & (PAGE_GUARD | PAGE_NOACCESS)).0 == 0
                    {
                        let end = info.BaseAddress as usize + info.RegionSize;
                        words = ((end - rsp) / 8).min(STACK_WORDS);
                        std::ptr::copy_nonoverlapping(rsp as *const usize, stack.as_mut_ptr(), words);
                    }
                    ResumeThread(handle);
                    ok
                }
            };
            if !captured {
                continue;
            }
            let mut chain: Chain = [0; DEPTH];
            chain[0] = function_start(context.0.Rip as usize);
            let mut depth = 1;
            for &word in &stack[..words] {
                if depth == DEPTH {
                    break;
                }
                if let Some(module) = module_of(&list, word)
                    && module.title
                    && follows_call(module, word)
                {
                    let function = function_start(word);
                    if chain[depth - 1] != function {
                        chain[depth] = function;
                        depth += 1;
                    }
                }
            }
            let mut counts = COUNTS.lock().unwrap_or_else(|e| e.into_inner());
            *counts.get_or_insert_with(HashMap::new).entry((tid, chain)).or_insert(0) += 1;
        }
    }
}

pub(crate) fn label(list: &[Module], address: usize) -> String {
    match module_of(list, address) {
        Some(module) => format!("{}+{:#x}", module.name, address - module.base),
        None => format!("{address:#x}"),
    }
}

fn top<K: std::hash::Hash + Eq>(rows: HashMap<K, u32>, total: u32, count: usize, text: impl Fn(&K) -> String) -> String {
    let mut rows: Vec<(K, u32)> = rows.into_iter().collect();
    rows.sort_by_cached_key(|(key, n)| (std::cmp::Reverse(*n), text(key))); // ties by name: runs diff cleanly
    rows.iter().take(count).map(|(key, n)| format!("{} {:.0}%", text(key), f64::from(*n) * 100.0 / f64::from(total)))
        .collect::<Vec<_>>().join(", ")
}

/// One line per sampled thread for the samples since the last call:
/// "hot thread NAME(TID): N samples; leaf: module+0xRVA 41%, ...; via: callee < caller < caller 33%, ...".
/// Addresses are function starts; "via" lists callers inside the title's own
/// modules (the game and the providers beside it), nearest first.
pub(crate) fn report() -> Vec<String> {
    let Some(counts) = COUNTS.lock().unwrap_or_else(|e| e.into_inner()).take() else { return Vec::new() };
    let list = MODULES.lock().unwrap_or_else(|e| e.into_inner());
    let names = NAMES.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let mut threads: Vec<u32> = counts.keys().map(|k| k.0).collect();
    threads.sort_unstable();
    threads.dedup();
    threads.iter().map(|&tid| {
        let (mut leaf, mut via, mut total) = (HashMap::new(), HashMap::new(), 0u32);
        for ((_, chain), n) in counts.iter().filter(|(k, _)| k.0 == tid) {
            *leaf.entry(chain[0]).or_insert(0u32) += n;
            *via.entry([chain[1], chain[2], chain[3]]).or_insert(0u32) += n;
            total += n;
        }
        let audio = AUDIO_NAMES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|n| n.0 == tid).map(|n| n.1.clone());
        let name = names.iter().find(|n| n.0 == tid).map(|n| n.1.clone()).or(audio).unwrap_or_else(|| "?".into());
        format!("hot thread {name}({tid}): {total} samples; leaf: {}; via: {}",
            top(leaf, total, 6, |&a| label(&list, a)),
            top(via, total, 6, |c| {
                let callers: Vec<String> = c.iter().filter(|&&a| a != 0).map(|&a| label(&list, a)).collect();
                if callers.is_empty() { "(no title caller on the stack)".into() } else { callers.join(" < ") }
            }))
    }).collect()
}

static AUDIO_NAMES: Mutex<Vec<(u32, String)>> = Mutex::new(Vec::new());

/// Up to 3 threads of this process whose start address is in xaudio2_9,
/// audioses or the audio engine modules, named "start module" for the report.
fn audio_thread_ids(list: &[Module]) -> Vec<u32> {
    use windows::Win32::System::Diagnostics::ToolHelp::{TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    type Query = unsafe extern "system" fn(HANDLE, u32, *mut usize, u32, *mut u32) -> i32;
    let Ok(ntdll) = (unsafe { GetModuleHandleW(windows::core::w!("ntdll.dll")) }) else { return Vec::new() };
    let Some(query) = (unsafe { GetProcAddress(ntdll, windows::core::s!("NtQueryInformationThread")) }) else { return Vec::new() };
    let query: Query = unsafe { std::mem::transmute(query) };
    let mut found: Vec<(u32, String)> = Vec::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return Vec::new() };
        let pid = windows::Win32::System::Threading::GetCurrentProcessId();
        let mut entry = THREADENTRY32 { dwSize: size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = Thread32First(snap, &mut entry).is_ok();
        while more && found.len() < 3 {
            if entry.th32OwnerProcessID == pid
                && let Ok(thread) = OpenThread(THREAD_QUERY_INFORMATION, false, entry.th32ThreadID)
            {
                let mut start = 0usize;
                // ThreadQuerySetWin32StartAddress
                if query(thread, 9, &mut start, 8, std::ptr::null_mut()) >= 0
                    && let Some(module) = module_of(list, start)
                    && ["xaudio2_9", "audioses", "mmdevapi"].contains(&module.name.as_str())
                {
                    found.push((entry.th32ThreadID, format!("{} thread", module.name)));
                }
                let _ = CloseHandle(thread);
            }
            more = Thread32Next(snap, &mut entry).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    let ids = found.iter().map(|f| f.0).collect();
    *AUDIO_NAMES.lock().unwrap_or_else(|e| e.into_inner()) = found;
    ids
}
