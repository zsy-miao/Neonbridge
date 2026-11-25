//! Where the engines live. Nothing is embedded in the launcher and nothing is unpacked or
//! downloaded at run time: the release ZIP puts a `runtime` folder beside ScarlettPort.exe
//! (ScarlettPort-Runtime.exe, the VIctory-based runtime, and its `D3D12` folder for the World engine; xcl.exe, dxcompiler.dll and
//! dxil.dll for the Classic engine when present), and the launcher runs them from there.
//! `SCARLETT_RUNTIME_DIR` points at another folder (for development builds).
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Settings and the default cache location (data only, never executables).
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("ScarlettPort")
}

/// The `runtime` folder beside the launcher.
pub fn runtime_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("SCARLETT_RUNTIME_DIR") {
        return PathBuf::from(d);
    }
    std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(|p| p.join("runtime")))
        .unwrap_or_else(|| PathBuf::from("runtime"))
}

pub fn xcl_exe() -> PathBuf {
    runtime_dir().join("xcl.exe")
}

/// The release's name for the VIctory-based runtime (victory.exe).
pub const RUNTIME_EXE: &str = "ScarlettPort-Runtime.exe";

/// The World engine: `ScarlettPort-Runtime.exe`, else a development build's `victory.exe`.
pub fn victory_exe() -> PathBuf {
    let named = runtime_dir().join(RUNTIME_EXE);
    if named.is_file() {
        return named;
    }
    runtime_dir().join("victory.exe")
}

/// What was found in the runtime folder: (folder, World engine hash, Classic engine hash).
/// A hash is the SHA-256 of the engine's executable; it is what "shaders built for" refers to.
pub struct Found {
    pub dir: PathBuf,
    pub world: Option<String>,
    pub classic: Option<String>,
}

pub fn file_hash(p: &Path) -> Option<String> {
    let bytes = std::fs::read(p).ok().filter(|b| !b.is_empty())?;
    Some(Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

/// Looks for the engines beside the launcher.
pub fn locate() -> Result<Found, String> {
    let dir = runtime_dir();
    let found = Found { world: file_hash(&victory_exe()), classic: file_hash(&xcl_exe()), dir: dir.clone() };
    if found.world.is_none() && found.classic.is_none() {
        return Err(format!(
            "No engine found in {}. Extract the whole release ZIP and keep the runtime folder beside ScarlettPort.exe.",
            dir.display()
        ));
    }
    Ok(found)
}
