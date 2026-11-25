//! Stubs for imports nobody implements yet.
//!
//! Each unresolved import gets 32 bytes of generated code:
//!
//! ```text
//! mov rcx, <&'static ImportSite>   ; 48 B9 imm64
//! mov rax, <unimplemented_import>  ; 48 B8 imm64
//! jmp rax                          ; FF E0
//! ```
//!
//! The jump keeps the title's return address on the stack, so the handler
//! returns straight to the caller. Bring-up then shows exactly which API to
//! implement next, rather than failing at load time.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Result, bail};
use windows_sys::Win32::System::Diagnostics::Debug::{
    DebugBreak, FlushInstructionCache, IsDebuggerPresent,
};
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualProtect,
};
use windows_sys::Win32::System::Threading::GetCurrentProcess;

/// What an unimplemented import does when called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnimplementedPolicy {
    /// Log, break into an attached debugger, then abort. The default:
    /// silently continuing usually crashes later and further from the cause.
    #[default]
    Trap,
    /// Log once per import and return 0. Useful to push through bring-up.
    ReturnZero,
}

/// Identifies one unresolved import. Leaked: lives as long as the process.
#[derive(Debug)]
pub struct ImportSite {
    pub dll: String,
    pub symbol: String,
    pub policy: UnimplementedPolicy,
    pub calls: AtomicU64,
}

static UNIMPLEMENTED_CALLS: AtomicU64 = AtomicU64::new(0);

/// Total number of calls that reached an unimplemented-import stub.
pub fn unimplemented_call_count() -> u64 {
    UNIMPLEMENTED_CALLS.load(Ordering::Relaxed)
}

extern "system" fn unimplemented_import(site: &'static ImportSite) -> u64 {
    UNIMPLEMENTED_CALLS.fetch_add(1, Ordering::Relaxed);
    let previous = site.calls.fetch_add(1, Ordering::Relaxed);
    match site.policy {
        UnimplementedPolicy::ReturnZero => {
            if previous == 0 {
                tracing::warn!(dll = %site.dll, symbol = %site.symbol, "unimplemented import called, returning 0");
            }
            0
        }
        UnimplementedPolicy::Trap => {
            tracing::error!(dll = %site.dll, symbol = %site.symbol, "unimplemented import called");
            // Into the crash log too: `abort` ends the process without
            // running the handlers that write it.
            let (dll, symbol) = (&site.dll, &site.symbol);
            let text = format!("VIctory: unimplemented import {dll}!{symbol} called");
            crate::log_crash(&text);
            // SAFETY: plain Win32 calls without preconditions.
            unsafe {
                if IsDebuggerPresent() != 0 {
                    DebugBreak();
                }
            }
            std::process::abort();
        }
    }
}

const THUNK_SIZE: usize = 32;

/// Executable memory holding one stub per unresolved import.
pub(crate) struct ThunkBlock {
    base: *mut u8,
    len: usize,
    used: usize,
}

impl ThunkBlock {
    /// Reserves room for `count` stubs (writable until [`Self::seal`]).
    pub(crate) fn new(count: usize) -> Result<Self> {
        let len = (count.max(1) * THUNK_SIZE).next_multiple_of(4096);
        // SAFETY: fresh anonymous allocation; no existing memory is touched.
        let base = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_COMMIT | MEM_RESERVE,
                PAGE_READWRITE,
            )
        } as *mut u8;
        if base.is_null() {
            bail!(
                "VirtualAlloc for import stubs failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(ThunkBlock { base, len, used: 0 })
    }

    /// Emits a stub for `site` and returns its address.
    pub(crate) fn emit(&mut self, site: &'static ImportSite) -> Result<usize> {
        if self.used + THUNK_SIZE > self.len {
            bail!("import stub block is full");
        }
        let mut code = [0xCC_u8; THUNK_SIZE]; // int3 padding
        code[0..2].copy_from_slice(&[0x48, 0xB9]);
        code[2..10].copy_from_slice(&(site as *const ImportSite as u64).to_le_bytes());
        code[10..12].copy_from_slice(&[0x48, 0xB8]);
        let handler = unimplemented_import as extern "system" fn(&'static ImportSite) -> u64;
        code[12..20].copy_from_slice(&(handler as *const () as u64).to_le_bytes());
        code[20..22].copy_from_slice(&[0xFF, 0xE0]);
        // SAFETY: `used + THUNK_SIZE <= len`, and the block is still writable.
        let at = unsafe {
            let at = self.base.add(self.used);
            std::ptr::copy_nonoverlapping(code.as_ptr(), at, THUNK_SIZE);
            at
        };
        self.used += THUNK_SIZE;
        Ok(at as usize)
    }

    /// Makes the block read+execute. The block is intentionally leaked:
    /// title code may hold these addresses for the life of the process.
    pub(crate) fn seal(self) -> Result<()> {
        let mut old = 0;
        // SAFETY: `base..base+len` is the allocation made in `new`.
        let ok = unsafe { VirtualProtect(self.base as _, self.len, PAGE_EXECUTE_READ, &mut old) };
        if ok == 0 {
            bail!(
                "VirtualProtect for import stubs failed: {}",
                std::io::Error::last_os_error()
            );
        }
        // SAFETY: flushing our own process's instruction cache for our own block.
        unsafe { FlushInstructionCache(GetCurrentProcess(), self.base as _, self.len) };
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In a child process of the next test: calls an unimplemented import
    /// that traps, with the crash log where `VICTORY_TRAP_LOG` says.
    #[test]
    fn trap_child() {
        let Some(log) = std::env::var_os("VICTORY_TRAP_LOG") else {
            return;
        };
        crate::log_crashes_to(log.into());
        let site = Box::leak(Box::new(ImportSite {
            dll: "probe.dll".into(),
            symbol: "Missing".into(),
            policy: UnimplementedPolicy::Trap,
            calls: AtomicU64::new(0),
        }));
        unimplemented_import(site);
    }

    #[test]
    fn a_trapped_import_is_in_the_crash_log() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("crashes.log");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "thunks::tests::trap_child", "--nocapture"])
            .env("VICTORY_TRAP_LOG", &log)
            .output()
            .unwrap();
        assert!(!out.status.success(), "the trap ends the process");
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        let named = text.contains("VIctory: unimplemented import probe.dll!Missing called");
        assert!(named, "crash log: {text:?}");
    }
}
