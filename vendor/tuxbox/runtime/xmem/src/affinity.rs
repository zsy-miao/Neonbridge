// Console core affinity on the host.
//
// Copyright (C) omonkey (https://github.com/omonkeyz/xbox-emulator)
// Copyright (C) Neonbridge contributors
//
// Copied from omonkeyz/xbox-emulator
// crates/compat/compat-common/src/os_host/affinity.rs (commit 27122f7),
// with its hook list from os_host/mod.rs. Changes: TOMBOX_* switches are
// TUXBOX_*, logging goes through xmem's log, and the hooks are installed on
// the exe's and title DLLs' imports and GetProcAddress lookups.
//
// This program is free software: you can redistribute it and/or modify it
// under the terms of the GNU General Public License as published by the Free
// Software Foundation, either version 3 of the License, or (at your option)
// any later version. This program is distributed in the hope that it will be
// useful, but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the GNU General
// Public License for more details: <https://www.gnu.org/licenses/>.

//! The console's 7 cores map to separate physical host cores (one logical
//! processor each, fastest first). SetThreadAffinityMask and
//! SetProcessAffinityMask take console masks and pin to those host cores;
//! GetProcessAffinityMask answers in console cores; GetCurrentProcessorNumber
//! returns the console core; title threads start confined to the mapped
//! cores. Before, a console mask pinned threads to host logical processors
//! 0-6, so Bink's audio and decode threads could share hyperthread
//! siblings with busy title threads.
//! TUXBOX_AFFINITY=0 installs none of it; TUXBOX_CONFINE_THREADS=0 leaves
//! new threads unconfined; TUXBOX_CONSOLE_CORES=<n> changes the core count.
// Kept as copied: the unsafe calls sit in unsafe fns without inner blocks.
#![allow(unsafe_op_in_unsafe_fn)]

use core::ffi::c_void;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLogicalProcessorInformationEx(rel: u32, buf: *mut u8, len: *mut u32) -> i32;
    fn GetThreadId(h: *mut c_void) -> u32;
    fn GetProcessId(h: *mut c_void) -> u32;
    fn GetCurrentProcessId() -> u32;
    fn SetLastError(code: u32);
}

pub const DEFAULT_CONSOLE_CORES: usize = 7;
const ERROR_INVALID_PARAMETER: u32 = 87;

#[derive(Clone, Debug)]
pub struct Topology {
    pub cores: usize,
    pub full: u64,
    pub host: u64,
    pub map: Vec<u64>,
}

fn full_mask(n: usize) -> u64 {
    if n >= 64 {
        u64::MAX
    } else {
        (1u64 << n) - 1
    }
}

pub fn build_map(physical: &[u64], n: usize) -> Vec<u64> {
    let n = n.clamp(1, 64);
    let physical: Vec<u64> = physical.iter().copied().filter(|m| *m != 0).collect();
    if physical.is_empty() {
        return vec![0; n];
    }
    let units: Vec<u64> = if physical.len() >= n {
        physical.clone()
    } else {
        let mut v = Vec::new();
        let mut layer = 0;
        loop {
            let mut any = false;
            for m in &physical {
                let bits: Vec<u32> = (0..64).filter(|b| m & (1u64 << b) != 0).collect();
                if let Some(b) = bits.get(layer) {
                    v.push(1u64 << b);
                    any = true;
                }
            }
            if !any {
                break;
            }
            layer += 1;
        }
        v
    };
    let mut map = vec![0u64; n];
    if physical.len() >= n {
        for (i, m) in map.iter_mut().enumerate() {
            *m = physical[i] & physical[i].wrapping_neg();
        }
    } else if units.len() >= n {
        for (j, u) in units.iter().enumerate() {
            map[j % n] |= u;
        }
    } else {
        for (i, m) in map.iter_mut().enumerate() {
            *m = units[i % units.len()];
        }
    }
    map
}

impl Topology {
    pub fn new(physical: &[u64], host: u64, cores: usize) -> Topology {
        let cores = cores.clamp(1, 64);
        let phys: Vec<u64> = physical.iter().map(|m| m & host).filter(|m| *m != 0).collect();
        let phys = if phys.is_empty() {
            (0..64).filter(|b| host & (1u64 << b) != 0).map(|b| 1u64 << b).collect()
        } else {
            phys
        };
        Topology { cores, full: full_mask(cores), host, map: build_map(&phys, cores) }
    }

    pub fn to_host(&self, mask: u64, process_console: u64) -> Option<u64> {
        if mask == 0 {
            return None;
        }
        if mask & !self.full != 0 || mask & !process_console != 0 {
            return None;
        }
        let mut h = 0u64;
        for (i, m) in self.map.iter().enumerate() {
            if mask & (1u64 << i) != 0 {
                h |= m;
            }
        }
        let h = h & self.host;
        if h == 0 {
            None
        } else {
            Some(h)
        }
    }

    pub fn to_console(&self, host_mask: u64) -> u64 {
        let mut c = 0u64;
        for (i, m) in self.map.iter().enumerate() {
            if m & host_mask != 0 {
                c |= 1u64 << i;
            }
        }
        c
    }

    pub fn console_index(&self, host_lp: u32) -> u32 {
        let bit = if host_lp < 64 { 1u64 << host_lp } else { 0 };
        match self.map.iter().position(|m| m & bit != 0) {
            Some(i) => i as u32,
            None => host_lp % self.cores as u32,
        }
    }

    pub fn describe(&self) -> String {
        let parts: Vec<String> = self.map.iter().enumerate().map(|(i, m)| format!("c{i}->{m:#x}")).collect();
        format!("console {} cores (mask {:#x}) -> host process mask {:#x}: {}", self.cores, self.full, self.host, parts.join(" "))
    }
}

type SetThreadFn = unsafe extern "system" fn(*mut c_void, usize) -> usize;
type GetProcFn = unsafe extern "system" fn(*mut c_void, *mut usize, *mut usize) -> i32;
type SetProcFn = unsafe extern "system" fn(*mut c_void, usize) -> i32;
type CreateThreadFn = unsafe extern "system" fn(*mut c_void, usize, *mut c_void, *mut c_void, u32, *mut u32) -> *mut c_void;
type ProcNumFn = unsafe extern "system" fn() -> u32;
type ResumeFn = unsafe extern "system" fn(*mut c_void) -> u32;

struct Real {
    set_thread: SetThreadFn,
    get_process: GetProcFn,
    set_process: SetProcFn,
    create_thread: CreateThreadFn,
    proc_num: ProcNumFn,
    resume: ResumeFn,
}

fn real() -> &'static Real {
    static R: OnceLock<Real> = OnceLock::new();
    R.get_or_init(|| unsafe {
        Real {
            set_thread: core::mem::transmute(resolve("SetThreadAffinityMask")),
            get_process: core::mem::transmute(resolve("GetProcessAffinityMask")),
            set_process: core::mem::transmute(resolve("SetProcessAffinityMask")),
            create_thread: core::mem::transmute(resolve("CreateThread")),
            proc_num: core::mem::transmute(resolve("GetCurrentProcessorNumber")),
            resume: core::mem::transmute(resolve("ResumeThread")),
        }
    })
}

const CREATE_SUSPENDED: u32 = 4;

pub fn confine_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("TUXBOX_CONFINE_THREADS").ok().map(|v| v.trim() != "0").unwrap_or(true)
    })
}

pub unsafe extern "system" fn create_thread(sa: *mut c_void, stack: usize, start: *mut c_void, param: *mut c_void, flags: u32, tid: *mut u32) -> *mut c_void {
    let r = real();
    if !confine_enabled() {
        return (r.create_thread)(sa, stack, start, param, flags, tid);
    }
    let t = topology();
    let Some(host) = t.to_host(process_console_mask(), t.full) else {
        return (r.create_thread)(sa, stack, start, param, flags, tid);
    };
    let h = (r.create_thread)(sa, stack, start, param, flags | CREATE_SUSPENDED, tid);
    if h.is_null() {
        return h;
    }
    (r.set_thread)(h, host as usize);
    if flags & CREATE_SUSPENDED == 0 {
        (r.resume)(h);
    }
    static LOGGED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !LOGGED.swap(true, Ordering::Relaxed) {
        crate::loader::log(&format!("affinity: CreateThread: title threads start on console cores {:#x} -> host {host:#x}", process_console_mask()));
    }
    h
}

pub unsafe extern "system" fn get_current_processor_number() -> u32 {
    topology().console_index((real().proc_num)())
}

pub fn console_cores() -> usize {
    std::env::var("TUXBOX_CONSOLE_CORES")
        .ok()
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|n| (1..=64).contains(n))
        .unwrap_or(DEFAULT_CONSOLE_CORES)
}

pub unsafe fn host_physical_cores() -> Vec<(u8, u64)> {
    let mut len = 0u32;
    GetLogicalProcessorInformationEx(0, core::ptr::null_mut(), &mut len);
    if len == 0 {
        return Vec::new();
    }
    let mut buf = vec![0u8; len as usize];
    if GetLogicalProcessorInformationEx(0, buf.as_mut_ptr(), &mut len) == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + 48 <= len as usize {
        let rel = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap());
        let size = u32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
        if size == 0 {
            break;
        }
        if rel == 0 {
            let eff = buf[off + 9];
            let groups = u16::from_le_bytes(buf[off + 30..off + 32].try_into().unwrap()) as usize;
            for g in 0..groups {
                let p = off + 32 + g * 16;
                if p + 16 > off + size {
                    break;
                }
                let mask = u64::from_le_bytes(buf[p..p + 8].try_into().unwrap());
                let group = u16::from_le_bytes(buf[p + 8..p + 10].try_into().unwrap());
                if group == 0 && mask != 0 {
                    out.push((eff, mask));
                }
            }
        }
        off += size;
    }
    out
}

pub fn order_cores(mut cores: Vec<(u8, u64)>) -> Vec<u64> {
    cores.sort_by(|a, b| b.0.cmp(&a.0));
    cores.into_iter().map(|c| c.1).collect()
}

pub fn topology() -> &'static Topology {
    static T: OnceLock<Topology> = OnceLock::new();
    T.get_or_init(|| unsafe {
        let mut p = 0usize;
        let mut s = 0usize;
        let host = if (real().get_process)(-1isize as *mut c_void, &mut p, &mut s) != 0 && p != 0 { p as u64 } else { 1 };
        let cores = order_cores(host_physical_cores());
        Topology::new(&cores, host, console_cores())
    })
}

static PROCESS_CONSOLE: AtomicU64 = AtomicU64::new(0);

pub fn process_console_mask() -> u64 {
    let v = PROCESS_CONSOLE.load(Ordering::Acquire);
    if v == 0 {
        topology().full
    } else {
        v
    }
}

fn threads() -> &'static Mutex<HashMap<u32, u64>> {
    static M: OnceLock<Mutex<HashMap<u32, u64>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

fn note(api: &str, given: u64, host: Option<u64>) {
    static SEEN: OnceLock<Mutex<HashSet<(u8, u64)>>> = OnceLock::new();
    let tag = match api {
        "SetThreadAffinityMask" => 0u8,
        "SetProcessAffinityMask" => 1,
        _ => 2,
    };
    let mut s = SEEN.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap_or_else(|e| e.into_inner());
    if s.len() >= 256 || !s.insert((tag, given)) {
        return;
    }
    drop(s);
    let t = topology();
    let shape = if given & !t.full != 0 { "outside console cores" } else { "console-shaped, remapped" };
    let detail = match host {
        Some(h) => format!("{given:#x} ({shape}) -> host {h:#x}"),
        None => format!("{given:#x} rejected as on console (ERROR_INVALID_PARAMETER)"),
    };
    crate::loader::log(&format!("affinity: {api} {detail}"));
}

unsafe fn is_self_process(h: *mut c_void) -> bool {
    h as isize == -1 || (!h.is_null() && GetProcessId(h) == GetCurrentProcessId())
}

pub unsafe extern "system" fn set_thread_affinity_mask(h: *mut c_void, mask: usize) -> usize {
    let t = topology();
    let given = mask as u64;
    let host = t.to_host(given, process_console_mask());
    note("SetThreadAffinityMask", given, host);
    let Some(host) = host else {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    };
    let r = (real().set_thread)(h, host as usize);
    if r == 0 {
        return 0;
    }
    let tid = GetThreadId(h);
    let mut m = threads().lock().unwrap_or_else(|e| e.into_inner());
    let prev = m.insert(tid, given).unwrap_or_else(|| {
        let c = t.to_console(r as u64);
        if c == 0 || r as u64 & t.host == t.host {
            process_console_mask()
        } else {
            c
        }
    });
    if m.len() > 4096 {
        m.clear();
        m.insert(tid, given);
    }
    prev as usize
}

pub unsafe extern "system" fn get_process_affinity_mask(h: *mut c_void, process: *mut usize, system: *mut usize) -> i32 {
    if process.is_null() || system.is_null() || !is_self_process(h) {
        return (real().get_process)(h, process, system);
    }
    let mut p = 0usize;
    let mut s = 0usize;
    if (real().get_process)(h, &mut p, &mut s) == 0 {
        return 0;
    }
    *process = process_console_mask() as usize;
    *system = topology().full as usize;
    1
}

pub unsafe extern "system" fn set_process_affinity_mask(h: *mut c_void, mask: usize) -> i32 {
    if !is_self_process(h) {
        return (real().set_process)(h, mask);
    }
    let t = topology();
    let given = mask as u64;
    let host = t.to_host(given, t.full);
    note("SetProcessAffinityMask", given, host);
    let Some(host) = host else {
        SetLastError(ERROR_INVALID_PARAMETER);
        return 0;
    };
    let r = (real().set_process)(h, host as usize);
    if r != 0 {
        let c = if given & !t.full != 0 { t.to_console(host) } else { given };
        PROCESS_CONSOLE.store(c, Ordering::Release);
    }
    r
}

fn resolve(func: &str) -> usize {
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
        fn LoadLibraryW(name: *const u16) -> *mut c_void;
        fn GetProcAddress(module: *mut c_void, name: *const u8) -> *const c_void;
    }
    let name = format!("{func}\0");
    for dll in ["kernelbase.dll", "kernel32.dll"] {
        let w: Vec<u16> = dll.encode_utf16().chain([0]).collect();
        let mut h = unsafe { GetModuleHandleW(w.as_ptr()) };
        if h.is_null() {
            h = unsafe { LoadLibraryW(w.as_ptr()) };
        }
        if !h.is_null() {
            let p = unsafe { GetProcAddress(h, name.as_ptr()) } as usize;
            if p != 0 {
                return p;
            }
        }
    }
    0
}

fn enabled() -> bool {
    !std::env::var("TUXBOX_AFFINITY").is_ok_and(|v| v.trim() == "0")
}

fn aff_dll(d: &str) -> bool {
    d.starts_with("api-ms-win-core-processtopology-") || d == "kernel32.dll" || d == "kernelbase.dll"
}

fn thread_dll(d: &str) -> bool {
    d.starts_with("api-ms-win-core-processthreads-") || d == "kernel32.dll" || d == "kernelbase.dll"
}

fn hooks() -> [(&'static str, usize, fn(&str) -> bool); 5] {
    [
        ("SetThreadAffinityMask", set_thread_affinity_mask as *const () as usize, aff_dll),
        ("GetProcessAffinityMask", get_process_affinity_mask as *const () as usize, aff_dll),
        ("SetProcessAffinityMask", set_process_affinity_mask as *const () as usize, aff_dll),
        ("CreateThread", create_thread as *const () as usize, thread_dll),
        ("GetCurrentProcessorNumber", get_current_processor_number as *const () as usize, thread_dll),
    ]
}

/// GetProcAddress hook (net.rs).
pub(crate) fn lookup(dll: &str, name: &str) -> Option<usize> {
    if !enabled() {
        return None;
    }
    hooks().into_iter().find(|(n, _, accepts)| *n == name && accepts(dll)).map(|(_, f, _)| f)
}

/// Wraps a module's imports of the affinity functions.
pub(crate) unsafe fn patch_module(module: *mut c_void) -> usize {
    if !enabled() || module.is_null() {
        return 0;
    }
    let _ = real();
    hooks().into_iter().map(|(name, f, accepts)| unsafe {
        crate::loader::patch_module_imports(module.cast(), accepts, &[(name, f)], &[])
    }).sum()
}

/// Called from loader::install, after the exe's imports are bound.
pub(crate) unsafe fn install() {
    if !enabled() {
        crate::loader::log("affinity: TUXBOX_AFFINITY=0, console masks pass through");
        return;
    }
    unsafe extern "system" {
        fn GetModuleHandleW(name: *const u16) -> *mut c_void;
    }
    let n = unsafe { patch_module(GetModuleHandleW(core::ptr::null())) };
    crate::loader::log(&format!("affinity: {n} import(s) wrapped; {}{}", topology().describe(),
        if confine_enabled() { "" } else { "; new threads unconfined (TUXBOX_CONFINE_THREADS=0)" }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seven_console_cores_map_to_separate_physical_cores() {
        // 8 physical cores with two logical processors each.
        let physical: Vec<u64> = (0..8).map(|i| 0b11u64 << (2 * i)).collect();
        let t = Topology::new(&physical, 0xffff, 7);
        assert_eq!(t.map, vec![0x1, 0x4, 0x10, 0x40, 0x100, 0x400, 0x1000]);
        assert_eq!(t.to_host(0b10, t.full), Some(0x4));
        assert_eq!(t.to_host(0x80, t.full), None);
        assert_eq!(t.console_index(4), 2);
        assert_eq!(t.to_console(0x10), 0b100);
    }
}
