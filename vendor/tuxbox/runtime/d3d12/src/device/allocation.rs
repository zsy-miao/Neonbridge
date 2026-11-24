//! Frosty reads a 16-byte result and hashes each 56-byte resource description.
//! Slot 25 passes the result in RCX and the receiver in RDX (title RVA 0x57945e9).
use super::Device;
use core::slice;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B5G5R5A1_UNORM, DXGI_FORMAT_R16_UINT, DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R9G9B9E5_SHAREDEXP,
};
use windows::core::Interface;

const _: () = assert!(
    size_of::<D3D12_RESOURCE_DESC>() == 56 && size_of::<D3D12_RESOURCE_ALLOCATION_INFO>() == 16
);

// DCC is selected by the console. The host driver chooses compression for its
// own image, so allocation and copy-footprint queries must match image creation.
pub(super) const DCC_FLAGS: i32 = 0x00c0_0000;

fn host_description(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_DESC> {
    use super::format::HOST_FLAGS;
    // Console-only flag bits (including DCC) are dropped; the host driver
    // chooses its own compression for the image it creates.
    super::depth::primary(desc).or_else(|| {
        (desc.Flags.0 & !HOST_FLAGS != 0)
            .then(|| D3D12_RESOURCE_DESC { Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & HOST_FLAGS), ..*desc })
    })
}

// Preserve the packed render/sampling format. A raw 16-bit storage view can
// share those exact bits even on adapters without typed storage for BGRA5551.
pub(super) fn packed_uav_formats(format: DXGI_FORMAT, flags: D3D12_RESOURCE_FLAGS) -> &'static [DXGI_FORMAT] {
    if format == DXGI_FORMAT_B5G5R5A1_UNORM && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS) {
        &[DXGI_FORMAT_R16_UINT]
    } else if format == DXGI_FORMAT_R32_UINT
        && flags.contains(D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET) {
        // GTA renders packed HDR colour as R32_UINT and samples it as R9G9B9E5.
        // Not for UAV-only images: a castable list made their own-format UAVs invalid.
        &[DXGI_FORMAT_R9G9B9E5_SHAREDEXP]
    } else {
        &[]
    }
}

pub(super) fn packed_requirements(
    device: &ID3D12Device, visible: u32, desc: &D3D12_RESOURCE_DESC1,
) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    let formats = packed_uav_formats(desc.Format, desc.Flags);
    if formats.is_empty() { return None; }
    let native = D3D12_RESOURCE_DESC1 { Flags: D3D12_RESOURCE_FLAGS(desc.Flags.0 & 0x1ff), ..*desc };
    let count = formats.len() as u32;
    let entries = formats.as_ptr();
    let api: ID3D12Device12 = device.cast().ok()?;
    Some(unsafe { api.GetResourceAllocationInfo3(visible, 1, &native, Some(&count), Some(&entries), None) })
}

// Like GetResourceAllocationInfo, this Xbox vtable entry places the struct
// return pointer before the device. GTA queries default, upload and readback.
pub(super) unsafe extern "system" fn custom_heap_properties(
    out: *mut D3D12_HEAP_PROPERTIES,
    device: &Device,
    node_mask: u32,
    heap_type: D3D12_HEAP_TYPE,
) -> *mut D3D12_HEAP_PROPERTIES {
    if !out.is_null() {
        unsafe { out.write(device.native.GetCustomHeapProperties(node_mask, heap_type)) };
    }
    out
}

/// TUXBOX_TRACE_ALLOCATION_BYTES: up to 16 more answer sizes for the size
/// watch, decimal, 0x hex or `invalid` (u64::MAX, a failed answer). TuxBox
/// be20365 (Valdi320, Apache-2.0), folded into the watch below.
fn traced_sizes() -> &'static [u64] {
    static SIZES: std::sync::OnceLock<Vec<u64>> = std::sync::OnceLock::new();
    SIZES.get_or_init(|| {
        let Ok(value) = std::env::var("TUXBOX_TRACE_ALLOCATION_BYTES") else { return Vec::new() };
        let mut sizes = Vec::new();
        for item in value.split(',').map(str::trim) {
            let bytes = match item.strip_prefix("0x") {
                Some(hex) => u64::from_str_radix(hex, 16).ok(),
                None if item == "invalid" => Some(u64::MAX),
                None => item.parse().ok(),
            };
            match bytes {
                Some(b) if !sizes.contains(&b) && sizes.len() < 16 => sizes.push(b),
                Some(_) => {}
                None => return Vec::new(),
            }
        }
        sizes
    })
}

/// Size/alignment watch (TUXBOX_SIZE_WATCH=0 off): 160x90 textures, small
/// format 24/26 textures and 0x4b0000-byte answers, with inputs, answer and callers.
fn size_watch(slot: u32, descs: &[D3D12_RESOURCE_DESC], size: u64, alignment: u64) {
    let wanted = descs.iter().any(|d| (d.Width == 160 && d.Height == 90)
        || (matches!(d.Format.0, 24 | 26) && d.Width <= 320 && d.Height <= 180)) || size == 0x4b_0000
        || traced_sizes().contains(&size);
    if !wanted || std::env::var_os("TUXBOX_SIZE_WATCH").is_some_and(|v| v == "0") {
        return;
    }
    static LINES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    if LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 64 {
        return;
    }
    let inputs: Vec<String> = descs.iter().take(4).map(|d| format!("dim {} {}x{}x{} mips {} format {} layout {} flags {:#x} align-in {:#x}",
        d.Dimension.0, d.Width, d.Height, d.DepthOrArraySize, d.MipLevels, d.Format.0, d.Layout.0, d.Flags.0, d.Alignment)).collect();
    crate::diagnostics::log_line(format_args!("size query slot {slot}: [{}] -> size {size:#x} alignment {alignment:#x}; callers {}",
        inputs.join("; "), crate::diagnostics::audio::callers()));
}

pub(super) unsafe extern "system" fn info_watched(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let result = unsafe { info(out, device, visible, count, descs) };
    if !out.is_null() && !descs.is_null() && count > 0 {
        let answer = unsafe { *out };
        size_watch(25, unsafe { slice::from_raw_parts(descs, count as usize) }, answer.SizeInBytes, answer.Alignment);
    }
    result
}

/// TUXBOX_CONSOLE_SIZES=0: only Xbox layouts 0x100-0x10a and packed UAVs get
/// console sizes; other descriptions get the host's answer, as before.
/// On (leonida-ce 9bd2a8d3 / VIctory resource.rs, re-derived): the title
/// carves its GPU allocations from a pool it reserves at start, sized from
/// these answers; PC answers (64 KiB per small texture) filled it ~87 s in.
fn console_sizes() -> bool {
    crate::env_toggle!("TUXBOX_CONSOLE_SIZES") != Some("0")
}

/// The console footprint of one description: buffers are their width at
/// 64 KiB; textures of the default (unknown) layout take XG's 4 KiB pick, row
/// major takes the linear layout, Xbox layouts their own. None when the
/// console layout cannot size it (the host answer is kept then).
fn console_item(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        let size = desc.Width.checked_next_multiple_of(65536).filter(|&n| n != 0)?;
        return Some(D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: size, Alignment: 65536 });
    }
    if super::placement::nv12_pitch(desc).is_some() {
        return None;
    }
    if super::plane_size::enabled() && let Some(answer) = super::plane_size::console_size(desc) {
        return Some(answer);
    }
    if !matches!(desc.Layout.0, 0 | 1 | 0x100..=0x10a) {
        return None;
    }
    let console = super::placement::console_layout(desc);
    let plane = super::placement::color_plane(&console).ok()?;
    (plane.size != 0 && plane.alignment != 0).then(|| D3D12_RESOURCE_ALLOCATION_INFO {
        SizeInBytes: plane.size, Alignment: plane.alignment.max(desc.Alignment),
    })
}

/// The batch's console footprint, each item at its alignment, or None if
/// any item has no console size.
pub(super) fn console_footprint(descs: &[D3D12_RESOURCE_DESC]) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    let mut bytes = 0u64;
    let mut alignment = 256u64;
    for desc in descs {
        let item = console_item(desc)?;
        bytes = bytes.checked_next_multiple_of(item.Alignment)?.checked_add(item.SizeInBytes)?;
        alignment = alignment.max(item.Alignment);
    }
    Some(D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: bytes.checked_next_multiple_of(alignment)?, Alignment: alignment })
}

/// A depth-stencil or multisampled description the desktop rejects keeps
/// the desktop failure (nothing shows the console accepts it), except
/// formats 118-120, which have no desktop form (Xbox2Windows 992116ec).
fn desktop_accepts_planes(device: &Device, descs: &[D3D12_RESOURCE_DESC]) -> bool {
    !super::plane_size::enabled() || descs.iter().all(|desc| {
        super::plane_size::console_size(desc).is_none() || super::plane_size::answers_over_failure(desc)
            || unsafe { device.native.GetResourceAllocationInfo(0, slice::from_ref(desc)) }.SizeInBytes != u64::MAX
    })
}

pub(super) unsafe extern "system" fn info(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let Some(out) = (unsafe { out.as_mut() }) else {
        return out;
    };
    let invalid = D3D12_RESOURCE_ALLOCATION_INFO {
        SizeInBytes: u64::MAX,
        Alignment: 0,
    };
    if descs.is_null() || count == 0 || visible > 1 {
        *out = invalid;
        return out;
    }
    let descs = unsafe { slice::from_raw_parts(descs, count as usize) };
    if console_sizes() && desktop_accepts_planes(device, descs) && let Some(answer) = console_footprint(descs) {
        *out = answer;
        return out;
    }
    if descs.iter().any(|d| matches!(d.Layout.0, 0x100..=0x102 | 0x105 | 0x109 | 0x10a)
        || !packed_uav_formats(d.Format, d.Flags).is_empty()) {
        // The guest will allocate these bytes itself and pass the address to
        // CreatePlacedResourceX. Account for the Xbox planes and batch padding.
        let mut bytes = 0u64;
        let mut alignment = 256;
        for desc in descs {
            let layout = if let Some(pitch) = super::placement::nv12_pitch(desc) {
                let y = pitch * u64::from(desc.Height);
                D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: y + y / 2, Alignment: 256 }
            } else if matches!(desc.Layout.0, 0x100..=0x102 | 0x105 | 0x109 | 0x10a) {
                match super::placement::color_plane(desc) {
                    Ok(plane) => D3D12_RESOURCE_ALLOCATION_INFO {
                        SizeInBytes: plane.size,
                        Alignment: plane.alignment.max(desc.Alignment),
                    },
                    Err(_) => invalid,
                }
            } else if !packed_uav_formats(desc.Format, desc.Flags).is_empty() {
                let native = D3D12_RESOURCE_DESC1 {
                    Dimension: desc.Dimension, Alignment: desc.Alignment, Width: desc.Width, Height: desc.Height,
                    DepthOrArraySize: desc.DepthOrArraySize, MipLevels: desc.MipLevels, Format: desc.Format,
                    SampleDesc: desc.SampleDesc, Layout: desc.Layout, Flags: desc.Flags, ..Default::default()
                };
                packed_requirements(&device.native, visible, &native).unwrap_or(invalid)
            } else {
                let mut item = invalid;
                unsafe { info(&mut item, device, visible, 1, desc) };
                item
            };
            if layout.Alignment == 0 || layout.SizeInBytes == u64::MAX {
                static BAD_LAYOUT: std::sync::Once = std::sync::Once::new();
                crate::diagnostics::log_once(&BAD_LAYOUT, || format!("Xbox layout {:#x} dim={} {}x{}x{} format={} flags={:#x} alignment={} mips={} samples={}", desc.Layout.0, desc.Dimension.0, desc.Width, desc.Height, desc.DepthOrArraySize, desc.Format.0, desc.Flags.0, desc.Alignment, desc.MipLevels, desc.SampleDesc.Count));
                *out = invalid;
                return out;
            }
            let Some(end) = bytes.checked_next_multiple_of(layout.Alignment)
                .and_then(|offset| offset.checked_add(layout.SizeInBytes)) else {
                *out = invalid;
                return out;
            };
            bytes = end;
            alignment = alignment.max(layout.Alignment);
        }
        *out = D3D12_RESOURCE_ALLOCATION_INFO {
            SizeInBytes: bytes.checked_next_multiple_of(alignment).unwrap_or(u64::MAX), Alignment: alignment,
        };
        return out;
    }
    let supported = descs.iter().all(|d| {
        matches!(
            d.Dimension,
            D3D12_RESOURCE_DIMENSION_BUFFER
                | D3D12_RESOURCE_DIMENSION_TEXTURE1D
                | D3D12_RESOURCE_DIMENSION_TEXTURE2D
                | D3D12_RESOURCE_DIMENSION_TEXTURE3D
        ) && matches!(
            d.Layout,
            D3D12_TEXTURE_LAYOUT_UNKNOWN | D3D12_TEXTURE_LAYOUT_ROW_MAJOR
        ) && d.Width != 0
            && d.Height != 0
            && d.DepthOrArraySize != 0
            && d.SampleDesc.Count != 0
    });
    *out = if supported {
        // Let the same adapter that creates the placed resource determine its
        // requirements, including padding between descriptors in a batch.
        if descs.iter().any(|d| host_description(d).is_some()) {
            let mut host = Vec::new();
            if host.try_reserve_exact(descs.len()).is_err() {
                *out = invalid;
                return out;
            }
            host.extend(descs.iter().map(|d| host_description(d).unwrap_or(*d)));
            unsafe { device.native.GetResourceAllocationInfo(visible, &host) }
        } else {
            unsafe { device.native.GetResourceAllocationInfo(visible, descs) }
        }
    } else {
        invalid
    };
    if out.SizeInBytes == u64::MAX {
        let d = descs[0];
        static INVALID: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&INVALID, || format!("allocation requires dim={} {}x{}x{} mips={} format={} layout={:#x} flags={:#x} align={}", d.Dimension.0,d.Width,d.Height,d.DepthOrArraySize,d.MipLevels,d.Format.0,d.Layout.0,d.Flags.0,d.Alignment));
    }
    out
}

// Device8 extends each description with a sampler-feedback mip region and may
// return a placement offset, alignment and size for each individual resource.
pub(super) unsafe extern "system" fn info2_watched(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC1,
    details: *mut D3D12_RESOURCE_ALLOCATION_INFO1,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let result = unsafe { info2(out, device, visible, count, descs, details) };
    if !out.is_null() && !descs.is_null() && count > 0 {
        let answer = unsafe { *out };
        let plain: Vec<D3D12_RESOURCE_DESC> = unsafe { slice::from_raw_parts(descs, count as usize) }.iter().map(|d| D3D12_RESOURCE_DESC {
            Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height, DepthOrArraySize: d.DepthOrArraySize,
            MipLevels: d.MipLevels, Format: d.Format, SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
        }).collect();
        size_watch(108, &plain, answer.SizeInBytes, answer.Alignment);
    }
    result
}

/// palmetto cc7e044b (GPL-3.0): a description the host cannot size got an
/// invalid answer (u64::MAX) the title then used. It now gets a conservative
/// 64 KiB-aligned size: 16 bytes a texel (the widest format) for every
/// sample, slice and mip, twice the top mip for a chain. Logged.
/// TUXBOX_SLOT108_FALLBACK=0 answers invalid again.
fn fallback_size(desc: &D3D12_RESOURCE_DESC) -> Option<D3D12_RESOURCE_ALLOCATION_INFO> {
    if crate::env_toggle!("TUXBOX_SLOT108_FALLBACK") == Some("0") { return None; }
    let bytes = if desc.Dimension == D3D12_RESOURCE_DIMENSION_BUFFER {
        desc.Width
    } else {
        let texels = desc.Width.checked_mul(u64::from(desc.Height.max(1)))?.checked_mul(u64::from(desc.DepthOrArraySize.max(1)))?;
        let chain = if desc.MipLevels == 1 { 1 } else { 2 };
        texels.checked_mul(16 * u64::from(desc.SampleDesc.Count.max(1)) * chain)?
    };
    let size = bytes.max(1).checked_next_multiple_of(0x10000)?;
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n <= 4 || n.is_power_of_two() {
        crate::diagnostics::log_line(format_args!(
            "allocation info2: answered {size:#x} bytes at 64 KiB for a description the host could not size (format {} {}x{}; {n} so far; TUXBOX_SLOT108_FALLBACK=0 answers invalid)",
            desc.Format.0, desc.Width, desc.Height));
    }
    Some(D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: size, Alignment: 0x10000 })
}

pub(super) unsafe extern "system" fn info2(
    out: *mut D3D12_RESOURCE_ALLOCATION_INFO,
    device: &Device,
    visible: u32,
    count: u32,
    descs: *const D3D12_RESOURCE_DESC1,
    details: *mut D3D12_RESOURCE_ALLOCATION_INFO1,
) -> *mut D3D12_RESOURCE_ALLOCATION_INFO {
    let Some(result) = (unsafe { out.as_mut() }) else { return out };
    let invalid = D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: u64::MAX, Alignment: 0 };
    *result = invalid;
    if descs.is_null() || count == 0 || visible > 1 { return out; }
    let inputs = unsafe { slice::from_raw_parts(descs, count as usize) };
    let mut offset = 0u64;
    let mut alignment = 1;
    let mut placements = Vec::new();
    if !details.is_null() && placements.try_reserve_exact(inputs.len()).is_err() { return out; }
    for d in inputs {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height,
            DepthOrArraySize: d.DepthOrArraySize, MipLevels: d.MipLevels, Format: d.Format,
            SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
        };
        let console = console_sizes() && console_item(&desc).is_some();
        let requirement = if d.Layout.0 >= 0x100 || console {
            let mut item = invalid;
            unsafe { info(&mut item, device, visible, 1, &desc) };
            item
        } else if let Some(requirement) = packed_requirements(&device.native, visible, d) {
            requirement
        } else if let Ok(host) = device.native.cast::<ID3D12Device8>() {
            // Console-only flag bits (DCC and others) are dropped, as at creation.
            let mut native = *d;
            native.Flags.0 &= super::format::HOST_FLAGS;
            let guest = D3D12_RESOURCE_DESC {
                Dimension: d.Dimension, Alignment: d.Alignment, Width: d.Width, Height: d.Height,
                DepthOrArraySize: d.DepthOrArraySize, MipLevels: d.MipLevels, Format: d.Format,
                SampleDesc: d.SampleDesc, Layout: d.Layout, Flags: d.Flags,
            };
            if let Some(primary) = super::depth::primary(&guest) {
                native.Format = primary.Format;
                native.Flags = primary.Flags;
            }
            unsafe { host.GetResourceAllocationInfo2(visible, 1, &native, None) }
        } else { return out };
        let requirement = if requirement.Alignment == 0 || requirement.SizeInBytes == u64::MAX {
            crate::diagnostics::recording_error(format_args!("allocation info2 dim={} {}x{} format={} flags={:#x} layout={:#x} alignment={}", d.Dimension.0, d.Width, d.Height, d.Format.0, d.Flags.0, d.Layout.0, d.Alignment));
            match fallback_size(&desc) { Some(r) => r, None => return out }
        } else { requirement };
        let Some(start) = offset.checked_next_multiple_of(requirement.Alignment) else { return out };
        let Some(end) = start.checked_add(requirement.SizeInBytes) else { return out };
        if !details.is_null() {
            placements.push(D3D12_RESOURCE_ALLOCATION_INFO1 {
                Offset: start, Alignment: requirement.Alignment, SizeInBytes: requirement.SizeInBytes,
            });
        }
        offset = end;
        alignment = alignment.max(requirement.Alignment);
    }
    let Some(total) = offset.checked_next_multiple_of(alignment) else { return out };
    *result = D3D12_RESOURCE_ALLOCATION_INFO { SizeInBytes: total, Alignment: alignment };
    if !placements.is_empty() {
        unsafe { core::ptr::copy_nonoverlapping(placements.as_ptr(), details, placements.len()) };
    }
    out
}

// The shipping texture upload routine (RVA 0x57e12a1) supplies independent
// arrays with strides 32, 4 and 8 for layout, row count and unpadded row size.
pub(super) unsafe extern "system" fn footprints(
    device: &Device,
    desc: *const D3D12_RESOURCE_DESC,
    first: u32,
    count: u32,
    base: u64,
    layouts: *mut D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    rows: *mut u32,
    row_bytes: *mut u64,
    total: *mut u64,
) {
    const _: () = assert!(size_of::<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>() == 32);
    let native = unsafe { desc.as_ref() }.and_then(host_description);
    let desc = native.as_ref().map_or(desc, core::ptr::from_ref);
    // All four output arrays are optional. Their storage is owned by the guest.
    unsafe {
        (device.native.vtable().GetCopyableFootprints)(
            device.native.as_raw(),
            desc,
            first,
            count,
            base,
            layouts,
            rows,
            row_bytes,
            total,
        );
    }
}

#[cfg(test)]
mod console_size_tests {
    use super::*;

    fn texture(width: u64, height: u32, format: i32, layout: i32, flags: i32) -> D3D12_RESOURCE_DESC {
        D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D, Width: width, Height: height, DepthOrArraySize: 1,
            MipLevels: 1, Format: windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT(format),
            SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT(layout), Flags: D3D12_RESOURCE_FLAGS(flags), ..Default::default()
        }
    }

    #[test]
    fn buffers_are_their_width_at_64k() {
        let buffer = D3D12_RESOURCE_DESC { Dimension: D3D12_RESOURCE_DIMENSION_BUFFER, Width: 100, ..texture(100, 1, 0, 1, 0) };
        let item = console_item(&buffer).unwrap();
        assert_eq!((item.SizeInBytes, item.Alignment), (65536, 65536));
    }

    #[test]
    fn a_small_default_texture_is_far_below_megabytes() {
        // 160x90 RGBA8: the PC answer path once asked 4.9 MB for this.
        let item = console_item(&texture(160, 90, 28, 0, 0)).unwrap();
        assert!(item.SizeInBytes < 0x2_0000, "{:#x}", item.SizeInBytes);
        assert_eq!(item.SizeInBytes % 4096, 0);
        let batch = console_footprint(&[texture(160, 90, 28, 0, 0), texture(160, 90, 28, 0, 0)]).unwrap();
        assert!(batch.SizeInBytes >= 2 * item.SizeInBytes);
    }
}
