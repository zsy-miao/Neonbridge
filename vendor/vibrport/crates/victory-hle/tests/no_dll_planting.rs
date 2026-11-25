//! Host pass-through must only load DLLs from System32, never from the
//! current folder (e.g. a title's folder shipping its own `msvcp140_*.dll`).
#![cfg(windows)]

use victory_hle::{HleResolver, HostPolicy};
use victory_pe::ImportSymbol;

#[test]
fn ignores_dlls_in_current_folder() {
    let name = "msvcp140_victorytest.dll";
    assert!(
        HostPolicy::default().allows(name),
        "test needs an allow-listed name"
    );
    assert!(HostPolicy::default().allows("vcruntime140_1.dll"));

    let dir = tempfile::tempdir().unwrap();
    // Any loadable DLL will do.
    std::fs::copy(r"C:\Windows\System32\version.dll", dir.path().join(name)).unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let symbol = ImportSymbol::Name {
        hint: 0,
        name: "GetFileVersionInfoSizeW".into(),
    };
    let found = HleResolver::default().classify(name, &symbol);
    std::env::set_current_dir(env!("CARGO_MANIFEST_DIR")).unwrap();
    assert_eq!(found, None, "DLL was loaded from the current folder");
}
