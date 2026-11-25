//! Mapped upload buffers and host storage for Xbox acceleration structures.
use super::{Device, shader_cache::source_digest};
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, Once, Weak};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
use windows::core::{IUnknown, Interface, Result};

mod inline_geometry;
pub(crate) mod instance_fixup;
mod native_normals;
mod scene_view;
pub(crate) use inline_geometry::{
    NativeBuild, copy_geometry, inline_geometry, set_inline_instances,
};
pub(super) use scene_view::SceneView;
pub(crate) use scene_view::{inline_scene, inline_scene_address};
mod rebuild;
pub(super) use rebuild::upload_buffer;

type Index = Mutex<BTreeMap<(Kind, u64), Vec<Arc<Buffer>>>>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(PartialOrd, Ord, PartialEq, Eq, Clone, Copy)]
enum Kind {
    Upload,
    Structure,
    Default,
}

struct Buffer {
    native: ID3D12Resource,
    start: u64,
    bytes: u64,
    kind: Kind,
    // Recording chooses a backing for new commands. Submission publishes the
    // address captured by each write in guest execution order.
    targets: Mutex<BTreeMap<u64, Option<ID3D12Resource>>>,
    // Upper bounds for the most recently submitted writes to each suballocation.
    sizes: Mutex<BTreeMap<u64, u64>>,
    mappings: Arc<Mutex<Mappings>>,
}

#[derive(Default)]
struct Mappings {
    addresses: BTreeMap<u64, u64>,
    // Only addresses changed while preparing the current queue batch.
    previous: BTreeMap<u64, Option<u64>>,
    previous_sizes: Vec<(Weak<Buffer>, u64, Option<u64>)>,
    snapshot: Option<Arc<[(u64, u64)]>>,
    table: Option<instance_fixup::Table>,
}

impl Mappings {
    fn dirty(&mut self) {
        self.snapshot = None;
        self.table = None;
    }

    fn snapshot(&mut self) -> Arc<[(u64, u64)]> {
        self.snapshot
            .get_or_insert_with(|| {
                self.addresses
                    .iter()
                    .map(|(&guest, &native)| (guest, native))
                    .collect()
            })
            .clone()
    }
}

pub(super) struct Registration {
    buffer: Arc<Buffer>,
    index: Weak<Index>,
    geometry: Weak<Mutex<inline_geometry::Geometry>>,
}

pub(crate) fn is_structure(resource: &super::Resource) -> bool {
    resource
        .state
        .blas_buffer
        .as_ref()
        .is_some_and(|r| r.buffer.kind == Kind::Structure)
}

impl Drop for Registration {
    fn drop(&mut self) {
        if self.buffer.kind == Kind::Structure {
            if let Some(geo) = self.geometry.upgrade() {
                lock(&geo).release(&self.buffer);
            }
            let targets = lock(&self.buffer.targets);
            let mut mappings = lock(&self.buffer.mappings);
            for (&address, backing) in targets.iter() {
                mappings.previous.remove(&address);
                if backing.is_some() && mappings.addresses.remove(&address).is_some() {
                    mappings.dirty();
                }
            }
        }
        if let Some(index) = self.index.upgrade() {
            let mut index = lock(&index);
            if let Some(entries) = index.get_mut(&(self.buffer.kind, self.buffer.start)) {
                entries.retain(|b| !Arc::ptr_eq(b, &self.buffer));
                if entries.is_empty() {
                    index.remove(&(self.buffer.kind, self.buffer.start));
                }
            }
        }
    }
}

#[derive(Default)]
pub(super) struct Buffers {
    pub(super) views: Arc<super::placement::DescriptorMap<SceneView>>,
    index: Arc<Index>,
    geometry: Arc<Mutex<inline_geometry::Geometry>>,
    builder: Mutex<Option<rebuild::Builder>>,
    fixup: Mutex<Option<instance_fixup::Kernel>>,
    empty_instances: Mutex<Option<ID3D12Resource>>,
    mappings: Arc<Mutex<Mappings>>,
    submission: Mutex<()>,
}

impl Buffers {
    pub(super) fn register(
        &self,
        resource: &ID3D12Resource,
        heap: D3D12_HEAP_TYPE,
        desc: &D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES,
    ) -> Option<Registration> {
        if desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER {
            return None;
        }
        let kind = if initial == D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE {
            Kind::Structure
        } else if heap == D3D12_HEAP_TYPE_UPLOAD {
            Kind::Upload
        } else if heap == D3D12_HEAP_TYPE_DEFAULT {
            Kind::Default
        } else {
            return None;
        };
        let start = unsafe { resource.GetGPUVirtualAddress() };
        let buffer = Arc::new(Buffer {
            native: resource.clone(),
            start,
            bytes: desc.Width,
            kind,
            targets: Mutex::default(),
            sizes: Mutex::default(),
            mappings: self.mappings.clone(),
        });
        lock(&self.index)
            .entry((kind, start))
            .or_default()
            .push(buffer.clone());
        Some(Registration {
            buffer,
            index: Arc::downgrade(&self.index),
            geometry: Arc::downgrade(&self.geometry),
        })
    }

    fn find(&self, address: u64, kind: Kind) -> Result<Arc<Buffer>> {
        let index = lock(&self.index);
        let mut candidates = index
            .range((kind, 0)..=(kind, address))
            .rev()
            .flat_map(|(_, group)| group)
            .filter(|buf| address - buf.start < buf.bytes);
        let result = candidates.next().cloned().ok_or(E_INVALIDARG)?;
        if candidates.next().is_some() {
            return Err(E_NOTIMPL.into()); // ambiguous overlapping placed resources
        }
        Ok(result)
    }
}

/// # Safety
/// The controlling unknown must belong to a live guest device.
unsafe fn guest(owner: &IUnknown) -> &Device {
    unsafe { &*owner.as_raw().cast::<Device>() }
}

/// # Safety
/// The owner must identify the controlling guest device.
pub(crate) unsafe fn instance_buffer(owner: &IUnknown, bytes: u64) -> Result<ID3D12Resource> {
    rebuild::upload_buffer(&unsafe { guest(owner) }.native, bytes)
}

/// Empty TLASes still need a valid root SRV; no ray can index an instance.
/// Committed upload memory starts zeroed and is read-only to every list.
///
/// # Safety
/// The owner must be the live controlling guest device.
pub(crate) unsafe fn empty_instance_buffer(owner: &IUnknown) -> Result<ID3D12Resource> {
    let device = unsafe { guest(owner) };
    let mut empty = lock(&device.state.blas.empty_instances);
    if let Some(buffer) = empty.as_ref() {
        return Ok(buffer.clone());
    }
    let bytes = size_of::<D3D12_RAYTRACING_INSTANCE_DESC>() as u64;
    let buffer = rebuild::upload_buffer(&device.native, bytes)?;
    *empty = Some(buffer.clone());
    Ok(buffer)
}

/// # Safety
/// The owner must identify the controlling guest device.
pub(crate) unsafe fn has_relocations(owner: &IUnknown) -> bool {
    !lock(&unsafe { guest(owner) }.state.blas.mappings)
        .addresses
        .is_empty()
}

/// # Safety
/// The owner must be the list's controlling TuxBox device. Addresses map only
/// while the original guest AS resource is alive.
pub(crate) unsafe fn relocated(owner: &IUnknown) -> Arc<[(u64, u64)]> {
    lock(&unsafe { guest(owner) }.state.blas.mappings).snapshot()
}

/// Return the address visible to commands at this point in submission order.
/// The owner must be the controlling guest device.
pub(crate) unsafe fn submitted_address(owner: &IUnknown, address: u64) -> u64 {
    lock(&unsafe { guest(owner) }.state.blas.mappings)
        .addresses
        .get(&address)
        .copied()
        .unwrap_or(address)
}

pub(crate) struct MappingSubmission<'a> {
    mappings: &'a Mutex<Mappings>,
    _submission: MutexGuard<'a, ()>,
}

/// Serialize address publication in the same order as host queue submissions.
/// The owner must remain the controlling guest device for the guard's lifetime.
pub(crate) unsafe fn begin_submission(owner: &IUnknown) -> MappingSubmission<'_> {
    let registry = &unsafe { guest(owner) }.state.blas;
    MappingSubmission {
        mappings: &registry.mappings,
        _submission: lock(&registry.submission),
    }
}

impl MappingSubmission<'_> {
    pub(crate) fn commit(self) {
        let mut maps = lock(self.mappings);
        maps.previous.clear();
        maps.previous_sizes.clear();
    }
}

impl Drop for MappingSubmission<'_> {
    fn drop(&mut self) {
        let mut maps = lock(self.mappings);
        for (pool, address, old) in maps.previous_sizes.drain(..).rev() {
            if let Some(pool) = pool.upgrade() {
                let mut sizes = lock(&pool.sizes);
                if let Some(bytes) = old {
                    sizes.insert(address, bytes);
                } else {
                    sizes.remove(&address);
                }
            }
        }
        if !maps.previous.is_empty() {
            for (address, old) in std::mem::take(&mut maps.previous) {
                if let Some(native) = old {
                    maps.addresses.insert(address, native);
                } else {
                    maps.addresses.remove(&address);
                }
            }
            maps.dirty();
        }
    }
}

/// Publish the address written by a recorded AS command, including an older
/// list replayed after the same guest location was promoted to private storage.
/// The owner must be the controlling guest device.
pub(crate) unsafe fn publish_destination(
    owner: &IUnknown,
    address: u64,
    native: u64,
    bytes: Option<u64>,
) {
    let registry = &unsafe { guest(owner) }.state.blas;
    let pool = registry.find(address, Kind::Structure).ok();
    let mut mappings = lock(&registry.mappings);
    if let Some(pool) = pool {
        let mut sizes = lock(&pool.sizes);
        let old = sizes.get(&address).copied();
        if old != bytes {
            mappings
                .previous_sizes
                .push((Arc::downgrade(&pool), address, old));
            if let Some(bytes) = bytes {
                sizes.insert(address, bytes);
            } else {
                sizes.remove(&address);
            }
        }
    }
    let old = mappings.addresses.get(&address).copied();
    if old.unwrap_or(address) == native {
        return;
    }
    mappings.previous.entry(address).or_insert(old);
    if address == native {
        mappings.addresses.remove(&address);
    } else {
        mappings.addresses.insert(address, native);
    }
    mappings.dirty();
}

/// # Safety
/// The owner must identify the list's controlling TuxBox device.
pub(crate) unsafe fn resolve(owner: &IUnknown, address: u64) -> Result<Option<ID3D12Resource>> {
    let buffer = match unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Structure)
    {
        Ok(buffer) => buffer,
        // An unregistered address can be a native resource owned outside the
        // guest provider. Overlapping registered resources are different: the
        // backing cannot be chosen safely, so preserve the ambiguity error.
        Err(error) if error.code() == E_INVALIDARG => return Ok(None),
        Err(error) => return Err(error),
    };
    let result = lock(&buffer.targets).get(&address).and_then(Clone::clone);
    Ok(result)
}

struct Upload {
    buffer: Arc<Buffer>,
    offset: usize,
}

impl Upload {
    fn bytes(&self, len: usize) -> Result<Vec<u8>> {
        let end = self
            .offset
            .checked_add(len)
            .filter(|&v| (v as u64) <= self.buffer.bytes)
            .ok_or(E_INVALIDARG)?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| E_OUTOFMEMORY)?;
        bytes.resize(len, 0);
        let range = D3D12_RANGE {
            Begin: self.offset,
            End: end,
        };
        let mut ptr = core::ptr::null_mut();
        // SAFETY: restrict the mapping to the validated suballocation extent. Nested
        // Map/Unmap preserves existing guest uploads and their flush ranges.
        unsafe { self.buffer.native.Map(0, Some(&range), Some(&mut ptr)) }?;
        let mut copied = 0;
        let copied_ok = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                ptr.cast::<u8>().wrapping_add(self.offset).cast(),
                bytes.as_mut_ptr().cast(),
                len,
                Some(&mut copied),
            )
        };
        unsafe { self.buffer.native.Unmap(0, Some(&D3D12_RANGE::default())) };
        copied_ok?;
        if copied != len {
            return Err(E_INVALIDARG.into());
        }
        Ok(bytes)
    }
}

impl Upload {
    fn snapshot(&self, guest_capacity: u64) -> Result<(Vec<u8>, u64)> {
        let header = self.bytes(64)?;
        let length = usize::try_from(u64::from_le_bytes(header[32..40].try_into().unwrap()))
            .map_err(|_| E_INVALIDARG)?;
        let guest_bytes = u64::from_le_bytes(header[40..48].try_into().unwrap());
        if length < 64
            || guest_bytes.checked_add(64) != Some(length as u64)
            || guest_bytes > guest_capacity
        {
            return Err(E_INVALIDARG.into());
        }
        let bytes = self.bytes(length)?;
        if bytes[..64] != header {
            return Err(E_INVALIDARG.into());
        }
        Ok((bytes, guest_bytes))
    }
}

pub(crate) struct Input {
    upload: Upload,
    snapshot: Mutex<(Vec<u8>, inline_geometry::Normals, u64)>,
    source: ID3D12Resource,
    guest_destination: u64,
    native_destination: u64,
    guest_capacity: u64,
    host_capacity: u64,
}

impl Input {
    /// Refresh the native source when the title rewrites a recorded upload
    /// after recording. The guest observes the contents at queue submission.
    /// # Safety
    /// The owner must be the controlling device that recorded the list.
    pub(crate) unsafe fn prepare(&self, owner: &IUnknown) -> Result<()> {
        let mut previous = lock(&self.snapshot);
        let (bytes, _) = self.upload.snapshot(self.guest_capacity)?;
        let registry = &unsafe { guest(owner) }.state.blas;
        let pool = registry.find(self.guest_destination, Kind::Structure)?;
        if previous.0 == bytes {
            unsafe {
                publish_destination(
                    owner,
                    self.guest_destination,
                    self.native_destination,
                    Some(previous.2),
                )
            };
            inline_geometry::set_decoded(registry, &pool, self.native_destination, &previous.1);
            return Ok(());
        }
        let src = self.upload.buffer.start + self.upload.offset as u64;
        let dst = self.guest_destination;
        let triangles =
            crate::commands::serialized_blas::decode_for_build(&bytes).map_err(|e| {
                failure(src, dst, "updated triangle decoder", e, Some(&bytes));
                E_NOTIMPL
            })?;
        let mut builder_guard = lock(&unsafe { guest(owner) }.state.blas.builder);
        let builder = builder_guard.as_mut().ok_or(E_INVALIDARG)?;
        let (updated, compacted) = builder
            .build(&triangles)
            .inspect_err(|e| failure(src, dst, "updated native build", e.code(), Some(&bytes)))?;
        if compacted > self.host_capacity {
            failure(
                src,
                dst,
                "updated compacted size exceeds backing",
                compacted,
                Some(&bytes),
            );
            return Err(E_NOTIMPL.into());
        }
        builder.replace(&self.source, updated).inspect_err(|e| {
            failure(
                src,
                dst,
                "updated source replacement",
                e.code(),
                Some(&bytes),
            )
        })?;
        drop(builder_guard);
        let normals = Arc::new(inline_geometry::decoded(&triangles));
        unsafe {
            publish_destination(
                owner,
                self.guest_destination,
                self.native_destination,
                Some(compacted),
            )
        };
        inline_geometry::set_decoded(registry, &pool, self.native_destination, &normals);
        *previous = (bytes, normals, compacted);
        Ok(())
    }
}

pub(crate) struct Rebuilt {
    pub input: Input,
    pub source: ID3D12Resource,
    pub destination: ID3D12Resource,
    pub destination_address: u64,
}

// Failed inputs are bounded, opt-in diagnostics for examining additional console
// geometry layouts. A trace alone never writes title data to disk.
fn failure(src: u64, dst: u64, stage: &str, why: impl std::fmt::Debug, bytes: Option<&[u8]>) {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    static CAPTURE_BYTES: Mutex<usize> = Mutex::new(0);
    static TRACE: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_TRACE_RAYTRACING").as_deref() == Ok("1"));
    static CAPTURE: LazyLock<Option<std::path::PathBuf>> =
        LazyLock::new(|| std::env::var_os("TUXBOX_CAPTURE_BLAS").map(Into::into));
    if (!*TRACE && CAPTURE.is_none()) || COUNT.fetch_add(1, Ordering::Relaxed) >= 64 {
        return;
    }
    let digest = bytes.and_then(source_digest);
    crate::diagnostics::log_once(&Once::new(), || {
        format!(
            "Xbox BLAS {stage} failed {why:?}, source {src:#x}, destination {dst:#x}, bytes {}, SHA-256 {}",
            bytes.map_or(0, <[u8]>::len),
            digest.as_deref().unwrap_or("unavailable")
        )
    });
    if let (Some(folder), Some(bytes), Some(digest)) = (&*CAPTURE, bytes, digest) {
        let mut written = lock(&CAPTURE_BYTES);
        if bytes.len() <= 256 * 1024 * 1024 - *written
            && let Ok(mut file) = std::fs::File::create_new(folder.join(format!("{digest}.blas")))
        {
            *written += bytes.len();
            if let Err(error) = file.write_all(bytes) {
                crate::diagnostics::log_once(&Once::new(), || {
                    format!("Xbox BLAS capture failed: {error}")
                });
            }
        }
    }
}

/// # Safety
/// The owner must identify the controlling guest device.
pub(crate) unsafe fn deserialize(owner: &IUnknown, src: u64, dst: u64) -> Result<Rebuilt> {
    let device = unsafe { guest(owner) };
    let registry = &device.state.blas;
    let buffer = registry
        .find(src, Kind::Upload)
        .inspect_err(|e| failure(src, dst, "source lookup", e.code(), None))?;
    let upload = Upload {
        offset: (src - buffer.start) as usize,
        buffer,
    };
    let buffer = registry
        .find(dst, Kind::Structure)
        .inspect_err(|e| failure(src, dst, "destination lookup", e.code(), None))?;
    let (snapshot, guest_bytes) = upload
        .snapshot(buffer.bytes - (dst - buffer.start))
        .inspect_err(|e| failure(src, dst, "upload header", e.code(), None))?;
    let triangles = crate::commands::serialized_blas::decode_for_build(&snapshot).map_err(|e| {
        failure(src, dst, "triangle decoder", e, Some(&snapshot));
        E_NOTIMPL
    })?;
    let mut builder = lock(&registry.builder);
    if builder.is_none() {
        *builder = Some(rebuild::Builder::new(&device.native)?);
    }
    let (source, compacted) = builder
        .as_mut()
        .unwrap()
        .build(&triangles)
        .inspect_err(|e| failure(src, dst, "native build", e.code(), Some(&snapshot)))?;
    drop(builder);
    let (destination, destination_address) =
        target(&device.native, &buffer, dst, guest_bytes, compacted)
            .inspect_err(|e| failure(src, dst, "placement", e.code(), Some(&snapshot)))?;
    static TRACE: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_TRACE_RAYTRACING").as_deref() == Ok("1"));
    static FIRST: Once = Once::new();
    if *TRACE {
        crate::diagnostics::log_once(&FIRST, || {
            format!(
                "rebuilt Xbox BLAS {} at {dst:#x}: {} triangles, {guest_bytes} guest bytes, {compacted} compacted host bytes, host VA {destination_address:#x}",
                source_digest(&snapshot).unwrap_or_default(),
                triangles.len()
            )
        });
    }
    let normals = Arc::new(inline_geometry::decoded(&triangles));
    inline_geometry::set_decoded(registry, &buffer, destination_address, &normals);
    Ok(Rebuilt {
        input: Input {
            upload,
            snapshot: Mutex::new((snapshot, normals, compacted)),
            source: source.clone(),
            guest_destination: dst,
            native_destination: destination_address,
            guest_capacity: guest_bytes,
            host_capacity: guest_bytes.max(compacted),
        },
        source,
        destination,
        destination_address,
    })
}

fn target(
    device: &ID3D12Device,
    buffer: &Buffer,
    address: u64,
    guest_bytes: u64,
    bytes: u64,
) -> Result<(ID3D12Resource, u64)> {
    let mut targets = lock(&buffer.targets);
    if let Some(Some(existing)) = targets.get(&address) {
        let capacity = unsafe { existing.GetDesc() }.Width;
        if bytes > capacity {
            crate::diagnostics::recording_error(format_args!(
                "reused BLAS at {address:#x} needs {bytes} host bytes, backing holds {capacity}"
            ));
            return Err(E_NOTIMPL.into());
        }
        // A pool address can be repopulated with a different cooked mesh.
        // Keep its native address stable for previously recorded consumers.
        return Ok((existing.clone(), unsafe { existing.GetGPUVirtualAddress() }));
    }
    if bytes <= guest_bytes {
        targets.insert(address, None);
        return Ok((buffer.native.clone(), address));
    }
    // A committed buffer already consumes whole placement-alignment units.
    // Expose that space for later meshes reusing the same guest address.
    let capacity = bytes
        .checked_next_multiple_of(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT as u64)
        .ok_or(E_INVALIDARG)?;
    let resource = rebuild::as_buffer(device, capacity)?;
    let host = unsafe { resource.GetGPUVirtualAddress() };
    targets.insert(address, Some(resource.clone()));
    Ok((resource, host))
}

/// # Safety
/// The owner must identify the controlling guest device.
pub(crate) unsafe fn copy_dest(
    owner: &IUnknown,
    dst: u64,
    source: &ID3D12Resource,
) -> Result<(ID3D12Resource, u64)> {
    let device = unsafe { guest(owner) };
    let buffer = device.state.blas.find(dst, Kind::Structure)?;
    // A previously relocated address must keep serving recorded consumers.
    // New copies receive backing as large as the source backing, so replayed
    // builds at the source can safely change its contents.
    if let Some(backing) = lock(&buffer.targets).get(&dst).and_then(Clone::clone) {
        return Ok((backing.clone(), unsafe { backing.GetGPUVirtualAddress() }));
    }
    target(
        &device.native,
        &buffer,
        dst,
        0,
        unsafe { source.GetDesc() }.Width,
    )
}

/// Retain a direct source while a recorded copy references it. Unregistered
/// addresses give us neither a lifetime nor a safe bound for private storage.
pub(crate) unsafe fn direct_source(owner: &IUnknown, address: u64) -> Result<ID3D12Resource> {
    let pool = unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Structure)?;
    Ok(pool.native.clone())
}

/// Bound the copy against the submitted source. A registered native pool also
/// bounds an external writer even if this provider did not record its build.
pub(crate) unsafe fn submitted_size(owner: &IUnknown, address: u64, native: u64) -> Option<u64> {
    let pool = unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Structure)
        .ok()?;
    let size = lock(&pool.sizes).get(&address).copied();
    size.or_else(|| (address == native).then(|| pool.bytes - (address - pool.start)))
}

/// # Safety
/// The owner must identify the controlling guest device.
pub(crate) unsafe fn upload_range(
    owner: &IUnknown,
    address: u64,
) -> Result<(ID3D12Resource, u64, u64)> {
    let buffer = unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Upload)?;
    let offset = address - buffer.start;
    Ok((buffer.native.clone(), offset, buffer.bytes - offset))
}

/// The guest device owns the returned buffer. Report ambiguity separately from a
/// native address registered outside the guest provider.
pub(crate) unsafe fn default_instances(
    owner: &IUnknown,
    address: u64,
) -> Result<(ID3D12Resource, u64)> {
    let buffer = unsafe { guest(owner) }
        .state
        .blas
        .find(address, Kind::Default)?;
    Ok((
        buffer.native.clone(),
        buffer.bytes - (address - buffer.start),
    ))
}
