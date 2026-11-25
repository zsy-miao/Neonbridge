//! Pre-push churn check for xshader commits: translates a sample of the
//! programs the title asked for before (the shader cache's `seen/`, sources
//! in `requests/`) with the working tree's xshader and with BASE's (default
//! HEAD~1), and fails when more than MAX% of them emit different HLSL. Every
//! changed program is a fresh dxc compile at the next prewarm (da94591 changed
//! about 25% of them, 297 of 1306 in the 9eea0b4 prewarm, for a fill most
//! programs never read). A commit meant to change many programs passes with
//! `--max-pct 100` and says so in its message.
//!
//! Run from anywhere in the repo, before every xshader push:
//!   cargo run --release --offline --bin churn-check -- [--base REV] [--n 50] [--max-pct 5] [--cache DIR]
//! Builds go to %TEMP%/xshader-churn (BASE's is kept per revision).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn run(cmd: &mut Command) -> Result<String, String> {
    let out = cmd.output().map_err(|e| format!("{cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?}: {}\n{}", out.status, String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// Builds xshader from `crate_dir` into `target`; returns the exe.
fn build(crate_dir: &Path, target: &Path) -> Result<PathBuf, String> {
    eprintln!("churn-check: building {}", crate_dir.display());
    run(Command::new("cargo").args(["build", "--release", "--offline", "--bin", "xshader"])
        .current_dir(crate_dir).env("CARGO_TARGET_DIR", target))?;
    Ok(target.join("release").join("xshader.exe"))
}

fn translate(exe: &Path, source: &Path, out: &Path) -> Option<Vec<u8>> {
    let _ = std::fs::remove_file(out);
    let status = Command::new(exe).arg(source).arg("--hlsl-out").arg(out).arg("--profile-out").arg(out.with_extension("profile"))
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().ok()?;
    // Unsupported programs (exit 65) and failures compare as "no output".
    if status.success() { std::fs::read(out).ok() } else { None }
}

fn main() -> ExitCode {
    match check() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("churn-check: {e}");
            ExitCode::from(2)
        }
    }
}

fn check() -> Result<bool, String> {
    let args: Vec<String> = std::env::args().collect();
    let base = arg(&args, "--base").unwrap_or_else(|| "HEAD~1".into());
    let n: usize = arg(&args, "--n").and_then(|v| v.parse().ok()).unwrap_or(50);
    let max_pct: f64 = arg(&args, "--max-pct").and_then(|v| v.parse().ok()).unwrap_or(5.0);
    let temp = std::env::temp_dir();
    let cache = arg(&args, "--cache").map(PathBuf::from).unwrap_or_else(|| temp.join("tuxbox_shader_cache"));

    let root = PathBuf::from(run(Command::new("git").args(["rev-parse", "--show-toplevel"]))?);
    let rev = run(Command::new("git").args(["rev-parse", "--short", &base]).current_dir(&root))?;
    let work = temp.join("xshader-churn");
    let tree = work.join(&rev);
    let paths = ["vendor/tuxbox/tools/shader-compiler", "vendor/tuxbox/runtime/dxbc", "vendor/tuxbox/Cargo.toml"];
    if !tree.join(paths[0]).exists() {
        std::fs::create_dir_all(&tree).map_err(|e| e.to_string())?;
        let tar = tree.join("src.tar");
        run(Command::new("git").arg("archive").arg("-o").arg(&tar).arg(&rev).args(paths).current_dir(&root))?;
        run(Command::new("tar").arg("-xf").arg(&tar).arg("-C").arg(&tree))?;
    }
    let old = build(&tree.join(paths[0]), &work.join(format!("target-{rev}")))?;
    let new = build(&root.join(paths[0]), &work.join("target-head"))?;

    // An even spread over the sorted seen names: the same sample every run.
    let mut seen: Vec<String> = std::fs::read_dir(cache.join("seen")).map_err(|e| format!("{}: {e}", cache.join("seen").display()))?
        .flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|f| f.ends_with(".dxbc")).collect();
    seen.sort();
    let step = (seen.len() / n.max(1)).max(1);
    let sample: Vec<&String> = seen.iter().step_by(step).take(n).collect();

    let (mut compared, mut changed) = (0usize, Vec::new());
    let out = work.join("out");
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    for name in &sample {
        let source = cache.join("requests").join(name);
        if !source.exists() {
            continue;
        }
        let a = translate(&old, &source, &out.join("old.hlsl"));
        let b = translate(&new, &source, &out.join("new.hlsl"));
        if a.is_none() && b.is_none() {
            continue;
        }
        compared += 1;
        if a != b {
            if let (Some(_), Some(_)) = (&a, &b) {
                let keep = out.join(name.trim_end_matches(".dxbc"));
                let _ = std::fs::copy(out.join("old.hlsl"), keep.with_extension("old.hlsl"));
                let _ = std::fs::copy(out.join("new.hlsl"), keep.with_extension("new.hlsl"));
            }
            changed.push(name.as_str());
        }
    }
    if compared == 0 {
        return Err(format!("no programs to compare under {}", cache.display()));
    }
    let pct = changed.len() as f64 * 100.0 / compared as f64;
    for name in &changed {
        println!("changed {name}");
    }
    let pass = pct <= max_pct;
    println!("churn-check: {} of {compared} programs emit different HLSL than {rev} ({pct:.0}%, limit {max_pct}%): {}{}",
        changed.len(), if pass { "pass" } else { "FAIL" },
        if changed.is_empty() { String::new() } else { format!("; old/new HLSL in {}", out.display()) });
    Ok(pass)
}
