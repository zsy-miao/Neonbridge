//! Xbox List6 batches 48-byte DXR build descriptions with 32 bytes of extensions.
use super::{
    List, Recording,
    raytracing::{SceneCommand, retain},
    recording,
};
use crate::device::{blas, ray_build};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
use windows::core::{GUID, IUnknown, Result};

pub(crate) const LIST6: GUID = GUID::from_u128(0xc3827890_4024_474c_98cf_645b51dc4874);

#[repr(C)]
pub(super) struct Entry {
    build: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
    extension: [u64; 4],
}
const _: () = assert!(size_of::<Entry>() == 80);

// Upload-backed instance records can be edited after Close and before each Execute.
// Keep a separate upload so the guest always sees its original GPU addresses.
pub(super) enum Instances {
    Direct,
    Gpu(blas::instance_fixup::Instances),
    Upload {
        address: u64,
        count: usize,
        indirect: bool,
        patched: ID3D12Resource,
    },
}

const INSTANCE_BYTES: usize = size_of::<D3D12_RAYTRACING_INSTANCE_DESC>();
const AS_OFFSET: usize =
    core::mem::offset_of!(D3D12_RAYTRACING_INSTANCE_DESC, AccelerationStructure);
const _: () = assert!(INSTANCE_BYTES == 64 && AS_OFFSET == 56);

fn read_upload(owner: &IUnknown, address: u64, out: &mut [u8]) -> Result<()> {
    let (buffer, offset, available) = unsafe { blas::upload_range(owner, address)? };
    if available < out.len() as u64 {
        return Err(E_INVALIDARG.into());
    }
    let begin = offset as usize;
    let range = D3D12_RANGE {
        Begin: begin,
        End: begin + out.len(),
    };
    let mut data = core::ptr::null_mut();
    unsafe { buffer.Map(0, Some(&range), Some(&mut data))? };
    let mut copied = 0;
    let result = unsafe {
        ReadProcessMemory(
            GetCurrentProcess(),
            data.cast::<u8>().add(begin).cast(),
            out.as_mut_ptr().cast(),
            out.len(),
            Some(&mut copied),
        )
    };
    unsafe { buffer.Unmap(0, Some(&D3D12_RANGE::default())) };
    result?;
    if copied != out.len() {
        return Err(E_INVALIDARG.into());
    }
    Ok(())
}

impl Instances {
    pub(super) unsafe fn prepare(&self, owner: &IUnknown) -> Result<()> {
        if let Self::Gpu(instances) = self {
            return unsafe { instances.prepare(owner) };
        }
        let Self::Upload {
            address,
            count,
            indirect,
            patched,
        } = self
        else {
            // Foreign addresses and GPU arrays of pointers have no safely resolved
            // input range. Native traversal remains valid until a BLAS moves.
            if unsafe { blas::has_relocations(owner) } {
                return Err(E_NOTIMPL.into());
            }
            return Ok(());
        };
        let bytes = *count * INSTANCE_BYTES;
        let mut records = Vec::new();
        records
            .try_reserve_exact(bytes)
            .map_err(|_| E_OUTOFMEMORY)?;
        records.resize(bytes, 0u8);
        if *indirect {
            let mut pointers = Vec::new();
            let pointer_bytes = count * 8;
            pointers
                .try_reserve_exact(pointer_bytes)
                .map_err(|_| E_OUTOFMEMORY)?;
            pointers.resize(pointer_bytes, 0u8);
            read_upload(owner, *address, &mut pointers)?;
            for (ptr, record) in pointers
                .chunks_exact(8)
                .zip(records.chunks_exact_mut(INSTANCE_BYTES))
            {
                let va = u64::from_le_bytes(ptr.try_into().unwrap());
                if !va.is_multiple_of(u64::from(D3D12_RAYTRACING_INSTANCE_DESCS_BYTE_ALIGNMENT)) {
                    return Err(E_INVALIDARG.into());
                }
                read_upload(owner, va, record)?;
            }
        } else {
            read_upload(owner, *address, &mut records)?;
        }
        let relocated = unsafe { blas::relocated(owner) };
        for record in records.chunks_exact_mut(INSTANCE_BYTES) {
            let field = &mut record[AS_OFFSET..];
            let address = u64::from_le_bytes((*field).try_into().unwrap());
            if let Ok(index) = relocated.binary_search_by_key(&address, |&(guest, _)| guest) {
                field.copy_from_slice(&relocated[index].1.to_le_bytes());
            }
        }
        let mut dest = core::ptr::null_mut();
        unsafe {
            patched.Map(0, Some(&D3D12_RANGE::default()), Some(&mut dest))?;
            core::ptr::copy_nonoverlapping(records.as_ptr(), dest.cast(), bytes);
            patched.Unmap(
                0,
                Some(&D3D12_RANGE {
                    Begin: 0,
                    End: bytes,
                }),
            );
        }
        Ok(())
    }
}

fn translate(
    state: &mut Recording,
    owner: &IUnknown,
    host: &ID3D12GraphicsCommandList4,
    mut desc: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
) -> Result<(
    D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
    Option<ID3D12Resource>,
    u64,
)> {
    let relocated = unsafe { blas::resolve(owner, desc.DestAccelerationStructureData) }?;
    let mut info = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO::default();
    let mut device = None::<ID3D12Device5>;
    unsafe { host.GetDevice(&mut device)? };
    unsafe {
        device
            .ok_or(E_NOTIMPL)?
            .GetRaytracingAccelerationStructurePrebuildInfo(&desc.Inputs, &mut info)
    };
    let scratch = if desc
        .Inputs
        .Flags
        .contains(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE)
    {
        info.UpdateScratchDataSizeInBytes
    } else {
        info.ScratchDataSizeInBytes
    };
    if info.ResultDataMaxSizeInBytes == 0
        || (desc.ScratchAccelerationStructureData == 0 && scratch != 0)
    {
        return Err(E_INVALIDARG.into());
    }
    if let Some(target) = relocated {
        if info.ResultDataMaxSizeInBytes > unsafe { target.GetDesc() }.Width {
            return Err(E_NOTIMPL.into());
        }
        retain(state, &[&target])?;
        desc.DestAccelerationStructureData = unsafe { target.GetGPUVirtualAddress() };
    }
    if desc.SourceAccelerationStructureData != 0
        && let Some(source) = unsafe { blas::resolve(owner, desc.SourceAccelerationStructureData) }?
    {
        retain(state, &[&source])?;
        desc.SourceAccelerationStructureData = unsafe { source.GetGPUVirtualAddress() };
    }
    if desc.Inputs.Type != D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL {
        return Ok((desc, None, info.ResultDataMaxSizeInBytes));
    }
    if desc.Inputs.NumDescs == 0 {
        // Failure to allocate optional shader metadata cannot invalidate an
        // ordinary empty TLAS build. Inline traversal will report its absence.
        return Ok((
            desc,
            unsafe { blas::empty_instance_buffer(owner) }.ok(),
            info.ResultDataMaxSizeInBytes,
        ));
    }
    let address = unsafe { desc.Inputs.Anonymous.InstanceDescs };
    if desc.Inputs.NumDescs > D3D12_RAYTRACING_MAX_INSTANCES_PER_TOP_LEVEL_ACCELERATION_STRUCTURE {
        return Err(E_INVALIDARG.into());
    }
    let count = desc.Inputs.NumDescs as usize;
    let indirect = desc.Inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS;
    let stride = if indirect { 8 } else { INSTANCE_BYTES };
    let bytes = (count * stride) as u64;
    let instances = match unsafe { blas::upload_range(owner, address) } {
        Ok((source, _, available)) => {
            if available < bytes {
                return Err(E_INVALIDARG.into());
            }
            let patched = unsafe { blas::instance_buffer(owner, (count * INSTANCE_BYTES) as u64)? };
            retain(state, &[&source, &patched])?;
            desc.Inputs.Anonymous.InstanceDescs = unsafe { patched.GetGPUVirtualAddress() };
            desc.Inputs.DescsLayout = D3D12_ELEMENTS_LAYOUT_ARRAY;
            Instances::Upload {
                address,
                count,
                indirect,
                patched,
            }
        }
        Err(e) if e.code() != E_INVALIDARG => return Err(e),
        Err(_) => match unsafe { blas::default_instances(owner, address) } {
            Ok((source, available)) if !indirect => {
                if available < bytes
                    || unsafe { source.GetDesc() }
                        .Flags
                        .contains(D3D12_RESOURCE_FLAG_DENY_SHADER_RESOURCE)
                {
                    return Err(E_INVALIDARG.into());
                }
                let patched = unsafe {
                    blas::instance_fixup::Instances::record(owner, host, address, count as u32)?
                };
                unsafe {
                    state.bindings.restore(host);
                }
                retain(state, &[&source, patched.output()])?;
                desc.Inputs.Anonymous.InstanceDescs =
                    unsafe { patched.output().GetGPUVirtualAddress() };
                Instances::Gpu(patched)
            }
            Err(e) if e.code() != E_INVALIDARG => return Err(e),
            _ => Instances::Direct,
        },
    };
    let snapshot = match &instances {
        Instances::Direct => None,
        Instances::Upload { patched, .. } => Some(patched.clone()),
        Instances::Gpu(records) => Some(records.output().clone()),
    };
    state.scene_commands.push(SceneCommand::Fixup(instances));
    Ok((desc, snapshot, info.ResultDataMaxSizeInBytes))
}

// The scene builder passes one ordinary 48-byte description to List4, along
// with the standard count and optional array of postbuild requests.
pub(super) unsafe extern "system" fn single(
    list: &List,
    desc: *const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC,
    postbuild_count: u32,
    postbuild: *const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC,
) {
    if desc.is_null() || (postbuild_count != 0 && postbuild.is_null()) {
        // The BVH builder sends this for a build it cannot express.
        super::Recording::skip("acceleration structure build: missing description", E_INVALIDARG);
        return;
    }
    let entry = Entry {
        build: unsafe { *desc },
        extension: [0; 4],
    };
    let outputs = if postbuild_count == 0 {
        &[]
    } else {
        // The guest supplies this array for the duration of the build call.
        unsafe { core::slice::from_raw_parts(postbuild, postbuild_count as usize) }
    };
    unsafe { record_builds(list, 1, &entry, outputs) };
}

pub(super) unsafe extern "system" fn batch(list: &List, count: u32, entries: *const Entry) {
    unsafe { record_builds(list, count, entries, &[]) };
}

/// Why a guest build entry cannot be recorded, if it cannot.
fn refusal(entry: &Entry) -> Option<(&'static str, windows::core::HRESULT)> {
    if entry.extension != [0; 4] {
        return Some(("unsupported extension", E_NOTIMPL));
    }
    let b = &entry.build;
    let alignment = u64::from(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT);
    let update = b
        .Inputs
        .Flags
        .contains(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE);
    if !unsafe { ray_build::valid(&b.Inputs) } {
        return Some(("invalid inputs", E_INVALIDARG));
    }
    if b.DestAccelerationStructureData == 0
        || !(b.DestAccelerationStructureData
            | b.ScratchAccelerationStructureData
            | b.SourceAccelerationStructureData)
            .is_multiple_of(alignment)
        || (update && b.SourceAccelerationStructureData == 0)
    {
        return Some(("destination, scratch or source address", E_INVALIDARG));
    }
    if b.Inputs.Type == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL
        && b.Inputs.NumDescs != 0
    {
        let va = unsafe { b.Inputs.Anonymous.InstanceDescs };
        let align = if b.Inputs.DescsLayout == D3D12_ELEMENTS_LAYOUT_ARRAY {
            u64::from(D3D12_RAYTRACING_INSTANCE_DESCS_BYTE_ALIGNMENT)
        } else {
            8
        };
        if va == 0 || !va.is_multiple_of(align) {
            return Some(("instance address", E_INVALIDARG));
        }
    }
    None
}

unsafe fn record_builds(
    list: &List,
    count: u32,
    entries: *const Entry,
    outputs: &[D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC],
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) || count == 0 {
        return;
    }
    let Some(host) = &list.state.raytracing else {
        state.error = E_NOTIMPL;
        return;
    };
    if entries.is_null() {
        state.error = E_INVALIDARG;
        return;
    }
    // Build without any size report TuxBox cannot produce.
    let outputs: Vec<_> = outputs
        .iter()
        .filter(|output| match super::raytracing::validate_size_output(output) {
            Ok(()) => true,
            Err(error) => {
                super::Recording::skip(
                    &format!(
                        "acceleration structure report type {} at {:#x}",
                        output.InfoType.0, output.DestBuffer
                    ),
                    error.code(),
                );
                false
            }
        })
        .copied()
        .collect();
    let entries = unsafe { core::slice::from_raw_parts(entries, count as usize) };
    let Some(owner) = &list.parent else {
        state.error = E_INVALIDARG;
        return;
    };
    for entry in entries {
        // One build can need a late-bound instance table, an update-source
        // check, and a destination publication, in that command order.
        if state.scene_commands.try_reserve(3).is_err() {
            state.error = E_OUTOFMEMORY;
            return;
        }
        // A build TuxBox cannot record is left out on its own, so the rest of
        // the list still runs.
        let translated = match refusal(entry) {
            Some((reason, code)) => Err((reason, code)),
            None => translate(&mut state, owner, host, entry.build)
                .map_err(|e| ("translation", e.code())),
        };
        let (build, instances, bytes) = match translated {
            Ok(build) => build,
            Err((reason, code)) => {
                let b = &entry.build;
                super::Recording::skip(
                    &format!(
                        "acceleration structure build: {reason} (type={} flags={:#x} count={} layout={} dest={:#x} scratch={:#x} source={:#x} extension={:x?})",
                        b.Inputs.Type.0,
                        b.Inputs.Flags.0,
                        b.Inputs.NumDescs,
                        b.Inputs.DescsLayout.0,
                        b.DestAccelerationStructureData,
                        b.ScratchAccelerationStructureData,
                        b.SourceAccelerationStructureData,
                        entry.extension,
                    ),
                    code,
                );
                continue;
            }
        };
        if entry.build.SourceAccelerationStructureData != 0 {
            state.scene_commands.push(SceneCommand::Source {
                guest: entry.build.SourceAccelerationStructureData,
                native: build.SourceAccelerationStructureData,
            });
        }
        unsafe {
            host.BuildRaytracingAccelerationStructure(&build, (!outputs.is_empty()).then_some(outputs.as_slice()));
            let guest_va = entry.build.DestAccelerationStructureData;
            let native_va = build.DestAccelerationStructureData;
            let command =
                if build.Inputs.Type == D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL {
                    blas::set_inline_instances(owner, guest_va, native_va, instances.clone());
                    SceneCommand::Instances {
                        guest: guest_va,
                        native: native_va,
                        records: instances,
                        bytes,
                    }
                } else {
                    SceneCommand::Native {
                        guest: guest_va,
                        native: native_va,
                        bytes,
                        build: blas::NativeBuild::record(owner, guest_va, native_va, &build.Inputs),
                    }
                };
            state.scene_commands.push(command);
        }
    }
}
