//! Gap 6 remainder: the title packs light as R9G9B9E5 into UAV-only R32_UINT
//! textures with compute and samples them through R9G9B9E5 textures placed at
//! the same address. 5e3a1ee packed_target serves render-target owners with a
//! castable texture, but UAV-only images cannot be castable (allocation.rs
//! packed_uav_formats), so such an alias is its own texture, filled only from
//! title memory (test152: 12 of them). Ported from VIctory-fxceless
//! resource.rs:1187-1237 alias_of and texture.rs alias (GPL-3.0, credit to its
//! authors): when a batch reads the alias after an earlier batch wrote the
//! owner, every level and layer is copied owner -> buffer -> alias on the
//! submitting queue before the batch, and the alias skips its title-memory
//! refresh. Placed textures are simultaneous-access, so the copies need no
//! state tracking. A write in the same batch as the read shows from the next
//! batch on. TUXBOX_PACKED_ALIAS_COPY=0 fills such aliases from title memory
//! as before.
use super::{Alias, Texture, host_desc, lock};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R9G9B9E5_SHAREDEXP, DXGI_SAMPLE_DESC};
use windows::core::Result;

/// The most levels times layers copied (a cube of nine levels), as fxceless.
const PARTS: u32 = 54;

type Footprints = Vec<(D3D12_PLACED_SUBRESOURCE_FOOTPRINT, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)>;

struct Pair {
    owner: Weak<Texture>,
    alias: Weak<Texture>,
    buffer: ID3D12Resource,
    /// Per subresource: the buffer layout written as R32_UINT, read as R9G9B9E5.
    parts: Footprints,
    /// The owner was written since the last copy.
    dirty: bool,
    /// The owner was ever written: the alias no longer reads title memory.
    written: bool,
}

// SAFETY: D3D12 objects are free-threaded; Weak<Texture> is shared already.
unsafe impl Send for Pair {}

static PAIRS: Mutex<Vec<Pair>> = Mutex::new(Vec::new());

/// A copy list in flight, kept until the reader fence passes its tick.
#[allow(dead_code)]
struct Flight(ID3D12CommandAllocator, ID3D12GraphicsCommandList, ID3D12Fence, u64);
// SAFETY: as above.
unsafe impl Send for Flight {}
static FLIGHT: Mutex<Vec<Flight>> = Mutex::new(Vec::new());

pub(super) fn on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::env_toggle!("TUXBOX_PACKED_ALIAS_COPY") != Some("0"))
}

/// The live UAV-only R32_UINT texture of the same shape at `address`, for an
/// R9G9B9E5 placement there.
pub(super) fn owner_of(images: &BTreeMap<u64, Vec<Alias>>, address: u64, desc: &D3D12_RESOURCE_DESC) -> Option<Arc<Texture>> {
    if desc.Format != DXGI_FORMAT_R9G9B9E5_SHAREDEXP || desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER
        || desc.SampleDesc.Count > 1 || !on() {
        return None;
    }
    images.get(&address)?.iter().filter(|a| {
        let d = &a.desc;
        d.Format == DXGI_FORMAT_R32_UINT && d.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS)
            && !d.Flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET)
            && d.Dimension == desc.Dimension && d.Width == desc.Width && d.Height == desc.Height
            && d.DepthOrArraySize == desc.DepthOrArraySize && d.MipLevels == desc.MipLevels && d.SampleDesc.Count <= 1
    }).find_map(|a| a.texture.upgrade())
}

/// Pairs a new alias with its owner: footprints of both, one copy buffer.
pub(super) fn register(device: &ID3D12Device, owner: &Arc<Texture>, alias: &Arc<Texture>) {
    let (from, to) = (host_desc(&owner.desc), host_desc(&alias.desc));
    let count = |d: &D3D12_RESOURCE_DESC| u32::from(d.MipLevels.max(1))
        * if d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE3D { 1 } else { u32::from(d.DepthOrArraySize) };
    let parts = count(&from);
    if parts == 0 || parts > PARTS || parts != count(&to) {
        return;
    }
    let mut rows = [(); 2].map(|()| vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); parts as usize]);
    let mut bytes = [0u64; 2];
    unsafe {
        device.GetCopyableFootprints(&from, 0, parts, 0, Some(rows[0].as_mut_ptr()), None, None, Some(&mut bytes[0]));
        device.GetCopyableFootprints(&to, 0, parts, 0, Some(rows[1].as_mut_ptr()), None, None, Some(&mut bytes[1]));
    }
    let alike = rows[0].iter().zip(&rows[1]).all(|(a, b)| {
        let (fa, fb) = (a.Footprint, b.Footprint);
        a.Offset == b.Offset && (fa.Width, fa.Height, fa.Depth, fa.RowPitch) == (fb.Width, fb.Height, fb.Depth, fb.RowPitch)
    });
    if bytes[0] == 0 || bytes[0] != bytes[1] || !alike {
        return;
    }
    let heap = D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: bytes[0], Height: 1, DepthOrArraySize: 1, MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR, ..Default::default()
    };
    let mut buffer: Option<ID3D12Resource> = None;
    let made = unsafe {
        device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COMMON, None, &mut buffer)
    };
    let (Ok(()), Some(buffer)) = (made, buffer) else { return };
    crate::dred::name(&buffer, "tuxbox packed alias copy");
    let [written, read] = rows;
    let mut pairs = lock(&PAIRS);
    pairs.retain(|p| p.alias.strong_count() != 0 && p.owner.strong_count() != 0);
    pairs.push(Pair {
        owner: Arc::downgrade(owner), alias: Arc::downgrade(alias), buffer,
        parts: written.into_iter().zip(read).collect(), dirty: false, written: false,
    });
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed) + 1;
    if n <= 8 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: R9G9B9E5 {}x{} ({parts} parts) at {:#x} over a UAV-only R32_UINT texture: copied from it after its writes ({n} so far; TUXBOX_PACKED_ALIAS_COPY=0 reads title memory)",
            alias.desc.Width, alias.desc.Height, alias.address));
    }
}

/// For a batch reading `textures` and writing `writes`: the copy list it
/// needs first (aliases whose owners earlier batches wrote), for a `kind`
/// queue, and the aliases that skip their title-memory refresh. Then marks
/// this batch's writes for the next batch.
pub(super) fn before_batch(device: &ID3D12Device, kind: D3D12_COMMAND_LIST_TYPE, textures: &[Arc<Texture>], writes: &[Arc<Texture>])
    -> (Option<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)>, Vec<*const Texture>) {
    let mut pairs = lock(&PAIRS);
    if pairs.is_empty() {
        return (None, Vec::new());
    }
    let mut skip = Vec::new();
    let mut due = Vec::new();
    for (i, p) in pairs.iter().enumerate() {
        if p.written && textures.iter().any(|t| Weak::as_ptr(&p.alias) == Arc::as_ptr(t)) {
            skip.push(Weak::as_ptr(&p.alias));
            if p.dirty {
                due.push(i);
            }
        }
    }
    let mut list = None;
    if !due.is_empty() {
        match record(device, kind, &pairs, &due) {
            Ok(made) => {
                due.iter().for_each(|&i| pairs[i].dirty = false);
                list = Some(made);
            }
            // Not copied: the alias reads title memory this batch.
            Err(_) => skip.clear(),
        }
    }
    for p in pairs.iter_mut() {
        if writes.iter().any(|w| Weak::as_ptr(&p.owner) == Arc::as_ptr(w)) {
            p.dirty = true;
            p.written = true;
        }
    }
    (list, skip)
}

fn record(device: &ID3D12Device, kind: D3D12_COMMAND_LIST_TYPE, pairs: &[Pair], due: &[usize])
    -> Result<(ID3D12CommandAllocator, ID3D12GraphicsCommandList)> {
    let allocator: ID3D12CommandAllocator = unsafe { device.CreateCommandAllocator(kind)? };
    let list: ID3D12GraphicsCommandList = unsafe { device.CreateCommandList(0, kind, &allocator, None)? };
    crate::dred::name(&list, "tuxbox packed alias copy");
    let mut copied = Vec::new();
    for &i in due {
        let p = &pairs[i];
        let (Some(owner), Some(alias)) = (p.owner.upgrade(), p.alias.upgrade()) else { continue };
        for (sub, (written, _)) in p.parts.iter().enumerate() {
            copy(&list, location_buffer(&p.buffer, *written), location_texture(&owner.native, sub as u32));
        }
        let mut barrier = D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 { Transition: core::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: core::mem::ManuallyDrop::new(Some(p.buffer.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: D3D12_RESOURCE_STATE_COPY_DEST,
                StateAfter: D3D12_RESOURCE_STATE_COPY_SOURCE,
            }) },
        };
        unsafe {
            list.ResourceBarrier(core::slice::from_ref(&barrier));
            core::mem::ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
        }
        for (sub, (_, read)) in p.parts.iter().enumerate() {
            copy(&list, location_texture(&alias.native, sub as u32), location_buffer(&p.buffer, *read));
        }
        copied.push((alias.desc.Width, alias.desc.Height));
    }
    unsafe { list.Close()? };
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(copied.len() as u64, Ordering::Relaxed) + copied.len() as u64;
    static LINES: AtomicU64 = AtomicU64::new(0);
    let line = LINES.fetch_add(1, Ordering::Relaxed) + 1;
    if line <= 8 || line.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "placed textures: copied {} R9G9B9E5 aliases from their written R32_UINT owners ({copied:?}; {n} so far)", copied.len()));
    }
    Ok((allocator, list))
}

fn copy(list: &ID3D12GraphicsCommandList, mut dst: D3D12_TEXTURE_COPY_LOCATION, mut src: D3D12_TEXTURE_COPY_LOCATION) {
    unsafe {
        list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
        core::mem::ManuallyDrop::drop(&mut src.pResource);
        core::mem::ManuallyDrop::drop(&mut dst.pResource);
    }
}

fn location_texture(resource: &ID3D12Resource, sub: u32) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: core::mem::ManuallyDrop::new(Some(resource.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: sub },
    }
}

fn location_buffer(resource: &ID3D12Resource, footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT) -> D3D12_TEXTURE_COPY_LOCATION {
    D3D12_TEXTURE_COPY_LOCATION {
        pResource: core::mem::ManuallyDrop::new(Some(resource.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { PlacedFootprint: footprint },
    }
}

/// Keeps a copy list alive until `reader` passes `tick`; drops finished ones.
pub(super) fn retain(list: (ID3D12CommandAllocator, ID3D12GraphicsCommandList), reader: &ID3D12Fence, tick: u64) {
    let mut flight = lock(&FLIGHT);
    flight.retain(|f| unsafe { f.2.GetCompletedValue() } < f.3);
    flight.push(Flight(list.0, list.1, reader.clone(), tick));
}
