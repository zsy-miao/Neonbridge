//! TUXBOX_RAW_STRUCTURED (default on with xshader c1b81c0 TB_IDXEN_RAW; =0 off):
//! a structured buffer view (format UNKNOWN, stride > 0) is created as a raw
//! view (R32_TYPELESS, FLAG_RAW) over the same bytes. Translated shaders read
//! heap buffers as ByteAddressBuffer (xshader hlsl.rs heap loads, the idxen
//! form included once it scales by the slot's stride), and a raw read through
//! a structured view reads zeros on NVIDIA: the title's meshlet culling read
//! its scene buffers that way and drew almost nothing. Ported from
//! VIctory-fxceless 578d459 descriptor/host.rs raw_window, buffer_srv and
//! buffer_uav (GPL-3.0, credited). A window not on 16 bytes or not whole
//! dwords stays structured, as there. slot_views keeps the title's stride.
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_TYPELESS, DXGI_FORMAT_UNKNOWN};

pub(crate) fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_RAW_STRUCTURED") != Some("0"))
}

/// (first dword, dwords) of a structured window, when it can be raw.
fn raw_window(first: u64, count: u32, stride: u32) -> Option<(u64, u32)> {
    if stride == 0 {
        return None;
    }
    let start = first.checked_mul(u64::from(stride))?;
    let bytes = u64::from(count) * u64::from(stride);
    (start % 16 == 0 && bytes % 4 == 0 && bytes != 0).then(|| u32::try_from(bytes / 4).ok().map(|dwords| (start / 4, dwords))).flatten()
}

fn note(kind: &str, stride: u32) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "raw structured: {kind} of stride {stride} created raw ({n} so far; TUXBOX_RAW_STRUCTURED=0 off)"));
    }
}

/// Rewrites a structured buffer SRV as raw in place.
pub(crate) fn srv(view: &mut D3D12_SHADER_RESOURCE_VIEW_DESC) {
    if !enabled() || view.ViewDimension != D3D12_SRV_DIMENSION_BUFFER || view.Format != DXGI_FORMAT_UNKNOWN {
        return;
    }
    let b = unsafe { view.Anonymous.Buffer };
    let Some((first, dwords)) = raw_window(b.FirstElement, b.NumElements, b.StructureByteStride) else { return };
    view.Format = DXGI_FORMAT_R32_TYPELESS;
    view.Anonymous.Buffer = D3D12_BUFFER_SRV { FirstElement: first, NumElements: dwords, StructureByteStride: 0, Flags: D3D12_BUFFER_SRV_FLAG_RAW };
    note("SRV", b.StructureByteStride);
}

/// Rewrites a structured buffer UAV with no counter as raw in place.
pub(crate) fn uav(view: &mut D3D12_UNORDERED_ACCESS_VIEW_DESC, counter: bool) {
    if !enabled() || counter || view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER || view.Format != DXGI_FORMAT_UNKNOWN {
        return;
    }
    let b = unsafe { view.Anonymous.Buffer };
    let Some((first, dwords)) = raw_window(b.FirstElement, b.NumElements, b.StructureByteStride) else { return };
    view.Format = DXGI_FORMAT_R32_TYPELESS;
    view.Anonymous.Buffer = D3D12_BUFFER_UAV { FirstElement: first, NumElements: dwords, StructureByteStride: 0, CounterOffsetInBytes: 0, Flags: D3D12_BUFFER_UAV_FLAG_RAW };
    note("UAV", b.StructureByteStride);
}
