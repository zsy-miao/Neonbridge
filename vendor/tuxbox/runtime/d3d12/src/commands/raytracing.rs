//! Xbox list slot 119 records AS copies and deserializes cooked console geometry.
use super::{List, Recording, recording};
use crate::device::blas;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{IUnknown, Result};

// Replay geometry publications in GPU command order. Upload-backed BLAS
// vertices, indices, transforms and cooked payloads can all change after Close.
pub(super) enum SceneCommand {
    Native {
        guest: u64,
        native: u64,
        build: blas::NativeBuild,
        bytes: u64,
    },
    Fixup(super::build_raytracing::Instances),
    Source {
        guest: u64,
        native: u64,
    },
    Cooked(blas::Input),
    Instances {
        guest: u64,
        native: u64,
        records: Option<ID3D12Resource>,
        bytes: u64,
    },
    Copy {
        guest: u64,
        native: u64,
        source_guest: u64,
        source_native: u64,
        capacity: Option<u64>,
    },
    Check(super::compute_bindings::SceneCheck),
}

impl SceneCommand {
    pub(super) fn prepare(&self, list: &List) -> Result<()> {
        let owner = list.parent.as_ref().ok_or(E_INVALIDARG)?;
        unsafe {
            match self {
                Self::Native {
                    guest,
                    native,
                    build,
                    bytes,
                } => {
                    build.prepare(owner);
                    blas::publish_destination(owner, *guest, *native, Some(*bytes));
                }
                Self::Fixup(instances) => instances.prepare(owner)?,
                Self::Cooked(input) => input.prepare(owner)?,
                Self::Instances {
                    guest,
                    native,
                    records,
                    bytes,
                } => {
                    blas::set_inline_instances(owner, *guest, *native, records.clone());
                    blas::publish_destination(owner, *guest, *native, Some(*bytes));
                }
                Self::Source { guest, native } => validate_source(owner, *guest, *native)?,
                Self::Copy {
                    guest,
                    native,
                    source_guest,
                    source_native,
                    capacity,
                } => {
                    validate_source(owner, *source_guest, *source_native)?;
                    let bytes = blas::submitted_size(owner, *source_guest, *source_native);
                    if let Some(capacity) = capacity
                        && bytes.is_none_or(|needed| needed > *capacity)
                    {
                        static FIRST: std::sync::Once = std::sync::Once::new();
                        crate::diagnostics::log_once(&FIRST, || {
                            format!(
                                "AS copy {source_guest:#x} ({source_native:#x}) to {guest:#x} ({native:#x}): submitted source bound {bytes:?}, private capacity {capacity}"
                            )
                        });
                        return Err(E_NOTIMPL.into());
                    }
                    blas::copy_geometry(owner, *guest, *native, *source_native);
                    blas::publish_destination(owner, *guest, *native, bytes);
                }
                Self::Check(check) => check.validate(list).inspect_err(|_| {
                    static FIRST: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&FIRST, || {
                        "inline ray scene or geometry changed after recording, rejecting submission"
                            .into()
                    });
                })?,
            }
        }
        Ok(())
    }
}

fn validate_source(owner: &IUnknown, guest: u64, recorded: u64) -> Result<()> {
    let submitted = unsafe { blas::submitted_address(owner, guest) };
    if submitted != recorded {
        crate::diagnostics::recording_error(format_args!(
            "recorded AS source {guest:#x} used {recorded:#x}, now resolves to {submitted:#x}"
        ));
        return Err(E_INVALIDARG.into());
    }
    Ok(())
}

pub(super) fn retain(recording: &Recording, resources: &[&ID3D12Resource]) -> Result<()> {
    let mut held = recording
        .resources
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    held.try_reserve(resources.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    held.extend(resources.iter().map(|r| ID3D12Pageable::from((*r).clone())));
    Ok(())
}

fn record(
    state: &mut Recording,
    host: &ID3D12GraphicsCommandList4,
    owner: &IUnknown,
    destination: u64,
    source: u64,
    mode: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE,
) -> Result<()> {
    let alignment = u64::from(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT);
    if source == 0
        || destination == 0
        || !source.is_multiple_of(alignment)
        || !destination.is_multiple_of(alignment)
    {
        return Err(E_INVALIDARG.into());
    }
    match mode {
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE
        | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT => {
            // SAFETY: owner is the controlling device retained by the list.
            let resolve = |address, role| {
                unsafe { blas::resolve(owner, address) }.inspect_err(|err| {
                static FIRST: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&FIRST, || format!(
                    "AS copy {source:#x} to {destination:#x}: {role} registration at {address:#x}: {:?}", err.code()
                ));
            })
            };
            let source_as = resolve(source, "source")?;
            let destination_as = resolve(destination, "destination")?;
            if source == destination {
                return Err(E_INVALIDARG.into());
            }
            let (src, dst, capacity) = if let Some(backing) = source_as {
                let (dst, va) = unsafe { blas::copy_dest(owner, destination, &backing) }?;
                retain(state, &[&backing, &dst])?;
                (
                    unsafe { backing.GetGPUVirtualAddress() },
                    va,
                    Some(unsafe { dst.GetDesc() }.Width),
                )
            } else if let Some(backing) = destination_as {
                let direct = unsafe { blas::direct_source(owner, source) }.map_err(|err| {
                    static FIRST: std::sync::Once = std::sync::Once::new();
                    crate::diagnostics::log_once(&FIRST, || format!(
                        "AS copy {source:#x} to relocated {destination:#x}: no unique registered native source, {:?}", err.code()
                    ));
                    if err.code() == E_INVALIDARG { windows::core::Error::from(E_NOTIMPL) } else { err }
                })?;
                retain(state, &[&direct, &backing])?;
                (
                    source,
                    unsafe { backing.GetGPUVirtualAddress() },
                    Some(unsafe { backing.GetDesc() }.Width),
                )
            } else {
                (source, destination, None)
            };
            state
                .scene_commands
                .try_reserve(1)
                .map_err(|_| E_OUTOFMEMORY)?;
            unsafe {
                host.CopyRaytracingAccelerationStructure(dst, src, mode);
                blas::copy_geometry(owner, destination, dst, src);
            }
            state.scene_commands.push(SceneCommand::Copy {
                guest: destination,
                native: dst,
                source_guest: source,
                source_native: src,
                capacity,
            });
        }
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_DESERIALIZE => {
            state
                .scene_commands
                .try_reserve(1)
                .map_err(|_| E_OUTOFMEMORY)?;
            let result = unsafe { blas::deserialize(owner, source, destination) }?;
            retain(state, &[&result.source, &result.destination])?;
            state
                .scene_commands
                .push(SceneCommand::Cooked(result.input));
            // SAFETY: the private build is complete, the copy fits the checked
            // guest suballocation or its dedicated backing. The guest allocator
            // retains both resources until all executions using it have completed.
            unsafe {
                host.CopyRaytracingAccelerationStructure(
                    result.destination_address,
                    result.source.GetGPUVirtualAddress(),
                    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT,
                )
            };
        }
        _ => return Err(E_NOTIMPL.into()),
    }
    Ok(())
}

pub(super) unsafe extern "system" fn copy(
    list: &List,
    destination: u64,
    source: u64,
    mode: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let Some(host) = &list.state.raytracing else {
        state.error = E_NOTIMPL;
        return;
    };
    let Some(owner) = &list.parent else {
        state.error = E_POINTER;
        return;
    };
    if let Err(error) = record(&mut state, host, owner, destination, source, mode) {
        crate::diagnostics::recording_error(format_args!(
            "slot 119 AS copy mode {}, source {source:#x}, destination {destination:#x}: {:?}",
            mode.0,
            error.code()
        ));
        state.error = error.code();
    }
}

pub(super) fn validate_size_output(
    desc: &D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC,
) -> Result<()> {
    if !matches!(
        desc.InfoType,
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE
            | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_CURRENT_SIZE
    ) {
        return Err(E_NOTIMPL.into());
    }
    if desc.DestBuffer == 0 || !desc.DestBuffer.is_multiple_of(8) {
        return Err(E_INVALIDARG.into());
    }
    Ok(())
}

// FD3D12RayTracingCompactionRequestHandler::Update passes a destination,
// count and array of BLAS GPU addresses to Xbox list slot 118 (RVA 0x57a95d5).
pub(super) unsafe extern "system" fn postbuild(
    list: &List,
    desc: *const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC,
    count: u32,
    addresses: *const u64,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    let result = (|| -> Result<()> {
        let desc = unsafe { desc.as_ref() }.ok_or(E_POINTER)?;
        let host = list.state.raytracing.as_ref().ok_or(E_NOTIMPL)?;
        let owner = list.parent.as_ref().ok_or(E_POINTER)?;
        validate_size_output(desc)?;
        if count == 0 {
            return Ok(());
        }
        if addresses.is_null() {
            return Err(E_POINTER.into());
        }
        let sources = unsafe { std::slice::from_raw_parts(addresses, count as usize) };
        let alignment = u64::from(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT);
        if sources
            .iter()
            .any(|&va| va == 0 || !va.is_multiple_of(alignment))
        {
            return Err(E_INVALIDARG.into());
        }
        let mut patched = Vec::new();
        patched
            .try_reserve_exact(sources.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        state
            .scene_commands
            .try_reserve(sources.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        for &guest in sources {
            let native = if let Some(target) = unsafe { blas::resolve(owner, guest) }? {
                retain(&state, &[&target])?;
                unsafe { target.GetGPUVirtualAddress() }
            } else {
                guest
            };
            patched.push(native);
            state
                .scene_commands
                .push(SceneCommand::Source { guest, native });
        }
        unsafe { host.EmitRaytracingAccelerationStructurePostbuildInfo(desc, &patched) };
        Ok(())
    })();
    if let Err(error) = result {
        state.error = error.code();
        crate::diagnostics::recording_error(format_args!(
            "slot 118 AS size query: {:?}",
            error.code()
        ));
    }
}
