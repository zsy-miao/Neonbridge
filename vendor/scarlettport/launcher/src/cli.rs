//! Running the runtime's own tools (`victory saves ...`, `victory crashes list`, `--version`)
//! and reading what they print. Commands run without a console window, on a worker thread.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Runs `exe args` with `env` in the exe's folder; Ok(stdout) on success, Err(stderr or why).
pub fn run(exe: &Path, args: &[String], env: &[(&'static str, String)]) -> Result<String, String> {
    let mut cmd = Command::new(exe);
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = exe.parent() {
        cmd.current_dir(d);
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| format!("could not start {}: {e}", exe.display()))?;
    let so = crate::perf::strip_ansi(&String::from_utf8_lossy(&out.stdout));
    let se = crate::perf::strip_ansi(&String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(so)
    } else {
        let msg = se.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        Err(if msg.is_empty() { format!("exit code {:?}", out.status.code()) } else { msg })
    }
}

/// True when an error says the subcommand does not exist in this build.
pub fn unknown_command(err: &str) -> bool {
    let e = err.to_lowercase();
    e.contains("unrecognized subcommand") || e.contains("unexpected argument") || e.contains("invalid value")
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Container {
    pub name: String,
    pub state: String,
    pub backups: u32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct User {
    pub id: String,
    pub who: String,
    pub containers: Vec<Container>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Slot {
    pub title: String,
    pub name: String,
    pub users: Vec<User>,
}

/// Parses `victory saves list`:
/// `title 7A4E5C29:` / `  default` / `    user 0009... (Ziggy), config ...` /
/// `      Player (v3, 1234 bytes, 4 backups)`.
pub fn parse_saves(text: &str) -> Vec<Slot> {
    let mut out: Vec<Slot> = Vec::new();
    let mut title = String::new();
    for l in text.lines() {
        let indent = l.len() - l.trim_start().len();
        let t = l.trim();
        if t.is_empty() {
            continue;
        }
        if indent == 0 {
            if let Some(id) = t.strip_prefix("title ") {
                title = id.trim_end_matches(':').split(',').next().unwrap_or("").to_string();
            }
        } else if indent <= 2 {
            out.push(Slot { title: title.clone(), name: t.split(" (").next().unwrap_or(t).to_string(), users: Vec::new() });
        } else if indent <= 4 {
            if let (Some(s), Some(rest)) = (out.last_mut(), t.strip_prefix("user ")) {
                let id = rest.split_whitespace().next().unwrap_or("").to_string();
                let who = rest.find('(').and_then(|i| rest[i + 1..].split(')').next()).unwrap_or("").to_string();
                s.users.push(User { id, who, containers: Vec::new() });
            }
        } else if let Some(u) = out.last_mut().and_then(|s| s.users.last_mut()) {
            if t == "no containers" {
                continue;
            }
            let (name, inner) = match t.find(" (") {
                Some(i) => (&t[..i], t[i + 2..].trim_end_matches(')')),
                None => (t, ""),
            };
            let backups = inner.rsplit(", ").next().and_then(|b| b.split_whitespace().next()).and_then(|n| n.parse().ok()).unwrap_or(0);
            let state = inner.rsplit_once(", ").map(|(a, _)| a).unwrap_or(inner).to_string();
            u.containers.push(Container { name: name.to_string(), state, backups });
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
pub struct Backup {
    pub user: String,
    pub container: String,
    pub id: String,
    pub version: String,
    pub bytes: String,
    pub age: String,
}

/// Parses the listing `victory saves restore <slot>` prints without a container:
/// `  user X: Player` / `    <id>  v3  1234 bytes  5m ago`.
pub fn parse_backups(text: &str) -> Vec<Backup> {
    let mut out = Vec::new();
    let (mut user, mut container) = (String::new(), String::new());
    for l in text.lines() {
        let t = l.trim();
        if let Some(rest) = t.strip_prefix("user ") {
            if let Some((u, c)) = rest.split_once(": ") {
                user = u.to_string();
                container = c.to_string();
            }
            continue;
        }
        let f: Vec<&str> = t.split_whitespace().collect();
        if f.len() >= 5 && f[1].starts_with('v') && f[3] == "bytes" {
            out.push(Backup { user: user.clone(), container: container.clone(), id: f[0].into(), version: f[1].into(), bytes: f[2].into(), age: f[4..].join(" ") });
        }
    }
    out
}

/// `(xuid, gamertag)` pairs of the runtime's local profiles file.
pub fn profiles() -> Vec<(String, String)> {
    let Some(base) = std::env::var_os("LOCALAPPDATA") else { return Vec::new() };
    let text = std::fs::read_to_string(Path::new(&base).join("VIctory").join("profiles.txt")).unwrap_or_default();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#'))
        .filter_map(|l| l.trim().split_once(char::is_whitespace).map(|(a, b)| (a.to_string(), b.trim().to_string())))
        .collect()
}

/// The runtime's save folder: `VICTORY_DATA_DIR`, else `%LOCALAPPDATA%\victory\data`.
pub fn saves_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("VICTORY_DATA_DIR") {
        return PathBuf::from(d);
    }
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("victory").join("data")
}

/// The runtime's crash folder default (`crashes` beside the data folder).
pub fn crash_dir() -> PathBuf {
    saves_dir().with_file_name("crashes")
}

/// A crash folder from the stability watchdogs, read directly (for builds without `victory crashes`).
pub fn crashes_from_folder(dir: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut v: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let summary = std::fs::read_to_string(e.path().join("summary.txt")).map(|s| s.trim().replace('\t', "  ")).unwrap_or_else(|_| "(no summary)".into());
            (name, summary)
        })
        .collect();
    v.sort();
    v.into_iter().map(|(n, s)| format!("{n}  {s}")).collect()
}

/// What a runtime executable says about itself: `--version` output when it has one, else its
/// file version resource is not read; the date and a short SHA-256 identify the build.
pub fn describe(exe: &Path) -> String {
    if let Ok(out) = run(exe, &["--version".into()], &[]) {
        let l = out.lines().next().unwrap_or("").trim().to_string();
        if !l.is_empty() {
            return l;
        }
    }
    let meta = std::fs::metadata(exe).ok();
    let when = meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| crate::saves::stamp(d.as_secs())[..16].replace('_', " ")).unwrap_or_default();
    let hash = crate::runtime::file_hash(exe).map(|h| h[..8].to_string()).unwrap_or_default();
    // a farm build's target folder name says which branch it is (fxfork-cand-target -> cand)
    let branch = exe
        .ancestors()
        .filter_map(|a| a.file_name().map(|n| n.to_string_lossy().into_owned()))
        .find(|n| n.ends_with("-target"))
        .map(|n| format!("{}, ", n.trim_end_matches("-target")))
        .unwrap_or_default();
    format!("{branch}built {when} UTC, sha {hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_listing() {
        let t = "title 7A4E5C29:\n  default\n    user 0009000000ABCDEF (Ziggy), config 1\n      Player (v3, 1234 bytes, 4 backups)\n      Old (deleted, backups only, 2 backups)\n  fresh (pre-slot location)\n    user 01 (not in profiles.txt), config 1\n      no containers\n";
        let s = parse_saves(t);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].title, "7A4E5C29");
        assert_eq!(s[0].users[0].who, "Ziggy");
        assert_eq!(s[0].users[0].containers[0], Container { name: "Player".into(), state: "v3, 1234 bytes".into(), backups: 4 });
        assert_eq!(s[0].users[0].containers[1].backups, 2);
        assert_eq!(s[1].name, "fresh");
        assert!(s[1].users[0].containers.is_empty());
    }

    #[test]
    fn backup_listing() {
        let t = "title 7A4E5C29, slot default:\n  user 0009: Player\n    1700000000000  v3  1234 bytes  5m ago\n    1699  v2  1200 bytes  2h ago\n";
        let b = parse_backups(t);
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].container, "Player");
        assert_eq!(b[1].age, "2h ago");
    }

    #[test]
    fn unknown_subcommand() {
        assert!(unknown_command("error: unrecognized subcommand 'crashes'"));
        assert!(!unknown_command("no title data"));
    }
}
