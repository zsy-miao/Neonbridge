//! Save folder, save backups and crash reports of the runtime (xcl).
//! The title's saves live under `<data>\<package>\saves` (connected storage) and `...\pls`
//! (persistent local storage); xcl writes crash reports to `%LOCALAPPDATA%\xcl\crash-reports`.
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Automatic backups kept (the newest ones).
pub const KEEP: usize = 10;

pub fn saves_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("saves")
}

/// Folders a backup copies, with their name inside the backup.
pub fn sources(data_dir: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![("saves", data_dir.join("saves")), ("pls", data_dir.join("pls"))]
}

pub fn backup_root(cache_dir: &str) -> PathBuf {
    let c = cache_dir.trim();
    if c.is_empty() {
        crate::runtime::data_dir().join("cache").join("save-backups")
    } else {
        Path::new(c).join("save-backups")
    }
}

pub fn reports_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("xcl").join("crash-reports")
}

/// `YYYY-MM-DD_HH-MM-SS` (UTC) from unix seconds; sorts by time.
pub fn stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86400) as i64, secs % 86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}_{:02}-{:02}-{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn now_stamp() -> String {
    stamp(SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

/// Copies `from` into `to`; returns the number of files and bytes.
pub fn copy_tree(from: &Path, to: &Path) -> std::io::Result<(u64, u64)> {
    std::fs::create_dir_all(to)?;
    let (mut files, mut bytes) = (0, 0);
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let t = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            let (f, b) = copy_tree(&e.path(), &t)?;
            files += f;
            bytes += b;
        } else {
            bytes += std::fs::copy(e.path(), &t)?;
            files += 1;
        }
    }
    Ok((files, bytes))
}

/// Backs the existing source folders up to `<root>\<label>`. `None` when there was nothing to copy.
pub fn backup(sources: &[(&str, PathBuf)], root: &Path, label: &str) -> std::io::Result<Option<(PathBuf, u64, u64)>> {
    let dest = root.join(label);
    let (mut files, mut bytes) = (0, 0);
    for (name, src) in sources {
        if src.is_dir() {
            let (f, b) = copy_tree(src, &dest.join(name))?;
            files += f;
            bytes += b;
        }
    }
    if files == 0 {
        let _ = std::fs::remove_dir_all(&dest);
        return Ok(None);
    }
    Ok(Some((dest, files, bytes)))
}

/// Deletes all but the newest `keep` backup folders in `root` (names sort by time).
pub fn prune(root: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let mut dirs: Vec<PathBuf> = rd.filter_map(|e| e.ok()).filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
    dirs.sort();
    let excess = dirs.len().saturating_sub(keep);
    for d in &dirs[..excess] {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// The newest crash report in `dir` written at or after `since`.
pub fn fresh_report(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("zip")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .filter(|(t, _)| *t >= since)
        .max_by_key(|(t, _)| *t)
        .map(|(_, p)| p)
}

/// Opens a folder (created when missing) in Explorer.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
}

/// Opens Explorer with `file` selected.
pub fn reveal(file: &Path) {
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", file.display())).spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sp-saves-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stamps_sort_and_format() {
        assert_eq!(stamp(0), "1970-01-01_00-00-00");
        assert_eq!(stamp(1_700_000_000), "2023-11-14_22-13-20");
        assert!(stamp(1_700_000_000) < stamp(1_700_000_001));
    }

    #[test]
    fn backup_copies_and_prunes() {
        let data = tmp("data");
        std::fs::create_dir_all(data.join("saves/slot1")).unwrap();
        std::fs::write(data.join("saves/slot1/a.bin"), b"abc").unwrap();
        std::fs::write(data.join("saves/b.bin"), b"de").unwrap();
        let root = tmp("root");
        let (dest, files, bytes) = backup(&sources(&data), &root, "2026-01-01_00-00-00").unwrap().unwrap();
        assert_eq!((files, bytes), (2, 5));
        assert_eq!(std::fs::read(dest.join("saves/slot1/a.bin")).unwrap(), b"abc");
        // nothing to copy: no empty backup folder is left
        let empty = tmp("empty");
        assert!(backup(&sources(&empty), &root, "x").unwrap().is_none());
        assert!(!root.join("x").exists());
        for i in 0..12 {
            std::fs::create_dir_all(root.join(format!("2026-02-{:02}_00-00-00", i + 1))).unwrap();
        }
        prune(&root, KEEP);
        let left: Vec<String> = std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left.len(), KEEP);
        assert!(!left.contains(&"2026-01-01_00-00-00".to_string()));
        assert!(left.contains(&"2026-02-12_00-00-00".to_string()));
        for d in [data, root, empty] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn finds_fresh_reports() {
        let d = tmp("reports");
        assert!(fresh_report(&d, SystemTime::UNIX_EPOCH).is_none());
        std::fs::write(d.join("a.zip"), b"z").unwrap();
        std::fs::write(d.join("note.txt"), b"t").unwrap();
        assert_eq!(fresh_report(&d, SystemTime::UNIX_EPOCH).unwrap().file_name().unwrap(), "a.zip");
        assert!(fresh_report(&d, SystemTime::now() + std::time::Duration::from_secs(60)).is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn backup_root_uses_cache() {
        assert_eq!(backup_root(" D:\\ScarlettCache "), Path::new("D:\\ScarlettCache").join("save-backups"));
    }
}
