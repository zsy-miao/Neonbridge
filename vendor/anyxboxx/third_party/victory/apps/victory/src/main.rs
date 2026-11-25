//! `victory`: the command-line front end.
//!
//! Every command reads the user's own files and writes only to stdout or to
//! the user's cache directory (outside the repository).

mod census;
mod inspect;
mod run;
mod shader;
mod translate;
mod discord;

/// Counts the Rust heap's live bytes and their peak for the resident line. Not in diagnostics
/// builds, where `victory-gpu`'s allocator does the counting.
#[cfg(not(feature = "diagnostics"))]
struct Counting;

// SAFETY: every call goes to the system's allocator unchanged; only counters are kept beside it.
#[cfg(not(feature = "diagnostics"))]
unsafe impl std::alloc::GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let at = unsafe { std::alloc::System.alloc(layout) };
        if !at.is_null() {
            victory_core::heapcount::grow(layout.size());
        }
        at
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let at = unsafe { std::alloc::System.alloc_zeroed(layout) };
        if !at.is_null() {
            victory_core::heapcount::grow(layout.size());
        }
        at
    }

    unsafe fn dealloc(&self, at: *mut u8, layout: std::alloc::Layout) {
        victory_core::heapcount::shrink(layout.size());
        // SAFETY: the caller's contract, passed on.
        unsafe { std::alloc::System.dealloc(at, layout) };
    }

    unsafe fn realloc(&self, at: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        // SAFETY: the caller's contract, passed on.
        let moved = unsafe { std::alloc::System.realloc(at, layout, size) };
        if !moved.is_null() {
            if size > layout.size() {
                victory_core::heapcount::grow(size - layout.size());
            } else {
                victory_core::heapcount::shrink(layout.size() - size);
            }
        }
        moved
    }
}

#[cfg(not(feature = "diagnostics"))]
#[global_allocator]
static HEAP: Counting = Counting;

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
        /// Worker threads (default: every logical core).
        #[arg(long)]
        jobs: Option<usize>,
        /// Warm the run's shader cache: a file of shader hashes (the 16
        /// hex digits a run's log names a shader by, anywhere in a line);
        /// the containers with those hashes are translated into the
        /// cache and nothing else is done. Set `VICTORY_HOST_WAVE` and
        /// `VICTORY_VIEW_LIMIT` to what the run's log says of the host.
        #[arg(long)]
        warm: Option<PathBuf>,
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

/// (Neptune) Diagnostics builds: the redistributable D3D12 runtime and debug layer in `D3D12\`
/// beside the executable are used if they are there (`build.rs` exports these two; the
/// version is the middle number of the package's, 1.620.2).
#[cfg(all(windows, feature = "diagnostics"))]
#[unsafe(no_mangle)]
#[used]
pub static D3D12SDKVersion: u32 = 620;

#[cfg(all(windows, feature = "diagnostics"))]
#[repr(transparent)]
pub struct SdkPath(*const u8);
// SAFETY: a pointer to a constant string.
#[cfg(all(windows, feature = "diagnostics"))]
unsafe impl Sync for SdkPath {}

#[cfg(all(windows, feature = "diagnostics"))]
#[unsafe(no_mangle)]
#[used]
pub static D3D12SDKPath: SdkPath = SdkPath(b".\\D3D12\\\0".as_ptr());

fn main() -> Result<()> {
    // `VICTORY_HEAP_COUNT=1`: the Rust heap's live bytes for the resident line (off: it slowed loading).
    victory_core::heapcount::enable(std::env::var_os("VICTORY_HEAP_COUNT").is_some_and(|v| v == "1"));
    // (Neptune) Discord Rich Presence while a title runs (`discord.rs`).
    if std::env::args().nth(1).as_deref() == Some("run") {
        discord::start();
    }
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
            jobs,
            warm,
        } => {
            // `VICTORY_VIEW_LIMIT=<null slot>`: translate as on a host whose view heap is capped
            if let Some(n) = std::env::var("VICTORY_VIEW_LIMIT").ok().and_then(|v| v.parse().ok()) {
                victory_shader::emit::set_view_heap_limit(n);
            }
            // `VICTORY_HOST_WAVE=32`: translate as for a GPU whose waves are at most 32 lanes
            if let Some(n) = std::env::var("VICTORY_HOST_WAVE").ok().and_then(|v| v.parse().ok()) {
                victory_shader::emit::set_host_max_wave(n);
            }
            if let Some(file) = warm {
                return translate::warm_translations(paths, &file, jobs);
            }
            translate::run(paths, validate, pipelines, list, jobs)
        }
        Command::Warm { paths, jobs, no_pipelines } => translate::warm(paths, jobs, !no_pipelines),
        Command::Run {
            exe,
            unimplemented,
            strict,
            execute,
            console,
            title_args: _,
        } => {
            let result = run::run(&exe, unimplemented, strict, execute, console);
            // The process's own end, as `main`'s Result maps it: Ok is 0,
            // Err is 1. A title that ends the process itself logs its own
            // line and never returns here.
            let code = if result.is_ok() { 0 } else { 1 };
            tracing::info!("runtime exit: code {code}");
            result
        }
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
