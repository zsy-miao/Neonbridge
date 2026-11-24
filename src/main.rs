//! 6ixlaunch: run a dumped Xbox Series (Scarlett / GDK) executable on
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

const USAGE: &str = "usage: 6ixlaunch --game <dir> [--tuxbox <TuxBox checkout>] [--exe <name>] [--log <file>]
                      [--build-only | --stage-only | --restore] [-- <game args>]
  --game       dumped package folder (holds MicrosoftGame.Config and the exe)
  --tuxbox     provider source (default: the vendored vendor/tuxbox, or env TUXBOX_ROOT)
  providers build into %LOCALAPPDATA%/6ixpack/tuxbox
  --exe        executable name (default: the MicrosoftGame.Config Executable, else the only x64 exe)
  --log        game log file (default %LOCALAPPDATA%/6ixpack/logs/<exe>_<unix time>.log)
  --build-only build the providers, touch nothing in the game folder
  --stage-only build and stage the providers beside the exe, do not start it
  --restore    remove staged providers and put the original game files back
  --debug      run under cdb (Windows Kits debugger): first-chance exceptions, the
               crash event, registers, stack and loaded modules go to the log
  --sandbox    run inside Sandboxie-Plus (box DefaultBox, or env SANDBOXIE_BOX); the
               game's writes stay in the box; with --debug the cdb log is fetched back
  --cdb FILE   extra cdb commands (breakpoints etc.) run before the game starts; implies --debug
Needs Visual Studio Build Tools (x64 C++), cargo and the x86_64-pc-windows-msvc target.";

/// (cargo package, output DLL, export table relative to the TuxBox root, extra import libs, extra C++ sources)
const PROVIDERS: &[(&str, &str, &str, &[&str], &[&str])] = &[
    ("tuxbox-xmem-http", "tuxbox_xhttp.dll", "runtime/xmem/xhttp.def", &["winhttp.lib"], &[]),
    ("tuxbox-xmem", "xmem.dll", "runtime/xmem/xmem.def", &["tuxbox_xhttp.lib", "/include:DllMain"], &[]), // own DllMain must win over the libcmt stub; forwards XMemSetWinHttpHooks to tuxbox_xhttp
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
    Restore,
}

fn run(args: &[OsString]) -> Result<(), Box<dyn Error>> {
    let (mut game, mut tuxbox, mut exe, mut log, mut mode, mut game_args) = (None, env_path("TUXBOX_ROOT"), None, None, Mode::Run, Vec::new());
    let (mut debug, mut sandbox, mut cdb_extra) = (false, false, None);
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
            Some("--restore") => mode = Mode::Restore,
            Some("--debug") => debug = true,
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

    let out = env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("6ixpack/tuxbox");
    let dlls = build_providers(&tuxbox, &out)?;
    if mode == Mode::BuildOnly {
        println!("[launch] built {} providers in {}", dlls.len(), out.display());
        return Ok(());
    }
    let (game, target) = (game.unwrap(), target.unwrap());
    stage(&game, &dlls)?;
    if mode == Mode::StageOnly {
        println!("[launch] staged {} providers beside {}; not started (--stage-only)", dlls.len(), target.display());
        return Ok(());
    }
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
                match shaders::Worker::start(shaders::Tools { xshader, dxc }, cache_root, session.clone(),
                    move |line| { let _ = writeln!(sink.lock().unwrap(), "{line}"); }) {
                    Ok(w) => Some((w, name, session, log_path)),
                    Err(e) => { println!("[launch] shader worker not started: {e}"); None }
                }
            }
            (Err(e), _) => { println!("[launch] shader worker not started: xshader build failed: {e}"); None }
            (_, None) => { println!("[launch] shader worker not started: no Windows SDK dxc.exe with dxil.dll (or set DXC)"); None }
        }
    };
    let env: Vec<(String, String)> = worker.iter().map(|(_, name, _, _)| ("TUXBOX_SHADER_SESSION".to_owned(), name.clone())).collect();
    let result = start(&game, &target, &game_args, log, debug.then(|| cdb_extra.unwrap_or_default()), sandbox, &env);
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
fn start(game: &Path, exe: &Path, args: &[OsString], log: Option<PathBuf>, debug: Option<String>, sandbox: bool, env: &[(String, String)]) -> Result<(), Box<dyn Error>> {
    let log = log.unwrap_or_else(|| {
        let base = env_path("LOCALAPPDATA").unwrap_or_else(std::env::temp_dir).join("6ixpack/logs");
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        base.join(format!("{}_{secs}.log", exe.file_stem().unwrap().to_string_lossy()))
    });
    if let Some(p) = log.parent() {
        fs::create_dir_all(p)?;
    }
    let file = fs::OpenOptions::new().create(true).append(true).open(&log)?;
    let cdb_log = log.with_extension("cdb.txt");
    let mut argv: Vec<OsString> = vec![exe.as_os_str().into()];
    argv.extend(args.iter().cloned());
    if let Some(extra) = &debug {
        let cdb = find_cdb().ok_or("--debug needs cdb.exe (Windows SDK 'Debugging Tools for Windows')")?;
        let _ = fs::remove_file(&cdb_log);
        // -g/-G: skip the initial and final breaks; first-chance exceptions are logged
        // as they happen; at the fatal event print it, registers, stack and modules.
        // Commands go in a script file: Sandboxie's Start.exe re-splits a quoted -c argument.
        let script = log.with_extension("cdb.cmd");
        // Trace every C++ (e06d7363), msxml/app (e0000001) and WinRT-originate (40080201)
        // exception with its record and stack, then dump the fatal event. Illegal
        // instructions are the title's CPU feature probes: not logged.
        fs::write(&script, format!("sxe -c \".exr -1;kn 25;gn\" e06d7363
sxe -c \".exr -1;kn 25;gn\" e0000001
sxe -c \".exr -1;kn 25;gn\" 40080201
sxe -c \".exr -1;kn 25;gn\" av
sxi ii
{extra}
g
.lastevent
r
kn 60
.ecxr
kn 60
q
"))?;
        // -hd: the debug heap that cdb otherwise turns on serializes every allocation.
        let mut d: Vec<OsString> = vec![cdb.into(), "-G".into(), "-hd".into(), "-logo".into(), cdb_log.clone().into(), "-cf".into(), script.into()];
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
        .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let tee = |r: Box<dyn std::io::Read + Send>, mut f: fs::File| {
        std::thread::spawn(move || {
            for line in BufReader::new(r).split(b'\n').map_while(Result::ok) {
                let line = String::from_utf8_lossy(&line);
                eprintln!("{line}");
                let _ = writeln!(f, "{line}");
            }
        })
    };
    let a = tee(Box::new(child.stdout.take().unwrap()), file.try_clone()?);
    let b = tee(Box::new(child.stderr.take().unwrap()), file.try_clone()?);
    let status = child.wait()?;
    let _ = (a.join(), b.join());
    wait_for_image(exe);
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
                for l in text.lines().filter(|l| l.contains("chance") || l.starts_with("Subcode") || l.starts_with("Last event")).take(12) {
                    println!("[launch:debug] {l}");
                }
                let stack: Vec<_> = text.lines().filter(|l| l.len() > 3 && l.as_bytes()[2] == b' ' && l[..2].chars().all(|c| c.is_ascii_hexdigit())).take(12).collect();
                for l in stack {
                    println!("[launch:debug] {l}");
                }
            }
            Err(_) => println!("[launch:debug] no debugger log at {}", cdb_log.display()),
        }
    }
    println!("[launch] log: {}", log.display());
    Ok(())
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
    if let Err(e) = run(&args) {
        eprintln!("6ixlaunch: {e}");
        std::process::exit(1);
    }
}
