//! Null descriptors of the kind a shader expects (VIctory 8c26061, "the
//! world stays up"). A table slot the title never wrote used to get a null
//! 2D texture view whatever the shader declared there; a shader reading a
//! buffer, constant buffer or another texture dimension through it is
//! undefined on the host and removed VIctory's device seconds into the
//! world. The kind comes from the translated DXIL's pipeline state
//! validation part (PSV0 resource bindings) matched against the root
//! signature the shader was translated for; slots no binding reaches get
//! the default for their range type. Never-written slots of whole heaps are
//! filled with null views at creation, as their host contents are undefined.
use std::sync::Arc;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum NullKind {
    SrvTexture(D3D12_SRV_DIMENSION),
    SrvTyped,
    SrvRaw,
    SrvStructured,
    SrvAccel,
    UavTexture(D3D12_UAV_DIMENSION),
    UavTyped,
    UavRaw,
    UavStructured,
    Cbv,
}

impl NullKind {
    pub(crate) const DEFAULT: NullKind = NullKind::SrvTexture(D3D12_SRV_DIMENSION_TEXTURE2D);

    /// A nonzero tag, distinct per kind (a table cache key).
    pub(crate) fn code(self) -> u64 {
        match self {
            NullKind::SrvTexture(d) => 0x100 | d.0 as u64,
            NullKind::SrvTyped => 2,
            NullKind::SrvRaw => 3,
            NullKind::SrvStructured => 4,
            NullKind::SrvAccel => 5,
            NullKind::UavTexture(d) => 0x200 | d.0 as u64,
            NullKind::UavTyped => 7,
            NullKind::UavRaw => 8,
            NullKind::UavStructured => 9,
            NullKind::Cbv => 10,
        }
    }

    /// Writes a null descriptor of this kind at `at`.
    pub(crate) fn write(self, device: &ID3D12Device, at: D3D12_CPU_DESCRIPTOR_HANDLE) {
        let srv = |format, dimension, buffer: Option<D3D12_BUFFER_SRV>| {
            let mut desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: dimension,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                ..Default::default()
            };
            if let Some(buffer) = buffer {
                desc.Anonymous.Buffer = buffer;
            }
            unsafe { device.CreateShaderResourceView(None, Some(&desc), at) };
        };
        let uav = |format, dimension, buffer: Option<D3D12_BUFFER_UAV>| {
            let mut desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
                Format: format,
                ViewDimension: dimension,
                ..Default::default()
            };
            if let Some(buffer) = buffer {
                desc.Anonymous.Buffer = buffer;
            }
            unsafe { device.CreateUnorderedAccessView(None, None, Some(&desc), at) };
        };
        let one = |stride, flags| D3D12_BUFFER_SRV { NumElements: 1, StructureByteStride: stride, Flags: flags, ..Default::default() };
        let one_uav = |stride, flags| D3D12_BUFFER_UAV { NumElements: 1, StructureByteStride: stride, Flags: flags, ..Default::default() };
        match self {
            NullKind::SrvTexture(d) => srv(DXGI_FORMAT_R8G8B8A8_UNORM, d, None),
            NullKind::SrvTyped => srv(DXGI_FORMAT_R32_UINT, D3D12_SRV_DIMENSION_BUFFER, Some(one(0, D3D12_BUFFER_SRV_FLAG_NONE))),
            NullKind::SrvRaw => srv(DXGI_FORMAT_R32_TYPELESS, D3D12_SRV_DIMENSION_BUFFER, Some(one(0, D3D12_BUFFER_SRV_FLAG_RAW))),
            NullKind::SrvStructured => srv(DXGI_FORMAT_UNKNOWN, D3D12_SRV_DIMENSION_BUFFER, Some(one(4, D3D12_BUFFER_SRV_FLAG_NONE))),
            NullKind::SrvAccel => srv(DXGI_FORMAT_UNKNOWN, D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE, None),
            NullKind::UavTexture(d) => {
                let desc = uav_texture_desc(d);
                unsafe { device.CreateUnorderedAccessView(None, None, Some(&desc), at) };
            }
            NullKind::UavTyped => uav(DXGI_FORMAT_R32_UINT, D3D12_UAV_DIMENSION_BUFFER, Some(one_uav(0, D3D12_BUFFER_UAV_FLAG_NONE))),
            NullKind::UavRaw => uav(DXGI_FORMAT_R32_TYPELESS, D3D12_UAV_DIMENSION_BUFFER, Some(one_uav(0, D3D12_BUFFER_UAV_FLAG_RAW))),
            NullKind::UavStructured => uav(DXGI_FORMAT_UNKNOWN, D3D12_UAV_DIMENSION_BUFFER, Some(one_uav(4, D3D12_BUFFER_UAV_FLAG_NONE))),
            NullKind::Cbv => unsafe {
                device.CreateConstantBufferView(Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC::default()), at)
            },
        }
    }
}

/// TUXBOX_NULL_UAV_EXTENT=0: null texture UAVs leave array size and 3D
/// depth at zero, as before.
fn full_extent() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("TUXBOX_NULL_UAV_EXTENT").is_ok_and(|v| v.trim() == "0"))
}

/// A null texture UAV of the shader's dimension (1D, 1D/2D arrays of one
/// slice, 3D with WSize -1 = every depth slice); MSAA, cube and unknown
/// kinds come here already mapped to 2D or a 2D array (VIctory cbb50b8).
fn uav_texture_desc(dimension: D3D12_UAV_DIMENSION) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    let mut desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: DXGI_FORMAT_R8G8B8A8_UNORM,
        ViewDimension: dimension,
        ..Default::default()
    };
    if full_extent() {
        match dimension {
            D3D12_UAV_DIMENSION_TEXTURE1DARRAY => desc.Anonymous.Texture1DArray.ArraySize = 1,
            D3D12_UAV_DIMENSION_TEXTURE2DARRAY => desc.Anonymous.Texture2DArray.ArraySize = 1,
            D3D12_UAV_DIMENSION_TEXTURE3D => desc.Anonymous.Texture3D.WSize = u32::MAX,
            _ => {}
        }
    }
    desc
}

/// Per root parameter, the null kind of each table slot (later entries win).
#[derive(Debug, Default)]
pub(crate) struct NullKinds(Vec<Vec<(u32, u32, NullKind)>>);

impl NullKinds {
    pub(crate) fn get(&self, parameter: usize, slot: u32) -> Option<NullKind> {
        self.0
            .get(parameter)?
            .iter()
            .rev()
            .find(|(start, end, _)| (*start..*end).contains(&slot))
            .map(|&(_, _, kind)| kind)
    }
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// PSV0 resource bindings: (resource type, space, lower, upper, kind).
pub(super) fn bindings(shader: &[u8]) -> Vec<[u32; 5]> {
    let read = || -> Option<Vec<[u32; 5]>> {
        let dxbc = tuxbox_dxbc::Dxbc::parse(shader).ok()?;
        let chunk = dxbc.chunk(b"PSV0")?;
        let body = shader.get(chunk.offset..chunk.offset + chunk.size)?;
        let runtime = word(body, 0)? as usize;
        let mut at = 4 + runtime;
        let count = word(body, at)? as usize;
        at += 4;
        if count == 0 {
            return Some(Vec::new());
        }
        let size = word(body, at)? as usize;
        at += 4;
        if size < 16 || count > 4096 {
            return None;
        }
        (0..count)
            .map(|i| {
                let entry = at + i * size;
                let kind = if size >= 24 { word(body, entry + 16)? } else { 0 };
                Some([word(body, entry)?, word(body, entry + 4)?, word(body, entry + 8)?, word(body, entry + 12)?, kind])
            })
            .collect()
    };
    read().unwrap_or_default()
}

/// (descriptor range type, kind) for a PSV0 binding.
fn classify(resource: u32, kind: u32) -> Option<(u32, NullKind)> {
    let srv_texture = |kind| {
        Some(NullKind::SrvTexture(match kind {
            1 => D3D12_SRV_DIMENSION_TEXTURE1D,
            3 => D3D12_SRV_DIMENSION_TEXTURE2DMS,
            4 => D3D12_SRV_DIMENSION_TEXTURE3D,
            5 => D3D12_SRV_DIMENSION_TEXTURECUBE,
            6 => D3D12_SRV_DIMENSION_TEXTURE1DARRAY,
            7 => D3D12_SRV_DIMENSION_TEXTURE2DARRAY,
            8 => D3D12_SRV_DIMENSION_TEXTURE2DMSARRAY,
            9 => D3D12_SRV_DIMENSION_TEXTURECUBEARRAY,
            _ => D3D12_SRV_DIMENSION_TEXTURE2D,
        }))
    };
    let uav_texture = |kind| {
        Some(NullKind::UavTexture(match kind {
            1 => D3D12_UAV_DIMENSION_TEXTURE1D,
            4 => D3D12_UAV_DIMENSION_TEXTURE3D,
            6 => D3D12_UAV_DIMENSION_TEXTURE1DARRAY,
            7 | 5 | 9 => D3D12_UAV_DIMENSION_TEXTURE2DARRAY,
            _ => D3D12_UAV_DIMENSION_TEXTURE2D,
        }))
    };
    // DXIL resource kinds: 10 typed buffer, 11 raw, 12 structured, 16 ray scene.
    let null = match resource {
        2 => Some(NullKind::Cbv),
        3 | 4 | 5 if kind == 16 => Some(NullKind::SrvAccel),
        3 if kind == 10 => Some(NullKind::SrvTyped),
        3 => srv_texture(kind),
        4 => Some(NullKind::SrvRaw),
        5 => Some(NullKind::SrvStructured),
        6 if kind == 10 => Some(NullKind::UavTyped),
        6 => uav_texture(kind),
        7 => Some(NullKind::UavRaw),
        8 | 9 => Some(NullKind::UavStructured),
        _ => None,
    }?;
    let range = match resource {
        2 => 2,
        3..=5 => 0,
        _ => 1,
    };
    Some((range, null))
}

/// The null kinds of a pipeline's table slots, for the root signature (RTS0
/// payload) its translated `stages` were made for.
pub(crate) fn reflect<'a>(root: &[u8], stages: impl IntoIterator<Item = &'a [u8]>) -> Option<Arc<NullKinds>> {
    use tuxbox_dxbc::rootsig::RootParamKind;
    let root = tuxbox_dxbc::rootsig::parse_rts0(root).ok()?;
    let resources: Vec<_> = stages
        .into_iter()
        .flat_map(bindings)
        .filter_map(|[resource, space, lower, upper, kind]| {
            classify(resource, kind).map(|(range, null)| (range, space, lower, upper, null))
        })
        .collect();
    let mut any = false;
    let kinds = root
        .params
        .iter()
        .map(|param| {
            let RootParamKind::Table(ranges) = &param.kind else { return Vec::new() };
            let mut out = Vec::new();
            let mut overrides = Vec::new();
            let mut next = 0u32;
            for range in ranges {
                let start = if range.offset_in_table == u32::MAX { next } else { range.offset_in_table };
                let count = if range.num == u32::MAX { 1 << 20 } else { range.num };
                let Some(end) = start.checked_add(count) else { break };
                next = end;
                let default = match range.range_type {
                    0 => NullKind::DEFAULT,
                    1 => NullKind::UavTexture(D3D12_UAV_DIMENSION_TEXTURE2D),
                    2 => NullKind::Cbv,
                    _ => continue,
                };
                out.push((start, end, default));
                let last = range.base_register.saturating_add(count - 1);
                for &(kind, space, lower, upper, null) in &resources {
                    if kind != range.range_type || space != range.space {
                        continue;
                    }
                    let lo = lower.max(range.base_register);
                    let hi = upper.min(last);
                    if lo <= hi {
                        overrides.push((start + (lo - range.base_register), start + (hi - range.base_register) + 1, null));
                    }
                }
            }
            any |= !out.is_empty();
            out.extend(overrides);
            out
        })
        .collect();
    any.then(|| Arc::new(NullKinds(kinds)))
}

/// Fills `count` slots of a CBV/SRV/UAV heap with null views. A shader-
/// visible heap is filled from a CPU heap, since copies read CPU heaps only.
pub(crate) fn fill(device: &ID3D12Device, heap: &ID3D12DescriptorHeap, count: u32, visible: bool) {
    const CHUNK: u32 = 1 << 16;
    let kind = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
    let stride = unsafe { device.GetDescriptorHandleIncrementSize(kind) } as usize;
    let double = |start: D3D12_CPU_DESCRIPTOR_HANDLE, length: u32| {
        NullKind::DEFAULT.write(device, start);
        let mut done = 1;
        while done < length {
            let step = done.min(length - done);
            let to = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + done as usize * stride };
            unsafe { device.CopyDescriptorsSimple(step, to, start, kind) };
            done += step;
        }
    };
    let start = unsafe { heap.GetCPUDescriptorHandleForHeapStart() };
    if !visible {
        double(start, count);
        return;
    }
    let source = D3D12_DESCRIPTOR_HEAP_DESC { Type: kind, NumDescriptors: count.min(CHUNK), ..Default::default() };
    let Ok(source) = (unsafe { device.CreateDescriptorHeap::<ID3D12DescriptorHeap>(&source) }) else { return };
    let from = unsafe { source.GetCPUDescriptorHandleForHeapStart() };
    double(from, count.min(CHUNK));
    let mut done = 0;
    while done < count {
        let step = (count - done).min(CHUNK);
        let to = D3D12_CPU_DESCRIPTOR_HANDLE { ptr: start.ptr + done as usize * stride };
        unsafe { device.CopyDescriptorsSimple(step, to, from, kind) };
        done += step;
    }
}

#[cfg(test)]
mod uav_dimensions {
    use super::*;

    /// Every DXIL texture kind reaches the null UAV with the shader's
    /// dimension, and the arrays and 3D views cover a whole slice / depth.
    #[test]
    fn null_texture_uavs_keep_the_shader_dimension() {
        let expect = [
            (1, D3D12_UAV_DIMENSION_TEXTURE1D),
            (2, D3D12_UAV_DIMENSION_TEXTURE2D),
            (3, D3D12_UAV_DIMENSION_TEXTURE2D), // MSAA
            (4, D3D12_UAV_DIMENSION_TEXTURE3D),
            (5, D3D12_UAV_DIMENSION_TEXTURE2DARRAY), // cube
            (6, D3D12_UAV_DIMENSION_TEXTURE1DARRAY),
            (7, D3D12_UAV_DIMENSION_TEXTURE2DARRAY),
            (8, D3D12_UAV_DIMENSION_TEXTURE2D), // MSAA array
            (9, D3D12_UAV_DIMENSION_TEXTURE2DARRAY),
            (0, D3D12_UAV_DIMENSION_TEXTURE2D),
        ];
        for (kind, dimension) in expect {
            assert_eq!(classify(6, kind).map(|c| c.1), Some(NullKind::UavTexture(dimension)), "kind {kind}");
        }
        let desc = uav_texture_desc(D3D12_UAV_DIMENSION_TEXTURE3D);
        assert_eq!(unsafe { desc.Anonymous.Texture3D.WSize }, u32::MAX);
        assert_eq!(unsafe { uav_texture_desc(D3D12_UAV_DIMENSION_TEXTURE2DARRAY).Anonymous.Texture2DArray.ArraySize }, 1);
        assert_eq!(unsafe { uav_texture_desc(D3D12_UAV_DIMENSION_TEXTURE1DARRAY).Anonymous.Texture1DArray.ArraySize }, 1);
    }
}
