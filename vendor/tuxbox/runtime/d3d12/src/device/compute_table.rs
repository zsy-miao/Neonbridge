//! Desktop signatures for shaders whose physical table layout is encoded in FSSM.
use super::compute::RootSource;
use std::sync::Arc;
use tuxbox_dxbc::rootsig::{RootParamKind, parse_rts0};
use windows::Win32::Foundation::{E_INVALIDARG, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{Interface, Result};

pub(crate) struct ComputeTable {
    pub(crate) native: ID3D12RootSignature,
    pub(crate) original: RootSource,
    pub(crate) layout: RootSource,
    pub(crate) lengths: Vec<u32>,
    pub(crate) accessed_views: Vec<(usize, std::ops::Range<u32>)>,
    pub(crate) index_map: Option<IndexMapBinding>,
    /// Root slot of the root SRV holding xshader's literal sampler lookup
    /// table (t1, space32766; see desc_table).
    pub(crate) desc_table: Option<u32>,
    /// Root slot of the root SRV holding typed view formats per heap slot
    /// (t2, space32766; see view_formats).
    pub(crate) view_formats: Option<u32>,
    /// Root slot of the root SRV holding buffer view addresses per heap slot
    /// (t3, space32766; see slot_views).
    pub(crate) slot_views: Option<u32>,
    /// Root parameter of the console descriptor words table (t6, space32766; slot_words).
    pub(crate) slot_words: Option<u32>,
    /// The first root parameter of the table-word SRVs and the guest root
    /// parameters they cover, in order (t<p>, space32765; table_words).
    pub(crate) table_words: Option<(u32, Vec<u32>)>,
    /// TUXBOX_BVH_RAW_SRV (default on, =0 off): the first of the raw root SRVs at t5..,
    /// space32766 over each guest root SRV parameter's acceleration
    /// structure, and those guest parameters' slots (see bvh_raw).
    pub(crate) bvh_raw: Option<(u32, Vec<u32>)>,
    /// TUXBOX_ROOT_RAW_STRIDE: the root SRV slot of tb_root_raw_stride
    /// (t4 space32766) and each guest root SRV/UAV parameter's slot and
    /// key, register + 1024 for a UAV (see root_strides).
    pub(crate) root_strides: Option<(u32, Vec<(u32, u32)>)>,
    /// Root slot of the two root constants of SetSampleMaskX's quad mask
    /// (b0, space32766; see quad_mask).
    pub(crate) quad_mask: Option<u32>,
    /// Root slot of the root SRV holding view-mark entries per heap slot
    /// (t4, space32766), the bitmap's root UAV (u0) the next one (see
    /// view_marks).
    pub(crate) view_marks: Option<u32>,
}

#[derive(Clone, Copy)]
pub(crate) struct IndexMapBinding {
    pub(crate) slot: u32,
    pub(crate) scalar_word: u32,
    pub(crate) scalar_mask: u32,
}

#[derive(Clone, Copy)]
pub(super) enum Program<'a> {
    Compute(&'a [u8]),
    Raster { vertex: &'a [u8], pixel: &'a [u8] },
}

pub(super) enum TableLayout {
    Unsupported,
    Original,
    Reflected(Arc<ComputeTable>),
}

pub(super) fn reflected(
    device: &ID3D12Device,
    original: &RootSource,
    program: Program<'_>,
    node: u32,
) -> Result<TableLayout> {
    let layout = reflected_tables(device, original, program, node)?;
    let flags = parse_rts0(original.payload()).map_or(0, |s| s.flags);
    // Bindless raster pipelines: translated stages read ResourceDescriptorHeap through
    // the index map, or only SamplerDescriptorHeap, which needs the guest root alone.
    if let (TableLayout::Unsupported, Program::Raster { vertex, pixel }) = (&layout, program)
        && flags & 0xc00 != 0
    {
        let (v, p) = (features(vertex), features(pixel));
        if flags & 0x400 != 0 && (v | p) & (1 << 25) != 0 {
            return heap_compute(device, original, node, &[]);
        }
        if (v | p) & (1 << 26) != 0 {
            return Ok(TableLayout::Original);
        }
    }
    // Translated heap-indexed compute shaders also need the index map.
    if let (TableLayout::Reflected(table), Program::Compute(shader)) = (&layout, program)
        && flags & 0x400 != 0
        && heap_indexed(shader)
        && table.index_map.is_none()
    {
        let samplers = parse_rts0(table.layout.payload()).ok().filter(|s| s.static_samplers.is_empty())
            .and_then(|_| tuxbox_dxbc::inline_sampler::heap_compute(shader)).unwrap_or_default();
        return extend_root_from(device, &table.layout, original, node, true, &samplers,
            table.lengths.clone(), table.accessed_views.clone());
    }
    if let (TableLayout::Original | TableLayout::Unsupported, Program::Compute(shader)) = (&layout, program)
        && flags & 0x400 != 0
        && heap_indexed(shader)
    {
        let samplers = parse_rts0(original.payload()).ok().filter(|s| s.static_samplers.is_empty())
            .and_then(|_| tuxbox_dxbc::inline_sampler::heap_compute(shader)).unwrap_or_default();
        return heap_compute(device, original, node, &samplers);
    }
    // Literal sampler states the guest root does not declare become private static
    // samplers appended to it, so translated stages can sample with them.
    if matches!(layout, TableLayout::Original | TableLayout::Unsupported)
        && let Ok(signature) = parse_rts0(original.payload())
        && !signature.static_samplers.iter().any(|s| s[11] == 32765)
    {
        let shader = match program {
            Program::Compute(shader) => shader,
            Program::Raster { pixel, .. } => pixel,
        };
        if let Some(samplers) = tuxbox_dxbc::inline_sampler::literal(shader) {
            return extend_root(device, original, node, false, &samplers);
        }
    }
    Ok(layout)
}

fn reflected_tables(
    device: &ID3D12Device,
    original: &RootSource,
    program: Program<'_>,
    node: u32,
) -> Result<TableLayout> {
    let Ok(signature) = parse_rts0(original.payload()) else {
        return Ok(TableLayout::Unsupported);
    };
    let mut scalar_index = None;
    let mut resolve_constants = None;
    let mut recovered_samplers = Vec::new();
    let bindings = match (&program, signature.params.as_slice()) {
        (Program::Compute(shader), params) => {
            let tables = match params {
                [tables @ .., last] if matches!(last.kind, RootParamKind::Constants { num32: 1..=4, .. }) => tables,
                _ => params,
            };
            if !(1..=2).contains(&tables.len()) || tables.iter().any(|p| !matches!(p.kind, RootParamKind::Table(_))) {
                return Ok(TableLayout::Unsupported);
            }
            if signature.static_samplers.is_empty() && signature.flags & 0x800 == 0 {
                recovered_samplers = tuxbox_dxbc::inline_sampler::private(shader, 5).unwrap_or_default();
            }
            if tables.len() == 2 {
                tuxbox_dxbc::compute_table::compute_root_bindings(shader, &signature).map(
                    |(bindings, constants)| {
                        resolve_constants = constants;
                        bindings
                    },
                )
            } else {
                tuxbox_dxbc::compute_table::bindings(shader, 5)
            }
        }
        (Program::Raster { vertex, pixel }, [table, rest @ ..])
            if matches!(table.kind, RootParamKind::Table(_)) && (rest.is_empty()
                || matches!(rest, [param] if signature.flags & 0xc00 != 0 &&
                matches!(param.kind, RootParamKind::Constants { num32: 1..=4, .. }))) =>
        {
            if let [constants] = rest
                && let RootParamKind::Constants { num32, .. } = constants.kind
                && let Some(index) = tuxbox_dxbc::compute_table::scalar_heap_index(pixel, num32)
            {
                capture_stage_root(pixel, original);
                if signature.flags & 0x400 != 0 { scalar_index = Some(index); }
            }
            if !rest.is_empty() && signature.flags & 0x800 == 0 && scalar_index.is_none() {
                return Ok(TableLayout::Unsupported);
            }
            let ordinary = rest.is_empty() && signature.flags & 0xc00 == 0;
            let both_stages = |check_user_layout| {
                let get = |shader: &[u8], stage| {
                    if check_user_layout {
                        tuxbox_dxbc::compute_table::root_bindings(shader, stage, &signature)
                    } else {
                        tuxbox_dxbc::compute_table::bindings(shader, stage)
                    }
                };
                Some((get(vertex, 1)?, get(pixel, 0)?))
            };
            let checked = ordinary.then(|| both_stages(true)).flatten();
            let Some((mut bindings, pixel_bindings)) = checked.or_else(|| both_stages(false)) else {
                return Ok(TableLayout::Unsupported);
            };
            if signature.static_samplers.is_empty() && ordinary {
                recovered_samplers = tuxbox_dxbc::inline_sampler::pixel(pixel)
                    .map(|s| vec![s])
                    .or_else(|| tuxbox_dxbc::inline_sampler::private(pixel, 0))
                    .unwrap_or_default();
            }
            for b in pixel_bindings {
                if let Some(existing) = bindings.iter().find(|r| r.slot == b.slot) {
                    if *existing != b {
                        return Err(E_INVALIDARG.into());
                    }
                } else {
                    bindings.push(b);
                }
            }
            bindings.sort_by_key(|r| r.slot);
            Some(bindings)
        }
        _ => return Ok(TableLayout::Unsupported),
    };
    let Some(bindings) = bindings else {
        if let Program::Compute(shader) = program {
            return append_corroborated_ranges(device, original, shader, &signature, node);
        }
        return Ok(TableLayout::Unsupported);
    };
    if signature.version != 2 || (bindings.is_empty() && scalar_index.is_none()) {
        return Ok(TableLayout::Unsupported);
    }
    let mut lengths = [0; 2];
    let mut rewrite = [false; 2];
    let mut accessed_views = Vec::new();
    for (index, param) in signature.params.iter().enumerate() {
        if let RootParamKind::Table(ranges) = &param.kind
            && (index >= lengths.len() || param.visibility != 0 || ranges.is_empty()
                || ranges.iter().any(|r| r.range_type > 2 || r.num == u32::MAX))
        {
            return Ok(TableLayout::Unsupported);
        }
    }
    for b in &bindings {
        let index = b.table as usize;
        let Some(RootParamKind::Table(ranges)) = signature.params.get(index).map(|p| &p.kind) else {
            return Ok(TableLayout::Unsupported);
        };
        let end = b.slot.checked_add(1).ok_or(E_INVALIDARG)?;
        lengths[index] = lengths[index].max(end);
        if b.kind < 2 { accessed_views.push((index, b.slot..end)); }
        let mut next = 0;
        let covered = ranges.iter().any(|r| {
            let start = if r.offset_in_table == u32::MAX { next } else { r.offset_in_table };
            next = start.saturating_add(r.num);
            (start..next).contains(&b.slot)
                && r.range_type == b.kind && r.space == b.space
                && r.base_register.checked_add(b.slot - start) == Some(b.register)
        });
        rewrite[index] |= !covered;
    }
    let rewrite_constants = resolve_constants.filter(|c| {
        !matches!(signature.params[2].kind, RootParamKind::Constants { register, space, .. }
            if (register, space) == (c.register, c.space))
    });
    if !rewrite.contains(&true) && scalar_index.is_none() && rewrite_constants.is_none() && recovered_samplers.is_empty() {
        return Ok(TableLayout::Original);
    }

    let bytes = original.serialized();
    // Keep the original static samplers, descriptor volatility and root flags.
    // The input description stays live through D3D12SerializeVersionedRootSignature.
    unsafe {
        let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let mut desc = *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?;
        let source = desc.Anonymous.Desc_1_1;
        if source.NumParameters as usize != signature.params.len() || source.pParameters.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut parameters =
            std::slice::from_raw_parts(source.pParameters, signature.params.len()).to_vec();
        let mut range_storage = [Vec::new(), Vec::new()];
        for (index, ranges) in range_storage.iter_mut().enumerate().filter(|(i, _)| rewrite[*i]) {
            let parameter = &mut parameters[index];
            let table = parameter.Anonymous.DescriptorTable;
            let guest_ranges = std::slice::from_raw_parts(table.pDescriptorRanges, table.NumDescriptorRanges as usize);
            for b in bindings.iter().filter(|b| b.table as usize == index) {
                let ty = D3D12_DESCRIPTOR_RANGE_TYPE(b.kind as i32);
                let mut matched = guest_ranges.iter().filter(|r| r.RangeType == ty);
                let flags = if let Some(first) = matched.next() {
                    if matched.any(|r| r.Flags != first.Flags) { return Ok(TableLayout::Unsupported); }
                    first.Flags
                } else {
                    // The guest root omits this resource class from the table (the shader's
                    // FSSM binding records name it): volatile flags accept any descriptor.
                    D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE
                };
                ranges.push(D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: ty,
                    NumDescriptors: 1,
                    BaseShaderRegister: b.register,
                    RegisterSpace: b.space,
                    Flags: flags,
                    OffsetInDescriptorsFromTableStart: b.slot,
                });
            }
            parameter.Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: ranges.len() as u32, pDescriptorRanges: ranges.as_ptr(),
            };
        }
        if let Some(constants) = rewrite_constants {
            parameters[2].Anonymous.Constants = D3D12_ROOT_CONSTANTS {
                ShaderRegister: constants.register,
                RegisterSpace: constants.space,
                Num32BitValues: 4,
            };
        }
        let index_map = scalar_index.map(|(scalar_word, scalar_mask)| IndexMapBinding {
            slot: source.NumParameters, scalar_word, scalar_mask,
        });
        if index_map.is_some() {
            // Read through a root SRV so the map itself never occupies a
            // guest-addressable descriptor. The CPU can publish new indices
            // after this draw has been recorded.
            parameters.push(D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                        ShaderRegister: 0,
                        RegisterSpace: 32766,
                        Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                    },
                },
            });
            desc.Anonymous.Desc_1_1.NumParameters = parameters.len() as u32;
        }
        let samplers: Vec<_> = recovered_samplers.iter().map(static_sampler).collect();
        if !samplers.is_empty() {
            desc.Anonymous.Desc_1_1.pStaticSamplers = samplers.as_ptr();
            desc.Anonymous.Desc_1_1.NumStaticSamplers = samplers.len() as u32;
        }
        desc.Anonymous.Desc_1_1.pParameters = parameters.as_ptr();
        let _padded = pad_root_constants(&mut desc);
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        let native = device.CreateRootSignature(node, bytes)?;
        Ok(TableLayout::Reflected(Arc::new(ComputeTable {
            native,
            original: original.clone(),
            layout: RootSource::new(bytes),
            lengths: lengths.into(),
            accessed_views,
            index_map,
            desc_table: None,
            view_formats: None,
            slot_views: None,
            slot_words: None,
            table_words: None,
            bvh_raw: None,
            root_strides: None,
            view_marks: None,
            quad_mask: None,
        })))
    }
}

fn static_sampler(s: &[u32; 13]) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER(s[0] as i32),
        AddressU: D3D12_TEXTURE_ADDRESS_MODE(s[1] as i32),
        AddressV: D3D12_TEXTURE_ADDRESS_MODE(s[2] as i32),
        AddressW: D3D12_TEXTURE_ADDRESS_MODE(s[3] as i32),
        MipLODBias: f32::from_bits(s[4]), MaxAnisotropy: s[5],
        ComparisonFunc: D3D12_COMPARISON_FUNC(s[6] as i32), BorderColor: D3D12_STATIC_BORDER_COLOR(s[7] as i32),
        MinLOD: f32::from_bits(s[8]), MaxLOD: f32::from_bits(s[9]), ShaderRegister: s[10], RegisterSpace: s[11],
        ShaderVisibility: D3D12_SHADER_VISIBILITY(s[12] as i32),
    }
}

/// SFI0 required feature flags.
fn features(shader: &[u8]) -> u64 {
    let Ok(d) = tuxbox_dxbc::Dxbc::parse(shader) else { return 0 };
    d.chunk(b"SFI0")
        .and_then(|c| shader.get(c.offset..c.offset + c.size))
        .and_then(|b| b.first_chunk::<8>())
        .map_or(0, |b| u64::from_le_bytes(*b))
}

/// The shader indexes ResourceDescriptorHeap (SFI0 bit 25).
fn heap_indexed(shader: &[u8]) -> bool {
    features(shader) & (1 << 25) != 0
}

/// The guest signature plus a root SRV holding the resource index map, which
/// translated compute shaders read as `t0, space32766` to turn guest heap
/// indices into resident ones.
fn heap_compute(device: &ID3D12Device, original: &RootSource, node: u32, samplers: &[[u32; 13]]) -> Result<TableLayout> {
    extend_root(device, original, node, true, samplers)
}

/// The guest root plus, optionally, the resource index map SRV and private static samplers.
fn extend_root(device: &ID3D12Device, original: &RootSource, node: u32, index_map: bool, samplers: &[[u32; 13]]) -> Result<TableLayout> {
    extend_root_from(device, original, original, node, index_map, samplers, Vec::new(), Vec::new())
}

#[allow(clippy::too_many_arguments)]
fn extend_root_from(
    device: &ID3D12Device, base: &RootSource, original: &RootSource, node: u32, index_map: bool,
    samplers: &[[u32; 13]], lengths: Vec<u32>, accessed_views: Vec<(usize, std::ops::Range<u32>)>,
) -> Result<TableLayout> {
    let bytes = base.serialized();
    unsafe {
        let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(), bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID, (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let mut desc = *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?;
        let source = desc.Anonymous.Desc_1_1;
        let mut parameters = if source.NumParameters == 0 { Vec::new() } else {
            if source.pParameters.is_null() { return Err(E_INVALIDARG.into()); }
            std::slice::from_raw_parts(source.pParameters, source.NumParameters as usize).to_vec()
        };
        if index_map { parameters.push(D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                    ShaderRegister: 0, RegisterSpace: 32766, Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
                },
            },
        }); }
        desc.Anonymous.Desc_1_1.NumParameters = parameters.len() as u32;
        desc.Anonymous.Desc_1_1.pParameters = parameters.as_ptr();
        // Literal sampler states in the ISA, as private static samplers in space 32765.
        let mut samplers: Vec<_> = samplers.iter().map(|s| static_sampler(s)).collect();
        if !samplers.is_empty() {
            if source.NumStaticSamplers != 0 {
                if source.pStaticSamplers.is_null() { return Err(E_INVALIDARG.into()); }
                let guest = std::slice::from_raw_parts(source.pStaticSamplers, source.NumStaticSamplers as usize);
                samplers.splice(0..0, guest.iter().copied());
            }
            // Guest anisotropic point-mip filters the host lacks go linear-mip.
            for s in &mut samplers { s.Filter = super::features::host_filter(device, s.Filter); }
            desc.Anonymous.Desc_1_1.pStaticSamplers = samplers.as_ptr();
            desc.Anonymous.Desc_1_1.NumStaticSamplers = samplers.len() as u32;
        }
        let _padded = pad_root_constants(&mut desc);
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let layout = std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        let native = device.CreateRootSignature(node, layout)?;
        Ok(TableLayout::Reflected(Arc::new(ComputeTable {
            native,
            original: original.clone(),
            layout: RootSource::new(layout),
            lengths,
            accessed_views,
            index_map: index_map.then_some(IndexMapBinding { slot: source.NumParameters, scalar_word: 0, scalar_mask: 0 }),
            desc_table: None,
            view_formats: None,
            slot_views: None,
            slot_words: None,
            table_words: None,
            bvh_raw: None,
            root_strides: None,
            view_marks: None,
            quad_mask: None,
        })))
    }
}

/// `table`'s root, or the guest root for a pipeline without one, plus a root
/// SRV at t1, space32766 for translated stages that declare tb_desc_table
/// (`desc`) and one at t2, space32766 for those that declare tb_view_formats
/// (`formats`). Existing parameters keep their slots.
pub(super) fn with_runtime_tables(
    device: &ID3D12Device, table: Option<&Arc<ComputeTable>>, guest: &RootSource, node: u32,
    desc: bool, formats: bool, slots: bool, marks: bool, quad: bool, bvh: bool, strides: bool, words: bool, tw: &[u32],
) -> Result<Arc<ComputeTable>> {
    let add_tw = !tw.is_empty() && !table.is_some_and(|t| t.table_words.is_some());
    let add_words = words && !table.is_some_and(|t| t.slot_words.is_some());
    let add_desc = desc && !table.is_some_and(|t| t.desc_table.is_some());
    let add_formats = formats && !table.is_some_and(|t| t.view_formats.is_some());
    let add_slots = slots && !table.is_some_and(|t| t.slot_views.is_some());
    let add_marks = marks && !table.is_some_and(|t| t.view_marks.is_some());
    let add_quad = quad && !table.is_some_and(|t| t.quad_mask.is_some());
    let add_bvh = bvh && !table.is_some_and(|t| t.bvh_raw.is_some());
    let add_strides = strides && !table.is_some_and(|t| t.root_strides.is_some());
    if let Some(table) = table.filter(|_| !add_tw && !add_words && !add_desc && !add_formats && !add_slots && !add_marks && !add_quad && !add_bvh && !add_strides) {
        return Ok(table.clone());
    }
    let root_srv = |register: u32, space: u32| D3D12_ROOT_PARAMETER1 {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_SRV,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        Anonymous: D3D12_ROOT_PARAMETER1_0 {
            Descriptor: D3D12_ROOT_DESCRIPTOR1 {
                ShaderRegister: register,
                RegisterSpace: space,
                Flags: D3D12_ROOT_DESCRIPTOR_FLAG_DATA_VOLATILE,
            },
        },
    };
    let bytes = table.map_or(guest, |t| &t.layout).serialized();
    unsafe {
        let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(), bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID, (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let mut desc = *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?;
        let source = desc.Anonymous.Desc_1_1;
        let mut parameters = if source.NumParameters == 0 { Vec::new() } else {
            if source.pParameters.is_null() { return Err(E_INVALIDARG.into()); }
            std::slice::from_raw_parts(source.pParameters, source.NumParameters as usize).to_vec()
        };
        let desc_slot = if add_desc {
            parameters.push(root_srv(super::desc_table::REGISTER, super::desc_table::SPACE));
            Some(parameters.len() as u32 - 1)
        } else { table.and_then(|t| t.desc_table) };
        let formats_slot = if add_formats {
            parameters.push(root_srv(super::view_formats::REGISTER, super::view_formats::SPACE));
            Some(parameters.len() as u32 - 1)
        } else { table.and_then(|t| t.view_formats) };
        let slots_slot = if add_slots {
            parameters.push(root_srv(super::slot_views::REGISTER, super::slot_views::SPACE));
            Some(parameters.len() as u32 - 1)
        } else { table.and_then(|t| t.slot_views) };
        let words_slot = if add_words {
            parameters.push(root_srv(super::slot_words::REGISTER, super::slot_words::SPACE));
            Some(parameters.len() as u32 - 1)
        } else { table.and_then(|t| t.slot_words) };
        let tw_slot = if add_tw {
            let first = parameters.len() as u32;
            for &p in tw {
                parameters.push(root_srv(p, super::table_words::SPACE));
            }
            Some((first, tw.to_vec()))
        } else { table.and_then(|t| t.table_words.clone()) };
        // Write marks (view_marks, VIctory cd94d35): the table, then the bitmap.
        let marks_slot = if add_marks {
            parameters.push(root_srv(super::view_marks::TABLE_REGISTER, super::view_marks::SPACE));
            parameters.push(D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_UAV,
                ..root_srv(super::view_marks::BITMAP_REGISTER, super::view_marks::SPACE)
            });
            Some(parameters.len() as u32 - 2)
        } else { table.and_then(|t| t.view_marks) };
        let quad_slot = if add_quad {
            parameters.push(D3D12_ROOT_PARAMETER1 {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
                Anonymous: D3D12_ROOT_PARAMETER1_0 {
                    Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: super::quad_mask::REGISTER, RegisterSpace: super::quad_mask::SPACE, Num32BitValues: 2 },
                },
            });
            Some(parameters.len() as u32 - 1)
        } else { table.and_then(|t| t.quad_mask) };
        let bvh_slot = if add_bvh {
            let guests: Vec<u32> = parameters.iter().enumerate()
                .filter(|(_, p)| p.ParameterType == D3D12_ROOT_PARAMETER_TYPE_SRV
                    && p.Anonymous.Descriptor.RegisterSpace != super::bvh_raw::SPACE)
                .map(|(i, _)| i as u32).collect();
            let first = parameters.len() as u32;
            for k in 0..guests.len() as u32 {
                parameters.push(root_srv(super::bvh_raw::REGISTER + k, super::bvh_raw::SPACE));
            }
            (!guests.is_empty()).then_some((first, guests))
        } else { table.and_then(|t| t.bvh_raw.clone()) };
        let strides_slot = if add_strides {
            let guests: Vec<(u32, u32)> = parameters.iter().enumerate()
                .filter(|(_, p)| matches!(p.ParameterType, D3D12_ROOT_PARAMETER_TYPE_SRV | D3D12_ROOT_PARAMETER_TYPE_UAV)
                    && p.Anonymous.Descriptor.RegisterSpace != super::root_strides::SPACE)
                .map(|(i, p)| (i as u32, p.Anonymous.Descriptor.ShaderRegister
                    + if p.ParameterType == D3D12_ROOT_PARAMETER_TYPE_UAV { 1024 } else { 0 }))
                .collect();
            parameters.push(root_srv(super::root_strides::REGISTER, super::root_strides::SPACE));
            Some((parameters.len() as u32 - 1, guests))
        } else { table.and_then(|t| t.root_strides.clone()) };
        desc.Anonymous.Desc_1_1.NumParameters = parameters.len() as u32;
        desc.Anonymous.Desc_1_1.pParameters = parameters.as_ptr();
        let _padded = pad_root_constants(&mut desc);
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let layout = std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        let native = device.CreateRootSignature(node, layout)?;
        Ok(Arc::new(ComputeTable {
            native,
            original: table.map_or_else(|| guest.clone(), |t| t.original.clone()),
            layout: RootSource::new(layout),
            lengths: table.map(|t| t.lengths.clone()).unwrap_or_default(),
            accessed_views: table.map(|t| t.accessed_views.clone()).unwrap_or_default(),
            index_map: table.and_then(|t| t.index_map),
            desc_table: desc_slot,
            view_formats: formats_slot,
            slot_views: slots_slot,
            slot_words: words_slot,
            table_words: tw_slot,
            view_marks: marks_slot,
            quad_mask: quad_slot,
            bvh_raw: bvh_slot,
            root_strides: strides_slot,
        }))
    }
}

/// Keep every embedded range and append FSSM slots the root left out.
/// Shaders that already translate keep their parameter indexes. The game's
/// descriptor-table handle still addresses the slot FSSM recorded.
fn append_corroborated_ranges(
    device: &ID3D12Device,
    original: &RootSource,
    shader: &[u8],
    signature: &tuxbox_dxbc::rootsig::RootSig,
    node: u32,
) -> Result<TableLayout> {
    if signature.version != 2 {
        return Ok(TableLayout::Unsupported);
    }
    let extra = tuxbox_dxbc::compute_table::missing_root_descriptors(shader, signature);
    if extra.is_empty() {
        return Ok(TableLayout::Unsupported);
    }
    let bytes = original.serialized();
    unsafe {
        let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(),
            bytes.len(),
            &ID3D12VersionedRootSignatureDeserializer::IID,
            (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let mut desc = *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?;
        let source = desc.Anonymous.Desc_1_1;
        if source.NumParameters as usize != signature.params.len() || source.pParameters.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut parameters =
            std::slice::from_raw_parts(source.pParameters, signature.params.len()).to_vec();
        let mut stored = vec![Vec::new(); signature.params.len()];
        let mut lengths = vec![0u32; signature.params.len()];
        let mut accessed_views = Vec::new();
        for (index, param) in signature.params.iter().enumerate() {
            let RootParamKind::Table(declared) = &param.kind else { continue };
            let additions: Vec<_> = extra.iter().filter(|binding| binding.table as usize == index).collect();
            if additions.is_empty() {
                continue;
            }
            let table = parameters[index].Anonymous.DescriptorTable;
            let guest = std::slice::from_raw_parts(
                table.pDescriptorRanges,
                table.NumDescriptorRanges as usize,
            );
            let mut ranges = guest.to_vec();
            for binding in &additions {
                let ty = D3D12_DESCRIPTOR_RANGE_TYPE(binding.kind as i32);
                let flags = guest
                    .iter()
                    .find(|range| range.RangeType == ty)
                    .map(|range| range.Flags)
                    .unwrap_or(
                        D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
                            | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                    );
                ranges.push(D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: ty,
                    NumDescriptors: 1,
                    BaseShaderRegister: binding.register,
                    RegisterSpace: binding.space,
                    Flags: flags,
                    OffsetInDescriptorsFromTableStart: binding.slot,
                });
            }
            stored[index] = ranges;
            parameters[index].Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: stored[index].len() as u32,
                pDescriptorRanges: stored[index].as_ptr(),
            };
            let Some((mut extent, mut views)) = finite_table(declared) else { continue };
            for binding in &additions {
                extent = extent.max(binding.slot.saturating_add(1));
                if binding.kind < 2 {
                    views.push(binding.slot..binding.slot + 1);
                }
            }
            lengths[index] = extent;
            accessed_views.extend(views.into_iter().map(|range| (index, range)));
        }
        desc.Anonymous.Desc_1_1.pParameters = parameters.as_ptr();
        let _padded = pad_root_constants(&mut desc);
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        let native = device.CreateRootSignature(node, bytes)?;
        Ok(TableLayout::Reflected(Arc::new(ComputeTable {
            native,
            original: original.clone(),
            layout: RootSource::new(bytes),
            lengths,
            accessed_views,
            index_map: None,
            desc_table: None,
            view_formats: None,
            slot_views: None,
            slot_words: None,
            table_words: None,
            bvh_raw: None,
            root_strides: None,
            view_marks: None,
            quad_mask: None,
        })))
    }
}

/// SRV-as-UAV piece 3: the guest root with each `.slots` slot (an SRV the
/// program stores to, see shader_cache::slots) split out of its SRV range
/// as a one-descriptor UAV range at the same table offset, for this
/// pipeline only. Its tables still point at the guest heap; the dispatch
/// puts a UAV in each such slot (piece 4). None: a slot does not fall in
/// an SRV range of a finite table, or names no register.
pub(super) fn srv_as_uav(
    device: &ID3D12Device,
    original: &RootSource,
    slots: &[super::shader_cache::slots::Slot],
    node: u32,
) -> Result<Option<Arc<ComputeTable>>> {
    let Ok(signature) = parse_rts0(original.payload()) else { return Ok(None) };
    if signature.version != 2 {
        return Ok(None);
    }
    let bytes = original.serialized();
    unsafe {
        let mut reader = None::<ID3D12VersionedRootSignatureDeserializer>;
        D3D12CreateVersionedRootSignatureDeserializer(
            bytes.as_ptr().cast(), bytes.len(), &ID3D12VersionedRootSignatureDeserializer::IID, (&raw mut reader).cast(),
        )?;
        let reader = reader.ok_or(E_POINTER)?;
        let mut desc = *reader.GetRootSignatureDescAtVersion(D3D_ROOT_SIGNATURE_VERSION_1_1)?;
        let source = desc.Anonymous.Desc_1_1;
        if source.NumParameters as usize != signature.params.len() || source.pParameters.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut parameters = std::slice::from_raw_parts(source.pParameters, signature.params.len()).to_vec();
        let mut stored = vec![Vec::new(); signature.params.len()];
        let mut lengths = vec![0u32; signature.params.len()];
        let mut accessed_views = Vec::new();
        for (index, param) in signature.params.iter().enumerate() {
            let RootParamKind::Table(declared) = &param.kind else { continue };
            let Some((extent, views)) = finite_table(declared) else {
                if slots.iter().any(|s| s.param as usize == index) { return Ok(None); }
                continue;
            };
            let mine: Vec<_> = slots.iter().filter(|s| s.param as usize == index).collect();
            if mine.is_empty() {
                continue;
            }
            let table = parameters[index].Anonymous.DescriptorTable;
            let guest = std::slice::from_raw_parts(table.pDescriptorRanges, table.NumDescriptorRanges as usize);
            // Resolve append offsets so split ranges keep their places.
            let mut next = 0u32;
            let mut ranges: Vec<D3D12_DESCRIPTOR_RANGE1> = guest.iter().map(|r| {
                let start = if r.OffsetInDescriptorsFromTableStart == u32::MAX { next } else { r.OffsetInDescriptorsFromTableStart };
                next = start.saturating_add(r.NumDescriptors);
                D3D12_DESCRIPTOR_RANGE1 { OffsetInDescriptorsFromTableStart: start, ..*r }
            }).collect();
            for slot in &mine {
                // The guest range named by its index (earlier splits keep the
                // guest ranges' order before them only when slots are sorted, so
                // look it up by its original offset).
                let Some(named) = guest.get(slot.range as usize) else { return Ok(None) };
                let named_start = {
                    let mut next = 0u32;
                    let mut found = None;
                    for (i, r) in guest.iter().enumerate() {
                        let start = if r.OffsetInDescriptorsFromTableStart == u32::MAX { next } else { r.OffsetInDescriptorsFromTableStart };
                        next = start.saturating_add(r.NumDescriptors);
                        if i == slot.range as usize { found = Some(start); }
                    }
                    found
                };
                let Some(at) = ranges.iter().position(|r| r.RangeType == D3D12_DESCRIPTOR_RANGE_TYPE_SRV
                    && r.RegisterSpace == named.RegisterSpace
                    && (r.OffsetInDescriptorsFromTableStart..r.OffsetInDescriptorsFromTableStart + r.NumDescriptors).contains(&slot.slot))
                else { return Ok(None) };
                let r = ranges.remove(at);
                let start = r.OffsetInDescriptorsFromTableStart;
                let before = slot.slot - start;
                let register = slot.register.unwrap_or(named.BaseShaderRegister + slot.slot.saturating_sub(named_start.unwrap_or(start)));
                let after = r.NumDescriptors - before - 1;
                if before != 0 {
                    ranges.push(D3D12_DESCRIPTOR_RANGE1 { NumDescriptors: before, ..r });
                }
                ranges.push(D3D12_DESCRIPTOR_RANGE1 {
                    RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
                    NumDescriptors: 1,
                    BaseShaderRegister: register,
                    RegisterSpace: slot.space,
                    Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                    OffsetInDescriptorsFromTableStart: slot.slot,
                });
                if after != 0 {
                    ranges.push(D3D12_DESCRIPTOR_RANGE1 {
                        NumDescriptors: after,
                        BaseShaderRegister: r.BaseShaderRegister + before + 1,
                        OffsetInDescriptorsFromTableStart: slot.slot + 1,
                        ..r
                    });
                }
            }
            stored[index] = ranges;
            parameters[index].Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: stored[index].len() as u32,
                pDescriptorRanges: stored[index].as_ptr(),
            };
            lengths[index] = extent;
            accessed_views.extend(views.into_iter().map(|range| (index, range)));
        }
        desc.Anonymous.Desc_1_1.pParameters = parameters.as_ptr();
        let _padded = pad_root_constants(&mut desc);
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let bytes = std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        let native = device.CreateRootSignature(node, bytes)?;
        Ok(Some(Arc::new(ComputeTable {
            native,
            original: original.clone(),
            layout: RootSource::new(bytes),
            lengths,
            accessed_views,
            index_map: None,
            desc_table: None,
            view_formats: None,
            slot_views: None,
            slot_words: None,
            table_words: None,
            bvh_raw: None,
            root_strides: None,
            view_marks: None,
            quad_mask: None,
        })))
    }
}

/// Exclusive end of a finite descriptor table, plus the SRV and UAV spans in it.
fn finite_table(
    ranges: &[tuxbox_dxbc::rootsig::RootRange],
) -> Option<(u32, Vec<std::ops::Range<u32>>)> {
    let mut end = 0u32;
    let mut extent = 0u32;
    let mut views = Vec::new();
    for range in ranges {
        if range.num == u32::MAX {
            return None;
        }
        let start = if range.offset_in_table == u32::MAX { end } else { range.offset_in_table };
        let next = start.checked_add(range.num)?;
        end = next;
        extent = extent.max(next);
        if range.range_type < 2 {
            views.push(start..next);
        }
    }
    Some((extent, views))
}

pub(super) fn capture_stage_root(stage: &[u8], root: &RootSource) {
    use std::{io::Write, path::PathBuf, sync::LazyLock};
    static ROOTS: LazyLock<Option<PathBuf>> =
        LazyLock::new(|| std::env::var_os("TUXBOX_CAPTURE_RASTER_ROOTS").map(Into::into));
    static INPUTS: LazyLock<Option<PathBuf>> =
        LazyLock::new(|| std::env::var_os("TUXBOX_CAPTURE_SHADER_INPUTS").map(Into::into));
    if ROOTS.is_none() && INPUTS.is_none() { return; }
    let Some(shader) = super::shader_cache::source_digest(stage) else { return; };
    let write = |path: PathBuf, data: &[u8]| {
        if let Ok(mut file) = std::fs::File::create_new(&path)
            && file.write_all(data).is_err()
        {
            drop(file);
            let _ = std::fs::remove_file(path);
        }
    };
    if let Some(directory) = ROOTS.as_ref()
        && let Some(signature) = super::shader_cache::source_digest(root.payload())
    {
        write(directory.join(format!("{shader}-{signature}.rootsig")), root.serialized());
    }
    if let Some(directory) = INPUTS.as_ref() {
        write(directory.join(format!("{shader}.dxbc")), stage);
    }
}

/// TUXBOX_ROOTCONST_PAD4 (default on, =0 off) rounds every root-constant parameter of a host
/// compute root up to whole 16-byte rows while the root stays within 64
/// DWORDs. test140-153: four compute PSOs failed E_INVALIDARG on title roots
/// whose b500 constants hold 1-3 DWORDs, while the translated cbuffer is at
/// least one uint4; D3D12 refuses a root constant smaller than the shader's
/// cbuffer. The title sets only its first N; the padding is never written.
/// Opt-in (it changes root digests, so warm pipeline caches miss).
/// Returns the parameter copy the description points at, kept alive until
/// serialization.
unsafe fn pad_root_constants(desc: &mut D3D12_VERSIONED_ROOT_SIGNATURE_DESC) -> Option<Vec<D3D12_ROOT_PARAMETER1>> {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| crate::env_toggle!("TUXBOX_ROOTCONST_PAD4") != Some("0")) || desc.Version != D3D_ROOT_SIGNATURE_VERSION_1_1 {
        return None;
    }
    let d = unsafe { &mut desc.Anonymous.Desc_1_1 };
    if d.pParameters.is_null() || d.NumParameters == 0 {
        return None;
    }
    let mut params = unsafe { core::slice::from_raw_parts(d.pParameters, d.NumParameters as usize) }.to_vec();
    let cost = |p: &D3D12_ROOT_PARAMETER1| match p.ParameterType {
        D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS => unsafe { p.Anonymous.Constants.Num32BitValues },
        D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE => 1,
        _ => 2,
    };
    let mut total: u32 = params.iter().map(cost).sum();
    let mut changed = false;
    for p in params.iter_mut() {
        if p.ParameterType != D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS {
            continue;
        }
        let n = unsafe { p.Anonymous.Constants.Num32BitValues };
        let padded = n.next_multiple_of(4);
        if padded != n && total + (padded - n) <= 64 {
            total += padded - n;
            p.Anonymous.Constants.Num32BitValues = padded;
            changed = true;
        }
    }
    if !changed {
        return None;
    }
    d.pParameters = params.as_ptr();
    Some(params)
}
