//! Heaps, committed and placed resources, and resource sizes.
//!
//! Heaps and resources are host memory and a resource's GPU address is its
//! CPU address, as on the console; textures also get a host copy, and
//! buffers a host copy when a draw reads them (`shadow`).
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::ffi::c_void;
use std::ptr::NonNull;

use victory_formats::texture;

use crate::object::{
    self, E_INVALIDARG, E_OUTOFMEMORY, HEAP, HRESULT, Iid, Object, RESOURCE, S_OK, Table, This,
    guid,
};

/// The interfaces every device child also answers for.
pub(crate) const PAGEABLE_IIDS: [Iid; 3] = [
    guid(
        0x63ee58fb,
        0x1268,
        0x4835,
        [0x86, 0xda, 0xf0, 0x08, 0xce, 0x62, 0xf0, 0xd6],
    ),
    guid(
        0x905db94b,
        0xa00c,
        0x4140,
        [0x9d, 0xf5, 0x2b, 0x64, 0xca, 0x9e, 0xa3, 0x57],
    ),
    guid(
        0xc4fec28f,
        0x7966,
        0x4e95,
        [0x9f, 0x94, 0xf4, 0x31, 0xcb, 0x56, 0xc3, 0xb8],
    ),
];
const HEAP_IIDS: [Iid; 4] = [
    guid(
        0x6b3b2502,
        0x6e51,
        0x45b3,
        [0x90, 0xee, 0x98, 0x84, 0x26, 0x5e, 0x8d, 0xf3],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];
const RESOURCE_IIDS: [Iid; 4] = [
    guid(
        0x696442be,
        0xa72e,
        0x4059,
        [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

const KIB_64: u64 = 64 << 10;
const MIB_4: u64 = 4 << 20;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeapProperties {
    kind: u32,
    cpu_pages: u32,
    pool: u32,
    creation_nodes: u32,
    visible_nodes: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeapDesc {
    size: u64,
    properties: HeapProperties,
    alignment: u64,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct ResourceDesc {
    dimension: u32,
    alignment: u64,
    width: u64,
    height: u32,
    depth_or_array_size: u16,
    mip_levels: u16,
    format: u32,
    sample_count: u32,
    sample_quality: u32,
    layout: u32,
    flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AllocationInfo {
    size: u64,
    alignment: u64,
}

const _: () = assert!(size_of::<HeapProperties>() == 20);
const _: () = assert!(size_of::<HeapDesc>() == 48);
const _: () = assert!(size_of::<ResourceDesc>() == 56);

const BUFFER: u32 = 1;
const TEXTURE_3D: u32 = 4;

/// Bytes per element and the element's width in texels (4 for block
/// compression), by DXGI format number. `None` for 1-bit and video
/// formats. The console adds 116/117 (10-bit float color), 118-120 (16-bit
/// depth with 8-bit stencil, two planes: 3 bytes per texel), 189 (signed
/// 10-bit color), 190 (two 4-bit channels) and 191 (24/8 color).
pub(crate) fn element(format: u32) -> Option<(u64, u64)> {
    Some(match format {
        1..=4 => (16, 1),
        5..=8 => (12, 1),
        9..=22 => (8, 1),
        23..=47 | 67 | 87..=93 | 116 | 117 | 189 | 191 => (4, 1),
        118..=120 => (3, 1),
        48..=59 | 68 | 69 | 85 | 86 | 115 => (2, 1),
        60..=65 | 190 => (1, 1),
        70..=72 | 79..=81 => (8, 4),
        73..=78 | 82..=84 | 94..=99 => (16, 4),
        _ => return None,
    })
}

/// A texture's layout in console memory, as the console's layout library
/// gives it (`victory-hle`'s `xg` serves the same): the description's
/// layout field is `0x100 | swizzle mode`, 1 linear, 0 the library's pick.
pub(crate) fn texture_layout(desc: &ResourceDesc) -> Option<texture::Texture> {
    use texture::{Dimension, Swizzle, TextureDesc};
    const TEXTURE_1D: u32 = 2;
    const TEXTURE_2D: u32 = 3;
    const RENDER_TARGET: u32 = 1;
    const DEPTH: u32 = 2;
    let dimension = match desc.dimension {
        TEXTURE_1D => Dimension::Tex1d,
        TEXTURE_2D => Dimension::Tex2d,
        TEXTURE_3D => Dimension::Tex3d,
        _ => return None,
    };
    let swizzle = match desc.layout {
        0 if dimension == Dimension::Tex1d => Swizzle::LINEAR,
        0 if desc.flags & DEPTH != 0 => Swizzle::SW_64KB_Z_X,
        0 if desc.flags & RENDER_TARGET != 0 => Swizzle::SW_64KB_R_X,
        0 => Swizzle::SW_4KB_S,
        1 => Swizzle::LINEAR,
        l if l & !0xff == 0x100 => Swizzle(l & 0xff),
        _ => return None,
    };
    texture::Texture::new(&TextureDesc {
        dimension,
        width: u32::try_from(desc.width).ok()?,
        height: desc.height,
        depth_or_array: u32::from(desc.depth_or_array_size),
        mips: u32::from(desc.mip_levels),
        format: desc.format,
        samples: desc.sample_count.max(1),
        swizzle,
    })
    .ok()
}

/// Bytes and alignment a resource takes: buffers their width (64 KiB
/// aligned), textures their console layout (formats it does not know:
/// every mip of every array slice and sample, packed, 64 KiB aligned),
/// rounded up to the alignment.
fn footprint(desc: &ResourceDesc) -> Option<AllocationInfo> {
    if desc.dimension != BUFFER
        && let Some(t) = texture_layout(desc)
    {
        // As the console: the layout's own alignment unless more is asked
        // (4 KiB for 4 KiB modes, where PC says 64 KiB). Grand Theft Auto
        // VI keeps these sizes in its texture tables, and its streaming
        // asserts when they disagree with the streamed layout.
        let alignment = desc.alignment.max(t.alignment);
        if !alignment.is_power_of_two() {
            return None;
        }
        return Some(AllocationInfo {
            size: t.size.checked_next_multiple_of(alignment)?,
            alignment,
        });
    }
    let bytes = if desc.dimension == BUFFER {
        desc.width
    } else {
        let (element, block) = element(desc.format)?;
        let (depth, slices) = if desc.dimension == TEXTURE_3D {
            (u64::from(desc.depth_or_array_size), 1)
        } else {
            (1, u64::from(desc.depth_or_array_size))
        };
        let height = u64::from(desc.height);
        let mips = mip_count(desc);
        let mut chain = 0u64;
        for mip in 0..mips.min(64) {
            let w = (desc.width >> mip).max(1).div_ceil(block);
            let h = (height >> mip).max(1).div_ceil(block);
            let d = (depth >> mip).max(1);
            chain = chain.checked_add(w.checked_mul(h)?.checked_mul(d)?.checked_mul(element)?)?;
        }
        chain
            .checked_mul(slices)?
            .checked_mul(u64::from(desc.sample_count.max(1)))?
    };
    if bytes == 0 {
        return None;
    }
    let minimum = if desc.sample_count > 1 { MIB_4 } else { KIB_64 };
    let alignment = desc.alignment.max(minimum);
    if !alignment.is_power_of_two() {
        return None;
    }
    Some(AllocationInfo {
        size: bytes.checked_next_multiple_of(KIB_64)?,
        alignment,
    })
}

/// A format as the host has it. The console's 16-bit depth with stencil
/// (and its typeless views) has no PC format: the 24-bit ones stand in.
pub(crate) fn pc_format(format: u32) -> u32 {
    match format {
        118 => 45,
        119 => 46,
        120 => 47,
        other => other,
    }
}

/// R8G8B8A8_UNORM, which stands in for a packed 16-bit render target the
/// host cannot make (see [`target_format`]).
const RGBA8_UNORM: u32 = 28;

/// The console's packed 16-bit colour formats a title renders to:
/// B5G5R5A1_UNORM (86) and B4G4R4A4_UNORM (115).
const PACKED_16: [u32; 2] = [86, 115];

/// Whether the host GPU lacks render-target or typed unordered-access
/// support for `format` (asked once per format). NVIDIA has neither for
/// B4G4R4A4 and no typed UAV for B5G5R5A1, which Grand Theft Auto VI's
/// 2560x1440 UI targets need; AMD has both.
#[cfg(windows)]
fn host_lacks_target(format: u32) -> bool {
    use std::sync::OnceLock;
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT;
    static LACKS: [OnceLock<bool>; 2] = [OnceLock::new(), OnceLock::new()];
    let Some(i) = PACKED_16.iter().position(|&f| f == format) else {
        return false;
    };
    *LACKS[i].get_or_init(|| {
        let Some(host) = crate::host::host() else { return false };
        let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
            Format: DXGI_FORMAT(format as i32),
            ..Default::default()
        };
        // SAFETY: the structure the feature names, ours.
        let asked = unsafe {
            host.device.CheckFeatureSupport(
                D3D12_FEATURE_FORMAT_SUPPORT,
                (&raw mut support).cast(),
                size_of::<D3D12_FEATURE_DATA_FORMAT_SUPPORT>() as u32,
            )
        };
        let needed = D3D12_FORMAT_SUPPORT1_RENDER_TARGET.0 | D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW.0;
        let lacks = asked.is_err() || support.Support1.0 & needed != needed;
        if lacks {
            tracing::info!(format, "packed 16-bit render targets are R8G8B8A8_UNORM on this GPU");
        }
        lacks
    })
}

#[cfg(not(windows))]
fn host_lacks_target(_: u32) -> bool {
    false
}

/// The format a render target or unordered-access texture of `format` is
/// made in on the host, and that render target views and pipelines name:
/// R8G8B8A8_UNORM for a packed 16-bit format the GPU cannot render to or
/// write typed (each channel keeps its value, with more precision); else
/// the format as the host has it. Textures only sampled keep theirs.
pub(crate) fn target_format(format: u32) -> u32 {
    let format = pc_format(format);
    if PACKED_16.contains(&format) && host_lacks_target(format) {
        RGBA8_UNORM
    } else {
        format
    }
}

/// The typeless format of a format's family (itself if it has none).
pub(crate) fn family(format: u32) -> u32 {
    const FAMILIES: [(u32, u32); 22] = [
        (1, 4),
        (5, 8),
        (9, 14),
        (15, 18),
        (19, 22),
        (23, 25),
        (27, 32),
        (33, 38),
        (39, 43),
        (44, 47),
        (48, 52),
        (53, 59),
        (60, 65),
        (70, 72),
        (73, 75),
        (76, 78),
        (79, 81),
        (82, 84),
        (90, 91),
        (92, 93),
        (94, 96),
        (97, 99),
    ];
    match format {
        87 => 90,
        88 => 92,
        f => FAMILIES
            .iter()
            .find(|&&(first, last)| (first..=last).contains(&f))
            .map_or(f, |&(first, _)| first),
    }
}

/// sRGB formats and their linear twins.
pub(crate) const SRGB: [(u32, u32); 3] = [(29, 28), (91, 87), (93, 88)];

/// The format a view of a title texture must name when its host copy is
/// typeless (see [`DEPTH_FORMATS`], [`SRGB`]): the title's own; `None` if
/// the host copy is typed.
pub(crate) fn view_format(this: This) -> Option<u32> {
    let own = pc_format(resource(this).desc.format);
    let host = host_resource(this)?;
    // SAFETY: a live resource.
    let format = unsafe { host.GetDesc() }.Format.0 as u32;
    (format != own).then_some(own)
}

/// Depth formats with their typeless family: depth textures that shaders
/// read are made typeless on the host, which only allows views of
/// typeless depth resources.
pub(crate) const DEPTH_FORMATS: [(u32, u32); 4] = [(20, 19), (40, 39), (45, 44), (55, 53)];

/// Mip levels, the full chain for 0.
fn mip_count(desc: &ResourceDesc) -> u32 {
    let depth = if desc.dimension == TEXTURE_3D {
        u64::from(desc.depth_or_array_size)
    } else {
        1
    };
    match desc.mip_levels {
        0 => {
            64 - desc
                .width
                .max(u64::from(desc.height))
                .max(depth)
                .max(1)
                .leading_zeros()
        }
        n => u32::from(n),
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PlacedFootprint {
    pub(crate) offset: u64,
    pub(crate) format: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) depth: u32,
    pub(crate) row_pitch: u32,
}

impl PlacedFootprint {
    /// `(lines, row_bytes, total_bytes)` spanned in a buffer: every row
    /// pitched but the last.
    pub(crate) fn layout(&self) -> Option<(u64, u64, u64)> {
        let (element, block) = element(self.format)?;
        let lines = u64::from(self.height)
            .div_ceil(block)
            .checked_mul(u64::from(self.depth))?;
        let row_bytes = u64::from(self.width).div_ceil(block).checked_mul(element)?;
        if lines == 0 {
            return Some((0, row_bytes, 0));
        }
        let total = u64::from(self.row_pitch)
            .checked_mul(lines - 1)?
            .checked_add(row_bytes)?;
        Some((lines, row_bytes, total))
    }

    /// The bytes it spans in a buffer: every row pitched but the last.
    pub(crate) fn bytes(&self) -> Option<u64> {
        self.layout().map(|(_, _, total)| total)
    }
}
const _: () = assert!(size_of::<PlacedFootprint>() == 32);

const NO_FOOTPRINT: PlacedFootprint = PlacedFootprint {
    offset: u64::MAX,
    format: 0,
    width: 0,
    height: 0,
    depth: 0,
    row_pitch: 0,
};

/// One subresource's size in an upload buffer: footprint (offset 0), rows
/// and bytes per row. Planes of depth-stencil formats are not told apart.
fn subresource(desc: &ResourceDesc, index: u32) -> Option<(PlacedFootprint, u32, u64)> {
    const PITCH: u64 = 256;
    if desc.dimension == BUFFER {
        let footprint = PlacedFootprint {
            offset: 0,
            format: 0,
            width: u32::try_from(desc.width).ok()?,
            height: 1,
            depth: 1,
            row_pitch: u32::try_from(desc.width.next_multiple_of(PITCH)).ok()?,
        };
        return (index == 0).then_some((footprint, 1, desc.width));
    }
    let mips = mip_count(desc);
    let slices = if desc.dimension == TEXTURE_3D {
        1
    } else {
        u32::from(desc.depth_or_array_size).max(1)
    };
    let (mip, slice) = (index % mips, index / mips);
    if slice >= slices {
        return None;
    }
    let (element, block) = element(desc.format)?;
    let width = (desc.width >> mip).max(1).next_multiple_of(block);
    let height = (u64::from(desc.height) >> mip)
        .max(1)
        .next_multiple_of(block);
    let depth = if desc.dimension == TEXTURE_3D {
        (u32::from(desc.depth_or_array_size) >> mip).max(1)
    } else {
        1
    };
    let row_size = width / block * element;
    let footprint = PlacedFootprint {
        offset: 0,
        format: desc.format,
        width: u32::try_from(width).ok()?,
        height: u32::try_from(height).ok()?,
        depth,
        row_pitch: u32::try_from(row_size.next_multiple_of(PITCH)).ok()?,
    };
    Some((footprint, u32::try_from(height / block).ok()?, row_size))
}

/// Device method: where each subresource goes in an upload buffer, rows
/// 256-byte and subresources 512-byte aligned, as on PC. Unknown sizes give
/// the documented all-ones values.
pub(crate) extern "system" fn copyable_footprints(
    _: This,
    desc: *const ResourceDesc,
    first: u32,
    count: u32,
    base: u64,
    layouts: *mut PlacedFootprint,
    rows: *mut u32,
    row_sizes: *mut u64,
    total: *mut u64,
) {
    const PLACEMENT: u64 = 512;
    // SAFETY: the title passes a description (null is tolerated).
    let desc = unsafe { desc.as_ref() };
    let mut end = base;
    let mut known = desc.is_some();
    for i in 0..count {
        let one = desc.and_then(|d| subresource(d, first + i));
        let (layout, row_count, row_size) = match one {
            Some((mut footprint, row_count, row_size)) => {
                footprint.offset = end.next_multiple_of(PLACEMENT);
                let pitch = u64::from(footprint.row_pitch);
                let lines = u64::from(row_count) * u64::from(footprint.depth);
                end = footprint.offset + pitch * lines.saturating_sub(1) + row_size;
                (footprint, row_count, row_size)
            }
            None => {
                known = false;
                (NO_FOOTPRINT, u32::MAX, u64::MAX)
            }
        };
        let i = i as usize;
        // SAFETY: each output is null or holds `count` entries.
        unsafe {
            if !layouts.is_null() {
                layouts.add(i).write(layout);
            }
            if !rows.is_null() {
                rows.add(i).write(row_count);
            }
            if !row_sizes.is_null() {
                row_sizes.add(i).write(row_size);
            }
        }
    }
    if !known {
        tracing::warn!(?desc, "no footprint for this resource");
    }
    if !total.is_null() {
        // SAFETY: checked non-null.
        unsafe { total.write(if known { end - base } else { u64::MAX }) };
    }
}

/// Device method: the footprint of resources placed one after another.
/// Returns a 16-byte value, through a hidden first argument. Invalid input
/// gives the documented size `u64::MAX`.
pub(crate) extern "system" fn allocation_info(
    out: *mut AllocationInfo,
    _: This,
    _visible: u32,
    count: u32,
    descs: *const ResourceDesc,
) -> *mut AllocationInfo {
    let descs = if descs.is_null() {
        &[][..]
    } else {
        // SAFETY: the title passes `count` descriptions.
        unsafe { std::slice::from_raw_parts(descs, count as usize) }
    };
    // SAFETY: the caller passes room for the value.
    unsafe { out.write(place(descs.iter(), |_, _| ())) };
    out
}

/// The newer description: the plain one, then a sampler feedback mip
/// region (width, height, depth).
#[repr(C)]
pub(crate) struct ResourceDesc1 {
    desc: ResourceDesc,
    mip_region: [u32; 3],
}
const _: () = assert!(size_of::<ResourceDesc1>() == 72);

/// Where one resource goes in [`allocation_info2`]'s placement.
#[repr(C)]
pub(crate) struct AllocationInfo1 {
    offset: u64,
    alignment: u64,
    size: u64,
}

/// Device method 0x360 (`ID3D12Device8`): [`allocation_info`] for newer
/// descriptions, also telling where each resource goes.
pub(crate) extern "system" fn allocation_info2(
    out: *mut AllocationInfo,
    _: This,
    _visible: u32,
    count: u32,
    descs: *const ResourceDesc1,
    each: *mut AllocationInfo1,
) -> *mut AllocationInfo {
    let descs = if descs.is_null() {
        &[][..]
    } else {
        // SAFETY: the title passes `count` descriptions.
        unsafe { std::slice::from_raw_parts(descs, count as usize) }
    };
    let total = place(descs.iter().map(|d| &d.desc), |i, info| {
        if !each.is_null() {
            // SAFETY: the title passes room for `count` entries.
            unsafe { each.add(i).write(info) };
        }
    });
    // SAFETY: the caller passes room for the value.
    unsafe { out.write(total) };
    out
}

/// Resources placed one after another: the whole footprint, telling `each`
/// where every resource goes. Unknown sizes give the documented `u64::MAX`.
fn place<'a>(
    descs: impl Iterator<Item = &'a ResourceDesc>,
    mut each: impl FnMut(usize, AllocationInfo1),
) -> AllocationInfo {
    let mut total = AllocationInfo {
        size: 0,
        alignment: KIB_64,
    };
    for (i, desc) in descs.enumerate() {
        let Some(one) = footprint(desc) else {
            tracing::warn!(?desc, "no size for this resource");
            each(
                i,
                AllocationInfo1 {
                    offset: u64::MAX,
                    alignment: 0,
                    size: u64::MAX,
                },
            );
            return AllocationInfo {
                size: u64::MAX,
                alignment: 0,
            };
        };
        tracing::debug!(?desc, ?one, "allocation info");
        let offset = total.size.next_multiple_of(one.alignment);
        each(
            i,
            AllocationInfo1 {
                offset,
                alignment: one.alignment,
                size: one.size,
            },
        );
        total.size = offset + one.size;
        total.alignment = total.alignment.max(one.alignment);
    }
    total
}

/// Zeroed host memory standing in for GPU memory.
pub(crate) struct Memory {
    ptr: NonNull<u8>,
    layout: Layout,
    /// Set when the memory came from the title's allocator, with these
    /// allocation attributes (it goes back there).
    attributes: Option<u64>,
}

pub type AllocRoutine = extern "system" fn(usize, u64) -> *mut c_void;
pub type FreeRoutine = extern "system" fn(*mut c_void, u64);
static ALLOCATOR: std::sync::OnceLock<(AllocRoutine, FreeRoutine)> = std::sync::OnceLock::new();

/// Committed resources get their memory from the title's general
/// allocator, as on the console, where titles hook it to place graphics
/// memory themselves (Grand Theft Auto VI places its back buffers in a
/// block of its own and checks they fill it). The runtime names it before
/// the device exists.
pub fn use_allocator(alloc: AllocRoutine, free: FreeRoutine) {
    let _ = ALLOCATOR.set((alloc, free));
}

/// Maps the physical pages behind one range of title memory at another,
/// `(to, from, len)` (`from` 0 unmaps); false on failure.
pub type PageMapper = fn(u64, u64, u64) -> bool;
static PAGE_MAPPER: std::sync::OnceLock<PageMapper> = std::sync::OnceLock::new();

/// The runtime's page mapper, for the queues' page-mapping copies (memory
/// is the runtime's, as on the console).
pub fn use_page_mapper(mapper: PageMapper) {
    let _ = PAGE_MAPPER.set(mapper);
}

pub(crate) fn page_mapper() -> Option<PageMapper> {
    PAGE_MAPPER.get().copied()
}

/// Allocation attributes for resource memory: allocator id, page size
/// (bits 14-15: 2 MiB pages from 2 MiB up, else 64 KiB), alignment as log2
/// (bits 24-28) and memory type (bits 29-32: write-combined for GPU-only
/// heaps, cacheable for the CPU-visible ones).
/// shortcut: the console runtime's own choice of page size and memory type
/// is not known; this is the plausible one.
fn resource_attributes(info: AllocationInfo, heap_kind: u32) -> u64 {
    const GRAPHICS_ALLOCATOR: u64 = 128 << 16;
    const DEFAULT_HEAP: u32 = 1;
    let pages: u64 = if info.size >= 2 << 20 { 2 } else { 1 };
    let memory: u64 = if heap_kind == DEFAULT_HEAP { 1 } else { 3 };
    let alignment = u64::from(info.alignment.max(1).trailing_zeros());
    GRAPHICS_ALLOCATOR | pages << 14 | alignment << 24 | memory << 29
}

impl Memory {
    /// A committed resource's memory: from the title's allocator if the
    /// runtime named one, else our own; zeroed either way.
    fn for_resource(info: AllocationInfo, heap_kind: u32) -> Result<Memory, HRESULT> {
        let Some(&(alloc, _)) = ALLOCATOR.get() else {
            return Memory::new(info);
        };
        let layout = Layout::from_size_align(info.size as usize, info.alignment as usize)
            .map_err(|_| E_INVALIDARG)?;
        let attributes = resource_attributes(info, heap_kind);
        let ptr =
            NonNull::new(alloc(layout.size(), attributes).cast::<u8>()).ok_or(E_OUTOFMEMORY)?;
        // SAFETY: the allocator handed out `size` bytes.
        unsafe { ptr.as_ptr().write_bytes(0, layout.size()) };
        crate::memstat::add(&crate::memstat::TITLE_GPU, layout.size() as u64);
        Ok(Memory {
            ptr,
            layout,
            attributes: Some(attributes),
        })
    }

    /// `size` zeroed bytes aligned to `alignment` (a power of two).
    pub(crate) fn zeroed(size: u64, alignment: u64) -> Result<Memory, HRESULT> {
        Memory::new(AllocationInfo { size, alignment })
    }

    pub(crate) fn address(&self) -> u64 {
        self.ptr.as_ptr() as u64
    }

    fn new(info: AllocationInfo) -> Result<Memory, HRESULT> {
        let layout = usize::try_from(info.size)
            .ok()
            .zip(usize::try_from(info.alignment).ok())
            .filter(|&(size, _)| size > 0)
            .and_then(|(size, align)| Layout::from_size_align(size, align).ok())
            .ok_or(E_INVALIDARG)?;
        // SAFETY: the size is non-zero.
        let ptr = NonNull::new(unsafe { alloc_zeroed(layout) }).ok_or(E_OUTOFMEMORY)?;
        crate::memstat::add(&crate::memstat::TITLE_GPU, layout.size() as u64);
        Ok(Memory {
            ptr,
            layout,
            attributes: None,
        })
    }
}

impl Drop for Memory {
    fn drop(&mut self) {
        crate::memstat::sub(&crate::memstat::TITLE_GPU, self.layout.size() as u64);
        match (self.attributes, ALLOCATOR.get()) {
            (Some(attributes), Some(&(_, free))) => free(self.ptr.as_ptr().cast(), attributes),
            // SAFETY: allocated in `new` with this layout.
            _ => unsafe { dealloc(self.ptr.as_ptr(), self.layout) },
        }
    }
}

#[repr(C)]
struct Heap {
    object: Object,
    desc: HeapDesc,
    memory: Memory,
}

pub(crate) extern "system" fn create_heap(
    _: This,
    desc: *const HeapDesc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    tracing::debug!(
        size = desc.size,
        kind = desc.properties.kind,
        flags = format_args!("{:#x}", desc.flags),
        "heap"
    );
    if desc.alignment != 0 && !desc.alignment.is_power_of_two() {
        return E_INVALIDARG;
    }
    let info = AllocationInfo {
        size: desc.size,
        alignment: desc.alignment.max(KIB_64),
    };
    let memory = match Memory::new(info) {
        Ok(memory) => memory,
        Err(hr) => return hr,
    };
    let heap = Box::new(Heap {
        object: Object::new(&HEAP_FUNCTIONS),
        desc,
        memory,
    });
    object::hand_out(HEAP, heap, iid, &HEAP_IIDS, out)
}

extern "system" fn heap_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(HEAP, iid, object::is(iid, &HEAP_IIDS).then_some(this), out)
}
extern "system" fn heap_release(this: This) -> u32 {
    object::release_boxed::<Heap>(this)
}
extern "system" fn heap_desc(this: This, out: *mut HeapDesc) {
    if !out.is_null() {
        // SAFETY: `this` is a heap; the title passes room for a description.
        unsafe { out.write((*this.cast::<Heap>()).desc) };
    }
}

/// A resource's memory: its own, or a range of a heap it holds a reference
/// to.
enum Backing {
    Committed(Memory),
    Placed {
        heap: This,
        offset: u64,
    },
    /// Memory the title mapped itself, at this address.
    Address(*mut u8),
}

impl Drop for Backing {
    fn drop(&mut self) {
        if let Backing::Placed { heap, .. } = *self {
            heap_release(heap);
        }
    }
}

#[repr(C)]
struct Resource {
    object: Object,
    desc: ResourceDesc,
    heap: HeapProperties,
    heap_flags: u32,
    backing: Backing,
    /// Textures also live on the host GPU (decision 0007); buffers do not
    /// yet.
    #[cfg(windows)]
    host: Option<std::sync::Arc<HostTexture>>,
}

/// A title texture's host copy and the state it is in. The title's barriers
/// name console states (looser than the host's), so we transition from this
/// one. Lists that use the texture hold it until they are reset: the title
/// may release a resource whose commands it recorded but not yet submitted
/// (console memory outlives the object; a host texture must outlive the
/// lists that name it).
#[cfg(windows)]
pub(crate) struct HostTexture {
    pub(crate) texture: windows::Win32::Graphics::Direct3D12::ID3D12Resource,
    pub(crate) state: std::sync::atomic::AtomicU32,
    /// What the driver allocates for it (counted in [`crate::memstat`]).
    pub(crate) bytes: u64,
    /// Its memory, if in a shared heap.
    pub(crate) slot: Option<crate::texpool::Slot>,
    /// The title's largest mips it goes without ([`over_cap`]).
    pub(crate) skip: u32,
    /// When a list last moved it to a state shaders or draws write in, in
    /// order: of textures in the same memory, the last holds its contents.
    pub(crate) written: std::sync::atomic::AtomicU64,
    /// Planes shaders write as images, for depth textures the title also
    /// writes so ([`crate::twin`]); made on the first such view.
    pub(crate) twin: std::sync::OnceLock<Option<crate::twin::Twin>>,
}

/// Textures the host GPU writes, by the title memory they are placed in.
/// Titles read such memory through other views, with no resource object:
/// Grand Theft Auto VI writes its HDR images as 32-bit integers and reads
/// them as shared-exponent colors.
#[cfg(windows)]
static TARGETS: std::sync::Mutex<std::collections::BTreeMap<u64, Vec<usize>>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Shared-exponent colors (R9G9B9E5), which 32-bit textures that shaders
/// write may be read as.
pub(crate) const SHARED_EXPONENT: u32 = 67;

/// The written texture at `address` that a view described by `d` reads, the
/// last written of several.
#[cfg(windows)]
fn target_at(address: u64, d: &ResourceDesc) -> Option<This> {
    let targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    targets
        .get(&address)?
        .iter()
        .map(|&r| r as This)
        .filter(|&r| {
            let o = &resource(r).desc;
            (o.dimension, o.width, o.height, o.depth_or_array_size, o.mip_levels)
                == (d.dimension, d.width, d.height, d.depth_or_array_size, d.mip_levels)
                && (family(o.format) == family(d.format)
                    || d.format == SHARED_EXPONENT && (39..=43).contains(&o.format))
        })
        .max_by_key(|&r| {
            resource(r)
                .host
                .as_ref()
                .map_or(0, |h| h.written.load(std::sync::atomic::Ordering::Relaxed))
        })
}

/// Every texture the host GPU writes, by title address (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn targets_text() -> String {
    let targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    let mut text = String::new();
    for (at, list) in targets.iter() {
        for &r in list {
            let o = &resource(r as This).desc;
            let written = resource(r as This)
                .host
                .as_ref()
                .map_or(0, |h| h.written.load(std::sync::atomic::Ordering::Relaxed));
            text.push_str(&format!(
                "target {at:#x} {r:#x} {}x{}x{} mips {} f{} flags {:#x} layout {:#x} written {written}
",
                o.width, o.height, o.depth_or_array_size, o.mip_levels, o.format, o.flags, o.layout
            ));
        }
    }
    text
}

/// Where the title has the texture `r`, and the host resource object (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn data_of(r: This) -> u64 {
    resource(r).data() as u64
}

/// A list moves `texture` to `state`.
#[cfg(windows)]
pub(crate) fn note_written(resource_: This, state: u32) {
    static CLOCK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    const RENDER_TARGET: u32 = 0x4;
    const UNORDERED: u32 = 0x8;
    if state & (RENDER_TARGET | UNORDERED) != 0
        && let Some(h) = &resource(resource_).host
    {
        let now = CLOCK.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        h.written.store(now, std::sync::atomic::Ordering::Relaxed);
    }
}

/// `desc` as a texture that 32-bit and shared-exponent views may be made of;
/// in the common state.
#[cfg(windows)]
fn castable(
    device: &windows::Win32::Graphics::Direct3D12::ID3D12Device,
    desc: &windows::Win32::Graphics::Direct3D12::D3D12_RESOURCE_DESC,
    clear: Option<*const windows::Win32::Graphics::Direct3D12::D3D12_CLEAR_VALUE>,
) -> Option<windows::Win32::Graphics::Direct3D12::ID3D12Resource> {
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;
    use windows::core::Interface;
    static DEVICE: std::sync::OnceLock<Option<ID3D12Device10>> = std::sync::OnceLock::new();
    let device = DEVICE
        .get_or_init(|| {
            let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS12::default();
            // SAFETY: a structure of the size the feature has.
            unsafe {
                device
                    .CheckFeatureSupport(
                        D3D12_FEATURE_D3D12_OPTIONS12,
                        std::ptr::from_mut(&mut options).cast(),
                        std::mem::size_of_val(&options) as u32,
                    )
                    .ok()?;
            }
            if !options.RelaxedFormatCastingSupported.as_bool() {
                tracing::warn!("the host cannot read 32-bit textures as shared-exponent colors");
                return None;
            }
            device.cast().ok()
        })
        .as_ref()?;
    let desc = D3D12_RESOURCE_DESC1 {
        Dimension: desc.Dimension,
        Alignment: desc.Alignment,
        Width: desc.Width,
        Height: desc.Height,
        DepthOrArraySize: desc.DepthOrArraySize,
        MipLevels: desc.MipLevels,
        Format: desc.Format,
        SampleDesc: desc.SampleDesc,
        Layout: desc.Layout,
        Flags: desc.Flags,
        ..Default::default()
    };
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let formats = [
        DXGI_FORMAT_R32_UINT,
        DXGI_FORMAT_R32_FLOAT,
        DXGI_FORMAT_R32_SINT,
        DXGI_FORMAT_R9G9B9E5_SHAREDEXP,
    ];
    let mut texture: Option<ID3D12Resource> = None;
    // SAFETY: whole descriptions.
    unsafe {
        device.CreateCommittedResource3(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_BARRIER_LAYOUT_COMMON,
            clear,
            None,
            Some(&formats),
            &mut texture,
        )
    }
    .ok()?;
    texture
}

#[cfg(windows)]
impl Drop for HostTexture {
    fn drop(&mut self) {
        crate::memstat::sub(&crate::memstat::HOST_TEXTURES, self.bytes);
        crate::memstat::sub(&crate::memstat::HOST_TEXTURE_COUNT, 1);
        #[cfg(feature = "diagnostics")]
        census(&self.texture, -(self.bytes as i64));
        // A submission recorded but not yet run may still name it.
        crate::host::retire((self.texture.clone(), self.slot.take()));
    }
}

/// Host textures by what they are: size, format, flags; count and bytes.
#[cfg(all(windows, feature = "diagnostics"))]
type Census = std::collections::HashMap<(u64, u32, u16, u16, i32, u32), (i64, i64)>;
#[cfg(all(windows, feature = "diagnostics"))]
static CENSUS: std::sync::LazyLock<std::sync::Mutex<Census>> =
    std::sync::LazyLock::new(Default::default);

#[cfg(all(windows, feature = "diagnostics"))]
fn census(texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource, bytes: i64) {
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let key = (d.Width, d.Height, d.DepthOrArraySize, d.MipLevels, d.Format.0, d.Flags.0 as u32);
    let mut all = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    let e = all.entry(key).or_default();
    e.0 += bytes.signum();
    e.1 += bytes;
}

/// The kinds of host textures that take most (diagnostics).
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn census_log() {
    let all = CENSUS.lock().unwrap_or_else(|e| e.into_inner());
    let mut v: Vec<_> = all.iter().collect();
    v.sort_by_key(|(_, e)| -e.1);
    for (k, e) in v.iter().take(40) {
        tracing::info!("textures {k:?}: {} of {} MiB", e.0, e.1 >> 20);
    }
    let targets: i64 = v.iter().filter(|(k, _)| k.5 & 7 != 0).map(|(_, e)| e.1).sum();
    let small: i64 = v.iter().filter(|(_, e)| e.1 / e.0.max(1) <= 65536).map(|(_, e)| e.1).sum();
    tracing::info!("textures with target or write flags {} MiB, of 64 KiB each {} MiB", targets >> 20, small >> 20);
}

/// The driver's allocation size for `texture`.
#[cfg(windows)]
fn allocation_bytes(texture: &windows::Win32::Graphics::Direct3D12::ID3D12Resource) -> u64 {
    let Some(host) = crate::host::host() else {
        return 0;
    };
    // SAFETY: a live resource and its own description.
    unsafe {
        let desc = texture.GetDesc();
        host.device
            .GetResourceAllocationInfo(0, &[desc])
            .SizeInBytes
    }
}

/// Moves the host texture to `wanted` (a host texture state); the host
/// resource and the state it leaves, if that is a change.
#[cfg(windows)]
pub(crate) fn host_transition(this: This, wanted: u32) -> Option<(*mut c_void, u32)> {
    use windows::core::Interface;
    let host = resource(this).host.as_ref()?;
    let before = host
        .state
        .swap(wanted, std::sync::atomic::Ordering::Relaxed);
    (before != wanted).then(|| (host.texture.as_raw(), before))
}

/// The host GPU's copy of a title resource, if it has one.
#[cfg(windows)]
pub(crate) fn host_resource<'a>(
    this: This,
) -> Option<&'a windows::Win32::Graphics::Direct3D12::ID3D12Resource> {
    if this.is_null() {
        return None;
    }
    resource(this).host.as_deref().map(|h| &h.texture)
}

/// The twins of a depth texture the title also writes as an image (see
/// [`crate::twin`]), made on first use; `None` for other textures.
#[cfg(windows)]
pub(crate) fn depth_twin<'a>(this: This) -> Option<&'a crate::twin::Twin> {
    if this.is_null() {
        return None;
    }
    let r = resource(this);
    if !crate::twin::wanted(r.desc.format, r.desc.flags) {
        return None;
    }
    let host = r.host.as_deref()?;
    host.twin.get_or_init(|| crate::twin::make(&host.texture)).as_ref()
}

/// A title texture's host copy, shared, for a list to hold.
#[cfg(windows)]
pub(crate) fn host_copy(this: This) -> Option<std::sync::Arc<HostTexture>> {
    if this.is_null() {
        return None;
    }
    resource(this).host.clone()
}

#[cfg(windows)]
thread_local! {
    /// The texture being made is one the title placed in memory it streams.
    static STREAMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Of it, the largest mip the title's view reads: memory of larger ones
    /// may be another texture's.
    static LARGEST: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// How much of the video memory we may use is in use, in percent, as last
/// asked a little while ago.
#[cfg(windows)]
fn video_memory_percent() -> u64 {
    static LAST: std::sync::Mutex<(Option<std::time::Instant>, u64)> =
        std::sync::Mutex::new((None, 0));
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if last.0.is_none_or(|at| at.elapsed() > std::time::Duration::from_millis(50)) {
        last.0 = Some(std::time::Instant::now());
        if let Some([(used, budget), _]) = crate::host::video_memory() {
            last.1 = used * 100 / budget.max(1);
        }
    }
    last.1
}

/// How many of a streamed texture's largest mips to do without, for none of
/// its sides to pass `VICTORY_TEXTURE_MAX`: by default 1024, 512 and then 256 as
/// video memory fills up. All textures together did not fit, and paging them
/// cost seconds a frame, or the driver failed.
#[cfg(windows)]
fn over_cap(d: &ResourceDesc) -> u32 {
    static SET: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    let set = *SET.get_or_init(|| {
        std::env::var("VICTORY_TEXTURE_MAX")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
    });
    if !STREAMED.get() || textures() != Textures::Stream {
        return 0;
    }
    let max = set.unwrap_or_else(|| match video_memory_percent() {
        0..70 => 1024,
        70..85 => 512,
        _ => 256,
    });
    const TEXTURE2D: u32 = 3;
    let block = (70..=84).contains(&d.format) || (94..=99).contains(&d.format);
    if cfg!(test)
        || max == 0
        || d.dimension != TEXTURE2D
        || d.flags & 7 != 0
        || !block
        || d.mip_levels < 2
    {
        return 0;
    }
    let mut skip = 0;
    while (d.width.max(u64::from(d.height)) >> skip) > max
        && skip + 1 < u32::from(d.mip_levels)
        && (d.width >> (skip + 1)).is_multiple_of(4)
        && (d.height >> (skip + 1)).is_multiple_of(4)
        && d.width >> (skip + 1) > 0
        && d.height >> (skip + 1) > 0
    {
        skip += 1;
    }
    skip
}

/// The title's largest mips the host copy of a texture goes without.
#[cfg(windows)]
pub(crate) fn skipped_mips(this: This) -> u32 {
    if this.is_null() || resource(this).desc.dimension == BUFFER {
        return 0;
    }
    resource(this).host.as_ref().map_or(0, |h| h.skip)
}

/// A host texture like the title's: same description, in GPU memory,
/// without the console's layout and flags.
#[cfg(windows)]
fn host_texture(
    desc: &ResourceDesc,
    state: u32,
    clear: *const c_void,
) -> Option<(
    windows::Win32::Graphics::Direct3D12::ID3D12Resource,
    Option<crate::texpool::Slot>,
    u32,
    bool,
)> {
    use windows::Win32::Graphics::Direct3D12::*;
    const PC_FLAGS: u32 = 0xf; // render target, depth, UAV, no shader access
    if desc.dimension == BUFFER {
        return None;
    }
    let host = crate::host::host()?;
    let mut d = *desc;
    d.layout = 0;
    d.alignment = 0;
    let skip = over_cap(&d).max(LARGEST.get().min(u32::from(d.mip_levels) - 1));
    d.width >>= skip;
    d.height >>= skip;
    d.mip_levels -= skip as u16;
    d.flags &= PC_FLAGS;
    // PC refuses depth targets that are also unordered-access (the console
    // allows both; Grand Theft Auto VI's shadow maps ask for it).
    // shortcut: the host copy loses UAV use; give it a second view-only
    // copy if draws write it as a UAV.
    const DEPTH: u32 = 2;
    const UNORDERED: u32 = 4;
    if d.flags & DEPTH != 0 {
        d.flags &= !UNORDERED;
    }
    const RENDER_TARGET: u32 = 1;
    d.format = if d.flags & (RENDER_TARGET | UNORDERED) != 0 {
        target_format(d.format)
    } else {
        pc_format(d.format)
    };
    const DENY_SHADER_RESOURCE: u32 = 8;
    if d.flags & DENY_SHADER_RESOURCE == 0
        && let Some(&(_, typeless)) = DEPTH_FORMATS.iter().find(|&&(f, _)| f == d.format)
    {
        d.format = typeless;
    }
    // The host has no sRGB unordered access: such textures are typeless,
    // their unordered access views plain UNORM.
    if d.flags & UNORDERED != 0 && SRGB.iter().any(|&(srgb, _)| srgb == d.format) {
        d.format = family(d.format);
    }
    // The clear value names the format too: format, then color or depth
    // and stencil (5 words).
    let clear_value = (!clear.is_null()).then(|| {
        // SAFETY: the title passes a whole clear value.
        let mut value = unsafe { clear.cast::<[u32; 5]>().read_unaligned() };
        value[0] = if d.flags & (RENDER_TARGET | UNORDERED) != 0 {
            target_format(value[0])
        } else {
            pc_format(value[0])
        };
        value
    });
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let mut texture: Option<ID3D12Resource> = None;
    let create = |d: &ResourceDesc, texture: &mut Option<ID3D12Resource>| {
        // A clear value is only legal on render targets and depth.
        let clear = clear_value
            .as_ref()
            .filter(|_| d.flags & (1 | DEPTH) != 0)
            .map(|v| std::ptr::from_ref(v).cast());
        // SAFETY: the description has the host layout (checked size, same
        // fields); the out pointer is ours.
        unsafe {
            host.device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                std::ptr::from_ref(d).cast(),
                D3D12_RESOURCE_STATES(crate::host::legal_for(
                    d.flags,
                    crate::host::texture_state(state),
                ) as i32),
                // The same layout: format, then color or depth and stencil.
                clear,
                texture,
            )
        }
    };
    // SAFETY: the same fields (checked size).
    let desc = unsafe { std::ptr::from_ref(&d).cast::<D3D12_RESOURCE_DESC>().read() };
    let first = D3D12_RESOURCE_STATES(crate::host::legal_for(
        d.flags,
        crate::host::texture_state(state),
    ) as i32);
    if let Some((texture, slot)) = crate::texpool::place(&host.device, &desc, first) {
        return Some((texture, Some(slot), skip, false));
    }
    if (39..=43).contains(&d.format)
        && d.flags & UNORDERED != 0
        && let Some(texture) = castable(
            &host.device,
            &desc,
            clear_value
                .as_ref()
                .filter(|_| d.flags & RENDER_TARGET != 0)
                .map(|v| std::ptr::from_ref(v).cast()),
        )
    {
        return Some((texture, None, skip, true));
    }
    let mut made = create(&d, &mut texture);
    // Not every host GPU offers every use of every format (NVIDIA: no
    // unordered access to BGRA8, no B4G4R4A4 render targets, where the
    // console's AMD GPU has both). Give up the uses the host refuses, UAV
    // first; the views the host cannot make are skipped as for depth.
    for drop in [UNORDERED, 1] {
        if made.is_ok() || d.flags & drop == 0 {
            continue;
        }
        let mut fewer = d;
        fewer.flags &= !drop;
        let mut retry: Option<ID3D12Resource> = None;
        if create(&fewer, &mut retry).is_ok() {
            tracing::warn!(desc = ?d, dropped = drop, "host texture without a use the host refuses");
            d = fewer;
            texture = retry;
            made = Ok(());
        } else if drop == UNORDERED {
            d = fewer; // also try with neither
        }
    }
    if let Err(e) = made {
        tracing::warn!(error = %e, desc = ?d, "no host texture");
    }
    if let Some(t) = &texture {
        crate::host::name(t, || {
            format!(
                "texture {}x{} f{} flags {:#x}",
                d.width, d.height, d.format, d.flags
            )
        });
    }
    texture.map(|t| (t, None, skip, false))
}

/// A texture's console memory and description; `None` for buffers.
pub(crate) fn texture_memory(this: This) -> Option<(*const u8, ResourceDesc)> {
    if this.is_null() {
        return None;
    }
    let r = resource(this);
    (r.desc.dimension != BUFFER).then(|| (r.data().cast_const(), r.desc))
}

/// Whether `this` is one of our texture resources (it may be any address).
#[cfg(windows)]
pub(crate) fn is_texture(this: This) -> bool {
    let mut vtable = [0u8; 8];
    crate::host::read_into(this.cast::<u8>(), &mut vtable)
        && u64::from_le_bytes(vtable) == RESOURCE_FUNCTIONS.0.as_ptr() as u64
        && resource(this).host.is_some()
}

/// A buffer's host memory and size; `None` for textures.
pub(crate) fn buffer_bytes(this: This) -> Option<(*mut u8, u64)> {
    if this.is_null() {
        return None;
    }
    let r = resource(this);
    (r.desc.dimension == BUFFER).then(|| (r.data(), r.desc.width))
}

impl Resource {
    fn data(&self) -> *mut u8 {
        match self.backing {
            Backing::Committed(ref memory) => memory.ptr.as_ptr(),
            // SAFETY: placement checked the range lies inside the heap.
            Backing::Placed { heap, offset } => unsafe {
                (*heap.cast::<Heap>())
                    .memory
                    .ptr
                    .as_ptr()
                    .add(offset as usize)
            },
            Backing::Address(at) => at,
        }
    }
}

fn resource<'a>(this: This) -> &'a Resource {
    // SAFETY: called from resource methods only.
    unsafe { &*this.cast::<Resource>() }
}

// The creation methods' own arguments, passed on.
#[allow(clippy::too_many_arguments)]
fn hand_out_resource(
    desc: ResourceDesc,
    heap: HeapProperties,
    heap_flags: u32,
    backing: Backing,
    state: u32,
    clear: *const c_void,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    #[cfg(not(windows))]
    let _ = (state, clear);
    #[cfg(windows)]
    let host = host_texture(&desc, state, clear);
    let resource = Box::new(Resource {
        object: Object::new(&RESOURCE_FUNCTIONS),
        desc,
        heap,
        heap_flags,
        backing,
        #[cfg(windows)]
        host: host.map(|(texture, slot, skip, common)| {
            let state = if common {
                0
            } else {
                crate::host::legal_state(&texture, crate::host::texture_state(state))
            };
            let bytes = allocation_bytes(&texture);
            crate::memstat::add(&crate::memstat::HOST_TEXTURES, bytes);
            crate::memstat::add(&crate::memstat::HOST_TEXTURE_COUNT, 1);
            #[cfg(feature = "diagnostics")]
            census(&texture, bytes as i64);
            std::sync::Arc::new(HostTexture {
                texture,
                bytes,
                slot,
                skip,
                written: Default::default(),
                twin: Default::default(),
                state: state.into(),
            })
        }),
    });
    #[cfg(windows)]
    if desc.dimension == BUFFER {
        let at = std::ptr::from_ref::<Resource>(&resource) as usize;
        crate::shadow::register(at, resource.data() as u64, desc.width);
    } else if desc.flags & 5 != 0 && resource.host.is_some() {
        let at = std::ptr::from_ref::<Resource>(&resource) as usize;
        let mut targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
        targets.entry(resource.data() as u64).or_default().push(at);
    }
    object::hand_out(RESOURCE, resource, iid, &RESOURCE_IIDS, out)
}

#[cfg(windows)]
impl Drop for Resource {
    fn drop(&mut self) {
        if self.desc.flags & 5 != 0 && self.host.is_some() {
            let at = std::ptr::from_ref::<Resource>(self) as usize;
            let mut targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
            let start = self.data() as u64;
            if let Some(list) = targets.get_mut(&start) {
                list.retain(|&r| r != at);
                if list.is_empty() {
                    targets.remove(&start);
                }
            }
        }
        #[cfg(feature = "diagnostics")]
        if self
            .host
            .as_ref()
            .is_some_and(|h| std::sync::Arc::strong_count(h) > 1)
        {
            // Before lists held their textures, this freed a texture that
            // lists not yet run still named (a GPU page fault).
            static OUTLIVED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = OUTLIVED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            if n.is_power_of_two() {
                tracing::info!(
                    n,
                    desc = ?self.desc,
                    "a title texture was released while a list not yet reset names its host copy"
                );
            }
        }
        // A running submission may still read the host copy.
        if let Some(host) = self.host.take() {
            crate::host::retire(host);
        }
        crate::texture::forget(std::ptr::from_ref::<Resource>(self) as usize);
        crate::descriptor::forget(std::ptr::from_ref::<Resource>(self) as usize);
        if self.desc.dimension == BUFFER {
            crate::shadow::unregister(
                std::ptr::from_ref::<Resource>(self) as usize,
                self.data() as u64,
            );
        }
    }
}

pub(crate) extern "system" fn create_committed(
    _: This,
    heap: *const HeapProperties,
    heap_flags: u32,
    desc: *const ResourceDesc,
    state: u32,
    clear: *const c_void,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes both descriptions (null is tolerated).
    let (Some(&heap), Some(&desc)) = (unsafe { (heap.as_ref(), desc.as_ref()) }) else {
        return E_INVALIDARG;
    };
    let Some(info) = footprint(&desc) else {
        tracing::warn!(?desc, "no size for this resource");
        return E_INVALIDARG;
    };
    tracing::debug!(
        dimension = desc.dimension,
        size = info.size,
        heap = heap.kind,
        "committed resource"
    );
    let memory = match Memory::for_resource(info, heap.kind) {
        Ok(memory) => memory,
        Err(hr) => return hr,
    };
    hand_out_resource(
        desc,
        heap,
        heap_flags,
        Backing::Committed(memory),
        state,
        clear,
        iid,
        out,
    )
}

/// Device method 0x3f8, inferred from Grand Theft Auto VI's calls
/// (title+0x193e1b4: its back buffers, heap flag "allow display"): the
/// committed resource creation of the enhanced-barrier interfaces,
/// `(heap, heap flags, desc1, initial layout, clear value, protected
/// session, castable format count, castable formats, iid, out)`. The host
/// copy starts in the common state, as layouts are not tracked.
#[allow(clippy::too_many_arguments)] // the method's own arguments
pub(crate) extern "system" fn create_committed3(
    this: This,
    heap: *const HeapProperties,
    heap_flags: u32,
    desc: *const ResourceDesc,
    _layout: u32,
    clear: *const c_void,
    _protected: This,
    _castable_count: u32,
    _castable: *const u32,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    const COMMON: u32 = 0;
    create_committed(this, heap, heap_flags, desc, COMMON, clear, iid, out)
}

/// Registered page pools (64 KiB pages the title maps into tiled
/// resources by page index): first address and page count; a handle is an
/// index plus one, a removed pool leaves `None`.
static PAGE_POOLS: std::sync::Mutex<Vec<Option<(u64, u32)>>> = std::sync::Mutex::new(Vec::new());

fn page_pools() -> std::sync::MutexGuard<'static, Vec<Option<(u64, u32)>>> {
    PAGE_POOLS.lock().unwrap_or_else(|e| e.into_inner())
}

/// Device method 0x250: `(address, page count, handle out)`.
pub(crate) extern "system" fn register_page_pool(
    _: This,
    address: u64,
    pages: u32,
    out: *mut usize,
) -> HRESULT {
    if out.is_null() || pages == 0 || !address.is_multiple_of(KIB_64) {
        return E_INVALIDARG;
    }
    tracing::debug!(address = format_args!("{address:#x}"), pages, "page pool");
    let mut pools = page_pools();
    pools.push(Some((address, pages)));
    // SAFETY: checked non-null; the title passes room for a handle.
    unsafe { out.write(pools.len()) };
    S_OK
}

/// Device method 0x258.
pub(crate) extern "system" fn unregister_page_pool(_: This, handle: usize) -> HRESULT {
    match page_pools().get_mut(handle.wrapping_sub(1)) {
        Some(pool @ Some(_)) => {
            *pool = None;
            S_OK
        }
        _ => E_INVALIDARG,
    }
}

/// Device method 0x428, one of the `ID3D12Device9` to `ID3D12Device11`
/// additions. From Grand Theft Auto VI's call: a resource at a virtual
/// address the title mapped itself, `(address, desc1, initial barrier
/// layout, clear value, iid, out)`, as `CreatePlacedResourceX` with
/// enhanced barriers. GPU addresses are CPU addresses, so the resource is
/// a view of that memory. `D3D12_RESOURCE_DESC1` starts like the plain
/// description; its sampler feedback region is not needed.
pub(crate) extern "system" fn create_at_address(
    _: This,
    address: u64,
    desc: *const ResourceDesc,
    layout: u32,
    clear: *const c_void,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if address == 0 {
        return E_INVALIDARG;
    }
    tracing::debug!(
        address = format_args!("{address:#x}"),
        dimension = desc.dimension,
        width = desc.width,
        layout,
        "resource at an address"
    );
    const COMMON: u32 = 0;
    const DEFAULT: HeapProperties = HeapProperties {
        kind: 1,
        cpu_pages: 0,
        pool: 0,
        creation_nodes: 1,
        visible_nodes: 1,
    };
    #[cfg(windows)]
    STREAMED.set(true);
    let made = hand_out_resource(
        desc,
        DEFAULT,
        0,
        Backing::Address(address as *mut u8),
        COMMON,
        clear,
        iid,
        out,
    );
    #[cfg(windows)]
    STREAMED.set(false);
    made
}

/// Textures the title placed in its own memory and views without a
/// resource object (device 0x230, streamed and interface textures), by
/// address and description: a resource at that address and where its
/// footprint ends, made the first time a draw reads one and evicted when
/// [`crate::watch::title_memory_remapped`] unmaps or remaps the range.
#[cfg(windows)]
type PlacedTextureMap = std::collections::BTreeMap<(u64, [u32; 14]), (usize, u64)>;

#[cfg(windows)]
static PLACED_TEXTURES: std::sync::Mutex<PlacedTextureMap> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Incremented whenever [`evict_placed_textures`] removes an entry so
/// bindless heaps re-check placed texture slots.
#[cfg(windows)]
static PLACED_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg(windows)]
pub(crate) fn placed_epoch() -> u64 {
    PLACED_EPOCH.load(std::sync::atomic::Ordering::Acquire)
}

/// A reference to a placed texture: eviction does not free it while held.
#[cfg(windows)]
pub(crate) struct HeldTexture(This);

#[cfg(windows)]
impl Drop for HeldTexture {
    fn drop(&mut self) {
        resource_release(self.0);
    }
}

/// A reference to `texture` if it is a placed texture alive now.
#[cfg(windows)]
pub(crate) fn hold_placed(texture: This) -> Option<HeldTexture> {
    let _map = PLACED_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
    let live = PLACED_LIVE.lock().unwrap_or_else(|e| e.into_inner());
    live.contains(&(texture as usize)).then(|| {
        object::add_ref(texture);
        HeldTexture(texture)
    })
}

/// The placed textures alive now, and the longest there was. Locked after
/// [`PLACED_TEXTURES`], changed with it.
#[cfg(windows)]
static PLACED_LIVE: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<usize>>> =
    std::sync::LazyLock::new(Default::default);
#[cfg(windows)]
static PLACED_LONGEST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The placed textures alive now (resource objects).
#[cfg(windows)]
pub(crate) fn placed_live() -> std::collections::HashSet<usize> {
    let mut live: std::collections::HashSet<usize> = PLACED_TEXTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .map(|&(r, _)| r)
        .collect();
    let targets = TARGETS.lock().unwrap_or_else(|e| e.into_inner());
    live.extend(targets.values().flatten());
    live
}

/// The resource for a texture placed at `address` with the resource
/// description `desc` (as words), made on first use.
#[cfg(windows)]
pub(crate) fn texture_at(address: u64, desc: &[u32; 14], largest: u32) -> Option<This> {
    if address == 0 {
        return None;
    }
    // The description's padding holds anything; the same texture was made
    // up to four times for it. It holds the largest mip here.
    let mut desc = *desc;
    desc[1] = 0;
    desc[13] = largest;
    let desc = &desc;
    // SAFETY: 56 bytes in the description's layout.
    let d = unsafe { desc.as_ptr().cast::<ResourceDesc>().read_unaligned() };
    if d.dimension == BUFFER {
        return None;
    }
    if let Some(target) = target_at(address, &d) {
        return Some(target);
    }
    {
        let map = PLACED_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(&(r, _)) = map.get(&(address, *desc)) {
            return Some(r as This);
        }
    }
    let size = match texture_layout(&d) {
        Some(t) if largest > 0 && t.planes.len() == 1 => t.planes[0].layout.reach(largest),
        _ => footprint(&d).map_or(1, |info| info.size.max(1)),
    };
    let end = address.saturating_add(size);
    // The title's pages for it may not be there yet (streaming): nothing to
    // show, and the console's whole address space would not fit in memory.
    if !cfg!(test) && !crate::texture::committed(address as *const u8, size as usize) {
        return None;
    }
    let mut out = std::ptr::null();
    // Outside the lock: making one takes a while.
    LARGEST.set(largest);
    let made = create_at_address(
        std::ptr::null(),
        address,
        &d,
        0,
        std::ptr::null(),
        &RESOURCE_IIDS[0],
        &mut out,
    );
    LARGEST.set(0);
    if made != S_OK {
        return None;
    }
    if resource(out).host.is_none() {
        resource_release(out);
        return None;
    }
    let mut map = PLACED_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(&(r, _)) = map.get(&(address, *desc)) {
        drop(map);
        resource_release(out);
        return Some(r as This);
    }
    PLACED_LIVE.lock().unwrap_or_else(|e| e.into_inner()).insert(out as usize);
    PLACED_LONGEST.fetch_max(end - address, std::sync::atomic::Ordering::AcqRel);
    map.insert((address, *desc), (out as usize, end));
    crate::memstat::PLACED_COUNT.store(map.len() as u64, std::sync::atomic::Ordering::Relaxed);
    Some(out)
}

/// What is done about textures whose title memory changes
/// (`VICTORY_TEXTURES`, to tell what texture streaming here costs and does):
/// `keep` uploads each once, whole, and keeps it whatever the title does
/// with its memory; `always` looks at each at every use, whole.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Textures {
    Stream,
    Keep,
    Always,
}

pub(crate) fn textures() -> Textures {
    static MODE: std::sync::OnceLock<Textures> = std::sync::OnceLock::new();
    *MODE.get_or_init(|| match std::env::var("VICTORY_TEXTURES").as_deref() {
        Ok("keep") => Textures::Keep,
        Ok("always") => Textures::Always,
        _ => Textures::Stream,
    })
}

/// Placed textures whose memory the runtime wrote since they were uploaded,
/// and how many were (bindless heaps take those they have not seen).
#[cfg(windows)]
static REWRITTEN: std::sync::Mutex<std::collections::VecDeque<usize>> =
    std::sync::Mutex::new(std::collections::VecDeque::new());
#[cfg(windows)]
static REWRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many placed textures were evicted so far.
#[cfg(windows)]
pub(crate) static EVICTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How many textures were rewritten so far.
#[cfg(windows)]
pub(crate) fn rewrites() -> u64 {
    REWRITES.load(std::sync::atomic::Ordering::Acquire)
}

/// The textures rewritten after the first `seen`, and how many were by now.
#[cfg(windows)]
pub(crate) fn rewritten_since(seen: u64) -> (Vec<usize>, u64) {
    let list = REWRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    let now = rewrites();
    let new = now.saturating_sub(seen).min(list.len() as u64) as usize;
    (list.iter().skip(list.len() - new).copied().collect(), now)
}

/// The runtime wrote `[start, end)`: the textures placed there are stale.
#[cfg(windows)]
pub(crate) fn textures_rewritten(start: u64, end: u64) {
    if textures() == Textures::Keep {
        return;
    }
    let found: Vec<usize> = {
        let map = PLACED_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
        let longest = PLACED_LONGEST.load(std::sync::atomic::Ordering::Acquire);
        let from = (start.saturating_sub(longest), [0u32; 14]);
        map.range(from..(end, [0u32; 14]))
            .filter(|(_, (_, tex_end))| *tex_end > start)
            .map(|(_, &(r, _))| r)
            .collect()
    };
    if found.is_empty() {
        return;
    }
    for &r in &found {
        crate::texture::rewritten(r);
    }
    const KEPT: usize = 1 << 16;
    let mut list = REWRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    list.extend(&found);
    while list.len() > KEPT {
        list.pop_front();
    }
    REWRITES.fetch_add(found.len() as u64, std::sync::atomic::Ordering::Release);
}

/// Releases placed textures whose backing range overlaps `[start, end)`.
#[cfg(windows)]
pub(crate) fn evict_placed_textures(start: u64, end: u64) {
    if start >= end || textures() == Textures::Keep {
        return;
    }
    let mut evicted = Vec::new();
    {
        let mut map = PLACED_TEXTURES.lock().unwrap_or_else(|e| e.into_inner());
        let longest = PLACED_LONGEST.load(std::sync::atomic::Ordering::Acquire);
        let from = (start.saturating_sub(longest), [0u32; 14]);
        let over: Vec<_> = map
            .range(from..(end, [0u32; 14]))
            .filter(|(_, (_, tex_end))| *tex_end > start)
            .map(|(k, _)| *k)
            .collect();
        let mut live = PLACED_LIVE.lock().unwrap_or_else(|e| e.into_inner());
        for key in over {
            if let Some((res, _)) = map.remove(&key) {
                live.remove(&res);
                evicted.push(res);
            }
        }
    }
    EVICTED.fetch_add(evicted.len() as u64, std::sync::atomic::Ordering::Relaxed);
    if !evicted.is_empty() {
        PLACED_EPOCH.fetch_add(1, std::sync::atomic::Ordering::Release);
        for res in evicted {
            resource_release(res as This);
        }
    }
}

/// Diagnostics: the placed textures counted, to tell why they hold more
/// than the title's own memory does. Logs how many there are and the title
/// bytes they cover; how many addresses hold more than one (other largest
/// mips or descriptions) and the bytes of the extra ones; and how many
/// stand on memory the title no longer has.
#[cfg(all(windows, feature = "diagnostics"))]
pub(crate) fn placed_census() {
    // What the host holds for them: all, and those holding over twice
    // their title memory, by title format (count, title bytes, host bytes).
    let mut heavy: std::collections::BTreeMap<u32, (u64, u64, u64)> = Default::default();
    let mut host_bytes = 0u64;
    let entries: Vec<(u64, u64, u32)> = PLACED_TEXTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(&(address, desc), &(r, end))| {
            let res = resource(r as This);
            if let Some(h) = res.host.as_ref() {
                host_bytes += h.bytes;
                if h.bytes > 2 * (end - address) {
                    let e = heavy.entry(res.desc.format).or_default();
                    *e = (e.0 + 1, e.1 + (end - address), e.2 + h.bytes);
                }
            }
            (address, end, desc[13])
        })
        .collect();
    let mut heavy: Vec<_> = heavy.into_iter().collect();
    heavy.sort_unstable_by_key(|&(_, (_, _, host))| std::cmp::Reverse(host));
    heavy.truncate(5);
    tracing::info!(
        "placed textures hold {} MiB on the host; over twice their title memory, by title format (count, title MiB, host MiB): {:?}",
        host_bytes >> 20,
        heavy.iter().map(|&(f, (n, t, h))| (f, n, t >> 20, h >> 20)).collect::<Vec<_>>()
    );
    let mb = |b: u64| b >> 20;
    let total: u64 = entries.iter().map(|&(a, e, _)| e - a).sum();
    let (mut shared, mut extra, mut gone, mut gone_bytes) = (0u64, 0u64, 0u64, 0u64);
    let mut i = 0;
    while i < entries.len() {
        let mut j = i + 1;
        while j < entries.len() && entries[j].0 == entries[i].0 {
            j += 1;
        }
        if j - i > 1 {
            shared += 1;
            let sizes = entries[i..j].iter().map(|&(a, e, _)| e - a);
            extra += sizes.clone().sum::<u64>() - sizes.max().unwrap_or(0);
        }
        i = j;
    }
    // A byte of each texture's first and last pages only: cheap enough for
    // tens of thousands of them once in a while.
    for &(address, end, _) in &entries {
        let mut byte = [0u8; 1];
        if !crate::host::read_into(address as *const u8, &mut byte)
            || !crate::host::read_into((end - 1) as *const u8, &mut byte)
        {
            gone += 1;
            gone_bytes += end - address;
        }
    }
    // Stretches of title memory covered by more than one texture.
    let mut spans: Vec<(u64, u64)> = entries.iter().map(|&(a, e, _)| (a, e)).collect();
    spans.sort_unstable();
    let (mut covered, mut reach) = (0u64, 0u64);
    for (a, e) in spans {
        if e > reach {
            covered += e - a.max(reach);
            reach = e;
        }
    }
    for (what, (held, used, slabs, worst)) in ["textures", "staging"].iter().zip(crate::texpool::census()) {
        tracing::info!(
            "texture pool, {what}: {} MiB in {slabs} slabs, {} MiB of them in use; most waste in slots of {} KiB: {} MiB held, {} MiB used",
            mb(held),
            mb(used),
            worst.0 >> 10,
            mb(worst.1),
            mb(worst.2)
        );
    }
    tracing::info!(
        "placed textures: {} over {} MiB of title memory ({} MiB once overlaps are counted once);          {shared} addresses hold several, {} MiB extra; {gone} on memory the title no longer has, {} MiB",
        entries.len(),
        mb(total),
        mb(covered),
        mb(extra),
        mb(gone_bytes)
    );
}

#[cfg(all(test, windows))]
pub(crate) fn has_placed_texture(address: u64, desc: &[u32; 14]) -> bool {
    PLACED_TEXTURES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .contains_key(&(address, *desc))
}

pub(crate) extern "system" fn create_placed(
    _: This,
    heap: This,
    offset: u64,
    desc: *const ResourceDesc,
    state: u32,
    clear: *const c_void,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if heap.is_null() {
        return E_INVALIDARG;
    }
    // SAFETY: the title passes one of our heaps.
    let h = unsafe { &*heap.cast::<Heap>() };
    let fits = footprint(&desc).is_some_and(|info| {
        offset.is_multiple_of(info.alignment)
            && offset
                .checked_add(info.size)
                .is_some_and(|end| end <= h.desc.size)
    });
    if !fits {
        tracing::warn!(
            ?desc,
            offset,
            heap = h.desc.size,
            "placed resource does not fit"
        );
        return E_INVALIDARG;
    }
    object::add_ref(heap);
    let backing = Backing::Placed { heap, offset };
    hand_out_resource(
        desc,
        h.desc.properties,
        h.desc.flags,
        backing,
        state,
        clear,
        iid,
        out,
    )
}

extern "system" fn resource_query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(
        RESOURCE,
        iid,
        object::is(iid, &RESOURCE_IIDS).then_some(this),
        out,
    )
}
extern "system" fn resource_release(this: This) -> u32 {
    object::release_boxed::<Resource>(this)
}
/// Memory is always mapped (subresources are not told apart yet); a null
/// `out` only asks for residency.
extern "system" fn map(this: This, _sub: u32, _read: *const c_void, out: *mut *mut u8) -> HRESULT {
    if !out.is_null() {
        // SAFETY: checked non-null; the title passes room for a pointer.
        unsafe { out.write(resource(this).data()) };
    }
    S_OK
}
extern "system" fn unmap(_: This, _sub: u32, _written: *const c_void) {}
extern "system" fn resource_desc(this: This, out: *mut ResourceDesc) {
    if !out.is_null() {
        // SAFETY: checked non-null; the title passes room for a description.
        unsafe { out.write(resource(this).desc) };
    }
}
/// Device method `GetCustomHeapProperties`, returning its struct through a
/// hidden first argument: what a standard heap type is as a custom heap.
/// Memory is unified and cache-coherent (as the architecture query says),
/// so every type is in pool L0, default heaps are not CPU-visible and the
/// others are write-back.
pub(crate) extern "system" fn custom_heap_properties(
    out: *mut HeapProperties,
    _: This,
    nodes: u32,
    kind: u32,
) -> *mut HeapProperties {
    const CUSTOM: u32 = 4;
    const NOT_AVAILABLE: u32 = 1;
    const WRITE_BACK: u32 = 3;
    const L0: u32 = 1;
    let nodes = nodes.max(1);
    let cpu_pages = match kind {
        1 => NOT_AVAILABLE,
        2 | 3 => WRITE_BACK,
        _ => 0,
    };
    let properties = HeapProperties {
        kind: CUSTOM,
        cpu_pages,
        pool: if cpu_pages == 0 { 0 } else { L0 },
        creation_nodes: nodes,
        visible_nodes: nodes,
    };
    // SAFETY: the title passes room for the result (null is tolerated).
    if let Some(out) = unsafe { out.as_mut() } {
        *out = properties;
    }
    out
}

extern "system" fn gpu_address(this: This) -> u64 {
    let r = resource(this);
    let at = r.data() as u64;
    // The console's GPU addresses fit in 40 bits; its ray-tracing builder
    // refuses others (diagnostics: which memory breaks that).
    #[cfg(feature = "diagnostics")]
    if at >> 40 != 0 {
        static ONCE: [std::sync::Once; 2] = [std::sync::Once::new(), std::sync::Once::new()];
        let backing = match r.backing {
            Backing::Committed(ref m) => format!("committed, attributes {:?}", m.attributes),
            Backing::Placed { offset, .. } => format!("placed in a heap at {offset:#x}"),
            Backing::Address(_) => "at a title address".into(),
        };
        ONCE[usize::from(r.desc.dimension == BUFFER)].call_once(|| {
            tracing::warn!(address = format_args!("{at:#x}"), backing, desc = ?r.desc, "a GPU address above 40 bits")
        });
    }
    at
}
extern "system" fn heap_properties(
    this: This,
    properties: *mut HeapProperties,
    flags: *mut u32,
) -> HRESULT {
    let r = resource(this);
    // SAFETY: either pointer may be null; otherwise the title passes room.
    unsafe {
        if let Some(p) = properties.as_mut() {
            *p = r.heap;
        }
        if let Some(f) = flags.as_mut() {
            *f = r.heap_flags;
        }
    }
    S_OK
}

/// IUnknown 3, object 4, device child 1, heap 1.
static HEAP_FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<HEAP, 24>());
    f[0] = heap_query as *const ();
    f[2] = heap_release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    f[8] = heap_desc as *const (); // 0x40
    Table(f)
};

/// IUnknown 3, object 4, device child 1, resource 10 (7 as on PC, then 3
/// console ones).
static RESOURCE_FUNCTIONS: Table<32> = {
    let mut f = object::d3d12_object(object::missing_table::<RESOURCE, 32>());
    f[0] = resource_query as *const ();
    f[2] = resource_release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    f[8] = map as *const (); // 0x40
    f[9] = unmap as *const (); // 0x48
    f[10] = resource_desc as *const (); // 0x50
    f[11] = gpu_address as *const (); // 0x58
    f[14] = heap_properties as *const (); // 0x70
    Table(f)
};

#[cfg(test)]
pub(crate) fn test_buffer(width: u64) -> This {
    let heap = HeapProperties {
        kind: 1,
        cpu_pages: 0,
        pool: 0,
        creation_nodes: 0,
        visible_nodes: 0,
    };
    let desc = ResourceDesc {
        dimension: BUFFER,
        alignment: 0,
        width,
        height: 1,
        depth_or_array_size: 1,
        mip_levels: 1,
        format: 0,
        sample_count: 1,
        sample_quality: 0,
        layout: 1,
        flags: 0,
    };
    let mut out = std::ptr::null();
    let hr = create_committed(
        std::ptr::null(),
        &heap,
        0,
        &desc,
        0,
        std::ptr::null(),
        &RESOURCE_IIDS[0],
        &mut out,
    );
    assert_eq!(hr, S_OK);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::slot;

    type Release = extern "system" fn(This) -> u32;
    type Map = extern "system" fn(This, u32, *const c_void, *mut *mut u8) -> HRESULT;
    type Address = extern "system" fn(This) -> u64;

    const PROPERTIES: HeapProperties = HeapProperties {
        kind: 1,
        cpu_pages: 0,
        pool: 0,
        creation_nodes: 0,
        visible_nodes: 0,
    };

    fn buffer(width: u64) -> ResourceDesc {
        ResourceDesc {
            dimension: BUFFER,
            alignment: 0,
            width,
            height: 1,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: 0,
            sample_count: 1,
            sample_quality: 0,
            layout: 1,
            flags: 0,
        }
    }

    fn texture(width: u64, height: u32, format: u32, mips: u16) -> ResourceDesc {
        ResourceDesc {
            dimension: 3,
            width,
            height,
            format,
            mip_levels: mips,
            layout: 0,
            ..buffer(0)
        }
    }

    fn heap(size: u64) -> This {
        let desc = HeapDesc {
            size,
            properties: PROPERTIES,
            alignment: 0,
            flags: 0,
        };
        let mut heap = std::ptr::null();
        assert_eq!(
            create_heap(std::ptr::null(), &desc, &HEAP_IIDS[0], &mut heap),
            S_OK
        );
        heap
    }

    #[test]
    fn heaps_are_zeroed_aligned_memory() {
        let heap = heap(1 << 20);
        // SAFETY: a heap we just made.
        let memory = unsafe { &(*heap.cast::<Heap>()).memory };
        assert_eq!(memory.ptr.as_ptr() as u64 % KIB_64, 0);
        let mut back = std::mem::MaybeUninit::<HeapDesc>::uninit();
        slot::<extern "system" fn(This, *mut HeapDesc)>(heap, 0x40)(heap, back.as_mut_ptr());
        // SAFETY: written above.
        assert_eq!(unsafe { back.assume_init() }.size, 1 << 20);
        assert_eq!(slot::<Release>(heap, 0x10)(heap), 0);

        let bad = HeapDesc {
            size: 1 << 20,
            properties: PROPERTIES,
            alignment: 3,
            flags: 0,
        };
        let mut out = std::ptr::null();
        assert_eq!(
            create_heap(std::ptr::null(), &bad, &HEAP_IIDS[0], &mut out),
            E_INVALIDARG
        );
    }

    #[test]
    fn device_setup_heap_properties_and_resources_at_addresses() {
        let device = crate::device::device();
        // GetCustomHeapProperties (0xd0): the struct comes back through the
        // hidden first argument, which is also returned.
        type Custom =
            extern "system" fn(*mut HeapProperties, This, u32, u32) -> *mut HeapProperties;
        let mut p = PROPERTIES;
        let back = slot::<Custom>(device, 0xd0)(&mut p, device, 1, 2);
        assert_eq!(back, &raw mut p);
        assert_eq!(
            (
                p.kind,
                p.cpu_pages,
                p.pool,
                p.creation_nodes,
                p.visible_nodes
            ),
            (4, 3, 1, 1, 1),
            "upload: custom, write-back, L0"
        );
        slot::<Custom>(device, 0xd0)(&mut p, device, 1, 1);
        assert_eq!(p.cpu_pages, 1, "default: not CPU-visible");

        // 0x428: a buffer over memory the title owns, as GTA VI asks.
        type At = extern "system" fn(
            This,
            u64,
            *const ResourceDesc,
            u32,
            *const c_void,
            *const Iid,
            *mut This,
        ) -> HRESULT;
        let memory = vec![0u8; 0x1_0000];
        let address = memory.as_ptr() as u64;
        let mut r = std::ptr::null();
        let create = slot::<At>(device, 0x428);
        let desc = buffer(0x1_0000);
        let null = std::ptr::null();
        assert_eq!(
            create(
                device,
                address,
                &desc,
                u32::MAX,
                null,
                &RESOURCE_IIDS[0],
                &mut r
            ),
            0
        );
        assert_eq!(slot::<Address>(r, 0x58)(r), address);
        assert_eq!(slot::<Release>(r, 0x10)(r), 0);
        // The title's memory is still there after the release.
        assert_eq!(memory[0xffff], 0);
        assert_ne!(
            create(device, 0, &desc, u32::MAX, null, &RESOURCE_IIDS[0], &mut r),
            0
        );
    }

    #[test]
    fn sizes_cover_every_mip() {
        let size = |d: &ResourceDesc| footprint(d).map(|i| i.size);
        assert_eq!(size(&buffer(256)), Some(KIB_64));
        // Textures take their console layout and its alignment (4 KiB
        // standard swizzle when the description leaves it to the library):
        // 512x512 BC7 with 10 mips is 0x56000 bytes, 4 KiB aligned, as on
        // the console.
        let layout = |d: &ResourceDesc| texture_layout(d).unwrap().size;
        let rgba = texture(256, 256, 28, 0);
        assert_eq!(size(&rgba), Some(layout(&rgba).next_multiple_of(4096)));
        let bc7 = footprint(&texture(512, 512, 98, 10));
        let console = AllocationInfo {
            size: 0x56000,
            alignment: 0x1000,
        };
        assert_eq!(bc7, Some(console));
        assert!(layout(&rgba) >= 4 * (256 * 256 * 4) / 3, "every mip");
        // BC1 is 8 bytes per 4x4 block: 1024x1024, one mip = 512 KiB.
        assert_eq!(size(&texture(1024, 1024, 71, 1)), Some(512 << 10));
        assert_eq!(size(&texture(16, 16, 66, 1)), None, "1-bit format");
        // Linear layout (1) and an explicit mode (64 KiB display, 0x10a).
        let linear = ResourceDesc {
            layout: 1,
            ..texture(100, 60, 28, 1)
        };
        assert_eq!(
            texture_layout(&linear).unwrap().planes[0]
                .layout
                .desc
                .swizzle
                .0,
            0
        );
        let display = ResourceDesc {
            layout: 0x10a,
            ..texture(100, 60, 28, 1)
        };
        assert_eq!(
            texture_layout(&display).unwrap().planes[0]
                .layout
                .desc
                .swizzle
                .0,
            10
        );

        let mut info = AllocationInfo {
            size: 0,
            alignment: 0,
        };
        let descs = [buffer(100), texture(256, 256, 28, 0)];
        allocation_info(&mut info, std::ptr::null(), 0, 2, descs.as_ptr());
        assert_eq!(
            info,
            AllocationInfo {
                size: KIB_64 + size(&descs[1]).unwrap(),
                alignment: KIB_64
            }
        );
        allocation_info(&mut info, std::ptr::null(), 0, 1, &texture(1, 1, 999, 1));
        assert_eq!(info.size, u64::MAX);
    }

    #[test]
    fn upload_footprints_follow_the_pc_rules() {
        let null = std::ptr::null();
        let mut layouts = [NO_FOOTPRINT; 2];
        let (mut rows, mut sizes, mut total) = ([0; 2], [0; 2], 0);
        let mut footprints = |desc: &ResourceDesc, first, count| {
            let (l, r, s) = (layouts.as_mut_ptr(), rows.as_mut_ptr(), sizes.as_mut_ptr());
            copyable_footprints(null, desc, first, count, 0, l, r, s, &mut total);
            (layouts, rows, sizes, total)
        };
        // 100x60 RGBA8, 2 mips: 400-byte rows pitched to 512.
        let (l, r, s, t) = footprints(&texture(100, 60, 28, 2), 0, 2);
        assert_eq!((l[0].row_pitch, r[0], s[0]), (512, 60, 400));
        // Mip 1 (50x30, 200-byte rows) starts after 59 pitched rows and one
        // packed row, rounded up to 512.
        assert_eq!(l[1].offset, (512 * 59 + 400u64).next_multiple_of(512));
        assert_eq!((l[1].width, l[1].height, r[1]), (50, 30, 30));
        assert_eq!(t, l[1].offset + 256 * 29 + 200);
        // BC1: rows of 4x4 blocks, 8 bytes each.
        let (_, r, s, _) = footprints(&texture(64, 64, 71, 1), 0, 1);
        assert_eq!((r[0], s[0]), (16, 128));
        let (_, _, _, t) = footprints(&texture(64, 64, 71, 1), 1, 1);
        assert_eq!(t, u64::MAX, "no second subresource");
    }


    /// A 4x4 texture filled from a title buffer through our list and queue,
    /// then read back from the host GPU.
    #[cfg(windows)]
    #[test]
    fn buffer_data_reaches_the_host_texture() {
        use windows::Win32::Graphics::Direct3D12::*;
        let Some(host) = crate::host::host() else {
            return; // no host GPU here
        };
        let null = std::ptr::null();
        let iid = &RESOURCE_IIDS[0];
        const COPY_DEST: u32 = 0x400;
        let (mut image, mut source) = (null, null);
        let rgba = texture(4, 4, 28, 1);
        assert_eq!(
            create_committed(
                null,
                &PROPERTIES,
                0,
                &rgba,
                COPY_DEST,
                null.cast(),
                iid,
                &mut image
            ),
            S_OK
        );
        assert_eq!(
            create_committed(
                null,
                &PROPERTIES,
                0,
                &buffer(1024),
                0,
                null.cast(),
                iid,
                &mut source
            ),
            S_OK
        );
        let (data, _) = buffer_bytes(source).unwrap();
        // SAFETY: the buffer holds 1024 bytes.
        let bytes = unsafe { std::slice::from_raw_parts_mut(data, 1024) };
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }

        let queue_iid = guid(
            0x0ec870a6,
            0x5d7e,
            0x4c22,
            [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed],
        );
        let allocator_iid = guid(
            0x6102dee4,
            0xaf59,
            0x4b09,
            [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24],
        );
        let list_iid = guid(
            0x5b160d0f,
            0xac1b,
            0x4185,
            [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55],
        );
        let (mut queue, mut allocator, mut list) = (null, null, null);
        let queue_desc = [0u32; 4];
        assert_eq!(
            crate::queue::create(null, queue_desc.as_ptr().cast(), &queue_iid, &mut queue),
            S_OK
        );
        assert_eq!(
            crate::list::create_allocator(null, 0, &allocator_iid, &mut allocator),
            S_OK
        );
        assert_eq!(
            crate::list::create_list(null, 0, 0, allocator, null, &list_iid, &mut list),
            S_OK
        );

        #[repr(C)]
        struct Location {
            resource: This,
            kind: u32,
            footprint: PlacedFootprint,
        }
        let rows = PlacedFootprint {
            offset: 0,
            format: 28,
            width: 4,
            height: 4,
            depth: 1,
            row_pitch: 256,
        };
        let to = Location {
            resource: image,
            kind: 0,
            footprint: PlacedFootprint { offset: 0, ..rows },
        };
        let from = Location {
            resource: source,
            kind: 1,
            footprint: rows,
        };
        type Copy =
            extern "system" fn(This, *const Location, u32, u32, u32, *const Location, *const u8);
        slot::<Copy>(list, 0x80)(list, &to, 0, 0, 0, &from, null.cast());
        slot::<extern "system" fn(This) -> HRESULT>(list, 0x48)(list);
        slot::<extern "system" fn(This, u32, *const This)>(queue, 0x50)(queue, 1, &list);

        // Read the host texture back into a readback buffer.
        let readback_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_READBACK,
            ..Default::default()
        };
        let readback_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: 1024,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            ..Default::default()
        };
        let mut readback: Option<ID3D12Resource> = None;
        // SAFETY: plain creation of a readback buffer.
        unsafe {
            host.device
                .CreateCommittedResource(
                    &readback_heap,
                    D3D12_HEAP_FLAG_NONE,
                    &readback_desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut readback,
                )
                .unwrap();
        }
        let readback = readback.unwrap();
        let host_image = host_resource(image).unwrap();
        let back = host.new_list().unwrap();
        let (_, before) = host_transition(image, 0x800).unwrap();
        // The upload list holds the texture until it is reset.
        assert!(std::sync::Arc::strong_count(&host_copy(image).unwrap()) > 2);
        use windows::core::Interface;
        let out_to = Location {
            resource: readback.as_raw().cast_const().cast(),
            kind: 1,
            footprint: rows,
        };
        let out_from = Location {
            resource: host_image.as_raw().cast_const().cast(),
            kind: 0,
            footprint: to.footprint,
        };
        // SAFETY: an open list, live resources, locations in the host layout.
        unsafe {
            back.list.ResourceBarrier(&[crate::host::transition(
                host_image,
                D3D12_RESOURCE_STATES(before as i32),
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            back.list.CopyTextureRegion(
                (&raw const out_to).cast(),
                0,
                0,
                0,
                (&raw const out_from).cast(),
                None,
            );
        }
        back.close();
        host.execute(&[&back]);
        let mut mapped = std::ptr::null_mut();
        // SAFETY: the copy finished (execute waits); the buffer is 1024 bytes.
        let read = unsafe {
            readback
                .Map(
                    0,
                    Some(&D3D12_RANGE {
                        Begin: 0,
                        End: 1024,
                    }),
                    Some(&mut mapped),
                )
                .unwrap();
            std::slice::from_raw_parts(mapped.cast::<u8>(), 1024)
        };
        for row in 0..4 {
            let at = row * 256;
            assert_eq!(read[at..at + 16], bytes[at..at + 16], "row {row}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn copy_texture_region_reads_back_into_title_buffer() {
        if crate::host::host().is_none() {
            return;
        }
        let null = std::ptr::null();
        let iid = &RESOURCE_IIDS[0];
        let tex_desc = ResourceDesc {
            dimension: 3, // Texture2D
            alignment: 0,
            width: 4,
            height: 4,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: 28, // R8G8B8A8_UNORM
            sample_count: 1,
            sample_quality: 0,
            layout: 0,
            flags: 0,
        };
        let (mut image, mut upload_buf, mut dst_aligned, mut dst_packed, mut dst_crop) =
            (null, null, null, null, null);
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &tex_desc, 0, null.cast(), iid, &mut image),
            S_OK
        );
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &buffer(1024), 0, null.cast(), iid, &mut upload_buf),
            S_OK
        );
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &buffer(2048), 0, null.cast(), iid, &mut dst_aligned),
            S_OK
        );
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &buffer(256), 0, null.cast(), iid, &mut dst_packed),
            S_OK
        );
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &buffer(1024), 0, null.cast(), iid, &mut dst_crop),
            S_OK
        );

        // Fill a 4x4 RGBA8 pattern (row_pitch 256): pixel (x, y) = [x*40+1, y*50+2, (x+y)*20+3, 0xff].
        let (up_ptr, _) = buffer_bytes(upload_buf).unwrap();
        // SAFETY: upload_buf holds 1024 bytes.
        let up = unsafe { std::slice::from_raw_parts_mut(up_ptr, 1024) };
        for y in 0..4usize {
            for x in 0..4usize {
                let at = y * 256 + x * 4;
                up[at..at + 4].copy_from_slice(&[
                    (x * 40 + 1) as u8,
                    (y * 50 + 2) as u8,
                    ((x + y) * 20 + 3) as u8,
                    0xff,
                ]);
            }
        }

        let queue_iid = guid(
            0x0ec870a6,
            0x5d7e,
            0x4c22,
            [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed],
        );
        let allocator_iid = guid(
            0x6102dee4,
            0xaf59,
            0x4b09,
            [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24],
        );
        let list_iid = guid(
            0x5b160d0f,
            0xac1b,
            0x4185,
            [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55],
        );
        let (mut queue, mut allocator, mut list) = (null, null, null);
        let queue_desc = [0u32; 4];
        assert_eq!(
            crate::queue::create(null, queue_desc.as_ptr().cast(), &queue_iid, &mut queue),
            S_OK
        );
        assert_eq!(
            crate::list::create_allocator(null, 0, &allocator_iid, &mut allocator),
            S_OK
        );
        assert_eq!(
            crate::list::create_list(null, 0, 0, allocator, null, &list_iid, &mut list),
            S_OK
        );

        #[repr(C)]
        struct Location {
            resource: This,
            kind: u32,
            footprint: PlacedFootprint,
        }
        type Copy =
            extern "system" fn(This, *const Location, u32, u32, u32, *const Location, *const [u32; 6]);
        let copy = slot::<Copy>(list, 0x80);

        let full_rows = PlacedFootprint {
            offset: 0,
            format: 28,
            width: 4,
            height: 4,
            depth: 1,
            row_pitch: 256,
        };
        let tex_loc = Location {
            resource: image,
            kind: 0,
            footprint: PlacedFootprint { offset: 0, ..full_rows },
        };
        let up_loc = Location {
            resource: upload_buf,
            kind: 1,
            footprint: full_rows,
        };
        // 1) Buffer -> Texture upload.
        copy(list, &tex_loc, 0, 0, 0, &up_loc, std::ptr::null());

        // 2) Texture -> Buffer readback at non-zero offset (256), 256-aligned row pitch.
        let aligned_loc = Location {
            resource: dst_aligned,
            kind: 1,
            footprint: PlacedFootprint {
                offset: 256,
                ..full_rows
            },
        };
        copy(list, &aligned_loc, 0, 0, 0, &tex_loc, std::ptr::null());

        // 3) Texture -> Buffer readback with unaligned row_pitch (16 bytes, packed).
        let packed_loc = Location {
            resource: dst_packed,
            kind: 1,
            footprint: PlacedFootprint {
                offset: 32,
                format: 28,
                width: 4,
                height: 4,
                depth: 1,
                row_pitch: 16,
            },
        };
        copy(list, &packed_loc, 0, 0, 0, &tex_loc, std::ptr::null());

        // 4) Texture -> Buffer sub-box readback: 2x2 crop from (1, 1) to (3, 3).
        let crop_box = [1u32, 1, 0, 3, 3, 1];
        let crop_loc = Location {
            resource: dst_crop,
            kind: 1,
            footprint: PlacedFootprint {
                offset: 0,
                format: 28,
                width: 2,
                height: 2,
                depth: 1,
                row_pitch: 256,
            },
        };
        copy(list, &crop_loc, 0, 0, 0, &tex_loc, &crop_box);

        // Both a source box and a whole subresource can fill only part of a footprint.
        let larger = PlacedFootprint {
            width: 6,
            height: 6,
            row_pitch: 24,
            ..full_rows
        };
        let partials = [(full_rows, 0, 0, Some(crop_box)), (larger, 1, 1, None)].map(
            |(footprint, x, y, region)| {
                let size = u64::from(footprint.row_pitch) * u64::from(footprint.height);
                let mut output = null;
                assert_eq!(
                    create_committed(
                        null, &PROPERTIES, 0, &buffer(size), 0, null.cast(), iid, &mut output,
                    ),
                    S_OK
                );
                let (data, len) = buffer_bytes(output).unwrap();
                // SAFETY: fill this mapped destination before the GPU copy.
                unsafe { data.write_bytes(0xa5, len as usize) };
                let dst = Location {
                    resource: output,
                    kind: 1,
                    footprint,
                };
                let src_box = region.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
                copy(list, &dst, x, y, 0, &tex_loc, src_box);
                (output, footprint, x, y, region)
            },
        );

        // 5) Console formats 118..=120 (3 B/texel) map to 4-byte PC formats
        // (45..=47): skipped cleanly without breaking Close() or touching dst.
        let mut dst_d16s8 = null;
        assert_eq!(
            create_committed(null, &PROPERTIES, 0, &buffer(1024), 0, null.cast(), iid, &mut dst_d16s8),
            S_OK
        );
        let d16s8_loc = Location {
            resource: dst_d16s8,
            kind: 1,
            footprint: PlacedFootprint {
                offset: 0,
                format: 118,
                width: 4,
                height: 4,
                depth: 1,
                row_pitch: 256,
            },
        };
        copy(list, &d16s8_loc, 0, 0, 0, &tex_loc, std::ptr::null());

        assert_eq!(slot::<extern "system" fn(This) -> HRESULT>(list, 0x48)(list), S_OK);
        slot::<extern "system" fn(This, u32, *const This)>(queue, 0x50)(queue, 1, &list);

        // Verify 256-aligned readback at offset 256 (leading 256 bytes untouched).
        let (al_ptr, _) = buffer_bytes(dst_aligned).unwrap();
        // SAFETY: dst_aligned holds 2048 bytes.
        let al = unsafe { std::slice::from_raw_parts(al_ptr, 2048) };
        assert!(al[..256].iter().all(|&b| b == 0), "bytes before offset touched");
        for row in 0..4 {
            let at = 256 + row * 256;
            assert_eq!(al[at..at + 16], up[row * 256..row * 256 + 16], "aligned row {row}");
        }

        // Verify unaligned packed readback (row_pitch 16, offset 32).
        let (pk_ptr, _) = buffer_bytes(dst_packed).unwrap();
        // SAFETY: dst_packed holds 256 bytes.
        let pk = unsafe { std::slice::from_raw_parts(pk_ptr, 256) };
        assert!(pk[..32].iter().all(|&b| b == 0), "bytes before packed offset touched");
        for row in 0..4 {
            let at = 32 + row * 16;
            assert_eq!(pk[at..at + 16], up[row * 256..row * 256 + 16], "packed row {row}");
        }

        // Verify 2x2 sub-box crop from (1, 1)..(3, 3).
        let (cr_ptr, _) = buffer_bytes(dst_crop).unwrap();
        // SAFETY: dst_crop holds 1024 bytes.
        let cr = unsafe { std::slice::from_raw_parts(cr_ptr, 1024) };
        for row in 0..2 {
            let src_at = (row + 1) * 256 + 4; // x=1, y=row+1
            let dst_at = row * 256;
            assert_eq!(cr[dst_at..dst_at + 8], up[src_at..src_at + 8], "crop row {row}");
        }

        // Verify skipped 118..=120 readback left its buffer untouched.
        let (d16_ptr, _) = buffer_bytes(dst_d16s8).unwrap();
        // SAFETY: dst_d16s8 holds 1024 bytes.
        let d16 = unsafe { std::slice::from_raw_parts(d16_ptr, 1024) };
        assert!(d16.iter().all(|&b| b == 0), "mismatched format 118 buffer touched");

        for (output, footprint, dst_x, dst_y, region) in partials {
            let [left, top, _, right, bottom, _] = region.unwrap_or([0, 0, 0, 4, 4, 1]);
            let (data, len) = buffer_bytes(output).unwrap();
            // SAFETY: the destination is still mapped after the submission.
            let pixels = unsafe { std::slice::from_raw_parts(data, len as usize) };
            for y in 0..footprint.height {
                for x in 0..footprint.width {
                    let at = (y * footprint.row_pitch + x * 4) as usize;
                    let expected: &[u8] = if (dst_x..dst_x + (right - left)).contains(&x)
                        && (dst_y..dst_y + (bottom - top)).contains(&y)
                    {
                        let src_at = ((top + y - dst_y) * 256 + (left + x - dst_x) * 4) as usize;
                        &up[src_at..src_at + 4]
                    } else {
                        &[0xa5; 4]
                    };
                    assert_eq!(
                        &pixels[at..at + 4], expected,
                        "pixel ({x}, {y}) in {footprint:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn committed_and_placed_resources_map_at_their_gpu_address() {
        let null = std::ptr::null();
        let mut committed = null;
        let hr = create_committed(
            null,
            &PROPERTIES,
            0,
            &buffer(256),
            0,
            null.cast(),
            &RESOURCE_IIDS[0],
            &mut committed,
        );
        assert_eq!(hr, S_OK);
        let mut data = std::ptr::null_mut();
        assert_eq!(
            slot::<Map>(committed, 0x40)(committed, 0, null.cast(), &mut data),
            S_OK
        );
        assert_eq!(slot::<Address>(committed, 0x58)(committed), data as u64);
        // SAFETY: the buffer is at least 256 zeroed bytes.
        assert_eq!(unsafe { *data.add(255) }, 0);
        assert_eq!(slot::<Release>(committed, 0x10)(committed), 0);

        let heap = heap(4 * KIB_64);
        let place = |offset, desc: &ResourceDesc, out: &mut This| {
            create_placed(
                null,
                heap,
                offset,
                desc,
                0,
                null.cast(),
                &RESOURCE_IIDS[0],
                out,
            )
        };
        let mut placed = null;
        assert_eq!(place(KIB_64, &buffer(KIB_64), &mut placed), S_OK);
        // SAFETY: a heap we made.
        let base = unsafe { (*heap.cast::<Heap>()).memory.ptr.as_ptr() } as u64;
        assert_eq!(slot::<Address>(placed, 0x58)(placed), base + KIB_64);
        let mut other = null;
        assert_eq!(
            place(4 * KIB_64, &buffer(1), &mut other),
            E_INVALIDARG,
            "past the end"
        );
        assert_eq!(
            place(100, &buffer(1), &mut other),
            E_INVALIDARG,
            "misaligned"
        );
        // The resource keeps the heap alive.
        assert_eq!(slot::<Release>(heap, 0x10)(heap), 1);
        assert_eq!(slot::<Release>(placed, 0x10)(placed), 0);
    }
}
