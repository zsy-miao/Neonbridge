//! Capture the object-space face normals of a native build from its upload inputs.
use super::{Buffers, Kind, Upload, inline_geometry::normal};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::Result;

fn read(registry: &Buffers, address: u64, size: usize) -> Result<Vec<u8>> {
    // A DEFAULT buffer may only be accessible on the GPU. Its current contents
    // cannot be inferred from the last CPU upload to that address.
    let buffer = registry
        .find(address, Kind::Upload)
        .map_err(|_| E_NOTIMPL)?;
    let offset = usize::try_from(address - buffer.start).map_err(|_| E_INVALIDARG)?;
    Upload { buffer, offset }.bytes(size)
}

fn read_f32(bytes: &[u8]) -> f32 {
    f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn triangle_normals(
    registry: &Buffers,
    triangles: &D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC,
) -> Result<Vec<[f32; 3]>> {
    let index_size = match (triangles.IndexFormat, triangles.IndexBuffer) {
        (DXGI_FORMAT_UNKNOWN, 0) if triangles.IndexCount == 0 => 0,
        (DXGI_FORMAT_R16_UINT, _) => 2,
        (DXGI_FORMAT_R32_UINT, _) => 4,
        _ => return Err(E_NOTIMPL.into()),
    };
    let count = if index_size == 0 {
        triangles.VertexCount
    } else {
        triangles.IndexCount
    } as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    if !count.is_multiple_of(3) || triangles.VertexCount == 0 {
        return Err(E_INVALIDARG.into());
    }
    let vertex_size = match triangles.VertexFormat {
        DXGI_FORMAT_R32G32B32_FLOAT => 12,
        DXGI_FORMAT_R32G32_FLOAT => 8,
        _ => return Err(E_NOTIMPL.into()),
    };
    let stride = usize::try_from(triangles.VertexBuffer.StrideInBytes).map_err(|_| E_INVALIDARG)?;
    let size = (triangles.VertexCount as usize - 1)
        .checked_mul(stride)
        .and_then(|offset| offset.checked_add(vertex_size))
        .ok_or(E_INVALIDARG)?;
    let vertices = read(registry, triangles.VertexBuffer.StartAddress, size)?;
    let indices = if index_size == 0 {
        Vec::new()
    } else {
        read(
            registry,
            triangles.IndexBuffer,
            count.checked_mul(index_size).ok_or(E_INVALIDARG)?,
        )?
    };
    let transform = if triangles.Transform3x4 == 0 {
        None
    } else {
        let bytes = read(registry, triangles.Transform3x4, 48)?;
        Some(core::array::from_fn::<_, 12, _>(|i| {
            read_f32(&bytes[4 * i..])
        }))
    };

    let mut normals = Vec::new();
    normals
        .try_reserve_exact(count / 3)
        .map_err(|_| E_OUTOFMEMORY)?;
    for first in (0..count).step_by(3) {
        let mut points = [[0.; 3]; 3];
        for (corner, point) in points.iter_mut().enumerate() {
            let index = first + corner;
            let address = index * index_size;
            let vertex = match index_size {
                0 => index,
                2 => u16::from_le_bytes([indices[address], indices[address + 1]]) as usize,
                _ => u32::from_le_bytes([
                    indices[address],
                    indices[address + 1],
                    indices[address + 2],
                    indices[address + 3],
                ]) as usize,
            };
            if vertex >= triangles.VertexCount as usize {
                return Err(E_INVALIDARG.into());
            }
            let bytes = &vertices[vertex * stride..];
            let x = read_f32(bytes);
            let y = read_f32(&bytes[4..]);
            let z = if vertex_size == 12 {
                read_f32(&bytes[8..])
            } else {
                0.
            };
            *point = if let Some(m) = transform {
                core::array::from_fn(|i| {
                    let row = &m[4 * i..];
                    row[0] * x + row[1] * y + row[2] * z + row[3]
                })
            } else {
                [x, y, z]
            };
        }
        normals.push(normal(points));
    }
    Ok(normals)
}

// D3D12 borrows the geometry descriptor array only during recording. Retain
// its scalar fields so the GPU addresses can be read again on every submission.
pub(super) struct Inputs(Vec<Option<D3D12_RAYTRACING_GEOMETRY_TRIANGLES_DESC>>);

impl Inputs {
    pub(super) fn capture(&self, registry: &Buffers) -> Result<Vec<Vec<[f32; 3]>>> {
        self.0
            .iter()
            .map(|g| match g {
                Some(triangles) => triangle_normals(registry, triangles),
                None => Ok(Vec::new()),
            })
            .collect()
    }
}

/// # Safety
/// The caller supplies live native geometry descriptors in the declared array
/// layout, as required for the D3D12 build call.
pub(super) unsafe fn inputs(
    inputs: &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS,
) -> Result<Inputs> {
    if inputs.Type != D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL {
        return Err(E_INVALIDARG.into());
    }
    let mut geometries = Vec::new();
    geometries
        .try_reserve_exact(inputs.NumDescs as usize)
        .map_err(|_| E_OUTOFMEMORY)?;
    for index in 0..inputs.NumDescs as usize {
        // SAFETY: the two layouts select the active union member and the caller
        // supplies NumDescs contiguous descriptors or descriptor pointers.
        let geometry = unsafe {
            match inputs.DescsLayout {
                D3D12_ELEMENTS_LAYOUT_ARRAY if !inputs.Anonymous.pGeometryDescs.is_null() => {
                    &*inputs.Anonymous.pGeometryDescs.add(index)
                }
                D3D12_ELEMENTS_LAYOUT_ARRAY_OF_POINTERS
                    if !inputs.Anonymous.ppGeometryDescs.is_null() =>
                {
                    (*inputs.Anonymous.ppGeometryDescs.add(index))
                        .as_ref()
                        .ok_or(E_INVALIDARG)?
                }
                _ => return Err(E_INVALIDARG.into()),
            }
        };
        match geometry.Type {
            D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES => {
                // SAFETY: the descriptor's discriminant selects Triangles.
                geometries.push(Some(unsafe { geometry.Anonymous.Triangles }));
            }
            // The inline query skips procedural primitives, but indices of
            // subsequent triangle geometries still include this geometry.
            D3D12_RAYTRACING_GEOMETRY_TYPE_PROCEDURAL_PRIMITIVE_AABBS => geometries.push(None),
            _ => return Err(E_NOTIMPL.into()),
        }
    }
    Ok(Inputs(geometries))
}
