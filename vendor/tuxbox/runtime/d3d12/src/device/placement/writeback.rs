//! Publish the GPU's texels to the original Xbox pages before the guest fence.
use super::{Texture, committed, lock};
use core::{mem::ManuallyDrop, ops::Range};
use std::sync::Arc;
use windows::Win32::Foundation::{
    E_ACCESSDENIED, E_FAIL, E_INVALIDARG, E_OUTOFMEMORY, HANDLE, NTSTATUS,
};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_SAMPLE_DESC};
use windows::Win32::System::{
    Diagnostics::Debug::WriteProcessMemory, Memory::*, Threading::GetCurrentProcess,
};
use windows::core::{Interface, Result};

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtWriteVirtualMemory(
        process: HANDLE,
        address: *mut core::ffi::c_void,
        buffer: *const core::ffi::c_void,
        size: usize,
        written: *mut usize,
    ) -> NTSTATUS;
}

pub(super) struct Readback {
    pub(super) list: ID3D12CommandList,
    _allocator: ID3D12CommandAllocator,
    buffer: ID3D12Resource,
    size: usize,
    /// Bytes counted in IN_FLIGHT until this is dropped.
    reserved: u64,
    // One footprint offset per mip (including array slices) in the readback buffer.
    textures: Vec<(Arc<Texture>, Vec<u64>)>,
}

/// test84 ended with three 939 MiB readback buffers alive (one batch's
/// write-back of about 500 textures each) and 259 MiB of free commit. Live
/// readback buffers together stay within TUXBOX_WRITEBACK_INFLIGHT_MB (512;
/// 0: no cap): a texture that would pass it is not written back this batch
/// and the title's CPU keeps its old bytes there.
static IN_FLIGHT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl Drop for Readback {
    fn drop(&mut self) {
        IN_FLIGHT.fetch_sub(self.reserved, std::sync::atomic::Ordering::Relaxed);
    }
}

impl Readback {
    pub(super) fn record(
        device: &ID3D12Device,
        kind: D3D12_COMMAND_LIST_TYPE,
        writers: &[Arc<Texture>],
    ) -> Result<Self> {
        let mut total = 0u64;
        let mut textures = Vec::new();
        // test72 made 2821 MiB of readback buffers in ten seconds for four
        // batches (a texture with levels of hundreds of MiB, copied back to
        // title memory on every write). A texture with a level over
        // TUXBOX_WRITEBACK_MAX_MB (256) is not written back: the title's CPU
        // keeps its old bytes there. 0 lifts the cap.
        static MB: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let cap = *MB.get_or_init(|| crate::env_toggle!("TUXBOX_WRITEBACK_MAX_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(256)) << 20;
        static BUDGET: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
        let budget = *BUDGET.get_or_init(|| crate::env_toggle!("TUXBOX_WRITEBACK_INFLIGHT_MB").and_then(|v| v.trim().parse().ok()).unwrap_or(512)) << 20;
        let in_flight = IN_FLIGHT.load(std::sync::atomic::Ordering::Relaxed);
        let (mut over, mut over_bytes) = (0usize, 0u64);
        for texture in writers {
            if budget != 0 {
                let bytes: u64 = texture.mips.iter().map(|m| m.bytes.next_multiple_of(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT.into())).sum();
                if in_flight + total + bytes > budget {
                    over += 1;
                    over_bytes += bytes;
                    continue;
                }
            }
            if cap != 0 && let Some(big) = texture.mips.iter().map(|m| m.bytes).max().filter(|&b| b > cap) {
                static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n <= 4 || n.is_power_of_two() {
                    let d = &texture.desc;
                    crate::diagnostics::log_line(format_args!(
                        "placed textures: a {}x{}x{} format {} texture with a {} MiB level not written back to title memory ({n} so far; TUXBOX_WRITEBACK_MAX_MB=0 writes it)",
                        d.Width, d.Height, d.DepthOrArraySize, d.Format.0, big >> 20));
                }
                continue;
            }
            let mut offsets = Vec::with_capacity(texture.mips.len());
            for mip in &texture.mips {
                let offset = total
                    .checked_next_multiple_of(D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT.into())
                    .ok_or(E_OUTOFMEMORY)?;
                total = offset.checked_add(mip.bytes).ok_or(E_OUTOFMEMORY)?;
                offsets.push(offset);
            }
            textures.push((texture.clone(), offsets));
        }
        if over != 0 {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n <= 8 || n.is_power_of_two() {
                crate::diagnostics::log_line(format_args!(
                    "placed textures: {over} of {} written textures ({} MiB) not written back: readback buffers in flight {} MiB + this batch {} MiB reach TUXBOX_WRITEBACK_INFLIGHT_MB ({n} so far; 0 lifts it)",
                    writers.len(), over_bytes >> 20, in_flight >> 20, total >> 20));
            }
        }
        let size = usize::try_from(total)
            .ok()
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or(E_OUTOFMEMORY)?;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            // Every writer may be over the cap: an empty copy still completes.
            Width: total.max(256),
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
        let buffer = committed(
            device,
            D3D12_HEAP_TYPE_READBACK,
            &desc,
            D3D12_RESOURCE_STATE_COPY_DEST,
            None,
        )?;
        // SAFETY: the new allocator and command list belong to this device;
        // both, the buffer and the source textures are retained to completion.
        let allocator = unsafe { device.CreateCommandAllocator(kind)? };
        let list: ID3D12GraphicsCommandList =
            unsafe { device.CreateCommandList(0, kind, &allocator, None)? };
        crate::dred::name(&list, "tuxbox placed texture readback");
        for (texture, offsets) in &textures {
            for (level, &offset) in offsets.iter().enumerate() {
                let mut footprint = texture.mips[level].footprint;
                footprint.Offset = offset;
                let mut src = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(texture.native.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        SubresourceIndex: level as u32,
                    },
                };
                let mut dst = D3D12_TEXTURE_COPY_LOCATION {
                    pResource: ManuallyDrop::new(Some(buffer.clone())),
                    Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                    Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                        PlacedFootprint: footprint,
                    },
                };
                // SAFETY: each tag selects its initialized union field. A separate
                // ExecuteCommandLists boundary decays the simultaneous-access image.
                unsafe {
                    list.CopyTextureRegion(&dst, 0, 0, 0, &src, None);
                    ManuallyDrop::drop(&mut src.pResource);
                    ManuallyDrop::drop(&mut dst.pResource);
                }
            }
        }
        unsafe { list.Close()? };
        IN_FLIGHT.fetch_add(total, std::sync::atomic::Ordering::Relaxed);
        Ok(Self {
            reserved: total,
            list: list.cast()?,
            _allocator: allocator,
            buffer,
            size,
            textures,
        })
    }

    /// Called after the native fence covering every footprint has completed.
    pub(super) fn complete(self) -> Result<()> {
        let mut mapped = core::ptr::null_mut();
        let range = D3D12_RANGE {
            Begin: 0,
            End: self.size,
        };
        unsafe { self.buffer.Map(0, Some(&range), Some(&mut mapped))? };
        // SAFETY: this is a completed GPU readback. The mapping covers this
        // allocation, remains live for the loop and is only accessed for reads.
        let data = unsafe { core::slice::from_raw_parts(mapped.cast::<u8>(), self.size) };
        let mut tiled = Vec::new();
        let mut valid = Vec::new();
        let result = self.textures.iter().try_for_each(|(texture, offsets)| {
            let mut state = lock(&texture.contents);
            for (level, &offset) in offsets.iter().enumerate() {
                let mip = &texture.mips[level];
                let size = (mip.source.end - mip.source.start) as usize;
                let element = mip.element_bytes as usize;
                let address = (texture.address + mip.source.start) as usize;
                let mut region = query_region(address)?;
                // There are no guest bytes to publish if a single unmapped
                // region covers this mip. Recheck on every publication, since
                // the guest can commit backing after creating the texture.
                if region.0 - address >= size && !mapped_region(&region.1) {
                    state.previous[level] = None;
                    continue;
                }
                tiled.clear();
                tiled.try_reserve_exact(size).map_err(|_| E_OUTOFMEMORY)?;
                tiled.resize(size, 0u8);
                let elements = size.div_ceil(element);
                let dense = u64::from(mip.elements)
                    * u64::from(mip.rows)
                    * u64::from(mip.footprint.Footprint.Depth)
                    * u64::from(mip.element_bytes)
                    == size as u64;
                if !dense {
                    valid.clear();
                    let words = elements.div_ceil(64);
                    valid.try_reserve_exact(words).map_err(|_| E_OUTOFMEMORY)?;
                    valid.resize(words, 0u64);
                }
                let linear = data
                    .get(offset as usize..(offset + mip.bytes) as usize)
                    .ok_or(E_INVALIDARG)?;
                texture.layout_runs(level, |target, source| {
                    tiled
                        .get_mut(target.clone())
                        .ok_or(E_INVALIDARG)?
                        .copy_from_slice(linear.get(source).ok_or(E_INVALIDARG)?);
                    if !dense {
                        mark_valid(&mut valid, target.start / element..target.end / element);
                    }
                    Ok(())
                })?;
                // Coalesce adjacent texels in physical address order. Never
                // write row padding, partial-tile holes or neighbouring mips.
                if dense {
                    write_mapped(address, &tiled, 0..size, &mut region)?;
                } else {
                    let mut first = next_bit(&valid, 0, elements, true);
                    while first < elements {
                        let end = next_bit(&valid, first, elements, false);
                        write_mapped(address, &tiled, first * element..end * element, &mut region)?;
                        first = next_bit(&valid, end, elements, true);
                    }
                }
                // Include padding from guest memory in the comparison baseline:
                // upload::refresh compares the entire physical mip snapshot.
                state.previous[level] = texture.snapshot_mip(mip, &mut tiled)?.then(|| super::level_hash(&tiled));
            }
            Ok(())
        });
        // SAFETY: no CPU writes were made to the GPU readback allocation.
        unsafe { self.buffer.Unmap(0, Some(&D3D12_RANGE::default())) };
        result
    }
}

fn mark_valid(valid: &mut [u64], mut range: Range<usize>) {
    while range.start < range.end {
        let bit = range.start % 64;
        let count = range.len().min(64 - bit);
        valid[range.start / 64] |= (u64::MAX >> (64 - count)) << bit;
        range.start += count;
    }
}

fn next_bit(valid: &[u64], mut at: usize, limit: usize, set: bool) -> usize {
    while at < limit {
        let word = valid[at / 64];
        let matches = if set { word } else { !word };
        let bit = at % 64;
        let next = (matches & (u64::MAX << bit)).trailing_zeros() as usize;
        at = at - bit + next;
        if next < 64 {
            break;
        }
    }
    at.min(limit)
}

fn query_region(address: usize) -> Result<(usize, MEMORY_BASIC_INFORMATION)> {
    let mut info = MEMORY_BASIC_INFORMATION::default();
    // SAFETY: VirtualQuery probes the address and fills a local structure.
    if unsafe {
        VirtualQuery(
            Some(address as *const _),
            &mut info,
            size_of::<MEMORY_BASIC_INFORMATION>(),
        )
    } == 0
    {
        return Err(E_FAIL.into());
    }
    let end = (info.BaseAddress as usize)
        .checked_add(info.RegionSize)
        .filter(|&end| end > address)
        .ok_or(E_INVALIDARG)?;
    Ok((end, info))
}

fn mapped_region(info: &MEMORY_BASIC_INFORMATION) -> bool {
    info.State == MEM_COMMIT && info.Protect.0 & 0xff != PAGE_NOACCESS.0
}

/// Xbox can reserve a GPU address with no accessible CPU backing. Preserve its
/// native texture until pages are supplied. Never change guest page protection.
fn write_mapped(
    base: usize,
    bytes: &[u8],
    range: Range<usize>,
    region: &mut (usize, MEMORY_BASIC_INFORMATION),
) -> Result<()> {
    let mut at = range.start;
    while at < range.end {
        let addr = base.checked_add(at).ok_or(E_INVALIDARG)?;
        // Runs visit increasing physical addresses. Reuse the protection scan
        // for the spans in this region until crossing into a different one.
        if addr >= region.0 {
            *region = query_region(addr)?;
        }
        let end = (region.0 - base).min(range.end);
        let info = &region.1;
        if mapped_region(info) {
            let access = PAGE_PROTECTION_FLAGS(info.Protect.0 & 0xff);
            if info.Protect.contains(PAGE_GUARD)
                || !matches!(
                    access,
                    PAGE_READWRITE
                        | PAGE_WRITECOPY
                        | PAGE_EXECUTE_READWRITE
                        | PAGE_EXECUTE_WRITECOPY
                )
            {
                return Err(E_ACCESSDENIED.into());
            }
            let block = &bytes[at..end];
            let mut written = 0;
            // SAFETY: the source is an owned initialized slice. Guest mappings
            // remain owned by the guest until the submission fence completes.
            // Use the native write for data pages so Wine doesn't repeat its
            // region scan for every run. Keep the instruction-cache flush for
            // executable mappings.
            unsafe {
                if matches!(access, PAGE_READWRITE | PAGE_WRITECOPY) {
                    let status = NtWriteVirtualMemory(
                        GetCurrentProcess(),
                        addr as *mut _,
                        block.as_ptr().cast(),
                        block.len(),
                        &mut written,
                    );
                    if status.0 < 0 {
                        return Err(E_FAIL.into());
                    }
                } else {
                    WriteProcessMemory(
                        GetCurrentProcess(),
                        addr as *const _,
                        block.as_ptr().cast(),
                        block.len(),
                        Some(&mut written),
                    )?;
                }
            };
            if written != block.len() {
                return Err(E_FAIL.into());
            }
        }
        at = end;
    }
    Ok(())
}
