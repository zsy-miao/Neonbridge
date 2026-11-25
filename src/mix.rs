//! Mix mode: one victory build made of crates from different victory-family
//! trees (victory, scarlettport's runtime/victory, vitoriabr, ztrx-victory
//! share the crate names). A mix names the tree of the app (apps/victory,
//! and with it the workspace's Cargo.toml, Cargo.lock, toolchain and the
//! runtime's NEONBRIDGE.toml: run line, environment, warm step) and the tree
//! of each crate.
//!
//! The build workspace is %LOCALAPPDATA%\6ixpack\mix\<hash of the mix>: each
//! crate copied from its tree with that tree's rt-<tree>.patch applied to it,
//! the app tree's workspace files, and the [workspace.dependencies] the other
//! trees add. It builds once (the app runtime's build line, without
//! --manifest-path or --locked) and its victory.exe is kept; a mix that does
//! not compile is remembered as incompatible (incompatible.txt) with its
//! errors. Named mixes live in mix\saved\<name>.txt, the last one that built
//! and started in mix\last-good.txt.
//!
//! vendor/mix-compat.toml (Coder5's matrix) lists in [incompatible] the
//! "<part>@<tree> + <part>@<tree>" pairs known not to compile together: the
//! picker marks those choices and a mix holding one is refused before it
//! builds (NEONBRIDGE_MIX_COMPAT=0: build it anyway). A pair not listed is
//! unverified, not known good.

use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

/// The app: apps/victory, the exe, and the workspace files with it.
pub const APP: &str = "app";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mix {
    /// "app" and each crate name -> the runtime whose tree supplies it.
    pub parts: BTreeMap<String, String>,
}

impl Mix {
    pub fn app(&self) -> Option<&str> {
        self.parts.get(APP).map(String::as_str)
    }

    /// `name=runtime` lines (the saved form).
    pub fn parse(text: &str) -> Mix {
        let parts = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
            .filter(|(k, v)| !k.is_empty() && !v.is_empty())
            .collect();
        Mix { parts }
    }

    pub fn text(&self) -> String {
        self.parts.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
    }

    /// Every part from `base` unless named.
    pub fn filled(&self, base: &str) -> Mix {
        let mut parts = self.parts.clone();
        parts.entry(APP.into()).or_insert_with(|| base.to_owned());
        for c in crates() {
            parts.entry(c).or_insert_with(|| base.to_owned());
        }
        Mix { parts }
    }
}

/// The runtimes a mix can take parts from: those with a victory tree.
pub fn trees() -> Vec<String> {
    crate::runtimes::names().into_iter().filter(|n| crate::runtimes::victory_tree(n).is_some_and(|t| t.join("apps/victory").is_dir())).collect()
}

/// The crate names any victory tree has (crates/<name>).
pub fn crates() -> Vec<String> {
    let mut all: Vec<String> = trees()
        .iter()
        .filter_map(|t| crate::runtimes::victory_tree(t))
        .flat_map(|t| fs::read_dir(t.join("crates")).into_iter().flatten().flatten())
        .filter(|e| e.path().join("Cargo.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    all.sort();
    all.dedup();
    all
}

/// Whether runtime `tree` has `part` (the app, or crates/<part>).
pub fn has(tree: &str, part: &str) -> bool {
    crate::runtimes::victory_tree(tree).is_some_and(|t| t.join(src_of(part)).join("Cargo.toml").is_file())
}

fn src_of(part: &str) -> PathBuf {
    if part == APP { PathBuf::from("apps/victory") } else { Path::new("crates").join(part) }
}

pub fn home() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack").join("mix")
}

fn saved_dir() -> PathBuf {
    home().join("saved")
}

pub fn saved_names() -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(saved_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_string_lossy().strip_suffix(".txt").map(str::to_owned))
        .collect();
    v.sort();
    v
}

/// A saved mix by name, or "last-good".
pub fn load(name: &str) -> Option<Mix> {
    let file = if name == "last-good" { home().join("last-good.txt") } else { saved_dir().join(format!("{name}.txt")) };
    fs::read_to_string(file).ok().map(|t| Mix::parse(&t)).filter(|m| !m.parts.is_empty())
}

pub fn save(name: &str, mix: &Mix) -> std::io::Result<()> {
    let name: String = name.chars().filter(|c| c.is_alphanumeric() || "-_ .".contains(*c)).collect();
    fs::create_dir_all(saved_dir())?;
    fs::write(saved_dir().join(format!("{}.txt", name.trim())), mix.text())
}

pub fn remember_good(mix: &Mix) {
    let _ = fs::create_dir_all(home());
    let _ = fs::write(home().join("last-good.txt"), mix.text());
}

/// FNV-1a of the mix and the patches it applies: the build folder's name.
fn hash(mix: &Mix) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h = (h ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(mix.text().as_bytes());
    let mut used: Vec<&String> = mix.parts.values().collect();
    used.sort();
    used.dedup();
    for t in used {
        eat(&fs::read(crate::runtimes::patch_file(t)).unwrap_or_default());
    }
    format!("{h:016x}")
}

/// The folder a mix builds in.
pub fn dir(mix: &Mix) -> PathBuf {
    home().join(hash(mix))
}

/// The errors a mix failed to compile with, if it is known to.
pub fn incompatible(mix: &Mix) -> Option<String> {
    fs::read_to_string(dir(mix).join("incompatible.txt")).ok()
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let name = e.file_name();
        if name == "target" || name == ".git" {
            continue;
        }
        let (src, dst) = (e.path(), to.join(&name));
        if e.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// The `name = ...` lines of a Cargo.toml's [workspace.dependencies].
fn workspace_deps(text: &str) -> Vec<(String, String)> {
    let mut on = false;
    let mut out = Vec::new();
    for l in text.lines() {
        let t = l.trim();
        if t.starts_with('[') {
            on = t == "[workspace.dependencies]";
            continue;
        }
        if on && let Some((k, _)) = t.split_once('=') && !t.starts_with('#') {
            out.push((k.trim().to_owned(), l.to_owned()));
        }
    }
    out
}

/// The app tree's Cargo.toml with the [workspace.dependencies] that the other
/// chosen trees add (an external crate only one fork uses, say).
fn merged_manifest(app: &str, others: &[PathBuf]) -> String {
    let have: Vec<String> = workspace_deps(app).into_iter().map(|(k, _)| k).collect();
    let mut extra: Vec<String> = Vec::new();
    for o in others {
        for (k, line) in workspace_deps(&fs::read_to_string(o.join("Cargo.toml")).unwrap_or_default()) {
            if !have.contains(&k) && !extra.iter().any(|e| e.trim_start().starts_with(&format!("{k} "))) {
                extra.push(line);
            }
        }
    }
    if extra.is_empty() {
        return app.to_owned();
    }
    app.replacen("[workspace.dependencies]", &format!("[workspace.dependencies]\n# Neonbridge mix: from the other trees\n{}", extra.join("\n")), 1)
}

type Pair = ((String, String), (String, String), String);

/// vendor/mix-compat.toml's [incompatible] pairs: (part, tree) twice, why.
fn known_bad() -> &'static [Pair] {
    static BAD: std::sync::OnceLock<Vec<Pair>> = std::sync::OnceLock::new();
    BAD.get_or_init(|| {
        let text = fs::read_to_string(crate::runtimes::vendor().join("mix-compat.toml")).unwrap_or_default();
        let side = |s: &str| s.trim().trim_matches('"').split_once('@').map(|(p, t)| (p.trim().to_owned(), t.trim().to_owned()));
        let mut out = Vec::new();
        let mut on = false;
        for l in text.lines().map(str::trim) {
            if l.starts_with('[') {
                on = l == "[incompatible]";
                continue;
            }
            if !on || l.starts_with('#') {
                continue;
            }
            let Some((k, why)) = l.split_once("\" = ") else { continue };
            let why = why.trim().trim_matches('"').replace("\\\\", "\\").replace("\\\"", "\"");
            if let Some((a, b)) = k.split_once(" + ") {
                if let (Some(a), Some(b)) = (side(a), side(b)) {
                    out.push((a, b, why));
                }
            }
        }
        out
    })
}

/// Why `part` from `tree` cannot go with the mix's other picks (the first
/// listed clash), if a listed pair says so.
pub fn clash(mix: &Mix, part: &str, tree: &str) -> Option<String> {
    let picked = |p: &(String, String)| mix.parts.get(&p.0).is_some_and(|t| *t == p.1);
    known_bad().iter().find_map(|(a, b, why)| {
        let other = if a.0 == part && a.1 == tree {
            b
        } else if b.0 == part && b.1 == tree {
            a
        } else {
            return None;
        };
        picked(other).then(|| format!("{part}@{tree} + {}@{}: {why}", other.0, other.1))
    })
}

/// Every listed clash in the mix.
pub fn conflicts(mix: &Mix) -> Vec<String> {
    let picked = |p: &(String, String)| mix.parts.get(&p.0).is_some_and(|t| *t == p.1);
    known_bad().iter().filter(|(a, b, _)| picked(a) && picked(b)).map(|(a, b, why)| format!("{}@{} + {}@{}: {why}", a.0, a.1, b.0, b.1)).collect()
}

/// Lays out and builds the mix (once), returning its victory.exe.
pub fn build(mix: &Mix) -> Result<PathBuf, Box<dyn Error>> {
    let app = mix.app().ok_or("a mix names no app tree")?.to_owned();
    for (part, tree) in &mix.parts {
        if !has(tree, part) {
            return Err(format!("mix: {tree} has no {}", src_of(part).display()).into());
        }
    }
    let bad = conflicts(mix);
    if !bad.is_empty() && std::env::var("NEONBRIDGE_MIX_COMPAT").map_or(true, |v| v.trim() != "0") {
        return Err(format!("this mix holds pairs vendor/mix-compat.toml lists as incompatible (NEONBRIDGE_MIX_COMPAT=0 builds it anyway):\n{}", bad.join("\n")).into());
    }
    let dir = dir(mix);
    if let Some(e) = incompatible(mix) {
        return Err(format!("this mix is known not to compile ({}):\n{e}", dir.display()).into());
    }
    let exe = dir.join("victory.exe");
    if exe.is_file() {
        println!("[launch] mix {}: built before", dir.display());
        return Ok(exe);
    }
    crate::splash::status(0.03, "Assembling the mix");
    let ws = dir.join("ws");
    if ws.exists() {
        fs::remove_dir_all(&ws)?;
    }
    let app_root = crate::runtimes::victory_tree(&app).ok_or("mix: no app tree")?;
    let mut trees: Vec<PathBuf> = Vec::new();
    for (part, tree) in &mix.parts {
        let root = crate::runtimes::victory_tree(tree).ok_or("mix: no tree")?;
        copy_tree(&root.join(src_of(part)), &ws.join(src_of(part)))?;
        if !trees.contains(&root) {
            trees.push(root);
        }
    }
    for f in ["Cargo.lock", "rust-toolchain.toml", "rustfmt.toml", ".cargo/config.toml"] {
        if app_root.join(f).is_file() {
            fs::create_dir_all(ws.join(f).parent().unwrap_or(&ws))?;
            fs::copy(app_root.join(f), ws.join(f))?;
        }
    }
    if app_root.join("xtask").is_dir() {
        copy_tree(&app_root.join("xtask"), &ws.join("xtask"))?;
    }
    let manifest = merged_manifest(&fs::read_to_string(app_root.join("Cargo.toml"))?, &trees);
    fs::write(ws.join("Cargo.toml"), manifest)?;
    fs::write(dir.join("mix.txt"), mix.text())?;
    // Each tree's patch, on the parts it supplies only.
    let mut by_tree: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (part, tree) in &mix.parts {
        by_tree.entry(tree).or_default().push(format!("{}/*", src_of(part).display()).replace('\\', "/"));
    }
    for (tree, includes) in by_tree {
        let patch = crate::runtimes::patch_file(tree);
        if !patch.is_file() || !crate::runtimes::patching() {
            continue;
        }
        let mut c = crate::runtimes::command("git");
        c.args(["apply", "--ignore-whitespace"]).args(includes.iter().map(|i| format!("--include={i}"))).arg(crate::runtimes::plain(&patch)).current_dir(&ws);
        let (ok, out) = crate::runtimes::quiet(c);
        if !ok {
            return Err(format!("mix: {} does not apply to the crates from {tree}:\n{out}", patch.display()).into());
        }
        println!("[launch] mix: applied {} to {}", patch.display(), includes.join(" "));
    }
    let (_, spec) = crate::runtimes::spec_of(&app)?;
    let mut cmd = spec.ok_or("mix: the app runtime has no build line")?;
    // --manifest-path <x> and --locked fit the tree, not the mix.
    if let Some(i) = cmd.iter().position(|a| a == "--manifest-path") {
        cmd.drain(i..(i + 2).min(cmd.len()));
    }
    cmd.retain(|a| a != "--locked");
    let target = home().join("target");
    match crate::runtimes::cargo(&cmd, &ws, &target, "Building the mix") {
        Ok(()) => {}
        Err(errors) => {
            let _ = fs::write(dir.join("incompatible.txt"), &errors);
            return Err(format!("this mix does not compile; remembered as incompatible ({}):\n{errors}", dir.display()).into());
        }
    }
    fs::copy(target.join("release").join("victory.exe"), &exe)?;
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_text_round_trips() {
        let m = Mix::parse("app=scarlettport\nvictory-gpu = vitoriabr\n\nbad line\n");
        assert_eq!(m.app(), Some("scarlettport"));
        assert_eq!(Mix::parse(&m.text()), m);
    }

    #[test]
    fn extra_workspace_deps_are_added() {
        let app = "[workspace.dependencies]\nanyhow = \"1\"\n[profile.release]\n";
        let deps = workspace_deps("[workspace.dependencies]\nanyhow = \"1\"\nminiz_oxide = \"0.9.1\"\n");
        assert_eq!(deps.len(), 2);
        let have: Vec<String> = workspace_deps(app).into_iter().map(|(k, _)| k).collect();
        assert_eq!(have, ["anyhow"]);
    }
}
