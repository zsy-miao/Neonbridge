//! Pre-launch check (the idea of xcl's preflight.rs, written for Neonbridge): before a
//! run, ask the graphics card what every runtime needs (D3D12 at feature level 12_0,
//! Shader Model 6.6, mesh shaders, 64- or 32-lane waves) and look at RAM and free disk,
//! so "the game draws nothing" becomes a named problem with a fix. Read-only.
//!
//! `neonbridge --check` prints it all; runs print only problems (NEONBRIDGE_PREFLIGHT=0 off).

use std::path::Path;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Ok,
    Note,
    Warn,
    Bad,
}

pub struct Check {
    pub level: Level,
    pub what: String,
}

fn c(level: Level, what: impl Into<String>) -> Check {
    Check { level, what: what.into() }
}

struct Gpu {
    name: String,
    vram_gb: f64,
    sm: u32, // 0x66 = 6.6
    mesh: bool,
    wave_min: u32,
    wave_max: u32,
}

fn gpu() -> Result<Gpu, String> {
    use windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::*;
    unsafe {
        let factory: IDXGIFactory6 = CreateDXGIFactory1().map_err(|e| format!("DXGI: {e}"))?;
        let adapter: IDXGIAdapter1 = factory.EnumAdapterByGpuPreference(0, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE).map_err(|e| format!("no graphics card: {e}"))?;
        let desc = adapter.GetDesc1().map_err(|e| e.to_string())?;
        let name = String::from_utf16_lossy(&desc.Description).trim_end_matches('\0').to_owned();
        let vram_gb = desc.DedicatedVideoMemory as f64 / (1u64 << 30) as f64;
        let mut dev: Option<ID3D12Device> = None;
        D3D12CreateDevice(&adapter, D3D_FEATURE_LEVEL_12_0, &mut dev).map_err(|e| format!("{name}: no D3D12 feature level 12_0 ({e})"))?;
        let dev = dev.ok_or("no device")?;
        let mut sm = D3D12_FEATURE_DATA_SHADER_MODEL { HighestShaderModel: D3D_SHADER_MODEL_6_6 };
        let sm = if dev.CheckFeatureSupport(D3D12_FEATURE_SHADER_MODEL, &mut sm as *mut _ as *mut _, size_of_val(&sm) as u32).is_ok() { sm.HighestShaderModel.0 as u32 } else { 0 };
        let mut o7 = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        let mesh = dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &mut o7 as *mut _ as *mut _, size_of_val(&o7) as u32).is_ok() && o7.MeshShaderTier != D3D12_MESH_SHADER_TIER_NOT_SUPPORTED;
        let mut o1 = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
        let _ = dev.CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS1, &mut o1 as *mut _ as *mut _, size_of_val(&o1) as u32);
        Ok(Gpu { name, vram_gb, sm, mesh, wave_min: o1.WaveLaneCountMin, wave_max: o1.WaveLaneCountMax })
    }
}

fn ram_gb() -> f64 {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    unsafe { GlobalMemoryStatusEx(&mut m) };
    m.ullTotalPhys as f64 / (1u64 << 30) as f64
}

fn free_gb(p: &Path) -> Option<f64> {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    let ok = unsafe { windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(w.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut()) };
    (ok != 0).then(|| free as f64 / (1u64 << 30) as f64)
}

pub fn checks(cache: &Path) -> Vec<Check> {
    let mut v = Vec::new();
    match gpu() {
        Err(e) => v.push(c(Level::Bad, format!("graphics card: {e}. Update the graphics driver; a D3D12 card (NVIDIA GTX 16 / RTX 20, AMD RX 6000, Intel Arc or newer) is needed"))),
        Ok(g) => {
            v.push(c(Level::Ok, format!("graphics card: {}, {:.0} GB", g.name, g.vram_gb)));
            if g.sm < 0x66 {
                v.push(c(Level::Bad, format!("Shader Model {}.{} only: the runtimes need 6.6. Update the graphics driver", g.sm >> 4, g.sm & 0xf)));
            }
            if !g.mesh {
                v.push(c(Level::Warn, "no mesh shaders: some of the game's geometry is not drawn (an RTX 20 / RX 6000 / Arc or newer card has them)"));
            }
            let (w64, w32) = (g.wave_min <= 64 && g.wave_max >= 64, g.wave_min <= 32 && g.wave_max >= 32);
            if !w64 && w32 {
                v.push(c(Level::Note, format!("{}-{} lane waves: the console's 64-lane shaders run as two halves (normal on NVIDIA, a little slower)", g.wave_min, g.wave_max)));
            } else if !w64 && !w32 && g.wave_max > 0 {
                v.push(c(Level::Bad, format!("{}-{} lane waves: neither 64 nor 32, the console's shaders cannot run", g.wave_min, g.wave_max)));
            }
            if g.vram_gb > 0.0 && g.vram_gb < 7.5 {
                v.push(c(Level::Warn, format!("{:.0} GB of video memory: 8 GB or more is recommended", g.vram_gb)));
            }
        }
    }
    let ram = ram_gb();
    v.push(c(if ram < 15.0 { Level::Warn } else { Level::Ok }, format!("{ram:.0} GB of RAM{}", if ram < 15.0 { ": 16 GB or more is recommended" } else { "" })));
    if let Some(f) = free_gb(cache) {
        v.push(c(if f < 20.0 { Level::Warn } else { Level::Ok }, format!("{f:.0} GB free for the shader cache{}", if f < 20.0 { ": a first run can need 5-20 GB" } else { "" })));
    }
    v
}

/// Prints the checks (only notes and problems unless `all`); the worst level.
pub fn print(cache: &Path, all: bool) -> Level {
    let v = checks(cache);
    for k in v.iter().filter(|k| all || k.level != Level::Ok) {
        let tag = match k.level { Level::Ok => "ok  ", Level::Note => "note", Level::Warn => "WARN", Level::Bad => "BAD " };
        println!("[check] {tag} {}", k.what);
    }
    v.iter().map(|k| k.level).max().unwrap_or(Level::Ok)
}
