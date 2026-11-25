//! Do-nothing pipelines for Xbox shaders that do not translate yet.
//!
//! A title treats pipeline creation failure as fatal or takes error paths that
//! never run on the console (ProjectScarlett later dereferences a freed object).
//! Returning a valid pipeline whose draws and dispatches have no effect keeps
//! the title on its normal path; the failure itself is still logged with the
//! shader hashes for preparation. Disable with `TUXBOX_NULL_PIPELINES=0`.
//!
//! The shaders bind no resources, so they are compatible with any root
//! signature. The vertex shader emits a clipped position (w = 0); the pixel
//! shader writes nothing.
//!
//! Per-shader controls (VIctory pipeline.rs:240, rebox console_compile.c:835):
//! - `TUXBOX_FALLBACK=flat`: a rejected raster pipeline first keeps its own
//!   vertex stages and gets a flat magenta pixel shader, so a refused pixel
//!   shader shows where it draws; only if that fails is the whole pipeline nulled.
//! - `TUXBOX_NULL_SHADERS=<digest prefix>,...`: pipelines with a stage whose
//!   source digest starts with one of these are nulled even when they translate.
//! - `TUXBOX_NULL_BISECT=<bits>:<hex value>`: null every raster pipeline with a
//!   stage whose digest's low `bits` bits equal `value`; halving the range run
//!   by run finds the shader behind a hang or corruption.
use std::sync::{LazyLock, Mutex, OnceLock};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW};
use windows::core::{Interface, s, w};

#[derive(Clone, Copy)]
pub(super) enum Stage {
    Vertex,
    Pixel,
    Compute,
}

pub(super) fn enabled() -> bool {
    static ENABLED: LazyLock<bool> =
        LazyLock::new(|| std::env::var_os("TUXBOX_NULL_PIPELINES").is_none_or(|v| v != "0"));
    *ENABLED
}

/// DXBC for a resource-free shader of this stage, compiled once.
pub(super) fn shader(stage: Stage) -> Option<&'static [u8]> {
    static CODE: [OnceLock<Option<Vec<u8>>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    let (index, source, profile) = match stage {
        Stage::Vertex => (0, "float4 main() : SV_Position { return 0; }", s!("vs_5_0")),
        Stage::Pixel => (1, "void main() { }", s!("ps_5_0")),
        Stage::Compute => (2, "[numthreads(1, 1, 1)] void main() { }", s!("cs_5_0")),
    };
    CODE[index].get_or_init(|| compile(source, profile)).as_deref()
}

/// A pixel shader that writes flat magenta to render target 0 (`TUXBOX_FALLBACK=flat`).
pub(super) fn flat_pixel() -> Option<&'static [u8]> {
    static CODE: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    CODE.get_or_init(|| compile("float4 main() : SV_Target { return float4(1, 0, 1, 1); }", s!("ps_5_0"))).as_deref()
}

pub(super) fn flat_enabled() -> bool {
    static FLAT: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_FALLBACK").is_ok_and(|v| v.trim().eq_ignore_ascii_case("flat")));
    *FLAT
}

struct Rules {
    prefixes: Vec<String>,
    bisect: Option<(u32, u64)>,
}

static RULES: LazyLock<Option<Rules>> = LazyLock::new(|| {
    let prefixes: Vec<String> = std::env::var("TUXBOX_NULL_SHADERS")
        .unwrap_or_default()
        .split(',')
        .map(|h| h.trim().to_ascii_lowercase())
        .filter(|h| !h.is_empty())
        .collect();
    let bisect = std::env::var("TUXBOX_NULL_BISECT").ok().and_then(|v| {
        let (bits, value) = v.trim().split_once(':')?;
        let bits: u32 = bits.trim().parse().ok().filter(|b| (1..=60).contains(b))?;
        let value = u64::from_str_radix(value.trim().trim_start_matches("0x"), 16).ok()?;
        Some((bits, value))
    });
    (!prefixes.is_empty() || bisect.is_some()).then_some(Rules { prefixes, bisect })
});

/// Whether TUXBOX_NULL_SHADERS or TUXBOX_NULL_BISECT is set.
pub(super) fn forcing() -> bool {
    RULES.is_some()
}

/// The first of these source digests that TUXBOX_NULL_SHADERS or
/// TUXBOX_NULL_BISECT selects.
pub(super) fn forced_null<'a>(digests: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let rules = RULES.as_ref()?;
    digests.into_iter().find(|d| {
        rules.prefixes.iter().any(|p| d.starts_with(p.as_str()))
            || rules.bisect.is_some_and(|(bits, value)| {
                let mask = (1u64 << bits) - 1;
                u64::from_str_radix(&d[d.len().saturating_sub(16)..], 16).is_ok_and(|h| h & mask == value & mask)
            })
    })
}

fn compile(source: &str, profile: windows::core::PCSTR) -> Option<Vec<u8>> {
    type Compile = unsafe extern "system" fn(
        *const core::ffi::c_void, usize, *const i8, *const core::ffi::c_void, *const core::ffi::c_void,
        *const u8, *const u8, u32, u32, *mut *mut core::ffi::c_void, *mut *mut core::ffi::c_void,
    ) -> windows::core::HRESULT;
    // Serialize: compilation happens at most three times per process.
    static LOCK: Mutex<()> = Mutex::new(());
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let library = LoadLibraryExW(w!("d3dcompiler_47.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32).ok()?;
        let compile: Compile = core::mem::transmute(GetProcAddress(library, s!("D3DCompile"))?);
        let mut raw = core::ptr::null_mut();
        compile(source.as_ptr().cast(), source.len(), core::ptr::null(), core::ptr::null(), core::ptr::null(),
            s!("main").as_ptr(), profile.as_ptr(), 0, 0, &mut raw, core::ptr::null_mut()).ok().ok()?;
        let blob = windows::Win32::Graphics::Direct3D::ID3DBlob::from_raw(raw);
        Some(core::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()).to_vec())
    }
}

pub(super) fn bytecode(code: &[u8]) -> windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE {
    windows::Win32::Graphics::Direct3D12::D3D12_SHADER_BYTECODE {
        pShaderBytecode: code.as_ptr().cast(),
        BytecodeLength: code.len(),
    }
}

/// Count and occasionally report substituted pipelines. Counts are per kind
/// so each "so far" label describes the pipelines it names.
pub(super) fn substituted(kind: &'static str) {
    crate::diagnostics::frame_work::add(crate::diagnostics::frame_work::Work::NullPipelines);
    static COUNTS: Mutex<Vec<(&str, u64)>> = Mutex::new(Vec::new());
    let count = bump(&mut COUNTS.lock().unwrap_or_else(|e| e.into_inner()), kind);
    if count.is_power_of_two() {
        crate::diagnostics::log_once(&std::sync::Once::new(), || {
            format!("substituted null {kind} pipeline for an untranslated shader ({count} so far)")
        });
    }
}

fn bump(counts: &mut Vec<(&'static str, u64)>, kind: &'static str) -> u64 {
    match counts.iter_mut().find(|(seen, _)| *seen == kind) {
        Some((_, count)) => {
            *count += 1;
            *count
        }
        None => {
            counts.push((kind, 1));
            1
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn counts_each_kind_separately() {
        let mut counts = Vec::new();
        assert_eq!(super::bump(&mut counts, "compute"), 1);
        assert_eq!(super::bump(&mut counts, "compute"), 2);
        assert_eq!(super::bump(&mut counts, "raster stream"), 1);
        assert_eq!(super::bump(&mut counts, "compute"), 3);
    }
}
