//! Slot 62, `CopyMemoryToMemoryX(Destination, Source, SizeBytes, Flags)`
//! (GDK d3d12_xs.h): a GPU copy between two GPU virtual addresses.
//!
//! Either address can name guest memory or a texture. ProjectScarlett reads
//! textures back this way (title RVA 0x979650): it passes a CPU staging
//! allocation as the destination and the texture as the source, waits for the
//! GPU, then reads the rows at a 256-byte pitch. That is the desktop copy
//! footprint, so texture bytes are exchanged in that linear layout. A guest
//! that expects tiled bytes from a raw copy is not reproduced.

use std::sync::atomic::{AtomicU32, Ordering};

use windows::core::Result;
use windows::Win32::Foundation::{E_FAIL, E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;

use super::{parent, recording, List};
use crate::device::{self, LinearResource};

const PLACEMENT: u64 = D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT as u64;

struct Region {
    index: u32,
    offset: u64,
    span: u64,
    pitch: u32,
}

enum Plan {
    Whole,
    One(u32),
}

/// Guest rows include the tail of the last row. The driver footprint may end
/// a little earlier, or a packed chain may add placement padding.
fn fits(guest: u64, needed: u64, slack: u64) -> bool {
    guest >= needed && guest - needed < slack
}

fn plan(guest_offset: u64, guest_bytes: u64, total: u64, regions: &[Region]) -> Option<Plan> {
    if regions.is_empty() || guest_bytes == 0 || total == 0 {
        return None;
    }
    let mut last_end = 0u64;
    let mut slack = PLACEMENT;
    for region in regions {
        if region.pitch == 0 || region.span == 0 {
            return None;
        }
        let end = region.offset.checked_add(region.span)?;
        last_end = last_end.max(end);
        slack = slack.max(u64::from(region.pitch));
    }
    let whole = guest_offset == 0
        && regions
            .iter()
            .all(|region| region.offset.is_multiple_of(PLACEMENT))
        && last_end <= guest_bytes
        && fits(guest_bytes, last_end, slack)
        && guest_bytes.saturating_add(slack) >= total
        && fits(total.max(guest_bytes), guest_bytes.min(total), slack);
    if whole {
        return Some(Plan::Whole);
    }
    // A single-subresource texture can arrive with its rows padded to an
    // aligned height (a 128x4 RGBA16 target uploads 0x2000 bytes for a 0x1000
    // footprint). Only the footprint is read, so accept a tail of at most one
    // footprint. Larger buffers stay rejected.
    if let [region] = regions
        && guest_offset == 0
        && region.offset == 0
        && guest_bytes >= region.span
        && guest_bytes - region.span <= region.span
    {
        return Some(Plan::One(region.index));
    }
    regions
        .iter()
        .find(|region| {
            let at_start = guest_offset == 0 && region.index == 0;
            (at_start || guest_offset == region.offset)
                && fits(
                    guest_bytes,
                    region.span,
                    u64::from(region.pitch).max(PLACEMENT),
                )
        })
        .map(|region| Plan::One(region.index))
}

fn footprint_bytes(pitch: u32, rows: u32, depth: u32) -> Option<u64> {
    u64::from(pitch)
        .checked_mul(u64::from(rows))?
        .checked_mul(u64::from(depth))
}

fn note(detail: &str) {
    static COUNT: AtomicU32 = AtomicU32::new(0);
    if COUNT.fetch_add(1, Ordering::Relaxed) < 8 {
        crate::diagnostics::log(detail);
    }
}

/// A device-local buffer holding bytes between the two halves of a copy.
fn staging_buffer(device: &ID3D12Device, bytes: u64) -> Result<ID3D12Resource> {
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
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
        ..Default::default()
    };
    let mut native = None;
    // SAFETY: D3D12 borrows the descriptor only for the duration of the call.
    unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_COMMON,
            None,
            &mut native,
        )?;
    }
    native.ok_or_else(|| E_FAIL.into())
}

/// The staging buffer was promoted to a copy destination by its first copy.
fn staging_becomes_source(list: &ID3D12GraphicsCommandList, buffer: &ID3D12Resource) {
    let mut barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: std::mem::ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: D3D12_RESOURCE_STATE_COPY_DEST,
                StateAfter: D3D12_RESOURCE_STATE_COPY_SOURCE,
            }),
        },
    };
    // SAFETY: the transition arm is the active one, and the call borrows the
    // barrier only while it records it.
    unsafe {
        list.ResourceBarrier(std::slice::from_ref(&barrier));
        std::mem::ManuallyDrop::drop(&mut (*barrier.Anonymous.Transition).pResource);
    }
}

fn plane_count(
    device: &ID3D12Device,
    format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT,
) -> u32 {
    let mut info = D3D12_FEATURE_DATA_FORMAT_INFO {
        Format: format,
        PlaneCount: 1,
    };
    // SAFETY: info is a live FORMAT_INFO record and the size matches that struct.
    let queried = unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_FORMAT_INFO,
            core::ptr::from_mut(&mut info).cast(),
            core::mem::size_of_val(&info) as u32,
        )
    };
    if queried.is_ok() && info.PlaneCount > 0 {
        u32::from(info.PlaneCount)
    } else {
        1
    }
}

fn subresource_count(device: &ID3D12Device, desc: &D3D12_RESOURCE_DESC) -> Option<u32> {
    let mips = u32::from(desc.MipLevels);
    // NV12 and the other planar video formats have one subresource per plane.
    let (slices, planes) = match desc.Dimension {
        D3D12_RESOURCE_DIMENSION_TEXTURE1D | D3D12_RESOURCE_DIMENSION_TEXTURE2D => (
            u32::from(desc.DepthOrArraySize),
            plane_count(device, desc.Format),
        ),
        D3D12_RESOURCE_DIMENSION_TEXTURE3D => (1, 1),
        _ => return None,
    };
    mips.checked_mul(slices)?
        .checked_mul(planes)
        .filter(|count| (1..=2048).contains(count))
}

fn regions_for(
    device: &ID3D12Device,
    resource: &ID3D12Resource,
) -> Result<(u64, Vec<Region>, Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>)> {
    let desc = unsafe { resource.GetDesc() };
    let count = subresource_count(device, &desc).ok_or(E_INVALIDARG)?;
    let mut layouts = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); count as usize];
    let mut rows = vec![0u32; count as usize];
    let mut row_bytes = vec![0u64; count as usize];
    let mut total = 0u64;
    // SAFETY: the three output slices have one entry per subresource.
    unsafe {
        device.GetCopyableFootprints(
            &desc,
            0,
            count,
            0,
            Some(layouts.as_mut_ptr()),
            Some(rows.as_mut_ptr()),
            Some(row_bytes.as_mut_ptr()),
            Some(&mut total),
        );
    }
    let mut regions = Vec::with_capacity(count as usize);
    for (index, layout) in layouts.iter().enumerate() {
        let footprint = &layout.Footprint;
        let pitch = footprint.RowPitch;
        let row = row_bytes[index];
        if footprint.Width == 0
            || rows[index] == 0
            || footprint.Depth == 0
            || row == 0
            || row > u64::from(pitch)
        {
            crate::diagnostics::recording_error(format_args!(
                "slot 62 subresource {index} of {}x{} format {} has no copyable footprint",
                desc.Width, desc.Height, desc.Format.0
            ));
            return Err(E_INVALIDARG.into());
        }
        let Some(span) = footprint_bytes(pitch, rows[index], footprint.Depth) else {
            return Err(E_INVALIDARG.into());
        };
        regions.push(Region {
            index: index as u32,
            offset: layout.Offset,
            span,
            pitch,
        });
    }
    Ok((total, regions, layouts))
}

#[derive(Clone, Copy)]
enum Direction {
    IntoTexture,
    FromTexture,
}

/// Copy one subresource between a texture and its footprint in `buffer`.
fn record_texture(
    list: &ID3D12GraphicsCommandList,
    texture: &ID3D12Resource,
    buffer: &ID3D12Resource,
    subresource: u32,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    direction: Direction,
) {
    let mut linear = D3D12_TEXTURE_COPY_LOCATION {
        pResource: std::mem::ManuallyDrop::new(Some(buffer.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: footprint,
        },
    };
    let mut image = D3D12_TEXTURE_COPY_LOCATION {
        pResource: std::mem::ManuallyDrop::new(Some(texture.clone())),
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: subresource,
        },
    };
    // SAFETY: each location's active union field matches its type tag.
    // CopyTextureRegion copies the resource pointers before returning.
    unsafe {
        match direction {
            Direction::IntoTexture => list.CopyTextureRegion(&image, 0, 0, 0, &linear, None),
            Direction::FromTexture => list.CopyTextureRegion(&linear, 0, 0, 0, &image, None),
        }
        std::mem::ManuallyDrop::drop(&mut linear.pResource);
        std::mem::ManuallyDrop::drop(&mut image.pResource);
    }
}

/// The subresources a guest byte range covers, with their footprints placed in
/// a staging buffer, and how many bytes of that buffer belong to the range.
fn texture_layout(
    device: &ID3D12Device,
    texture: &ID3D12Resource,
    guest_offset: u64,
    guest_bytes: u64,
) -> Result<(u64, u64, Vec<(u32, D3D12_PLACED_SUBRESOURCE_FOOTPRINT)>)> {
    let (total, regions, layouts) = regions_for(device, texture)?;
    let Some(plan) = plan(guest_offset, guest_bytes, total, &regions) else {
        let desc = unsafe { texture.GetDesc() };
        let heads: Vec<String> = regions
            .iter()
            .take(4)
            .map(|region| {
                format!(
                    "#{} off={:#x} span={:#x} pitch={}",
                    region.index, region.offset, region.span, region.pitch
                )
            })
            .collect();
        crate::diagnostics::recording_error(format_args!(
            "slot 62 texture {}x{}x{} format={} mips={} offset={guest_offset:#x} bytes={guest_bytes:#x} total={total:#x} [{}]",
            desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.MipLevels, heads.join(", ")
        ));
        return Err(E_INVALIDARG.into());
    };
    match plan {
        Plan::Whole => Ok((
            total,
            total.min(guest_bytes),
            regions.iter().map(|r| r.index).zip(layouts).collect(),
        )),
        Plan::One(index) => {
            let mut layout = *layouts.get(index as usize).ok_or(E_INVALIDARG)?;
            layout.Offset = 0;
            let span = regions
                .iter()
                .find(|r| r.index == index)
                .ok_or(E_INVALIDARG)?
                .span;
            Ok((span, span.min(guest_bytes), vec![(index, layout)]))
        }
    }
}

fn record_copy(
    list: &List,
    state: &mut super::Recording,
    device: &ID3D12Device,
    destination: LinearResource,
    source: LinearResource,
    bytes: u64,
) -> Result<()> {
    use LinearResource::{Buffer, Texture};
    let native = &list.native;
    match (destination, source) {
        (Buffer { resource: to, offset: to_offset }, Buffer { resource: from, offset: from_offset }) => {
            super::raytracing::retain(state, &[&to, &from])?;
            if to == from {
                // One buffer cannot be both ends of a desktop copy.
                let staging = staging_buffer(device, bytes)?;
                super::raytracing::retain(state, &[&staging])?;
                unsafe { native.CopyBufferRegion(&staging, 0, &from, from_offset, bytes) };
                staging_becomes_source(native, &staging);
                unsafe { native.CopyBufferRegion(&to, to_offset, &staging, 0, bytes) };
            } else {
                // SAFETY: both buffers belong to this open list and contain the range.
                unsafe { native.CopyBufferRegion(&to, to_offset, &from, from_offset, bytes) };
            }
        }
        (Buffer { resource: to, offset: to_offset }, Texture { resource: from, placed, offset }) => {
            if let Some(texture) = &placed {
                state.prepare_copy(None, Some(texture)).map_err(windows::core::Error::from)?;
            }
            let (size, used, footprints) = texture_layout(device, &from, offset, bytes)?;
            // Footprints need an aligned buffer offset, which a guest address
            // need not have, so go through a staging buffer.
            let staging = staging_buffer(device, size)?;
            super::raytracing::retain(state, &[&to, &from, &staging])?;
            for (index, layout) in footprints {
                record_texture(native, &from, &staging, index, layout, Direction::FromTexture);
            }
            staging_becomes_source(native, &staging);
            // SAFETY: `used` fits both the staging buffer and the guest range.
            unsafe { native.CopyBufferRegion(&to, to_offset, &staging, 0, used) };
        }
        (Texture { resource: to, placed, offset }, Buffer { resource: from, offset: from_offset }) => {
            if let Some(texture) = &placed {
                state.prepare_copy(Some(texture), None).map_err(windows::core::Error::from)?;
            }
            let (size, used, footprints) = texture_layout(device, &to, offset, bytes)?;
            let staging = staging_buffer(device, size)?;
            super::raytracing::retain(state, &[&to, &from, &staging])?;
            // SAFETY: `used` fits both the staging buffer and the guest range.
            unsafe { native.CopyBufferRegion(&staging, 0, &from, from_offset, used) };
            staging_becomes_source(native, &staging);
            for (index, layout) in footprints {
                record_texture(native, &to, &staging, index, layout, Direction::IntoTexture);
            }
        }
        (Texture { .. }, Texture { .. }) => {
            crate::diagnostics::recording_error(format_args!(
                "slot 62 texture-to-texture copy of {bytes:#x} bytes"
            ));
            return Err(E_NOTIMPL.into());
        }
    }
    Ok(())
}

pub(super) unsafe extern "system" fn copy_memory(
    list: &List,
    destination: u64,
    source: u64,
    bytes: u64,
    flags: u32,
) {
    let mut state = recording(list);
    if !state.can_record(&list.native) {
        return;
    }
    // The console flags decompress or reinterpret surfaces during the copy.
    if flags != 0 || (bytes != 0 && (destination == 0 || source == 0)) {
        crate::diagnostics::recording_error(format_args!(
            "slot 62 copy destination={destination:#x} source={source:#x} bytes={bytes:#x} flags={flags:#x}"
        ));
        state.error = if flags != 0 { E_NOTIMPL } else { E_INVALIDARG };
        return;
    }
    if bytes == 0 {
        return;
    }
    let result = (|| -> Result<()> {
        super::memory_wait::trace_write("CopyMemoryToMemoryX", destination, bytes, 0);
        let owner = parent(&list.parent);
        let from = unsafe { device::linear_resource(owner, source, bytes)? };
        let to = unsafe { device::linear_resource(owner, destination, bytes)? };
        let kind = |resource: &LinearResource| match resource {
            LinearResource::Buffer { .. } => "memory",
            LinearResource::Texture { .. } => "texture",
        };
        let (to_kind, from_kind) = (kind(&to), kind(&from));
        let mut native_device: Option<ID3D12Device> = None;
        unsafe { list.native.GetDevice(&mut native_device)? };
        record_copy(list, &mut state, &native_device.ok_or(E_FAIL)?, to, from, bytes)?;
        note(&format!(
            "slot 62 copied {bytes:#x} bytes from {from_kind} {source:#x} to {to_kind} {destination:#x}"
        ));
        Ok(())
    })();
    if let Err(error) = result {
        state.error = error.code();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(index: u32, offset: u64, span: u64, pitch: u32) -> Region {
        Region {
            index,
            offset,
            span,
            pitch,
        }
    }

    #[test]
    fn one_plane_matches_its_row_pitch() {
        let regions = [region(0, 0, 8_294_400, 7680)];
        assert!(matches!(
            plan(0, 8_294_400, 8_294_400, &regions),
            Some(Plan::Whole)
        ));
    }

    #[test]
    fn mip0_is_not_the_whole_chain() {
        let regions = [region(0, 0, 1000, 256), region(1, 1024, 500, 256)];
        assert!(matches!(plan(0, 1000, 1524, &regions), Some(Plan::One(0))));
    }

    #[test]
    fn packed_chain_uses_driver_offsets() {
        let regions = [region(0, 0, 2048, 256), region(1, 2048, 1024, 256)];
        assert!(matches!(plan(0, 3072, 3072, &regions), Some(Plan::Whole)));
    }

    #[test]
    fn second_subresource_can_be_addressed_alone() {
        let regions = [region(0, 0, 2048, 256), region(1, 2048, 1024, 256)];
        assert!(matches!(
            plan(2048, 1024, 3072, &regions),
            Some(Plan::One(1))
        ));
    }

    #[test]
    fn height_padded_single_subresource_uploads_its_footprint() {
        let regions = [region(0, 0, 0x1000, 1024)];
        assert!(matches!(plan(0, 0x2000, 0x1000, &regions), Some(Plan::One(0))));
        assert!(plan(0, 0x2001, 0x1000, &regions).is_none());
    }

    #[test]
    fn oversized_guest_buffer_is_rejected() {
        let regions = [region(0, 0, 1000, 256)];
        assert!(plan(0, 50_000, 1000, &regions).is_none());
    }
}
