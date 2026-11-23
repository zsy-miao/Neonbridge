//! Byte-offset buffer views from the DirectX Agility SDK. The windows crate's
//! bindings currently stop at ID3D12Device14.
use super::Device;
use core::ffi::c_void;
use windows::Win32::Foundation::E_INVALIDARG;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, Result};

pub(in crate::device) enum AddressViews {
    Elements,
    ByteOffsets(ByteOffsetDevice),
    VulkanDescriptorSets,
}

#[repr(transparent)]
#[derive(Clone)]
struct VulkanInterop(IUnknown);

// The vkd3d-proton device's DXVK interoperability interface identifies its
// descriptor implementation. We need only QueryInterface/Release.
unsafe impl Interface for VulkanInterop {
    type Vtable = IUnknown_Vtbl;
    const IID: GUID = GUID::from_u128(0x39da4e09_bd1c_4198_9fae_86bbe3be41fd);
}

impl AddressViews {
    pub(super) fn get(device: &Device) -> &Self {
        device.state.byte_offset_views.get_or_init(|| {
            let mut options = [0u32; 4]; // D3D12_FEATURE_DATA_D3D12_OPTIONS22
            if unsafe {
                device.native.CheckFeatureSupport(
                    D3D12_FEATURE(65),
                    options.as_mut_ptr().cast(),
                    size_of_val(&options) as u32,
                )
            }
            .is_ok()
                && options[1] != 0
                && let Ok(host) = device.native.cast()
            {
                return Self::ByteOffsets(host);
            }
            if device.native.cast::<VulkanInterop>().is_ok() {
                Self::VulkanDescriptorSets
            } else {
                Self::Elements
            }
        })
    }

    pub(super) fn supports(
        &self,
        format: i32,
        flags: i32,
        structure_stride: u32,
        count: u32,
    ) -> bool {
        count != 0
            && match self {
                Self::ByteOffsets(_) => true,
                Self::Elements => false,
                Self::VulkanDescriptorSets => {
                    format == 0
                        && flags == 0
                        && structure_stride >= 4
                        && structure_stride.is_multiple_of(4)
                }
            }
    }

    pub(super) fn create_srv(
        &self,
        device: &Device,
        resource: &ID3D12Resource,
        mut view: D3D12_SHADER_RESOURCE_VIEW_DESC,
        prefix: u64,
        stride: u32,
        handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    ) -> Result<()> {
        let b = unsafe { view.Anonymous.Buffer };
        let bytes = ByteRange::new(prefix, stride, b.FirstElement, b.NumElements, b.Flags.0)?;
        if let Self::ByteOffsets(host) = self {
            let native = BufferSrvByteOffset {
                offset: bytes.offset,
                size: bytes.size,
                structure_byte_stride: b.StructureByteStride,
                flags: b.Flags,
            };
            view.ViewDimension = D3D12_SRV_DIMENSION(12); // D3D12_SRV_DIMENSION_BUFFER_BYTE_OFFSET
            // BufferByteOffset fits the SDK's existing SRV union.
            unsafe {
                (&raw mut view.Anonymous)
                    .cast::<BufferSrvByteOffset>()
                    .write(native);
                return (host.vtable().try_create_srv)(
                    host.as_raw(),
                    resource.as_raw(),
                    &view,
                    handle,
                )
                .ok();
            }
        }
        let b = unsafe { &mut view.Anonymous.Buffer };
        (b.FirstElement, b.NumElements, b.StructureByteStride) = bytes.storage_elements(stride)?;
        unsafe {
            device
                .native
                .CreateShaderResourceView(resource, Some(&view), handle)
        };
        Ok(())
    }

    pub(super) fn create_uav(
        &self,
        device: &Device,
        resource: &ID3D12Resource,
        counter: Option<&ID3D12Resource>,
        mut view: D3D12_UNORDERED_ACCESS_VIEW_DESC,
        prefix: u64,
        stride: u32,
        handle: D3D12_CPU_DESCRIPTOR_HANDLE,
    ) -> Result<()> {
        let b = unsafe { view.Anonymous.Buffer };
        let bytes = ByteRange::new(prefix, stride, b.FirstElement, b.NumElements, b.Flags.0)?;
        if let Self::ByteOffsets(host) = self {
            // The revised desktop UAV union is larger than the guest's UAV union.
            let native = BufferUavByteOffsetDesc {
                format: view.Format.0,
                dimension: 9,
                offset: bytes.offset,
                size: bytes.size,
                structure_byte_stride: b.StructureByteStride,
                counter_offset: b.CounterOffsetInBytes,
                flags: b.Flags,
            };
            return unsafe {
                (host.vtable().try_create_uav)(
                    host.as_raw(),
                    resource.as_raw(),
                    counter.map_or(core::ptr::null_mut(), Interface::as_raw),
                    &native,
                    handle,
                )
                .ok()
            };
        }
        let b = unsafe { &mut view.Anonymous.Buffer };
        (b.FirstElement, b.NumElements, b.StructureByteStride) = bytes.storage_elements(stride)?;
        unsafe {
            device
                .native
                .CreateUnorderedAccessView(resource, counter, Some(&view), handle)
        };
        Ok(())
    }

    pub(super) fn name(&self) -> &'static str {
        match self {
            Self::Elements => "whole elements",
            Self::ByteOffsets(_) => "D3D12 byte offsets",
            Self::VulkanDescriptorSets => "Vulkan descriptor sets",
        }
    }
}

#[repr(transparent)]
#[derive(Clone)]
pub(in crate::device) struct ByteOffsetDevice(ID3D12Device14);

#[repr(C)]
pub(in crate::device) struct Vtable {
    base: ID3D12Device14_Vtbl,
    register_trim: usize,
    unregister_trim: usize,
    try_create_srv: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *const D3D12_SHADER_RESOURCE_VIEW_DESC,
        D3D12_CPU_DESCRIPTOR_HANDLE,
    ) -> HRESULT,
    try_create_uav: unsafe extern "system" fn(
        *mut c_void,
        *mut c_void,
        *mut c_void,
        *const BufferUavByteOffsetDesc,
        D3D12_CPU_DESCRIPTOR_HANDLE,
    ) -> HRESULT,
}

// SAFETY: this is the prefix of ID3D12Device15 through TryCreateUnorderedAccessView.
unsafe impl Interface for ByteOffsetDevice {
    type Vtable = Vtable;
    const IID: GUID = GUID::from_u128(0x76cff76f_1e9b_4450_8cdc_34f1af788e5b);
}

#[repr(C)]
struct BufferSrvByteOffset {
    offset: u64,
    size: u64,
    structure_byte_stride: u32,
    flags: D3D12_BUFFER_SRV_FLAGS,
}

const _: () = {
    assert!(size_of::<BufferSrvByteOffset>() == size_of::<D3D12_SHADER_RESOURCE_VIEW_DESC_0>());
    assert!(align_of::<BufferSrvByteOffset>() == align_of::<D3D12_SHADER_RESOURCE_VIEW_DESC_0>());
};

#[repr(C)]
struct BufferUavByteOffsetDesc {
    format: i32,
    dimension: i32,
    offset: u64,
    size: u64,
    structure_byte_stride: u32,
    counter_offset: u64,
    flags: D3D12_BUFFER_UAV_FLAGS,
}
const _: () = {
    assert!(core::mem::offset_of!(BufferUavByteOffsetDesc, counter_offset) == 32);
    assert!(size_of::<BufferUavByteOffsetDesc>() == 48);
};

struct ByteRange {
    offset: u64,
    size: u64,
}

impl ByteRange {
    fn new(prefix: u64, stride: u32, first: u64, count: u32, flags: i32) -> Result<Self> {
        let stride = u64::from(stride);
        let offset = first
            .checked_mul(stride)
            .and_then(|first| prefix.checked_add(first))
            .ok_or(E_INVALIDARG)?;
        let size = u64::from(count) * stride;
        // A zero Size means "to the resource end" in the byte-offset API.
        let alignment = if flags & 1 != 0 {
            16
        } else {
            (1 << stride.trailing_zeros()).min(16)
        };
        if size == 0 || !offset.is_multiple_of(alignment) {
            return Err(E_INVALIDARG.into());
        }
        Ok(Self { offset, size })
    }

    fn storage_elements(&self, stride: u32) -> Result<(u64, u32, u32)> {
        // vkd3d's legacy descriptor sets store only (offset, size) for a structured
        // SSBO. DXIL supplies the actual stride for load/store and GetDimensions.
        let (mut unit, mut divisor) = (u64::from(stride), self.offset);
        while divisor != 0 {
            (unit, divisor) = (divisor, unit % divisor);
        }
        if !unit.is_multiple_of(4) {
            return Err(E_INVALIDARG.into());
        }
        Ok((
            self.offset / unit,
            (self.size / unit).try_into().map_err(|_| E_INVALIDARG)?,
            unit as u32,
        ))
    }
}
