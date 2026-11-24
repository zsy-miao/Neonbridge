//! Neonbridge: run a dumped Xbox Series (Scarlett / GDK) executable on
//! Windows through TuxBox's Rust console-library providers.
//!
//! The game's x64 code runs natively; the console DLLs it imports (d3d12_xs,
//! xg_xs, xmem, xapu, chad, AcpHal, GameInput, XCRDAPI, XboxBvhBuilder, ...)
//! are replaced by TuxBox providers built from a local TuxBox checkout
//! (`runtime/*`, staticlib crates linked against their `.def` export tables,
//! the same recipe as TuxBox `build.py`). Providers are staged beside the exe;
//! any game file they replace is moved to `<game>/_6ixpack_orig/` first and
//! `--restore` puts it back.

mod shaders;
mod splash;

use std::{
    collections::HashMap,
    error::Error,
    ffi::OsString,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

const USAGE: &str = "usage: neonbridge --game <dir> [--tuxbox <TuxBox checkout>] [--exe <name>] [--log <file>]
                      [--build-only | --stage-only | --build-shaders | --restore] [-- <game args>]
  --game       dumped package folder (holds MicrosoftGame.Config and the exe)
  --tuxbox     provider source (default: the vendored vendor/tuxbox, or env TUXBOX_ROOT)
  providers build into %LOCALAPPDATA%/6ixpack/tuxbox
  --exe        executable name (default: the MicrosoftGame.Config Executable, else the only x64 exe)
  --log        game log file (default %LOCALAPPDATA%/6ixpack/logs/<exe>_<unix time>.log)
  --build-only build the providers, touch nothing in the game folder
  --stage-only build and stage the providers beside the exe, do not start it
  --build-shaders translate every shader in shaders.rpf into the cache, do not start the game
  --restore    remove staged providers and put the original game files back
  --debug      run under cdb (Windows Kits debugger): first-chance exceptions, the
               crash event, registers, stack and loaded modules go to the log
  --sandbox    run inside Sandboxie-Plus (box DefaultBox, or env SANDBOXIE_BOX); the
               game's writes stay in the box; with --debug the cdb log is fetched back
  --debug-full like --debug, plus every first-chance C++/WinRT exception with its stack
               (each one stalls the game for the stack walk: slow, use to chase a throw)
  --cdb FILE   extra cdb commands (breakpoints etc.) run before the game starts; implies --debug
  --timeout S  stop the game (and cdb) after S seconds, then print the summary as usual
  --summary LOG  print the end-of-run summary of an existing log (and its cdb part); runs nothing
Needs Visual Studio Build Tools (x64 C++), cargo and the x86_64-pc-windows-msvc target.";

/// (cargo package, output DLL, export table relative to the TuxBox root, extra import libs, extra C++ sources)
const PROVIDERS: &[(&str, &str, &str, &[&str], &[&str])] = &[
    ("tuxbox-xmem-http", "tuxbox_xhttp.dll", "runtime/xmem/xhttp.def", &["winhttp.lib"], &[]),
    ("tuxbox-xmem", "xmem.dll", "runtime/xmem/xmem.def", &["tuxbox_xhttp.lib", "ole32.lib", "/include:DllMain"], &[]), // own DllMain must win over the libcmt stub; forwards XMemSetWinHttpHooks to tuxbox_xhttp
    ("tuxbox-acphal", "AcpHal.dll", "runtime/acphal/AcpHal.def", &[], &[]),
    ("tuxbox-xapu", "xapu.dll", "runtime/xapu/xapu.def", &[], &[]),
    ("tuxbox-xapu", "chad.dll", "runtime/xapu/chad.def", &[], &[]),
    ("tuxbox-xcrdapi", "XCRDAPI.dll", "runtime/xcrdapi/xcrdapi.def", &[], &[]),
    ("tuxbox-xgameruntime", "xgameruntime.dll", "runtime/xgameruntime/xgameruntime.def", &[], &[]),
    ("tuxbox-bvh", "XboxBvhBuilder.dll", "runtime/bvh/XboxBvhBuilder.def", &[], &[]),
    ("tuxbox-xg", "xg_xs.dll", "runtime/xg/xg_xs.def", &[], &[]),
    ("tuxbox-dxc", "dxcompiler_xs.dll", "runtime/dxc/dxcompiler_xs.def", &[], &[]),
    ("tuxbox-d3d12", "d3d12_xs.dll", "runtime/d3d12/d3d12_xs.def", &["dxgi.lib", "d3d12.lib", "user32.lib", "d3dcompiler.lib"], &[]),
    ("tuxbox-gameinput", "GameInput.dll", "runtime/gameinput/GameInput.def", &["libcmt.lib", "user32.lib", "xinput.lib"] /* CRT first: xinput.lib also exports DllMain */, &["runtime/gameinput/abi.cpp"]),
];
/// `rustc --print native-static-libs` for the MSVC target with +crt-static (TuxBox build.py).
const RUST_STD_LIBS: &[&str] = &["kernel32.lib", "ntdll.lib", "userenv.lib", "ws2_32.lib", "dbghelp.lib", "/defaultlib:libcmt"];
const TARGET: &str = "x86_64-pc-windows-msvc";
const BACKUP: &str = "_6ixpack_orig";
const STAGED: &str = "_6ixpack_staged.txt";

#[derive(PartialEq)]
enum Mode {
    Run,
    BuildOnly,
    StageOnly,
    BuildShaders,
    Restore,
}

fn run(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let (mut game, mut tuxbox, mut exe, mut log, mut mode, mut game_args) = (None, env_path("TUXBOX_ROOT"), None, None, Mode::Run, Vec::new());
    let (mut debug, mut sandbox, mut cdb_extra, mut timeout, mut trace_throws) = (false, false, None, None, false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut val = |n: &str| it.next().cloned().ok_or_else(|| format!("{n} needs a value\n{USAGE}"));
        match a.to_str() {
            Some("--game") => game = Some(PathBuf::from(val("--game")?)),
            Some("--tuxbox") => tuxbox = Some(PathBuf::from(val("--tuxbox")?)),
            Some("--exe") => exe = Some(val("--exe")?.to_string_lossy().into_owned()),
            Some("--log") => log = Some(PathBuf::from(val("--log")?)),
            Some("--build-only") => mode = Mode::BuildOnly,
            Some("--stage-only") => mode = Mode::StageOnly,
            Some("--build-shaders") => mode = Mode::BuildShaders,
            Some("--restore") => mode = Mode::Restore,
            Some("--debug") => debug = true,
            Some("--debug-full") => {
                debug = true;
                trace_throws = true;
            }
            Some("--timeout") => timeout = Some(val("--timeout")?.to_string_lossy().parse::<f64>().map_err(|e| format!("--timeout: {e}"))?),
            Some("--summary") => return print_summary(Path::new(&val("--summary")?)),
            Some("--sandbox") => sandbox = true,
            Some("--cdb") => {
                cdb_extra = Some(fs::read_to_string(val("--cdb")?)?);
                debug = true;
            }
            Some("--") => {
                game_args.extend(it.by_ref().cloned());
            }
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            _ => return Err(format!("launch: unknown argument {}\n{USAGE}", a.to_string_lossy()).into()),
        }
    }
    if mode == Mode::Restore {
        return restore(&game.ok_or(USAGE)?);
    }
    let tuxbox = tuxbox.unwrap_or_else(|| PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/vendor/tuxbox")));
    if !tuxbox.join("runtime/d3d12/d3d12_xs.def").is_file() {
        return Err(format!("{} is not a TuxBox provider source (no runtime/d3d12/d3d12_xs.def)", tuxbox.display()).into());
    }
    let game = if mode == Mode::BuildOnly { game } else { Some(game.ok_or(USAGE)?) };
    let target = match &game {
        Some(g) => Some(find_exe(g, exe.as_deref())?),
        None => None,
    };

    if mode == Mode::Run {
        splash::show("Building the TuxBox runtime");
    }
    let out = env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("6ixpack/tuxbox");
    let dlls = build_providers(&tuxbox, &out)?;
    splash::status(0.3, "Staging the runtime beside the game");
    if mode == Mode::BuildOnly {
        println!("[launch] built {} providers in {}", dlls.len(), out.display());
        return Ok(());
    }
    let (game, target) = (game.unwrap(), target.unwrap());
    // d3d12_xs compiles Xbox rectangle-list pipelines with DXC loaded from the
    // game folder; stage the SDK's signed compiler beside it.
    let mut dlls = dlls;
    if let Some(dxc) = shaders::find_dxc() {
        dlls.extend(["dxcompiler.dll", "dxil.dll"].map(|n| dxc.with_file_name(n)).into_iter().filter(|p| p.is_file()));
    }
    if mode != Mode::BuildShaders {
        stage(&game, &dlls)?;
    }
    if mode == Mode::StageOnly {
        println!("[launch] staged {} providers beside {}; not started (--stage-only)", dlls.len(), target.display());
        return Ok(());
    }
    splash::status(0.4, "Preparing shaders");
    // Runtime shader worker (TuxBox runtime_shaders.py on Windows): xshader + SDK dxc.
    let worker = if std::env::var("TUXBOX_RUNTIME_SHADERS").as_deref() == Ok("0") { None } else {
        match (build_xshader(&tuxbox, &out), shaders::find_dxc()) {
            (Ok(xshader), Some(dxc)) => {
                let cache_root = std::env::temp_dir().join("tuxbox_shader_cache");
                // Build every program in shaders.rpf into the prewarm archive (once per shaders.rpf).
                let rpf = game.join("shaders.rpf");
                // The archive only feeds the prewarm; TUXBOX_PREWARM=0 skips both.
                if rpf.is_file() && std::env::var("TUXBOX_PREWARM").as_deref() != Ok("0") {
                    println!("[launch] building shaders from {}", rpf.display());
                    match shaders::build_requests(&rpf, &cache_root.join("requests")) {
                        Ok(Some(b)) => println!("[launch] shaders.rpf: {} programs, {} new requests archived", b.programs, b.written),
                        Ok(None) => println!("[launch] shaders.rpf already built"),
                        Err(e) => println!("[launch] shaders.rpf not built: {e}"),
                    }
                }
                let name = format!("run-{}", std::process::id());
                let real = cache_root.join(&name);
                let session = if sandbox { boxed_path(&real).unwrap_or(real) } else { real };
                println!("[launch] shader worker: {} + {}; session {}", xshader.display(), dxc.display(), session.display());
                let log_path = out.join("shader-worker.log");
                let _ = fs::remove_file(&log_path);
                let sink = std::sync::Mutex::new(fs::OpenOptions::new().create(true).append(true).open(&log_path)?);
                match shaders::Worker::start(shaders::Tools { xshader, dxc, defines: Vec::new() }, cache_root, session.clone(),
                    move |line| { let _ = writeln!(sink.lock().unwrap(), "{line}"); }) {
                    Ok(w) => Some((w, name, session, log_path)),
                    Err(e) => { println!("[launch] shader worker not started: {e}"); None }
                }
            }
            (Err(e), _) => { println!("[launch] shader worker not started: xshader build failed: {e}"); None }
            (_, None) => { println!("[launch] shader worker not started: no Windows SDK dxc.exe with dxil.dll (or set DXC)"); None }
        }
    };
    if mode == Mode::BuildShaders {
        let (w, _, session, log_path) = worker.ok_or("shader worker not started")?;
        let st = w.stop();
        use std::sync::atomic::Ordering::Relaxed;
        println!("[launch] shaders built: {} translated, {} from cache, {} unsupported, {} failed (log {})",
            st.prewarmed.load(Relaxed), st.cached.load(Relaxed), st.unsupported.load(Relaxed), st.failed.load(Relaxed), log_path.display());
        let total = (st.unsupported.load(Relaxed) + st.failed.load(Relaxed)).max(1) as f64;
        println!("[launch] by construct:");
        for (group, count) in st.top_constructs(15) {
            println!("[launch]   {count:>6}  {:>5.1}%  {group}", 100.0 * count as f64 / total);
        }
        println!("[launch] by reason:");
        for (reason, count) in st.top_reasons(25) {
            println!("[launch]   {count:>6}  {:>5.1}%  {reason}", 100.0 * count as f64 / total);
        }
        let _ = fs::remove_dir_all(session);
        return Ok(());
    }
    let env: Vec<(String, String)> = worker.iter().map(|(_, name, _, _)| ("TUXBOX_SHADER_SESSION".to_owned(), name.clone())).collect();
    splash::status(0.6, "Starting the game");
    let result = start(&game, &target, &game_args, log, debug.then(|| cdb_extra.unwrap_or_default()), trace_throws, sandbox, &env, timeout);
    if let Some((w, _, session, log_path)) = worker {
        let st = w.stop();
        use std::sync::atomic::Ordering::Relaxed;
        println!("[launch] shaders: {} prewarmed, {} compiled, {} from cache, {} unsupported, {} failed (log {})",
            st.prewarmed.load(Relaxed), st.compiled.load(Relaxed), st.cached.load(Relaxed), st.unsupported.load(Relaxed), st.failed.load(Relaxed), log_path.display());
        let _ = fs::remove_dir_all(session);
    }
    result
}

/// Build the vendored TuxBox shader translator (tools/shader-compiler -> xshader.exe).
fn build_xshader(tuxbox: &Path, out: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let manifest = tuxbox.join("tools/shader-compiler/Cargo.toml");
    if !manifest.is_file() {
        return Err(format!("{} missing", manifest.display()).into());
    }
    let target_dir = out.join("xshader");
    // /Brepro: a rebuild of unchanged sources gives the same bytes, so the shader
    // cache (keyed by xshader's hash) survives touched files and relinks.
    let status = Command::new("cargo").args(["build", "--release", "--quiet", "--manifest-path"]).arg(&manifest)
        .arg("--target-dir").arg(&target_dir).env("RUSTFLAGS", "-C link-arg=/Brepro").status()?;
    if !status.success() {
        return Err(format!("cargo build of xshader failed ({status})").into());
    }
    Ok(target_dir.join("release/xshader.exe"))
}

/// Block while any process runs this exe image (Start.exe /wait can return early).
fn wait_for_image(exe: &Path) {
    let Some(name) = exe.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
    let running = || Command::new("tasklist").args(["/FI", &format!("IMAGENAME eq {name}"), "/NH"]).output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_ascii_lowercase().contains(&name.to_ascii_lowercase()))
        .unwrap_or(false);
    let t = Instant::now();
    let mut waited = false;
    while running() {
        waited = true;
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    if waited {
        println!("[launch] {name} ended {:.1}s later", t.elapsed().as_secs_f64());
    }
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

/// The exe named by MicrosoftGame.Config (`<Executable Name="...">`), else the
/// only x64 PE executable in the folder.
fn find_exe(game: &Path, name: Option<&str>) -> Result<PathBuf, Box<dyn Error>> {
    if let Some(n) = name {
        let p = game.join(n);
        return if p.is_file() { Ok(p) } else { Err(format!("{} not found", p.display()).into()) };
    }
    for cfg in ["MicrosoftGame.Config", "MicrosoftGame.config"] {
        if let Ok(text) = fs::read_to_string(game.join(cfg)) {
            for tag in text.split("<Executable").skip(1) {
                if let Some(v) = tag.split("Name=\"").nth(1).and_then(|s| s.split('"').next()) {
                    let p = game.join(v);
                    if p.is_file() {
                        return Ok(p);
                    }
                }
            }
        }
    }
    let exes: Vec<_> = fs::read_dir(game)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && is_x64_exe(p))
        .collect();
    match exes.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(format!("no x64 executable in {}", game.display()).into()),
        _ => Err(format!("several executables in {}; pick one with --exe", game.display()).into()),
    }
}

fn is_x64_exe(p: &Path) -> bool {
    let Ok(b) = fs::read(p).map(|b| b[..b.len().min(4096)].to_vec()) else { return false };
    if b.len() < 0x40 || &b[..2] != b"MZ" {
        return false;
    }
    let pe = u32::from_le_bytes(b[0x3c..0x40].try_into().unwrap()) as usize;
    b.len() >= pe + 24 && &b[pe..pe + 4] == b"PE\0\0" && u16::from_le_bytes([b[pe + 4], b[pe + 5]]) == 0x8664 && u16::from_le_bytes([b[pe + 22], b[pe + 23]]) & 0x2000 == 0
}

/// Environment of an x64 Visual Studio developer prompt (vcvars64.bat via vswhere).
fn msvc_env() -> Result<HashMap<String, String>, Box<dyn Error>> {
    let pf86 = std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into());
    let vswhere = Path::new(&pf86).join(r"Microsoft Visual Studio\Installer\vswhere.exe");
    let o = Command::new(&vswhere)
        .args(["-latest", "-products", "*", "-requires", "Microsoft.VisualStudio.Component.VC.Tools.x86.x64", "-property", "installationPath"])
        .output()
        .map_err(|e| format!("vswhere ({}): {e}; install Visual Studio Build Tools with the x64 C++ tools", vswhere.display()))?;
    let root = String::from_utf8_lossy(&o.stdout).lines().next().unwrap_or("").trim().to_string();
    let vcvars = Path::new(&root).join(r"VC\Auxiliary\Build\vcvars64.bat");
    if root.is_empty() || !vcvars.is_file() {
        return Err("no Visual Studio install with the x64 C++ tools (vcvars64.bat) found".into());
    }
    // A script file sidesteps cmd /c quoting of the space-laden vcvars path.
    let script = std::env::temp_dir().join(format!("6ixpack_vcvars_{}.cmd", std::process::id()));
    fs::write(&script, format!("@call \"{}\" >nul\r\n@set\r\n", vcvars.display()))?;
    let o = Command::new("cmd").arg("/c").arg(&script).output();
    let _ = fs::remove_file(&script);
    let o = o?;
    let env: HashMap<_, _> = String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    if !env.keys().any(|k| k.eq_ignore_ascii_case("LIB")) {
        return Err(format!("{} did not set up an x64 environment", vcvars.display()).into());
    }
    Ok(env)
}

fn tool(env: &HashMap<String, String>, exe: &str) -> PathBuf {
    let path = env.iter().find(|(k, _)| k.eq_ignore_ascii_case("PATH")).map(|(_, v)| v.as_str()).unwrap_or("");
    path.split(';').map(|d| Path::new(d).join(exe)).find(|p| p.is_file()).unwrap_or_else(|| PathBuf::from(exe))
}

fn check(cmd: &mut Command, what: &str) -> Result<(), Box<dyn Error>> {
    let s = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    if s.success() { Ok(()) } else { Err(format!("{what} failed ({s})").into()) }
}

/// Build every provider DLL into `out`; returns their paths.
fn build_providers(tuxbox: &Path, out: &Path) -> Result<Vec<PathBuf>, Box<dyn Error>> {
    let env = msvc_env()?;
    let (link, cl) = (tool(&env, "link.exe"), tool(&env, "cl.exe"));
    let cargo_dir = out.join("cargo");
    fs::create_dir_all(out)?;
    let mut packages: Vec<&str> = PROVIDERS.iter().map(|p| p.0).collect();
    packages.dedup();
    let t = Instant::now();
    eprintln!("[launch] cargo: {} provider crates ({TARGET}, release)", packages.len());
    let mut cargo = Command::new("cargo");
    cargo.current_dir(tuxbox).envs(&env).args(["build", "--locked", "--release", "--target", TARGET, "--target-dir"]).arg(&cargo_dir);
    for p in &packages {
        cargo.args(["--package", p]);
    }
    check(&mut cargo, "cargo build")?;
    eprintln!("[launch] cargo done in {:.0}s", t.elapsed().as_secs_f64());
    let lib_dir = cargo_dir.join(TARGET).join("release");
    let mut dlls = Vec::new();
    for (i, (pkg, dll, def, libs, cpp)) in PROVIDERS.iter().enumerate() {
        // Skip the link when the DLL is newer than its archive, .def and C++ sources.
        let mtime = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
        let inputs: Vec<PathBuf> = std::iter::once(lib_dir.join(format!("{}.lib", pkg.replace('-', "_"))))
            .chain(std::iter::once(tuxbox.join(def)))
            .chain(cpp.iter().map(|c| tuxbox.join(c)))
            .collect();
        if let Some(t) = mtime(&out.join(dll)) {
            if inputs.iter().all(|p| mtime(p).is_some_and(|m| m <= t)) {
                dlls.push(out.join(dll));
                continue;
            }
        }
        eprintln!("[launch] link {}/{}: {dll}", i + 1, PROVIDERS.len());
        let mut objs = Vec::new();
        for src in *cpp {
            let src = tuxbox.join(src);
            let obj = format!("{}.obj", src.file_stem().unwrap().to_string_lossy());
            check(
                Command::new(&cl).current_dir(out).envs(&env).args(["/nologo", "/c", "/O2", "/MT", "/Brepro", "/std:c++17", "/EHsc", "/W4", "/WX"]).arg(format!("/Fo{obj}")).arg(&src),
                &format!("cl {}", src.display()),
            )?;
            objs.push(obj);
        }
        let stem = dll.trim_end_matches(".dll");
        check(
            Command::new(&link)
                .current_dir(out)
                .envs(&env)
                .args(["/nologo", "/machine:x64", "/Brepro", "/incremental:no", "/dll"])
                .arg(format!("/def:{}", tuxbox.join(def).display()))
                .arg(format!("/implib:{stem}.lib"))
                .arg(format!("/out:{dll}"))
                // symbol addresses for the sampler's "module+0xRVA" (see hot_lines)
                .arg(format!("/map:{stem}.map"))
                .args(&objs)
                // extra libs (CRT first) before the Rust archive, so DllMain binds to the
                // CRT stub, not to an import library that also exports it (xinput.lib)
                .args(libs.iter().take_while(|l| **l == "libcmt.lib"))
                .arg(lib_dir.join(format!("{}.lib", pkg.replace('-', "_"))))
                .args(libs.iter().skip_while(|l| **l == "libcmt.lib"))
                .args(RUST_STD_LIBS),
            &format!("link {dll}"),
        )?;
        dlls.push(out.join(dll));
    }
    Ok(dlls)
}

/// Copy providers beside the exe; a replaced game file is moved to `_6ixpack_orig/` once.
fn stage(game: &Path, dlls: &[PathBuf]) -> Result<(), Box<dyn Error>> {
    let backup = game.join(BACKUP);
    let mut staged = Vec::new();
    for d in dlls {
        let name = d.file_name().unwrap();
        let dst = game.join(name);
        // A file we staged has the recorded size; anything else (an original put
        // back by hand) is kept, never overwritten.
        let ours = staged_size(game, &name.to_string_lossy()).is_some_and(|n| fs::metadata(&dst).is_ok_and(|m| m.len() == n));
        if dst.is_file() && !ours {
            fs::create_dir_all(&backup)?;
            let mut saved = backup.join(name);
            let mut i = 1;
            while saved.exists() {
                saved = backup.join(format!("{}.{i}", name.to_string_lossy()));
                i += 1;
            }
            fs::rename(&dst, &saved)?;
            eprintln!("[launch] kept original {} as {}", name.to_string_lossy(), saved.display());
        }
        fs::copy(d, &dst)?;
        staged.push(format!("{}\t{}", name.to_string_lossy(), fs::metadata(&dst)?.len()));
    }
    fs::write(game.join(STAGED), staged.join("\n") + "\n")?;
    Ok(())
}

/// Size recorded when `name` was staged (older lists without sizes: unknown size, treated as foreign).
fn staged_size(game: &Path, name: &str) -> Option<u64> {
    let list = fs::read_to_string(game.join(STAGED)).ok()?;
    list.lines().find_map(|l| {
        let (n, size) = l.split_once('\t')?;
        n.eq_ignore_ascii_case(name).then(|| size.parse().ok()).flatten()
    })
}

fn restore(game: &Path) -> Result<(), Box<dyn Error>> {
    let list = fs::read_to_string(game.join(STAGED)).map_err(|_| format!("nothing staged in {}", game.display()))?;
    let backup = game.join(BACKUP);
    let (mut removed, mut put_back) = (0, 0);
    for name in list.lines().filter(|l| !l.is_empty()).map(|l| l.split('\t').next().unwrap_or(l)) {
        if fs::remove_file(game.join(name)).is_ok() {
            removed += 1;
        }
        if fs::rename(backup.join(name), game.join(name)).is_ok() {
            put_back += 1;
        }
    }
    fs::remove_file(game.join(STAGED))?;
    let _ = fs::remove_dir(&backup);
    println!("[launch] removed {removed} providers, restored {put_back} original files");
    Ok(())
}

/// Start the exe from its folder (optionally under cdb and/or in Sandboxie);
/// stdout+stderr go to the console and the log.
fn start(game: &Path, exe: &Path, args: &[OsString], log: Option<PathBuf>, debug: Option<String>, trace_throws: bool, sandbox: bool, env: &[(String, String)], timeout: Option<f64>) -> Result<(), Box<dyn Error>> {
    let log = log.unwrap_or_else(|| {
        let base = env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("6ixpack/logs");
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        base.join(format!("{}_{secs}.log", exe.file_stem().unwrap().to_string_lossy()))
    });
    if let Some(p) = log.parent() {
        fs::create_dir_all(p)?;
    }
    let file = fs::OpenOptions::new().create(true).append(true).open(&log)?;
    // The log is appended to; the summary reads only what this run wrote.
    let log_start = file.metadata().map(|m| m.len()).unwrap_or(0);
    let cdb_log = log.with_extension("cdb.txt");
    let mut argv: Vec<OsString> = vec![exe.as_os_str().into()];
    argv.extend(args.iter().cloned());
    if let Some(extra) = &debug {
        let cdb = find_cdb().ok_or("--debug needs cdb.exe (Windows SDK 'Debugging Tools for Windows')")?;
        let _ = fs::remove_file(&cdb_log);
        // The script runs at the initial break. First-chance exceptions are logged as
        // they happen; a first-chance AV gets the full fault dump (it may be handled,
        // so the run goes on); any break that stops the run (second-chance exception,
        // fast-fail) gets the same dump plus every thread, then cdb quits. A title
        // int 3 is a release-build assert that falls through: log it, step over it
        // and go on (without a debugger the title's own handler decides).
        // NtTerminateProcess is breakpointed so a title that kills itself (crash
        // handler, watchdog, anti-debug) leaves the caller's stack; no -G, so the exit
        // event itself is a break too and reports the exit code.
        // Commands go in a script file: Sandboxie's Start.exe re-splits a quoted -c argument.
        let script = log.with_extension("cdb.cmd");
        // Illegal instructions are the title's CPU feature probes: not logged; module
        // loads are not dumped (lm) since ModLoad lines already list them.
        let dump = ".echo ==== FAULT ====;.lastevent;.exr -1;r;!vprot @rip;.echo ==== code at rip ====;ub @rip L8;u @rip L4;.echo ==== stack ====;kn 40";
        // Plain --debug ignores C++/WinRT throws: each traced one costs a debugger
        // round trip and a stack walk, and the title throws many while running.
        let throws = if trace_throws {
            "sxe -c \".exr -1;kn 25;gn\" e06d7363
sxe -c \".exr -1;kn 25;gn\" e0000001
sxe -c \".exr -1;kn 25;gn\" 40080201"
        } else {
            "sxi e06d7363
sxi e0000001
sxi 40080201"
        };
        fs::write(&script, format!("{throws}
sxe -c \".echo ==== first-chance AV ====;{dump};gn\" av
sxi ii
sxe -c \".echo ==== int 3 (title assert, continued) ====;.lastevent;kn 12;.if (by(@rip) == 0xcc) {{r @rip=@rip+1}};gh\" bpe
sxe -c \".echo ==== process exit ====;.lastevent;q\" epr
bp ntdll!NtTerminateProcess \".echo ==== NtTerminateProcess ====;r rcx,rdx;kn 40;.echo ==== threads ====;~*kn 8;gc\"
{extra}
g
{dump}
.echo ==== threads ====
~*kn 8
q
"))?;
        // -hd: the debug heap that cdb otherwise turns on serializes every allocation.
        let mut d: Vec<OsString> = vec![cdb.into(), "-hd".into(), "-logo".into(), cdb_log.clone().into(), "-cf".into(), script.into()];
        d.extend(argv);
        argv = d;
    }
    if sandbox {
        let start = Path::new(&std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into())).join(r"Sandboxie-Plus\Start.exe");
        if !start.is_file() {
            return Err(format!("--sandbox needs Sandboxie-Plus ({} not found)", start.display()).into());
        }
        let bx = std::env::var("SANDBOXIE_BOX").unwrap_or_else(|_| "DefaultBox".into());
        let mut w: Vec<OsString> = vec![start.into(), format!("/box:{bx}").into(), "/wait".into()];
        w.extend(argv);
        argv = w;
    }
    println!("[launch] log: {}", log.display());
    if debug.is_some() {
        println!("[launch] debugger log: {}", cdb_log.display());
    }
    println!("[launch] starting {}", argv.iter().map(|a| a.to_string_lossy()).collect::<Vec<_>>().join(" "));
    let t = Instant::now();
    let mut child = Command::new(&argv[0])
        .current_dir(game)
        .args(&argv[1..])
        .env("TUXBOX_LOG", std::env::var("TUXBOX_LOG").unwrap_or_else(|_| "stderr".into()))
        // One readback every 120 presents: cheap, and every summary gets lit= samples.
        .env("TUXBOX_TRACE_PRESENT", std::env::var("TUXBOX_TRACE_PRESENT").unwrap_or_else(|_| "120".into()))
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    splash::watch(exe);
    let tee = |r: Box<dyn std::io::Read + Send>, mut f: fs::File| {
        std::thread::spawn(move || {
            for line in BufReader::new(r).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line);
                // The loading screen stays up until the title's first frame.
                if line.contains("completed PresentX #1 ") {
                    splash::close();
                } else if line.contains("D3D12") {
                    splash::status(0.8, "Waiting for the first frame");
                }
                eprintln!("{line}");
                let _ = writeln!(f, "{line}");
            }
        })
    };
    let a = tee(Box::new(child.stdout.take().unwrap()), file.try_clone()?);
    let b = tee(Box::new(child.stderr.take().unwrap()), file.try_clone()?);
    // --timeout: the launcher stops the run itself, so the summary below still prints
    // (an outside kill of the launcher loses it; `--summary LOG` recovers it then).
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if timeout.is_some_and(|limit| t.elapsed().as_secs_f64() >= limit) {
            println!("[launch] --timeout: stopping the run after {:.0}s", t.elapsed().as_secs_f64());
            let _ = child.kill();
            if let Some(name) = exe.file_name() {
                let _ = Command::new("taskkill").args(["/F", "/T", "/IM"]).arg(name).stdout(Stdio::null()).stderr(Stdio::null()).status();
            }
            break child.wait()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    };
    let _ = (a.join(), b.join());
    wait_for_image(exe);
    splash::close();
    println!("[launch] exited with {status}{} after {:.1}s", status.code().map(|c| format!(" (0x{:08x}{})", c as u32, ntstatus(c as u32))).unwrap_or_default(), t.elapsed().as_secs_f64());
    if debug.is_some() {
        // Sandboxie redirects the debugger's log write into the box; fetch it back.
        if !cdb_log.is_file() && sandbox {
            if let Some(boxed) = sandboxed_copy(&cdb_log) {
                let _ = fs::copy(boxed, &cdb_log);
            }
        }
        match fs::read_to_string(&cdb_log) {
            Ok(text) => {
                let mut f = file;
                let _ = writeln!(f, "---- cdb ----\n{text}");
                for l in debug_report(&text) {
                    println!("[launch:debug] {l}");
                }
            }
            Err(_) => println!("[launch:debug] no debugger log at {}", cdb_log.display()),
        }
    }
    println!("[launch] log: {}", log.display());
    let mut text = String::new();
    if let Ok(mut f) = fs::File::open(&log) {
        use std::io::{Read, Seek, SeekFrom};
        let _ = f.seek(SeekFrom::Start(log_start));
        let mut bytes = Vec::new();
        let _ = f.read_to_end(&mut bytes);
        text = String::from_utf8_lossy(&bytes).into_owned();
    }
    let (game_text, cdb_text) = text.split_once("\n---- cdb ----\n").unwrap_or((&text, ""));
    // Under --debug the child is cdb: the game's own exit code is in cdb's exit event.
    println!("{}", run_summary(game_text, cdb_exit_code(cdb_text).or(if debug.is_some() { None } else { status.code() })));
    for l in hot_lines(game_text) {
        println!("[launch] {l}");
    }
    // The fps above is not the game's own while these run: say which were on.
    let mut costly: Vec<String> = ["TUXBOX_LIST_RING", "TUXBOX_SURVEY", "TUXBOX_TRACE_RASTER_INPUTS", "TUXBOX_STRICT_LISTS"].iter()
        .filter(|k| std::env::var(k).is_ok_and(|v| !v.is_empty() && v != "0")).map(|k| k.to_string()).collect();
    if debug.is_some() {
        costly.push(if trace_throws { "--debug-full".into() } else { "--debug".into() });
    }
    if !costly.is_empty() {
        println!("[launch] fps measured with diagnostics on: {}", costly.join(" "));
    }
    Ok(())
}

/// `--summary LOG`: the end-of-run report of a log written earlier (or cut off).
fn print_summary(log: &Path) -> Result<(), Box<dyn Error>> {
    let bytes = fs::read(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let text = String::from_utf8_lossy(&bytes);
    let (game_text, cdb_text) = text.split_once("\n---- cdb ----\n").unwrap_or((&text, ""));
    // A killed launcher never appended the cdb log; read it from beside the game log.
    let side = if cdb_text.is_empty() {
        fs::read(log.with_extension("cdb.txt")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    } else {
        String::new()
    };
    let cdb_text = if cdb_text.is_empty() { side.as_str() } else { cdb_text };
    for l in debug_report(cdb_text) {
        println!("[launch:debug] {l}");
    }
    println!("{}", run_summary(game_text, cdb_exit_code(cdb_text)));
    for l in hot_lines(game_text) {
        println!("[launch] {l}");
    }
    Ok(())
}

/// The last heartbeat's "hot thread ..." lines (where the busiest threads
/// execute), with each provider "module+0xRVA" named from the link map
/// written beside the provider DLL.
fn hot_lines(text: &str) -> Vec<String> {
    let lines: Vec<&str> = text.lines().collect();
    let Some(last) = lines.iter().rposition(|l| l.contains("hot thread ")) else { return Vec::new() };
    let first = lines[..last].iter().rposition(|l| !l.contains("hot thread ")).map_or(0, |i| i + 1);
    let out = env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("6ixpack/tuxbox");
    let mut maps: HashMap<String, Option<Vec<(u64, String)>>> = HashMap::new();
    lines[first..=last].iter().filter_map(|l| l.split_once("hot thread ")).map(|(_, rest)| {
        let named: Vec<String> = rest.split(' ').map(|word| {
            let core = word.trim_end_matches([',', ';']);
            let tail = &word[core.len()..];
            let Some((module, rva)) = core.split_once("+0x") else { return word.to_owned() };
            let Ok(rva) = u64::from_str_radix(rva, 16) else { return word.to_owned() };
            let symbols = maps.entry(module.to_owned()).or_insert_with(|| map_symbols(&out.join(format!("{module}.map"))));
            match symbols.as_ref().and_then(|v| v[..v.partition_point(|e| e.0 <= rva)].last()) {
                Some((start, name)) if rva - start < 0x10000 => format!("{module}!{name}{tail}"),
                _ => word.to_owned(),
            }
        }).collect();
        format!("hot thread {}", named.join(" "))
    }).collect()
}

/// (RVA, name) of every symbol in an MSVC link map, by address.
fn map_symbols(map: &Path) -> Option<Vec<(u64, String)>> {
    let text = String::from_utf8_lossy(&fs::read(map).ok()?).into_owned();
    let base = text.lines().find_map(|l| l.trim().strip_prefix("Preferred load address is "))
        .and_then(|h| u64::from_str_radix(h.trim(), 16).ok())?;
    let mut symbols: Vec<(u64, String)> = text.lines().filter_map(|l| {
        let mut fields = l.split_whitespace();
        let (place, name, address) = (fields.next()?, fields.next()?, fields.next()?);
        if place.len() != 13 || place.as_bytes()[4] != b':' || address.len() != 16 {
            return None;
        }
        Some((u64::from_str_radix(address, 16).ok()?.checked_sub(base)?, demangle(name)))
    }).collect();
    symbols.sort();
    Some(symbols)
}

/// Rust's legacy symbol names ("_ZN4core3ptr4read17h0123456789abcdefE") as paths.
fn demangle(name: &str) -> String {
    let Some(mut rest) = name.strip_prefix("_ZN").and_then(|r| r.strip_suffix('E')) else { return name.to_owned() };
    let mut parts = Vec::new();
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        let Ok(len) = rest[..digits].parse::<usize>() else { return name.to_owned() };
        let Some(part) = rest.get(digits..digits + len) else { return name.to_owned() };
        rest = &rest[digits + len..];
        // the trailing hash segment
        if rest.is_empty() && len == 17 && part.starts_with('h') {
            break;
        }
        let mut part = part.to_owned();
        for (from, to) in [("$LT$", "<"), ("$GT$", ">"), ("$u20$", " "), ("$RF$", "&"), ("$BP$", "*"), ("$C$", ","),
            ("$LP$", "("), ("$RP$", ")"), ("$u7b$", "{"), ("$u7d$", "}"), ("$u5b$", "["), ("$u5d$", "]"), ("..", "::")] {
            part = part.replace(from, to);
        }
        parts.push(part.trim_start_matches('_').to_owned());
    }
    parts.join("::").replace(' ', "")
}

/// The debuggee's exit code from cdb's "Exit process 0:pid, code X" event.
fn cdb_exit_code(cdb: &str) -> Option<i32> {
    let line = cdb.lines().rev().find(|l| l.contains("Exit process") && l.contains(", code "))?;
    let hex = line.rsplit(", code ").next()?.split_whitespace().next()?;
    u32::from_str_radix(hex, 16).ok().map(|c| c as i32)
}

/// The lines of a cdb log worth printing: the last fault dump (or, failing that,
/// the last NtTerminateProcess stack), the exit event and the first-chance codes.
fn debug_report(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    // Headers are .echo output, so they start the line (the command echoes start "0:000>").
    let block = |marker: &str| -> Option<&[&str]> {
        let start = lines.iter().rposition(|l| l.starts_with(marker))?;
        let sub = |l: &&str| l.starts_with("==== code at rip") || l.starts_with("==== stack");
        let end = lines[start + 1..].iter().position(|l| l.starts_with("====") && !sub(l)).map_or(lines.len(), |i| start + 1 + i);
        Some(&lines[start..end.min(start + 80)])
    };
    let mut out: Vec<&str> = Vec::new();
    let mut codes: Vec<&str> = lines.iter().copied().filter(|l| l.contains("- code ") && l.contains("chance")).collect();
    codes.dedup();
    out.extend(codes.into_iter().rev().take(6).rev());
    if let Some(b) = block("==== FAULT ====").or_else(|| block("==== NtTerminateProcess ====")) {
        out.extend_from_slice(b);
    }
    out.extend(lines.iter().copied().filter(|l| l.starts_with("Last event:") && l.contains("Exit process")).take(1));
    out
}

/// One line from this run's game log: exit code, how far it got, first fatal line
/// and the counts of the diagnostics that say what broke.
fn run_summary(text: &str, code: Option<i32>) -> String {
    // Rate-limited lines carry "(N so far)": the largest N is the true count.
    let count = |needle: &str| {
        let hits: Vec<&str> = text.lines().filter(|l| l.contains(needle)).collect();
        hits.iter().filter_map(|l| l.rsplit_once(" so far)")?.0.rsplit('(').next()?.trim().parse::<usize>().ok()).max().unwrap_or(0).max(hits.len())
    };
    let presents = text.lines().filter_map(|l| l.split("completed PresentX #").nth(1))
        .filter_map(|r| r.split(|c: char| !c.is_ascii_digit()).next()?.parse::<u64>().ok()).max().unwrap_or(0);
    let lits: Vec<&str> = text.lines().filter_map(|l| l.split(" lit=").nth(1)?.split(' ').next()).collect();
    let fatal = text.lines().find(|l| {
        let l = l.to_ascii_lowercase();
        ["panicked", "presentx failed", "device removed", "0x887a0005", "0x887a0006", "fatal", "access violation", "stack overflow"].iter().any(|k| l.contains(k))
    });
    // Heartbeat: "completed PresentX #N (x fps)" or "(x fps; ms/frame: record a, close b, ...)".
    let beat = text.lines().rev().find_map(|l| l.split("completed PresentX #").nth(1)?.split_once(" (").map(|(_, r)| r.trim_end().strip_suffix(')').unwrap_or(r.trim_end()).to_owned()));
    let (beat, top) = match beat.as_deref().map(|b| b.split_once("; top sites ms/frame: ").unwrap_or((b, ""))) {
        Some((b, t)) => (Some(b.to_owned()), t.to_owned()),
        None => (None, String::new()),
    };
    let (fps, split) = match beat.as_deref().map(|b| b.split_once("; ms/frame: ").unwrap_or((b, ""))) {
        Some((f, s)) => (f.trim_end_matches(" fps").to_owned(), s.replace(", ", ",").replace(" (worker)", "").replace(' ', "-")),
        None => ("-".into(), String::new()),
    };
    let fps = if split.is_empty() { fps } else { format!("{fps} ms/frame={split}") };
    let fps = if top.is_empty() { fps } else { format!("{fps} top-record=[{top}]") };
    // "frame time X ms: gpu latency Y ms (...); cpu ms/frame: ..." from the heartbeat.
    let frame = text.lines().rev().find_map(|l| l.split_once("frame time ").filter(|(_, r)| r.contains("gpu latency")).map(|(_, r)| r.trim().to_owned()));
    let fps = match frame { Some(f) => format!("{fps} frame=[{f}]"), None => fps };
    let exit = code.map(|c| format!("0x{:08x}{}", c as u32, ntstatus(c as u32))).unwrap_or_else(|| "none".into());
    format!(
        "[launch] summary: exit={exit} presents={presents} fps={fps} lit={}x last={} close-failed={} rebuilt={} dropped-batches={} unresolved-desc={} presentx-failed={} \
refused-calls={} graphics-only-drops={} barrier-rewrites={} slot131-cache-barriers={} null-root-views={} barriers-fitted={} probe-rejects={} cannot-record={} unbalanced-closes={} first-fatal={}",
        lits.len(), lits.last().unwrap_or(&"-"), count("host list Close failed"), count("rebuilt host list"),
        count("dropped a command-list batch"), count("unresolved inline descriptor"), count("PresentX failed"),
        count("dropped one refused recording call"), count("dropped a graphics-only call"),
        count("recorded as") - count("list slot 131 recorded as cache barrier"),
        count("list slot 131 recorded as cache barrier"), count("root view at uncommitted title address"), count("fitted to host"), count("barrier probe: host Close"),
        count("command list cannot record"), count("closing a list with"),
        fatal.map(|l| format!("{:?}", l.trim())).unwrap_or_else(|| "none".into())
    )
}

fn find_cdb() -> Option<PathBuf> {
    ["ProgramFiles(x86)", "ProgramFiles"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .map(|p| Path::new(&p).join(r"Windows Kits\10\Debuggers\x64\cdb.exe"))
        .find(|p| p.is_file())
}

/// Where Sandboxie puts a file the boxed process wrote under the user profile.
fn sandboxed_copy(p: &Path) -> Option<PathBuf> {
    boxed_path(p).filter(|c| c.is_file())
}

/// The box's copy of a path under the user profile.
fn boxed_path(p: &Path) -> Option<PathBuf> {
    let user = std::env::var("USERNAME").ok()?;
    let bx = std::env::var("SANDBOXIE_BOX").unwrap_or_else(|_| "DefaultBox".into());
    let profile = env_path("USERPROFILE")?;
    let rel = p.strip_prefix(&profile).ok()?;
    Some(Path::new(r"C:\Sandbox").join(user).join(bx).join(r"user\current").join(rel))
}

fn ntstatus(c: u32) -> &'static str {
    match c {
        0xc0000005 => ", access violation",
        0xc0000409 => ", fail-fast / stack buffer overrun",
        0xc0000135 => ", DLL not found",
        0xc0000139 => ", entry point (import) not found",
        0xc000007b => ", bad DLL image",
        0xc0000142 => ", DLL init failed",
        0xc000001d => ", illegal instruction",
        0x80000003 => ", breakpoint",
        _ => "",
    }
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let result = run(&args);
    splash::close();
    if let Err(e) = result {
        eprintln!("neonbridge: {e}");
        std::process::exit(1);
    }
}
