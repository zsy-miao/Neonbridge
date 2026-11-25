//! TUXBOX_BUFFER_VIEW_OK (default on, =0 passes every buffer view on): a buffer
//! SRV or UAV over a committed resource the host would refuse is written as a
//! null view instead. Copied from VIctory-fxceless victory-gpu
//! descriptor/host.rs buffer_view_ok and resource.rs element/family (9376fd6,
//! GPL-3.0, credit to its authors): the host removes the device for such a
//! view, and the console is laxer (views past their memory's end, of no
//! elements, typeless without the raw flag, strides the host has no
//! structured buffers for).

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

pub(crate) fn enabled() -> bool {
    crate::env_toggle!("TUXBOX_BUFFER_VIEW_OK") != Some("0")
}

/// Bytes per element and the element's width in texels (4 for block
/// compression), by DXGI format number. `None` for 1-bit and video
/// formats. The console adds 116/117 (10-bit float color), 118-120 (16-bit
/// depth with 8-bit stencil, two planes: 3 bytes per texel), 189 (signed
/// 10-bit color), 190 (two 4-bit channels) and 191 (24/8 color).
fn element(format: u32) -> Option<(u64, u64)> {
    Some(match format {
        1..=4 => (16, 1),
        5..=8 => (12, 1),
        9..=22 => (8, 1),
        23..=47 | 67 | 87..=93 | 116 | 117 | 189 | 191 => (4, 1),
        118..=120 => (3, 1),
        48..=59 | 68 | 69 | 85 | 86 | 115 => (2, 1),
        60..=65 | 190 => (1, 1),
        70..=72 | 79..=81 => (8, 4),
        73..=78 | 82..=84 | 94..=99 => (16, 4),
        _ => return None,
    })
}

/// The typeless format of a format's family (itself if it has none).
fn family(format: u32) -> u32 {
    const FAMILIES: [(u32, u32); 22] = [
        (1, 4), (5, 8), (9, 14), (15, 18), (19, 22), (23, 25), (27, 32), (33, 38), (39, 43), (44, 47), (48, 52),
        (53, 59), (60, 65), (70, 72), (73, 75), (76, 78), (79, 81), (82, 84), (90, 91), (92, 93), (94, 96), (97, 99),
    ];
    match format {
        87 => 90,
        88 => 92,
        f => FAMILIES.iter().find(|&&(first, last)| (first..=last).contains(&f)).map_or(f, |&(first, _)| first),
    }
}

/// Whether the host accepts a buffer view with these fields of a buffer of
/// `width` bytes.
fn buffer_view_ok(format: DXGI_FORMAT, first: u64, count: u32, stride: u32, raw: bool, width: u64) -> bool {
    let f = format.0 as u32;
    let element = if raw {
        // raw: R32 typeless, counted in dwords, starting on 16 bytes
        if format != DXGI_FORMAT_R32_TYPELESS || stride != 0 || !first.is_multiple_of(4) {
            return false;
        }
        4
    } else if stride > 0 {
        // structured: no format, a stride the host takes
        if format != DXGI_FORMAT_UNKNOWN || stride > 2048 || !stride.is_multiple_of(4) {
            return false;
        }
        u64::from(stride)
    } else {
        // typed: a format with a size that is not typeless
        let typeless = family(f) == f && family(f + 1) == f;
        match element(f) {
            Some((bytes, 1)) if f != 0 && !typeless && bytes > 0 => bytes,
            _ => return false,
        }
    };
    count > 0
        && first
            .checked_add(u64::from(count))
            .and_then(|end| end.checked_mul(element))
            .is_some_and(|end| end <= width)
}

fn width(resource: &ID3D12Resource) -> Option<u64> {
    let d = unsafe { resource.GetDesc() };
    (d.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER).then_some(d.Width)
}

fn refused(kind: &str, format: DXGI_FORMAT, first: u64, count: u32, stride: u32, raw: bool, width: u64) {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "{kind} buffer view the host would refuse written null: format {} first {first} count {count} stride {stride} raw {raw} width {width} ({n} so far; TUXBOX_BUFFER_VIEW_OK=0 passes it on)",
            format.0));
    }
}

/// Whether a buffer SRV of `resource` must be written null.
pub(crate) fn srv_refused(resource: Option<&ID3D12Resource>, view: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> bool {
    if !enabled() || view.ViewDimension != D3D12_SRV_DIMENSION_BUFFER {
        return false;
    }
    let Some(width) = resource.and_then(width) else { return false };
    let b = unsafe { view.Anonymous.Buffer };
    let raw = b.Flags.0 & D3D12_BUFFER_SRV_FLAG_RAW.0 != 0;
    let bad = !buffer_view_ok(view.Format, b.FirstElement, b.NumElements, b.StructureByteStride, raw, width);
    if bad { refused("SRV", view.Format, b.FirstElement, b.NumElements, b.StructureByteStride, raw, width); }
    bad
}

/// Whether a buffer UAV of `resource` must be written null.
pub(crate) fn uav_refused(resource: Option<&ID3D12Resource>, view: &D3D12_UNORDERED_ACCESS_VIEW_DESC) -> bool {
    if !enabled() || view.ViewDimension != D3D12_UAV_DIMENSION_BUFFER {
        return false;
    }
    let Some(width) = resource.and_then(width) else { return false };
    let b = unsafe { view.Anonymous.Buffer };
    let raw = b.Flags.0 & D3D12_BUFFER_UAV_FLAG_RAW.0 != 0;
    let bad = !buffer_view_ok(view.Format, b.FirstElement, b.NumElements, b.StructureByteStride, raw, width);
    if bad { refused("UAV", view.Format, b.FirstElement, b.NumElements, b.StructureByteStride, raw, width); }
    bad
}

/// The null view written in place of a refused one: one raw dword, no resource.
pub(crate) fn null_srv(view: &D3D12_SHADER_RESOURCE_VIEW_DESC) -> D3D12_SHADER_RESOURCE_VIEW_DESC {
    let mut v = *view;
    v.Format = DXGI_FORMAT_R32_TYPELESS;
    v.Anonymous.Buffer = D3D12_BUFFER_SRV { FirstElement: 0, NumElements: 1, StructureByteStride: 0, Flags: D3D12_BUFFER_SRV_FLAG_RAW };
    v
}

pub(crate) fn null_uav(view: &D3D12_UNORDERED_ACCESS_VIEW_DESC) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut v = *view;
    v.Format = DXGI_FORMAT_R32_TYPELESS;
    v.Anonymous.Buffer = D3D12_BUFFER_UAV { FirstElement: 0, NumElements: 1, StructureByteStride: 0, CounterOffsetInBytes: 0, Flags: D3D12_BUFFER_UAV_FLAG_RAW };
    v
}
