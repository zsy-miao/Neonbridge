//! Automatic stubs for functions XCL does not implement yet.
//!
//! Each stub is a tiny x64 thunk:
//! ```text
//! sub  rsp, 0x28
//! mov  rcx, <StubInfo*>
//! mov  rdx, [rsp+0x28]        ; return address = caller
//! mov  rax, <stub_report>
//! call rax
//! add  rsp, 0x28
//! ret                         ; rax = configured return value
//! ```
//! Argument registers are clobbered but the stub never looks at them; RCX/RDX are
//! volatile in the Win64 ABI so the caller does not care either.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Deserialize;

use crate::util::parse_ret;

pub struct StubInfo {
    pub dll: String,
    pub name: String,
    pub ret: u64,
    pub calls: AtomicU64,
    pub first_caller: AtomicUsize,
    /// Optional `fn(&StubInfo, caller)` invoked on every call (e.g. present heuristics).
    pub hook: AtomicUsize,
}

pub type StubHook = fn(&StubInfo, usize);

static ALL: Mutex<Vec<&'static StubInfo>> = Mutex::new(Vec::new());
static ARENA: Mutex<(usize, usize)> = Mutex::new((0, 0)); // (next, end)
static BY_KEY: OnceLock<Mutex<HashMap<String, (usize, &'static StubInfo)>>> = OnceLock::new();

const THUNK_SIZE: usize = 48;
const ARENA_CHUNK: usize = 64 * 1024;

/// Count a call, log the first one, run the hook.
pub fn record_call(info: &StubInfo, caller: usize) {
    let n = info.calls.fetch_add(1, Ordering::Relaxed);
    if n == 0 {
        info.first_caller.store(caller, Ordering::Relaxed);
        crate::xlog_warn!(
            "[STUB] {}!{} first called from {} → returning {:#x}",
            info.dll,
            info.name,
            crate::modules::describe_address(caller),
            info.ret
        );
    }
    let h = info.hook.load(Ordering::Acquire);
    if h != 0 {
        let f: StubHook = unsafe { std::mem::transmute(h) };
        f(info, caller);
    }
}

unsafe extern "system" fn stub_report(info: *const StubInfo, caller: usize) -> u64 {
    let info = &*info;
    record_call(info, caller);
    info.ret
}

/// A StubInfo that is not backed by a stub thunk (used by D3D12.X slot guards).
pub fn new_info(dll: &str, name: &str, ret: u64) -> &'static StubInfo {
    let info: &'static StubInfo = Box::leak(Box::new(StubInfo {
        dll: dll.to_string(),
        name: name.to_string(),
        ret,
        calls: AtomicU64::new(0),
        first_caller: AtomicUsize::new(0),
        hook: AtomicUsize::new(0),
    }));
    ALL.lock().unwrap().push(info);
    info
}

/// Allocate `size` bytes of executable memory for generated thunks.
pub fn alloc_code(size: usize) -> *mut u8 {
    use windows_sys::Win32::System::Memory::{VirtualAlloc, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE};
    let size = (size + 15) & !15;
    let mut a = ARENA.lock().unwrap();
    if a.0 == 0 || a.0 + size > a.1 {
        let p = unsafe {
            VirtualAlloc(std::ptr::null(), ARENA_CHUNK, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE)
        } as usize;
        assert!(p != 0, "VirtualAlloc for stub arena failed");
        *a = (p, p + ARENA_CHUNK);
    }
    let p = a.0;
    a.0 += size;
    p as *mut u8
}

fn alloc_thunk() -> *mut u8 {
    alloc_code(THUNK_SIZE)
}

pub fn info_of_stub(addr: usize) -> Option<&'static StubInfo> {
    let map = BY_KEY.get()?;
    let g = map.lock().ok()?;
    g.values().find(|&&(a, _)| a == addr).map(|&(_, info)| info)
}

/// Create (or reuse) a stub for `dll!name`. Returns the callable address.
pub fn make_stub(dll: &str, name: &str, ret: u64) -> usize {
    make_stub_info(dll, name, ret).0
}

/// Like `make_stub`, also returning the StubInfo (to attach a hook).
pub fn make_stub_info(dll: &str, name: &str, ret: u64) -> (usize, &'static StubInfo) {
    let key = format!("{}!{}", dll.to_ascii_lowercase(), name);
    let map = BY_KEY.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = map.lock().unwrap();
    if let Some(&entry) = g.get(&key) {
        return entry;
    }
    let info: &'static StubInfo = Box::leak(Box::new(StubInfo {
        dll: dll.to_string(),
        name: name.to_string(),
        ret,
        calls: AtomicU64::new(0),
        first_caller: AtomicUsize::new(0),
        hook: AtomicUsize::new(0),
    }));
    let p = alloc_thunk();
    let mut code: Vec<u8> = Vec::with_capacity(THUNK_SIZE);
    code.extend_from_slice(&[0x48, 0x83, 0xEC, 0x28]); // sub rsp, 0x28
    code.extend_from_slice(&[0x48, 0xB9]); // mov rcx, imm64
    code.extend_from_slice(&(info as *const StubInfo as u64).to_le_bytes());
    code.extend_from_slice(&[0x48, 0x8B, 0x54, 0x24, 0x28]); // mov rdx, [rsp+0x28]
    code.extend_from_slice(&[0x48, 0xB8]); // mov rax, imm64
    code.extend_from_slice(&(stub_report as usize as u64).to_le_bytes());
    code.extend_from_slice(&[0xFF, 0xD0]); // call rax
    code.extend_from_slice(&[0x48, 0x83, 0xC4, 0x28]); // add rsp, 0x28
    code.push(0xC3); // ret
    debug_assert!(code.len() <= THUNK_SIZE);
    // SAFETY: `p` is freshly allocated with at least `THUNK_SIZE >= code.len()` bytes of RWX memory.
    unsafe {
        std::ptr::copy_nonoverlapping(code.as_ptr(), p, code.len());
        windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache(
            windows_sys::Win32::System::Threading::GetCurrentProcess(),
            p as *const _,
            code.len(),
        );
    }
    ALL.lock().unwrap().push(info);
    let entry = (p as usize, info);
    g.insert(key, entry);
    entry
}

pub fn all_stubs() -> Vec<&'static StubInfo> {
    ALL.lock().map(|v| v.clone()).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------
// Stub database: default return values + ordinal names
// ---------------------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct RawDb {
    #[serde(default)]
    module_defaults: HashMap<String, String>,
    #[serde(default)]
    functions: HashMap<String, String>,
    #[serde(default)]
    ordinals: HashMap<String, HashMap<String, String>>,
}

pub struct StubDb {
    module_defaults: HashMap<String, u64>,
    functions: HashMap<String, u64>,
    ordinals: HashMap<String, HashMap<u16, String>>,
}

static DB: OnceLock<StubDb> = OnceLock::new();

const BUILTIN_DB: &str = include_str!("../data/stubdb.toml");

impl StubDb {
    fn parse(text: &str, overrides: &std::collections::BTreeMap<String, String>) -> StubDb {
        let raw: RawDb = toml::from_str(text).unwrap_or_default();
        let mut functions: HashMap<String, u64> = raw
            .functions
            .iter()
            .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), parse_ret(v)?)))
            .collect();
        for (k, v) in overrides {
            if let Some(r) = parse_ret(v) {
                functions.insert(k.to_ascii_lowercase(), r);
            }
        }
        StubDb {
            module_defaults: raw
                .module_defaults
                .iter()
                .filter_map(|(k, v)| Some((k.to_ascii_lowercase(), parse_ret(v)?)))
                .collect(),
            functions,
            ordinals: raw
                .ordinals
                .into_iter()
                .map(|(dll, m)| {
                    (
                        dll.to_ascii_lowercase(),
                        m.into_iter().filter_map(|(o, n)| Some((o.parse().ok()?, n))).collect(),
                    )
                })
                .collect(),
        }
    }

    pub fn return_for(&self, dll: &str, name: &str) -> u64 {
        self.return_for_with_fallback(dll, None, name)
    }

    /// Look up the stub return value for `dll!name`, falling back to `fallback_dll` (e.g. a
    /// canonical HLE module name when `dll` is a routed alias) if `dll` has no explicit override
    /// or module default.
    pub fn return_for_with_fallback(&self, dll: &str, fallback_dll: Option<&str>, name: &str) -> u64 {
        let dll = dll.to_ascii_lowercase();
        let fallback = fallback_dll.map(str::to_ascii_lowercase);
        let n = name.to_ascii_lowercase();
        if let Some(&r) = self.functions.get(&format!("{dll}!{n}")) {
            return r;
        }
        if let Some(fb) = fallback.as_deref().filter(|&fb| fb != dll) {
            if let Some(&r) = self.functions.get(&format!("{fb}!{n}")) {
                return r;
            }
        }
        if let Some(&r) = self.functions.get(&format!("*!{n}")) {
            return r;
        }
        if let Some(&r) = self.module_defaults.get(&dll) {
            return r;
        }
        if let Some(fb) = fallback.as_deref() {
            if let Some(&r) = self.module_defaults.get(fb) {
                return r;
            }
        }
        0
    }

    pub fn ordinal_name(&self, dll: &str, ordinal: u16) -> Option<&str> {
        self.ordinals.get(&dll.to_ascii_lowercase())?.get(&ordinal).map(String::as_str)
    }
}

pub fn init_db(overrides: &std::collections::BTreeMap<String, String>) {
    let _ = DB.set(StubDb::parse(BUILTIN_DB, overrides));
}

pub fn db() -> &'static StubDb {
    DB.get_or_init(|| StubDb::parse(BUILTIN_DB, &Default::default()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stub_returns_configured_value_and_counts_calls() {
        let addr = make_stub("unit.dll", "Answer", 42);
        let f: extern "system" fn(usize, usize, usize, usize, usize) -> u64 = unsafe { std::mem::transmute(addr) };
        assert_eq!(f(1, 2, 3, 4, 5), 42);
        assert_eq!(f(0, 0, 0, 0, 0), 42);
        let info = all_stubs().into_iter().find(|s| s.name == "Answer").unwrap();
        assert_eq!(info.calls.load(Ordering::Relaxed), 2);
        assert_ne!(info.first_caller.load(Ordering::Relaxed), 0);
        // same key → same thunk
        assert_eq!(make_stub("UNIT.dll", "Answer", 1), addr);
    }

    #[test]
    fn builtin_db_parses_with_overrides() {
        let mut ov = std::collections::BTreeMap::new();
        ov.insert("kernelx.dll!Foo".to_string(), "7".to_string());
        ov.insert("xgameplatform.dll!PerAlias".to_string(), "0".to_string());
        ov.insert("xgameruntime.dll!CanonicalFn".to_string(), "1".to_string());
        let db = StubDb::parse(BUILTIN_DB, &ov);
        assert_eq!(db.return_for("kernelx.dll", "Foo"), 7);
        assert_eq!(db.return_for("xgameruntime.dll", "XSomethingUnknown"), 0x8000_4001);
        assert_eq!(db.return_for("kernelx.dll", "SomethingUnknown"), 0);
        assert_eq!(db.ordinal_name("d3d12_x.dll", 7), Some("D3D12CreateDevice"));
        // Routed HLE aliases inherit canonical module defaults while preserving per-alias overrides.
        assert_eq!(db.return_for_with_fallback("xgameplatform.dll", Some("xgameruntime.dll"), "XUnknown"), 0x8000_4001);
        assert_eq!(db.return_for_with_fallback("d3d12x.dll", Some("d3d12_xs.dll"), "D3D12Unknown"), 0x8000_4001);
        assert_eq!(db.return_for_with_fallback("xgameplatform.dll", Some("xgameruntime.dll"), "PerAlias"), 0);
        assert_eq!(db.return_for_with_fallback("xgameplatform.dll", Some("xgameruntime.dll"), "CanonicalFn"), 1);
        assert_eq!(db.return_for_with_fallback("xgameplatform.dll", Some("xgameruntime.dll"), "XGameUiShowErrorDialogAsync"), 0x8000_4004);
        assert_eq!(db.return_for_with_fallback("advapi32legacy.dll", Some("kernelx.dll"), "SomethingUnknown"), 0);
    }

    #[test]
    fn make_stub_info_is_race_free_under_concurrency() {
        use std::sync::{Arc, Barrier};

        const THREADS: usize = 8;
        const ITERS: usize = 50;

        for iter in 0..ITERS {
            let func = format!("ConcurrentRaceFn_{iter}");
            let barrier = Arc::new(Barrier::new(THREADS));
            let handles: Vec<_> = (0..THREADS)
                .map(|t| {
                    let barrier = Arc::clone(&barrier);
                    let func = func.clone();
                    let dll = if t % 2 == 0 { "RaceStub.dll" } else { "racestub.dll" };
                    std::thread::spawn(move || {
                        barrier.wait();
                        let (addr, info) = make_stub_info(dll, &func, 0x8000_4001);
                        (addr, info as *const StubInfo as usize)
                    })
                })
                .collect();

            let results: Vec<(usize, usize)> = handles.into_iter().map(|h| h.join().unwrap()).collect();
            let first = results[0];
            assert_ne!(first.0, 0);
            for &r in &results[1..] {
                assert_eq!(r, first, "iter {iter}: concurrent callers received different stubs");
            }
            assert_eq!(
                info_of_stub(first.0).map(|i| i as *const StubInfo as usize),
                Some(first.1)
            );
            let count = all_stubs()
                .into_iter()
                .filter(|s| s.dll.eq_ignore_ascii_case("racestub.dll") && s.name == func)
                .count();
            assert_eq!(count, 1, "iter {iter}: duplicate StubInfo pushed to ALL");
        }
    }
}
