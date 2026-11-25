//! Extra inputs for inline queries that need the hit triangle's object-space normal.
//! Native acceleration structures are opaque. The geometry and primitive indices
//! used here match the data passed to the host builder, including inactive slots.
use super::{Buffer, Buffers, Kind, guest, lock, rebuild};
use crate::commands::serialized_blas::Triangle;
use std::{
    collections::BTreeMap,
    sync::{Arc, Weak},
};
use windows::Win32::{
    Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY},
    Graphics::Direct3D12::*,
};
use windows::core::{IUnknown, Result};

pub(super) type Normals = Arc<Vec<Vec<[f32; 3]>>>;

fn same_normals(a: &Normals, b: &Normals) -> bool {
    Arc::ptr_eq(a, b)
        || (a.len() == b.len()
            && a.iter().zip(b.iter()).all(|(left, right)| {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|(&x, &y)| x.map(f32::to_bits) == y.map(f32::to_bits))
            }))
}
#[derive(Clone)]
enum Contents {
    Bottom(Normals),
    Top(ID3D12Resource),
}
struct Entry {
    owner: Weak<Buffer>,
    contents: Contents,
}

#[derive(Default)]
pub(super) struct Geometry {
    entries: BTreeMap<u64, Entry>,
    directory: Option<ID3D12Resource>,
}

pub(super) fn normal([a, b, c]: [[f32; 3]; 3]) -> [f32; 3] {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    [
        u[1] * v[2] - u[2] * v[1],
        u[2] * v[0] - u[0] * v[2],
        u[0] * v[1] - u[1] * v[0],
    ]
}

pub(super) fn decoded(triangles: &[Triangle]) -> Vec<Vec<[f32; 3]>> {
    triangles
        .chunk_by(|a, b| a.geometry == b.geometry)
        .map(|group| group.iter().map(|t| normal(t.vertices)).collect())
        .collect()
}

impl Geometry {
    fn set(&mut self, pool: &Arc<Buffer>, native: u64, contents: Option<Contents>) {
        if let (Some(current), Some(Contents::Bottom(next))) =
            (self.entries.get(&native), contents.as_ref())
            && current.owner.ptr_eq(&Arc::downgrade(pool))
            && let Contents::Bottom(previous) = &current.contents
            && same_normals(previous, next)
        {
            return;
        }
        let old = self.entries.remove(&native);
        if matches!(contents, Some(Contents::Bottom(_)))
            || matches!(old.map(|e| e.contents), Some(Contents::Bottom(_)))
        {
            self.directory = None;
        }
        if let Some(contents) = contents {
            self.entries.insert(
                native,
                Entry {
                    owner: Arc::downgrade(pool),
                    contents,
                },
            );
        }
    }

    pub(super) fn release(&mut self, pool: &Arc<Buffer>) {
        let owner = Arc::downgrade(pool);
        let mut dirty = false;
        self.entries.retain(|_, e| {
            let remove = e.owner.ptr_eq(&owner);
            dirty |= remove && matches!(e.contents, Contents::Bottom(_));
            !remove
        });
        if dirty {
            self.directory = None;
        }
    }

    fn directory(&mut self, device: &ID3D12Device) -> Result<ID3D12Resource> {
        if let Some(resource) = &self.directory {
            return Ok(resource.clone());
        }
        let entries = self
            .entries
            .iter()
            .filter_map(|(&va, e)| match &e.contents {
                Contents::Bottom(normals) => Some((va, normals)),
                _ => None,
            });
        let blas_count = entries.clone().count();
        let header_words = blas_count
            .checked_add(1)
            .and_then(|n| n.checked_mul(4))
            .ok_or(E_INVALIDARG)?;
        let mut words = Vec::new();
        words
            .try_reserve_exact(header_words)
            .map_err(|_| E_OUTOFMEMORY)?;
        words.resize(header_words, 0u32);
        // Clone/compact destinations share the same ordered triangles as their source.
        // Their native VAs get distinct keys, but only one copy of the triangle data.
        let mut geometry_offsets = BTreeMap::new();
        let offset = |words: &[u32]| {
            u32::try_from(words.len())
                .ok()
                .and_then(|n| n.checked_mul(4))
                .ok_or(E_INVALIDARG)
        };
        words[0] = u32::try_from(blas_count).map_err(|_| E_INVALIDARG)?;
        for (i, (va, geometries)) in entries.enumerate() {
            let identity = Arc::as_ptr(geometries);
            let previous = geometry_offsets.get(&identity).copied();
            let table_offset = previous.unwrap_or(offset(&words)?);
            let header = [
                va as u32,
                (va >> 32) as u32,
                table_offset,
                u32::try_from(geometries.len()).map_err(|_| E_INVALIDARG)?,
            ];
            words[4 + i * 4..8 + i * 4].copy_from_slice(&header);
            if previous.is_some() {
                continue;
            }
            geometry_offsets.insert(identity, table_offset);
            let table = words.len();
            let slots = geometries.len().checked_mul(2).ok_or(E_INVALIDARG)?;
            words.try_reserve(slots).map_err(|_| E_OUTOFMEMORY)?;
            words.resize(table + slots, 0);
            for (j, normals) in geometries.iter().enumerate() {
                words[table + j * 2] = offset(&words)?;
                words[table + j * 2 + 1] =
                    u32::try_from(normals.len()).map_err(|_| E_INVALIDARG)?;
                words
                    .try_reserve(normals.len().checked_mul(3).ok_or(E_INVALIDARG)?)
                    .map_err(|_| E_OUTOFMEMORY)?;
                for n in normals {
                    words.extend(n.map(f32::to_bits));
                }
            }
        }
        let bytes = offset(&words)?;
        words[1] = bytes;
        let buffer = rebuild::upload_buffer(device, u64::from(bytes))?;
        let mut data = core::ptr::null_mut();
        unsafe {
            buffer.Map(0, Some(&D3D12_RANGE::default()), Some(&mut data))?;
            core::ptr::copy_nonoverlapping(words.as_ptr(), data.cast(), words.len());
            buffer.Unmap(0, None);
        }
        self.directory = Some(buffer.clone());
        Ok(buffer)
    }
}

pub(super) fn set_decoded(registry: &Buffers, pool: &Arc<Buffer>, native: u64, normals: &Normals) {
    lock(&registry.geometry).set(pool, native, Some(Contents::Bottom(normals.clone())));
}

/// The list's controlling guest device owns both addresses.
pub(crate) unsafe fn set_inline_instances(
    owner: &IUnknown,
    guest_va: u64,
    native: u64,
    instances: Option<ID3D12Resource>,
) {
    let registry = &unsafe { guest(owner) }.state.blas;
    if let Ok(pool) = registry.find(guest_va, Kind::Structure) {
        lock(&registry.geometry).set(&pool, native, instances.map(Contents::Top));
    }
}

pub(crate) struct NativeBuild {
    guest: u64,
    native: u64,
    inputs: Option<super::native_normals::Inputs>,
}

impl NativeBuild {
    pub(crate) unsafe fn record(
        owner: &IUnknown,
        guest: u64,
        native: u64,
        inputs: &D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS,
    ) -> Self {
        let build = Self {
            guest,
            native,
            inputs: unsafe { super::native_normals::inputs(inputs) }.ok(),
        };
        unsafe { build.prepare(owner) };
        build
    }

    pub(crate) unsafe fn prepare(&self, owner: &IUnknown) {
        let registry = &unsafe { guest(owner) }.state.blas;
        if let Ok(pool) = registry.find(self.guest, Kind::Structure) {
            // The native GPU build remains usable for other rays if these
            // optional normals cannot be read from a registered upload.
            let normals = self.inputs.as_ref().and_then(|i| i.capture(registry).ok());
            lock(&registry.geometry).set(
                &pool,
                self.native,
                normals.map(|n| Contents::Bottom(Arc::new(n))),
            );
        }
    }
}

pub(crate) unsafe fn copy_geometry(owner: &IUnknown, guest_dst: u64, dst: u64, src: u64) {
    let registry = &unsafe { guest(owner) }.state.blas;
    if let Ok(pool) = registry.find(guest_dst, Kind::Structure) {
        let mut geo = lock(&registry.geometry);
        let source = geo.entries.get(&src).map(|e| e.contents.clone());
        geo.set(&pool, dst, source);
    }
}

pub(crate) unsafe fn inline_geometry(
    owner: &IUnknown,
    native: u64,
) -> Result<[(ID3D12Resource, u64); 2]> {
    let device = unsafe { guest(owner) };
    let mut geo = lock(&device.state.blas.geometry);
    let Some(Contents::Top(instances)) = geo.entries.get(&native).map(|e| &e.contents) else {
        return Err(E_NOTIMPL.into());
    };
    let instances = instances.clone();
    let directory = geo.directory(&device.native)?;
    Ok([instances, directory].map(|r| {
        let va = unsafe { r.GetGPUVirtualAddress() };
        (r, va)
    }))
}
