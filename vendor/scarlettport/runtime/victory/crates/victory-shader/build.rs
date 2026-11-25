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
    // Per-file and per-region digests for `rules` (see its notes).
    let mut file_digests: Vec<(String, String)> = Vec::new();
    let mut regions: std::collections::BTreeMap<String, blake3::Hasher> = Default::default();
    for (rel, full) in files {
        println!("cargo:rerun-if-changed={}", full.display());
        hasher.update(rel.as_bytes());
        hasher.update(&[0]);
        let contents = std::fs::read(&full)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", full.display()));
        hasher.update(&(contents.len() as u64).to_le_bytes());
        hasher.update(&contents);
        // Files that only tests build do not take part in any rule's fingerprint.
        if test_only(&rel) {
            continue;
        }
        let text = String::from_utf8_lossy(&contents);
        let mut base = blake3::Hasher::new();
        base.update(rel.as_bytes());
        base.update(&[0]);
        let mut open: Option<String> = None;
        for line in text.lines() {
            let t = line.trim();
            if let Some(name) = t.strip_prefix("// rule-region: begin ") {
                assert!(open.is_none(), "{rel}: rule-region inside a rule-region");
                open = Some(name.trim().to_owned());
            } else if t == "// rule-region: end" {
                let name = open.take().unwrap_or_else(|| panic!("{rel}: rule-region end without begin"));
                regions.entry(name).or_default().update(&[0xff]);
            } else if let Some(name) = &open {
                let h = regions.entry(name.clone()).or_default();
                h.update(line.as_bytes());
                h.update(b"
");
            } else {
                base.update(line.as_bytes());
                base.update(b"
");
            }
        }
        assert!(open.is_none(), "{rel}: rule-region without end");
        file_digests.push((rel, base.finalize().to_hex().to_string()));
    }
    println!(
        "cargo:rustc-env=VICTORY_SHADER_SOURCE_HASH={}",
        hasher.finalize().to_hex()
    );
    let mut out = String::from("pub(crate) const FILES: &[(&str, &str)] = &[
");
    for (rel, digest) in &file_digests {
        out.push_str(&format!("    ({rel:?}, {digest:?}),
"));
    }
    out.push_str("];
pub(crate) const REGIONS: &[(&str, &str)] = &[
");
    for (name, h) in regions {
        out.push_str(&format!("    ({name:?}, {:?}),
", h.finalize().to_hex().to_string()));
    }
    out.push_str("];
");
    let dest = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by Cargo")).join("rule_hashes.rs");
    std::fs::write(dest, out).expect("write rule_hashes.rs");
}

/// Sources only tests (or the host test harness) build.
fn test_only(rel: &str) -> bool {
    rel.ends_with("_tests.rs") || rel == "emit/host.rs"
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
