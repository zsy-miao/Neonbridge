//! `VICTORY_FFX_SIMPLE=1`: the title's FidelityFX upscale answered without
//! an upscaler. Each dispatch scales the colour input's render rectangle to
//! the output bilinearly, with no history, no jitter and no sharpening: to
//! tell what the upscaler adds to a picture from what the title draws.

use std::sync::OnceLock;

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{Error, Result, s};

use crate::host::{HostList, host};

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0), UAV(u0)), RootConstants(num32BitConstants=6, b0), " \
    "StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, " \
    "addressV=TEXTURE_ADDRESS_CLAMP, addressW=TEXTURE_ADDRESS_CLAMP)"
Texture2D<float4> color : register(t0);
RWTexture2D<float4> output : register(u0);
SamplerState linear_clamp : register(s0);
cbuffer Sizes : register(b0) { uint2 render; uint2 input; uint2 upscale; }

[RootSignature(RS)]
[numthreads(8, 8, 1)]
void main(uint3 id : SV_DispatchThreadID) {
    if (id.x >= upscale.x || id.y >= upscale.y) return;
    // The output pixel's centre in render pixels, kept half a pixel inside
    // the render rectangle: what lies past it in the input is not this frame.
    float2 at = (float2(id.xy) + 0.5) * float2(render) / float2(upscale);
    at = clamp(at, 0.5, float2(render) - 0.5);
    output[id.xy] = color.SampleLevel(linear_clamp, at / float2(input), 0);
}
"#;

struct Scaler {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
}

// SAFETY: the immutable D3D12 root signature and pipeline are free-threaded.
unsafe impl Send for Scaler {}
// SAFETY: as above.
unsafe impl Sync for Scaler {}

fn scaler() -> Option<&'static Scaler> {
    static SCALER: OnceLock<Option<Scaler>> = OnceLock::new();
    SCALER
        .get_or_init(|| match make() {
            Ok(s) => Some(s),
            Err(error) => {
                tracing::warn!(%error, "ffx: could not create the simple upscale pipeline");
                None
            }
        })
        .as_ref()
}

fn make() -> Result<Scaler> {
    let device = &host().ok_or_else(Error::empty)?.device;
    let (mut code, mut errors): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
    // SAFETY: the source and names live for the call and the out pointers are ours.
    let result = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("ffx-simple"),
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
            tracing::warn!(message = %String::from_utf8_lossy(text), "ffx: simple upscale shader compilation failed");
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
        Ok(Scaler { root, pipeline })
    }
}

/// Whether a texture's format takes typed stores through a view of its own
/// format (the output) or filtered reads (the input).
pub(super) fn supports(texture: &ID3D12Resource, store: bool) -> bool {
    let Some(host) = host() else { return false };
    // SAFETY: a live texture; a plain query into our structure.
    unsafe {
        let mut support = D3D12_FEATURE_DATA_FORMAT_SUPPORT {
            Format: texture.GetDesc().Format,
            ..Default::default()
        };
        if host
            .device
            .CheckFeatureSupport(
                D3D12_FEATURE_FORMAT_SUPPORT,
                (&raw mut support).cast(),
                size_of::<D3D12_FEATURE_DATA_FORMAT_SUPPORT>() as u32,
            )
            .is_err()
        {
            return false;
        }
        if store {
            support
                .Support1
                .contains(D3D12_FORMAT_SUPPORT1_TYPED_UNORDERED_ACCESS_VIEW)
                && support
                    .Support2
                    .contains(D3D12_FORMAT_SUPPORT2_UAV_TYPED_STORE)
        } else {
            support
                .Support1
                .contains(D3D12_FORMAT_SUPPORT1_SHADER_SAMPLE)
        }
    }
}

/// Records the scale of `input`'s top left `render` pixels to `output`'s
/// top left `upscale` pixels. The input is in a shader-resource state, the
/// output in unordered access, and both stay there. False if the pipeline
/// or the views could not be made.
pub(super) fn record(
    list: &HostList,
    input: &ID3D12Resource,
    output: &ID3D12Resource,
    render: [u32; 2],
    upscale: [u32; 2],
) -> bool {
    let (Some(scaler), Some(host)) = (scaler(), host()) else {
        return false;
    };
    // ponytail: a two-view heap per dispatch (one a frame), kept by the
    // list; cache it by texture pair if it ever shows in a profile.
    // SAFETY: plain object creation; views of live textures in their own
    // formats (checked by `supports`).
    let heap: ID3D12DescriptorHeap = match unsafe {
        host.device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: 2,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                ..Default::default()
            })
    } {
        Ok(heap) => heap,
        Err(_) => return false,
    };
    // SAFETY: as above.
    let input_size = unsafe {
        let step = host
            .device
            .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        let at = heap.GetCPUDescriptorHandleForHeapStart();
        host.device.CreateShaderResourceView(input, None, at);
        host.device.CreateUnorderedAccessView(
            output,
            None,
            None,
            D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: at.ptr + step as usize,
            },
        );
        let d = input.GetDesc();
        [d.Width as u32, d.Height]
    };
    list.keep(&heap);
    let constants = [
        render[0],
        render[1],
        input_size[0],
        input_size[1],
        upscale[0],
        upscale[1],
    ];
    #[cfg(feature = "diagnostics")]
    list.note_op(|| format!("simple upscale {render:?} to {upscale:?}"));
    // SAFETY: an open list; our pipeline and heap; live textures in the
    // states the views need.
    unsafe {
        let l = &list.list;
        l.SetDescriptorHeaps(&[Some(heap.clone())]);
        l.SetComputeRootSignature(&scaler.root);
        l.SetPipelineState(&scaler.pipeline);
        l.SetComputeRootDescriptorTable(0, heap.GetGPUDescriptorHandleForHeapStart());
        l.SetComputeRoot32BitConstants(1, 6, constants.as_ptr().cast(), 0);
        l.Dispatch(upscale[0].div_ceil(8), upscale[1].div_ceil(8), 1);
        // The output's writes before whatever reads it next.
        l.ResourceBarrier(&[D3D12_RESOURCE_BARRIER {
            Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
            Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
            Anonymous: D3D12_RESOURCE_BARRIER_0 {
                UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                    pResource: std::mem::transmute_copy(output),
                }),
            },
        }]);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader and its root signature compile (no GPU needed).
    #[test]
    fn simple_upscale_shader_compiles() {
        let (mut code, mut errors): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
        // SAFETY: as in `make`.
        let result = unsafe {
            D3DCompile(
                SOURCE.as_ptr().cast(),
                SOURCE.len(),
                s!("ffx-simple"),
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
        let text = errors.map(|e| {
            // SAFETY: the compiler blob owns its bytes.
            String::from_utf8_lossy(unsafe {
                std::slice::from_raw_parts(e.GetBufferPointer().cast::<u8>(), e.GetBufferSize())
            })
            .into_owned()
        });
        assert!(result.is_ok() && code.is_some(), "{text:?}");
    }
}
