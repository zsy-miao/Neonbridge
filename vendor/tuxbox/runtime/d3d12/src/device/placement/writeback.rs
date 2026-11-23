//! Publish the GPU's texels to the original Xbox pages before the guest fence.
use super::{Texture, committed, lock};
use core::{mem::ManuallyDrop, ops::Range};
use std::sync::Arc;
use windows::Win32::Foundation::{E_ACCESSDENIED, E_FAIL, E_INVALIDARG, E_OUTOFMEMORY};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::DXGI_SAMPLE_DESC};
use windows::Win32::System::{
    Diagnostics::Debug::WriteProcessMemory, Memory::*, Threading::GetCurrentProcess,
};
use windows::core::{Interface, Result};

pub(super) struct Readback {
    pub(super) list: ID3D12CommandList,
    _allocator: ID3D12CommandAllocator,
    buffer: ID3D12Resource,
    size: usize,
    // One footprint offset per mip (including array slices) in the readback buffer.
    textures: Vec<(Arc<Texture>, Vec<u64>)>,
}

impl Readback {
    pub(super) fn record(
        device: &ID3D12Device,
        kind: D3D12_COMMAND_LIST_TYPE,
        writers: &[Arc<Texture>],
    ) -> Result<Self> {
        let mut total = 0u64;
        let mut textures = Vec::new();
        for texture in writers {
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
        let size = usize::try_from(total)
            .ok()
            .filter(|&n| n <= isize::MAX as usize)
            .ok_or(E_OUTOFMEMORY)?;
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: total,
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
        Ok(Self {
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
                tiled.clear();
                tiled.try_reserve_exact(size).map_err(|_| E_OUTOFMEMORY)?;
                tiled.resize(size, 0u8);
                valid.clear();
                valid
                    .try_reserve_exact(size.div_ceil(element))
                    .map_err(|_| E_OUTOFMEMORY)?;
                valid.resize(size.div_ceil(element), false);
                let linear = data
                    .get(offset as usize..(offset + mip.bytes) as usize)
                    .ok_or(E_INVALIDARG)?;
                texture.layout_runs(level, |target, source| {
                    valid
                        .get_mut(target.start / element..target.end / element)
                        .ok_or(E_INVALIDARG)?
                        .fill(true);
                    tiled
                        .get_mut(target)
                        .ok_or(E_INVALIDARG)?
                        .copy_from_slice(linear.get(source).ok_or(E_INVALIDARG)?);
                    Ok(())
                })?;
                let address = (texture.address + mip.source.start) as usize;
                // Coalesce adjacent texels in physical address order. Never
                // write row padding, partial-tile holes or neighbouring mips.
                let mut index = 0;
                while let Some(skip) = valid[index..].iter().position(|&present| present) {
                    let first = index + skip;
                    index = first
                        + valid[first..]
                            .iter()
                            .position(|&present| !present)
                            .unwrap_or(valid.len() - first);
                    write_mapped(address, &tiled, first * element..index * element)?;
                }
                // Include padding from guest memory in the comparison baseline:
                // upload::refresh compares the entire physical mip snapshot.
                texture.snapshot_mip(mip, &mut state.previous[level])?;
            }
            Ok(())
        });
        // SAFETY: no CPU writes were made to the GPU readback allocation.
        unsafe { self.buffer.Unmap(0, Some(&D3D12_RANGE::default())) };
        result
    }
}

/// Xbox can reserve a GPU address with no accessible CPU backing. Preserve its
/// native texture until pages are supplied. Never change guest page protection.
fn write_mapped(base: usize, bytes: &[u8], range: Range<usize>) -> Result<()> {
    let mut at = range.start;
    while at < range.end {
        let addr = base.checked_add(at).ok_or(E_INVALIDARG)?;
        let mut info = MEMORY_BASIC_INFORMATION::default();
        // SAFETY: VirtualQuery probes the address and fills a local structure.
        if unsafe {
            VirtualQuery(
                Some(addr as *const _),
                &mut info,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        } == 0
        {
            return Err(E_FAIL.into());
        }
        let region_end = (info.BaseAddress as usize)
            .checked_add(info.RegionSize)
            .ok_or(E_INVALIDARG)?;
        let end = region_end
            .checked_sub(base)
            .ok_or(E_INVALIDARG)?
            .min(range.end);
        if end <= at {
            return Err(E_INVALIDARG.into());
        }
        let access = PAGE_PROTECTION_FLAGS(info.Protect.0 & 0xff);
        if info.State == MEM_COMMIT && access != PAGE_NOACCESS {
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
            // SAFETY: read from an owned initialized slice. The OS checks the
            // guest mapping instead of constructing a Rust reference to it.
            unsafe {
                WriteProcessMemory(
                    GetCurrentProcess(),
                    addr as *const _,
                    block.as_ptr().cast(),
                    block.len(),
                    Some(&mut written),
                )?
            };
            if written != block.len() {
                return Err(E_FAIL.into());
            }
        }
        at = end;
    }
    Ok(())
}
