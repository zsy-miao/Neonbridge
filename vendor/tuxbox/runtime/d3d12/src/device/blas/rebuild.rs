//! Build the decoded BLAS off the title's queue and query its compacted size
//! before recording a compact copy in the original command stream.
use crate::commands::serialized_blas::Triangle;
use core::mem::ManuallyDrop;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, E_POINTER, HANDLE};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::{Interface, Result};

pub(super) fn buffer(
    device: &ID3D12Device,
    bytes: u64,
    heap: D3D12_HEAP_TYPE,
    state: D3D12_RESOURCE_STATES,
    flags: D3D12_RESOURCE_FLAGS,
) -> Result<ID3D12Resource> {
    let properties = D3D12_HEAP_PROPERTIES {
        Type: heap,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: flags,
        ..Default::default()
    };
    let mut native = None;
    // SAFETY: both input records and the output reference live through creation.
    unsafe {
        device.CreateCommittedResource(
            &properties,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            state,
            None,
            &mut native,
        )
    }?;
    native.ok_or_else(|| E_POINTER.into())
}

pub(super) fn as_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource> {
    buffer(
        device,
        bytes,
        D3D12_HEAP_TYPE_DEFAULT,
        D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
        D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
    )
}

pub(in crate::device) fn upload_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource> {
    buffer(
        device,
        bytes,
        D3D12_HEAP_TYPE_UPLOAD,
        D3D12_RESOURCE_STATE_GENERIC_READ,
        D3D12_RESOURCE_FLAG_NONE,
    )
}

fn prepare_geometry(
    device: &ID3D12Device,
    triangles: &[Triangle],
) -> Result<(ID3D12Resource, Vec<D3D12_RAYTRACING_GEOMETRY_DESC>)> {
    let bytes = triangles.len().checked_mul(36).ok_or(E_INVALIDARG)?;
    let mesh = upload_buffer(device, bytes as u64)?;
    let mut descs = Vec::new();
    let geometries = triangles.last().ok_or(E_INVALIDARG)?.geometry as usize + 1;
    descs
        .try_reserve_exact(geometries)
        .map_err(|_| E_OUTOFMEMORY)?;
    let mut position = unsafe { mesh.GetGPUVirtualAddress() };
    for group in triangles.chunk_by(|a, b| a.geometry == b.geometry) {
        let count = u32::try_from(group.len())
            .ok()
            .and_then(|n| n.checked_mul(3))
            .ok_or(E_INVALIDARG)?;
        let vertices = D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC {
            VertexFormat: DXGI_FORMAT_R32G32B32_FLOAT,
            VertexCount: count,
            VertexBuffer: D3D12_GPU_VIRTUAL_ADDRESS_AND_STRIDE {
                StartAddress: position,
                StrideInBytes: 12,
            },
            ..Default::default()
        };
        descs.push(D3D12_RAYTRACING_GEOMETRY_DESC {
            Type: D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES,
            Flags: D3D12_RAYTRACING_GEOMETRY_FLAG_NONE,
            Anonymous: D3D12_RAYTRACING_GEOMETRY_DESC_0 {
                Triangles: vertices,
            },
        });
        position = position
            .checked_add(u64::from(count) * 12)
            .ok_or(E_INVALIDARG)?;
    }
    let mut ptr = core::ptr::null_mut();
    unsafe { mesh.Map(0, Some(&D3D12_RANGE::default()), Some(&mut ptr)) }?;
    // SAFETY: this private upload holds exactly one contiguous float3 triplet per
    // decoded triangle. Coordinates and winding are already checked by the parser.
    for (i, t) in triangles.iter().enumerate() {
        unsafe { ptr.cast::<[[f32; 3]; 3]>().add(i).write(t.vertices) };
    }
    unsafe { mesh.Unmap(0, None) };
    Ok((mesh, descs))
}

struct Pending {
    _input: ID3D12Resource,
    _scratch: Option<ID3D12Resource>,
    result: ID3D12Resource,
}

pub(super) struct Builder {
    device: ID3D12Device5,
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList4,
    fence: ID3D12Fence,
    tick: u64,
    sizes: ID3D12Resource,
    readback: ID3D12Resource,
    pending: Option<Pending>,
}

impl Builder {
    pub(super) fn new(device: &ID3D12Device) -> Result<Self> {
        let desc = D3D12_COMMAND_QUEUE_DESC {
            Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
            ..Default::default()
        };
        let allocator = unsafe { device.CreateCommandAllocator(desc.Type) }?;
        let list: ID3D12GraphicsCommandList4 =
            unsafe { device.CreateCommandList(0, desc.Type, &allocator, None) }?;
        unsafe { list.Close() }?;
        Ok(Self {
            device: device.cast()?,
            queue: unsafe { device.CreateCommandQueue(&desc) }?,
            allocator,
            list,
            fence: unsafe { device.CreateFence(0, D3D12_FENCE_FLAG_NONE) }?,
            tick: 0,
            sizes: buffer(
                device,
                8,
                D3D12_HEAP_TYPE_DEFAULT,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            )?,
            readback: buffer(
                device,
                8,
                D3D12_HEAP_TYPE_READBACK,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_FLAG_NONE,
            )?,
            pending: None,
        })
    }

    fn transition_sizes(&self, before: D3D12_RESOURCE_STATES, after: D3D12_RESOURCE_STATES) {
        let transition = D3D12_RESOURCE_TRANSITION_BARRIER {
            pResource: ManuallyDrop::new(Some(self.sizes.clone())),
            Subresource: 0,
            StateBefore: before,
            StateAfter: after,
        };
        let mut barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                Transition: ManuallyDrop::new(transition),
            },
        };
        // SAFETY: the active transition union member owns one native reference.
        unsafe {
            self.list.ResourceBarrier(core::slice::from_ref(&barrier));
            ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
        }
    }

    fn begin(&self) -> Result<()> {
        if self.pending.is_some() {
            return Err(E_FAIL.into());
        }
        unsafe {
            self.allocator.Reset()?;
            self.list.Reset(&self.allocator, None)
        }
    }

    fn finish(&mut self, pending: Pending) -> Result<ID3D12Resource> {
        let list = self.list.cast::<ID3D12CommandList>()?;
        self.tick = self.tick.checked_add(1).ok_or(E_FAIL)?;
        // Keep all buffers alive while the queue owns this submission, including
        // failed signal/wait calls where the GPU may still be executing.
        unsafe {
            self.list.Close()?;
            self.pending = Some(pending);
            self.queue.ExecuteCommandLists(&[Some(list)]);
            self.queue.Signal(&self.fence, self.tick)?;
            self.fence
                .SetEventOnCompletion(self.tick, HANDLE::default())?;
            if self.fence.GetCompletedValue() != self.tick {
                return Err(E_FAIL.into());
            }
        }
        Ok(self.pending.take().unwrap().result)
    }

    /// Preserve the address baked into an already recorded guest command list.
    pub(super) fn replace(&mut self, dest: &ID3D12Resource, src: ID3D12Resource) -> Result<()> {
        if unsafe { dest.GetDesc().Width < src.GetDesc().Width } {
            return Err(windows::Win32::Foundation::E_NOTIMPL.into());
        }
        self.begin()?;
        unsafe {
            self.list.CopyRaytracingAccelerationStructure(
                dest.GetGPUVirtualAddress(),
                src.GetGPUVirtualAddress(),
                D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE,
            );
        }
        self.finish(Pending {
            _input: src,
            _scratch: None,
            result: dest.clone(),
        })?;
        Ok(())
    }

    pub(super) fn build(&mut self, triangles: &[Triangle]) -> Result<(ID3D12Resource, u64)> {
        if self.pending.is_some() {
            return Err(E_FAIL.into());
        }
        let (mesh, geometries) = prepare_geometry(&self.device, triangles)?;
        let inputs = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS {
            Type: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL,
            Flags: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE
                | D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION,
            NumDescs: u32::try_from(geometries.len()).map_err(|_| E_INVALIDARG)?,
            DescsLayout: D3D12_ELEMENTS_LAYOUT_ARRAY,
            Anonymous: D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS_0 {
                pGeometryDescs: geometries.as_ptr(),
            },
        };
        let mut sizing = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO::default();
        unsafe {
            self.device
                .GetRaytracingAccelerationStructurePrebuildInfo(&inputs, &mut sizing)
        };
        if sizing.ResultDataMaxSizeInBytes == 0 || sizing.ScratchDataSizeInBytes == 0 {
            return Err(E_FAIL.into());
        }
        let result = as_buffer(&self.device, sizing.ResultDataMaxSizeInBytes)?;
        let scratch = buffer(
            &self.device,
            sizing.ScratchDataSizeInBytes,
            D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        )?;
        let build = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC {
            Inputs: inputs,
            DestAccelerationStructureData: unsafe { result.GetGPUVirtualAddress() },
            ScratchAccelerationStructureData: unsafe { scratch.GetGPUVirtualAddress() },
            SourceAccelerationStructureData: 0,
        };
        let post = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC {
            DestBuffer: unsafe { self.sizes.GetGPUVirtualAddress() },
            InfoType: D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE,
        };
        self.begin()?;
        unsafe {
            self.list
                .BuildRaytracingAccelerationStructure(&build, Some(core::slice::from_ref(&post)));
            self.transition_sizes(
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            );
            self.list
                .CopyBufferRegion(&self.readback, 0, &self.sizes, 0, 8);
            self.transition_sizes(
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
        }
        let result = self.finish(Pending {
            _input: mesh,
            _scratch: Some(scratch),
            result,
        })?;
        let mut ptr = core::ptr::null_mut();
        let read_range = D3D12_RANGE { Begin: 0, End: 8 };
        unsafe { self.readback.Map(0, Some(&read_range), Some(&mut ptr)) }?;
        let compacted = unsafe { ptr.cast::<u64>().read_unaligned() };
        unsafe { self.readback.Unmap(0, Some(&D3D12_RANGE::default())) };
        if compacted == 0 || compacted > sizing.ResultDataMaxSizeInBytes {
            return Err(E_FAIL.into());
        }
        Ok((result, compacted))
    }
}

impl Drop for Builder {
    fn drop(&mut self) {
        if self.pending.is_some() {
            unsafe {
                if self.fence.GetCompletedValue() < self.tick
                    && self.queue.Signal(&self.fence, self.tick).is_ok()
                {
                    let _ = self
                        .fence
                        .SetEventOnCompletion(self.tick, HANDLE::default());
                }
            }
        }
    }
}
