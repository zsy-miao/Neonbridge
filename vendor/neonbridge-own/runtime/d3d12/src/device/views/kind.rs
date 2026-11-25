//! TB_ZTRX_SHADER (c), TUXBOX_ZTRX_VIEW_KIND (default on, =0 the title's view
//! stays): a ztrx-translated shader declares each heap buffer view by how it
//! reads it, whatever kind the title's view description says: raw
//! (ByteAddressBuffer; R32_TYPELESS with the raw flag) for dword loads,
//! stores, atomics and typed-format loads without an index; structured with
//! the used-resource entry's stride when indexed; typed with four 32-bit
//! float channels for buffer_load_format/buffer_store_format. The host view
//! at that index must be of that kind and start at the title view's base
//! (ztrx victory-shader emit/mod.rs "Buffer views in the heap", GPL-3.0,
//! credit to the ztrx and VIctory authors). [`remake`] writes such a view
//! over the same bytes at a slot, from the resource and description
//! srv_views kept for it.
#![allow(dead_code)] // called by the ztrx bind once the sidecar names kinds

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReadKind {
    Raw,
    Structured(u32),
    Typed,
}

impl ReadKind {
    /// The sidecar's word: `raw`, `structured:<stride>`, `typed`.
    pub(crate) fn parse(word: &str) -> Option<ReadKind> {
        match word {
            "raw" => Some(ReadKind::Raw),
            "typed" => Some(ReadKind::Typed),
            w => w.strip_prefix("structured:")?.parse().ok().filter(|&s: &u32| s > 0).map(ReadKind::Structured),
        }
    }

    /// (format, element bytes, structure stride, raw).
    fn layout(self) -> (DXGI_FORMAT, u64, u32, bool) {
        match self {
            ReadKind::Raw => (DXGI_FORMAT_R32_TYPELESS, 4, 0, true),
            ReadKind::Structured(s) => (DXGI_FORMAT_UNKNOWN, u64::from(s), s, false),
            ReadKind::Typed => (DXGI_FORMAT_R32G32B32A32_FLOAT, 16, 0, false),
        }
    }
}

/// The byte window [first, first + bytes) of a buffer view in `format` with
/// `stride` (structured) from element `first` over `count` elements.
fn window(format: DXGI_FORMAT, stride: u32, first: u64, count: u32) -> Option<(u64, u64)> {
    let element = u64::from(super::element_stride(format.0, stride)?);
    Some((first.checked_mul(element)?, u64::from(count).checked_mul(element)?))
}

/// Rewrites the buffer view at `handle` as `kind` over the same bytes. False
/// (the slot left as it was) when off, when the slot holds no buffer view we
/// kept, or when the window does not fit the kind (a raw view starts on 16
/// bytes; elements must be whole).
pub(crate) fn remake(device: &ID3D12Device, handle: usize, kind: ReadKind) -> bool {
    if !super::super::srv_views::ztrx_kinds() {
        return false;
    }
    let (format, element, structure, raw) = kind.layout();
    let at = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: handle };
    if let Some((resource, desc)) = super::super::srv_views::get_uav(handle) {
        if desc.ViewDimension != D3D12_UAV_DIMENSION_BUFFER {
            return false;
        }
        let b = unsafe { desc.Anonymous.Buffer };
        let Some((start, bytes)) = window(desc.Format, b.StructureByteStride, b.FirstElement, b.NumElements) else { return false };
        if !fits(start, bytes, element, raw) {
            return note(false);
        }
        let view = D3D12_UNORDERED_ACCESS_VIEW_DESC {
            Format: format,
            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 { Buffer: D3D12_BUFFER_UAV {
                FirstElement: start / element,
                NumElements: (bytes / element) as u32,
                StructureByteStride: structure,
                CounterOffsetInBytes: 0,
                Flags: if raw { D3D12_BUFFER_UAV_FLAG_RAW } else { D3D12_BUFFER_UAV_FLAG_NONE },
            } },
        };
        unsafe { device.CreateUnorderedAccessView(&resource, None, Some(&view), at) };
        return note(true);
    }
    let Some((resource, desc)) = super::super::srv_views::get(handle) else { return false };
    if desc.ViewDimension != D3D12_SRV_DIMENSION_BUFFER {
        return false;
    }
    let b = unsafe { desc.Anonymous.Buffer };
    let Some((start, bytes)) = window(desc.Format, b.StructureByteStride, b.FirstElement, b.NumElements) else { return false };
    if !fits(start, bytes, element, raw) {
        return note(false);
    }
    let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Buffer: D3D12_BUFFER_SRV {
            FirstElement: start / element,
            NumElements: (bytes / element) as u32,
            StructureByteStride: structure,
            Flags: if raw { D3D12_BUFFER_SRV_FLAG_RAW } else { D3D12_BUFFER_SRV_FLAG_NONE },
        } },
    };
    unsafe { device.CreateShaderResourceView(&resource, Some(&view), at) };
    note(true)
}

fn fits(start: u64, bytes: u64, element: u64, raw: bool) -> bool {
    start.is_multiple_of(element) && (!raw || start.is_multiple_of(16)) && bytes >= element
}

fn note(made: bool) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
    static COUNTS: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
    let n = COUNTS[usize::from(made)].fetch_add(1, Relaxed) + 1;
    if n <= 2 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "ztrx view kind: {n} buffer views {} (TUXBOX_ZTRX_VIEW_KIND=0 off)",
            if made { "remade to the kind the shader reads" } else { "left as the title made them: window not on the kind's alignment" }));
    }
    made
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidecar_words() {
        assert_eq!(ReadKind::parse("raw"), Some(ReadKind::Raw));
        assert_eq!(ReadKind::parse("typed"), Some(ReadKind::Typed));
        assert_eq!(ReadKind::parse("structured:12"), Some(ReadKind::Structured(12)));
        assert_eq!(ReadKind::parse("structured:0"), None);
    }
}
