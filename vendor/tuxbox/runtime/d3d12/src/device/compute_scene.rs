//! Private host root bindings for translated Lumen inline ray queries.
use core::{fmt::Write, ops::Range};
use std::sync::{Arc, Once};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{Interface, Result};

// A translated compute program opts into the private root ABI by declaring a
// native TLAS at t0, space1. A hit that needs face normals also declares raw
// t1 and t2 in that space. Match the DXIL actually supplied to the driver.
pub(crate) struct InlineProgram {
    pub(crate) source_hash: String,
    pub(crate) triangle_geometry: bool,
    pub(crate) first_dispatch: Once,
}

fn bindings(shader: &[u8]) -> Option<impl Iterator<Item = &[u8]>> {
    let directory = tuxbox_dxbc::Dxbc::parse(shader).ok()?;
    let part = directory.chunk(b"PSV0")?;
    let psv = shader.get(part.offset..part.offset.checked_add(part.size)?)?;
    let after_runtime = psv.get(4usize.checked_add(word(psv, 0)? as usize)?..)?;
    let count = word(after_runtime, 0)? as usize;
    let stride = word(after_runtime, 4)? as usize;
    if stride < 24 {
        return None;
    }
    Some(
        after_runtime
            .get(8..)?
            .get(..count.checked_mul(stride)?)?
            .chunks_exact(stride),
    )
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

// PSVResourceBindInfo1 has six DWORDs: type, space, lower, upper, kind, flags.
fn inline_layout(guest: &[u8], host: &[u8]) -> Option<(u32, bool)> {
    let mut private = 0u8;
    for r in bindings(host)? {
        if word(r, 4)? != 1 {
            continue;
        }
        let slot = word(r, 8)?;
        if word(r, 0)? != 4
            || slot != word(r, 12)?
            || !matches!((slot, word(r, 16)?), (0, 16) | (1 | 2, 11))
        {
            return None;
        }
        let bit = 1 << slot;
        if private & bit != 0 {
            return None;
        }
        private |= bit;
    }
    if !matches!(private, 1 | 7) {
        return None;
    }
    let mut scene = None;
    for r in bindings(guest)? {
        if word(r, 4)? == 1 {
            return None;
        } // A space owned by the title stays title-owned.
        if word(r, 16)? == 16 {
            let slot = word(r, 8)?;
            if word(r, 0)? != 4
                || word(r, 4)? != 0
                || word(r, 12)? != slot
                || scene.replace(slot).is_some()
            {
                return None;
            }
        }
    }
    Some((scene?, private == 7))
}

// Keep the guest-provided serialization for cloning a selected PSO's layout.
// The same RTS0 can be supplied in a shader or in its own serialized container.
#[derive(Clone)]
pub(crate) struct RootSource {
    bytes: Arc<[u8]>,
    payload: Range<usize>,
    pub(crate) direct_resources: bool,
    pub(crate) paged_tables: Option<Arc<[Option<u32>]>>,
    pub(crate) placed_tables: Option<Arc<[(u32, Range<u32>)]>>,
}

impl RootSource {
    pub(super) fn new(bytes: &[u8]) -> Self {
        let payload = tuxbox_dxbc::Dxbc::parse(bytes)
            .ok()
            .and_then(|dxbc| {
                let part = dxbc.chunk(b"RTS0")?;
                Some(part.offset..part.offset + part.size)
            })
            .unwrap_or(0..bytes.len());
        use tuxbox_dxbc::rootsig::RootParamKind;
        let root = tuxbox_dxbc::rootsig::parse_rts0(&bytes[payload.clone()]).ok();
        let placed_tables = root.as_ref().and_then(|root| root.placed_texture_tables());
        let direct_resources = root.as_ref().is_some_and(|root| root.flags & D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED.0 as u32 != 0);
        let paged_tables = root
            .map(|root| root.params.iter().map(|param| {
                let RootParamKind::Table(ranges) = &param.kind else { return None };
                let mut end = 0;
                let mut extent = 0;
                for r in ranges {
                    if r.num == u32::MAX { return None; }
                    let start = if r.offset_in_table == u32::MAX { end } else { r.offset_in_table };
                    end = start.checked_add(r.num)?;
                    extent = extent.max(end);
                }
                Some(extent)
            }).collect::<Arc<[_]>>());
        Self {
            direct_resources,
            paged_tables,
            placed_tables: placed_tables.map(Arc::from),
            bytes: Arc::from(bytes),
            payload,
        }
    }

    pub(crate) fn matches(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.bytes, &other.bytes)
            || self.bytes[self.payload.clone()] == other.bytes[other.payload.clone()]
    }

    pub(super) fn payload(&self) -> &[u8] {
        &self.bytes[self.payload.clone()]
    }

    pub(super) fn serialized(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum SceneSource {
    Table { slot: u32, offset: u32 },
    RootSrv(u32),
}

#[derive(Clone)]
pub(crate) struct SceneRoot {
    pub(crate) program: Arc<InlineProgram>,
    pub(crate) native: ID3D12RootSignature,
    pub(crate) original: RootSource,
    pub(crate) source: SceneSource,
    pub(crate) appended: u32,
}

unsafe fn describe(params: &[D3D12_ROOT_PARAMETER1]) -> String {
    let mut text = String::new();
    for (i, p) in params.iter().enumerate() {
        let _ = write!(
            text,
            " p{i}(type={},vis={})",
            p.ParameterType.0, p.ShaderVisibility.0
        );
        unsafe {
            match p.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE => {
                    let t = p.Anonymous.DescriptorTable;
                    if t.NumDescriptorRanges != 0 {
                        for r in core::slice::from_raw_parts(
                            t.pDescriptorRanges,
                            t.NumDescriptorRanges as usize,
                        ) {
                            let _ = write!(
                                text,
                                "[type={} reg={} space={} n={} off={:#x} flags={:#x}]",
                                r.RangeType.0,
                                r.BaseShaderRegister,
                                r.RegisterSpace,
                                r.NumDescriptors,
                                r.OffsetInDescriptorsFromTableStart,
                                r.Flags.0
                            );
                        }
                    }
                }
                D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS => {
                    let c = p.Anonymous.Constants;
                    let _ = write!(
                        text,
                        "[b{} space={} dwords={}]",
                        c.ShaderRegister, c.RegisterSpace, c.Num32BitValues
                    );
                }
                _ => {
                    let d = p.Anonymous.Descriptor;
                    let _ = write!(
                        text,
                        "[reg={} space={} flags={:#x}]",
                        d.ShaderRegister, d.RegisterSpace, d.Flags.0
                    );
                }
            }
        }
    }
    text
}

unsafe fn scene_source(
    params: &[D3D12_ROOT_PARAMETER1],
    guest_register: u32,
    private_count: u32,
) -> Result<SceneSource> {
    let mut source = None;
    let mut cost = 2 * u64::from(private_count); // two DWORDs per root SRV
    for (slot, p) in params.iter().enumerate() {
        let slot = slot as u32;
        let visible = p.ShaderVisibility == D3D12_SHADER_VISIBILITY_ALL;
        unsafe {
            match p.ParameterType {
                D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE => {
                    cost += 1;
                    let table = p.Anonymous.DescriptorTable;
                    if !visible || table.NumDescriptorRanges == 0 {
                        continue;
                    }
                    let ranges = core::slice::from_raw_parts(
                        table.pDescriptorRanges,
                        table.NumDescriptorRanges as usize,
                    );
                    let mut next = 0u64;
                    for r in ranges {
                        let offset = if r.OffsetInDescriptorsFromTableStart
                            == D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND
                        {
                            next
                        } else {
                            u64::from(r.OffsetInDescriptorsFromTableStart)
                        };
                        next = offset.saturating_add(u64::from(r.NumDescriptors));
                        if r.RangeType != D3D12_DESCRIPTOR_RANGE_TYPE_SRV {
                            continue;
                        }
                        if r.RegisterSpace == 1 && r.BaseShaderRegister < private_count {
                            return Err(E_INVALIDARG.into());
                        }
                        if r.RegisterSpace == 0
                            && let Some(index) = guest_register.checked_sub(r.BaseShaderRegister)
                            && index < r.NumDescriptors
                        {
                            let offset = u32::try_from(offset + u64::from(index))
                                .map_err(|_| E_INVALIDARG)?;
                            if source
                                .replace(SceneSource::Table { slot, offset })
                                .is_some()
                            {
                                return Err(E_INVALIDARG.into());
                            }
                        }
                    }
                }
                D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS => {
                    cost += u64::from(p.Anonymous.Constants.Num32BitValues)
                }
                D3D12_ROOT_PARAMETER_TYPE_CBV
                | D3D12_ROOT_PARAMETER_TYPE_UAV
                | D3D12_ROOT_PARAMETER_TYPE_SRV => {
                    cost += 2;
                    if !visible || p.ParameterType != D3D12_ROOT_PARAMETER_TYPE_SRV {
                        continue;
                    }
                    let d = p.Anonymous.Descriptor;
                    match (d.RegisterSpace, d.ShaderRegister) {
                        (1, reg) if reg < private_count => return Err(E_INVALIDARG.into()),
                        (0, reg)
                            if reg == guest_register
                                && source.replace(SceneSource::RootSrv(slot)).is_some() =>
                        {
                            return Err(E_INVALIDARG.into());
                        }
                        _ => {}
                    }
                }
                _ => return Err(E_INVALIDARG.into()),
            }
        }
    }
    if cost > 64 {
        return Err(E_INVALIDARG.into());
    }
    source.ok_or_else(|| E_INVALIDARG.into())
}

// Clone the root actually supplied by the caller, appending only the private
// SRVs in space1. Use the shader's embedded root when no root was supplied.
pub(super) fn private_root(
    device: &ID3D12Device,
    source: &[u8],
    translated: &[u8],
    explicit: Option<&RootSource>,
    node_mask: u32,
) -> Result<Option<SceneRoot>> {
    let Some((guest_register, triangle_geometry)) = inline_layout(source, translated) else {
        return Ok(None);
    };
    let program = Arc::new(InlineProgram {
        source_hash: super::shader_cache::source_digest(source).ok_or(E_INVALIDARG)?,
        triangle_geometry,
        first_dispatch: Once::new(),
    });
    let private_count = if triangle_geometry { 3 } else { 1 };
    let original = explicit.cloned().unwrap_or_else(|| RootSource::new(source));
    let bytes = &original.bytes;
    let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
    // The native deserializer owns and validates every pointer in the returned description.
    unsafe {
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let original_desc = reader
            .GetUnconvertedRootSignatureDesc()
            .as_ref()
            .ok_or(E_POINTER)?;
        let mut desc = match original_desc.Version {
            D3D_ROOT_SIGNATURE_VERSION_1_0 => {
                *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?
            }
            D3D_ROOT_SIGNATURE_VERSION_1_1 | D3D_ROOT_SIGNATURE_VERSION_1_2 => *original_desc,
            _ => return Err(E_INVALIDARG.into()),
        };
        let (count, ptr, samplers, flags) = if desc.Version == D3D_ROOT_SIGNATURE_VERSION_1_2 {
            let d = desc.Anonymous.Desc_1_2;
            (d.NumParameters, d.pParameters, d.NumStaticSamplers, d.Flags)
        } else {
            let d = desc.Anonymous.Desc_1_1;
            (d.NumParameters, d.pParameters, d.NumStaticSamplers, d.Flags)
        };
        if ptr.is_null() || count == 0 {
            return Err(E_INVALIDARG.into());
        }
        let mut params = core::slice::from_raw_parts(ptr, count as usize).to_vec();
        let binding = scene_source(&params, guest_register, private_count);
        crate::diagnostics::log_once(&Once::new(), || {
            let hash = super::shader_cache::source_digest(&bytes[original.payload.clone()])
                .unwrap_or_default();
            format!(
                "shader {} root {hash} v{} flags={:#x} staticSamplers={samplers} scene={binding:?}{}",
                program.source_hash,
                original_desc.Version.0,
                flags.0,
                describe(&params)
            )
        });
        let binding = binding?;
        for register in 0..private_count {
            params.push(D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                        ShaderRegister: register,
                        RegisterSpace: 1,
                        Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                    },
                },
            });
        }
        if desc.Version == D3D_ROOT_SIGNATURE_VERSION_1_2 {
            desc.Anonymous.Desc_1_2.NumParameters = count + private_count;
            desc.Anonymous.Desc_1_2.pParameters = params.as_ptr();
        } else {
            desc.Anonymous.Desc_1_1.NumParameters = count + private_count;
            desc.Anonymous.Desc_1_1.pParameters = params.as_ptr();
        }
        let mut serialized = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut serialized, None)?;
        let blob = serialized.ok_or(E_POINTER)?;
        let bytes =
            core::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        Ok(Some(SceneRoot {
            program,
            native: device.CreateRootSignature(node_mask, bytes)?,
            original,
            source: binding,
            appended: count,
        }))
    }
}
