//! "Verify game files": checks a dump folder for completeness without changing anything.
//! Every expected top-level archive, a complete `pack\` (the title's nine packs, and the
//! entries the dump's `dump-manifest.log` lists under it), symbolic links that resolve, and no
//! empty files. A `pack\` that is missing or split across drives is called out on its own: the
//! title crashes at boot without it. Reads metadata only; never moves, writes or deletes.
use std::path::{Path, PathBuf};

/// The title's top-level files (archives, the executable and its config).
pub const TOP: [&str; 14] = [
    "game_scarlett_master.exe",
    "MicrosoftGame.config",
    "anim.rpf",
    "appdata0.rpf",
    "audio.rpf",
    "common.rpf",
    "data.rpf",
    "levels.rpf",
    "movies.rpf",
    "navmeshes.rpf",
    "packs.rpf",
    "script.rpf",
    "shaders.rpf",
    "textures.rpf",
];

/// The packs under `pack\`, each a folder with at least one archive named after it.
pub const PACKS: [&str; 9] = [
    "core_bvh4",
    "core_licenses",
    "core_restofworld",
    "core_restofworld001",
    "shared_minigame000",
    "shared_minigame000_bvh4",
    "story000",
    "story000_bvh4",
    "story000_restofworld",
];

pub const MANIFEST: &str = "dump-manifest.log";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Ok,
    Warn,
    Bad,
}

#[derive(Clone, Debug)]
pub struct Finding {
    pub level: Level,
    pub title: String,
    pub detail: String,
    /// What "Show me" opens in Explorer.
    pub path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    /// `pack\` is missing, incomplete or partly elsewhere.
    pub pack_problem: bool,
    pub files: u64,
    pub bytes: u64,
}

impl Report {
    pub fn worst(&self) -> Level {
        if self.findings.iter().any(|f| f.level == Level::Bad) {
            Level::Bad
        } else if self.findings.iter().any(|f| f.level == Level::Warn) {
            Level::Warn
        } else {
            Level::Ok
        }
    }
    pub fn problems(&self) -> usize {
        self.findings.iter().filter(|f| f.level != Level::Ok).count()
    }
}

/// One manifest line: `SHA-256 | SIZE | MODIFIED | REL_PATH`.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub size: u64,
    pub path: String,
}

/// The entries and the "Total entries" the header states (a manifest may list only some).
pub fn parse_manifest(text: &str) -> (Vec<Entry>, Option<u64>) {
    let mut total = None;
    let mut out = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix('#') {
            if let Some(i) = rest.find("Total entries:") {
                let n: String = rest[i + 14..].trim_start().chars().take_while(|c| c.is_ascii_digit() || *c == ',').filter(|c| *c != ',').collect();
                total = n.parse().ok();
            }
            continue;
        }
        let parts: Vec<&str> = t.split('|').map(str::trim).collect();
        if parts.len() == 4 {
            if let Ok(size) = parts[1].parse() {
                out.push(Entry { size, path: parts[3].replace('/', "\\") });
            }
        }
    }
    (out, total)
}

/// Size of `p` following links; Err says why it cannot be read.
fn resolved_len(p: &Path) -> Result<u64, String> {
    std::fs::metadata(p).map(|m| if m.is_dir() { 0 } else { m.len() }).map_err(|e| e.to_string())
}

fn is_link(p: &Path) -> bool {
    std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}

/// Walks `dir` (following links), counting files and noting empty files and broken links.
fn walk(dir: &Path, rep: &mut Report, empty: &mut Vec<PathBuf>, broken: &mut Vec<(PathBuf, String)>, depth: u32) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if is_link(&p) {
            if let Err(err) = std::fs::metadata(&p) {
                broken.push((p, err.to_string()));
                continue;
            }
        }
        match std::fs::metadata(&p) {
            Ok(m) if m.is_dir() => {
                if depth < 12 {
                    walk(&p, rep, empty, broken, depth + 1);
                }
            }
            Ok(m) => {
                rep.files += 1;
                rep.bytes += m.len();
                if m.len() == 0 {
                    empty.push(p);
                }
            }
            Err(err) => broken.push((p, err.to_string())),
        }
    }
}

/// Folders holding the targets of the dump's top-level links (where a split copy may live).
fn link_homes(dir: &Path) -> Vec<PathBuf> {
    let mut homes: Vec<PathBuf> = Vec::new();
    for n in TOP {
        let p = dir.join(n);
        if is_link(&p) {
            if let Some(h) = std::fs::read_link(&p).ok().and_then(|t| t.parent().map(Path::to_path_buf)) {
                if !homes.contains(&h) {
                    homes.push(h);
                }
            }
        }
    }
    homes
}

pub fn run(dir: &Path) -> Report {
    let mut rep = Report::default();
    let push = |rep: &mut Report, level, title: String, detail: String, path: Option<PathBuf>| rep.findings.push(Finding { level, title, detail, path });

    // ---- top level
    let mut missing = Vec::new();
    let mut top_bad = Vec::new();
    for n in TOP {
        let p = dir.join(n);
        let link = is_link(&p);
        match resolved_len(&p) {
            Ok(0) => top_bad.push((n, "is empty (0 bytes)".to_string())),
            Ok(_) => {}
            Err(e) if link => top_bad.push((n, format!("is a link that does not resolve ({e})"))),
            Err(_) => missing.push(n),
        }
    }
    if missing.is_empty() && top_bad.is_empty() {
        push(&mut rep, Level::Ok, format!("All {} top-level files present", TOP.len()), TOP[2..].join(", "), Some(dir.to_path_buf()));
    }
    if !missing.is_empty() {
        push(&mut rep, Level::Bad, format!("Missing {} top-level file(s)", missing.len()), missing.join(", "), Some(dir.to_path_buf()));
    }
    for (n, why) in top_bad {
        push(&mut rep, Level::Bad, format!("{n} {why}"), "Re-copy it from the console dump, or fix the link.".into(), Some(dir.join(n)));
    }

    // ---- pack\
    let pack = dir.join("pack");
    let pack_ok = std::fs::metadata(&pack).is_ok_and(|m| m.is_dir());
    let elsewhere: Vec<PathBuf> = link_homes(dir).into_iter().map(|h| h.join("pack")).filter(|p| p.is_dir() && *p != pack).collect();
    if !pack_ok {
        rep.pack_problem = true;
        let detail = match elsewhere.first() {
            Some(e) => format!("There is no pack\\ here, but one exists at {}. The game crashes at boot without pack\\ in the game folder: copy (or link) it here yourself.", e.display()),
            None => "The game crashes at boot without it. Copy pack\\ from the console dump into the game folder.".into(),
        };
        push(&mut rep, Level::Bad, "pack\\ is missing".into(), detail, Some(elsewhere.first().cloned().unwrap_or_else(|| dir.to_path_buf())));
    } else {
        let mut absent = Vec::new();
        for n in PACKS {
            let d = pack.join(n);
            let has_archive = std::fs::read_dir(&d).is_ok_and(|rd| {
                rd.flatten().any(|e| {
                    let f = e.file_name().to_string_lossy().to_lowercase();
                    f.starts_with(n) && f.ends_with(".rpf") && resolved_len(&e.path()).is_ok_and(|l| l > 0)
                })
            });
            if !has_archive {
                absent.push(n);
            }
        }
        let extra: Vec<&PathBuf> = elsewhere.iter().collect();
        if absent.is_empty() {
            push(&mut rep, Level::Ok, format!("pack\\ complete: all {} packs", PACKS.len()), PACKS.join(", "), Some(pack.clone()));
            if let Some(e) = extra.first() {
                push(&mut rep, Level::Warn, "A second pack\\ exists elsewhere".into(), format!("{} also has a pack\\ folder. The game reads only the one in the game folder; the other is unused.", e.display()), Some((*e).clone()));
            }
        } else {
            rep.pack_problem = true;
            let split = extra.first().map(|e| format!(" Part of it is at {}: the pack\\ is split across folders.", e.display())).unwrap_or_default();
            push(
                &mut rep,
                Level::Bad,
                format!("pack\\ incomplete: {} of {} packs missing", absent.len(), PACKS.len()),
                format!("Missing {}.{split} The game crashes at boot until pack\\ is whole in the game folder.", absent.join(", ")),
                Some(extra.first().map(|e| (*e).clone()).unwrap_or_else(|| pack.clone())),
            );
        }
    }

    // ---- every file: empty files and broken links (the port's own _native_port folder is not game data)
    let mut empty = Vec::new();
    let mut broken = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name == "_native_port" {
                continue;
            }
            match std::fs::metadata(&p) {
                Ok(m) if m.is_dir() => walk(&p, &mut rep, &mut empty, &mut broken, 0),
                Ok(m) => {
                    rep.files += 1;
                    rep.bytes += m.len();
                    if m.len() == 0 && !TOP.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
                        empty.push(p);
                    }
                }
                Err(err) if !TOP.iter().any(|t| t.eq_ignore_ascii_case(&name)) => broken.push((p, err.to_string())),
                Err(_) => {}
            }
        }
    }
    if broken.is_empty() {
        push(&mut rep, Level::Ok, "Links resolve".into(), "Every symbolic link points at a file that exists.".into(), None);
    } else {
        let list: Vec<String> = broken.iter().take(6).map(|(p, _)| rel(dir, p)).collect();
        push(&mut rep, Level::Bad, format!("{} link(s) do not resolve", broken.len()), list.join(", "), broken.first().map(|(p, _)| p.clone()));
    }
    if empty.is_empty() {
        let d = format!("{} files, {}", rep.files, crate::game::human(rep.bytes));
        push(&mut rep, Level::Ok, "No empty files".into(), d, None);
    } else {
        let list: Vec<String> = empty.iter().take(6).map(|p| rel(dir, p)).collect();
        push(&mut rep, Level::Warn, format!("{} empty file(s)", empty.len()), list.join(", "), empty.first().cloned());
    }

    // ---- the dump's manifest
    let mp = dir.join(MANIFEST);
    match std::fs::read_to_string(&mp) {
        Err(_) => push(&mut rep, Level::Warn, "No dump-manifest.log".into(), "The dump has no manifest to compare against; the checks above still apply.".into(), Some(dir.to_path_buf())),
        Ok(text) => {
            let (entries, total) = parse_manifest(&text);
            let mut gone = Vec::new();
            let mut sized = Vec::new();
            for e in &entries {
                match resolved_len(&dir.join(&e.path)) {
                    Err(_) => gone.push(e.path.clone()),
                    Ok(l) if l != e.size => sized.push(format!("{} ({} here, {} listed)", e.path, crate::game::human(l), crate::game::human(e.size))),
                    Ok(_) => {}
                }
            }
            let pack_listed = entries.iter().filter(|e| e.path.to_lowercase().starts_with("pack\\")).count();
            let partial = total.is_some_and(|t| t > entries.len() as u64);
            let scope = match total {
                Some(t) if partial => format!("lists {} of its {t} entries in this file", entries.len()),
                _ => format!("{} entries", entries.len()),
            };
            if gone.is_empty() && sized.is_empty() {
                push(&mut rep, Level::Ok, "Matches dump-manifest.log".into(), format!("The manifest {scope}; all of them match. {pack_listed} are under pack\\."), Some(mp.clone()));
            } else {
                let mut d = format!("The manifest {scope}. ");
                if !gone.is_empty() {
                    d += &format!("Not in the folder: {}. ", gone.iter().take(4).cloned().collect::<Vec<_>>().join(", "));
                }
                if !sized.is_empty() {
                    d += &format!("Different size: {}.", sized.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
                }
                if gone.iter().any(|g| g.to_lowercase().starts_with("pack\\")) {
                    rep.pack_problem = true;
                }
                push(&mut rep, Level::Warn, format!("{} manifest entries do not match", gone.len() + sized.len()), d, Some(mp.clone()));
            }
            if pack_listed == 0 && entries.len() > 0 {
                push(&mut rep, Level::Warn, "The manifest lists nothing under pack\\".into(), "pack\\ could only be checked against the title's known nine packs.".into(), Some(mp));
            }
        }
    }
    rep
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base).map(|r| r.display().to_string()).unwrap_or_else(|_| p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sp-verify-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn full_dump(d: &Path) {
        for n in TOP {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        for n in PACKS {
            std::fs::create_dir_all(d.join("pack").join(n)).unwrap();
            std::fs::write(d.join("pack").join(n).join(format!("{n}.rpf")), b"x").unwrap();
        }
    }

    #[test]
    fn manifest_parse() {
        let t = "# Total entries: 24,484  |  Total size: 1 bytes\n#\nabc | 12 | 2026 | pack/story000/story000.rpf\nnot a line\n";
        let (e, total) = parse_manifest(t);
        assert_eq!(total, Some(24484));
        assert_eq!(e, vec![Entry { size: 12, path: "pack\\story000\\story000.rpf".into() }]);
    }

    #[test]
    fn complete_dump_passes() {
        let d = tmp("ok");
        full_dump(&d);
        std::fs::write(d.join(MANIFEST), "a | 1 | t | pack\\core_bvh4\\core_bvh4.rpf\n").unwrap();
        let r = run(&d);
        assert_eq!(r.worst(), Level::Ok, "{:?}", r.findings);
        assert!(!r.pack_problem);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn flags_missing_pack_and_archive() {
        let d = tmp("nopack");
        full_dump(&d);
        std::fs::remove_dir_all(d.join("pack").join("story000")).unwrap();
        std::fs::remove_file(d.join("levels.rpf")).unwrap();
        std::fs::write(d.join("empty.bin"), b"").unwrap();
        let r = run(&d);
        assert!(r.pack_problem);
        assert_eq!(r.worst(), Level::Bad);
        assert!(r.findings.iter().any(|f| f.title.contains("pack\\ incomplete") && f.detail.contains("story000")));
        assert!(r.findings.iter().any(|f| f.title.contains("top-level") && f.detail.contains("levels.rpf")));
        assert!(r.findings.iter().any(|f| f.title.contains("empty file")));
        std::fs::remove_dir_all(d.join("pack")).unwrap();
        assert!(run(&d).findings.iter().any(|f| f.title == "pack\\ is missing"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn never_writes() {
        let d = tmp("ro");
        full_dump(&d);
        let before: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        run(&d);
        let after: Vec<_> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(d);
    }
}
