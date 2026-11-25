//! Crash-safe file replacement, and retries for the transient errors Windows
//! gives while a virus scanner, indexer or Explorer window briefly holds a
//! handle (sharing violation, lock violation, access denied).
//!
//! [`replace`] writes the new bytes to a temporary file next to the target,
//! flushes them to disk, keeps the previous contents as `<name>.bak` and only
//! then renames the temporary file over the target. A kill at any point
//! leaves the old file or the new one (plus, at worst, a stray temporary
//! file), never half of each. [`read_good`] falls back to the `.bak` copy
//! when the target is missing or fails the caller's check.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Tries for an operation that keeps failing with a transient error.
const TRIES: u32 = 12;

/// Windows errors that clear up by themselves: access denied (5), sharing
/// violation (32), lock violation (33).
pub fn transient(e: &io::Error) -> bool {
    cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33))
}

/// Runs `op`, again after a short wait while it fails with a [`transient`]
/// error (about two seconds in all).
pub fn retry<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut wait = 10;
    let mut tries = 1;
    loop {
        match op() {
            Err(e) if transient(&e) && tries < TRIES => {
                std::thread::sleep(Duration::from_millis(wait));
                wait = (wait * 2).min(250);
                tries += 1;
            }
            other => return other,
        }
    }
}

/// `std::fs::rename`, retried.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    retry(|| std::fs::rename(from, to))
}

/// Removes a folder and everything in it, retried; a missing folder is done.
pub fn remove_dir_all(path: &Path) -> io::Result<()> {
    match retry(|| std::fs::remove_dir_all(path)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Creates `path`, writes `data` and flushes it all the way to the disk.
pub fn write_synced(path: &Path, data: &[u8]) -> io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(data)?;
    file.sync_all()
}

fn sibling(path: &Path, prefix: &str, suffix: &str) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    path.with_file_name(format!("{prefix}{name}{suffix}"))
}

/// The previous contents of `path`.
pub fn backup_path(path: &Path) -> PathBuf {
    sibling(path, "", ".bak")
}

/// The temporary file `path`'s new contents are written to first.
pub fn temp_path(path: &Path) -> PathBuf {
    sibling(path, ".", ".tmp")
}

/// Step one of [`replace`]: the new contents, complete and flushed, in the
/// temporary file. The target is untouched. Returns the temporary file.
pub fn stage(path: &Path, data: &[u8]) -> io::Result<PathBuf> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    write_synced(&tmp, data)?;
    Ok(tmp)
}

/// Step two of [`replace`]: keeps the target's current contents as `.bak`
/// (itself replaced by a rename) and renames the staged file over the target.
pub fn publish(path: &Path, tmp: &Path) -> io::Result<()> {
    if path.is_file() {
        // A failure here costs the backup, never the save.
        let bak = backup_path(path);
        let bak_tmp = sibling(path, ".", ".bak.tmp");
        if std::fs::copy(path, &bak_tmp).is_ok() {
            let _ = rename(&bak_tmp, &bak);
        }
        let _ = std::fs::remove_file(&bak_tmp);
    }
    let renamed = rename(tmp, path);
    if renamed.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    renamed
}

/// Replaces the contents of `path` with `data`, atomically (see the module
/// documentation).
pub fn replace(path: &Path, data: &[u8]) -> io::Result<()> {
    let tmp = stage(path, data)?;
    publish(path, &tmp)
}

/// Where [`read_good`] found the contents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    Main,
    /// The last good copy; the main file was missing or failed the check.
    Backup,
}

/// The contents of `path` if `valid` accepts them, else of its `.bak` copy.
/// `None` when neither exists or passes.
pub fn read_good(path: &Path, valid: impl Fn(&[u8]) -> bool) -> Option<(Vec<u8>, Found)> {
    let read = |p: &Path| retry(|| std::fs::read(p)).ok().filter(|d| valid(d));
    if let Some(d) = read(path) {
        return Some((d, Found::Main));
    }
    read(&backup_path(path)).map(|d| (d, Found::Backup))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &Path) -> String {
        String::from_utf8(std::fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn replace_keeps_the_last_good_copy() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("sub/profiles.txt");
        replace(&f, b"one").unwrap(); // creates the folder
        assert_eq!(text(&f), "one");
        assert!(!backup_path(&f).exists(), "nothing to keep yet");
        replace(&f, b"two").unwrap();
        replace(&f, b"three").unwrap();
        assert_eq!(text(&f), "three");
        assert_eq!(text(&backup_path(&f)), "two");
        assert!(!temp_path(&f).exists());
    }

    /// A process killed after the new bytes are written but before the
    /// rename (simulated by running only the first step) changes nothing:
    /// the target and its backup read as before, and a later save works
    /// over the stray temporary file.
    #[test]
    fn a_crash_between_write_and_rename_loses_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("settings.bin");
        replace(&f, b"good 1").unwrap();
        replace(&f, b"good 2").unwrap();

        let tmp = stage(&f, b"half way to good 3").unwrap();
        assert!(tmp.exists());
        // "Crash": no publish. Everything an app would read is intact.
        assert_eq!(text(&f), "good 2");
        assert_eq!(text(&backup_path(&f)), "good 1");
        assert_eq!(read_good(&f, |_| true).unwrap(), (b"good 2".to_vec(), Found::Main));

        // The next run saves normally.
        replace(&f, b"good 3").unwrap();
        assert_eq!(text(&f), "good 3");
        assert_eq!(text(&backup_path(&f)), "good 2");
        assert!(!tmp.exists());
    }

    /// A crash after the old file was copied to `.bak` but before the
    /// rename also leaves the target whole.
    #[test]
    fn a_crash_after_the_backup_copy_keeps_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("x");
        replace(&f, b"old").unwrap();
        let _tmp = stage(&f, b"new").unwrap();
        std::fs::copy(&f, backup_path(&f)).unwrap(); // publish's first half
        assert_eq!(text(&f), "old");
        assert_eq!(read_good(&f, |_| true).unwrap().1, Found::Main);
    }

    #[test]
    fn a_damaged_or_missing_target_reads_the_last_good_copy() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("p.txt");
        let ok = |d: &[u8]| d.starts_with(b"OK");
        replace(&f, b"OK 1").unwrap();
        replace(&f, b"OK 2").unwrap();
        // The target gets truncated (a non-atomic writer was killed).
        std::fs::write(&f, b"").unwrap();
        assert_eq!(read_good(&f, ok).unwrap(), (b"OK 1".to_vec(), Found::Backup));
        std::fs::remove_file(&f).unwrap();
        assert_eq!(read_good(&f, ok).unwrap().1, Found::Backup);
        std::fs::remove_file(backup_path(&f)).unwrap();
        assert!(read_good(&f, ok).is_none());
    }

    #[test]
    fn retry_stops_on_other_errors_and_succeeds_after_transient_ones() {
        let mut calls = 0;
        let r: io::Result<()> = retry(|| {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert!(r.is_err() && calls == 1);
        assert!(remove_dir_all(Path::new("no/such/dir/anywhere")).is_ok());
        if cfg!(windows) {
            let mut calls = 0;
            let r = retry(|| {
                calls += 1;
                if calls < 3 { Err(io::Error::from_raw_os_error(32)) } else { Ok(calls) }
            });
            assert_eq!(r.unwrap(), 3);
        }
    }
}
