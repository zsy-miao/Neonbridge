//! Two presented planes as one image: the console's display puts the second
//! (the interface) over the first (the scene), each scaled to the screen.
//! Grand Theft Auto VI presents so on Series S.

use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s};

use crate::host::Host;

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=2), visibility=SHADER_VISIBILITY_PIXEL), StaticSampler(s0, filter=FILTER_MIN_MAG_MIP_LINEAR, addressU=TEXTURE_ADDRESS_CLAMP, addressV=TEXTURE_ADDRESS_CLAMP)"
Texture2D<float4> scene : register(t0);
Texture2D<float4> over : register(t1);
SamplerState smooth_ : register(s0);
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; };
[RootSignature(RS)]
V vs(uint id : SV_VertexID) {
    V v;
    v.uv = float2((id << 1) & 2, id & 2);
    v.at = float4(v.uv * float2(2, -2) + float2(-1, 1), 0, 1);
    return v;
}
[RootSignature(RS)]
float4 ps(V v) : SV_Target {
    float4 s = scene.SampleLevel(smooth_, v.uv, 0);
    float4 o = over.SampleLevel(smooth_, v.uv, 0);
    return float4(saturate(o.rgb + s.rgb * (1 - saturate(o.a))), 1);
}
"#;

/// Descriptors of as many frames as are ever in flight.
const FRAMES: u32 = 16;

struct Composer {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}
// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Composer {}
// SAFETY: as above.
unsafe impl Sync for Composer {}

fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; the out pointer is
    // ours.
    unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("compose"),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut code,
            None,
        )?;
    }
    code.ok_or_else(windows::core::Error::empty)
}

fn make(host: &Host, format: DXGI_FORMAT) -> windows::core::Result<Composer> {
    let (vs, ps) = (compile(s!("vs"), s!("vs_5_1"))?, compile(s!("ps"), s!("ps_5_1"))?);
    // SAFETY: the blobs' own bytes, alive through the calls.
    unsafe {
        let bytes = |b: &ID3DBlob| {
            std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize())
        };
        let device = &host.device;
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes(&vs))?;
        let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE {
            pShaderBytecode: b.GetBufferPointer(),
            BytecodeLength: b.GetBufferSize(),
        };
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            VS: code(&vs),
            PS: code(&ps),
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                ..Default::default()
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        desc.RTVFormats[0] = format;
        desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
        let pso = device.CreateGraphicsPipelineState(&desc)?;
        let views = device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: 2 * FRAMES,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?;
        Ok(Composer {
            root,
            pso,
            views,
            step: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            next: AtomicU32::new(0),
        })
    }
}

/// Records the planes (either may be missing), in the shader resource state,
/// into `back`, a render target of `format`.
pub(crate) fn record(
    host: &Host,
    list: &ID3D12GraphicsCommandList,
    planes: [Option<(&ID3D12Resource, DXGI_FORMAT)>; 2],
    back: &ID3D12Resource,
    (width, height, format): (u32, u32, DXGI_FORMAT),
) -> bool {
    static COMPOSER: std::sync::Mutex<Option<(DXGI_FORMAT, Option<Composer>)>> =
        std::sync::Mutex::new(None);
    let mut composer = COMPOSER.lock().unwrap_or_else(|e| e.into_inner());
    if composer.as_ref().is_none_or(|c| c.0 != format) {
        let made = make(host, format);
        if let Err(e) = &made {
            tracing::warn!(error = %e, "presented planes not put together");
        }
        *composer = Some((format, made.ok()));
    }
    let Some((_, Some(c))) = &*composer else {
        return false;
    };
    let first = 2 * (c.next.fetch_add(1, Ordering::Relaxed) % FRAMES);
    // SAFETY: an open list; the views are made in slots no frame in flight
    // reads; live textures.
    unsafe {
        let (cpu, gpu) = (
            c.views.GetCPUDescriptorHandleForHeapStart(),
            c.views.GetGPUDescriptorHandleForHeapStart(),
        );
        for (i, plane) in planes.iter().enumerate() {
            let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + ((first as usize + i) * c.step as usize),
            };
            let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: plane.map_or(DXGI_FORMAT_R8G8B8A8_UNORM, |p| p.1),
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            host.device
                .CreateShaderResourceView(plane.map(|p| p.0), Some(&desc), at);
        }
        let rtv = host.rtv(back, None);
        list.OMSetRenderTargets(1, Some(&rtv), false, None);
        list.SetDescriptorHeaps(&[Some(c.views.clone())]);
        list.SetGraphicsRootSignature(&c.root);
        list.SetPipelineState(&c.pso);
        list.SetGraphicsRootDescriptorTable(
            0,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + u64::from(first) * u64::from(c.step),
            },
        );
        list.IASetPrimitiveTopology(
            windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
        );
        list.RSSetViewports(&[D3D12_VIEWPORT {
            Width: width as f32,
            Height: height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        }]);
        list.RSSetScissorRects(&[windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        }]);
        list.DrawInstanced(3, 1, 0, 0);
    }
    true
}
