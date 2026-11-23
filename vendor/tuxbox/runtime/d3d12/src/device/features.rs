//! Slot 13 record layouts from the packaged Scarlett driver, xgs12_10 RVA 0x512f0.
use super::Device;
use core::ffi::c_void;
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL, S_OK};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_FORMAT_B4G4R4A4_UNORM};
use windows::core::{HRESULT, Interface};

fn host_query<T>(device: &Device, feature: D3D12_FEATURE, mut record: T) -> Result<T, HRESULT> {
    let host = &device.native;
    // Each caller uses the desktop record for this feature, including its exact size.
    let hr = unsafe {
        (host.vtable().CheckFeatureSupport)(
            host.as_raw(),
            feature,
            (&raw mut record).cast(),
            size_of::<T>() as u32,
        )
    };
    hr.ok().map(|()| record).map_err(|e| e.code())
}

fn options(device: &Device) -> Result<[u32; 16], HRESULT> {
    let host = host_query(
        device,
        D3D12_FEATURE_D3D12_OPTIONS,
        D3D12_FEATURE_DATA_D3D12_OPTIONS::default(),
    )?;
    // The Xbox driver writes the address width at +36 and heap tier at +60.
    // Keep the unverified flags between them disabled. Tiled resource mappings
    // and ROVs are also unavailable through this guest device.
    Ok([
        u32::from(host.DoublePrecisionFloatShaderOps.as_bool()),
        u32::from(host.OutputMergerLogicOp.as_bool()),
        0,
        0,
        host.ResourceBindingTier.0.clamp(0, 3) as u32,
        u32::from(host.PSSpecifiedStencilRefSupported.as_bool()),
        u32::from(host.TypedUAVLoadAdditionalFormats.as_bool()),
        0,
        host.ConservativeRasterizationTier.0.clamp(0, 2) as u32,
        host.MaxGPUVirtualAddressBitsPerResource.min(40),
        0,
        0,
        0,
        0,
        0,
        host.ResourceHeapTier.0.clamp(0, 2) as u32,
    ])
}

// These DXGI format numbers have the same meaning on desktop and Xbox.
fn shared_format(format: DXGI_FORMAT) -> bool {
    (1..=DXGI_FORMAT_B4G4R4A4_UNORM.0).contains(&format.0)
}

pub(super) unsafe extern "system" fn query(
    device: &Device,
    feature: D3D12_FEATURE,
    data: *mut c_void,
    size: u32,
) -> HRESULT {
    let minimum = match feature {
        D3D12_FEATURE_D3D12_OPTIONS => 64,
        D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS => 16,
        D3D12_FEATURE_FORMAT_SUPPORT | D3D12_FEATURE_D3D12_OPTIONS5 => 12,
        D3D12_FEATURE_FORMAT_INFO
        | D3D12_FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT
        | D3D12_FEATURE_D3D12_OPTIONS7 => 8,
        D3D12_FEATURE_ROOT_SIGNATURE => 4,
        _ => return E_NOTIMPL,
    };
    if data.is_null() || size < minimum {
        return E_INVALIDARG;
    }
    // The guest owns at least the known prefix. Read only the inputs and
    // commit only successful outputs, leaving padding and extensions intact.
    unsafe { translate(device, feature, data.cast()) }.map_or_else(|error| error, |()| S_OK)
}

unsafe fn translate(
    device: &Device,
    feature: D3D12_FEATURE,
    words: *mut u32,
) -> Result<(), HRESULT> {
    match feature {
        D3D12_FEATURE_D3D12_OPTIONS => unsafe {
            words.cast::<[u32; 16]>().write_unaligned(options(device)?)
        },
        D3D12_FEATURE_D3D12_OPTIONS5 => {
            // TuxBox has no guest tiled SRV, render-pass, or DXR command interface.
            unsafe { words.cast::<[u32; 3]>().write_unaligned([0; 3]) };
        }
        D3D12_FEATURE_D3D12_OPTIONS7 => {
            // At RVA 0x51844 the Scarlett UMD writes mesh tier then sampler
            // feedback tier. Feedback maps require additional device methods.
            let mesh = match host_query(
                device,
                feature,
                D3D12_FEATURE_DATA_D3D12_OPTIONS7::default(),
            ) {
                Ok(host) if host.MeshShaderTier.0 >= D3D12_MESH_SHADER_TIER_1.0 => {
                    D3D12_MESH_SHADER_TIER_1
                }
                // Older desktop runtimes may not recognize this feature query.
                Ok(_) | Err(E_INVALIDARG | E_NOTIMPL) => D3D12_MESH_SHADER_TIER_NOT_SUPPORTED,
                Err(error) => return Err(error),
            };
            unsafe { words.cast::<[u32; 2]>().write_unaligned([mesh.0 as u32, 0]) };
        }
        D3D12_FEATURE_ROOT_SIGNATURE => {
            // The packaged driver always reports 1.0, including for a 1.1 request.
            let record = D3D12_FEATURE_DATA_ROOT_SIGNATURE {
                HighestVersion: D3D_ROOT_SIGNATURE_VERSION_1_0,
            };
            let result = host_query(device, feature, record)?;
            unsafe { words.write_unaligned(result.HighestVersion.0 as u32) };
        }
        D3D12_FEATURE_GPU_VIRTUAL_ADDRESS_SUPPORT => {
            let result = host_query(
                device,
                feature,
                D3D12_FEATURE_DATA_GPU_VIRTUAL_ADDRESS_SUPPORT::default(),
            )?;
            let bits = [
                result.MaxGPUVirtualAddressBitsPerResource.min(40),
                result.MaxGPUVirtualAddressBitsPerProcess.min(40),
            ];
            unsafe { words.cast::<[u32; 2]>().write_unaligned(bits) };
        }
        D3D12_FEATURE_FORMAT_SUPPORT => {
            let format = unsafe { words.cast::<DXGI_FORMAT>().read_unaligned() };
            // Console-only format numbers have no native mapping here. Zero
            // support lets a title keep enumerating the remaining formats.
            let flags = if shared_format(format) {
                let record = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
                    Format: format,
                    ..Default::default()
                };
                let result = host_query(device, feature, record)?;
                let unavailable =
                    D3D12_FORMAT_SUPPORT2_TILED | D3D12_FORMAT_SUPPORT2_SAMPLER_FEEDBACK;
                [
                    result.Support1.0 as u32,
                    (result.Support2.0 & !unavailable.0) as u32,
                ]
            } else if (0..=191).contains(&format.0) {
                [0; 2]
            } else {
                return Err(E_INVALIDARG);
            };
            unsafe { words.add(1).cast::<[u32; 2]>().write_unaligned(flags) };
        }
        D3D12_FEATURE_FORMAT_INFO => {
            let format = unsafe { words.cast::<DXGI_FORMAT>().read_unaligned() };
            if !shared_format(format) {
                return Err(E_INVALIDARG);
            }
            let record = D3D12_FEATURE_DATA_FORMAT_INFO {
                Format: format,
                ..Default::default()
            };
            let result = host_query(device, feature, record)?;
            unsafe { words.add(1).cast::<u8>().write(result.PlaneCount) };
        }
        D3D12_FEATURE_MULTISAMPLE_QUALITY_LEVELS => {
            let [format, count, flags] = unsafe { words.cast::<[u32; 3]>().read_unaligned() };
            let format = DXGI_FORMAT(format as i32);
            if !shared_format(format) {
                return Err(E_INVALIDARG);
            }
            let record = D3D12_FEATURE_DATA_MULTISAMPLE_QUALITY_LEVELS {
                Format: format,
                SampleCount: count,
                Flags: D3D12_MULTISAMPLE_QUALITY_LEVEL_FLAGS(flags as i32),
                NumQualityLevels: 0,
            };
            let result = host_query(device, feature, record)?;
            // The packaged driver offers one quality level for supported counts
            // through 8x, subject to its tiled-resource flag. Tile mapping is absent here.
            let levels = u32::from(
                matches!(count, 1 | 2 | 4 | 8) && flags == 0 && result.NumQualityLevels != 0,
            );
            unsafe { words.add(3).write_unaligned(levels) };
        }
        _ => return Err(E_NOTIMPL),
    }
    Ok(())
}
