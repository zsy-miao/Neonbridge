//! Save slots: `<data>/<TITLEID>/slots/<name>/{storage,saves}`.
//!
//! Each slot is a whole game: a new name starts fresh, an existing one
//! continues. Saves made before slots existed live in `<TITLEID>/storage`
//! and `<TITLEID>/saves`; they are slot `default` until a `slots/default`
//! folder exists (nothing is moved, the old place is read in place).
//! Deleting moves a slot to `<TITLEID>/trash/<name>-<unix seconds>`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const DEFAULT: &str = "default";
const SUBS: [&str; 2] = ["storage", "saves"];

/// Where a slot keeps its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotPaths {
    pub storage: PathBuf,
    pub saves: PathBuf,
    /// Reads the pre-slot location.
    pub legacy: bool,
}

/// The slot name from `VICTORY_SAVE_SLOT` (default `default`), validated.
pub fn slot_from_env() -> Result<String> {
    let name = crate::env::env("VICTORY_SAVE_SLOT").filter(|s| !s.is_empty());
    let name = name.unwrap_or_else(|| DEFAULT.to_owned());
    validate(&name)?;
    Ok(name)
}

/// Names are 1 to 64 of letters, digits, `-`, `_` and `.` (not leading), so
/// they can never hold a path separator or climb out.
pub fn validate(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && !name.starts_with('.')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
    if !ok {
        bail!(
            "invalid save slot name {name:?}: use 1-64 letters, digits, '-', '_' or '.', \
             not starting with '.'"
        );
    }
    Ok(())
}

fn slots_dir(title: &Path) -> PathBuf {
    title.join("slots")
}

fn has_legacy(title: &Path) -> bool {
    SUBS.iter().any(|s| title.join(s).exists())
}

/// Paths of `slot` under a title folder, without creating anything.
pub fn resolve(title: &Path, slot: &str) -> Result<SlotPaths> {
    validate(slot)?;
    let dir = slots_dir(title).join(slot);
    if slot == DEFAULT && !dir.exists() && has_legacy(title) {
        return Ok(SlotPaths {
            storage: title.join("storage"),
            saves: title.join("saves"),
            legacy: true,
        });
    }
    Ok(SlotPaths {
        storage: dir.join("storage"),
        saves: dir.join("saves"),
        legacy: false,
    })
}

/// Slot names present, sorted (`default` included when only legacy saves exist).
pub fn list(title: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(slots_dir(title))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| validate(n).is_ok())
        .collect();
    if has_legacy(title) && !names.iter().any(|n| n == DEFAULT) {
        names.push(DEFAULT.to_owned());
    }
    names.sort();
    names
}

fn exists(title: &Path, name: &str) -> bool {
    list(title).iter().any(|n| n == name)
}

/// Creates an empty slot; it must not exist.
pub fn create(title: &Path, name: &str) -> Result<()> {
    validate(name)?;
    if exists(title, name) {
        bail!("save slot {name:?} already exists");
    }
    let dir = slots_dir(title).join(name);
    for s in SUBS {
        std::fs::create_dir_all(dir.join(s))
            .with_context(|| format!("creating {}", dir.display()))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_tree(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// Copies slot `from` to a new slot `to`.
pub fn copy(title: &Path, from: &str, to: &str) -> Result<()> {
    validate(to)?;
    if !exists(title, from) {
        bail!("save slot {from:?} does not exist");
    }
    if exists(title, to) {
        bail!("save slot {to:?} already exists");
    }
    let src = resolve(title, from)?;
    let dst = slots_dir(title).join(to);
    let done = (|| -> Result<()> {
        for (a, name) in [(&src.storage, "storage"), (&src.saves, "saves")] {
            if a.is_dir() {
                copy_tree(a, &dst.join(name))?;
            } else {
                std::fs::create_dir_all(dst.join(name))?;
            }
        }
        Ok(())
    })();
    if done.is_err() {
        let _ = std::fs::remove_dir_all(&dst); // only our half-made copy
    }
    done
}

/// Moves a slot to the trash folder; returns where it went.
pub fn delete(title: &Path, name: &str, unix_secs: u64) -> Result<PathBuf> {
    validate(name)?;
    if !exists(title, name) {
        bail!("save slot {name:?} does not exist");
    }
    let mut dest = title.join("trash").join(format!("{name}-{unix_secs}"));
    let mut n = 1;
    while dest.exists() {
        dest = title
            .join("trash")
            .join(format!("{name}-{unix_secs}-{n}"));
        n += 1;
    }
    let p = resolve(title, name)?;
    if p.legacy {
        std::fs::create_dir_all(&dest)?;
        for (a, s) in [(&p.storage, "storage"), (&p.saves, "saves")] {
            if a.exists() {
                std::fs::rename(a, dest.join(s))?;
            }
        }
    } else {
        std::fs::create_dir_all(title.join("trash"))?;
        std::fs::rename(slots_dir(title).join(name), &dest)?;
    }
    Ok(dest)
}

/// Title folders under the data directory (8 hex digits).
pub fn titles(data: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(data)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.len() == 8 && n.bytes().all(|b| b.is_ascii_hexdigit())
        })
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_reject_separators_and_escapes() {
        let long = "n".repeat(65);
        for bad in ["", "a/b", "a\\b", "..", ".hidden", "a:b", "x y", long.as_str()] {
            assert!(validate(bad).is_err(), "{bad:?}");
        }
        for good in ["default", "run-2", "A_b.1"] {
            validate(good).unwrap();
        }
    }

    #[test]
    fn slot_paths_and_legacy_pickup() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        let r = resolve(p, "default").unwrap();
        assert!(!r.legacy && r.saves == p.join("slots/default/saves"));
        assert!(list(p).is_empty());
        // Old saves appear as default and are read in place.
        std::fs::create_dir_all(p.join("saves")).unwrap();
        std::fs::write(p.join("saves/x"), b"1").unwrap();
        let r = resolve(p, "default").unwrap();
        assert!(r.legacy && r.saves == p.join("saves"));
        assert_eq!(list(p), ["default"]);
        assert!(!resolve(p, "b").unwrap().legacy);
        assert!(resolve(p, "../x").is_err());
        create(p, "other").unwrap();
        copy(p, "default", "copy").unwrap();
        assert_eq!(std::fs::read(p.join("slots/copy/saves/x")).unwrap(), b"1");
        assert_eq!(list(p), ["copy", "default", "other"]);
        assert!(copy(p, "default", "copy").is_err());
        assert!(create(p, "default").is_err());
    }

    #[test]
    fn delete_moves_to_trash() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path();
        std::fs::create_dir_all(p.join("saves")).unwrap();
        std::fs::write(p.join("saves/x"), b"1").unwrap();
        let d = delete(p, "default", 5).unwrap();
        assert_eq!(std::fs::read(d.join("saves/x")).unwrap(), b"1");
        assert!(list(p).is_empty());
        create(p, "a").unwrap();
        let d = delete(p, "a", 5).unwrap();
        assert!(d.join("saves").is_dir() && !p.join("slots/a").exists());
        assert!(delete(p, "a", 5).is_err());
    }
}
