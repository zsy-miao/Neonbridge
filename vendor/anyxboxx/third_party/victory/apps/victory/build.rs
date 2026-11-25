//! (Neptune) The diagnostics build exports the two symbols by which the D3D12 loader finds a
//! redistributable runtime and debug layer beside the executable (`D3D12\`): the debug layer
//! Windows installs is older than its D3D12 runtime on some PCs, and a device is then refused.
fn main() {
    if std::env::var_os("CARGO_FEATURE_DIAGNOSTICS").is_some() {
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKVersion,DATA");
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKPath,DATA");
    }
}
