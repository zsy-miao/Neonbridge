//! Stamps the Windows resources: icon, version information and the application manifest.
//! The engines are not embedded; the release ZIP ships them in a `runtime` folder beside the exe.

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="ScarlettPort.Launcher" version="0.2.1.0" processorArchitecture="amd64"/>
  <description>ScarlettPort Launcher</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>
"#;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let version = std::env::var("CARGO_PKG_VERSION").unwrap();
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico")
            .set_manifest(MANIFEST)
            .set("ProductName", "ScarlettPort")
            .set("FileDescription", "ScarlettPort Launcher")
            .set("CompanyName", "ScarlettPort")
            .set("InternalName", "ScarlettPort")
            .set("OriginalFilename", "ScarlettPort.exe")
            .set("ProductVersion", &version)
            .set("FileVersion", &version)
            .set("LegalCopyright", "ScarlettPort project (MIT)");
        if let Err(e) = res.compile() {
            panic!("Windows resources not embedded: {e}");
        }
    }
}