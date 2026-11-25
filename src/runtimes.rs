//! One launcher for every leader's working emulator. NEONBRIDGE_RUNTIME=<name>
//! (or --runtime <name>) picks vendor/<name>, whose NEONBRIDGE.toml says how
//! to build, start and configure it; the default is scarlettport, and
//! `tuxbox` is the TuxBox provider path in main.rs. `--list` prints them.
//!
//! The launch follows ScarlettPort's launcher (launcher/src/engine.rs,
//! runtime.rs, scripts/setup-victory.ps1 and play-victory.ps1; credit to its
//! authors): the tree is built when its exe is older than its sources, then
//! run from the exe's folder with the toml's env plus the game folder, the
//! cache (%LOCALAPPDATA%\6ixpack\<name>\cache) and, when asked, the D3D12
//! Agility SDK (NEONBRIDGE_AGILITY_DIR, default
//! %LOCALAPPDATA%\6ixpack\agility-1.614.1, the official NuGet package's x64
//! DLLs) copied to <exe dir>\D3D12. An env var already set in ours wins.
//!
//! Logging keeps run.sh's contract: stdout and stderr go to --log, and each
//! frame-rate line ("12.3 fps, ...") is followed by
//! "[<name>] heartbeat: completed PresentX #N (F fps)", N being the frames
//! counted by a "frame times: N frames" line (VIctory's frame log), else the
//! fps lines' estimate.
//!
//! NEONBRIDGE.toml: flat keys and one [env] table; values are "strings",
//! ["string", "arrays"] or true/false.
//!   name, about                  what --list prints
//!   build = [..]                 run in vendor/<name> when the exe is stale
//!   watch = [..]                 sources that make it stale (default: the
//!                                tree minus target and .git)
//!   exe = "target/release/x.exe" relative to vendor/<name>
//!   cwd = ".."                   optional, default the exe's folder
//!   run = [..], prewarm = [..]   arguments; prewarm is optional
//!   game_env, cache_env          variables given the game folder / cache
//!   agility = true               copy the Agility DLLs beside the exe
//!   [env] KEY = "value"
//!   options = ["KEY=a|b (default a): text", "--flag=x|y (default x): text"]
//!                                every setting a leader's launcher offers;
//!                                `--set KEY=VALUE` sets an env var, `--set
//!                                --flag=VALUE` replaces (or adds) the flag's
//!                                value in the run arguments
//!   preset_<name> = ["KEY=VALUE", "--flag=VALUE"]   picked with --preset
//!   default_preset = "<name>"    the leader's own launcher settings, applied
//!                                when a run names no --preset and no --set
//!                                (NEONBRIDGE_AUTHOR_DEFAULTS=0: never)
//!   setup = ["name: command [asks first: download]", ..]  `--setup <runtime>`
//!                                runs each command in vendor/<name>; a step
//!                                marked "asks first" waits for a typed yes
//! Placeholders in run, prewarm and env: {game}, {game_exe}, {cache},
//! {jobs} (cores - 1), {press} (<cache>\press.txt, emptied at each start).
//! Settings apply in this order, the later winning: the toml's [env] and
//! run, our environment, the preset, then each --set.
use std::collections::BTreeMap;
use std::error::Error;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

/// Children get no console window of their own (the loader shows progress).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn hidden(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(CREATE_NO_WINDOW)
}

/// Every child the launcher starts is made here: CREATE_NO_WINDOW, so none
/// opens a console of its own. The picker's launcher has freed its console
/// (main.rs FreeConsole), and a console program started from there would
/// otherwise flash a cmd window. Its stdio is still the launcher's (or the
/// pipes the caller sets), so its output still reaches the log.
pub fn command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    // `cargo` is the one found by cargo_exe (a shortcut-started launcher may
    // not have ~/.cargo/bin on its PATH).
    let mut c = if program.as_ref() == "cargo" { Command::new(cargo_exe()) } else { Command::new(program) };
    hidden(&mut c);
    c
}

const GAME_EXE: &str = "game_scarlett_master.exe";
pub const DEFAULT: &str = "scarlettport";
/// Neonbridge's own build: TuxBox's providers with Neonbridge's fixes and
/// shader translator, vendored at vendor/neonbridge-own.
pub const TUXBOX: &str = "neonbridge-own";
/// Its old id, still taken for one release while no vendor/tuxbox runtime
/// (the clean brcly/TuxBox) has a NEONBRIDGE.toml of its own.
const OLD_OWN: &str = "tuxbox";

/// --runtime, else NEONBRIDGE_RUNTIME, else scarlettport.
pub fn name(arg: Option<&str>) -> String {
    arg.map(str::to_owned)
        .or_else(|| std::env::var("NEONBRIDGE_RUNTIME").ok())
        .map(|v| v.trim().to_ascii_lowercase())
        .filter(|v| !v.is_empty())
        .map(|v| alias(&v))
        .unwrap_or_else(|| DEFAULT.into())
}

/// `tuxbox` meant Neonbridge's own build until it was renamed neonbridge-own.
pub fn alias(name: &str) -> String {
    if name == OLD_OWN && !vendor().join(OLD_OWN).join("NEONBRIDGE.toml").is_file() {
        println!("[launch] runtime {OLD_OWN} is now {TUXBOX}; the old name works for this release only");
        return TUXBOX.to_owned();
    }
    name.to_owned()
}

/// NEONBRIDGE_VENDOR, else the vendor folder beside neonbridge.exe (a
/// release, `--package`), else the checkout it was built from.
pub fn vendor() -> PathBuf {
    std::env::var_os("NEONBRIDGE_VENDOR").map(PathBuf::from)
        .or_else(|| std::env::current_exe().ok()?.parent().map(|d| d.join("vendor")).filter(|v| v.is_dir()))
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("vendor"))
}

/// `--package`: every vendor/<name> runtime built (its patches applied for
/// the build) and laid out in `<stage>/vendor/<name>`: its NEONBRIDGE.toml,
/// its exe at the same relative path (so the toml finds it, and the release
/// never rebuilds: no sources, nothing stale), its LICENSE and UPSTREAM.
/// The Agility DLLs are not shipped: the toml's setup step stages them.
/// The cargo to run: `cargo` when it answers on PATH, else
/// %CARGO_HOME%\bin\cargo.exe, else %USERPROFILE%\.cargo\bin\cargo.exe (a
/// launcher started from an Explorer shortcut can lack the PATH entry its
/// shells have). Found once; its folder is put first on this process's PATH,
/// so builds and the tools they start (rustc through rustup) find it too.
pub fn cargo_exe() -> PathBuf {
    static FOUND: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    FOUND
        .get_or_init(|| {
            let answers = |p: &Path| Command::new(p).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).creation_flags_hidden().status().is_ok_and(|s| s.success());
            if answers(Path::new("cargo")) {
                return PathBuf::from("cargo");
            }
            let homes = [std::env::var_os("CARGO_HOME").map(PathBuf::from), std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join(".cargo"))];
            for home in homes.into_iter().flatten() {
                let exe = home.join("bin").join("cargo.exe");
                if exe.is_file() && answers(&exe) {
                    println!("[launch] cargo is not on PATH; using {}", exe.display());
                    let path = std::env::var_os("PATH").unwrap_or_default();
                    let dirs = std::iter::once(home.join("bin")).chain(std::env::split_paths(&path));
                    if let Ok(joined) = std::env::join_paths(dirs) {
                        // SAFETY: set once, before any build starts.
                        unsafe { std::env::set_var("PATH", joined) };
                    }
                    return exe;
                }
            }
            println!("[launch] cargo not found on PATH, in %CARGO_HOME%\\bin or %USERPROFILE%\\.cargo\\bin");
            PathBuf::from("cargo")
        })
        .clone()
}

trait HiddenExt {
    fn creation_flags_hidden(&mut self) -> &mut Self;
}

impl HiddenExt for Command {
    fn creation_flags_hidden(&mut self) -> &mut Self {
        use std::os::windows::process::CommandExt;
        self.creation_flags(CREATE_NO_WINDOW)
    }
}

/// A runtime whose prerequisite is missing or whose build fails is left
/// out, not fatal: it comes back in the second list as (name, reason), for
/// the package's README.
pub fn package(stage: &Path) -> Result<(Vec<String>, Vec<(String, String)>), Box<dyn Error>> {
    let (mut done, mut skipped) = (Vec::new(), Vec::new());
    for (name, _, _) in rows().into_iter().filter(|(n, _, _)| n != TUXBOX) {
        let (root, spec) = match load(&name) {
            Ok(l) => l,
            Err(e) => {
                println!("[package] {name}: not bundled: {e}");
                skipped.push((name, e.to_string()));
                continue;
            }
        };
        if let Err(e) = prerequisite(&root, &spec) {
            println!("[package] {name}: not bundled: {e}");
            skipped.push((name, e));
            continue;
        }
        println!("[package] {name}: building");
        let exe = match build(&name, &root, &spec) {
            Ok(e) => e,
            Err(e) => {
                let reason = e.to_string().lines().next().unwrap_or("its build failed").to_owned();
                println!("[package] {name}: not bundled: {reason}");
                skipped.push((name, reason));
                continue;
            }
        };
        let to = stage.join("vendor").join(&name);
        let rel = exe.strip_prefix(&root).map_err(|_| format!("runtime {name}: {} is outside its tree", exe.display()))?;
        fs::create_dir_all(to.join(rel).parent().unwrap_or(&to))?;
        fs::copy(&exe, to.join(rel))?;
        fs::copy(root.join("NEONBRIDGE.toml"), to.join("NEONBRIDGE.toml"))?;
        for f in ["LICENSE", "LICENSE.md", "UPSTREAM", "VENDORED.md"] {
            if root.join(f).is_file() {
                fs::copy(root.join(f), to.join(f))?;
            }
        }
        done.push(format!("{name} ({})", rel.display()));
    }
    Ok((done, skipped))
}

/// The tool a toml's build line (or `launcher`) starts must be there: dotnet
/// with the SDK the tree asks for (its global.json; `dotnet --version` in the
/// tree fails without it, 0x80008091), Python 3.10+, cargo. Err is the reason, for a person.
fn prerequisite(root: &Path, spec: &Spec) -> Result<(), String> {
    let first = |k: &str| spec.list(k).and_then(|l| l.first()).map(String::as_str);
    for tool in [first("build"), first("launcher")].into_iter().flatten() {
        match tool {
            "python" | "python3" | "py" => {
                python()?;
            }
            "dotnet" => {
                let o = crate::runtimes::command("dotnet").arg("--version").current_dir(root).output()
                    .map_err(|_| "needs the .NET SDK (dotnet was not found)".to_owned())?;
                if !o.status.success() {
                    let want = fs::read_to_string(root.join("global.json")).ok()
                        .and_then(|j| j.split("\"version\"").nth(1).and_then(|v| v.split('"').nth(1)).map(str::to_owned));
                    return Err(match want {
                        Some(v) => format!("needs the .NET SDK {v} (global.json), which is not installed"),
                        None => "needs a .NET SDK that is not installed".to_owned(),
                    });
                }
            }
            "cargo" => {
                let exe = cargo_exe();
                let o = crate::runtimes::command(&exe).arg("--version").output();
                let fail = match &o {
                    Ok(o) if o.status.success() => None,
                    Ok(o) => Some(format!("{} --version: {}: {}", exe.display(), o.status, String::from_utf8_lossy(&o.stderr).trim())),
                    Err(e) => Some(format!("{} --version: {e}", exe.display())),
                };
                if let Some(why) = fail {
                    println!("[launch] cargo check failed: {why}");
                    return Err("needs Rust (cargo was not found)".into());
                }
            }
            _ => {}
        }
    }
    // After the SDK: Five Star's build also links the PC GDK.
    if first("build") == Some("dotnet") && needs_gdk(root) && !gdk_installed() {
        return Err("needs the Microsoft PC GDK, which is not installed".into());
    }
    Ok(())
}

/// For the picker: a runtime that can't run here yet, as (reason, the
/// official download pages for what it lacks). None when its exe is built
/// (a release, or built before) or its prerequisites are all there.
pub fn missing(name: &str) -> Option<(String, Vec<&'static str>)> {
    let (root, spec) = load(name).ok()?;
    if spec.str("exe").is_some_and(|e| root.join(e).is_file()) {
        return None;
    }
    let reason = prerequisite(&root, &spec).err()?;
    let first = |k: &str| spec.list(k).and_then(|l| l.first()).cloned().unwrap_or_default();
    let tools = [first("build"), first("launcher")];
    let mut pages = Vec::new();
    if reason.contains("PC GDK") {
        pages.push(GDK_DOWNLOAD);
    } else if tools.iter().any(|t| t == "dotnet") {
        let major = fs::read_to_string(root.join("global.json")).ok()
            .and_then(|j| j.split("\"version\"").nth(1).and_then(|v| v.split('"').nth(1)).and_then(|v| v.split('.').next().map(str::to_owned)));
        pages.push(match major.as_deref() {
            Some("8") => "https://dotnet.microsoft.com/download/dotnet/8.0",
            Some("9") => "https://dotnet.microsoft.com/download/dotnet/9.0",
            _ => "https://dotnet.microsoft.com/download/dotnet/10.0",
        });
        // Five Star's build also links the PC GDK (its toml's build step says so).
        if fs::read_to_string(root.join("NEONBRIDGE.toml")).is_ok_and(|t| t.contains("GDK")) {
            pages.push("https://learn.microsoft.com/gaming/gdk/");
        }
    }
    if tools.iter().any(|t| matches!(t.as_str(), "python" | "python3" | "py")) {
        pages.push("https://www.python.org/downloads/windows/");
    }
    if tools.iter().any(|t| t == "cargo") {
        pages.push("https://rustup.rs/");
    }
    Some((reason, pages))
}

/// The PC GDK release Neonbridge fetches (with the user's yes) from
/// github.com/microsoft/GDK into %LOCALAPPDATA%\6ixpack\gdk\<version> and
/// installs with its own setup. Never committed.
pub const GDK_VERSION: &str = "2610.0.9661";
pub const GDK_URL: &str = "https://github.com/microsoft/GDK/releases/download/October-2026-v2610.0.9661/GDK_2610.0.9661.zip";
/// The picker's Install... page for it: a confirm-then-download, not a web page.
pub const GDK_DOWNLOAD: &str = "neonbridge:gdk";

pub fn gdk_dir() -> PathBuf {
    crate::data_dir().join("gdk").join(GDK_VERSION)
}

/// Fetches the GDK release into gdk_dir() and starts its installer, for
/// the picker's Install... after the user said yes. The zip is Microsoft's
/// signed setup (PGDK.exe, Installers\*.cab, SetupScripts\), not loose
/// files, so the GDK is installed system-wide: Windows asks the user to
/// allow PGDK.exe (UAC). Waits for the installer. Err is for a person.
pub fn fetch_gdk() -> Result<(), String> {
    let dir = gdk_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let setup = dir.join("PGDK.exe");
    if !setup.is_file() {
        let zip = dir.join(format!("GDK_{GDK_VERSION}.zip"));
        let ok = |c: &mut Command| c.stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success());
        if !ok(crate::runtimes::command("curl").args(["-L", "--fail", "-o"]).arg(&zip).arg(GDK_URL)) {
            return Err(format!("downloading {GDK_URL} failed"));
        }
        if !ok(crate::runtimes::command("powershell").args(["-NoProfile", "-Command", "Expand-Archive -LiteralPath $env:NB_ZIP -DestinationPath $env:NB_DIR -Force"]).env("NB_ZIP", &zip).env("NB_DIR", &dir)) {
            return Err(format!("unzipping {} failed", zip.display()));
        }
        let _ = fs::remove_file(&zip);
        if !setup.is_file() {
            return Err(format!("the GDK zip held no PGDK.exe ({})", dir.display()));
        }
    }
    // Start-Process goes through ShellExecute, so the installer's UAC prompt appears.
    let started = crate::runtimes::command("powershell")
        .args(["-NoProfile", "-Command", "Start-Process -FilePath $env:NB_SETUP -WorkingDirectory $env:NB_DIR -Wait"])
        .env("NB_SETUP", &setup)
        .env("NB_DIR", &dir)
        .status()
        .is_ok_and(|s| s.success());
    if !started {
        return Err("the GDK installer did not run (was it allowed?)".into());
    }
    if !gdk_installed() {
        return Err("the GDK installer finished, but no Microsoft GDK folder was found under Program Files (x86)".into());
    }
    Ok(())
}

/// The tree's toml says its build links the PC GDK (Five Star: xgameruntime.lib).
fn needs_gdk(root: &Path) -> bool {
    fs::read_to_string(root.join("NEONBRIDGE.toml")).is_ok_and(|t| t.contains("GDK"))
}

/// The PC GDK is installed: its GameDK / GameDKLatest variables, or its
/// folder under Program Files (x86).
fn gdk_installed() -> bool {
    ["GameDKLatest", "GameDK"].iter().filter_map(std::env::var_os).any(|d| Path::new(&d).is_dir())
        || std::env::var_os("ProgramFiles(x86)").is_some_and(|p| Path::new(&p).join("Microsoft GDK").is_dir())
}

pub enum Task {
    Build,
    Run,
    Prewarm,
}

#[derive(Clone, Debug, PartialEq)]
enum Value {
    Str(String),
    List(Vec<String>),
    Bool(bool),
}

#[derive(Default, Debug)]
struct Spec {
    keys: BTreeMap<String, Value>,
    env: Vec<(String, String)>,
}

impl Spec {
    fn str(&self, k: &str) -> Option<&str> {
        match self.keys.get(k) {
            Some(Value::Str(s)) => Some(s),
            _ => None,
        }
    }
    fn list(&self, k: &str) -> Option<&[String]> {
        match self.keys.get(k) {
            Some(Value::List(l)) => Some(l),
            _ => None,
        }
    }
    fn flag(&self, k: &str) -> bool {
        matches!(self.keys.get(k), Some(Value::Bool(true)))
    }
}

/// A "quoted" string (\" and \\ escapes) at the start of `s`, and the rest.
fn quoted(s: &str) -> Option<(String, &str)> {
    let mut chars = s.strip_prefix('"')?.char_indices();
    let mut out = String::new();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &s[i + 2..])),
            '\\' => out.push(chars.next()?.1),
            c => out.push(c),
        }
    }
    None
}

fn value(s: &str) -> Option<Value> {
    let s = s.trim();
    match s {
        "true" => return Some(Value::Bool(true)),
        "false" => return Some(Value::Bool(false)),
        _ => {}
    }
    // A bare number (rank = 3) is kept as its text.
    if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) {
        return Some(Value::Str(s.to_owned()));
    }
    if s.starts_with('"') {
        return quoted(s).map(|(v, _)| Value::Str(v));
    }
    let mut rest = s.strip_prefix('[')?.trim_start();
    let mut items = Vec::new();
    loop {
        if let Some(r) = rest.strip_prefix(']') {
            let _ = r;
            return Some(Value::List(items));
        }
        let (item, r) = quoted(rest)?;
        items.push(item);
        rest = r.trim_start();
        rest = rest.strip_prefix(',').unwrap_or(rest).trim_start();
    }
}

fn parse(text: &str) -> Result<Spec, String> {
    let mut spec = Spec::default();
    // The table the lines belong to: None at the top, "env" for [env]; any
    // other table's keys are kept as "<table>.<key>" ([controls] and the rest).
    let mut table: Option<String> = None;
    // A list may span lines: gather until its brackets close.
    let mut pending = String::new();
    for (n, raw) in text.lines().enumerate() {
        let line = if raw.trim_start().starts_with('#') { "" } else { raw };
        let line = if pending.is_empty() { line.trim().to_owned() } else { format!("{pending} {}", line.trim()) };
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && !line.contains('=') {
            let head = strip_comment(&line);
            let Some(t) = head.strip_prefix('[').and_then(|h| h.strip_suffix(']')).map(|t| t.trim().trim_matches('"')).filter(|t| !t.is_empty()) else {
                return Err(format!("line {}: cannot read the table name {head}", n + 1));
            };
            table = Some(t.to_owned());
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { return Err(format!("line {}: no key = value", n + 1)) };
        let v = strip_comment(v.trim());
        if v.starts_with('[') && !v.trim_end().ends_with(']') {
            pending = line.clone();
            continue;
        }
        pending.clear();
        let k = k.trim().trim_matches('"').to_owned();
        let val = value(v).ok_or_else(|| format!("line {}: cannot read the value of {k}", n + 1))?;
        if table.as_deref() == Some("env") {
            match val {
                Value::Str(s) => spec.env.push((k, expand_env(&s))),
                Value::Bool(b) => spec.env.push((k, if b { "1" } else { "0" }.into())),
                Value::List(_) => return Err(format!("line {}: [env] {k} is a list", n + 1)),
            }
        } else {
            spec.keys.insert(table.as_ref().map_or(k.clone(), |t| format!("{t}.{k}")), val);
        }
    }
    Ok(spec)
}

/// The value with a trailing `# comment` cut off (outside quotes).
fn strip_comment(v: &str) -> &str {
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in v.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '#' if !quoted => return v[..i].trim_end(),
            _ => {}
        }
    }
    v
}

fn load(name: &str) -> Result<(PathBuf, Spec), Box<dyn Error>> {
    let root = vendor().join(name);
    let file = root.join("NEONBRIDGE.toml");
    let text = fs::read_to_string(&file).map_err(|e| format!("runtime {name}: {} ({e}); `neonbridge --list` names the runtimes", file.display()))?;
    Ok((root, parse(&text).map_err(|e| format!("{}: {e}", file.display()))?))
}

/// One `options` entry: "KEY=a|b|c (default X; more): text" or "--flag=...".
#[derive(Debug, PartialEq)]
struct Opt {
    key: String,
    values: String,
    default: Option<String>,
    about: String,
}

fn opt(entry: &str) -> Opt {
    let (head, about) = match entry.find("): ") {
        Some(i) => (&entry[..=i], entry[i + 3..].trim()),
        None => entry.split_once(": ").unwrap_or((entry, "")),
    };
    let (key, rest) = head.split_once('=').unwrap_or((head, ""));
    let (values, note) = rest.split_once(" (").map_or((rest.trim(), ""), |(v, n)| (v.trim(), n.trim_end_matches(')')));
    let default = note.split(';').find_map(|p| p.trim().strip_prefix("default ")).map(|d| d.trim().to_owned());
    Opt { key: key.trim().to_owned(), values: values.to_owned(), default, about: about.to_owned() }
}

/// The value `key` has now: a --set, the preset, our environment, the toml.
fn current(key: &str, spec: &Spec, sets: &[(String, String)]) -> Option<String> {
    if let Some((_, v)) = sets.iter().rev().find(|(k, _)| k == key) {
        return Some(v.clone());
    }
    if key.starts_with("--") {
        let run = spec.list("run").unwrap_or_default();
        return run.iter().position(|a| a == key).and_then(|i| run.get(i + 1).cloned())
            .or_else(|| run.iter().find_map(|a| a.strip_prefix(&format!("{key}=")).map(str::to_owned)));
    }
    std::env::var(key).ok().or_else(|| spec.env.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
}

/// The preset's settings, then the --set ones.
fn settings(spec: &Spec, preset: Option<&str>, sets: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    // No choice made: the leader's own launcher defaults (default_preset).
    let author = (preset.is_none() && sets.is_empty() && std::env::var("NEONBRIDGE_AUTHOR_DEFAULTS").map_or(true, |v| v.trim() != "0"))
        .then(|| spec.str("default_preset")).flatten();
    if let Some(p) = preset.or(author) {
        let list = spec.list(&format!("preset_{p}")).ok_or_else(|| {
            let known: Vec<&str> = spec.keys.keys().filter_map(|k| k.strip_prefix("preset_")).collect();
            format!("no preset {p} (known: {})", if known.is_empty() { "none".into() } else { known.join(", ") })
        })?;
        out.extend(list.iter().filter_map(|e| e.split_once('=')).map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned())));
    }
    for e in sets {
        let (k, v) = e.split_once('=').ok_or_else(|| format!("--set {e}: NAME=VALUE"))?;
        out.push((k.trim().to_owned(), v.to_owned()));
    }
    Ok(out)
}

/// `--list <runtime>`: its options, defaults and the values a run would use.
pub fn list_runtime(name: &str, preset: Option<&str>, sets: &[String]) -> Result<(), Box<dyn Error>> {
    if name == TUXBOX {
        println!("{TUXBOX}: Neonbridge's own build, the TuxBox providers with Neonbridge's fixes (vendor/neonbridge-own), staged into the game folder; its options: neonbridge --runtime {TUXBOX} --help");
        return Ok(());
    }
    let (root, spec) = load(name)?;
    let sets = settings(&spec, preset, sets)?;
    println!("{name}: {} ({})", spec.str("about").unwrap_or(""), root.display());
    println!("  run: {} {}", spec.str("exe").unwrap_or("?"), spec.list("run").unwrap_or_default().join(" "));
    let presets: Vec<&str> = spec.keys.keys().filter_map(|k| k.strip_prefix("preset_")).collect();
    if !presets.is_empty() {
        println!("  presets (--preset): {}", presets.join(", "));
    }
    if let Some(d) = spec.str("default_preset") {
        println!("  default_preset: {d} (its author's launcher settings, used when a run names no --preset and no --set)");
    }
    let mut keys: Vec<String> = Vec::new();
    println!("  options (--set NAME=VALUE):");
    for o in spec.list("options").unwrap_or_default().iter().map(|e| opt(e)) {
        let now = current(&o.key, &spec, &sets).unwrap_or_else(|| "(unset)".into());
        println!("    {:<34} = {:<10} default {:<8} values {}  {}", o.key, now, o.default.as_deref().unwrap_or("-"), o.values, o.about);
        keys.push(o.key);
    }
    for (k, v) in spec.env.iter().filter(|(k, _)| !keys.contains(k)) {
        println!("    {k:<34} = {:<10} (set by its NEONBRIDGE.toml, was {v})", current(k, &spec, &sets).unwrap_or_default());
    }
    for step in spec.list("setup").unwrap_or_default() {
        println!("  setup (--setup {name}): {step}");
    }
    Ok(())
}

/// `--setup <runtime>`: the toml's setup steps, in order. A step marked
/// "asks first" (a download, a file of the user's) waits for a typed yes.
pub fn setup(name: &str) -> Result<(), Box<dyn Error>> {
    let (root, spec) = load(name)?;
    let steps = spec.list("setup").unwrap_or_default();
    // {game}: NEONBRIDGE_GAME, else the picker's last game folder. {exe}: the toml's exe.
    let game = std::env::var_os("NEONBRIDGE_GAME").map(PathBuf::from).or_else(|| {
        let f = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack/last_game.txt");
        fs::read_to_string(f).ok().map(|t| PathBuf::from(t.trim())).filter(|p| p.is_dir())
    });
    if steps.is_empty() {
        println!("[launch] {name}: its NEONBRIDGE.toml has no setup steps");
    }
    let ask = |mark: &str| {
        print!("[setup] {mark}. Run it? Type yes to run, anything else skips: ");
        let _ = std::io::stdout().flush();
        let mut answer = String::new();
        let _ = std::io::stdin().read_line(&mut answer);
        answer.trim() == "yes"
    };
    run_steps(name, &root, &spec, game.as_deref(), &ask)
}

/// The toml's setup steps in order. A step marked `[asks first: ...]` runs
/// only when `ask` says yes. Err names the first step that failed.
fn run_steps(name: &str, root: &Path, spec: &Spec, game: Option<&Path>, ask: &dyn Fn(&str) -> bool) -> Result<(), Box<dyn Error>> {
    let root = root.to_path_buf();
    let steps = spec.list("setup").unwrap_or_default();
    let exe = spec.str("exe").map(|e| root.join(e));
    for step in steps {
        let (title, rest) = step.split_once(": ").unwrap_or(("step", step));
        if rest.contains("{game}") && game.is_none() {
            return Err(format!("setup {name}: {title} needs the game folder: pick it in the launcher first, or set NEONBRIDGE_GAME").into());
        }
        let rest = rest.replace("{game}", &game.as_ref().map(|g| g.display().to_string()).unwrap_or_default());
        let rest = rest.replace("{exe}", &exe.as_ref().map(|e| e.display().to_string()).unwrap_or_default());
        let rest = rest.as_str();
        let (command, mark) = rest.split_once(" [").map_or((rest.trim(), ""), |(c, m)| (c.trim(), m.trim_end_matches(']')));
        println!("[setup] {name}: {title}: {command}");
        // A diagnostics step follows the Diagnostics switch, never a prompt.
        if title.contains("diag") || mark.contains("diagnostics") {
            if !diag() {
                println!("[setup] skipped (diagnostics off)");
                continue;
            }
        } else if mark.contains("asks first") && !ask(mark) {
            println!("[setup] skipped");
            continue;
        }
        // `python ...` steps run under the Python [`python`] finds.
        let command = match command.strip_prefix("python ") {
            Some(rest) => format!("{} {rest}", python()?.join(" ")),
            None => command.to_owned(),
        };
        let status = crate::runtimes::command("cmd").arg("/C").arg(&command).current_dir(&root).status()?;
        if !status.success() {
            return Err(format!("setup {name}: {title} failed ({status})").into());
        }
    }
    Ok(())
}

/// A launch runs the runtime's author setup itself (choose the folder and it
/// does the rest): the first time, and again whenever its setup list or the
/// game folder changes (setup-stamp.txt in its cache records both). Steps
/// that ask first (downloads, hours-long preparation) get a Yes/No box. A
/// failed step is logged and tried again next launch; the launch goes on.
/// NEONBRIDGE_AUTO_SETUP=0 leaves setup to --setup.
fn auto_setup(name: &str, root: &Path, spec: &Spec, game: &Path) {
    let Some(steps) = spec.list("setup").filter(|s| !s.is_empty()) else { return };
    if std::env::var("NEONBRIDGE_AUTO_SETUP").is_ok_and(|v| v.trim() == "0") {
        return;
    }
    let stamp = cache(name).join("setup-stamp.txt");
    let want = format!("{}\n{}", game.display(), steps.join("\n"));
    if fs::read_to_string(&stamp).is_ok_and(|s| s == want) {
        return;
    }
    crate::splash::status(0.46, &format!("Setting up {name}"));
    println!("[setup] {name}: first launch or changed setup: running its setup steps");
    let _ = std::io::stdout().flush();
    let ask = |mark: &str| {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_SETFOREGROUND, MB_TOPMOST, MB_YESNO, MessageBoxW};
        println!("[setup] {name}: asking whether to run a step that {mark}");
        let _ = std::io::stdout().flush();
        let wide = |t: &str| t.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(&format!("{name}'s setup has a step that {mark}.\n\nRun it now?")), wide("Neonbridge"));
        // SAFETY: both strings are NUL-terminated and live across the call.
        // Topmost and focused: an ownerless box opened behind the topmost
        // loader and the launch sat at "Setting up" with nothing to click.
        // No is the default.
        let yes = unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2 | MB_TOPMOST | MB_SETFOREGROUND) == IDYES };
        println!("[setup] {name}: {}", if yes { "yes" } else { "no, skipped" });
        yes
    };
    match run_steps(name, root, spec, Some(game), &ask) {
        Ok(()) => {
            let _ = fs::create_dir_all(cache(name));
            let _ = fs::write(&stamp, want);
        }
        Err(e) => println!("[setup] {e}; the launch goes on, and the setup runs again next time"),
    }
}

/// (name, about, presets) of tuxbox and every vendor/<name>/NEONBRIDGE.toml.
fn rows() -> Vec<(String, String, Vec<String>)> {
    let mut rows = vec![(TUXBOX.to_owned(), "Neonbridge's own build: the TuxBox providers with Neonbridge's fixes (vendor/neonbridge-own), staged into the game folder".to_owned(), Vec::new())];
    if let Ok(dir) = fs::read_dir(vendor()) {
        for e in dir.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if let Ok((_, spec)) = load(&n) {
                let presets = spec.keys.keys().filter_map(|k| k.strip_prefix("preset_")).map(str::to_owned).collect();
                rows.push((n, spec.str("about").unwrap_or("").to_owned(), presets));
            }
        }
    }
    rows.sort();
    rows
}

/// The runtime's about line from its toml ("<project> <hash>, <licence>").
pub fn about(name: &str) -> String {
    let a = rows().into_iter().find(|(n, _, _)| n == name).map(|(_, a, _)| a).unwrap_or_default();
    // brcly's upstream TuxBox: its README says Windows play is not there yet.
    if name == "tuxbox" && name != TUXBOX && !a.is_empty() { format!("{a} · upstream: Windows compatibility in development") } else { a }
}

/// Frames the runtime's last run presented (its last-run.log heartbeat), or
/// None when it has no run yet.
pub fn last_run_frames(name: &str) -> Option<u64> {
    let log = fs::read_to_string(cache(name).join("last-run.log")).ok()?;
    Some(log.lines().filter_map(|l| l.split("completed PresentX #").nth(1)).filter_map(|n| n.split(' ').next()?.parse::<u64>().ok()).max().unwrap_or(0))
}

/// The presets a runtime's toml names (preset_<name>).
pub fn presets(name: &str) -> Vec<String> {
    rows().into_iter().find(|(n, _, _)| n == name).map(|(_, _, p)| p).unwrap_or_default()
}

/// `--list`: every vendor/<name>/NEONBRIDGE.toml, and tuxbox.
pub fn list() {
    let chosen = name(None);
    for (n, about, _) in rows() {
        println!("{} {n:<14} {about}", if n == chosen { "*" } else { " " });
    }
    println!("(--runtime <name> or NEONBRIDGE_RUNTIME=<name>; default {DEFAULT}; --list <name> for its options)");
}

fn newest(path: &Path, best: &mut SystemTime) {
    let Ok(meta) = fs::symlink_metadata(path) else { return };
    if meta.is_dir() {
        let skip = path.file_name().is_some_and(|n| n == "target" || n == ".git");
        if !skip && let Ok(dir) = fs::read_dir(path) {
            dir.flatten().for_each(|e| newest(&e.path(), best));
        }
    } else if let Ok(t) = meta.modified() {
        *best = (*best).max(t);
    }
}

fn stale(root: &Path, spec: &Spec, exe: &Path) -> bool {
    let Ok(built) = fs::metadata(exe).and_then(|m| m.modified()) else { return true };
    let mut src = SystemTime::UNIX_EPOCH;
    match spec.list("watch") {
        Some(paths) => paths.iter().for_each(|p| newest(&root.join(p), &mut src)),
        None => newest(root, &mut src),
    }
    if patching() {
        newest(&vendor().join("patches"), &mut src);
    }
    src > built
}

/// Diagnostics builds, on by default for every runtime (the user's rule);
/// NEONBRIDGE_DIAG=0 or the picker's Diagnostics box turns them off. A tree
/// builds with its toml's `diag` line, else (victory family) with its
/// `diagnostics` cargo feature, and runs with VICTORY_D3D12_DRED=1.
pub fn diag() -> bool {
    std::env::var("NEONBRIDGE_DIAG").map_or(true, |v| v.trim() != "0")
}

/// Under [`diag`], a victory tree whose app has a `diagnostics` feature and
/// whose build line lacks it: (the build line with `--features diagnostics`,
/// the target folder target-diag beside its target, the exe in it). The
/// release build in target\ stays as it is, cached.
fn diag_build(name: &str, root: &Path, spec: &Spec, cmd: &[String], exe: &Path) -> Option<(Vec<String>, PathBuf, PathBuf)> {
    if !diag() {
        return None;
    }
    // The toml's own `diag_build` line (the author's diagnostics build) and
    // its `diag_exe`; `diag_build = []`: the tree has none, its build is
    // used as it is. The diag exe lives apart from the release one (cargo's
    // diag profile in target\diag; with no diag_exe, a cargo line builds into
    // target-diag), so each keeps its own stamp and switching the box never
    // forces a rebuild.
    if let Some(line) = spec.list("diag_build") {
        if line.is_empty() {
            return None;
        }
        let (target, exe) = match spec.str("diag_exe") {
            Some(e) => (PathBuf::new(), root.join(e)),
            None => {
                let to = root.join("target-diag");
                let exe = exe.strip_prefix(root.join("target")).map(|rel| to.join(rel)).unwrap_or_else(|_| exe.to_path_buf());
                (if line.first().map(String::as_str) == Some("cargo") { to } else { PathBuf::new() }, exe)
            }
        };
        return Some((line.to_vec(), target, exe));
    }
    if cmd.first().map(String::as_str) != Some("cargo") || cmd.iter().any(|a| a.contains("diagnostics")) {
        return None;
    }
    let tree = victory_tree(name)?;
    let manifest = fs::read_to_string(tree.join("apps").join("victory").join("Cargo.toml")).ok()?;
    if !manifest.lines().any(|l| l.trim_start().starts_with("diagnostics =")) {
        return None;
    }
    let target = tree.join("target");
    let rel = exe.strip_prefix(&target).ok()?;
    let to = tree.join("target-diag");
    let mut line = cmd.to_vec();
    let at = line.iter().position(|a| a == "--").unwrap_or(line.len());
    line.splice(at..at, ["--features".to_owned(), "diagnostics".to_owned()]);
    let _ = root;
    Some((line, to.clone(), to.join(rel)))
}

/// Builds the tree when its exe is stale. NEONBRIDGE_BUILD=0 never builds.
fn build(name: &str, root: &Path, spec: &Spec) -> Result<PathBuf, Box<dyn Error>> {
    let exe = root.join(spec.str("exe").ok_or_else(|| format!("runtime {name}: NEONBRIDGE.toml names no exe"))?);
    let line = spec.list("build").map(<[String]>::to_vec).unwrap_or_default();
    let (line, target, exe) = match diag_build(name, root, spec, &line, &exe) {
        Some((l, t, e)) => {
            println!("[launch] {name}: diagnostics build ({}) -> {} (NEONBRIDGE_DIAG=0 builds release)", l.join(" "), e.display());
            (l, Some(t).filter(|t| !t.as_os_str().is_empty()), e)
        }
        None => (line, None, exe),
    };
    let never = std::env::var("NEONBRIDGE_BUILD").is_ok_and(|v| v.trim() == "0");
    // The build stamp beside the exe: when it matches, neither the patch nor
    // cargo runs. The mtime check alone rebuilt on every launch: applying and
    // reverting the rt patch rewrites the patched sources' mtimes each time,
    // so both stale() and cargo's own fingerprints fired.
    let stamp_file = PathBuf::from(format!("{}.nbstamp", exe.display()));
    let stamp = build_stamp(name, root, &line, target.as_deref());
    if !never && exe.is_file() && let Some(s) = &stamp && fs::read_to_string(&stamp_file).is_ok_and(|t| t == *s) {
        println!("[launch] {name}: up to date (build stamp matches); not building");
        return Ok(exe);
    }
    // A tree built before stamps existed: an up-to-date exe gets its stamp, no build.
    if !never && exe.is_file() && let Some(s) = &stamp && !stamp_file.exists() && !stale(root, spec, &exe) {
        let _ = fs::write(&stamp_file, s);
        println!("[launch] {name}: up to date; build stamp written");
        return Ok(exe);
    }
    if never || (stamp.is_none() && !stale(root, spec, &exe)) {
        return if exe.is_file() { Ok(exe) } else { Err(format!("runtime {name}: {} is missing (NEONBRIDGE_BUILD=0)", exe.display()).into()) };
    }
    let cmd = Some(line.as_slice()).filter(|c| !c.is_empty()).ok_or_else(|| format!("runtime {name}: {} is stale and NEONBRIDGE.toml names no build", exe.display()))?;
    prerequisite(root, spec).map_err(|e| format!("runtime {name} {e}"))?;
    println!("[launch] {name}: building ({})", cmd.join(" "));
    let applied = patch(name, root)?;
    crate::splash::status(0.1, &format!("Building {name}"));
    let python_build = matches!(cmd.first().map(String::as_str), Some("python" | "python3" | "py"));
    let cmd = interpreter(cmd)?;
    let mut c = crate::runtimes::command(&cmd[0]);
    if python_build {
        build_env(name, &mut c)?;
    }
    c.args(&cmd[1..]).current_dir(root).env_remove("CARGO_TARGET_DIR").env("CARGO_TERM_PROGRESS_WHEN", "always").env("CARGO_TERM_PROGRESS_WIDTH", "100").stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(t) = &target {
        c.env("CARGO_TARGET_DIR", t);
    }
    let status = hidden(&mut c).spawn().and_then(|mut child| {
        // cargo's "Building [..] N/M" (or a Python build's N/M) moves the
        // loader bar from 0.1 to 0.45.
        let out = child.stdout.take().map(|o| std::thread::spawn(move || {
            for l in BufReader::new(o).lines().map_while(Result::ok) {
                println!("{l}");
            }
        }));
        crate::progress::build_lines(child.stderr.take().unwrap(), &format!("Building {name}"), 0.1, 0.45, |l| eprintln!("{l}"));
        let _ = out.map(|t| t.join());
        child.wait()
    });
    unpatch(name, root, &applied);
    let status = status.map_err(|e| format!("runtime {name}: {}: {e}", cmd[0]))?;
    if !status.success() {
        return Err(format!("runtime {name}: build failed ({status})").into());
    }
    // Reverting the patches touched the sources: the exe stays the newer.
    if !applied.is_empty() && let Ok(f) = fs::File::options().append(true).open(&exe) {
        let _ = f.set_modified(SystemTime::now());
    }
    if !exe.is_file() {
        return Err(format!("runtime {name}: the build made no {}", exe.display()).into());
    }
    if let Some(s) = &stamp {
        let _ = fs::write(&stamp_file, s);
    }
    Ok(exe)
}

/// What a tree's build depends on, as one sha256: its committed git tree
/// (`HEAD:./`), its uncommitted changes and untracked files, its rt patch,
/// the build line and the target folder. Taken with the patch reverted, so
/// it never sees our own edits. None (outside git, a release) falls back to
/// the mtime check. NEONBRIDGE_BUILD_STAMP=0 turns it off.
fn build_stamp(name: &str, root: &Path, line: &[String], target: Option<&Path>) -> Option<String> {
    use sha2::{Digest, Sha256};
    if std::env::var("NEONBRIDGE_BUILD_STAMP").as_deref() == Ok("0") {
        return None;
    }
    let git = |args: &[&str]| -> Option<Vec<u8>> {
        let o = crate::runtimes::command("git").arg("-C").arg(root).args(args).stderr(Stdio::null()).output().ok()?;
        o.status.success().then_some(o.stdout)
    };
    let mut h = Sha256::new();
    h.update(git(&["rev-parse", "HEAD:./"])?);
    h.update(git(&["diff", "HEAD", "--binary", "--", "."])?);
    h.update(git(&["ls-files", "--others", "--exclude-standard", "--", "."])?);
    if patching() && let Ok(p) = fs::read(patch_file(name)) {
        h.update(b"patch");
        h.update(&p);
    }
    h.update(line.join("\x1f").as_bytes());
    if let Some(t) = target {
        h.update(t.display().to_string().as_bytes());
    }
    Some(format!("{:x}", h.finalize()))
}

/// Python 3 for a toml command that starts with `python`: the py launcher's
/// `py -3` when it works, else `python` on PATH. Neither: a clear error.
fn python() -> Result<Vec<String>, String> {
    static FOUND: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();
    let ok = |args: &[&str]| match crate::runtimes::command(args[0]).args(&args[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(s) => s.success(),
        Err(e) => {
            println!("[launch] {} could not start: {e}", args[0]);
            false
        }
    };
    FOUND
        .get_or_init(|| {
            const CHECK: &str = "import sys; assert sys.version_info >= (3, 10)";
            if ok(&["py", "-3", "-c", CHECK]) {
                return Some(vec!["py".into(), "-3".into()]);
            }
            if ok(&["python", "-c", CHECK]) {
                return Some(vec!["python".into()]);
            }
            // Not on PATH: python.org's per-user install places.
            let programs = std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Programs").join("Python"))?;
            let launcher = programs.join("Launcher").join("py.exe");
            if launcher.is_file() && ok(&[&launcher.to_string_lossy(), "-3", "-c", CHECK]) {
                return Some(vec![launcher.to_string_lossy().into_owned(), "-3".into()]);
            }
            let mut found: Vec<PathBuf> = fs::read_dir(&programs).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("Python3"))).map(|p| p.join("python.exe")).filter(|p| p.is_file()).collect();
            found.sort();
            for exe in found.into_iter().rev() {
                let exe = exe.to_string_lossy().into_owned();
                if ok(&[&exe, "-c", CHECK]) {
                    return Some(vec![exe]);
                }
            }
            println!("[launch] python not found: py -3, python, {} and Python3*\\python.exe there", launcher.display());
            None
        })
        .clone()
        .ok_or_else(|| "Python 3.10 or newer was not found (tried py -3 and python). Install it from python.org with the py launcher, then try again.".to_owned())
}

/// A toml command line with a leading `python` swapped for [`python`].
fn interpreter(cmd: &[String]) -> Result<Vec<String>, String> {
    match cmd.first().map(String::as_str) {
        Some("python" | "python3" | "py") => {
            let rest = if cmd[0] == "py" && cmd.get(1).is_some_and(|a| a.starts_with('-')) { &cmd[2..] } else { &cmd[1..] };
            Ok(python()?.into_iter().chain(rest.iter().cloned()).collect())
        }
        _ => Ok(cmd.to_vec()),
    }
}

/// A Python build (upstream TuxBox's build.py) runs in an x64 Visual Studio
/// developer environment (vcvars64 via vswhere, as Neonbridge's own build
/// does) with the dxc Neonbridge finds (shaders::find_dxc) first on PATH, so
/// choosing the game folder is all it needs. NEONBRIDGE_BUILD_VCVARS=0 runs
/// it in the launcher's own environment.
fn build_env(name: &str, c: &mut Command) -> Result<(), String> {
    if std::env::var("NEONBRIDGE_BUILD_VCVARS").as_deref() == Ok("0") {
        return Ok(());
    }
    let mut env = crate::msvc_env().map_err(|e| format!("runtime {name}: its build needs the Visual Studio x64 C++ tools: {e}"))?;
    if let Some(dir) = crate::shaders::find_dxc().and_then(|d| d.parent().map(Path::to_path_buf)) {
        let key = env.keys().find(|k| k.eq_ignore_ascii_case("PATH")).cloned().unwrap_or_else(|| "PATH".into());
        let path = env.get(&key).cloned().unwrap_or_default();
        env.insert(key, format!("{};{path}", dir.display()));
    }
    println!("[launch] {name}: building in an x64 Visual Studio environment (vcvars64), dxc on PATH");
    c.env_clear().envs(env);
    Ok(())
}

/// The command a run or warm starts: the toml's `launcher` (TuxBox's
/// `python tuxbox.py`, in the tree's folder) when it names one, else the
/// built exe. `exe` then only marks the build's staleness.
fn starter(name: &str, root: &Path, spec: &Spec, program: &Path) -> Result<Command, String> {
    match spec.list("launcher").filter(|l| !l.is_empty()) {
        Some(l) => {
            let l = interpreter(l).map_err(|e| format!("runtime {name}: {e}"))?;
            let mut c = crate::runtimes::command(&l[0]);
            c.args(&l[1..]).current_dir(spec.str("cwd").map(|c| root.join(c)).unwrap_or_else(|| root.to_path_buf()));
            Ok(c)
        }
        None => {
            let mut c = crate::runtimes::command(program);
            c.current_dir(spec.str("cwd").map(|c| root.join(c)).or_else(|| program.parent().map(Path::to_path_buf)).unwrap_or_else(|| root.to_path_buf()));
            Ok(c)
        }
    }
}

/// NEONBRIDGE_PATCHES=0: the trees build as vendored, without vendor/patches.
pub fn patching() -> bool {
    std::env::var("NEONBRIDGE_PATCHES").map_or(true, |v| v.trim() != "0")
}

/// The victory tree a runtime's patches apply to: scarlettport's is its
/// runtime/victory; xcl and tuxbox take none.
/// vendor/patches/rt-<name>.patch, the runtime's own patch.
pub fn patch_file(name: &str) -> PathBuf {
    vendor().join("patches").join(format!("rt-{name}.patch"))
}

/// The runtime's victory tree, if it has one (the mix's parts come from it).
pub fn victory_tree(name: &str) -> Option<PathBuf> {
    patch_root(name, &vendor().join(name))
}

/// Every vendor runtime with a NEONBRIDGE.toml (not tuxbox).
pub fn names() -> Vec<String> {
    rows().into_iter().map(|(n, _, _)| n).filter(|n| n != TUXBOX).collect()
}

/// Where a runtime sits in the picker, best first: its toml's `rank = N`.
/// Neonbridge's own build has no toml and is 10. None: unranked, listed last.
pub fn rank(name: &str) -> Option<u32> {
    if name == TUXBOX {
        return Some(10);
    }
    load(name).ok().and_then(|(_, s)| s.str("rank").and_then(|r| r.trim().parse().ok()))
}

/// A runtime as people know it: "<project> <version or upstream hash>", from
/// its toml's about (up to the first comma, without the licence).
pub fn title(name: &str) -> String {
    if name == TUXBOX {
        return "Neonbridge (own build)".into();
    }
    let about = load(name).ok().and_then(|(_, s)| s.str("about").map(str::to_owned)).unwrap_or_default();
    let head = about.split(',').next().unwrap_or("").trim();
    let head = match head.find(" (GPL").or_else(|| head.find(" (Apache")) {
        Some(i) => head[..i].trim(),
        None => head,
    };
    if head.is_empty() { name.to_owned() } else { head.to_owned() }
}

/// (root, build line) of a runtime.
pub fn spec_of(name: &str) -> Result<(PathBuf, Option<Vec<String>>), Box<dyn Error>> {
    let (root, spec) = load(name)?;
    Ok((root, spec.list("build").map(<[String]>::to_vec)))
}

/// The runtime's [controls] rows (tuxbox: its own map).
pub fn controls_of(name: &str) -> Vec<(String, String)> {
    if name == TUXBOX {
        return crate::controls::tuxbox_rows();
    }
    let Ok((_, spec)) = load(name) else { return Vec::new() };
    crate::controls::rows(|k| spec.str(&format!("controls.{k}")).map(str::to_owned))
}

/// The runtime's options entries ("KEY=values (default X): text").
pub fn options_of(name: &str) -> Vec<String> {
    load(name).ok().and_then(|(_, s)| s.list("options").map(<[String]>::to_vec)).unwrap_or_default()
}

/// Runs a cargo line in `cwd` with `target` as its target folder, hidden,
/// echoed, with its Compiling lines on the loader bar. Err: the compile
/// errors (the `error` blocks), for the mix's incompatible.txt.
pub fn cargo(cmd: &[String], cwd: &Path, target: &Path, label: &str) -> Result<(), String> {
    crate::splash::status(0.1, label);
    let mut c = crate::runtimes::command(&cmd[0]);
    c.args(&cmd[1..]).current_dir(cwd).env("CARGO_TARGET_DIR", target).env("CARGO_TERM_PROGRESS_WHEN", "always").env("CARGO_TERM_PROGRESS_WIDTH", "100").stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = hidden(&mut c).spawn().map_err(|e| format!("{}: {e}", cmd[0]))?;
    let out = child.stdout.take().map(|o| std::thread::spawn(move || {
        for l in BufReader::new(o).lines().map_while(Result::ok) {
            println!("{l}");
        }
    }));
    let (mut errors, mut in_error) = (Vec::new(), false);
    crate::progress::build_lines(child.stderr.take().unwrap(), label, 0.1, 0.45, |l| {
        eprintln!("{l}");
        let l = l.to_owned();
        if l.starts_with("error") {
            in_error = true;
        } else if l.starts_with("warning") || l.trim_start().starts_with("Compiling") {
            in_error = false;
        }
        if in_error && errors.len() < 80 {
            errors.push(l);
        }
    });
    let _ = out.map(|t| t.join());
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        s => Err(if errors.is_empty() { format!("cargo ended {s:?}") } else { errors.join("\n") }),
    }
}

fn patch_root(name: &str, root: &Path) -> Option<PathBuf> {
    match name {
        "victory" | "vitoriabr" => Some(root.to_path_buf()),
        "scarlettport" => Some(root.join("runtime").join("victory")),
        _ => None,
    }
}

/// Applies the runtime's own vendor/patches/rt-<name>.patch to its victory
/// tree for the build, unless it is already in (its reverse check passes):
/// `git apply --ignore-whitespace` from the repository's top with
/// `--directory=<tree>` (git apply in a subfolder silently skips the files).
/// A patch that does not apply is an error: the build would ship without it.
/// [`unpatch`] takes it back after the build, so the vendored tree stays as
/// its upstream ships it.
fn patch(name: &str, root: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let Some(tree) = patch_root(name, root).filter(|_| patching()) else { return Ok(Vec::new()) };
    let f = patch_file(name);
    if !f.is_file() {
        return Ok(Vec::new());
    }
    // canonicalize gives a \\?\ verbatim path, which git cannot open: plain,
    // with forward slashes.
    let f = plain(&fs::canonicalize(&f).unwrap_or(f));
    crate::splash::status(0.05, &format!("Applying Neonbridge patches to {name}"));
    if git_apply(&tree, &f, &["--check", "-R"]).0 {
        println!("[launch] {name}: {} is already applied", f.display());
        return Ok(Vec::new());
    }
    let (ok, err) = git_apply(&tree, &f, &[]);
    if !ok {
        return Err(format!("runtime {name}: {} does not apply to {}\n{err}\n(NEONBRIDGE_PATCHES=0 builds without it)", f.display(), tree.display()).into());
    }
    println!("[launch] {name}: applied {}", f.display());
    Ok(vec![f])
}

/// `path` without the `\\?\` verbatim prefix, with forward slashes.
pub fn plain(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\UNC\").map(|r| format!(r"\\{r}")).unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned());
    PathBuf::from(s.replace('\\', "/"))
}

fn unpatch(name: &str, root: &Path, applied: &[PathBuf]) {
    let Some(tree) = patch_root(name, root) else { return };
    for f in applied.iter().rev() {
        if !git_apply(&tree, f, &["-R"]).0 {
            println!("[launch] {name}: could not revert {} in {}", f.display(), tree.display());
        }
    }
}

/// (success, output) of a hidden command.
pub fn quiet(mut c: Command) -> (bool, String) {
    match hidden(c.stdin(Stdio::null())).output() {
        Ok(o) => (o.status.success(), format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr)).trim().to_owned()),
        Err(e) => (false, e.to_string()),
    }
}

fn git_apply(tree: &Path, patch: &Path, extra: &[&str]) -> (bool, String) {
    let mut top = crate::runtimes::command("git");
    top.args(["rev-parse", "--show-toplevel"]).current_dir(tree);
    let (ok, top) = quiet(top);
    let top = PathBuf::from(top);
    let Some(rel) = ok.then(|| fs::canonicalize(tree).ok().zip(fs::canonicalize(&top).ok())).flatten()
        .and_then(|(t, r)| t.strip_prefix(&r).ok().map(|p| p.to_string_lossy().replace('\\', "/")))
    else {
        return (false, format!("{} is not inside a git checkout", tree.display()));
    };
    let mut c = crate::runtimes::command("git");
    c.args(["apply", "--ignore-whitespace"]).args(extra).arg(format!("--directory={rel}")).arg(patch).current_dir(&top);
    quiet(c)
}

/// An error the loader window cannot show: a message box, so a double-click
/// launch (no console) still says why it stopped.
pub fn fail_box(e: &dyn std::fmt::Display) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
    let wide = |t: &str| t.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
    let (text, title) = (wide(&e.to_string()), wide("Neonbridge"));
    // SAFETY: both strings are NUL-terminated and live across the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_OK | MB_ICONERROR) };
}

fn agility(name: &str, exe: &Path) {
    let Some(to) = exe.parent().map(|d| d.join("D3D12")) else { return };
    // NEONBRIDGE_AGILITY_DIR first (an empty one is unset), then the default;
    // a set one without D3D12Core.dll no longer hides the default.
    let tried: Vec<PathBuf> = [std::env::var_os("NEONBRIDGE_AGILITY_DIR").filter(|v| !v.is_empty()).map(PathBuf::from), Some(pack().join("agility-1.614.1"))].into_iter().flatten().collect();
    let Some(from) = tried.iter().find(|d| d.join("D3D12Core.dll").is_file()).cloned() else {
        if to.join("D3D12Core.dll").is_file() {
            return;
        }
        let list = tried.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", ");
        println!("[launch] {name}: no D3D12 Agility SDK (no D3D12Core.dll in {list}); the system's D3D12 is used");
        return;
    };
    let _ = fs::create_dir_all(&to);
    for n in ["D3D12Core.dll", "d3d12SDKLayers.dll"] {
        let (src, dst) = (from.join(n), to.join(n));
        let same = fs::metadata(&src).ok().zip(fs::metadata(&dst).ok()).is_some_and(|(a, b)| a.len() == b.len());
        if !same && let Err(e) = fs::copy(&src, &dst) {
            println!("[launch] {name}: copying {n} from {}: {e}", from.display());
        }
    }
}

pub fn pack_dir() -> PathBuf {
    pack()
}

fn pack() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack")
}

/// The victory family's one cache, %LOCALAPPDATA%\6ixpack\cache (a real
/// folder): VICTORY_CACHE_DIR of victory, vitoriabr and
/// scarlettport. shaders\ and images\ are shared as they are, since each tree
/// keys its entries on its own translator identity. What would clash
/// (pipelines\host-pipelines.bin, early-pipelines.txt, precompiled\) the rt
/// patches name per runtime from NEONBRIDGE_RUNTIME, which the launcher sets.
fn family_cache() -> PathBuf {
    std::env::var_os("NEONBRIDGE_CACHE").map(|d| PathBuf::from(d).join("cache")).unwrap_or_else(|| pack().join("cache"))
}

/// Same bytes in both files.
fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(x), Ok(y)) if x.len() == y.len() => fs::read(a).ok().zip(fs::read(b).ok()).is_some_and(|(x, y)| x == y),
        _ => false,
    }
}

#[derive(Default, Debug, PartialEq)]
struct Moved {
    moved: u64,
    duplicates: u64,
    kept: u64,
}

/// Moves `from` (a file or a folder, walked file by file) to `to`. A file
/// whose name is free is renamed, or copied, verified byte for byte and only
/// then deleted; one already there with the same bytes is a duplicate and
/// the source goes; one there with other bytes is left where it is. A
/// junction or link is unlinked (never walked into or deleted through);
/// emptied folders are removed, others stay.
fn move_verified(from: &Path, to: &Path, n: &mut Moved) {
    let Ok(meta) = fs::symlink_metadata(from) else { return };
    if meta.file_type().is_symlink() {
        let _ = fs::remove_dir(from).or_else(|_| fs::remove_file(from));
        return;
    }
    if meta.is_dir() {
        let _ = fs::create_dir_all(to);
        for e in fs::read_dir(from).into_iter().flatten().flatten() {
            move_verified(&e.path(), &to.join(e.file_name()), n);
        }
        let _ = fs::remove_dir(from);
        return;
    }
    if to.exists() {
        if same_file(from, to) {
            n.duplicates += 1;
            let _ = fs::remove_file(from);
        } else {
            n.kept += 1;
        }
        return;
    }
    if let Some(d) = to.parent() {
        let _ = fs::create_dir_all(d);
    }
    if fs::rename(from, to).is_ok() {
        n.moved += 1;
    } else if fs::copy(from, to).is_ok() && same_file(from, to) {
        n.moved += 1;
        let _ = fs::remove_file(from);
    } else {
        let _ = fs::remove_file(to);
        n.kept += 1;
    }
}

/// Moves the old caches into [`family_cache`], file by file: each victory
/// tree's 6ixpack\<name>\cache and the junction era's 6ixpack\victory-shared.
/// shaders\ and images\ merge as they are; pipelines\host-pipelines.bin and
/// early-pipelines.txt become host-pipelines-<name>.bin and
/// early-pipelines-<name>.txt, precompiled\ becomes precompiled-<name>\.
/// press.txt, warm-total.txt and last-run.log stay in the old folder (they
/// are the runtime's own). On a clean machine there is nothing to move.
fn migrate_caches(to: &Path) {
    let mut olds: Vec<(Option<String>, PathBuf)> = ["victory", "vitoriabr", "scarlettport"].iter().map(|n| (Some((*n).to_owned()), cache(n))).collect();
    olds.push((None, pack().join("victory-shared")));
    for (name, old) in olds {
        if !old.exists() || old == to {
            continue;
        }
        let mut n = Moved::default();
        for sub in ["shaders", "images"] {
            move_verified(&old.join(sub), &to.join(sub), &mut n);
        }
        if let Some(name) = &name {
            let p = old.join("pipelines");
            move_verified(&p.join("host-pipelines.bin"), &to.join("pipelines").join(format!("host-pipelines-{name}.bin")), &mut n);
            move_verified(&p.join("early-pipelines.txt"), &to.join("pipelines").join(format!("early-pipelines-{name}.txt")), &mut n);
            move_verified(&p, &to.join("pipelines"), &mut n);
            move_verified(&old.join("precompiled"), &to.join(format!("precompiled-{name}")), &mut n);
        }
        if name.is_none() {
            let _ = fs::remove_dir(&old);
        }
        if n != Moved::default() {
            println!("[launch] cache {}: {} files moved to {}, {} duplicates dropped, {} left there (a different file has the name)", old.display(), n.moved, to.display(), n.duplicates, n.kept);
        }
    }
}

/// The victory family's one save folder, %LOCALAPPDATA%\6ixpack\save
/// (VICTORY_DATA_DIR): <TITLEID>\{storage,saves,temp}, storage\ holding the
/// first-boot settings.
fn family_saves() -> PathBuf {
    pack().join("save")
}

/// Moves the trees' old save folders into [`family_saves`], file by file
/// (copied, verified, then deleted; a file already there is never
/// overwritten). Newest first, so on a clash the first source's file stays:
/// 6ixpack\victory\data (fxceless runs), %LOCALAPPDATA%\victory\data
/// (vitoriabr's and scarlettport's default).
/// ScarlettPort reads <TITLEID>\slots\default\{storage,saves} instead when
/// that folder exists (its core saves.rs:60), so those merge into
/// <TITLEID>\{storage,saves} where a file is missing and slots\default is
/// renamed slots\default-migrated: all four trees then read one place.
fn migrate_saves(to: &Path) {
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    for old in [pack().join("victory").join("data"), local.join("victory").join("data")] {
        if !old.exists() || old == to {
            continue;
        }
        let mut n = Moved::default();
        for e in fs::read_dir(&old).into_iter().flatten().flatten() {
            move_verified(&e.path(), &to.join(e.file_name()), &mut n);
        }
        let _ = fs::remove_dir(&old);
        if n != Moved::default() {
            println!("[launch] saves {}: {} files moved to {}, {} duplicates dropped, {} left there (a different file has the name)", old.display(), n.moved, to.display(), n.duplicates, n.kept);
        }
    }
    for title in fs::read_dir(to).into_iter().flatten().flatten() {
        let slot = title.path().join("slots").join("default");
        if !slot.is_dir() {
            continue;
        }
        let mut n = Moved::default();
        for sub in ["storage", "saves"] {
            move_verified(&slot.join(sub), &title.path().join(sub), &mut n);
        }
        let done = title.path().join("slots").join("default-migrated");
        if !done.exists() && fs::rename(&slot, &done).is_ok() {
            println!("[launch] saves: {} merged into {} ({} moved, {} already there); renamed to slots\\default-migrated", slot.display(), title.path().display(), n.moved, n.duplicates + n.kept);
        }
    }
}

/// xcl's DXIL cache (XCL_DXIL_CACHE): %LOCALAPPDATA%\6ixpack\cache\xcl. Its
/// own default, %LOCALAPPDATA%\xcl\dxil, is moved there (a rename) the first
/// time. NEONBRIDGE_SHADER_ROOT_MOVE=0 keeps the old one.
fn xcl_cache() -> PathBuf {
    let new = family_cache().join("xcl");
    let old = std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("xcl").join("dxil"));
    if std::env::var("NEONBRIDGE_SHADER_ROOT_MOVE").is_ok_and(|v| v.trim() == "0") {
        return old.unwrap_or(new);
    }
    if let Some(old) = old.filter(|o| o.is_dir()) && !new.exists() {
        let _ = fs::create_dir_all(family_cache());
        match fs::rename(&old, &new) {
            Ok(()) => println!("[launch] xcl: moved its shader cache {} -> {}", old.display(), new.display()),
            Err(e) => println!("[launch] xcl: could not move {} -> {}: {e}", old.display(), new.display()),
        }
    }
    new
}

pub fn cache_of(name: &str) -> PathBuf {
    cache(name)
}

fn cache(name: &str) -> PathBuf {
    std::env::var_os("NEONBRIDGE_CACHE").map(|d| PathBuf::from(d).join(name)).unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("6ixpack").join(name).join("cache")
    })
}

struct Places {
    game: String,
    game_exe: String,
    cache: String,
    jobs: String,
    press: String,
}

/// `%NAME%` replaced by that environment variable, as cmd.exe would; an
/// unset one stays as written. A toml value like "%LOCALAPPDATA%\6ixpack\save"
/// reached Flamingo literally, a relative path that resolved inside its tree.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 && after[..j].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'(' || b == b')') => {
                match std::env::var(&after[..j]) {
                    Ok(v) => out.push_str(&v),
                    Err(_) => out.push_str(&rest[i..i + j + 2]),
                }
                rest = &after[j + 1..];
            }
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn fill(s: &str, p: &Places) -> String {
    expand_env(s).replace("{game_exe}", &p.game_exe).replace("{game}", &p.game).replace("{cache}", &p.cache).replace("{jobs}", &p.jobs).replace("{press}", &p.press)
}

/// Runs `cmd` with stdout and stderr into `log` (or this console), adding
/// run.sh's heartbeat lines; kills it after `timeout` seconds.
/// NEONBRIDGE_LOG=0 is the only way to turn run logging off.
pub fn logging() -> bool {
    std::env::var("NEONBRIDGE_LOG").map_or(true, |v| v.trim() != "0")
}

fn supervised(name: &str, mut cmd: Command, log: Option<&Path>, timeout: Option<f64>, echo: bool) -> Result<(), Box<dyn Error>> {
    let sink: Arc<Mutex<Box<dyn Write + Send>>> = Arc::new(Mutex::new(match log {
        Some(p) => {
            if let Some(d) = p.parent() {
                let _ = fs::create_dir_all(d);
            }
            // Appended: a picker launch's own lines are in it already.
            Box::new(fs::OpenOptions::new().create(true).append(true).open(p)?)
        }
        None => Box::new(std::io::stderr()),
    }));
    let mut child = hidden(cmd.stdout(Stdio::piped()).stderr(Stdio::piped())).spawn()?;
    let frames = Arc::new(Mutex::new((0u64, false)));
    let pump = |r: Box<dyn std::io::Read + Send>| {
        let (sink, frames, tag) = (sink.clone(), frames.clone(), name.to_owned());
        std::thread::spawn(move || {
            for line in BufReader::new(r).lines().map_while(Result::ok) {
                let mut out = sink.lock().unwrap_or_else(|e| e.into_inner());
                let _ = writeln!(out, "{line}");
                if echo {
                    eprintln!("{line}");
                }
                let mut f = frames.lock().unwrap_or_else(|e| e.into_inner());
                crate::fps::feed(&line);
                if !crate::splash::closed() && let Some((d, m, title)) = crate::progress::setup_line(&line) {
                    let label = match title {
                        Some(t) => format!("Setting up ({tag}): {t}"),
                        None => format!("Setting up ({tag})"),
                    };
                    crate::splash::step(0.8, 0.95, &label, d, Some(m));
                }
                // The loader stays up until the title presents its first frame.
                if line.contains("PresentX #") || line.contains("frame times: ") || fps(&line).is_some() {
                    crate::splash::close();
                }
                if let Some(n) = line.split("frame times: ").nth(1).and_then(|s| s.split(' ').next()).and_then(|s| s.parse::<u64>().ok()) {
                    f.0 += n;
                    f.1 = true;
                } else if let Some(fps) = fps(&line) {
                    if !f.1 {
                        f.0 += fps.round() as u64;
                    }
                    let _ = writeln!(out, "[{tag}] heartbeat: completed PresentX #{} ({fps:.1} fps)", f.0);
                }
                let _ = out.flush();
            }
        })
    };
    let a = pump(Box::new(child.stdout.take().unwrap()));
    let b = pump(Box::new(child.stderr.take().unwrap()));
    let started = std::time::Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if timeout.is_some_and(|t| t > 0.0 && started.elapsed().as_secs_f64() >= t) {
            println!("[launch] {name}: --timeout reached; stopping it");
            let _ = child.kill();
            break child.wait()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let _ = (a.join(), b.join());
    crate::splash::close();
    println!("[launch] {name} exited ({status})");
    if !status.success() {
        let code = status.code().map_or_else(|| status.to_string(), |c| format!("{c} (0x{:08X})", c as u32));
        return Err(format!("{name} exited with code {code}.\nLog: {}", log.map_or_else(|| "this console".to_owned(), |p| p.display().to_string())).into());
    }
    Ok(())
}

/// One line of a warm step, for the loader: (records or shaders done, the
/// run's total if the line gives it).
#[derive(Debug, PartialEq)]
enum WarmLine {
    /// victory's `--warm`: `warm-up  <n> shaders so far, <t> s`.
    Shaders(u64),
    /// ScarlettPort's precompile-graphics: `<t> s  <done> records: <made> made, ...`.
    Records(u64),
    /// Its last line: `precompile-graphics  <total> records: ...`.
    Total(u64),
    /// A pipeline made (INFO `host mesh pipeline`): a tick between record lines.
    Pipeline,
    /// xcl's `precompiling <total> shaders into ...`.
    Planned(u64),
    Other,
}

/// Keeps a copy of a runtime's shader/pipeline cache in
/// %LOCALAPPDATA%\6ixpack\cache-backup\<same path> (or NEONBRIDGE_CACHE_BACKUP=<dir>;
/// =0 turns it off), so hours of compiling survive a deleted or clobbered cache.
/// robocopy in the background at idle priority: only new or changed files are
/// copied, nothing is ever deleted from the backup.
fn backup_cache(cache: &Path) {
    let setting = std::env::var("NEONBRIDGE_CACHE_BACKUP").unwrap_or_default();
    if setting.trim() == "0" {
        return;
    }
    let base = pack();
    let rel = cache.strip_prefix(&base).map(Path::to_path_buf).unwrap_or_else(|_| PathBuf::from(cache.file_name().unwrap_or_default()));
    let root = if setting.trim().is_empty() { base.join("cache-backup") } else { PathBuf::from(setting.trim()) };
    let to = root.join(rel);
    let from = cache.to_path_buf();
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        // 0x40 IDLE_PRIORITY_CLASS, 0x0800_0000 CREATE_NO_WINDOW
        let r = Command::new("robocopy").arg(&from).arg(&to).args(["/E", "/XO", "/R:0", "/W:0", "/NFL", "/NDL", "/NJH", "/NJS", "/NP", "/MT:2"])
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).creation_flags(0x40 | 0x0800_0000).status();
        // robocopy: 0-7 success, 8+ failure
        match r.map(|s| s.code().unwrap_or(16)) {
            Ok(c) if c < 8 => println!("[launch] cache backed up to {}", to.display()),
            other => println!("[launch] cache backup to {} failed ({other:?})", to.display()),
        }
    });
}

fn warm_line(l: &str) -> WarmLine {
    let number_before = |at: usize| l[..at].trim_end().rsplit(' ').next().and_then(|n| n.parse().ok());
    let t = l.trim();
    if let Some(n) = t.strip_prefix("precompiling ").and_then(|r| r.split_once(" shaders")).and_then(|(n, _)| n.parse().ok()) {
        return WarmLine::Planned(n);
    }
    if let Some(at) = l.find(" records: ") {
        if let Some(n) = number_before(at) {
            return if l.trim_start().starts_with("precompile-graphics") { WarmLine::Total(n) } else { WarmLine::Records(n) };
        }
    }
    if l.contains("host mesh pipeline") && !l.contains("refused") {
        return WarmLine::Pipeline;
    }
    match l.find(" shaders").and_then(number_before) {
        Some(n) => WarmLine::Shaders(n),
        None => WarmLine::Other,
    }
}

/// The warm step, hidden, its output echoed. The bar moves from 0.5 to 0.8
/// by its own counts: records done against the total the last warm of this
/// runtime ended with (warm-total.txt in its cache; none the first time, so
/// an estimate that slows as it goes), pipelines made between record lines,
/// or victory's shader counts. The status line is ours, never a raw log line.
/// A failed warm stops the launch: Err carries its last lines.
fn warm(name: &str, mut cmd: Command, cache: &Path, to: f32) -> Result<(), String> {
    let mut child = hidden(cmd.stdout(Stdio::piped()).stderr(Stdio::piped())).spawn().map_err(|e| format!("{name}: the warm step did not start: {e}"))?;
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let pump = |r: Box<dyn std::io::Read + Send>, err: bool, tx: std::sync::mpsc::Sender<String>| {
        std::thread::spawn(move || {
            for l in BufReader::new(r).lines().map_while(Result::ok) {
                if err { eprintln!("{l}") } else { println!("{l}") }
                let _ = tx.send(l);
            }
        })
    };
    let a = pump(Box::new(child.stdout.take().unwrap()), false, tx.clone());
    let b = pump(Box::new(child.stderr.take().unwrap()), true, tx);
    let saved = cache.join("warm-total.txt");
    let mut total: Option<u64> = fs::read_to_string(&saved).ok().and_then(|t| t.trim().parse().ok()).filter(|&t| t > 0);
    let (mut records, mut ticks, mut last_total, mut seen) = (0u64, 0u64, None, 0u64);
    for l in rx {
        if tail.len() == 12 {
            tail.pop_front();
        }
        tail.push_back(l.clone());
        // A done/total count (xcl, TuxBox, Five Star) carries its own total.
        if let Some((d, Some(m))) = crate::progress::count(&l) {
            total = Some(m);
            seen = seen.max(d);
            crate::splash::step(0.5, to, &format!("Warming the shaders ({name}): translating shaders"), d, total);
            continue;
        }
        // xcl's --pipelines pass after its shaders: "  pipelines: 800 made, 0 refused"
        // (no total): the count and a pulse, so the bar doesn't sit on the last shader count.
        if let Some(n) = l.trim().strip_prefix("pipelines: ").and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<u64>().ok()) {
            crate::splash::step(0.5, to, &format!("Warming the shaders ({name}): building GPU pipelines"), n, None);
            continue;
        }
        let (done, what) = match warm_line(&l) {
            WarmLine::Records(n) => {
                (records, ticks) = (n, 0);
                (n, "compiling pipelines")
            }
            WarmLine::Pipeline => {
                ticks += 1;
                (records + ticks, "compiling pipelines")
            }
            WarmLine::Total(n) => {
                last_total = Some(n);
                (n, "compiling pipelines")
            }
            WarmLine::Shaders(n) => (n, "translating shaders"),
            WarmLine::Planned(n) => {
                total = Some(n).filter(|&n| n > 0);
                (0, "translating shaders")
            }
            WarmLine::Other => continue,
        };
        seen = seen.max(done);
        // No total (victory's "N shaders so far" the first time): the count
        // and a pulse, and this run's count is the next run's total.
        crate::splash::step(0.5, to, &format!("Warming the shaders ({name}): {what}"), done, total);
    }
    let _ = (a.join(), b.join());
    if let Some(t) = last_total.or((seen > 0).then_some(seen)) {
        let _ = fs::write(&saved, t.to_string());
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        s => Err(format!("{name}: the shader warm failed ({}), so the game was not started. Its last lines:\n{}", s.map_or_else(|e| e.to_string(), |s| s.to_string()), tail.into_iter().collect::<Vec<_>>().join("\n"))),
    }
}

/// Records per precompile-graphics run (ScarlettPort engine::CHUNK).
const GRAPHICS_CHUNK: u64 = 1000;

/// How long a precompile-graphics run may print nothing before it is
/// stopped and its records left to compile in game (ScarlettPort's
/// slow_secs, 60 s by default). NEONBRIDGE_GRAPHICS_IDLE_S sets it.
fn graphics_idle() -> std::time::Duration {
    std::time::Duration::from_secs(std::env::var("NEONBRIDGE_GRAPHICS_IDLE_S").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(60))
}

enum Chunk {
    /// Its summary's record count (a run with no summary counts as a full chunk).
    Done(u64),
    /// Nothing printed for graphics_idle(): stopped.
    Idle,
}

/// One precompile-graphics chunk, hidden, its output echoed, the bar moved
/// by its record lines (`<t> s  <done> records: ...`).
fn graphics_chunk(name: &str, mut cmd: Command, skip: u64, all: Option<u64>) -> Result<Chunk, String> {
    let mut child = hidden(cmd.stdout(Stdio::piped()).stderr(Stdio::piped())).spawn().map_err(|e| format!("{name}: precompile-graphics did not start: {e}"))?;
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let pump = |r: Box<dyn std::io::Read + Send>, err: bool, tx: std::sync::mpsc::Sender<String>| {
        std::thread::spawn(move || {
            for l in BufReader::new(r).lines().map_while(Result::ok) {
                if err { eprintln!("{l}") } else { println!("{l}") }
                let _ = tx.send(l);
            }
        })
    };
    let a = pump(Box::new(child.stdout.take().unwrap()), false, tx.clone());
    let b = pump(Box::new(child.stderr.take().unwrap()), true, tx);
    let mut tail: std::collections::VecDeque<String> = std::collections::VecDeque::new();
    let mut total = None;
    loop {
        match rx.recv_timeout(graphics_idle()) {
            Ok(l) => {
                if tail.len() == 12 {
                    tail.pop_front();
                }
                tail.push_back(l.clone());
                let done = match warm_line(&l) {
                    WarmLine::Records(n) => n,
                    WarmLine::Total(n) => {
                        total = Some(n);
                        n
                    }
                    _ => continue,
                };
                // Records over every chunk's total (the last full pass's, in
                // graphics-total.txt); none yet: the count and a pulse.
                crate::splash::step(0.65, 0.8, &format!("Warming the shaders ({name}): compiling graphics pipelines"), skip + done, all);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(Chunk::Idle);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = (a.join(), b.join());
    match child.wait() {
        Ok(s) if s.success() => Ok(Chunk::Done(total.unwrap_or(GRAPHICS_CHUNK))),
        s => Err(format!("{name}: precompile-graphics failed ({}), so the game was not started. Its last lines:\n{}", s.map_or_else(|e| e.to_string(), |s| s.to_string()), tail.into_iter().collect::<Vec<_>>().join("\n"))),
    }
}

/// The fps of a frame-rate line: `<prefix> 12.3 fps, 81.3 ms/frame ...`.
fn fps(line: &str) -> Option<f64> {
    let at = line.find(" fps, ")?;
    line[..at].rsplit(' ').next()?.parse().ok()
}

pub struct Choice<'a> {
    pub preset: Option<&'a str>,
    pub sets: &'a [String],
    /// Mix mode: the build is this mix's (mix.rs); `name` is its app tree.
    pub mix: Option<&'a crate::mix::Mix>,
}

#[allow(clippy::too_many_arguments)]
pub fn run(name: &str, task: Task, game: Option<&Path>, exe: Option<&str>, log: Option<PathBuf>, timeout: Option<f64>, game_args: &[std::ffi::OsString], choice: Choice) -> Result<(), Box<dyn Error>> {
    let (root, spec) = load(name)?;
    let sets = settings(&spec, choice.preset, choice.sets)?;
    // The loader window (NEONBRIDGE_SPLASH=0: none): patches, build, setup,
    // the shader warm, then the launch; it closes at the first frame.
    // Logging is on for every run (NEONBRIDGE_LOG=0 turns it off): with no
    // --log (a CLI run), 6ixpack\logs\<runtime>_<time>.log, the output also
    // echoed to this console.
    let echo = log.is_none();
    let log = log.or_else(|| {
        (logging() && matches!(task, Task::Run | Task::Prewarm)).then(|| {
            let secs = SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let p = crate::data_dir().join("logs").join(format!("{name}_{secs}.log"));
            println!("[launch] log: {}", p.display());
            p
        })
    });
    let echo = echo && log.is_some();
    // The loader names the tree that actually runs: a mix, or the runtime's
    // own (its about line up to the licence).
    if matches!(task, Task::Run) {
        let what = match choice.mix {
            Some(m) => format!("a mix (app from {})", m.app().unwrap_or(name)),
            None => {
                let about = about(name);
                let tree = about.split(" (").next().unwrap_or("").split(',').next().unwrap_or("").trim().to_owned();
                if tree.is_empty() { name.to_owned() } else { format!("{name}: {tree}") }
            }
        };
        println!("[launch] runs {what}");
        crate::splash::show(&format!("Preparing {what}"));
    }
    let built = match choice.mix {
        Some(m) => crate::mix::build(m),
        None => build(name, &root, &spec),
    };
    let program = built.inspect_err(|e| {
        crate::splash::close();
        if matches!(task, Task::Run) {
            fail_box(e);
        }
    })?;
    if spec.flag("agility") {
        crate::splash::status(0.45, "Copying the D3D12 Agility SDK");
        agility(name, &program);
    }
    let Some(game) = game else {
        return match task {
            Task::Build => Ok(()),
            _ => Err(format!("runtime {name}: --game is needed").into()),
        };
    };
    let cache = cache(name);
    let _ = fs::create_dir_all(&cache);
    backup_cache(&cache); // what the last run added
    // Verify the dump before a run (read-only, ScarlettPort's verify): problems are
    // printed and a bad dump is named on the loader; the run still goes ahead.
    // NEONBRIDGE_VERIFY=0 skips it.
    if matches!(task, Task::Run) && std::env::var("NEONBRIDGE_PREFLIGHT").map_or(true, |v| v.trim() != "0") && crate::preflight::print(&cache, false) == crate::preflight::Level::Bad {
        crate::splash::status(0.01, "This PC is missing something the runtimes need (see the log); starting anyway");
    }
    if matches!(task, Task::Run) && std::env::var("NEONBRIDGE_VERIFY").map_or(true, |v| v.trim() != "0") {
        let dir = if game.is_dir() { game.to_path_buf() } else { game.parent().map(Path::to_path_buf).unwrap_or_default() };
        if crate::verify::print(&dir, false) == crate::verify::Level::Bad {
            crate::splash::status(0.02, "Game files look incomplete (see the log); starting anyway");
        }
    }
    if matches!(task, Task::Run) && choice.mix.is_none() {
        auto_setup(name, &root, &spec, &game);
    }
    let family = victory_tree(name).is_some();
    // The victory family's cache is the one shared folder; this runtime's
    // own files (press.txt, warm-total.txt, last-run.log) stay in its own.
    let data = if family {
        let shared = family_cache();
        let _ = fs::create_dir_all(&shared);
        migrate_caches(&shared);
        let saves = family_saves();
        let _ = fs::create_dir_all(&saves);
        migrate_saves(&saves);
        shared
    } else {
        cache.clone()
    };
    let press = cache.join("press.txt");
    let _ = fs::write(&press, "");
    let places = Places {
        game: game.display().to_string(),
        game_exe: game.join(exe.unwrap_or(GAME_EXE)).display().to_string(),
        cache: data.display().to_string(),
        jobs: std::thread::available_parallelism().map_or(1, |n| n.get().saturating_sub(1).max(1)).to_string(),
        press: press.display().to_string(),
    };
    let mut cmd = match starter(name, &root, &spec, &program) {
        Ok(c) => c,
        Err(e) => {
            crate::splash::close();
            if matches!(task, Task::Run) {
                fail_box(&e);
            }
            return Err(e.into());
        }
    };
    let mut env: Vec<(String, String)> = Vec::new();
    if let Some(k) = spec.str("game_env") {
        env.push((k.to_owned(), places.game.clone()));
    }
    if let Some(k) = spec.str("cache_env") {
        env.push((k.to_owned(), places.cache.clone()));
    }
    // The rt patches name the per-runtime cache files from it.
    if family {
        env.push(("NEONBRIDGE_RUNTIME".to_owned(), name.to_owned()));
        if diag() {
            env.push(("VICTORY_D3D12_DRED".to_owned(), "1".to_owned()));
        }
        // Runtime logging on by default (victory's own VICTORY_LOG level too).
        if logging() {
            env.push(("VICTORY_LOG".to_owned(), "info".to_owned()));
        }
        // One save folder for the family (mix runs too); a toml or preset may still name another.
        env.push(("VICTORY_DATA_DIR".to_owned(), family_saves().display().to_string()));
    }
    env.extend(spec.env.iter().map(|(k, v)| (k.clone(), fill(v, &places))));
    if spec.flag("own_overlay") {
        crate::fps::OWN_OVERLAY.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if name == "xcl" && std::env::var_os("XCL_DXIL_CACHE").is_none() {
        env.push(("XCL_DXIL_CACHE".to_owned(), xcl_cache().display().to_string()));
    }
    if logging() && !env.iter().any(|(k, _)| k == "RUST_LOG") {
        env.push(("RUST_LOG".to_owned(), "info".to_owned()));
    }
    for (k, v) in env {
        if std::env::var_os(&k).is_none() {
            cmd.env(k, v);
        }
    }
    for (k, v) in sets.iter().filter(|(k, _)| !k.starts_with("--")) {
        cmd.env(k, fill(v, &places));
    }
    let args: Vec<String> = match task {
        Task::Build => return Ok(()),
        Task::Prewarm => match spec.list("prewarm") {
            Some(a) => a.iter().map(|a| fill(a, &places)).collect(),
            None => {
                println!("[launch] {name}: no prewarm in its NEONBRIDGE.toml; nothing to do");
                return Ok(());
            }
        },
        Task::Run => {
            let mut a: Vec<String> = spec.list("run").unwrap_or_default().iter().map(|a| fill(a, &places)).collect();
            for (flag, v) in sets.iter().filter(|(k, _)| k.starts_with("--")) {
                let v = fill(v, &places);
                if let Some(i) = a.iter().position(|x| x == flag) {
                    if a.get(i + 1).is_some_and(|n| !n.starts_with("--")) { a[i + 1] = v } else { a.insert(i + 1, v) }
                } else if let Some(i) = a.iter().position(|x| x.starts_with(&format!("{flag}="))) {
                    a[i] = format!("{flag}={v}");
                } else if v.is_empty() || v == "1" || v == "true" {
                    a.push(flag.clone());
                } else {
                    a.extend([flag.clone(), v]);
                }
            }
            a
        }
    };
    cmd.args(&args);
    if matches!(task, Task::Run) && !game_args.is_empty() {
        cmd.arg("--").args(game_args);
    }
    // A run warms the shaders first with the toml's prewarm (NEONBRIDGE_WARM=0
    // skips it): the same program, environment and cache, its "N shaders so
    // far" lines moving the bar.
    if matches!(task, Task::Run) && std::env::var("NEONBRIDGE_WARM").map_or(true, |v| v.trim() != "0") && let Some(w) = spec.list("prewarm") {
        let w: Vec<String> = w.iter().map(|a| fill(a, &places)).collect();
        crate::splash::status(0.5, "Warming the shaders");
        println!("[launch] {name} warm > {} {}", program.display(), w.join(" "));
        let mut wc = starter(name, &root, &spec, &program)?;
        wc.args(&w).current_dir(cmd.get_current_dir().map(Path::to_path_buf).unwrap_or_else(|| root.clone()));
        for (k, v) in cmd.get_envs() {
            match v {
                Some(v) => wc.env(k, v),
                None => wc.env_remove(k),
            };
        }
        let graphics = std::env::var("NEONBRIDGE_PRECOMPILE_GRAPHICS").map_or(true, |v| v.trim() != "0") && spec.list("prewarm_graphics").is_some();
        if let Err(e) = warm(name, wc, &cache, if graphics { 0.65 } else { 0.8 }) {
            crate::splash::close();
            fail_box(&e);
            return Err(e.into());
        }
        backup_cache(&cache);
        // ScarlettPort's second phase (launcher 0.3, shaderbuild.rs:272):
        // precompile-graphics in chunks of 1000 records, {skip} = 0, 1000,
        // ... until a chunk's summary gives fewer than 1000. Play runs set
        // VICTORY_PRECOMPILED_GRAPHICS=1, so these must be built first.
        // NEONBRIDGE_PRECOMPILE_GRAPHICS=0 skips it.
        if graphics && let Some(g) = spec.list("prewarm_graphics") {
            let all_file = cache.join("graphics-total.txt");
            let all: Option<u64> = fs::read_to_string(&all_file).ok().and_then(|t| t.trim().parse().ok()).filter(|&t| t > 0);
            let mut skip = 0u64;
            loop {
                let a: Vec<String> = g.iter().map(|a| fill(a, &places).replace("{skip}", &skip.to_string())).collect();
                println!("[launch] {name} graphics > {} {}", program.display(), a.join(" "));
                let mut gc = starter(name, &root, &spec, &program)?;
                gc.args(&a).current_dir(cmd.get_current_dir().map(Path::to_path_buf).unwrap_or_else(|| root.clone()));
                for (k, v) in cmd.get_envs() {
                    match v {
                        Some(v) => gc.env(k, v),
                        None => gc.env_remove(k),
                    };
                }
                match graphics_chunk(name, gc, skip, all) {
                    Ok(Chunk::Done(n)) if n < GRAPHICS_CHUNK => {
                        let _ = fs::write(&all_file, (skip + n).to_string());
                        break;
                    }
                    Ok(Chunk::Done(_)) => {}
                    Ok(Chunk::Idle) => println!("[launch] {name}: records {}-{} printed nothing for {} s: left to compile in game", skip + 1, skip + GRAPHICS_CHUNK, graphics_idle().as_secs()),
                    Err(e) => {
                        crate::splash::close();
                        fail_box(&e);
                        return Err(e.into());
                    }
                }
                skip += GRAPHICS_CHUNK;
            }
        }
    }
    if matches!(task, Task::Run) {
        crate::splash::status(0.8, &format!("Starting the game ({name})"));
        crate::splash::watch(&program);
        // The leader's own key map ([controls] in its NEONBRIDGE.toml).
        crate::controls::show(name, &program, crate::controls::rows(|k| spec.str(&format!("controls.{k}")).map(str::to_owned)));
    }
    println!("[launch] {name} > {} {}", program.display(), args.join(" "));
    // The mix built and the game starts: the last good mix.
    if matches!(task, Task::Run) && let Some(m) = choice.mix {
        crate::mix::remember_good(m);
    }
    let result = supervised(name, cmd, log.as_deref(), timeout, echo);
    if matches!(task, Task::Run) && let Some(l) = &log {
        let _ = fs::copy(l, cache.join("last-run.log"));
    }
    if matches!(task, Task::Run) && let Err(e) = &result {
        fail_box(e);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_verified_merges_file_by_file() {
        let t = std::env::temp_dir().join("neonbridge_move_test");
        let _ = fs::remove_dir_all(&t);
        let (from, to) = (t.join("old").join("shaders"), t.join("cache").join("shaders"));
        fs::create_dir_all(from.join("ab")).unwrap();
        fs::create_dir_all(to.join("ab")).unwrap();
        fs::write(from.join("ab").join("mine"), "1").unwrap();
        fs::write(from.join("ab").join("same"), "s").unwrap();
        fs::write(to.join("ab").join("same"), "s").unwrap();
        fs::write(from.join("ab").join("differs"), "old").unwrap();
        fs::write(to.join("ab").join("differs"), "new").unwrap();
        let mut n = Moved::default();
        move_verified(&from, &to, &mut n);
        assert_eq!(n, Moved { moved: 1, duplicates: 1, kept: 1 });
        // A colliding bucket's own files arrive; nothing that differs is lost.
        assert_eq!(fs::read_to_string(to.join("ab").join("mine")).unwrap(), "1");
        assert_eq!(fs::read_to_string(from.join("ab").join("differs")).unwrap(), "old");
        assert_eq!(fs::read_to_string(to.join("ab").join("differs")).unwrap(), "new");
        assert!(!from.join("ab").join("same").exists());
        let _ = fs::remove_dir_all(&t);
    }

    #[test]
    fn warm_lines() {
        assert_eq!(warm_line("    41 s  12000 records: 11996 made, 0 failed, 4 duplicates, 0 unmapped, 0 undecoded"), WarmLine::Records(12000));
        assert_eq!(warm_line("precompile-graphics  31000 records: 30990 pipelines made, 1 failed"), WarmLine::Total(31000));
        assert_eq!(warm_line("2026-01-01T00:00:00Z  INFO victory_gpu::pipeline::host: host mesh pipeline shaders=[..]"), WarmLine::Pipeline);
        assert_eq!(warm_line("WARN host mesh pipeline refused"), WarmLine::Other);
        assert_eq!(warm_line("warm-up      120 shaders so far, 3 s"), WarmLine::Shaders(120));
        assert_eq!(warm_line("INFO something else"), WarmLine::Other);
        assert_eq!(warm_line("precompiling 26503 shaders into C:\\x\\dxil (15 compile threads)"), WarmLine::Planned(26503));
        assert_eq!(warm_line("warm-up      120 shaders so far, 3 s"), WarmLine::Shaders(120));
    }

    #[test]
    fn fps_line() {
        assert_eq!(fps("2026-01-01T00:00:00Z  INFO victory_gpu::host: 12.3 fps, 81.3 ms/frame (4.0 preparing"), Some(12.3));
        assert_eq!(fps("no rate here"), None);
    }

    #[test]
    fn reads_the_spec() {
        let s = parse(r#"
# a comment
name = "victory"   # trailing
build = ["cargo", "build",
    "--release"]
exe = "target\\release\\v.exe"
agility = true
[env]
VICTORY_RELAY = "2"
VICTORY_PRESS_FILE = "{press}"
"#).unwrap();
        assert_eq!(s.str("name"), Some("victory"));
        assert_eq!(s.list("build").unwrap(), ["cargo", "build", "--release"]);
        assert_eq!(s.str("exe"), Some(r"target\release\v.exe"));
        assert!(s.flag("agility"));
        assert_eq!(parse("rank = 3\n").unwrap().str("rank"), Some("3"));
        let home = std::env::var("LOCALAPPDATA").unwrap();
        assert_eq!(expand_env(r"%LOCALAPPDATA%\6ixpack\save"), format!(r"{home}\6ixpack\save"));
        assert_eq!(expand_env("100% and %NO_SUCH_VAR_NB% stay"), "100% and %NO_SUCH_VAR_NB% stay");
        let env = parse("[env]\nVICTORY_DATA_DIR = \"%LOCALAPPDATA%\\\\6ixpack\\\\save\"\n").unwrap().env;
        assert_eq!(env[0].1, format!(r"{home}\6ixpack\save"));
        assert_eq!(s.env, [("VICTORY_RELAY".into(), "2".into()), ("VICTORY_PRESS_FILE".into(), "{press}".into())]);
    }

    #[test]
    fn reads_an_option() {
        let o = opt("VICTORY_RELAY=0|1|2 (default 1; play script sets 2): submission relay form");
        assert_eq!(o, Opt { key: "VICTORY_RELAY".into(), values: "0|1|2".into(), default: Some("1".into()), about: "submission relay form".into() });
        let f = opt("--console=series-x|series-s (default series-x): the console profile");
        assert_eq!((f.key.as_str(), f.default.as_deref()), ("--console", Some("series-x")));
    }
}
