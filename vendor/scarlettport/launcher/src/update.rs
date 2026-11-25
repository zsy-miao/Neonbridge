//! Updates from GitHub Releases (public API, no token). On request the release ZIP is downloaded
//! into `update\` in the launcher's own folder, checked against the release's SHA256SUMS.txt and
//! extracted to `update\staging`. Nothing from it runs until the next start, which swaps the
//! staged files in (each a rename on the same drive) and keeps the replaced ones in `previous\`
//! for a rollback. Nothing is written to %TEMP%.
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

const API: &str = "https://api.github.com/repos/ZigZagBoi1/ScarlettPort/releases?per_page=10";
const AGENT: &str = concat!("ScarlettPort-launcher/", env!("CARGO_PKG_VERSION"));
/// What the swap moves: the launcher, its runtime folder and the ZIP's text files.
const ITEMS: [&str; 5] = ["ScarlettPort.exe", "runtime", "README.txt", "LICENSE.md", "SHA256SUMS.txt"];

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub zip_name: String,
    pub zip_url: String,
    pub sums_url: String,
    pub page: String,
}

/// `v0.10.2` -> (0, 10, 2).
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let v = v.trim().trim_start_matches('v');
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|p| p.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

pub fn newer(remote: &str, local: &str) -> bool {
    matches!((parse_version(remote), parse_version(local)), (Some(r), Some(l)) if r > l)
}

/// The newest release in the API's JSON that has a launcher ZIP and a checksums file.
pub fn pick(json: &serde_json::Value) -> Option<Release> {
    let mut best: Option<Release> = None;
    for r in json.as_array()? {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let tag = r["tag_name"].as_str().unwrap_or_default();
        if !tag.starts_with('v') || parse_version(tag).is_none() {
            continue;
        }
        let assets = r["assets"].as_array()?;
        let find = |pred: &dyn Fn(&str) -> bool| {
            assets.iter().find(|a| pred(a["name"].as_str().unwrap_or_default())).map(|a| {
                (a["name"].as_str().unwrap_or_default().to_string(), a["browser_download_url"].as_str().unwrap_or_default().to_string())
            })
        };
        let (Some((zip_name, zip_url)), Some((_, sums_url))) = (find(&|n| n.ends_with("-win64.zip")), find(&|n| n == "SHA256SUMS.txt")) else { continue };
        let cand = Release { version: tag.to_string(), zip_name, zip_url, sums_url, page: r["html_url"].as_str().unwrap_or_default().to_string() };
        if best.as_ref().is_none_or(|b| newer(&cand.version, &b.version)) {
            best = Some(cand);
        }
    }
    best
}

fn get(url: &str) -> Result<ureq::Response, String> {
    ureq::get(url).set("User-Agent", AGENT).set("Accept", "application/vnd.github+json").timeout(std::time::Duration::from_secs(30)).call().map_err(|e| e.to_string())
}

/// Some(release) when one newer than this launcher is published.
pub fn check() -> Result<Option<Release>, String> {
    let json: serde_json::Value = get(API)?.into_json().map_err(|e| e.to_string())?;
    Ok(pick(&json).filter(|r| newer(&r.version, env!("CARGO_PKG_VERSION"))))
}

pub fn base_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

/// The expected SHA-256 of `name` in a SHA256SUMS.txt (`<hex>  <name>` lines).
pub fn expected_hash(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let (h, n) = l.trim().split_once(char::is_whitespace)?;
        (n.trim().trim_start_matches('*') == name && h.len() == 64).then(|| h.to_ascii_lowercase())
    })
}

/// Downloads, verifies and stages `r`. `progress` gets (bytes, total).
pub fn download(r: &Release, progress: &dyn Fn(u64, u64)) -> Result<(), String> {
    let base = base_dir().ok_or("the launcher's folder is unknown")?;
    let dir = base.join("update");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e} (is the launcher's folder writable?)", dir.display()))?;
    let mut sums = String::new();
    get(&r.sums_url)?.into_reader().take(1 << 20).read_to_string(&mut sums).map_err(|e| e.to_string())?;
    let want = expected_hash(&sums, &r.zip_name).ok_or("the release's SHA256SUMS.txt does not list its ZIP")?;
    let resp = get(&r.zip_url)?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    let zip_path = dir.join(&r.zip_name);
    let mut out = std::fs::File::create(&zip_path).map_err(|e| e.to_string())?;
    let mut reader = resp.into_reader().take(2 << 30);
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut got = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        progress(got, total);
    }
    drop(out);
    let have: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if have != want {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(format!("checksum mismatch for {} (expected {want}, got {have}); nothing was changed", r.zip_name));
    }
    let staging = dir.join("staging");
    let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = entry.enclosed_name() else { continue };
        let to = staging.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&to).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(p) = to.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        let mut f = std::fs::File::create(&to).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut f).map_err(|e| e.to_string())?;
    }
    let _ = std::fs::remove_file(&zip_path);
    if !staging.join("ScarlettPort.exe").is_file() {
        let _ = std::fs::remove_dir_all(&dir);
        return Err("the release ZIP has no ScarlettPort.exe".into());
    }
    std::fs::write(dir.join("ready.txt"), &r.version).map_err(|e| e.to_string())?;
    Ok(())
}

/// The version staged for the next start, if any.
pub fn staged() -> Option<String> {
    std::fs::read_to_string(base_dir()?.join("update").join("ready.txt")).ok().map(|s| s.trim().to_string())
}

/// Moves `ITEMS` from `from` to `to` (each a rename), undoing the moves made if one fails.
fn move_items(from: &Path, to: &Path) -> Result<(), String> {
    let mut moved = Vec::new();
    for item in ITEMS {
        let (a, b) = (from.join(item), to.join(item));
        if !a.exists() {
            continue;
        }
        if let Err(e) = std::fs::rename(&a, &b) {
            for (a, b) in moved.into_iter().rev() {
                let _ = std::fs::rename(b, a);
            }
            return Err(format!("{}: {e}", a.display()));
        }
        moved.push((a, b));
    }
    Ok(())
}

/// The swap: current files to `previous\`, staged files into place. A running exe can be
/// renamed on Windows, so this works from the launcher being replaced.
fn swap(base: &Path, incoming: &Path) -> Result<(), String> {
    let previous = base.join("previous");
    let _ = std::fs::remove_dir_all(&previous);
    std::fs::create_dir_all(&previous).map_err(|e| e.to_string())?;
    move_items(base, &previous)?;
    if let Err(e) = move_items(incoming, base) {
        let _ = move_items(&previous, base);
        return Err(e);
    }
    Ok(())
}

/// Run first thing at start: applies a staged update (or a requested rollback). Returns the
/// exe to start instead of continuing, when files were swapped.
pub fn apply_pending() -> Option<PathBuf> {
    let base = base_dir()?;
    let upd = base.join("update");
    let rollback = upd.join("rollback.txt");
    let result = if rollback.is_file() {
        let _ = std::fs::remove_file(&rollback);
        let prev = base.join("previous");
        if !prev.join("ScarlettPort.exe").is_file() {
            return None;
        }
        // previous <-> current through a scratch folder in update\.
        let scratch = upd.join("swap");
        let _ = std::fs::remove_dir_all(&scratch);
        std::fs::create_dir_all(&scratch).ok()?;
        move_items(&prev, &scratch).and_then(|_| swap(&base, &scratch))
    } else if upd.join("ready.txt").is_file() {
        let _ = std::fs::remove_file(upd.join("ready.txt"));
        swap(&base, &upd.join("staging"))
    } else {
        return None;
    };
    let _ = std::fs::remove_dir_all(&upd);
    result.ok().map(|_| base.join("ScarlettPort.exe"))
}

/// Asks the next start to swap `previous\` back in.
pub fn request_rollback() -> Result<(), String> {
    let base = base_dir().ok_or("the launcher's folder is unknown")?;
    if !base.join("previous").join("ScarlettPort.exe").is_file() {
        return Err("No previous version is kept.".into());
    }
    std::fs::create_dir_all(base.join("update")).map_err(|e| e.to_string())?;
    std::fs::write(base.join("update").join("rollback.txt"), "rollback").map_err(|e| e.to_string())
}

pub fn has_previous() -> bool {
    base_dir().is_some_and(|b| b.join("previous").join("ScarlettPort.exe").is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("v0.10.0", "0.2.0"));
        assert!(!newer("v0.2.0", "0.2.0"));
        assert!(!newer("launcher-v0.1.0", "0.0.1"));
        assert_eq!(parse_version("v1.2.3-rc1"), Some((1, 2, 3)));
    }

    #[test]
    fn picks_newest_release_with_assets() {
        let j = serde_json::json!([
            { "tag_name": "v0.2.0", "draft": false, "html_url": "p2", "assets": [
                { "name": "ScarlettPort-v0.2.0-win64.zip", "browser_download_url": "z2" },
                { "name": "SHA256SUMS.txt", "browser_download_url": "s2" } ] },
            { "tag_name": "v0.3.0", "draft": false, "html_url": "p3", "assets": [
                { "name": "ScarlettPort-v0.3.0-win64.zip", "browser_download_url": "z3" },
                { "name": "SHA256SUMS.txt", "browser_download_url": "s3" } ] },
            { "tag_name": "v0.4.0", "draft": true, "assets": [] },
            { "tag_name": "launcher-v0.1.0", "draft": false, "assets": [] }
        ]);
        let r = pick(&j).unwrap();
        assert_eq!((r.version.as_str(), r.zip_url.as_str(), r.sums_url.as_str()), ("v0.3.0", "z3", "s3"));
    }

    #[test]
    fn reads_checksums() {
        let h = "a".repeat(64);
        let sums = format!("{h}  ScarlettPort-v0.3.0-win64.zip\n{}  ScarlettPort.exe\n", "b".repeat(64));
        assert_eq!(expected_hash(&sums, "ScarlettPort-v0.3.0-win64.zip"), Some(h));
        assert_eq!(expected_hash(&sums, "other.zip"), None);
    }

    #[test]
    fn swap_keeps_previous_and_rolls_back_on_failure() {
        let d = std::env::temp_dir().join(format!("sp-upd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let inc = d.join("in");
        std::fs::create_dir_all(inc.join("runtime")).unwrap();
        std::fs::write(d.join("ScarlettPort.exe"), "old").unwrap();
        std::fs::write(inc.join("ScarlettPort.exe"), "new").unwrap();
        swap(&d, &inc).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("ScarlettPort.exe")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(d.join("previous").join("ScarlettPort.exe")).unwrap(), "old");
        assert!(d.join("runtime").is_dir());
        let _ = std::fs::remove_dir_all(&d);
    }
}
