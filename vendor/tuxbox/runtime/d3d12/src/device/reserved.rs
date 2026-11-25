//! Reserved resources and the device queries that go with tile mapping.
//! Console swizzle layouts stay refused. The host creates only a desktop
//! 64 KiB reserved resource, which has no heap until a queue maps tiles.

use super::{Device, Out, depth, format, heap, host_state, identity};
use crate::proxy::check_output;
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_OUTOFMEMORY};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN;
use windows::core::{GUID, HRESULT, Interface};

fn reserved_desc(desc: &D3D12_RESOURCE_DESC) -> Result<(), HRESULT> {
    let alignment = desc.Alignment;
    if alignment != 0
        && alignment != u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT)
        && alignment != u64::from(D3D12_DEFAULT_MSAA_RESOURCE_PLACEMENT_ALIGNMENT)
    {
        return Err(E_INVALIDARG);
    }
    if desc.Width == 0 || desc.SampleDesc.Count == 0 {
        return Err(E_INVALIDARG);
    }
    match desc.Dimension {
        D3D12_RESOURCE_DIMENSION_BUFFER => {
            if desc.Layout.0 >= 0x100 {
                return Err(E_NOTIMPL);
            }
            if desc.Layout != D3D12_TEXTURE_LAYOUT_ROW_MAJOR
                || desc.Height != 1
                || desc.DepthOrArraySize != 1
                || desc.MipLevels != 1
                || desc.Format != DXGI_FORMAT_UNKNOWN
                || desc.SampleDesc.Count != 1
                || desc.SampleDesc.Quality != 0
            {
                return Err(E_INVALIDARG);
            }
            Ok(())
        }
        D3D12_RESOURCE_DIMENSION_TEXTURE1D
        | D3D12_RESOURCE_DIMENSION_TEXTURE2D
        | D3D12_RESOURCE_DIMENSION_TEXTURE3D => {
            if desc.Height == 0 || desc.DepthOrArraySize == 0 || desc.MipLevels == 0 {
                return Err(E_INVALIDARG);
            }
            match desc.Layout {
                D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE
                | D3D12_TEXTURE_LAYOUT_64KB_STANDARD_SWIZZLE => Ok(()),
                layout if layout.0 >= 0x100 => Err(E_NOTIMPL),
                _ => Err(E_INVALIDARG),
            }
        }
        _ => Err(E_INVALIDARG),
    }
}

pub(super) unsafe extern "system" fn create(
    device: &Device,
    desc: *const D3D12_RESOURCE_DESC,
    initial: D3D12_RESOURCE_STATES,
    clear: *const D3D12_CLEAR_VALUE,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    let status = unsafe { check_output::<ID3D12Resource>(iid, out) };
    if status.is_err() {
        return status;
    }
    let Some(desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    if let Err(error) = reserved_desc(desc) {
        return error;
    }
    let mut host_desc = *desc;
    host_desc.Flags.0 &= format::HOST_FLAGS;
    let initial = format::host_initial_state(desc, initial);
    let texture = desc.Dimension != D3D12_RESOURCE_DIMENSION_BUFFER;
    let host_clear = format::host_clear(clear);
    let mut native = None;
    if let Err(error) = unsafe {
        device.native.CreateReservedResource::<ID3D12Resource>(
            &host_desc,
            host_state(initial, texture),
            Some(host_clear.as_ref().map_or(clear, core::ptr::from_ref)),
            &mut native,
        )
    } {
        return error.code();
    }
    let properties = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    // The guest description is what views and copies report. The host image
    // is the reserved resource, so it has no heap of its own.
    unsafe { depth::wrap::<false>(device, &properties, desc, None, initial, native, out) }
}

fn pageables(
    device: &Device,
    count: u32,
    objects: *const *mut c_void,
) -> Result<Vec<Option<ID3D12Pageable>>, HRESULT> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if objects.is_null() {
        return Err(E_INVALIDARG);
    }
    let mut native = Vec::new();
    native.try_reserve_exact(count as usize).map_err(|_| E_OUTOFMEMORY)?;
    let owner = identity(device).as_raw();
    for i in 0..count as usize {
        // SAFETY: the guest supplies count borrowed COM pointers.
        let object = unsafe { objects.add(i).read() };
        let Some(pageable) = (unsafe { heap::pageable(object, owner) }) else {
            return Err(E_INVALIDARG);
        };
        native.push(Some(pageable));
    }
    Ok(native)
}

pub(super) unsafe extern "system" fn make_resident(
    device: &Device,
    count: u32,
    objects: *const *mut c_void,
) -> HRESULT {
    if count == 0 {
        return HRESULT(0);
    }
    match pageables(device, count, objects) {
        Ok(native) => unsafe { device.native.MakeResident(&native).map_or_else(|e| e.code(), |()| HRESULT(0)) },
        Err(error) => error,
    }
}

pub(super) unsafe extern "system" fn evict(
    device: &Device,
    count: u32,
    objects: *const *mut c_void,
) -> HRESULT {
    if count == 0 {
        return HRESULT(0);
    }
    match pageables(device, count, objects) {
        Ok(native) => unsafe { device.native.Evict(&native).map_or_else(|e| e.code(), |()| HRESULT(0)) },
        Err(error) => error,
    }
}

fn is_reserved(resource: &ID3D12Resource) -> Result<bool, HRESULT> {
    let mut properties = D3D12_HEAP_PROPERTIES::default();
    // A reserved resource has no heap. GetHeapProperties reports that as E_INVALIDARG.
    match unsafe { resource.GetHeapProperties(Some(core::ptr::from_mut(&mut properties)), None) } {
        Err(error) if error.code() == E_INVALIDARG => Ok(true),
        Ok(()) => Ok(false),
        Err(error) => Err(error.code()),
    }
}

pub(super) unsafe extern "system" fn tiling(
    device: &Device,
    resource: *mut c_void,
    tiles: *mut u32,
    packed: *mut D3D12_PACKED_MIP_INFO,
    shape: *mut D3D12_TILE_SHAPE,
    count: *mut u32,
    first: u32,
    tilings: *mut D3D12_SUBRESOURCE_TILING,
) {
    let owner = identity(device).as_raw();
    let Some(resource) = (unsafe { super::resource(resource, owner) }) else {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || "GetResourceTiling resource".to_string());
        return;
    };
    match is_reserved(resource) {
        Ok(true) => {}
        Ok(false) => {
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || "GetResourceTiling resource is not reserved".to_string());
            return;
        }
        Err(error) => {
            static FIRST: std::sync::Once = std::sync::Once::new();
            crate::diagnostics::log_once(&FIRST, || format!("GetResourceTiling {:#010x}", error.0));
            return;
        }
    }
    if let Some(asked) = unsafe { count.as_ref() }
        && *asked > 0
        && tilings.is_null()
    {
        static FIRST: std::sync::Once = std::sync::Once::new();
        crate::diagnostics::log_once(&FIRST, || "GetResourceTiling missing tiling output".to_string());
        return;
    }
    unsafe {
        device.native.GetResourceTiling(
            resource,
            (!tiles.is_null()).then_some(tiles),
            (!packed.is_null()).then_some(packed),
            (!shape.is_null()).then_some(shape),
            (!count.is_null()).then_some(count),
            first,
            tilings,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::reserved_desc;
    use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
    use windows::Win32::Graphics::Direct3D12::*;
    use windows::Win32::Graphics::Dxgi::Common::*;

    fn texture(layout: D3D12_TEXTURE_LAYOUT) -> D3D12_RESOURCE_DESC {
        D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 256,
            Height: 256,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: layout,
            ..Default::default()
        }
    }

    #[test]
    fn accepts_a_64_kib_texture_and_a_row_major_buffer() {
        assert!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE)).is_ok());
        assert!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_64KB_STANDARD_SWIZZLE)).is_ok());
        let buffer = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
            Width: 65536,
            Height: 1,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_UNKNOWN,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
            Alignment: u64::from(D3D12_DEFAULT_RESOURCE_PLACEMENT_ALIGNMENT),
            ..Default::default()
        };
        assert!(reserved_desc(&buffer).is_ok());
    }

    #[test]
    fn refuses_console_layouts_and_ordinary_textures() {
        assert_eq!(reserved_desc(&texture(D3D12_TEXTURE_LAYOUT(0x101))).unwrap_err(), E_NOTIMPL);
        assert_eq!(
            reserved_desc(&texture(D3D12_TEXTURE_LAYOUT_UNKNOWN)).unwrap_err(),
            E_INVALIDARG
        );
        let mut wide = texture(D3D12_TEXTURE_LAYOUT_64KB_UNDEFINED_SWIZZLE);
        wide.Alignment = 4096;
        assert_eq!(reserved_desc(&wide).unwrap_err(), E_INVALIDARG);
        wide.Alignment = 0;
        wide.Width = 0;
        assert_eq!(reserved_desc(&wide).unwrap_err(), E_INVALIDARG);
    }
}
