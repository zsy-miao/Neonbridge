//! (Neptune) A fingerprint of the shader translator's sources, for the shader cache's key
//! (`shader_cache::build`): the cache then lives through every rebuild that leaves
//! `victory-shader` as it was.
use std::path::{Path, PathBuf};

fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|x| x == "rs") {
            files.push(path);
        }
    }
}

fn main() {
    let sources = Path::new(env!("CARGO_MANIFEST_DIR")).join("../victory-shader/src");
    let mut files = Vec::new();
    walk(&sources, &mut files);
    files.sort();
    // FNV-1a over every source file's bytes (line ends left out: a checkout's setting).
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for file in &files {
        for byte in std::fs::read(file).unwrap_or_default() {
            if byte != b'\r' {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    }
    println!("cargo:rustc-env=VICTORY_TRANSLATOR={hash:016x}-{}", files.len());
    println!("cargo:rerun-if-changed={}", sources.display());
    // The build tag: the git short hash (it moves with every commit through the log's file).
    let git = |args: &[&str]| {
        std::process::Command::new("git").args(args).output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    println!("cargo:rustc-env=VICTORY_BUILD={}", git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".into()));
    if let Some(log) = git(&["rev-parse", "--git-path", "logs/HEAD"]) {
        println!("cargo:rerun-if-changed={log}");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
