use std::path::Path;

use anyhow::{Context, Result};

use crate::{Console, Unimplemented};

#[cfg(windows)]
pub fn run(
    exe: &Path,
    unimplemented: Unimplemented,
    strict: bool,
    execute: bool,
    console: Console,
) -> Result<()> {
    use std::sync::Arc;
    use victory_hle::{HleResolver, HostPolicy};
    use victory_loader::{BindReport, UnimplementedPolicy, WithTitleDlls};

    // A title that relaunched itself: the old process must be gone first.
    victory_hle::modules::lifecycle::wait_for_previous();
    let cfg = victory_core::Config::load()?;
    let original = std::fs::read(exe).with_context(|| format!("reading {}", exe.display()))?;
    let prepared = victory_loader::prepare(&original)?;
    let hash = prepared.original_hash.to_hex();
    match victory_game::identify(&hash) {
        Some(build) => println!("build          {} ({hash})", build.label),
        None => println!("build          unknown ({hash})"),
    }

    let images = cfg.cache_subdir("images")?;
    victory_hle::modules::d3d12x::use_shader_cache(cfg.cache_subdir("shaders")?);
    let image = victory_loader::load(&prepared, &images)?;
    // The program is mapped from the copy `load` wrote: the file's bytes
    // and their patched copy are not read again, and were held for the
    // whole run (two blocks of 114.7 MB each for the title, in
    // physical memory: run 65's memory map).
    drop(prepared);
    drop(original);
    victory_hle::runtime::set_main_image(image.base());
    victory_hle::runtime::set_image_path(image.base(), exe);
    println!(
        "mapped at      {:#x} (preferred {:#x})",
        image.base(),
        image.info().image_base
    );
    let resolver = HleResolver::new(if strict {
        HostPolicy::none()
    } else {
        HostPolicy::default()
    });
    let wanted = |dll: &str| title_dll(&resolver.host, dll);
    let title_dir = exe.parent().unwrap_or(Path::new("."));
    // A missing manifest leaves id 0 and no storage quota.
    let manifest =
        std::fs::read_to_string(title_dir.join("MicrosoftGame.config")).unwrap_or_default();
    let title = victory_hle::runtime::Title::from_manifest(&manifest);
    let storage = cfg.data_subdir(&format!("{:08X}/storage", title.id))?;
    println!(
        "title          {:08X}, storage {} ({} MB)",
        title.id,
        storage.display(),
        title.storage_bytes >> 20
    );
    // The console's fixed drives: the package as `G:`, temporary files `T:`.
    victory_hle::modules::files::mount_at('G', title_dir);
    let temp = cfg.data_subdir(&format!("{:08X}/temp", title.id))?;
    victory_hle::modules::files::mount_at('T', &temp);
    victory_hle::runtime::set_title(victory_hle::runtime::Title { storage, ..title });
    victory_hle::console::set(match console {
        Console::SeriesX => victory_hle::console::Console::SeriesX,
        Console::SeriesS => victory_hle::console::Console::SeriesS,
    });
    println!("console        {}", victory_hle::console::current().name());
    let dlls = victory_loader::load_title_dlls(image.info(), title_dir, &images, &wanted)?;
    for (name, dll) in &dlls {
        println!("title dll      {name} at {:#x}", dll.base());
        victory_hle::runtime::set_image_path(dll.base(), &title_dir.join(name));
    }

    let policy = match unimplemented {
        Unimplemented::Trap => UnimplementedPolicy::Trap,
        Unimplemented::Zero => UnimplementedPolicy::ReturnZero,
    };
    let with_dlls = WithTitleDlls {
        inner: &resolver,
        dlls: &dlls,
    };
    let show = |name: &str, report: BindReport| {
        println!(
            "bound          {name}: {} hle, {} host, {} title, {} missing",
            report.hle,
            report.host,
            report.title,
            report.missing.len()
        );
        for m in &report.missing {
            println!("  missing      {}!{}", m.dll, m.symbol);
        }
    };
    for (name, dll) in &dlls {
        show(name, dll.bind_imports(&with_dlls, policy)?);
    }
    let exe_name = exe.file_name().unwrap_or_default().to_string_lossy();
    show(&exe_name, image.bind_imports(&with_dlls, policy)?);
    if !execute {
        println!("not executing (pass --execute to start the title)");
        return Ok(());
    }
    victory_loader::report_crashes();
    victory_loader::log_crashes_to(temp.with_file_name("crashes.log"));
    victory_hle::runtime::use_console_timers();
    #[cfg(feature = "diagnostics")]
    diagnostics(&image, &cfg);
    let late = LateDlls {
        host: resolver.host.clone(),
        title_dir: title_dir.to_owned(),
        cache_dir: images,
        policy,
        mapped: std::sync::Mutex::new(dlls.clone()),
        lock: LoaderLock::default(),
    };
    victory_hle::runtime::set_title_dll_loader(Box::new(move |file| {
        late.load(file, show).map_err(|e| format!("{e:#}"))
    }));
    // SAFETY: imports are bound; from here on we run title code.
    let main = unsafe { Arc::new(image).spawn_main(dlls)? };
    let code = main
        .join()
        .map_err(|_| anyhow::anyhow!("title main thread panicked"))??;
    println!("title exited with {code}");
    Ok(())
}

/// The debugging aids (README): scheduled stack dumps, watched values and
/// memory saved on a crash.
#[cfg(all(windows, feature = "diagnostics"))]
fn diagnostics(image: &victory_loader::LoadedImage, cfg: &victory_core::Config) {
    victory_loader::dump_stacks_on_schedule();
    victory_hle::modules::audio::capture::start();
    watch(image.base(), image.info().size_of_image as usize);
    upscaler_override(image);
    ffx_scale(image);
    // Debugging aid: `VICTORY_CRASH_SAVE=<rva>:<len>,...` (hex) writes those
    // title ranges to `crash-<rva>.bin` in the cache dir on a crash.
    let size = image.info().size_of_image as usize;
    let ranges = std::env::var("VICTORY_CRASH_SAVE").unwrap_or_default();
    let hex = |s: &str| usize::from_str_radix(s.trim().trim_start_matches("0x"), 16).ok();
    victory_loader::save_on_crash(
        ranges
            .split(',')
            .filter_map(|r| r.split_once(':'))
            .filter_map(|(rva, len)| Some((hex(rva)?, hex(len)?)))
            .filter(|&(rva, len)| rva.checked_add(len).is_some_and(|end| end <= size))
            .map(|(rva, len)| {
                let file = cfg.cache_dir.join(format!("crash-{rva:x}.bin"));
                (image.base() + rva, len, file)
            })
            .collect(),
    );
}

/// Debugging aid: `VICTORY_UPSCALER=<rva>[,<kind>]` (hex rva, decimal kind,
/// default 7) makes the title's upscaler picker return `kind`. In 1.0.2.1 the
/// picker is at 7E93F0 and reached by 4 direct calls; it returns 7 (AMD FSR)
/// only on Series S and 6 (the title's own upscaler) otherwise. The count of
/// direct calls and jumps to the rva is logged; with none it is the wrong rva
/// for this executable and nothing is patched.
#[cfg(all(windows, feature = "diagnostics"))]
fn upscaler_override(image: &victory_loader::LoadedImage) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
    let Ok(spec) = std::env::var("VICTORY_UPSCALER") else {
        return;
    };
    let (rva, kind) = spec.split_once(',').unwrap_or((&spec, "7"));
    let rva = usize::from_str_radix(rva.trim().trim_start_matches("0x"), 16);
    let (Ok(rva), Ok(kind)) = (rva, kind.trim().parse::<u32>()) else {
        eprintln!("VIctory: VICTORY_UPSCALER={spec}: expected <hex rva>[,<kind>]");
        return;
    };
    let base = image.base();
    let mut calls = 0;
    for s in &image.info().sections {
        if s.characteristics & IMAGE_SCN_MEM_EXECUTE == 0 {
            continue;
        }
        let start = s.rva as usize;
        // SAFETY: an executable section of the mapped image, readable.
        let code = unsafe {
            std::slice::from_raw_parts((base + start) as *const u8, s.virtual_size as usize)
        };
        for (i, w) in code.windows(5).enumerate() {
            let rel = i64::from(i32::from_le_bytes([w[1], w[2], w[3], w[4]]));
            if matches!(w[0], 0xE8 | 0xE9) && (start + i + 5) as i64 + rel == rva as i64 {
                calls += 1;
            }
        }
    }
    if calls == 0 || rva + 6 > image.info().size_of_image as usize {
        eprintln!(
            "VIctory: upscaler picker at title+{rva:#x} not patched: no direct call to it (wrong rva?)"
        );
        return;
    }
    let mut stub = [0xB8, 0, 0, 0, 0, 0xC3]; // mov eax, kind; ret
    stub[1..5].copy_from_slice(&kind.to_le_bytes());
    let at = (base + rva) as *mut u8;
    let mut old = 0;
    // SAFETY: six bytes at a function entry inside the image; no title code
    // runs yet.
    unsafe {
        VirtualProtect(at.cast(), stub.len(), PAGE_EXECUTE_READWRITE, &mut old);
        at.copy_from_nonoverlapping(stub.as_ptr(), stub.len());
        VirtualProtect(at.cast(), stub.len(), old, &mut old);
    }
    eprintln!("VIctory: upscaler picker at title+{rva:#x} returns {kind} ({calls} direct calls)");
}

/// Debugging aid: `VICTORY_FFX_SCALE=<scale>[,<hex rva>]` sets the render
/// scale of the title's FidelityFX path (0.5: 1080p for a 4K output, 0.6667:
/// 1440p). The rva is the method that picks the scale (1.0.2.1: 28EF680, the
/// default); it takes 2/3 on Series S and 1.0 on Series X, so Series X with
/// FidelityFX forced on renders at full 4K, which overflows its memory plan.
/// Both of its scale loads (`vmovss xmm0, [rip+d]`) are pointed at our
/// constant, kept in the padding after the method; with other code there,
/// nothing is patched.
#[cfg(all(windows, feature = "diagnostics"))]
fn ffx_scale(image: &victory_loader::LoadedImage) {
    use windows_sys::Win32::System::Memory::{PAGE_EXECUTE_READWRITE, VirtualProtect};
    let Ok(spec) = std::env::var("VICTORY_FFX_SCALE") else {
        return;
    };
    let (scale, rva) = spec.split_once(',').unwrap_or((&spec, "28EF680"));
    let scale = scale
        .trim()
        .parse::<f32>()
        .ok()
        .filter(|s| *s > 0.0 && *s <= 1.0);
    let rva = usize::from_str_radix(rva.trim().trim_start_matches("0x"), 16).ok();
    let (Some(scale), Some(rva)) = (scale, rva) else {
        eprintln!("VIctory: VICTORY_FFX_SCALE={spec}: expected <scale in (0, 1]>[,<hex rva>]");
        return;
    };
    if rva + FFX_SCALE_SPAN > image.info().size_of_image as usize {
        eprintln!("VIctory: VICTORY_FFX_SCALE: title+{rva:#x} is outside the image");
        return;
    }
    let at = (image.base() + rva) as *mut u8;
    let mut old = 0;
    // SAFETY: the method's first bytes and its padding, inside the mapped
    // image (checked above); no title code runs yet.
    let patched = unsafe {
        VirtualProtect(at.cast(), FFX_SCALE_SPAN, PAGE_EXECUTE_READWRITE, &mut old);
        let patched = patch_ffx_scale(std::slice::from_raw_parts_mut(at, FFX_SCALE_SPAN), scale);
        VirtualProtect(at.cast(), FFX_SCALE_SPAN, old, &mut old);
        patched
    };
    if patched {
        eprintln!("VIctory: FidelityFX render scale at title+{rva:#x} set to {scale}");
    } else {
        eprintln!(
            "VIctory: FidelityFX scale not patched: unexpected code at title+{rva:#x} (wrong rva?)"
        );
    }
}

/// The bytes of the scale method `patch_ffx_scale` reads and writes.
#[cfg(all(windows, feature = "diagnostics"))]
const FFX_SCALE_SPAN: usize = 0x68;

/// Points both scale loads of the method `code` starts with
/// (`vmovss xmm0, [rip+d]` at +0x28 and +0x32) at `scale`, written into the
/// method's padding at +0x64. False, with nothing written, for other code.
#[cfg(all(windows, feature = "diagnostics"))]
fn patch_ffx_scale(code: &mut [u8], scale: f32) -> bool {
    const LOADS: [usize; 2] = [0x28, 0x32];
    const SLOT: usize = 0x64;
    const VMOVSS_RIP: [u8; 4] = [0xC5, 0xFA, 0x10, 0x05];
    if code.len() < FFX_SCALE_SPAN
        || LOADS.iter().any(|&l| code[l..l + 4] != VMOVSS_RIP)
        || code[SLOT..SLOT + 4] != [0xCC; 4]
    {
        return false;
    }
    code[SLOT..SLOT + 4].copy_from_slice(&scale.to_le_bytes());
    for l in LOADS {
        let disp = (SLOT - (l + 8)) as i32;
        code[l + 4..l + 8].copy_from_slice(&disp.to_le_bytes());
    }
    true
}

/// Debugging aid: `VICTORY_WATCH=<rva>,...` (hex) logs the u32 at each
/// title+rva whenever it changes, polled every 5 ms (an engine's state, say).
#[cfg(all(windows, feature = "diagnostics"))]
fn watch(base: usize, size: usize) {
    let Ok(list) = std::env::var("VICTORY_WATCH") else {
        return;
    };
    let rvas: Vec<usize> = list
        .split(',')
        .filter_map(|r| usize::from_str_radix(r.trim().trim_start_matches("0x"), 16).ok())
        .filter(|&rva| rva + 4 <= size)
        .collect();
    std::thread::spawn(move || {
        let mut seen = vec![None; rvas.len()];
        loop {
            for (&rva, seen) in rvas.iter().zip(&mut seen) {
                // SAFETY: four bytes inside the mapped image, checked above.
                let value = unsafe { ((base + rva) as *const u32).read_volatile() };
                if *seen != Some(value) {
                    eprintln!("VIctory: title+{rva:#x} = {value:#x}");
                    *seen = Some(value);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    });
}

/// DLLs the host or our runtime provide are never taken from the title.
#[cfg(windows)]
fn title_dll(host: &victory_hle::HostPolicy, dll: &str) -> bool {
    !host.allows(dll) && !victory_hle::registry().iter().any(|m| m.serves(dll))
}

/// Title DLLs the title loads itself (`LoadLibrary`) that nothing imports,
/// e.g. The title's upscaler behind its FidelityFX loader.
#[cfg(windows)]
struct LateDlls {
    host: victory_hle::HostPolicy,
    title_dir: std::path::PathBuf,
    cache_dir: std::path::PathBuf,
    policy: victory_loader::UnimplementedPolicy,
    /// Every title DLL mapped so far, to bind new ones against.
    mapped: std::sync::Mutex<Vec<(String, victory_loader::LoadedImage)>>,
    lock: LoaderLock,
}

#[cfg(windows)]
impl LateDlls {
    /// Every `LoadLibrary` of the title comes here first. Returns a title
    /// image called `file`, mapping it and the title DLLs it needs first,
    /// binding them and running their `DllMain` on the calling thread, as
    /// the Windows loader would; `None` if `file` is no title image.
    fn load(
        &self,
        file: &str,
        show: impl Fn(&str, victory_loader::BindReport),
    ) -> Result<Option<usize>> {
        use victory_hle::runtime::{forget_image, image_named, set_image_path};
        // Waits while another thread initialises a DLL; a `DllMain` that
        // loads another DLL re-enters.
        self.lock.hold(|| {
            if let Some(base) = image_named(file) {
                return Ok(Some(base));
            }
            let new = {
                let mut mapped = self.mapped.lock().unwrap_or_else(|e| e.into_inner());
                let wanted = |dll: &str| title_dll(&self.host, dll) && image_named(dll).is_none();
                let new = victory_loader::load_title_dll(
                    file,
                    &self.title_dir,
                    &self.cache_dir,
                    &wanted,
                )?;
                let hle = victory_hle::HleResolver::new(self.host.clone());
                let old = victory_loader::WithTitleDlls {
                    inner: &hle,
                    dlls: &mapped,
                };
                let resolver = victory_loader::WithTitleDlls {
                    inner: &old,
                    dlls: &new,
                };
                for (name, dll) in &new {
                    println!(
                        "title dll      {name} at {:#x} (loaded by the title)",
                        dll.base()
                    );
                    show(name, dll.bind_imports(&resolver, self.policy)?);
                    set_image_path(dll.base(), &self.title_dir.join(name));
                }
                // Visible before their `DllMain`s run, which may load more.
                mapped.extend(new.iter().cloned());
                new
            };
            let failed = new.iter().enumerate().find_map(|(i, (name, dll))| {
                // SAFETY: bound above, dependencies attached first.
                let attached = unsafe { dll.attach() };
                attached
                    .err()
                    .map(|e| (i, e.context(format!("attaching {name}"))))
            });
            let Some((i, error)) = failed else {
                return Ok(new.last().map(|(_, dll)| dll.base()));
            };
            // As `LoadLibrary` does when a `DllMain` fails: detach what this
            // load attached (the failed one too) and unmap all of it.
            let bases: Vec<_> = new.iter().map(|(_, d)| d.base()).collect();
            self.mapped
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|(_, d)| !bases.contains(&d.base()));
            for (_, done) in new[..=i].iter().rev() {
                // SAFETY: attached (or failing to) on this thread just now.
                unsafe { done.detach() };
            }
            for (_, dll) in new {
                forget_image(dll.base());
                // SAFETY: detached, and no title code holds it yet.
                unsafe { dll.unload() };
            }
            Err(error)
        })
    }
}

/// The Windows loader lock's shape: one thread at a time, re-entered by
/// the thread that holds it.
#[cfg(windows)]
#[derive(Default)]
struct LoaderLock {
    owner: std::sync::Mutex<(Option<std::thread::ThreadId>, usize)>,
    free: std::sync::Condvar,
}

#[cfg(windows)]
impl LoaderLock {
    fn hold<T>(&self, f: impl FnOnce() -> T) -> T {
        let me = std::thread::current().id();
        let lock = || self.owner.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut owner = lock();
            while owner.0.is_some_and(|o| o != me) {
                owner = self.free.wait(owner).unwrap_or_else(|e| e.into_inner());
            }
            *owner = (Some(me), owner.1 + 1);
        }
        let result = f();
        let mut owner = lock();
        owner.1 -= 1;
        if owner.1 == 0 {
            owner.0 = None;
            self.free.notify_one();
        }
        result
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::sync::{Arc, Mutex};

    #[cfg(feature = "diagnostics")]
    #[test]
    fn ffx_scale_points_both_loads_at_the_constant() {
        // Synthetic code: the two loads, filler, and int3 padding.
        let mut code = vec![0x90u8; super::FFX_SCALE_SPAN];
        for at in [0x28, 0x32] {
            code[at..at + 8].copy_from_slice(&[0xC5, 0xFA, 0x10, 0x05, 1, 2, 3, 4]);
        }
        code[0x60..].fill(0xCC);
        assert!(super::patch_ffx_scale(&mut code, 0.5));
        assert_eq!(code[0x64..0x68], 0.5f32.to_le_bytes());
        // Each load's next instruction plus its displacement is the constant.
        for at in [0x28, 0x32] {
            let disp = i32::from_le_bytes(code[at + 4..at + 8].try_into().unwrap());
            assert_eq!(at as i32 + 8 + disp, 0x64);
        }
        // Already patched, so no padding left: nothing is written again.
        let before = code.clone();
        assert!(!super::patch_ffx_scale(&mut code, 0.25));
        assert_eq!(code, before);
    }

    #[test]
    fn loader_lock_is_reentrant_and_exclusive() {
        let lock = Arc::new(super::LoaderLock::default());
        assert_eq!(lock.hold(|| lock.hold(|| 7)), 7);
        let order = Arc::new(Mutex::new(Vec::new()));
        let other = lock.hold(|| {
            let (theirs, their_order) = (lock.clone(), order.clone());
            let other = std::thread::spawn(move || {
                theirs.hold(|| their_order.lock().unwrap().push(2));
            });
            std::thread::sleep(std::time::Duration::from_millis(50));
            order.lock().unwrap().push(1);
            other
        });
        other.join().unwrap();
        assert_eq!(*order.lock().unwrap(), [1, 2]);
    }
}

#[cfg(not(windows))]
pub fn run(_: &Path, _: Unimplemented, _: bool, _: bool, _: Console) -> Result<()> {
    anyhow::bail!("running titles requires Windows")
}
