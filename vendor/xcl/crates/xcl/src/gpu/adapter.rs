//! Which D3D12 adapter XCL uses: `d3d12_adapter` in `xcl.toml`, overridden by `XCL_ADAPTER`.
//!
//! - `auto` (or empty): the hardware adapters in high-performance order, the first one that
//!   creates a device;
//! - `warp`: the software renderer;
//! - a number `N`: the N-th hardware adapter in high-performance order (0 = the first); if `N`
//!   is out of range, falls back to matching `N` as a standalone numeric token in the adapter
//!   name (so e.g. `5070`, `3080`, or `7900` selects that GPU model);
//! - anything else: the hardware adapters whose name contains it (case-insensitive), e.g.
//!   `radeon` or `rtx 3080`.
//!
//! The same choice drives the title's device (`hle::d3d12x::real`) and the GPU test bench
//! (`gpu::recomp::runner`), so the GPU tests can be run on each adapter of a machine in turn.

use std::fmt;

use windows::Win32::Graphics::Dxgi::*;

/// The environment variable that overrides `d3d12_adapter`.
pub const ENV: &str = "XCL_ADAPTER";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selector {
    Auto,
    Warp,
    /// Position among the hardware adapters in high-performance order.
    Index(usize),
    /// Lower-case part of the adapter's name.
    Name(String),
}

impl Selector {
    pub fn parse(s: &str) -> Selector {
        let s = s.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("auto") {
            Selector::Auto
        } else if s.eq_ignore_ascii_case("warp") {
            Selector::Warp
        } else if (s == "0" || !s.starts_with('0')) && s.bytes().all(|b| b.is_ascii_digit()) {
            match s.parse::<usize>() {
                Ok(n) => Selector::Index(n),
                Err(_) => Selector::Name(s.to_lowercase()),
            }
        } else {
            Selector::Name(s.to_lowercase())
        }
    }

    /// `XCL_ADAPTER` if set (and not empty), otherwise `configured`.
    pub fn from_env_or(configured: &str) -> Selector {
        match std::env::var(ENV) {
            Ok(v) if !v.trim().is_empty() => Selector::parse(&v),
            _ => Selector::parse(configured),
        }
    }

    /// Whether the hardware adapter at `index` (high-performance order) named `name` matches
    /// directly (without the out-of-range numeric model fallback applied by [`candidates`]).
    pub fn allows(&self, index: usize, name: &str) -> bool {
        match self {
            Selector::Auto => true,
            Selector::Warp => false,
            Selector::Index(n) => *n == index,
            Selector::Name(part) => name.to_lowercase().contains(part.as_str()),
        }
    }
}

impl fmt::Display for Selector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Selector::Auto => f.write_str("auto"),
            Selector::Warp => f.write_str("warp"),
            Selector::Index(n) => write!(f, "{n}"),
            Selector::Name(s) => write!(f, "\"{s}\""),
        }
    }
}

/// True if `name` contains `digits` not immediately preceded or followed by another ASCII digit
/// (e.g. `"5070"` matches `"RTX 5070"` and `"RX 7900XTX"` matches `"7900"`, while `"2"` does not
/// match `"RTX 2080"`).
fn contains_numeric_token(name: &str, digits: &str) -> bool {
    if digits.is_empty() {
        return false;
    }
    let bytes = name.as_bytes();
    let mut start = 0;
    while let Some(rel) = name[start..].find(digits) {
        let pos = start + rel;
        let end = pos + digits.len();
        let before_ok = pos == 0 || !bytes[pos - 1].is_ascii_digit();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_digit();
        if before_ok && after_ok {
            return true;
        }
        start = end;
    }
    false
}

/// Indices into `names` allowed by `sel`. For `Selector::Index(n)` with `n >= names.len()`,
/// falls back to matching `n` as a standalone numeric token in each adapter's name.
pub(crate) fn select_indices<S: AsRef<str>>(names: &[S], sel: &Selector) -> Vec<usize> {
    let direct: Vec<usize> = names.iter().enumerate().filter(|(i, n)| sel.allows(*i, n.as_ref())).map(|(i, _)| i).collect();
    if !direct.is_empty() {
        return direct;
    }
    if let Selector::Index(n) = sel {
        if *n >= names.len() {
            let token = n.to_string();
            return names.iter().enumerate().filter(|(_, name)| contains_numeric_token(name.as_ref(), &token)).map(|(i, _)| i).collect();
        }
    }
    Vec::new()
}

/// The adapter's name from its description.
pub fn name(desc: &DXGI_ADAPTER_DESC1) -> String {
    let n = desc.Description.iter().position(|&c| c == 0).unwrap_or(desc.Description.len());
    String::from_utf16_lossy(&desc.Description[..n])
}

/// The hardware adapters (software ones skipped) in high-performance order.
pub fn hardware(factory: &IDXGIFactory6) -> Vec<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> {
    let mut out = Vec::new();
    let mut i = 0;
    unsafe {
        while let Ok(adapter) = factory.EnumAdapterByGpuPreference::<IDXGIAdapter1>(i, DXGI_GPU_PREFERENCE_HIGH_PERFORMANCE) {
            i += 1;
            let Ok(desc) = adapter.GetDesc1() else { continue };
            if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
                continue;
            }
            out.push((adapter, desc));
        }
    }
    out
}

/// The hardware adapters `sel` allows, in high-performance order (empty for `warp`).
pub fn candidates(factory: &IDXGIFactory6, sel: &Selector) -> Vec<(IDXGIAdapter1, DXGI_ADAPTER_DESC1)> {
    let hw = hardware(factory);
    let names: Vec<String> = hw.iter().map(|(_, d)| name(d)).collect();
    let mut keep = vec![false; hw.len()];
    for i in select_indices(&names, sel) {
        keep[i] = true;
    }
    hw.into_iter().enumerate().filter(|(i, _)| keep[*i]).map(|(_, a)| a).collect()
}

/// "`sel` matches no adapter (have: 0 = …, 1 = …)" – for logs and test skips.
pub fn no_match(factory: &IDXGIFactory6, sel: &Selector) -> String {
    let have: Vec<String> = hardware(factory).iter().enumerate().map(|(i, (_, d))| format!("{i} = {}", name(d))).collect();
    format!("{ENV} / d3d12_adapter {sel} matches no hardware adapter (have: {})", have.join(", "))
}

#[cfg(test)]
#[path = "adapter_tests.rs"]
mod tests;
