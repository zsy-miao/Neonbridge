//! The DXGI interface is embedded in the guest device and shares its reference count.
use super::{Device, Out};
use crate::proxy;
use core::{ffi::c_void, mem::offset_of, ptr};
use std::sync::OnceLock;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, E_POINTER, S_OK};
use windows::Win32::Graphics::{
    Direct3D12::{
        D3D12_FEATURE_D3D12_OPTIONS1, D3D12_FEATURE_DATA_D3D12_OPTIONS1, ID3D12Device2,
        ID3D12Object,
    },
    Dxgi::*,
};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, Result};

#[repr(C)]
pub(super) struct DxgiDevice {
    vtable: &'static IDXGIDevice1_Vtbl,
    reserved: usize,
    // Xbox inline descriptor helpers read this byte before choosing slot 24.
    // A desktop CPU descriptor handle is opaque, so request the device API.
    use_descriptor_api: u8,
    pub(super) device11_queue: bool,
    adapter: OnceLock<IDXGIAdapter>,
    pub(crate) descriptor_arenas: std::sync::Arc<super::descriptor_paging::Registry>,
    pub(super) pipeline_device: Option<ID3D12Device2>,
    pub(super) render_heaps: super::heap::RenderHeaps,
    pub(super) page_pools: super::heap::PagePools,
    pub(super) imported_views: std::sync::Arc<super::placement::DescriptorMap<super::views::DescriptorBacking>>,
    pub(super) byte_offset_views: OnceLock<super::views::AddressViews>,
    pub(super) texture_addresses: std::sync::Arc<super::placement::Views<super::views::TextureAddress>>,
    pub(super) blas: super::blas::Buffers,
    pub(super) media: super::media::Sources,
    pub(super) placements: super::placement::Placements,
    pub(super) emulate_stencil: OnceLock<bool>,
    wave_range: OnceLock<Option<(u32, u32)>>,
    pub(super) depth_views: std::sync::Arc<super::placement::Views<super::stencil::Surface>>,
    pub(super) disposal: std::sync::Mutex<Option<(super::DisposeCallback, usize)>>,
    pub(super) frames: super::frame_events::FrameEvents,
}

const _: () =
    assert!(offset_of!(Device, state) + offset_of!(DxgiDevice, use_descriptor_api) == 0x18);

impl DxgiDevice {
    pub(super) fn new(
        selected: Option<&IUnknown>,
        pipeline_device: Option<ID3D12Device2>,
        device11_queue: bool,
    ) -> Result<Self> {
        let adapter = match selected {
            Some(selected) => OnceLock::from(selected.cast::<IDXGIAdapter>()?),
            None => OnceLock::new(),
        };
        Ok(Self {
            vtable: &TABLE,
            reserved: 0,
            use_descriptor_api: 1,
            device11_queue,
            adapter,
            descriptor_arenas: Default::default(),
            pipeline_device,
            render_heaps: Default::default(),
            page_pools: Default::default(),
            imported_views: Default::default(),
            texture_addresses: Default::default(),
            byte_offset_views: Default::default(),
            emulate_stencil: Default::default(),
            wave_range: Default::default(),
            depth_views: Default::default(),
            frames: Default::default(),
            disposal: Default::default(),
            blas: Default::default(),
            media: Default::default(),
            placements: Default::default(),
        })
    }
}

// SAFETY: All DXGI vtable calls receive a live pointer to the state member of
// a repr(C) Device. The state begins with its own vtable at byte offset zero.
unsafe fn device(this: *mut c_void) -> *mut Device {
    unsafe { this.byte_sub(offset_of!(Device, state)).cast() }
}

// Called by both interfaces. Only the DXGI family gets the secondary pointer.
pub(super) unsafe extern "system" fn query(
    this: *mut Device,
    iid: *const GUID,
    out: Out,
) -> HRESULT {
    if !out.is_null() && !iid.is_null() {
        let id = unsafe { iid.read_unaligned() };
        if [IDXGIDevice1::IID, IDXGIDevice::IID, IDXGIObject::IID].contains(&id) {
            // SAFETY: this is the live controlling device. One owned reference
            // keeps the embedded DXGI interface valid until its Release.
            unsafe {
                proxy::add_ref(this);
                out.write(ptr::addr_of_mut!((*this).state).cast());
            }
            return S_OK;
        }
    }
    unsafe { proxy::query(this, iid, out) }
}

unsafe extern "system" fn dxgi_query(this: *mut c_void, iid: *const GUID, out: Out) -> HRESULT {
    unsafe { query(device(this), iid, out) }
}

unsafe extern "system" fn add_ref(this: *mut c_void) -> u32 {
    unsafe { proxy::add_ref(device(this)) }
}

unsafe extern "system" fn release(this: *mut c_void) -> u32 {
    unsafe { super::release_device(device(this)) }
}

fn adapter(device: &Device) -> Result<&IDXGIAdapter> {
    if let Some(adapter) = device.state.adapter.get() {
        return Ok(adapter);
    }
    // SAFETY: the guest owns a live native device. Its LUID identifies the
    // adapter D3D12 selected when the caller passed a null adapter.
    let native = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory1()?;
        factory.EnumAdapterByLuid(device.native.GetAdapterLuid())?
    };
    Ok(device.state.adapter.get_or_init(|| native))
}

/// Supported hardware wave lane range `[WaveLaneCountMin, WaveLaneCountMax]`,
/// queried once per host device via `D3D12_FEATURE_D3D12_OPTIONS1`.
pub(super) fn wave_range(device: &Device) -> Option<(u32, u32)> {
    *device.state.wave_range.get_or_init(|| {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS1::default();
        let ok = unsafe {
            device.native.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS1,
                (&raw mut options).cast(),
                size_of_val(&options) as u32,
            )
        }
        .is_ok();
        (ok && options.WaveOps.as_bool()
            && options.WaveLaneCountMin != 0
            && options.WaveLaneCountMax >= options.WaveLaneCountMin)
            .then_some((options.WaveLaneCountMin, options.WaveLaneCountMax))
    })
}

unsafe extern "system" fn get_parent(this: *mut c_void, iid: *const GUID, out: Out) -> HRESULT {
    if out.is_null() {
        return E_POINTER;
    }
    // SAFETY: the caller supplies a writable interface pointer.
    unsafe { out.write(ptr::null_mut()) };
    if iid.is_null() {
        return E_POINTER;
    }
    match adapter(unsafe { &*device(this) }) {
        Ok(parent) => unsafe { parent.query(iid, out) },
        Err(err) => err.code(),
    }
}

unsafe extern "system" fn get_adapter(this: *mut c_void, out: Out) -> HRESULT {
    if out.is_null() {
        return E_INVALIDARG;
    }
    unsafe { get_parent(this, &IDXGIAdapter::IID, out) }
}

unsafe extern "system" fn set_private(
    this: *mut c_void,
    key: *const GUID,
    size: u32,
    data: *const c_void,
) -> HRESULT {
    let obj: &ID3D12Object = unsafe { &(*device(this)).native }.into();
    unsafe { (obj.vtable().SetPrivateData)(obj.as_raw(), key, size, data) }
}

unsafe extern "system" fn set_interface(
    this: *mut c_void,
    key: *const GUID,
    value: *mut c_void,
) -> HRESULT {
    let obj: &ID3D12Object = unsafe { &(*device(this)).native }.into();
    unsafe { (obj.vtable().SetPrivateDataInterface)(obj.as_raw(), key, value) }
}

unsafe extern "system" fn get_private(
    this: *mut c_void,
    key: *const GUID,
    size: *mut u32,
    data: *mut c_void,
) -> HRESULT {
    let obj: &ID3D12Object = unsafe { &(*device(this)).native }.into();
    unsafe { (obj.vtable().GetPrivateData)(obj.as_raw(), key, size, data) }
}

unsafe extern "system" fn create_surface(
    _: *mut c_void,
    _: *const DXGI_SURFACE_DESC,
    _: u32,
    _: DXGI_USAGE,
    _: *const DXGI_SHARED_RESOURCE,
    _: Out,
) -> HRESULT {
    E_NOTIMPL
}
unsafe extern "system" fn query_residency(
    _: *mut c_void,
    _: *const *mut c_void,
    _: *mut DXGI_RESIDENCY,
    _: u32,
) -> HRESULT {
    E_NOTIMPL
}
unsafe extern "system" fn set_priority(_: *mut c_void, _: i32) -> HRESULT {
    E_NOTIMPL
}
unsafe extern "system" fn get_priority(_: *mut c_void, _: *mut i32) -> HRESULT {
    E_NOTIMPL
}
unsafe extern "system" fn set_latency(_: *mut c_void, _: u32) -> HRESULT {
    E_NOTIMPL
}
unsafe extern "system" fn get_latency(_: *mut c_void, _: *mut u32) -> HRESULT {
    E_NOTIMPL
}

static TABLE: IDXGIDevice1_Vtbl = IDXGIDevice1_Vtbl {
    base__: IDXGIDevice_Vtbl {
        base__: IDXGIObject_Vtbl {
            base__: IUnknown_Vtbl {
                QueryInterface: dxgi_query,
                AddRef: add_ref,
                Release: release,
            },
            SetPrivateData: set_private,
            SetPrivateDataInterface: set_interface,
            GetPrivateData: get_private,
            GetParent: get_parent,
        },
        GetAdapter: get_adapter,
        CreateSurface: create_surface,
        QueryResourceResidency: query_residency,
        SetGPUThreadPriority: set_priority,
        GetGPUThreadPriority: get_priority,
    },
    SetMaximumFrameLatency: set_latency,
    GetMaximumFrameLatency: get_latency,
};
