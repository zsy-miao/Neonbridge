//! Pass-through to the host's own implementation of shared APIs.

use victory_pe::ImportSymbol;

/// Which DLLs may be satisfied by the host's own implementation.
///
/// Console titles link mostly against Windows API sets that behave the same
/// on PC. Passing those through lets bring-up focus on what really differs.
/// Anything whose semantics differ on console (memory, files, threads) is
/// overridden in `modules`, which always wins over the host.
#[derive(Debug, Clone)]
pub struct HostPolicy {
    /// Lower-case patterns; a trailing `*` matches a prefix.
    pub allow: Vec<String>,
}

impl Default for HostPolicy {
    fn default() -> Self {
        let allow = [
            "api-ms-win-core-*",
            "api-ms-win-eventing-*",
            "api-ms-win-security-*",
            "api-ms-win-crt-*",
            "kernel32.dll",
            "kernelbase.dll",
            "ntdll.dll",
            "ucrtbase.dll",
            "vcruntime140*",
            "msvcp140*",
            // Off-machine traffic is refused in `modules::net`.
            "ws2_32.dll",
            "bcrypt.dll",
            "crypt32.dll",
            "ncrypt.dll",
            "winhttp.dll",
            "sspicli.dll",
            "cabinet.dll",
            "avrt.dll",
            // Window, message and shell APIs: the host's desktop window
            // stands in for the console's single core window.
            "ext-ms-win-rtcore-ntuser-*",
            "api-ms-win-shcore-*",
            "msvcp_win.dll",
            "ole32.dll",
            "advapi32.dll",
            "iphlpapi.dll",
            // The title's crash handler writes minidumps with it (loaded by name).
            "dbghelp.dll",
            // Same APIs as the console's (XAudio2 ordinal 1 is XAudio2Create
            // on both); Media Foundation reads local media files.
            "xaudio2_9.dll",
            "mfplat.dll",
            "mfreadwrite.dll",
        ];
        HostPolicy {
            allow: allow.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl HostPolicy {
    /// No pass-through at all: shows the full surface a title needs.
    pub fn none() -> Self {
        HostPolicy { allow: Vec::new() }
    }

    pub fn allows(&self, dll: &str) -> bool {
        let dll = dll.to_ascii_lowercase();
        self.allow
            .iter()
            .any(|pattern| match pattern.strip_suffix('*') {
                Some(prefix) => dll.starts_with(prefix),
                None => dll == *pattern,
            })
    }

    pub(crate) fn resolve(&self, dll: &str, symbol: &ImportSymbol) -> Option<usize> {
        if !self.allows(dll) {
            return None;
        }
        imp::resolve(dll, symbol)
    }
}

/// The host's own `name` from the System32 DLL `dll`: what a title import
/// of it is bound to when we do not override it.
pub(crate) fn function(dll: &str, name: &str) -> Option<usize> {
    let name = ImportSymbol::Name {
        hint: 0,
        name: name.into(),
    };
    imp::resolve(dll, &name)
}

#[cfg(windows)]
mod imp {
    use std::collections::HashMap;
    use std::ffi::CString;
    use std::sync::{LazyLock, Mutex};

    use victory_pe::ImportSymbol;
    use windows_sys::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExA,
    };

    /// Module handles by lower-case DLL name (stored as usize: Send + Sync).
    static MODULES: LazyLock<Mutex<HashMap<String, usize>>> = LazyLock::new(Default::default);

    pub(super) fn resolve(dll: &str, symbol: &ImportSymbol) -> Option<usize> {
        let module = module(dll)?;
        let proc = match symbol {
            ImportSymbol::Name { name, .. } => {
                let name = CString::new(name.as_str()).ok()?;
                // SAFETY: valid module handle, NUL-terminated name.
                unsafe { GetProcAddress(module as _, name.as_ptr().cast()) }
            }
            // SAFETY: MAKEINTRESOURCE-style ordinal lookup on a valid module.
            ImportSymbol::Ordinal(o) => unsafe { GetProcAddress(module as _, *o as usize as _) },
        };
        proc.map(|f| f as *const () as usize)
    }

    fn module(dll: &str) -> Option<usize> {
        let key = dll.to_ascii_lowercase();
        let cache = || MODULES.lock().unwrap_or_else(|e| e.into_inner());
        let cached = cache().get(&key).copied();
        if cached.is_some() {
            return cached;
        }
        let name = CString::new(key.as_str()).ok()?;
        // Not under the cache's lock: loading takes the OS loader lock, and
        // title code that runs under that one (a DLL's thread attach, a TLS
        // callback) may look a function up here.
        // System32 only: never pick up a same-named DLL from the current folder or PATH.
        // SAFETY: NUL-terminated DLL name; only allow-listed system DLLs get here.
        let handle = unsafe {
            LoadLibraryExA(
                name.as_ptr().cast(),
                std::ptr::null_mut(),
                LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
        } as usize;
        // A thread that loaded it meanwhile got the same module, which stays loaded.
        (handle != 0).then(|| *cache().entry(key).or_insert(handle))
    }

    #[cfg(test)]
    mod tests {
        use std::ffi::c_void;
        use std::ptr::{null, null_mut};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};

        use windows_sys::Win32::System::Threading::{
            CreateThread, INFINITE, Sleep, WaitForSingleObject,
        };

        const CHILD: &str = "VICTORY_HOST_LOCK_CHILD";
        static ARMED: AtomicBool = AtomicBool::new(false);
        static IN_ATTACH: AtomicBool = AtomicBool::new(false);
        static STARTED: AtomicBool = AtomicBool::new(false);

        /// Runs as title TLS callbacks do: under the loader lock, as a new
        /// thread starts.
        #[used]
        #[unsafe(link_section = ".CRT$XLY")]
        static ON_THREAD: unsafe extern "system" fn(*mut c_void, u32, *mut c_void) = attach;

        unsafe extern "system" fn attach(_: *mut c_void, reason: u32, _: *mut c_void) {
            const DLL_THREAD_ATTACH: u32 = 2;
            if reason == DLL_THREAD_ATTACH && ARMED.swap(false, Ordering::SeqCst) {
                IN_ATTACH.store(true, Ordering::SeqCst);
                // SAFETY: a plain sleep, for the other thread to start a load.
                unsafe { Sleep(500) };
                let found = super::super::function("kernel32.dll", "GetTickCount");
                assert!(found.is_some());
            }
        }

        unsafe extern "system" fn idle(_: *mut c_void) -> u32 {
            0
        }

        /// One thread loads a DLL nothing has loaded yet, which needs the
        /// loader lock, while another thread's attach holds that lock and
        /// looks a function up. Run in a child process: a deadlock inside the
        /// loader keeps a process from exiting, so the parent kills it.
        #[test]
        fn loading_a_host_dll_does_not_block_thread_attach() {
            if std::env::var_os(CHILD).is_none() {
                let name = "host::imp::tests::loading_a_host_dll_does_not_block_thread_attach";
                let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(["--exact", name])
                    .env(CHILD, "1")
                    .spawn()
                    .unwrap();
                let start = Instant::now();
                loop {
                    if let Some(status) = child.try_wait().unwrap() {
                        assert!(status.success());
                        return;
                    }
                    if start.elapsed() > Duration::from_secs(30) {
                        child.kill().unwrap();
                        panic!("deadlock: a host DLL loads under the module cache's lock");
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            // The attach below only needs the cache, no load of its own.
            assert!(super::super::function("kernel32.dll", "GetTickCount").is_some());
            let loader = std::thread::spawn(|| {
                STARTED.store(true, Ordering::SeqCst);
                while !IN_ATTACH.load(Ordering::SeqCst) {
                    std::hint::spin_loop();
                }
                super::super::function("cabinet.dll", "FDICreate")
            });
            // Its own attach is over: the next is the thread below.
            while !STARTED.load(Ordering::SeqCst) {
                std::hint::spin_loop();
            }
            ARMED.store(true, Ordering::SeqCst);
            let (security, start) = (std::ptr::null(), Some(idle as _));
            // SAFETY: a thread that returns at once.
            let thread = unsafe { CreateThread(security, 0, start, null(), 0, null_mut()) };
            assert!(!thread.is_null());
            assert!(loader.join().unwrap().is_some());
            // SAFETY: our own thread's handle.
            unsafe { WaitForSingleObject(thread, INFINITE) };
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn resolve(_dll: &str, _symbol: &victory_pe::ImportSymbol) -> Option<usize> {
        None
    }
}
