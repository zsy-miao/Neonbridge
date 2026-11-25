//! Writes FidelityFX's FP16 output to the title's RGB9E5 texture on hosts
//! that can sample RGB9E5 but cannot write it through a typed UAV.

use std::sync::OnceLock;

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{Error, Interface, Result, s};

use crate::host::{HostList, host, transition};
use crate::object::This;

use super::{FFX_UNORDERED_ACCESS, Resource};

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0)), UAV(u0), RootConstants(num32BitConstants=3, b0)"
Texture2D<float4> color : register(t0);
RWByteAddressBuffer packed : register(u0);
cbuffer Extent : register(b0) { uint width; uint height; uint pitch; }

// RGB9E5 stores three unsigned 9-bit mantissas and one shared 5-bit exponent.
uint pack_rgb9e5(float3 rgb) {
    rgb = clamp(rgb, 0.0, 65408.0);
    float largest = max(rgb.r, max(rgb.g, rgb.b));
    uint exponent = ((asuint(max(largest, exp2(-16.0))) >> 23) & 255u) - 111u;
    float scale = exp2(24.0 - float(exponent));
    if (floor(largest * scale + 0.5) >= 512.0) {
        exponent++;
        scale *= 0.5;
    }
    uint3 mantissa = uint3(floor(rgb * scale + 0.5));
    return mantissa.r | (mantissa.g << 9) | (mantissa.b << 18) | (exponent << 27);
}

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= width || id.y >= height) return;
    packed.Store(id.y * pitch + id.x * 4, pack_rgb9e5(color.Load(int3(id.xy, 0)).rgb));
}
"#;

struct Packer {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}

// SAFETY: the immutable D3D12 root signature and pipeline are free-threaded.
unsafe impl Send for Packer {}
// SAFETY: as above.
unsafe impl Sync for Packer {}

fn packer() -> Option<&'static Packer> {
    static PACKER: OnceLock<Option<Packer>> = OnceLock::new();
    PACKER
        .get_or_init(|| match make_packer() {
            Ok(packer) => Some(packer),
            Err(error) => {
                tracing::warn!(%error, "ffx: could not create RGB9E5 pack pipeline");
                None
            }
        })
        .as_ref()
}

fn make_packer() -> Result<Packer> {
    let device = &host().ok_or_else(Error::empty)?.device;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live for the call and the out pointers are ours.
    let result = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("ffx-rgb9e5"),
            None,
            None,
            s!("main"),
            s!("cs_5_1"),
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let Err(error) = result {
        if let Some(errors) = errors {
            // SAFETY: the compiler blob owns its bytes.
            let text = unsafe {
                std::slice::from_raw_parts(errors.GetBufferPointer().cast(), errors.GetBufferSize())
            };
            tracing::warn!(message = %String::from_utf8_lossy(text), "ffx: RGB9E5 shader compilation failed");
        }
        return Err(error);
    }
    let code = code.ok_or_else(Error::empty)?;
    // SAFETY: the compiler blob contains the shader and its root signature.
    unsafe {
        let bytes =
            std::slice::from_raw_parts(code.GetBufferPointer().cast(), code.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes)?;
        let pipeline = device.CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        })?;
        Ok(Packer { root, pipeline })
    }
}

/// One pair of scratch resources per title output. All lists execute on the
/// host's direct queue, and each use restores the scratch states, so lists
/// can share the pair even if they were recorded in a different order.
pub(crate) struct Output {
    float: ID3D12Resource,
    packed: ID3D12Resource,
    srv: ID3D12DescriptorHeap,
    footprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    packer: &'static Packer,
}

impl Output {
    pub(crate) fn new(destination: &ID3D12Resource) -> Option<Self> {
        match Self::allocate(destination, packer()?) {
            Ok(output) => Some(output),
            Err(error) => {
                static REPORTED: std::sync::Once = std::sync::Once::new();
                REPORTED
                    .call_once(|| tracing::warn!(%error, "ffx: RGB9E5 scratch allocation failed"));
                None
            }
        }
    }

    fn allocate(destination: &ID3D12Resource, packer: &'static Packer) -> Result<Self> {
        let device = &host().ok_or_else(Error::empty)?.device;
        // SAFETY: a live 2D texture. The footprint and size cover its sole subresource.
        unsafe {
            let desc = destination.GetDesc();
            let mut footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut size = 0;
            device.GetCopyableFootprints(
                &desc,
                0,
                1,
                0,
                Some(&mut footprint),
                None,
                None,
                Some(&mut size),
            );
            if size == u64::MAX || size > u64::from(u32::MAX) || footprint.Footprint.RowPitch == 0 {
                return Err(Error::from_hresult(
                    windows::Win32::Foundation::E_INVALIDARG,
                ));
            }
            let heap = D3D12_HEAP_PROPERTIES {
                Type: D3D12_HEAP_TYPE_DEFAULT,
                ..Default::default()
            };
            let mut float: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                    ..desc
                },
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                None,
                &mut float,
            )?;
            let float = float.ok_or_else(Error::empty)?;
            let mut packed: Option<ID3D12Resource> = None;
            device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: size,
                    Height: 1,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                    Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                    ..Default::default()
                },
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut packed,
            )?;
            let packed = packed.ok_or_else(Error::empty)?;
            let srv: ID3D12DescriptorHeap =
                device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                    Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                    NumDescriptors: 1,
                    Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                    ..Default::default()
                })?;
            device.CreateShaderResourceView(&float, None, srv.GetCPUDescriptorHandleForHeapStart());
            Ok(Self {
                float,
                packed,
                srv,
                footprint,
                packer,
            })
        }
    }

    pub(super) fn matches(&self, size: [u32; 2]) -> bool {
        let footprint = &self.footprint.Footprint;
        size == [footprint.Width, footprint.Height]
    }

    /// The FP16 texture the upscale writes (in unordered access), which
    /// [`Output::copy_to`] packs.
    pub(super) fn float(&self, list: &HostList) -> &ID3D12Resource {
        list.keep(&self.float);
        &self.float
    }

    pub(super) fn sdk_resource(&self, list: &HostList) -> Resource {
        // The SDK can record references even if dispatch returns an error.
        list.keep(&self.float);
        // SAFETY: a live texture. The field order is ffxApiGetResourceDX12's.
        let desc = unsafe { self.float.GetDesc() };
        const FFX_TEXTURE_2D: u32 = 2;
        const FFX_RGBA16_FLOAT: u32 = 4;
        const FFX_USAGE_UAV: u32 = 1 << 1;
        Resource {
            resource: self.float.as_raw(),
            description: [
                FFX_TEXTURE_2D,
                FFX_RGBA16_FLOAT,
                desc.Width as u32,
                desc.Height,
                desc.DepthOrArraySize.into(),
                desc.MipLevels.into(),
                0,
                FFX_USAGE_UAV,
            ],
            state: FFX_UNORDERED_ACCESS,
        }
    }

    pub(super) fn copy_to(&self, list: &HostList, title: This, destination: &ID3D12Resource) {
        crate::list::ensure_state(list, title, D3D12_RESOURCE_STATE_COPY_DEST.0 as u32);
        list.keep(&self.packed);
        list.keep(&self.srv);
        #[cfg(feature = "diagnostics")]
        list.note_op(|| format!("pack FSR output into RGB9E5 title resource {title:?}"));
        self.record_pack_and_copy(
            &list.list,
            destination,
            &self.float,
            &self.srv,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        );
    }

    /// Destination is already in COPY_DEST. Restore the input and scratch
    /// buffer states so the same resources can be recorded on other lists.
    fn record_pack_and_copy(
        &self,
        list: &ID3D12GraphicsCommandList,
        destination: &ID3D12Resource,
        input: &ID3D12Resource,
        srv: &ID3D12DescriptorHeap,
        input_state: D3D12_RESOURCE_STATES,
    ) {
        let f = self.footprint.Footprint;
        let constants = [f.Width, f.Height, f.RowPitch];
        let source = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the copy location borrows the live buffer without releasing it.
            pResource: unsafe { std::mem::transmute_copy(&self.packed) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: self.footprint,
            },
        };
        let target = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: the copy location borrows the live texture without releasing it.
            pResource: unsafe { std::mem::transmute_copy(destination) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        // SAFETY: an open list; the caller retains the resources and heap.
        unsafe {
            // These locations borrow the COM references for the duration of the copy.
            list.ResourceBarrier(&[
                transition(
                    input,
                    input_state,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                ),
                transition(
                    &self.packed,
                    D3D12_RESOURCE_STATE_COMMON,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                ),
            ]);
            list.SetDescriptorHeaps(&[Some(srv.clone())]);
            list.SetComputeRootSignature(&self.packer.root);
            list.SetPipelineState(&self.packer.pipeline);
            list.SetComputeRootDescriptorTable(0, srv.GetGPUDescriptorHandleForHeapStart());
            list.SetComputeRootUnorderedAccessView(1, self.packed.GetGPUVirtualAddress());
            list.SetComputeRoot32BitConstants(2, 3, constants.as_ptr().cast(), 0);
            list.Dispatch(f.Width.div_ceil(8), f.Height.div_ceil(8), 1);
            list.ResourceBarrier(&[
                transition(
                    input,
                    D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE,
                    input_state,
                ),
                transition(
                    &self.packed,
                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    D3D12_RESOURCE_STATE_COPY_SOURCE,
                ),
            ]);
            list.CopyTextureRegion(&target, 0, 0, 0, &source, None);
            list.ResourceBarrier(&[transition(
                &self.packed,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
                D3D12_RESOURCE_STATE_COMMON,
            )]);
        }
    }
}

#[cfg(all(test, feature = "diagnostics"))]
mod tests {
    use super::*;

    #[test]
    fn packs_hdr_pixels_into_a_sampleable_rgb9e5_texture() {
        let host = host().expect("D3D12 device for the RGB9E5 test");
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 2,
            Height: 2,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R9G9B9E5_SHAREDEXP,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let make_destination = || {
            let mut destination: Option<ID3D12Resource> = None;
            // SAFETY: a single-sample test texture and our own output pointer.
            unsafe {
                host.device.CreateCommittedResource(
                    &D3D12_HEAP_PROPERTIES {
                        Type: D3D12_HEAP_TYPE_DEFAULT,
                        ..Default::default()
                    },
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    None,
                    &mut destination,
                )
            }
            .unwrap();
            destination.unwrap()
        };
        let destination = make_destination();
        let sampled = make_destination();
        let output = Output::new(&destination).expect("RGB9E5 fallback resources and pipeline");
        let list = host.new_list().unwrap();
        let (upload, data) = host.upload_buffer(512).unwrap();
        // FP16 pixels (1, .5, .25), (4, 2, -1), (0, 0, 0), and (32, 1, .125).
        let pixels = [
            [0x3c00u16, 0x3800, 0x3400, 0, 0x4400, 0x4000, 0xbc00, 0],
            [0, 0, 0, 0, 0x5000, 0x3c00, 0x3000, 0],
        ];
        let from = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: upload stays live through execute.
            pResource: unsafe { std::mem::transmute_copy(&upload) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                    Offset: 0,
                    Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                        Format: DXGI_FORMAT_R16G16B16A16_FLOAT,
                        Width: 2,
                        Height: 2,
                        Depth: 1,
                        RowPitch: 256,
                    },
                },
            },
        };
        let to = D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: output stays live through execute.
            pResource: unsafe { std::mem::transmute_copy(&output.float) },
            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                SubresourceIndex: 0,
            },
        };
        // SAFETY: the mapped upload holds both padded rows; all resources
        // live through submission and the transitions match their states.
        let ((readback, footprint), _srv) = unsafe {
            for (y, row) in pixels.iter().enumerate() {
                data.add(y * 256).cast::<[u16; 8]>().write_unaligned(*row);
            }
            list.list.ResourceBarrier(&[transition(
                &output.float,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                D3D12_RESOURCE_STATE_COPY_DEST,
            )]);
            list.list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
            list.list.ResourceBarrier(&[transition(
                &output.float,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )]);
            output.record_pack_and_copy(
                &list.list,
                &destination,
                &output.float,
                &output.srv,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            );
            // Sample the RGB9E5 output in a later compute pass and repack it
            // into a second texture. Keep separate descriptors for both passes.
            let srv: ID3D12DescriptorHeap = host
                .device
                .CreateDescriptorHeap(&output.srv.GetDesc())
                .unwrap();
            host.device.CreateShaderResourceView(
                &destination,
                None,
                srv.GetCPUDescriptorHandleForHeapStart(),
            );
            output.record_pack_and_copy(
                &list.list,
                &sampled,
                &destination,
                &srv,
                D3D12_RESOURCE_STATE_COPY_DEST,
            );
            list.list.ResourceBarrier(&[transition(
                &sampled,
                D3D12_RESOURCE_STATE_COPY_DEST,
                D3D12_RESOURCE_STATE_COPY_SOURCE,
            )]);
            let readback = crate::capture::read_back(&host.device, &list.list, &sampled).unwrap();
            (readback, srv)
        };
        list.close();
        host.execute(&[&list]);
        let mut data = std::ptr::null_mut();
        let decode = |bits: u32| {
            let scale = 2f32.powi((bits >> 27) as i32 - 24);
            [0, 9, 18].map(|shift| ((bits >> shift) & 0x1ff) as f32 * scale)
        };
        // SAFETY: execute completed, and the footprint describes the mapped rows.
        unsafe {
            readback.Map(0, None, Some(&mut data)).unwrap();
            let pixel = |x: usize, y: usize| {
                let offset =
                    footprint.Offset as usize + y * footprint.Footprint.RowPitch as usize + x * 4;
                decode(data.cast::<u8>().add(offset).cast::<u32>().read_unaligned())
            };
            assert_eq!(pixel(0, 0), [1.0, 0.5, 0.25]);
            assert_eq!(pixel(1, 0), [4.0, 2.0, 0.0]);
            assert_eq!(pixel(0, 1), [0.0; 3]);
            assert_eq!(pixel(1, 1), [32.0, 1.0, 0.125]);
            readback.Unmap(0, None);
        }
        assert_eq!(host.log_messages(), None);
    }
}
