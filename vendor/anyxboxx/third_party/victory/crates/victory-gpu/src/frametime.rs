//! A frametime dump (`VICTORY_FRAMETIME_DUMP`): one CSV row per summary
//! interval, with the frame time and the buckets `host.rs` already measures
//! (preparing, copies, textures, GPU, copy-back, heap-buffer passes, texture
//! refreshes, the passes over the card's copies), plus the worst and average
//! frame over the interval, measured at every present.
//!
//! `VICTORY_FRAMETIME_DUMP=<path>` writes it there; `=1` writes
//! `<cache>\frametime.csv` (the app points this at Victory's cache folder;
//! `%TEMP%\victory-frametime.csv` if it did not). The file is truncated at
//! the first row.
//! This is deliberately release-buildable (no `diagnostics` feature): the
//! frames it is meant to explain are the slow ones.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: OnceLock<bool> = OnceLock::new();
static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
/// Where `=1` writes; the app points this at Victory's cache folder.
static DEFAULT_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static FILE: OnceLock<Option<Mutex<std::fs::File>>> = OnceLock::new();
static METADATA: OnceLock<String> = OnceLock::new();
static BUILD_LABEL: OnceLock<String> = OnceLock::new();

/// The title build the app identified (`<label> (<hash>)`), for the header.
pub fn use_build_label(label: String) {
    let _ = BUILD_LABEL.set(label);
}

/// Points `VICTORY_FRAMETIME_DUMP=1` at `path` (the app passes
/// `<cache>\frametime.csv`). Call before the first presented frame.
pub fn use_dump_path(path: PathBuf) {
    let _ = DEFAULT_PATH.set(Some(path));
}

/// When the previous present happened and how long the frames since the
/// last summary took (microseconds, and a count).
static LAST: Mutex<Option<Instant>> = Mutex::new(None);
static MAX_US: AtomicU64 = AtomicU64::new(0);
static SUM_US: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);

fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        let Some(value) = std::env::var_os("VICTORY_FRAMETIME_DUMP") else {
            return false;
        };
        if value.is_empty() {
            return false;
        }
        let path = if value.to_str() == Some("1") {
            DEFAULT_PATH
                .get()
                .and_then(Option::as_ref)
                .cloned()
                .unwrap_or_else(|| std::env::temp_dir().join("victory-frametime.csv"))
        } else {
            PathBuf::from(value)
        };
        let _ = PATH.set(Some(path));
        true
    })
}

fn file() -> Option<&'static Mutex<std::fs::File>> {
    FILE.get_or_init(|| {
        if !enabled() {
            return None;
        }
        let path = PATH.get().and_then(Option::as_ref)?;
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
        {
            Ok(mut file) => {
                let _ = file.write_all(metadata().as_bytes());
                let _ = file.write_all(HEADER.as_bytes());
                let _ = file.flush();
                tracing::info!(path = %path.display(), "frametime dump");
                Some(Mutex::new(file))
            }
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "no frametime dump");
                None
            }
        }
    })
    .as_ref()
}

const HEADER: &str = "t_ms,frame,frames,fps,ms_per_frame,frame_ms_max,frame_ms_avg,\
cpu_prepare_ms,gpu_ms,gpu_pct,copies_ms,copies_mb,textures_ms,textures_mb,\
copyback_ms,copyback_mb,draws,dropped,making,indirect,heap_passes,heap_ms,\
texture_refresh_ms,card_passes,card_compares,card_sent_mb,card_ranges,\
card_pieces,card_fetched,card_fetched_mb,card_waits,card_wait_ms,\
twins_into_depth,twins_into_colour,depth_planes,ram_mb,cpu_pct\n";

/// The `#` block a dump starts with: the machine and the switches, so a file
/// from another PC can be compared and reproduced. `pandas.read_csv(...,
/// comment='#')` skips it.
fn metadata() -> &'static str {
    METADATA.get_or_init(|| {
        let mut out = String::from("# victory frametime dump\n");
        out.push_str(&format!("# version: {}\n", env!("CARGO_PKG_VERSION")));
        if let Some(build) = BUILD_LABEL.get() {
            out.push_str(&format!("# game: {build}\n"));
        }
        #[cfg(windows)]
        {
            let cpu = registry_string(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString")
                .map(|name| name.trim().to_owned())
                .unwrap_or_else(|| {
                    std::env::var("PROCESSOR_IDENTIFIER").unwrap_or_else(|_| "unknown".into())
                });
            let logical = std::thread::available_parallelism().map_or(0, |n| n.get());
            out.push_str(&format!("# cpu: {cpu} ({logical} logical processors)\n"));
            if let Some((gpu, vram)) = crate::host::adapter_summary() {
                out.push_str(&format!("# gpu: {gpu} ({} MiB dedicated)\n", vram >> 20));
            }
            let gpu_name = crate::host::adapter_summary().map(|(name, _)| name);
            if let Some(driver) = display_driver_version(gpu_name.as_deref()) {
                out.push_str(&format!("# driver: {driver}\n"));
            }
            if let Some(total) = total_ram_mib() {
                out.push_str(&format!("# ram: {total} MiB\n"));
            }
            let product = registry_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName");
            let build = registry_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuild");
            let release = registry_string(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "DisplayVersion");
            // Windows 11 keeps "Windows 10 ..." in `ProductName`.
            let mut product = product.unwrap_or_default();
            if build.as_deref().and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22_000) {
                product = product.replace("Windows 10", "Windows 11");
            }
            if !product.is_empty() || build.is_some() {
                out.push_str(&format!(
                    "# os: {product}{}{}\n",
                    release.map(|r| format!(" {r}")).unwrap_or_default(),
                    build.map(|b| format!(" (build {b})")).unwrap_or_default()
                ));
            }
            let board = match (
                registry_string(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardManufacturer"),
                registry_string(r"HARDWARE\DESCRIPTION\System\BIOS", "BaseBoardProduct"),
            ) {
                (Some(m), Some(p)) => Some(format!("{m} {p}")),
                (Some(m), None) => Some(m),
                (None, Some(p)) => Some(p),
                _ => None,
            };
            if let Some(board) = board {
                out.push_str(&format!("# board: {board}\n"));
            }
            use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
            // SAFETY: a plain query of the primary display's size.
            let (w, h) = unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
            if w > 0 && h > 0 {
                out.push_str(&format!("# display: {w}x{h}\n"));
            }
        }
        // (Neptune) A dump is made to be posted: of a switch that names a file or a folder
        // (`VICTORY_FSR_DLL`, the dump's own path) only the last part is written, not where
        // it lies on this PC.
        let shown = |value: String| match value.rfind(['/', std::path::MAIN_SEPARATOR]) {
            Some(at) => format!("...{}", &value[at..]),
            None => value,
        };
        let mut switches: Vec<String> = std::env::vars()
            .filter(|(key, value)| key.starts_with("VICTORY_") && !value.is_empty())
            .map(|(key, value)| format!("{key}={}", shown(value)))
            .collect();
        switches.sort();
        if !switches.is_empty() {
            out.push_str(&format!("# switches: {}\n", switches.join(" ")));
        }
        out
    })
}

/// A `REG_SZ` value under `HKLM`, or `None`.
#[cfg(windows)]
fn registry_string(subkey: &str, value: &str) -> Option<String> {
    use windows_sys::Win32::System::Registry::{
        HKEY_LOCAL_MACHINE, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ, RegGetValueW,
    };
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain([0]).collect() };
    let (subkey, value) = (wide(subkey), wide(value));
    let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
    let mut bytes = 0u32;
    // SAFETY: a size query, then a read into a buffer of that size.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut bytes,
        )
    };
    if status != 0 || bytes == 0 {
        return None;
    }
    let mut buffer = vec![0u16; (bytes as usize).div_ceil(2)];
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            subkey.as_ptr(),
            value.as_ptr(),
            flags,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &mut bytes,
        )
    };
    if status != 0 {
        return None;
    }
    let len = (bytes as usize / 2).saturating_sub(1).min(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..len]))
}

/// Total physical RAM, in MiB.
#[cfg(windows)]
fn total_ram_mib() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: plain data the call fills in.
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: a sized structure.
    let ok = unsafe { GlobalMemoryStatusEx(&mut m) } != 0;
    ok.then_some(m.ullTotalPhys >> 20)
}

/// The display class key's driver version; for NVIDIA also the public number
/// (`32.0.16.2124` -> `621.24`), which is what people quote. The class has a
/// subkey per display adapter (`0000`, `0001`, ...), so match `DriverDesc`
/// against the DXGI adapter rather than assuming `0000`.
#[cfg(windows)]
fn display_driver_version(gpu: Option<&str>) -> Option<String> {
    const CLASS: &str =
        r"SYSTEM\CurrentControlSet\Control\Class\{4d36e968-e325-11ce-bfc1-08002be10318}";
    let gpu = gpu?.to_ascii_lowercase();
    let mut fallback = None;
    for index in 0..16 {
        let key = format!(r"{CLASS}\{index:04}");
        let Some(desc) = registry_string(&key, "DriverDesc") else {
            continue;
        };
        if desc.is_empty() {
            continue;
        }
        let desc = desc.to_ascii_lowercase();
        let nvidia = desc.contains("nvidia");
        if index == 0 {
            fallback = registry_string(&key, "DriverVersion");
        }
        let matches =
            desc == gpu || desc.contains(&gpu) || gpu.contains(&desc) || (nvidia && gpu.contains("nvidia"));
        if !matches {
            continue;
        }
        let version = registry_string(&key, "DriverVersion")?;
        if nvidia
            && let Some(public) = nvidia_driver_number(&version)
        {
            return Some(format!("{public} ({version})"));
        }
        return Some(version);
    }
    fallback
}

/// `32.0.16.2124` -> `621.24`: the last five digits of the third and fourth
/// parts.
#[cfg(windows)]
fn nvidia_driver_number(windows: &str) -> Option<String> {
    let mut parts = windows.split('.');
    let _major = parts.next()?;
    let _minor = parts.next()?;
    let third = parts.next()?;
    let fourth = parts.next()?;
    let digits: String = format!("{third}{fourth}")
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect();
    let tail = digits.get(digits.len().checked_sub(5)?..)?;
    Some(format!("{}.{}", &tail[..3], &tail[3..]))
}

/// Times one presented frame. Called for every present, before the frame
/// summary's own one-second gate, so a slow frame is seen even when the
/// summary is not printed.
pub(crate) fn tick() {
    if !enabled() {
        return;
    }
    let now = Instant::now();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(previous) = *last {
        let micros = now.duration_since(previous).as_micros() as u64;
        MAX_US.fetch_max(micros, Ordering::Relaxed);
        SUM_US.fetch_add(micros, Ordering::Relaxed);
        COUNT.fetch_add(1, Ordering::Relaxed);
    }
    *last = Some(now);
}

/// The worst and average frame in the interval, in milliseconds, and how
/// many frames were timed; resets them.
pub(crate) fn take_interval() -> (f64, f64, u64) {
    let max = MAX_US.swap(0, Ordering::Relaxed) as f64 / 1000.0;
    let sum = SUM_US.swap(0, Ordering::Relaxed) as f64 / 1000.0;
    let count = COUNT.swap(0, Ordering::Relaxed);
    let avg = if count > 0 {
        sum / count as f64
    } else {
        0.0
    };
    (max, avg, count)
}

/// One summary interval's figures: times per frame in ms, sizes per frame in
/// MB, counts per frame.
#[derive(Default)]
pub(crate) struct Row {
    pub frame: u64,
    pub frames: f64,
    pub fps: f64,
    pub ms: f64,
    pub frame_ms_max: f64,
    pub frame_ms_avg: f64,
    pub cpu_prepare_ms: f64,
    pub gpu_ms: f64,
    pub gpu_pct: f64,
    pub copies_ms: f64,
    pub copies_mb: f64,
    pub textures_ms: f64,
    pub textures_mb: f64,
    pub copyback_ms: f64,
    pub copyback_mb: f64,
    pub draws: f64,
    pub dropped: f64,
    pub making: f64,
    pub indirect: f64,
    pub heap_passes: f64,
    pub heap_ms: f64,
    pub texture_refresh_ms: f64,
    pub card_passes: f64,
    pub card_compares: f64,
    pub card_sent_mb: f64,
    pub card_ranges: f64,
    pub card_pieces: f64,
    pub card_fetched: f64,
    pub card_fetched_mb: f64,
    pub card_waits: f64,
    pub card_wait_ms: f64,
    pub twins_into_depth: f64,
    pub twins_into_colour: f64,
    pub depth_planes: f64,
    pub ram_mb: f64,
    pub cpu_pct: f64,
}

/// Appends `r` when the dump is on.
pub(crate) fn record(r: &Row) {
    let Some(file) = file() else {
        return;
    };
    let t_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let mut file = file.lock().unwrap_or_else(|e| e.into_inner());
    let _ = writeln!(
        file,
        "{t_ms},{frame},{frames:.3},{fps:.3},{ms:.3},{max:.3},{avg:.3},\
         {cpu:.3},{gpu:.3},{gpu_pct:.3},{copies_ms:.3},{copies_mb:.3},\
         {textures_ms:.3},{textures_mb:.3},{copyback_ms:.3},{copyback_mb:.3},\
         {draws:.3},{dropped:.3},{making:.3},{indirect:.3},{heap_passes:.3},{heap_ms:.3},\
         {texture_refresh_ms:.3},{card_passes:.3},{card_compares:.3},{card_sent_mb:.3},\
         {card_ranges:.3},{card_pieces:.3},{card_fetched:.3},{card_fetched_mb:.3},\
         {card_waits:.3},{card_wait_ms:.3},{twins_into_depth:.3},{twins_into_colour:.3},\
         {depth_planes:.3},{ram_mb:.3},{cpu_pct:.3}",
        frame = r.frame,
        frames = r.frames,
        fps = r.fps,
        ms = r.ms,
        max = r.frame_ms_max,
        avg = r.frame_ms_avg,
        cpu = r.cpu_prepare_ms,
        gpu = r.gpu_ms,
        gpu_pct = r.gpu_pct,
        copies_ms = r.copies_ms,
        copies_mb = r.copies_mb,
        textures_ms = r.textures_ms,
        textures_mb = r.textures_mb,
        copyback_ms = r.copyback_ms,
        copyback_mb = r.copyback_mb,
        draws = r.draws,
        dropped = r.dropped,
        making = r.making,
        indirect = r.indirect,
        heap_passes = r.heap_passes,
        heap_ms = r.heap_ms,
        texture_refresh_ms = r.texture_refresh_ms,
        card_passes = r.card_passes,
        card_compares = r.card_compares,
        card_sent_mb = r.card_sent_mb,
        card_ranges = r.card_ranges,
        card_pieces = r.card_pieces,
        card_fetched = r.card_fetched,
        card_fetched_mb = r.card_fetched_mb,
        card_waits = r.card_waits,
        card_wait_ms = r.card_wait_ms,
        twins_into_depth = r.twins_into_depth,
        twins_into_colour = r.twins_into_colour,
        depth_planes = r.depth_planes,
        ram_mb = r.ram_mb,
        cpu_pct = r.cpu_pct,
    );
    let _ = file.flush();
}
