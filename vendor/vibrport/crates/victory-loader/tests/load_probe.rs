//! End-to-end: prepare, load, bind and call into a stand-in title
//! (tests/fixtures/hle-probe), without any game data.
#![cfg(windows)]

use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::Command;

use victory_loader::{
    ImportResolver, Provenance, Resolution, UnimplementedPolicy, WithTitleDlls, prepare,
    unimplemented_call_count,
};
use victory_pe::ImportSymbol;
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};
use windows_sys::Win32::System::Threading::{CreateThread, INFINITE, WaitForSingleObject};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

/// Builds the fixture into its own target dir (the outer cargo holds the
/// lock on the main one while tests run), in the outer one's scratch folder
/// for tests, so that it follows `CARGO_TARGET_DIR` like the rest of the
/// build.
fn build_probe() -> PathBuf {
    let manifest = workspace_root().join("tests/fixtures/hle-probe/Cargo.toml");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("hle-probe");
    let status = Command::new(env!("CARGO"))
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .status()
        .expect("running cargo");
    assert!(status.success(), "building hle-probe failed");
    target.join("debug").join("hle_probe.dll")
}

extern "system" fn fake_xgameruntime_initialize() -> i32 {
    0x1234
}
/// Asks for the current thread, as our logging does: on a thread std is
/// still starting, that must not abort the process.
extern "system" fn fake_get_tick_count() -> u32 {
    let _ = std::thread::current().id();
    777
}
extern "system" fn fake_delayed() -> u32 {
    4242
}

struct TestResolver;

impl ImportResolver for TestResolver {
    fn resolve(&self, dll: &str, symbol: &ImportSymbol) -> Resolution {
        let dll = dll.to_ascii_lowercase();
        let ImportSymbol::Name { name, .. } = symbol else {
            return Resolution::Missing;
        };
        let hle = match (dll.as_str(), name.as_str()) {
            ("xgameruntime.dll", "XGameRuntimeInitialize") => {
                Some(fake_xgameruntime_initialize as *const () as usize)
            }
            ("kernel32.dll", "GetTickCount") => Some(fake_get_tick_count as *const () as usize),
            ("victory-probe-delayed.dll", "VIctoryProbeDelayed") => {
                Some(fake_delayed as *const () as usize)
            }
            _ => None,
        };
        if let Some(address) = hle {
            return Resolution::Found {
                address,
                provenance: Provenance::Hle,
            };
        }
        host(&dll, name)
            .map(|address| Resolution::Found {
                address,
                provenance: Provenance::Host,
            })
            .unwrap_or(Resolution::Missing)
    }
}

fn host(dll: &str, name: &str) -> Option<usize> {
    let dll = CString::new(dll).ok()?;
    let name = CString::new(name).ok()?;
    // SAFETY: NUL-terminated strings; loading system DLLs into the test process.
    unsafe {
        let module = LoadLibraryA(dll.as_ptr().cast());
        if module.is_null() {
            return None;
        }
        GetProcAddress(module, name.as_ptr().cast()).map(|f| f as *const () as usize)
    }
}

#[test]
fn loads_binds_and_calls_probe() {
    let dll = build_probe();
    let original = std::fs::read(&dll).unwrap();
    let prepared = prepare(&original).unwrap();

    let imports: Vec<_> = prepared
        .info
        .imports
        .iter()
        .map(|m| m.dll.to_ascii_lowercase())
        .collect();
    assert!(
        imports.contains(&"xgameruntime.dll".to_owned()),
        "{imports:?}"
    );
    assert!(
        imports.contains(&"victory-probe-missing.dll".to_owned()),
        "{imports:?}"
    );
    assert!(
        prepared
            .info
            .delay_imports
            .iter()
            .any(|m| m.dll.eq_ignore_ascii_case("victory-probe-delayed.dll")),
        "fixture must delay-load victory-probe-delayed.dll"
    );

    let dir = tempfile::tempdir().unwrap();
    let image = victory_loader::load(&prepared, dir.path()).unwrap();

    // The "title" ships victory-probe-title.dll: any real DLL will do.
    let title_dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        r"C:\Windows\System32\version.dll",
        title_dir.path().join("victory-probe-title.dll"),
    )
    .unwrap();
    let dlls =
        victory_loader::load_title_dlls(image.info(), title_dir.path(), dir.path(), &|_| true)
            .unwrap();
    let names: Vec<_> = dlls.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["victory-probe-title.dll"]);
    let resolver = WithTitleDlls {
        inner: &TestResolver,
        dlls: &dlls,
    };
    for (_, dll) in &dlls {
        dll.bind_imports(&resolver, UnimplementedPolicy::ReturnZero)
            .unwrap();
        // SAFETY: its imports are bound, and it depends on no other title DLL.
        unsafe { dll.attach() }.unwrap();
    }
    let report = image
        .bind_imports(&resolver, UnimplementedPolicy::ReturnZero)
        .unwrap();
    assert_eq!(report.title, 1);

    assert_eq!(report.hle, 3);
    assert!(report.host > 0);
    assert_eq!(report.missing.len(), 1);
    assert_eq!(report.missing[0].symbol, "VIctoryProbeNotImplemented");

    let call = |name: &str| -> usize { image.export(name).unwrap_or_else(|| panic!("{name}")) };
    // SAFETY: the exports have the signatures declared in the fixture and
    // every import they use has been bound.
    unsafe {
        let f: extern "system" fn() -> i32 = std::mem::transmute(call("probe_xgameruntime_init"));
        assert_eq!(f(), 0x1234, "console API must reach our implementation");

        let f: extern "system" fn() -> u32 = std::mem::transmute(call("probe_tick"));
        assert_eq!(f(), 777, "host API must be overridable");

        let f: extern "system" fn() -> u32 = std::mem::transmute(call("probe_pid"));
        assert_eq!(f(), std::process::id(), "host API must pass through");

        let f: extern "system" fn() -> u32 = std::mem::transmute(call("probe_title"));
        assert_ne!(f(), 0, "title DLL export must be bound");

        let f: extern "system" fn() -> u32 = std::mem::transmute(call("probe_delayed"));
        assert_eq!(f(), 4242, "delay-loaded API must be bound");

        let before = unimplemented_call_count();
        let f: extern "system" fn() -> u64 = std::mem::transmute(call("probe_missing"));
        assert_eq!(f(), 0);
        assert_eq!(unimplemented_call_count(), before + 1);

        // Once attached, every thread start reaches its TLS callback and
        // DllMain (other tests may start threads too, hence at least one).
        assert!(
            image
                .info()
                .tls
                .as_ref()
                .is_some_and(|t| !t.callback_rvas.is_empty())
        );
        image.attach().unwrap();
        let f: extern "system" fn() -> u64 = std::mem::transmute(call("probe_thread_starts"));
        let before = f();
        // A thread std starts, as our runtime's workers are: notified
        // before std sets it up, which must still find its state pristine.
        let worker = std::thread::Builder::new()
            .name("probe-worker".into())
            .spawn(|| std::thread::current().name().map(str::to_owned))
            .unwrap();
        assert_eq!(worker.join().unwrap().as_deref(), Some("probe-worker"));
        let mid = f();
        assert!(mid >> 32 > before >> 32 && mid as u32 > before as u32);
        let before = mid;
        // Starting in title code, as a title's threads do; the extra
        // argument is ignored.
        let start: unsafe extern "system" fn(*mut std::ffi::c_void) -> u32 =
            std::mem::transmute(call("probe_pid"));
        let thread = CreateThread(
            std::ptr::null(),
            0,
            Some(start),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
        );
        assert!(!thread.is_null());
        WaitForSingleObject(thread, INFINITE);
        CloseHandle(thread);
        let after = f();
        assert!(
            after >> 32 > before >> 32,
            "TLS callback: {before:#x} → {after:#x}"
        );
        assert!(
            after as u32 > before as u32,
            "DllMain: {before:#x} → {after:#x}"
        );
        // It has static TLS, so Windows would refuse too.
        assert_eq!(
            victory_loader::disable_thread_calls(image.base()),
            Some(false)
        );
        assert_eq!(victory_loader::disable_thread_calls(1), None);

        // `FreeLibrary`'s notice, as a failed late load sends it.
        let f: extern "system" fn() -> u32 = std::mem::transmute(call("probe_detaches"));
        assert_eq!(f(), 0);
        image.detach();
        assert_eq!(f(), 1);
        image.unload();
    }
}

#[test]
fn maps_a_title_dll_the_title_loads_itself() {
    let title_dir = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    // Any real DLL will do; it imports only host DLLs, which are not in the folder.
    std::fs::copy(
        r"C:\Windows\System32\version.dll",
        title_dir.path().join("Late.dll"),
    )
    .unwrap();
    let load = |name: &str, wanted: &dyn Fn(&str) -> bool| {
        victory_loader::load_title_dll(name, title_dir.path(), cache.path(), wanted).unwrap()
    };
    let dlls = load("LATE.DLL", &|_| true);
    let names: Vec<_> = dlls.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["late.dll"]);
    assert!(dlls[0].1.export("GetFileVersionInfoW").is_some());
    assert!(load("absent.dll", &|_| true).is_empty());
    assert!(load("late.dll", &|_| false).is_empty());
}
