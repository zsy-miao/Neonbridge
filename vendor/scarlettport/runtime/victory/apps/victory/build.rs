//! The Agility SDK opt-in (`D3D12SDKVersion`, `D3D12SDKPath` in `main.rs`) only works when the
//! executable exports both. lld-link exports `#[no_mangle]` statics of a program by itself;
//! MSVC's link.exe does not, and without them Windows 10 uses its inbox D3D12, which refuses the
//! root signatures the translator makes (heap directly indexed). Named here for every linker.
//! ScarlettPort: also stamps a version resource naming the program "ScarlettPort" (the release
//! ships it as runtime\ScarlettPort-Runtime.exe).
fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKVersion,DATA");
        println!("cargo:rustc-link-arg-bins=/EXPORT:D3D12SDKPath,DATA");
    }
    #[cfg(windows)]
    {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_default();
        let mut res = winresource::WindowsResource::new();
        res.set("ProductName", "ScarlettPort")
            .set("FileDescription", "ScarlettPort")
            .set("CompanyName", "ScarlettPort")
            .set("InternalName", "ScarlettPort-Runtime")
            .set("OriginalFilename", "ScarlettPort-Runtime.exe")
            .set("ProductVersion", &version)
            .set("FileVersion", &version)
            .set("LegalCopyright", "VIctory contributors and ScarlettPort (GPL-3.0-or-later)");
        if let Err(e) = res.compile() {
            println!("cargo:warning=version resource not embedded: {e}");
        }
    }
}