//! Associate a shader-visible scene view with the exact guest AS pool and suballocation.
use super::{Buffer, Buffers, Kind, guest, lock};
use crate::device::Resource;
use std::sync::{Arc, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_FORMAT_R32_TYPELESS};
use windows::core::{IUnknown, Result};

#[derive(Clone)]
pub(in crate::device) struct SceneView {
    owner: Weak<Buffer>,
    address: u64,
}

impl Buffers {
    pub(in crate::device) fn scene_view(
        &self,
        resource: Option<&Resource>,
        desc: Option<&D3D12_SHADER_RESOURCE_VIEW_DESC>,
    ) -> Option<SceneView> {
        let desc = desc?;
        let (owner, address) = match desc.ViewDimension {
            D3D12_SRV_DIMENSION_BUFFER if desc.Format == DXGI_FORMAT_R32_TYPELESS => {
                let buffer = &resource?.state.blas_buffer.as_ref()?.buffer;
                if buffer.kind != Kind::Structure {
                    return None;
                }
                // SAFETY: ViewDimension selects the Buffer arm.
                let view = unsafe { desc.Anonymous.Buffer };
                if view.Flags != D3D12_BUFFER_SRV_FLAG_RAW || view.StructureByteStride != 0 {
                    return None;
                }
                let start = view.FirstElement.checked_mul(4)?;
                let end = start.checked_add(u64::from(view.NumElements) * 4)?;
                if view.NumElements == 0 || end > buffer.bytes {
                    return None;
                }
                (Arc::downgrade(buffer), buffer.start.checked_add(start)?)
            }
            D3D12_SRV_DIMENSION_RAYTRACING_ACCELERATION_STRUCTURE if resource.is_none() => {
                // SAFETY: the AS arm supplies an address instead of a resource argument.
                let address = unsafe { desc.Anonymous.RaytracingAccelerationStructure.Location };
                (
                    Arc::downgrade(&self.find(address, Kind::Structure).ok()?),
                    address,
                )
            }
            _ => return None,
        };
        if !address.is_multiple_of(u64::from(
            D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT,
        )) {
            return None;
        }
        Some(SceneView { owner, address })
    }
}

/// Return the current backing and its full GPU address for 219's raw t5.
/// The command allocator retains the returned resource through GPU execution.
///
/// # Safety
/// The owner must be the live controlling TuxBox device for this descriptor.
pub(crate) unsafe fn inline_scene(
    owner: &IUnknown,
    cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
) -> Result<(ID3D12Resource, u64)> {
    let registry = &unsafe { guest(owner) }.state.blas;
    let view = registry.views.lookup(cpu.ptr).ok_or(E_INVALIDARG)?;
    let pool = view.owner.upgrade().ok_or(E_INVALIDARG)?;
    // Placed buffers can overlap. A recycled address cannot resurrect a stale view.
    let current = registry.find(view.address, Kind::Structure)?;
    if !Arc::ptr_eq(&current, &pool) {
        return Err(E_NOTIMPL.into());
    }
    resolve_backing(&pool, view.address)
}

/// Resolve a guest root SRV whose value is the scene address itself.
///
/// # Safety
/// The controlling TuxBox device must own the command list setting this root SRV.
pub(crate) unsafe fn inline_scene_address(
    owner: &IUnknown,
    address: u64,
) -> Result<(ID3D12Resource, u64)> {
    if address == 0
        || !address.is_multiple_of(u64::from(
            D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BYTE_ALIGNMENT,
        ))
    {
        return Err(E_INVALIDARG.into());
    }
    let pool = unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Structure)?;
    resolve_backing(&pool, address)
}

fn resolve_backing(pool: &Buffer, address: u64) -> Result<(ID3D12Resource, u64)> {
    // A list may execute again. The native VA captured in a root descriptor
    // must remain stable, including if a later AS copy would need larger storage.
    let mut targets = lock(&pool.targets);
    match targets.entry(address).or_insert(None) {
        Some(backing) => Ok((backing.clone(), unsafe { backing.GetGPUVirtualAddress() })),
        None => Ok((pool.native.clone(), address)),
    }
}
