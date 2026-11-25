use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by Cargo"),
    );
    let src_dir = manifest_dir.join("src");
    let mut files = Vec::new();
    collect_rs(&src_dir, &src_dir, &mut files);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut hasher = blake3::Hasher::new();
    for (rel, full) in files {
        println!("cargo:rerun-if-changed={}", full.display());
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        let contents = std::fs::read(&full)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", full.display()));
        hasher.update(&(contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
    }
    println!(
        "cargo:rustc-env=VICTORY_SHADER_SOURCE_HASH={}",
        hasher.finalize().to_hex()
    );
}

fn collect_rs(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) {
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read directory {}: {e}", dir.display()))
        .map(|entry| entry.expect("directory entry"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let rel = path
                .strip_prefix(root)
                .expect("path under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.push((rel, path));
        }
    }
}
