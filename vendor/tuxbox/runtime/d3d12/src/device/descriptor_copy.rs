//! CPU descriptor handles returned by our heaps are native on this device.
use super::Device;
use core::slice;
use windows::Win32::Graphics::Direct3D12::*;

struct Ranges<'a> {
    starts: &'a [D3D12_CPU_DESCRIPTOR_HANDLE],
    sizes: Option<&'a [u32]>,
}

impl<'a> Ranges<'a> {
    // SAFETY: the guest owns count readable entries in each supplied array
    // for the duration of the synchronous device call.
    unsafe fn read(
        count: u32,
        starts: *const D3D12_CPU_DESCRIPTOR_HANDLE,
        sizes: *const u32,
    ) -> Option<Self> {
        if count == 0 {
            return Some(Self {
                starts: &[],
                sizes: None,
            });
        }
        if starts.is_null() {
            return None;
        }
        let count = count as usize;
        Some(Self {
            starts: unsafe { slice::from_raw_parts(starts, count) },
            sizes: (!sizes.is_null()).then(|| unsafe { slice::from_raw_parts(sizes, count) }),
        })
    }

    fn spans(&self) -> impl Iterator<Item = (usize, u32)> + '_ {
        self.starts
            .iter()
            .enumerate()
            .map(|(i, handle)| (handle.ptr, self.sizes.map_or(1, |sizes| sizes[i])))
    }

    fn count(&self, stride: usize) -> Option<u64> {
        self.spans()
            .filter(|(_, size)| *size > 0)
            .try_fold(0u64, |total, (start, size)| {
                if start == 0 {
                    return None;
                }
                start.checked_add(size as usize * stride)?;
                Some(total + u64::from(size))
            })
    }

    fn handles(&self, stride: usize) -> impl Iterator<Item = D3D12_CPU_DESCRIPTOR_HANDLE> + '_ {
        self.spans().flat_map(move |(start, size)| {
            (0..size).map(move |i| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: start + i as usize * stride,
            })
        })
    }
}

pub(super) unsafe extern "system" fn ranges(
    device: &Device,
    dst_count: u32,
    dst: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    dst_sizes: *const u32,
    src_count: u32,
    src: *const D3D12_CPU_DESCRIPTOR_HANDLE,
    src_sizes: *const u32,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
) {
    // SetUAVs copies N independent sources to one contiguous destination.
    // A size array can be omitted independently on either side (one per range).
    if (0..D3D12_DESCRIPTOR_HEAP_TYPE_NUM_TYPES.0).contains(&kind.0)
        && let (Some(to), Some(from)) =
            (unsafe { Ranges::read(dst_count, dst, dst_sizes) }, unsafe {
                Ranges::read(src_count, src, src_sizes)
            })
    {
        let stride = unsafe { device.native.GetDescriptorHandleIncrementSize(kind) } as usize;
        if stride != 0
            && let Some(count) = to.count(stride)
            && Some(count) == from.count(stride)
        {
            if kind == D3D12_DESCRIPTOR_HEAP_TYPE_DSV {
                device
                    .state
                    .depth_views
                    .copy(stride, to.spans(), from.spans());
            }
            for (destination, source) in to.handles(stride).zip(from.handles(stride)) {
                unsafe { copy_host_range(device, 1, destination, source, kind) };
            }
            return;
        }
    }
    // Keep the native device's validation for malformed ranges.
    unsafe {
        device.native.CopyDescriptors(
            dst_count,
            dst,
            Some(dst_sizes),
            src_count,
            src,
            Some(src_sizes),
            kind,
        )
    };
}

pub(super) unsafe extern "system" fn simple(
    device: &Device,
    count: u32,
    dst: D3D12_CPU_DESCRIPTOR_HANDLE,
    src: D3D12_CPU_DESCRIPTOR_HANDLE,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
) {
    if kind == D3D12_DESCRIPTOR_HEAP_TYPE_DSV {
        let stride = unsafe { device.native.GetDescriptorHandleIncrementSize(kind) } as usize;
        device.state.depth_views.copy(
            stride,
            std::iter::once((dst.ptr, count)),
            std::iter::once((src.ptr, count)),
        );
    }
    unsafe { copy_host_range(device, count, dst, src, kind); }
}

unsafe fn copy_host_range(device: &Device, count: u32, dst: D3D12_CPU_DESCRIPTOR_HANDLE, src: D3D12_CPU_DESCRIPTOR_HANDLE, kind: D3D12_DESCRIPTOR_HEAP_TYPE) {
    let registry = &device.state.descriptor_arenas;
    let arena = registry.find(dst.ptr);
    let translated = arena.as_ref().map_or(dst, |a| a.cpu(dst.ptr));
    if count > 1 && kind == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV && registry.find(src.ptr).is_some() {
        let stride = unsafe { device.native.GetDescriptorHandleIncrementSize(kind) } as usize;
        for i in 0..count as usize {
            let offset = i * stride;
            unsafe { copy_host_range(device, 1, D3D12_CPU_DESCRIPTOR_HANDLE { ptr: dst.ptr + offset },
                D3D12_CPU_DESCRIPTOR_HANDLE { ptr: src.ptr + offset }, kind) };
        }
        return;
    }
    let from = registry.copy_source(src);
    if kind == D3D12_DESCRIPTOR_HEAP_TYPE_RTV {
        let stride = unsafe { device.native.GetDescriptorHandleIncrementSize(kind) } as usize;
        device.state.placements.targets.copy(stride, std::iter::once((dst.ptr, count)), std::iter::once((src.ptr, count)));
    }
    if kind == D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV {
        let stride = unsafe { device.native.GetDescriptorHandleIncrementSize(kind) } as usize;
        let to = std::iter::once((translated.ptr, count));
        let source = std::iter::once((from.ptr, count));
        device.state.imported_views.copy(stride, to.clone(), source.clone());
        device.state.blas.views.copy(stride, to.clone(), source.clone());
        crate::diagnostics::display::copy_texture_views(translated.ptr, from.ptr, count, stride);
        device.state.placements.views.copy(stride, to, source);
    }
    unsafe { device.native.CopyDescriptorsSimple(count, translated, from, kind) };
    if let Some(arena) = arena {
        arena.author(dst.ptr, count);
        arena.publish(dst.ptr, count);
    }
}
