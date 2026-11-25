//! On-disk layout for game saves: GameSave\<container>\b_<blob>, names escaped.
//! Free of platform calls so it can be tested on any host.
use std::path::{Path, PathBuf};
use std::{fs, io};

/// Escape every byte that could act as a separator, dot or wildcard in a path.
pub fn encode(name: &str) -> String {
    let mut out = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'_' || b == b'-' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn decode(name: &str) -> Option<String> {
    let b = name.as_bytes();
    let (mut out, mut i) = (Vec::new(), 0);
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(name.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

pub struct Container {
    pub name: String,
    pub display: String,
    pub blobs: Vec<(String, u32)>,
    pub modified: i64,
}

fn dir(root: &Path, container: &str) -> Option<PathBuf> {
    (!container.is_empty() && container.len() <= 256).then(|| root.join(encode(container)))
}

/// A container exists once an update has committed at least one blob to it.
pub fn container(root: &Path, name: &str) -> Option<Container> {
    let dir = dir(root, name)?;
    let (mut blobs, mut modified) = (Vec::new(), 0);
    for entry in fs::read_dir(&dir).ok()?.flatten() {
        let file = entry.file_name();
        let Some(blob) = file.to_str().and_then(|f| f.strip_prefix("b_")).filter(|f| !f.ends_with(".tmp")).and_then(decode) else { continue };
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(size) = u32::try_from(meta.len()) else { continue };
        if let Some(t) = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()) {
            modified = modified.max(t.as_secs() as i64);
        }
        blobs.push((blob, size));
    }
    blobs.sort();
    let display = fs::read_to_string(dir.join("display")).unwrap_or_default();
    (!blobs.is_empty()).then(|| Container { name: name.into(), display, blobs, modified })
}

pub fn containers(root: &Path) -> Vec<Container> {
    let mut all: Vec<_> = fs::read_dir(root).into_iter().flatten().flatten()
        .filter_map(|e| container(root, &decode(e.file_name().to_str()?)?)).collect();
    all.sort_by(|a, b| a.name.cmp(&b.name));
    all
}

pub fn read(root: &Path, container: &str, blob: &str) -> Option<Vec<u8>> {
    fs::read(dir(root, container)?.join(format!("b_{}", encode(blob)))).ok()
}

/// Commit staged writes (Some) and deletes (None). Each blob replaces its
/// predecessor by rename, so an interrupted write never truncates a save.
pub fn commit(root: &Path, container: &str, display: &str, changes: &[(String, Option<Vec<u8>>)]) -> io::Result<()> {
    let dir = dir(root, container).ok_or(io::ErrorKind::InvalidInput)?;
    fs::create_dir_all(&dir)?;
    let staged: Vec<_> = changes.iter().map(|(blob, data)| (dir.join(format!("b_{}", encode(blob))), data)).collect();
    for (path, data) in &staged {
        if let Some(data) = data { fs::write(path.with_extension("tmp"), data)? }
    }
    for (path, data) in &staged {
        match data {
            Some(_) => fs::rename(path.with_extension("tmp"), path)?,
            None => match fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            },
        }
    }
    fs::write(dir.join("display"), display)
}

pub fn delete(root: &Path, container: &str) -> io::Result<()> {
    match fs::remove_dir_all(dir(root, container).ok_or(io::ErrorKind::InvalidInput)?) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

#[cfg(test)]
#[path = "tests/save_store.rs"]
mod tests;
