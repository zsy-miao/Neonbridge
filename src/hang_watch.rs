//! `--watch-hang SPEC` (NEONBRIDGE_WATCH_HANG): samples title globals every
//! 100 ms from outside the game process and logs them as they change, and a
//! watched value that stands still for 5, 15 and 25 s. Ported from five-star
//! FiveStar.Loader/Diagnostics/GameHangWatch.cs (45f5ca8, GPL-3.0-or-later,
//! with credit): the same poll, change log and stall ladder, read with
//! ReadProcessMemory instead of in-process, and with the addresses given
//! rather than built in (five-star's are for the PC 1.0.2.1 exe). Thread
//! stacks and fence-wait registers stay with the runtime's own hang dump.
//!
//! SPEC is a comma list of `RVA[:size][=name]`: RVA is hex from the game
//! module's base (game+0x...), size 1, 2, 4 or 8 bytes (default 8), e.g.
//! `--watch-hang 0x71b6338:8=heartbeat*,0x71273b0:2=state`. A name ending in
//! `*` is a heartbeat: only its stalls are logged, not each move.

use std::time::{Duration, Instant};

struct Watch {
    rva: u64,
    size: usize,
    name: String,
    beat: bool,
}

fn parse(spec: &str) -> Result<Vec<Watch>, String> {
    spec.split(',').filter(|s| !s.trim().is_empty()).map(|item| {
        let (addr, name) = item.split_once('=').map_or((item, None), |(a, n)| (a, Some(n.trim().to_owned())));
        let (rva, size) = addr.split_once(':').map_or((addr, "8"), |(a, s)| (a, s));
        let hex = rva.trim().trim_start_matches("game+").trim_start_matches("0x");
        let rva = u64::from_str_radix(hex, 16).map_err(|e| format!("--watch-hang {item}: {e}"))?;
        let size: usize = size.trim().parse().map_err(|e| format!("--watch-hang {item}: {e}"))?;
        if ![1, 2, 4, 8].contains(&size) {
            return Err(format!("--watch-hang {item}: size must be 1, 2, 4 or 8"));
        }
        let name = name.unwrap_or_else(|| format!("game+0x{rva:x}"));
        let beat = name.ends_with('*');
        Ok(Watch { rva, size, name: name.trim_end_matches('*').to_owned(), beat })
    }).collect()
}

/// The game process (by exe name, so it is found under cdb too) and its main module base.
fn find(exe_name: &str) -> Option<(u32, u64)> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::*;
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut e: PROCESSENTRY32W = std::mem::zeroed();
        e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
        let mut pid = None;
        let mut ok = Process32FirstW(snap, &mut e) != 0;
        while ok {
            let n = String::from_utf16_lossy(&e.szExeFile[..e.szExeFile.iter().position(|&c| c == 0).unwrap_or(260)]);
            if n.eq_ignore_ascii_case(exe_name) {
                pid = Some(e.th32ProcessID);
                break;
            }
            ok = Process32NextW(snap, &mut e) != 0;
        }
        CloseHandle(snap);
        let pid = pid?;
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE, pid);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut m: MODULEENTRY32W = std::mem::zeroed();
        m.dwSize = size_of::<MODULEENTRY32W>() as u32;
        // The first module of the snapshot is the exe.
        let base = (Module32FirstW(snap, &mut m) != 0).then(|| m.modBaseAddr as u64);
        CloseHandle(snap);
        Some((pid, base?))
    }
}

/// Starts the watch thread when SPEC names anything; `log` gets each line.
pub fn start(spec: &str, exe_name: String, log: impl Fn(String) + Send + 'static) -> Result<(), String> {
    let watches = parse(spec)?;
    if watches.is_empty() {
        return Ok(());
    }
    std::thread::Builder::new().name("hang-watch".into()).spawn(move || {
        use windows_sys::Win32::System::Diagnostics::Debug::ReadProcessMemory;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ};
        const STALL_LADDER: [f64; 3] = [5.0, 15.0, 25.0];
        let started = Instant::now();
        let (process, base) = loop {
            if started.elapsed() > Duration::from_secs(1800) {
                return;
            }
            if let Some((pid, base)) = find(&exe_name) {
                let h = unsafe { OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
                if !h.is_null() {
                    break (h, base);
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        };
        log(format!("[hang] watching {} game globals every 100 ms at base 0x{base:x} (five-star --watch-hang)", watches.len()));
        let mut last: Vec<Option<u64>> = vec![None; watches.len()];
        let mut since = vec![Instant::now(); watches.len()];
        let mut rung = vec![0usize; watches.len()];
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let t = started.elapsed().as_secs_f64();
            let mut changed = Vec::new();
            for (i, w) in watches.iter().enumerate() {
                let mut buf = [0u8; 8];
                let mut read = 0usize;
                let ok = unsafe { ReadProcessMemory(process, (base + w.rva) as _, buf.as_mut_ptr().cast(), w.size, &mut read) } != 0;
                if !ok || read != w.size {
                    // The process is gone (or the page is): the watch ends with it.
                    if last[i].is_some() && find(&exe_name).is_none() {
                        log(format!("[hang] {t:.1} s: the game exited"));
                        return;
                    }
                    continue;
                }
                let v = u64::from_le_bytes(buf);
                if last[i] != Some(v) {
                    let stood = since[i].elapsed().as_secs_f64();
                    if last[i].is_some() && stood >= 2.0 {
                        log(format!("[hang] {t:.1} s: {} moved again after {stood:.1} s", w.name));
                    }
                    if !w.beat {
                        changed.push(format!("{}=0x{v:x}", w.name));
                    }
                    last[i] = Some(v);
                    since[i] = Instant::now();
                    rung[i] = 0;
                } else if rung[i] < STALL_LADDER.len() && since[i].elapsed().as_secs_f64() >= STALL_LADDER[rung[i]] {
                    rung[i] += 1;
                    log(format!("[hang] {t:.1} s: {} still 0x{v:x} for {:.1} s", w.name, since[i].elapsed().as_secs_f64()));
                }
            }
            if !changed.is_empty() {
                log(format!("[hang] {t:.1} s: {}", changed.join(", ")));
            }
        }
    }).map_err(|e| e.to_string())?;
    Ok(())
}
