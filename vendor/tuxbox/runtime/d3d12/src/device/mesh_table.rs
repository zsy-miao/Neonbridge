//! Use the FSSM layout at the same root parameter indices the title binds.
use super::{compute::RootSource, compute_table::ComputeTable};
use std::{ops::Range, sync::Arc};
use tuxbox_dxbc::{
    mesh_table::{MeshLayout, shader_resources},
    rootsig::{RootParamKind, RootSig, parse_rts0},
};
use windows::Win32::{
    Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER},
    Graphics::Direct3D12::*,
};
use windows::core::{Interface, Result};

pub(super) fn reflected(
    device: &ID3D12Device,
    original: &RootSource,
    mesh: &[u8],
    pixel: Option<&[u8]>,
    node: u32,
) -> Result<Option<Arc<ComputeTable>>> {
    let Ok(root) = parse_rts0(original.payload()) else {
        return Ok(None);
    };
    let Some(layout) = MeshLayout::from_shader(mesh, &root) else {
        return Ok(None);
    };
    if layout.replacements.is_empty() {
        return Ok(None);
    }
    let mut lengths = vec![0; root.params.len()];
    let mut accessed_views = Vec::new();
    for ((_, offset), binding) in &layout.tables {
        let slot = offset / 32;
        let end = slot + 1;
        lengths[binding.parameter] = lengths[binding.parameter].max(end);
        if binding.kind != 2 {
            accessed_views.push((binding.parameter, slot..end));
        }
    }
    if let Some(pixel) = pixel {
        collect_pixel_accesses(&root, &layout, pixel, &mut lengths, &mut accessed_views)?;
    }
    let bytes = original.serialized();
    // SAFETY: the deserializer owns the source pointers throughout serialization.
    // Replacement tables stay live until the serialized native root is built.
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
        if source.NumParameters as usize != root.params.len() || source.pParameters.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let mut params = std::slice::from_raw_parts(source.pParameters, root.params.len()).to_vec();
        let mut range_storage = vec![Vec::new(); params.len()];
        for (index, change) in layout.replacements {
            let parameter = &mut params[index];
            parameter.ShaderVisibility = D3D12_SHADER_VISIBILITY_MESH;
            match change {
                RootParamKind::Table(ranges) => {
                    let table = parameter.Anonymous.DescriptorTable;
                    if table.NumDescriptorRanges == 0 || table.pDescriptorRanges.is_null() {
                        return Err(E_INVALIDARG.into());
                    }
                    let old = std::slice::from_raw_parts(
                        table.pDescriptorRanges,
                        table.NumDescriptorRanges as usize,
                    );
                    let storage = &mut range_storage[index];
                    for range in ranges {
                        let kind = D3D12_DESCRIPTOR_RANGE_TYPE(range.range_type as i32);
                        let mut equivalents = old.iter().filter(|r| r.RangeType == kind);
                        let flags = equivalents.next().map(|r| r.Flags).unwrap_or(
                            D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
                                | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
                        );
                        if equivalents.any(|r| r.Flags != flags) {
                            return Err(E_NOTIMPL.into());
                        }
                        storage.push(D3D12_DESCRIPTOR_RANGE1 {
                            RangeType: kind,
                            NumDescriptors: 1,
                            BaseShaderRegister: range.base_register,
                            RegisterSpace: range.space,
                            Flags: flags,
                            OffsetInDescriptorsFromTableStart: range.offset_in_table,
                        });
                    }
                    parameter.Anonymous.DescriptorTable = D3D12_ROOT_DESCRIPTOR_TABLE1 {
                        NumDescriptorRanges: storage.len() as u32,
                        pDescriptorRanges: storage.as_ptr(),
                    };
                }
                RootParamKind::Descriptor {
                    register, space, ..
                } => {
                    parameter.Anonymous.Descriptor.ShaderRegister = register;
                    parameter.Anonymous.Descriptor.RegisterSpace = space;
                }
                RootParamKind::Constants {
                    register, space, ..
                } => {
                    parameter.Anonymous.Constants.ShaderRegister = register;
                    parameter.Anonymous.Constants.RegisterSpace = space;
                }
            }
        }
        desc.Anonymous.Desc_1_1.pParameters = params.as_ptr();
        let mut blob = None;
        D3D12SerializeVersionedRootSignature(&desc, &mut blob, None)?;
        let blob = blob.ok_or(E_POINTER)?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast(), blob.GetBufferSize());
        Ok(Some(Arc::new(ComputeTable {
            native: device.CreateRootSignature(node, bytes)?,
            original: original.clone(),
            layout: RootSource::new(bytes),
            lengths,
            accessed_views,
            index_map: None,
        })))
    }
}

fn collect_pixel_accesses(
    root: &RootSig,
    mesh: &MeshLayout,
    pixel: &[u8],
    lengths: &mut [u32],
    views: &mut Vec<(usize, Range<u32>)>,
) -> Result<()> {
    for [ty, space, low, high] in shader_resources(pixel).ok_or(E_NOTIMPL)? {
        let kind = match ty {
            1 => continue, // sampler tables and static samplers are unchanged
            2 => 2,
            3..=5 | 9 => 0,
            6..=8 => 1,
            _ => return Err(E_NOTIMPL.into()),
        };
        let last = u64::from(high) + 1;
        for (index, param) in root
            .params
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.visibility, 0 | 5))
        {
            let rewritten = mesh.replacements.contains_key(&index);
            match &param.kind {
                RootParamKind::Table(ranges) => {
                    let mut next = 0;
                    for range in ranges {
                        let start = if range.offset_in_table == u32::MAX {
                            next
                        } else {
                            u64::from(range.offset_in_table)
                        };
                        next = start.checked_add(u64::from(range.num)).ok_or(E_NOTIMPL)?;
                        if range.range_type != kind || range.space != space {
                            continue;
                        }
                        let base = u64::from(range.base_register);
                        let from = base.max(u64::from(low));
                        let to = (base + u64::from(range.num)).min(last);
                        if from >= to {
                            continue;
                        }
                        if rewritten {
                            return Err(E_NOTIMPL.into());
                        }
                        let start = u32::try_from(start + (from - base)).map_err(|_| E_NOTIMPL)?;
                        let end = start.checked_add((to - from) as u32).ok_or(E_NOTIMPL)?;
                        lengths[index] = lengths[index].max(end);
                        if kind != 2 {
                            views.push((index, start..end));
                        }
                    }
                }
                RootParamKind::Descriptor {
                    kind: descriptor,
                    register,
                    space: owner,
                } if rewritten
                    && *descriptor == 3
                    && kind == 0
                    && *owner == space
                    && (low..=high).contains(register) =>
                {
                    return Err(E_NOTIMPL.into());
                }
                RootParamKind::Constants {
                    register,
                    space: owner,
                    ..
                } if rewritten
                    && kind == 2
                    && *owner == space
                    && (low..=high).contains(register) =>
                {
                    return Err(E_NOTIMPL.into());
                }
                _ => {}
            }
        }
    }
    Ok(())
}
