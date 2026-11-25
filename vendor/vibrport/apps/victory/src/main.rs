//! `victory`: the command-line front end.
//!
//! Every command reads the user's own files and writes only to stdout or to
//! the user's cache directory (outside the repository).

mod census;
mod inspect;
mod run;
mod shader;
#[cfg(windows)]
mod title_patch;
mod translate;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "victory",
    version,
    about = "Run an Xbox Series title natively on Windows"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise one PE image: headers, sections, imports, TLS, build id.
    Inspect {
        file: PathBuf,
        /// List every imported function.
        #[arg(long)]
        imports: bool,
        /// List every exported function.
        #[arg(long)]
        exports: bool,
    },
    /// Aggregate the imports of every PE image under the given paths and
    /// show which ones our runtime covers. Defaults to the configured
    /// game and test-title directories.
    Census {
        paths: Vec<PathBuf>,
        /// Do not count host pass-through as covered.
        #[arg(long)]
        strict: bool,
        /// List every missing `dll!function`, most used first.
        #[arg(long)]
        missing: bool,
    },
    /// List the `dll!export` pairs an import library describes.
    LibExports {
        lib: PathBuf,
        /// List every export, not just per-DLL counts.
        #[arg(long)]
        list: bool,
    },
    /// Find shader containers in files or directories (also inside engine
    /// archives, if uncompressed) and report their layout.
    ShaderInfo {
        paths: Vec<PathBuf>,
        /// Print one line per container found.
        #[arg(long)]
        verbose: bool,
        /// Also list the decoded native instructions (implies --verbose).
        #[arg(long)]
        disasm: bool,
    },
    /// Run every shader found (containers, SGD5 files) through the
    /// translator and report what it cannot handle yet.
    Translate {
        paths: Vec<PathBuf>,
        /// Check emitted DXIL with dxil.dll's validator (`VICTORY_DXIL`,
        /// else the newest Windows SDK's).
        #[arg(long)]
        validate: bool,
        /// Also create a host D3D12 pipeline for each emitted shader.
        #[arg(long)]
        pipelines: bool,
        /// Print one line per program and outcome.
        #[arg(long)]
        list: bool,
        /// Write translated shaders into `<cache_dir>/shaders` to warm the
        /// runtime shader cache ahead of time (any build with the same
        /// translator sources reads them); shaders already there are skipped.
        #[arg(long)]
        write_cache: bool,
        /// Only the shaders earlier runs used (`used-shaders.txt` in the
        /// shader cache; every shader while there is no list yet).
        #[arg(long, requires = "write_cache")]
        used_only: bool,
        /// Translate as for a host whose waves are MIN-MAX lanes (32-32 for
        /// NVIDIA): sets the widest host wave, MAX, as a run on that GPU
        /// does (this translator does not use MIN). Default: with
        /// --write-cache, this GPU's.
        #[arg(long, value_name = "MIN-MAX", value_parser = parse_lanes)]
        host_lanes: Option<(u32, u32)>,
        /// Worker threads (default: every logical core).
        #[arg(long)]
        jobs: Option<usize>,
    },
    /// Translate the shaders of a title's archives into the cache and make
    /// their compute pipelines, so that a run finds them there.
    Warm {
        paths: Vec<PathBuf>,
        /// Worker threads (default: every logical core).
        #[arg(long)]
        jobs: Option<usize>,
        /// Only translate: no host pipelines.
        #[arg(long)]
        no_pipelines: bool,
    },
    /// Prepare, map and bind a title executable; with --execute, run it.
    Run {
        exe: PathBuf,
        /// What unimplemented imports do when called.
        #[arg(long, value_enum, default_value_t = Unimplemented::Trap)]
        unimplemented: Unimplemented,
        /// Disable host pass-through (everything must be implemented by us).
        #[arg(long)]
        strict: bool,
        /// Actually start the title. Without it, stop after binding.
        #[arg(long)]
        execute: bool,
        /// The console model the title sees (device type, memory).
        #[arg(long, value_enum, default_value_t = Console::SeriesX)]
        console: Console,
        /// Arguments for the title, after `--` (it reads the whole command
        /// line; a title that relaunches itself passes some).
        #[arg(last = true)]
        title_args: Vec<String>,
    },
    /// Show the resolved configuration and check it.
    Config,
}

#[derive(Clone, Copy, ValueEnum)]
enum Unimplemented {
    Trap,
    Zero,
}

#[derive(Clone, Copy, ValueEnum)]
enum Console {
    SeriesX,
    SeriesS,
}

fn main() -> Result<()> {
    victory_core::logging::init("info");
    // The audio recorder `run` starts for `VICTORY_AUDIO_CAPTURE`.
    #[cfg(all(windows, feature = "diagnostics"))]
    if victory_hle::modules::audio::capture::child() {
        return Ok(());
    }
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            file,
            imports,
            exports,
        } => inspect::run(&file, imports, exports),
        Command::Census {
            paths,
            strict,
            missing,
        } => census::run(paths, strict, missing),
        Command::LibExports { lib, list } => inspect::lib_exports(&lib, list),
        Command::ShaderInfo {
            paths,
            verbose,
            disasm,
        } => shader::run(paths, verbose || disasm, disasm),
        Command::Translate {
            paths,
            validate,
            pipelines,
            list,
            write_cache,
            used_only,
            host_lanes,
            jobs,
        } => {
            // `VICTORY_VIEW_LIMIT=<null slot>`: translate as on a host whose view heap is capped
            if let Some(n) = std::env::var("VICTORY_VIEW_LIMIT").ok().and_then(|v| v.parse().ok()) {
                victory_shader::emit::set_view_heap_limit(n);
            }
            // `VICTORY_HOST_WAVE=32`: translate as for a GPU whose waves are at most 32 lanes
            if let Some(n) = std::env::var("VICTORY_HOST_WAVE").ok().and_then(|v| v.parse().ok()) {
                victory_shader::emit::set_host_max_wave(n);
            }
            translate::run(paths, validate, pipelines, list, write_cache, used_only, host_lanes, jobs)
        }
        Command::Warm { paths, jobs, no_pipelines } => translate::warm(paths, jobs, !no_pipelines),
        Command::Run {
            exe,
            unimplemented,
            strict,
            execute,
            console,
            title_args: _,
        } => run::run(&exe, unimplemented, strict, execute, console),
        Command::Config => show_config(),
    }
}

fn show_config() -> Result<()> {
    let cfg = victory_core::Config::load()?;
    let status = |p: &Option<PathBuf>| match p {
        Some(p) if p.exists() => format!("{}", p.display()),
        Some(p) => format!("{}  (does not exist)", p.display()),
        None => "(not set)".to_owned(),
    };
    println!(
        "config file : {}",
        cfg.source
            .as_ref()
            .map_or("(none)".into(), |p| p.display().to_string())
    );
    println!("game_dir    : {}", status(&cfg.game_dir));
    println!("cache_dir   : {}", cfg.cache_dir.display());
    println!("data_dir    : {}", cfg.data_dir.display());
    for t in &cfg.test_titles {
        println!("test_title  : {}", status(&Some(t.clone())));
    }
    for p in cfg
        .game_dir
        .iter()
        .chain(&cfg.test_titles)
        .chain([&cfg.cache_dir, &cfg.data_dir])
    {
        if let Err(e) = victory_core::ensure_outside_repo(p) {
            println!("POLICY VIOLATION: {e}");
        }
    }
    Ok(())
}

/// `--host-lanes MIN-MAX` (or one size for both).
fn parse_lanes(s: &str) -> Result<(u32, u32), String> {
    let (a, b) = s.split_once('-').unwrap_or((s, s));
    let n = |x: &str| {
        x.trim()
            .parse::<u32>()
            .ok()
            .filter(|n| n.is_power_of_two() && (4..=128).contains(n))
            .ok_or_else(|| format!("{x:?} is not a wave size (4 to 128, a power of two)"))
    };
    let (a, b) = (n(a)?, n(b)?);
    if a > b {
        return Err(format!("{a} is more than {b}"));
    }
    Ok((a, b))
}

/// Paths given on the command line, or the configured game + test titles.
pub(crate) fn default_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>> {
    if !paths.is_empty() {
        return Ok(paths);
    }
    let cfg = victory_core::Config::load()?;
    let all: Vec<PathBuf> = cfg.game_dir.into_iter().chain(cfg.test_titles).collect();
    anyhow::ensure!(
        !all.is_empty(),
        "no paths given and none configured (see victory.example.toml)"
    );
    Ok(all)
}
