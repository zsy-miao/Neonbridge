//! Rolling backups of game-save containers, and restoring them.
//!
//! A committed version of a container is a folder `v<N>` of blob files and
//! a `manifest` (see the HLE's XGameSave). It is never changed after the
//! commit, so a backup is that folder hard-linked (copied where links are
//! not possible) into `<provider>/.backups/<container folder>/<id>`, where
//! the id is `<unix milliseconds, 13 digits>-v<N>`. The backup is staged
//! under a temporary name and renamed into place, so it is whole or absent.
//! The last [`KEEP`] backups of each container stay; older ones go.
//!
//! Backups live beside the containers, under the same user and service
//! configuration, so they follow the profile; they are not counted against
//! the title's save quota.

use std::io;
use std::path::{Path, PathBuf};

use crate::atomic;

/// Backups kept per container.
pub const KEEP: usize = 10;
/// The folder under a provider's root that holds the backups.
pub const DIR: &str = ".backups";
/// A version's manifest file.
pub const MANIFEST: &str = "manifest";

/// One backup of a container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Backup {
    pub id: String,
    pub path: PathBuf,
    pub version: u64,
    pub unix_ms: u64,
    pub bytes: u64,
}

fn parse_id(id: &str) -> Option<(u64, u64)> {
    let (ms, version) = id.split_once("-v")?;
    if ms.len() != 13 {
        return None;
    }
    Some((ms.parse().ok()?, version.parse().ok()?))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Where the backups of `container_dir` (a container's folder) are kept.
pub fn dir_of(container_dir: &Path) -> Option<PathBuf> {
    let name = container_dir.file_name()?;
    Some(container_dir.parent()?.join(DIR).join(name))
}

fn files_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// The backups in `dir` (from [`dir_of`]), newest first.
pub fn list(dir: &Path) -> Vec<Backup> {
    let mut out: Vec<Backup> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let id = e.file_name().into_string().ok()?;
            let (unix_ms, version) = parse_id(&id)?;
            let path = e.path();
            let bytes = files_size(&path);
            Some(Backup { id, path, version, unix_ms, bytes })
        })
        .collect();
    out.sort_by(|a, b| (b.unix_ms, b.version).cmp(&(a.unix_ms, a.version)));
    out
}

/// Links (or copies) the files of one version folder into `to`.
fn clone_files(from: &Path, to: &Path) -> io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let target = to.join(entry.file_name());
        if std::fs::hard_link(entry.path(), &target).is_err() {
            std::fs::copy(entry.path(), &target)?;
            std::fs::File::open(&target)?.sync_all().ok();
        }
    }
    Ok(())
}

/// Backs up the committed version `version_dir` (number `version`) of the
/// container in `container_dir`, then drops all but the newest `keep`
/// backups. Returns the new backup's folder.
pub fn snapshot(container_dir: &Path, version_dir: &Path, version: u64, keep: usize) -> io::Result<PathBuf> {
    let dir = dir_of(container_dir).ok_or_else(|| io::Error::other("no container folder name"))?;
    std::fs::create_dir_all(&dir)?;
    // Ids stay increasing even if the clock steps back.
    let newest = list(&dir).first().map_or(0, |b| b.unix_ms);
    let ms = now_ms().max(newest + 1);
    let id = format!("{ms:013}-v{version}");
    let staged = dir.join(format!(".tmp-{id}"));
    let _ = atomic::remove_dir_all(&staged);
    if let Err(e) = clone_files(version_dir, &staged) {
        let _ = atomic::remove_dir_all(&staged);
        return Err(e);
    }
    let done = dir.join(&id);
    if let Err(e) = atomic::rename(&staged, &done) {
        let _ = atomic::remove_dir_all(&staged);
        return Err(e);
    }
    prune(&dir, keep);
    Ok(done)
}

/// Removes the oldest backups in `dir` beyond `keep`, and leftover staging
/// folders. Failures are left for the next time.
pub fn prune(dir: &Path, keep: usize) {
    for old in list(dir).into_iter().skip(keep) {
        let _ = atomic::remove_dir_all(&old.path);
    }
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        if entry.file_name().to_string_lossy().starts_with(".tmp-") {
            let _ = atomic::remove_dir_all(&entry.path());
        }
    }
}

/// The committed version numbers in a container's folder, ascending.
fn versions(container_dir: &Path) -> Vec<u64> {
    let mut v: Vec<u64> = std::fs::read_dir(container_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            name.strip_prefix('v')?.parse().ok()
        })
        .collect();
    v.sort_unstable();
    v
}

/// The newest version of a container that has a manifest.
fn newest_version(container_dir: &Path) -> Option<(u64, PathBuf)> {
    versions(container_dir)
        .into_iter()
        .rev()
        .map(|n| (n, container_dir.join(format!("v{n}"))))
        .find(|(_, p)| p.join(MANIFEST).is_file())
}

/// Makes `backup` the container's newest version, as a new committed
/// version (staged, flushed, renamed). The version it replaces is backed up
/// first, so a restore can itself be undone. Returns the new version number.
pub fn restore(container_dir: &Path, backup: &Backup, keep: usize) -> io::Result<u64> {
    if !backup.path.join(MANIFEST).is_file() {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the backup has no manifest"));
    }
    std::fs::create_dir_all(container_dir)?;
    if let Some((n, dir)) = newest_version(container_dir) {
        snapshot(container_dir, &dir, n, keep)?;
    }
    let number = versions(container_dir).last().map_or(1, |n| n + 1);
    let staged = container_dir.join(format!("v{number}.restore.new"));
    let _ = atomic::remove_dir_all(&staged);
    std::fs::create_dir_all(&staged)?;
    let staged_ok = (|| -> io::Result<()> {
        for entry in std::fs::read_dir(&backup.path)? {
            let entry = entry?;
            if entry.file_type()?.is_file() {
                let data = std::fs::read(entry.path())?;
                atomic::write_synced(&staged.join(entry.file_name()), &data)?;
            }
        }
        Ok(())
    })();
    if let Err(e) = staged_ok {
        let _ = atomic::remove_dir_all(&staged);
        return Err(e);
    }
    if let Err(e) = atomic::rename(&staged, &container_dir.join(format!("v{number}"))) {
        let _ = atomic::remove_dir_all(&staged);
        return Err(e);
    }
    Ok(number)
}

/// The container name, the first length-prefixed string of a manifest.
pub fn manifest_name(bytes: &[u8]) -> Option<String> {
    let (len, tail) = bytes.split_first_chunk::<4>()?;
    let len = u32::from_le_bytes(*len) as usize;
    String::from_utf8(tail.get(..len)?.to_vec()).ok()
}

/// A container as found on disk, live and/or in the backups.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub name: String,
    /// The container's folder (it may not exist when only backups remain).
    pub dir: PathBuf,
    /// The newest committed version, `None` if the container is gone.
    pub version: Option<u64>,
    pub bytes: u64,
    pub backups: Vec<Backup>,
}

/// One user's saves under one service configuration.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    /// The user folder: the XUID in hex.
    pub user: String,
    pub config: String,
    pub root: PathBuf,
    pub containers: Vec<ContainerInfo>,
}

fn dirs(path: &Path) -> Vec<(String, PathBuf)> {
    let mut v: Vec<(String, PathBuf)> = std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    v.sort();
    v
}

fn name_from(dir: &Path) -> Option<String> {
    manifest_name(&std::fs::read(dir.join(MANIFEST)).ok()?)
}

/// Everything under a slot's `saves` folder: users, configurations,
/// containers (live ones and ones only the backups remember), backups.
/// Reads only.
pub fn inspect(saves: &Path) -> Vec<ProviderInfo> {
    let mut out = Vec::new();
    for (user, user_dir) in dirs(saves) {
        for (config, root) in dirs(&user_dir) {
            let mut folders: Vec<String> = dirs(&root)
                .into_iter()
                .map(|(n, _)| n)
                .filter(|n| n.contains('~') && !n.ends_with(".gone"))
                .collect();
            folders.extend(dirs(&root.join(DIR)).into_iter().map(|(n, _)| n));
            folders.sort();
            folders.dedup();
            let mut containers = Vec::new();
            for folder in folders {
                let dir = root.join(&folder);
                let backups = dir_of(&dir).map(|d| list(&d)).unwrap_or_default();
                let live = newest_version(&dir);
                let name = live
                    .as_ref()
                    .and_then(|(_, p)| name_from(p))
                    .or_else(|| backups.iter().find_map(|b| name_from(&b.path)));
                let Some(name) = name else { continue };
                let bytes = live.as_ref().map_or(0, |(_, p)| files_size(p));
                containers.push(ContainerInfo {
                    name,
                    dir,
                    version: live.map(|(n, _)| n),
                    bytes,
                    backups,
                });
            }
            out.push(ProviderInfo { user: user.clone(), config, root, containers });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str) -> Vec<u8> {
        let mut m = Vec::new();
        for s in [name, "display"] {
            m.extend_from_slice(&(s.len() as u32).to_le_bytes());
            m.extend_from_slice(s.as_bytes());
        }
        m
    }

    /// Commits `v<n>` with one blob `0` holding `data`.
    fn commit(container: &Path, n: u64, data: &str) -> PathBuf {
        let v = container.join(format!("v{n}"));
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(v.join("0"), data).unwrap();
        std::fs::write(v.join(MANIFEST), manifest("Slot")).unwrap();
        v
    }

    fn blob(container: &Path, n: u64) -> String {
        std::fs::read_to_string(container.join(format!("v{n}/0"))).unwrap()
    }

    #[test]
    fn keeps_the_last_ten_and_lists_newest_first() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("Slot~00");
        let mut made = Vec::new();
        for n in 1..=14 {
            let v = commit(&c, n, &format!("save {n}"));
            made.push(snapshot(&c, &v, n, KEEP).unwrap());
        }
        let backups = list(&dir_of(&c).unwrap());
        assert_eq!(backups.len(), KEEP);
        assert_eq!(backups[0].version, 14);
        assert_eq!(backups[9].version, 5);
        assert!(backups.windows(2).all(|w| w[0].unix_ms > w[1].unix_ms));
        assert!(!made[0].exists() && made[13].exists());
        // The data is whole, and survives the live version being deleted.
        std::fs::remove_dir_all(c.join("v14")).unwrap();
        assert_eq!(std::fs::read_to_string(backups[0].path.join("0")).unwrap(), "save 14");
        assert!(backups[0].path.join(MANIFEST).is_file());
        // No staging folders are left.
        let leftovers = std::fs::read_dir(dir_of(&c).unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .count();
        assert_eq!(leftovers, 0);
    }

    #[test]
    fn a_crash_while_staging_a_backup_leaves_no_backup_and_is_cleaned_up() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("Slot~00");
        let v = commit(&c, 1, "one");
        snapshot(&c, &v, 1, KEEP).unwrap();
        // A killed snapshot leaves its staging folder, half filled.
        let dir = dir_of(&c).unwrap();
        std::fs::create_dir_all(dir.join(".tmp-9999999999999-v2")).unwrap();
        std::fs::write(dir.join(".tmp-9999999999999-v2/0"), "ha").unwrap();
        assert_eq!(list(&dir).len(), 1, "the staging folder is no backup");
        let v2 = commit(&c, 2, "two");
        snapshot(&c, &v2, 2, KEEP).unwrap();
        assert_eq!(list(&dir).len(), 2);
        assert!(!dir.join(".tmp-9999999999999-v2").exists());
    }

    #[test]
    fn ids_keep_increasing_when_the_clock_steps_back() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("Slot~00");
        let dir = dir_of(&c).unwrap();
        std::fs::create_dir_all(dir.join("4102444800000-v1")).unwrap();
        std::fs::write(dir.join("4102444800000-v1/manifest"), manifest("Slot")).unwrap();
        let v = commit(&c, 2, "two");
        snapshot(&c, &v, 2, KEEP).unwrap();
        assert_eq!(list(&dir)[0].version, 2, "the new backup sorts newest");
    }

    #[test]
    fn restore_brings_back_an_older_version_and_can_be_undone() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("Slot~00");
        for n in 1..=3 {
            let v = commit(&c, n, &format!("save {n}"));
            snapshot(&c, &v, n, KEEP).unwrap();
        }
        // The game then wrote garbage as v4 (the live copy is "lost").
        commit(&c, 4, "garbage");
        let dir = dir_of(&c).unwrap();
        let backups = list(&dir);
        let good = backups.iter().find(|b| b.version == 3).unwrap().clone();
        let n = restore(&c, &good, KEEP).unwrap();
        assert_eq!(n, 5);
        assert_eq!(blob(&c, 5), "save 3");
        assert!(c.join("v5").join(MANIFEST).is_file());
        // The garbage was kept as a backup first: the restore is reversible.
        let after = list(&dir);
        let kept = after.iter().find(|b| b.version == 4).expect("pre-restore backup");
        assert_eq!(std::fs::read_to_string(kept.path.join("0")).unwrap(), "garbage");
        assert!(!c.join("v6.restore.new").exists() && !c.join("v5.restore.new").exists());
        let n = restore(&c, kept, KEEP).unwrap();
        assert_eq!(blob(&c, n), "garbage");
    }

    #[test]
    fn restore_refuses_a_backup_without_a_manifest() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("Slot~00");
        commit(&c, 1, "one");
        let bad = t.path().join("bad");
        std::fs::create_dir_all(&bad).unwrap();
        let b = Backup { id: "x".into(), path: bad, version: 1, unix_ms: 0, bytes: 0 };
        assert!(restore(&c, &b, KEEP).is_err());
        assert_eq!(blob(&c, 1), "one");
    }

    #[test]
    fn inspect_finds_live_and_deleted_containers() {
        let t = tempfile::tempdir().unwrap();
        let root = t.path().join("0009000000000001/cfg");
        let live = root.join("Slot~aa");
        let v = commit(&live, 3, "abc");
        snapshot(&live, &v, 3, KEEP).unwrap();
        // A deleted container: only its backup is left.
        let gone = root.join("Gone~bb");
        let g = commit(&gone, 1, "x");
        std::fs::write(g.join(MANIFEST), manifest("Gone")).unwrap();
        snapshot(&gone, &g, 1, KEEP).unwrap();
        std::fs::remove_dir_all(&gone).unwrap();
        // A user folder with nothing in it.
        std::fs::create_dir_all(t.path().join("0009FAB03B7EB9C2/cfg")).unwrap();

        let all = inspect(t.path());
        assert_eq!(all.len(), 2);
        let used = all.iter().find(|p| p.user == "0009000000000001").unwrap();
        let names: Vec<_> = used.containers.iter().map(|c| (c.name.as_str(), c.version)).collect();
        assert_eq!(names, [("Gone", None), ("Slot", Some(3))]);
        assert_eq!(used.containers[1].bytes, 3 + manifest("Slot").len() as u64);
        assert!(all.iter().find(|p| p.user == "0009FAB03B7EB9C2").unwrap().containers.is_empty());
    }
}
