//! Keep CPU-backed tiled Xbox textures coherent with their desktop images.
mod descriptors;
mod publication;
mod upload;
mod writeback;
pub(crate) use descriptors::{DescriptorMap, DescriptorRange, HeapViews, PlacedView, Views};
pub(crate) use publication::QueueAccess;

use super::Device;
use core::{ffi::c_void, ops::Range};
use std::collections::{BTreeMap, btree_map::Entry};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use tuxbox_xg_layout::{Desc, Plane};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY, E_POINTER};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::System::{Diagnostics::Debug::ReadProcessMemory, Threading::GetCurrentProcess};
use windows::core::{HRESULT, Result};

pub(crate) fn supported_state(state: D3D12_RESOURCE_STATES, flags: D3D12_RESOURCE_FLAGS) -> bool {
    let allowed = D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE
        | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE
        | D3D12_RESOURCE_STATE_COPY_SOURCE;
    state == D3D12_RESOURCE_STATE_COPY_DEST
        || state == D3D12_RESOURCE_STATE_RENDER_TARGET && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET)
        || state.0 & !allowed.0 == 0
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

fn committed(
    device: &ID3D12Device,
    kind: D3D12_HEAP_TYPE,
    desc: &D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: Option<&D3D12_CLEAR_VALUE>,
) -> Result<ID3D12Resource> {
    let heap = D3D12_HEAP_PROPERTIES {
        Type: kind,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native = None;
    // SAFETY: D3D12 only borrows the input structures during creation.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            desc,
            initial,
            clear.map(core::ptr::from_ref),
            &mut native,
        )?;
    }
    native.ok_or_else(|| E_POINTER.into())
}

struct Mip {
    slice: u32,
    level: u32,
    source: Range<u64>,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    elements: u32,
    element_bytes: u32,
    rows: u32,
    bytes: u64,
}

#[derive(Default)]
struct Contents {
    // An exact copy makes writes to shared pages and all-zero rewrites observable.
    previous: Vec<Vec<u8>>,
    uploaded: u64,
    readers: Vec<(ID3D12Fence, u64)>,
}

struct Alias {
    desc: D3D12_RESOURCE_DESC,
    texture: Weak<Texture>,
}

#[derive(Default)]
struct Images(Mutex<BTreeMap<u64, Vec<Alias>>>);

pub(crate) struct Texture {
    pub(super) native: ID3D12Resource,
    pub(super) desc: D3D12_RESOURCE_DESC,
    pub(super) address: u64,
    plane: Option<Plane>,
    size: u64,
    mips: Vec<Mip>,
    contents: Mutex<Contents>,
    images: Weak<Images>,
}

impl Drop for Texture {
    fn drop(&mut self) {
        crate::exports::profiling::mapping_released();
        let Some(images) = self.images.upgrade() else {
            return;
        };
        if let Entry::Occupied(mut entry) = lock(&images.0).entry(self.address) {
            entry
                .get_mut()
                .retain(|alias| alias.texture.strong_count() != 0);
            if entry.get().is_empty() {
                entry.remove();
            }
        }
    }
}

// XG's linear NV12 surface contains full-height luma followed by half-height
// interleaved chroma. Both use the same 256-byte-aligned row pitch.
pub(super) fn nv12_pitch(desc: &D3D12_RESOURCE_DESC) -> Option<u64> {
    (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && desc.Format == windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_NV12
        && desc.Layout.0 == 0x100 && desc.Flags.0 == 0x300000
        && desc.Width > 0 && desc.Width <= 16384 && desc.Width.is_multiple_of(2)
        && desc.Height > 0 && desc.Height <= 16384 && desc.Height.is_multiple_of(2)
        && desc.DepthOrArraySize == 1 && desc.MipLevels == 1
        && desc.SampleDesc.Count == 1 && desc.SampleDesc.Quality == 0)
        .then(|| desc.Width.next_multiple_of(256))
}

pub(super) fn color_plane(desc: &D3D12_RESOURCE_DESC) -> Result<Plane> {
    let flags = if (desc.Layout.0 == 0x105 && desc.Flags.0 & !0x300004 == 0)
        || (desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x105
            && desc.Flags.0 & !0x300005 == 0 && !matches!(desc.Format.0, 70..=84 | 94..=99))
    {
        0
    } else {
        desc.Flags.0 as u32
    };
    let layout = Desc {
            dimension: desc.Dimension.0 as _,
            alignment: desc.Alignment,
            width: desc.Width,
            height: desc.Height,
            slices: desc.DepthOrArraySize,
            mips: desc.MipLevels,
            format: desc.Format.0 as _,
            samples: desc.SampleDesc.Count,
            quality: desc.SampleDesc.Quality,
            layout: desc.Layout.0 as _,
            flags,
        };
    if matches!(layout.layout, 257 | 258) {
        return Plane::compute_microtile(&layout).map_err(|status| HRESULT(status).into());
    }
    if layout.layout == 265 {
        // 64 KiB standard swizzle, as selected by XGCreateTextureComputer.
        return Plane::compute_sparse_64k(&layout).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE1D && desc.Layout.0 == 0x100
        && desc.Height == 1 && matches!(desc.Alignment, 0 | 256 | 4096 | 65536) {
        let linear = Desc { dimension: 3, alignment: 0, ..layout };
        return Plane::compute(&linear).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x100
        && desc.Alignment == 0 && desc.Flags.0 & !0x300004 == 0 && !matches!(desc.Format.0, 70..=84 | 94..=99)
        && desc.Width <= 2048 && desc.Height <= 2048 {
        // Same selection as XGCreateTextureComputer for a linear volume.
        let volume = Desc { dimension: 3, alignment: 0, flags: 0, ..layout };
        return Plane::compute_linear_volume(&volume).map_err(|status| HRESULT(status).into());
    }
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D && desc.Layout.0 == 0x105
        && matches!(desc.Alignment, 0 | 4096) && desc.Width <= 2048 && desc.Height <= 2048 {
        // The XG volume tile calculator takes a 2D base plane and its physical depth.
        let volume = Desc { dimension: 3, alignment: 0, ..layout };
        return Plane::compute_4k_volume(&volume).map_err(|status| HRESULT(status).into());
    }
    Plane::compute(&layout).or_else(|_| Plane::compute_64k_color(&layout)).map_err(|status| HRESULT(status).into())
}

impl Texture {
    /// Guest address of a subresource's texels. On the console Map returns this
    /// memory directly; the uploader publishes CPU writes before GPU reads.
    pub(crate) fn guest_pointer(&self, subresource: u32) -> Option<u64> {
        let mip = self.mips.get(subresource as usize)?;
        self.address.checked_add(mip.source.start)
    }

    pub(crate) fn flags(&self) -> D3D12_RESOURCE_FLAGS {
        self.desc.Flags
    }

    fn new(
        device: &ID3D12Device,
        images: &Arc<Images>,
        address: u64,
        desc: &D3D12_RESOURCE_DESC,
        clear: Option<&D3D12_CLEAR_VALUE>,
    ) -> Result<Arc<Self>> {
        let pitch = nv12_pitch(desc);
        let plane = if pitch.is_some() { None } else { Some(color_plane(desc)?) };
        let luma_bytes = pitch.unwrap_or_default() * u64::from(desc.Height);
        let volume = desc.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D;
        let (size, alignment, count) = plane.as_ref().map_or(
            (luma_bytes + luma_bytes / 2, 256, 2),
            |p| (p.size, p.alignment, if volume { p.mip_count() } else { p.mip_count() * p.array_size() }));
        if address == 0
            || !address.is_multiple_of(alignment)
            || !crate::exports::placement_fits(address, size)
        {
            static RANGE: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&RANGE, || format!(
                "placed texture range at {address:#x}: size={size:#x} alignment={alignment:#x} fits={} {}x{} format={} layout={:#x}",
                crate::exports::placement_fits(address, size), desc.Width, desc.Height, desc.Format.0, desc.Layout.0));
            return Err(E_INVALIDARG.into());
        }
        crate::diagnostics::watch("placed texture", address, size, || format!(
            "{}x{}x{} format={} layout={:#x} flags={:#x} mips={}", desc.Width, desc.Height,
            desc.DepthOrArraySize, desc.Format.0, desc.Layout.0, desc.Flags.0, desc.MipLevels));
        // A simultaneous-access texture implicitly promotes on first use and
        // decays to COMMON after ExecuteCommandLists, including copy-queue uploads.
        let host = D3D12_RESOURCE_DESC {
            Alignment: 0,
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_SIMULTANEOUS_ACCESS
                | (desc.Flags & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)),
            ..*desc
        };
        let native = committed(
            device,
            D3D12_HEAP_TYPE_DEFAULT,
            &host,
            D3D12_RESOURCE_STATE_COMMON,
            clear,
        )?;
        let mut mips = Vec::with_capacity(count as usize);
        for index in 0..count {
            let (slice, level, element_bytes, source) = if let Some(p) = &plane {
                let slice = index / p.mip_count();
                let level = index % p.mip_count();
                (slice, level, p.element_bytes(), if volume { 0..p.size } else { p.mip_range_in_slice(slice, level).ok_or(E_INVALIDARG)? })
            } else if index == 0 {
                (0, 0, 1, 0..luma_bytes)
            } else {
                (0, 0, 2, luma_bytes..size)
            };
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let (mut rows, mut row_bytes, mut bytes) = (0, 0, 0);
            unsafe {
                device.GetCopyableFootprints(
                    &host,
                    index,
                    1,
                    0,
                    Some(&mut footprint),
                    Some(&mut rows),
                    Some(&mut row_bytes),
                    Some(&mut bytes),
                );
            }
            let elements = u32::try_from(row_bytes / u64::from(element_bytes))
                .map_err(|_| E_INVALIDARG)?;
            if rows == 0
                || elements == 0
                || !row_bytes.is_multiple_of(u64::from(element_bytes))
                || bytes == u64::MAX
                || !volume && plane.as_ref().is_some_and(|p| p.element_offset(level, u64::from(elements - 1), rows - 1).is_none())
                || pitch.is_some_and(|p| row_bytes > p || p * u64::from(rows) != source.end - source.start)
            {
                return Err(E_NOTIMPL.into());
            }
            mips.push(Mip {
                slice,
                level,
                source,
                element_bytes,
                footprint,
                elements,
                rows,
                bytes,
            });
        }
        let contents = Mutex::new(Contents {
            previous: vec![Vec::new(); mips.len()],
            ..Default::default()
        });
        Ok(Arc::new(Self {
            native,
            desc: *desc,
            address,
            plane,
            size,
            mips,
            contents,
            images: Arc::downgrade(images),
        }))
    }

    fn snapshot_mip(&self, mip: &Mip, output: &mut Vec<u8>) -> Result<bool> {
        let size = (mip.source.end - mip.source.start) as usize;
        output.clear();
        output.try_reserve_exact(size).map_err(|_| E_OUTOFMEMORY)?;
        let mut copied = 0;
        // SAFETY: the OS checks the guest mapping. A partial transfer never
        // exposes uninitialized bytes, including when pages are decommitted.
        let result = unsafe {
            ReadProcessMemory(
                GetCurrentProcess(),
                (self.address + mip.source.start) as *const _,
                output.as_mut_ptr().cast(),
                size,
                Some(&mut copied),
            )
        };
        if result.is_err() || copied != size {
            return Ok(false);
        }
        unsafe { output.set_len(size) };
        Ok(true)
    }

    fn changed(
        self: &Arc<Self>,
        changes: &mut Vec<upload::Change>,
        scratch: &mut Vec<u8>,
    ) -> Result<()> {
        let mut contents = lock(&self.contents);
        for (level, mip) in self.mips.iter().enumerate() {
            if !self.snapshot_mip(mip, scratch)? {
                contents.previous[level].clear();
                continue;
            }
            if *scratch != contents.previous[level] {
                changes.push(upload::Change {
                    texture: self.clone(),
                    level,
                    bytes: std::mem::take(scratch),
                });
            }
        }
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Placements {
    pub(super) views: Arc<DescriptorMap<PlacedView>>,
    pub(super) targets: Arc<Views<Texture>>,
    memory: Arc<PlacementMemory>,
}

pub(crate) unsafe fn render_target(device: *mut c_void, handle: usize) -> Option<Arc<Texture>> {
    let device = unsafe { &*device.cast::<Device>() };
    device.state.placements.targets.get(handle)
}

pub(crate) unsafe fn render_targets(
    device: *mut c_void, count: u32, handles: *const D3D12_CPU_DESCRIPTOR_HANDLE, contiguous: bool,
    output: &mut Vec<Arc<Texture>>,
) -> Result<()> {
    let device = unsafe { &*device.cast::<Device>() };
    output.clear();
    let stride = if contiguous && count > 1 {
        (unsafe { device.native.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) }) as usize
    } else { 0 };
    for i in 0..count as usize {
        let handle = if contiguous {
            unsafe { (*handles).ptr }.checked_add(i * stride).ok_or(E_INVALIDARG)?
        } else {
            unsafe { (*handles.add(i)).ptr }
        };
        if let Some(texture) = device.state.placements.targets.get(handle) {
            output.try_reserve(1).map_err(|_| E_OUTOFMEMORY)?;
            output.push(texture);
        }
    }
    Ok(())
}

#[derive(Default)]
struct PlacementMemory {
    device: OnceLock<ID3D12Device>,
    // Equal descriptors at the same VA name the same image, including writes
    // made before the title creates a second resource proxy for it.
    images: Arc<Images>,
    submissions: Mutex<Option<Submissions>>,
    failed: Arc<AtomicI32>,
}

// The Xbox export is process-wide. Keep only weak references so that a cache
// flush can visit each live device without extending its lifetime.
static FLUSH_DEVICES: Mutex<Vec<Weak<PlacementMemory>>> = Mutex::new(Vec::new());

pub(crate) fn flush_cpu_writes() {
    unsafe { windows::Win32::System::Threading::FlushProcessWriteBuffers() };
    let live: Vec<_> = {
        let mut registry = lock(&FLUSH_DEVICES);
        registry.retain(|device| device.strong_count() != 0);
        registry.iter().filter_map(Weak::upgrade).collect()
    };
    for memory in live {
        if let Err(error) = memory.flush() {
            crate::diagnostics::recording_error(format_args!(
                "placed texture cache flush: {error}"
            ));
        }
    }
}

impl PlacementMemory {
    fn flush(&self) -> Result<()> {
        let Some(device) = self.device.get() else {
            return Ok(());
        };
        let mut textures: Vec<_> = lock(&self.images.0)
            .values()
            .flatten()
            .filter_map(|alias| alias.texture.upgrade())
            .collect();
        let mut slot = lock(&self.submissions);
        HRESULT(self.failed.load(Ordering::Acquire)).ok()?;
        let submissions = match slot.as_mut() {
            Some(submissions) => submissions,
            None if textures.is_empty() => return Ok(()),
            None => slot.insert(Submissions {
                transfer: upload::Transfer::new(device)?,
                writes: Vec::new(),
            }),
        };
        // The CPU shadow of any pending GPU write is stale until publication.
        // This includes other layouts that alias the same guest pages.
        textures.retain(|texture| {
            !submissions.writes.iter().any(|write| {
                overlaps(&texture.range(), &write.range)
                    && unsafe { write.published.GetCompletedValue() } < write.tick
            })
        });
        submissions.transfer.refresh(device, &textures)
    }
}

impl Placements {
    fn find_texture(images: &BTreeMap<u64, Vec<Alias>>, address: u64, desc: &D3D12_RESOURCE_DESC) -> Option<Arc<Texture>> {
        images.get(&address)?.iter().filter(|alias| alias.desc == *desc)
            .find_map(|alias| alias.texture.upgrade())
    }

    pub(super) fn texture(
        &self,
        device: &ID3D12Device,
        address: u64,
        desc: &D3D12_RESOURCE_DESC,
        initial: D3D12_RESOURCE_STATES,
        clear: Option<&D3D12_CLEAR_VALUE>,
    ) -> Result<Arc<Texture>> {
        if !supported_state(initial, desc.Flags) {
            return Err(E_NOTIMPL.into());
        }
        if let Some(value) = clear {
            if !desc.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET) {
                return Err(E_INVALIDARG.into());
            }
            if value.Format != desc.Format {
                return Err(E_NOTIMPL.into());
            }
        }
        let mut images = lock(&self.memory.images.0);
        // An optimized clear value is a creation hint. Reopening a placed
        // texture with another hint must preserve its GPU and CPU contents.
        if let Some(texture) = Self::find_texture(&images, address, desc) {
            return Ok(texture);
        }
        let texture = Texture::new(device, &self.memory.images, address, desc, clear)?;
        self.memory.device.get_or_init(|| {
            let mut registry = lock(&FLUSH_DEVICES);
            registry.retain(|device| device.strong_count() != 0);
            registry.push(Arc::downgrade(&self.memory));
            device.clone()
        });
        images.entry(address).or_default().push(Alias {
            desc: *desc,
            texture: Arc::downgrade(&texture),
        });
        Ok(texture)
    }
}

struct PendingWrite {
    // The readback job owns the image only through native completion. An idle
    // queue need not keep its last frame's GPU allocations alive indefinitely.
    texture: Weak<Texture>,
    range: Range<u64>,
    published: ID3D12Fence,
    tick: u64,
}

struct Submissions {
    transfer: upload::Transfer,
    writes: Vec<PendingWrite>,
}

impl Texture {
    fn range(&self) -> Range<u64> {
        // The entire range was checked during construction.
        self.address..self.address + self.size
    }
}

fn overlaps(a: &Range<u64>, b: &Range<u64>) -> bool {
    a.start < b.end && b.start < a.end
}

fn unique(textures: &mut Vec<Arc<Texture>>) {
    textures.sort_unstable_by_key(Arc::as_ptr);
    textures.dedup_by(|a, b| Arc::ptr_eq(a, b));
}

// Descriptor contents may change after command-list Close. Resolve SRVs in the
// shader-visible heaps that actually participated in a draw or dispatch.
pub(crate) unsafe fn submit(
    device: *mut c_void,
    queue: &ID3D12CommandQueue,
    access: &QueueAccess,
    textures: &mut Vec<Arc<Texture>>,
    writes: &mut Vec<Arc<Texture>>,
    heaps: &[Range<usize>],
    execute: impl FnOnce() -> Result<()>,
) -> Result<()> {
    let device = unsafe { &*device.cast::<Device>() };
    let placements = &device.state.placements;
    placements.views.gather(heaps, textures, writes);
    unique(writes);
    textures
        .try_reserve(writes.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    textures.extend(writes.iter().cloned());
    if textures.is_empty() {
        return execute();
    }
    unique(textures);
    for writer in writes.iter() {
        if textures
            .iter()
            .any(|t| !Arc::ptr_eq(writer, t) && overlaps(&writer.range(), &t.range()))
        {
            crate::diagnostics::recording_error(format_args!(
                "overlapping placed texture layouts in one GPU write batch"
            ));
            return Err(E_NOTIMPL.into());
        }
    }

    let mut slot = lock(&placements.memory.submissions);
    HRESULT(placements.memory.failed.load(Ordering::Acquire)).ok()?;
    let submission = match slot.as_mut() {
        Some(s) => s,
        None => slot.insert(Submissions {
            transfer: upload::Transfer::new(&device.native)?,
            writes: Vec::new(),
        }),
    };
    let reader = access.reader(&device.native)?;
    submission
        .writes
        .retain(|w| unsafe { w.published.GetCompletedValue() } < w.tick);
    let mut refresh = Vec::new();
    refresh
        .try_reserve(textures.len())
        .map_err(|_| E_OUTOFMEMORY)?;
    for texture in textures.iter() {
        let mut pending = false;
        for write in submission
            .writes
            .iter()
            .filter(|w| overlaps(&texture.range(), &w.range))
        {
            if Arc::as_ptr(texture) != write.texture.as_ptr() {
                crate::diagnostics::recording_error(format_args!(
                    "placed texture layout alias before GPU write publication"
                ));
                return Err(E_NOTIMPL.into());
            }
            // CPU bytes still contain the old version. The native image contains
            // the ordered GPU write and can be read directly by the next batch.
            unsafe { queue.Wait(&write.published, write.tick)? };
            pending = true;
        }
        if !pending {
            refresh.push(texture.clone());
        }
    }

    let transfer = &mut submission.transfer;
    transfer.refresh(&device.native, &refresh)?;
    let required = textures
        .iter()
        .map(|t| lock(&t.contents).uploaded)
        .max()
        .unwrap_or(0);
    if unsafe { transfer.fence.GetCompletedValue() } < required {
        unsafe { queue.Wait(&transfer.fence, required)? };
    }
    for texture in writes.iter() {
        for (fence, value) in &lock(&texture.contents).readers {
            if fence != reader && unsafe { fence.GetCompletedValue() } < *value {
                unsafe { queue.Wait(fence, *value)? };
            }
        }
    }
    let tick = transfer.tick()?;
    let publication = if writes.is_empty() {
        None
    } else {
        submission
            .writes
            .try_reserve(writes.len())
            .map_err(|_| E_OUTOFMEMORY)?;
        let worker = access.worker(&device.native, &placements.memory.failed)?;
        let readback =
            writeback::Readback::record(&device.native, unsafe { queue.GetDesc() }.Type, writes)?;
        Some((worker, readback))
    };
    execute()?;
    if let Some((_, readback)) = &publication {
        // Separate ExecuteCommandLists calls allow simultaneous-access textures
        // to decay to COMMON between the guest batch and this copy.
        unsafe { queue.ExecuteCommandLists(&[Some(readback.list.clone())]) };
    }
    if let Err(error) = unsafe { queue.Signal(reader, tick) } {
        transfer.failed = Some(error.code());
        // A failed signal supplies no proof that the GPU released the recording.
        // Retry once to retire it, or retain it for the lifetime of a live driver.
        if let Some((worker, readback)) = publication {
            if unsafe { queue.Signal(reader, tick) }.is_ok() {
                worker.enqueue(readback, reader, tick)?;
            } else if unsafe { device.native.GetDeviceRemovedReason() }.is_ok() {
                std::mem::forget(readback);
            }
        }
        return Err(error);
    }
    if let Some((worker, readback)) = publication {
        if let Err(error) = worker.enqueue(readback, reader, tick) {
            transfer.failed = Some(error.code());
            return Err(error);
        }
        // A guest fence signaled after this operation must include the CPU
        // publication. The submitting CPU thread remains free to signal fences.
        if let Err(error) = unsafe { queue.Wait(&worker.published, tick) } {
            transfer.failed = Some(error.code());
            return Err(error);
        }
        for texture in writes.iter() {
            submission.writes.push(PendingWrite {
                texture: Arc::downgrade(texture),
                range: texture.range(),
                published: worker.published.clone(),
                tick,
            });
        }
    }
    for texture in textures {
        let mut state = lock(&texture.contents);
        state
            .readers
            .retain(|(f, v)| f != reader && unsafe { f.GetCompletedValue() } < *v);
        state.readers.push((reader.clone(), tick));
    }
    Ok(())
}
