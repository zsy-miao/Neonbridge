//! The presenter's scaling pass: the title's back buffer drawn into the window's swap chain,
//! aspect-correct (letterboxed) with a Catmull-Rom filter (a box filter when shrinking).

use std::collections::HashMap;
use std::ffi::c_void;

use windows::core::PCSTR;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

const VS: &str = "struct V { float4 p : SV_Position; float2 uv : TEXCOORD0; };\n\
V main(uint id : SV_VertexID) { V o; float2 t = float2((id << 1) & 2, id & 2); o.uv = t; o.p = float4(t * float2(2, -2) + float2(-1, 1), 0, 1); return o; }\n";

const PS: &str = "cbuffer C : register(b0) { float2 src_size; float2 ratio; };\n\
Texture2D<float4> T : register(t0);\n\
SamplerState S : register(s0);\n\
struct V { float4 p : SV_Position; float2 uv : TEXCOORD0; };\n\
float4 main(V i) : SV_Target {\n\
  float2 pos = i.uv * src_size;\n\
  float2 inv = 1.0 / src_size;\n\
  if (ratio.x > 1.5 || ratio.y > 1.5) {\n\
    float2 o = 0.25 * ratio;\n\
    float4 a = T.SampleLevel(S, (pos + float2(-o.x, -o.y)) * inv, 0) + T.SampleLevel(S, (pos + float2(o.x, -o.y)) * inv, 0)\n\
             + T.SampleLevel(S, (pos + float2(-o.x, o.y)) * inv, 0) + T.SampleLevel(S, (pos + float2(o.x, o.y)) * inv, 0);\n\
    return float4(a.rgb * 0.25, 1);\n\
  }\n\
  float2 tc = floor(pos - 0.5) + 0.5;\n\
  float2 f = pos - tc;\n\
  float2 w0 = f * (-0.5 + f * (1.0 - 0.5 * f));\n\
  float2 w1 = 1.0 + f * f * (-2.5 + 1.5 * f);\n\
  float2 w2 = f * (0.5 + f * (2.0 - 1.5 * f));\n\
  float2 w3 = f * f * (-0.5 + 0.5 * f);\n\
  float2 w12 = w1 + w2;\n\
  float2 t12 = (tc + w2 / w12) * inv;\n\
  float2 t0 = (tc - 1.0) * inv;\n\
  float2 t3 = (tc + 2.0) * inv;\n\
  float3 c = T.SampleLevel(S, float2(t12.x, t0.y), 0).rgb * (w12.x * w0.y)\n\
           + T.SampleLevel(S, float2(t0.x, t12.y), 0).rgb * (w0.x * w12.y)\n\
           + T.SampleLevel(S, float2(t12.x, t12.y), 0).rgb * (w12.x * w12.y)\n\
           + T.SampleLevel(S, float2(t3.x, t12.y), 0).rgb * (w3.x * w12.y)\n\
           + T.SampleLevel(S, float2(t12.x, t3.y), 0).rgb * (w12.x * w3.y);\n\
  float sum = w12.x * w0.y + w0.x * w12.y + w12.x * w12.y + w3.x * w12.y + w12.x * w3.y;\n\
  return float4(max(c / sum, 0), 1);\n\
}\n";

pub struct Blit {
    root: ID3D12RootSignature,
    psos: HashMap<i32, ID3D12PipelineState>,
    srv_heap: ID3D12DescriptorHeap,
    rtv_heap: ID3D12DescriptorHeap,
    vs: Vec<u8>,
    ps: Vec<u8>,
}

unsafe fn compile(src: &str, target: &str) -> Option<Vec<u8>> {
    let mut blob = None;
    let t = format!("{target}\0");
    D3DCompile(src.as_ptr() as *const c_void, src.len(), PCSTR::null(), None, None, PCSTR(b"main\0".as_ptr()), PCSTR(t.as_ptr()), 0, 0, &mut blob, None).ok()?;
    let b = blob?;
    Some(std::slice::from_raw_parts(b.GetBufferPointer() as *const u8, b.GetBufferSize()).to_vec())
}

/// the sRGB twin of a swap chain format (a view of the same buffer), if it has one
pub fn srgb_view(f: DXGI_FORMAT) -> DXGI_FORMAT {
    match f {
        DXGI_FORMAT_R8G8B8A8_UNORM => DXGI_FORMAT_R8G8B8A8_UNORM_SRGB,
        DXGI_FORMAT_B8G8R8A8_UNORM => DXGI_FORMAT_B8G8R8A8_UNORM_SRGB,
        o => o,
    }
}

pub fn is_srgb(f: DXGI_FORMAT) -> bool {
    matches!(f, DXGI_FORMAT_R8G8B8A8_UNORM_SRGB | DXGI_FORMAT_B8G8R8A8_UNORM_SRGB)
}

impl Blit {
    pub unsafe fn new(dev: &ID3D12Device) -> Option<Blit> {
        let vs = compile(VS, "vs_5_0")?;
        let ps = compile(PS, "ps_5_0")?;
        let range = D3D12_DESCRIPTOR_RANGE {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: 0,
            RegisterSpace: 0,
            OffsetInDescriptorsFromTableStart: D3D12_DESCRIPTOR_RANGE_OFFSET_APPEND,
        };
        let params = [
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
                Anonymous: D3D12_ROOT_PARAMETER_0 { Constants: D3D12_ROOT_CONSTANTS { ShaderRegister: 0, RegisterSpace: 0, Num32BitValues: 4 } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
            D3D12_ROOT_PARAMETER {
                ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
                Anonymous: D3D12_ROOT_PARAMETER_0 { DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE { NumDescriptorRanges: 1, pDescriptorRanges: &range } },
                ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            },
        ];
        let sampler = D3D12_STATIC_SAMPLER_DESC {
            Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
            AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
            MaxLOD: f32::MAX,
            ShaderRegister: 0,
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
            ..Default::default()
        };
        let desc = D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: 2,
            pParameters: params.as_ptr(),
            NumStaticSamplers: 1,
            pStaticSamplers: &sampler,
            Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
        };
        let mut ser = None;
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut ser, None).ok()?;
        let ser = ser?;
        let root: ID3D12RootSignature = dev.CreateRootSignature(0, std::slice::from_raw_parts(ser.GetBufferPointer() as *const u8, ser.GetBufferSize())).ok()?;
        let srv_heap: ID3D12DescriptorHeap = dev
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, NumDescriptors: 1, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, NodeMask: 0 })
            .ok()?;
        let rtv_heap: ID3D12DescriptorHeap = dev
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC { Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV, NumDescriptors: 1, Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE, NodeMask: 0 })
            .ok()?;
        Some(Blit { root, psos: HashMap::new(), srv_heap, rtv_heap, vs, ps })
    }

    unsafe fn pso(&mut self, dev: &ID3D12Device, rtv_fmt: DXGI_FORMAT) -> Option<ID3D12PipelineState> {
        if let Some(p) = self.psos.get(&rtv_fmt.0) {
            return Some(p.clone());
        }
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::ManuallyDrop::new(Some(self.root.clone())),
            VS: D3D12_SHADER_BYTECODE { pShaderBytecode: self.vs.as_ptr() as *const c_void, BytecodeLength: self.vs.len() },
            PS: D3D12_SHADER_BYTECODE { pShaderBytecode: self.ps.as_ptr() as *const c_void, BytecodeLength: self.ps.len() },
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            ..Default::default()
        };
        desc.RTVFormats[0] = rtv_fmt;
        desc.RasterizerState = D3D12_RASTERIZER_DESC { FillMode: D3D12_FILL_MODE_SOLID, CullMode: D3D12_CULL_MODE_NONE, DepthClipEnable: true.into(), ..Default::default() };
        desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
        let r = dev.CreateGraphicsPipelineState::<ID3D12PipelineState>(&desc);
        std::mem::ManuallyDrop::drop(&mut desc.pRootSignature);
        let p = r.ok()?;
        self.psos.insert(rtv_fmt.0, p.clone());
        Some(p)
    }

    /// Records the draw of `src` (src_w x src_h, viewed as `src_fmt`) into `dst`
    /// (dst_w x dst_h, viewed as `rtv_fmt`); `dst` is in RENDER_TARGET state, black bars
    /// around the image. The caller owns the state transitions.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record(
        &mut self,
        dev: &ID3D12Device,
        list: &ID3D12GraphicsCommandList,
        src: &ID3D12Resource,
        src_fmt: DXGI_FORMAT,
        src_wh: (u32, u32),
        dst: &ID3D12Resource,
        rtv_fmt: DXGI_FORMAT,
        dst_wh: (u32, u32),
    ) -> bool {
        let Some(pso) = self.pso(dev, rtv_fmt) else { return false };
        let srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: src_fmt,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 { Texture2D: D3D12_TEX2D_SRV { MostDetailedMip: 0, MipLevels: 1, PlaneSlice: 0, ResourceMinLODClamp: 0.0 } },
        };
        dev.CreateShaderResourceView(src, Some(&srv), self.srv_heap.GetCPUDescriptorHandleForHeapStart());
        let rtv = D3D12_RENDER_TARGET_VIEW_DESC { Format: rtv_fmt, ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D, ..Default::default() };
        let rtv_h = self.rtv_heap.GetCPUDescriptorHandleForHeapStart();
        dev.CreateRenderTargetView(dst, Some(&rtv), rtv_h);
        let (x, y, w, h) = super::display::letterbox(src_wh.0, src_wh.1, dst_wh.0, dst_wh.1);
        let consts = [src_wh.0 as f32, src_wh.1 as f32, src_wh.0 as f32 / w.max(1) as f32, src_wh.1 as f32 / h.max(1) as f32];
        list.SetGraphicsRootSignature(&self.root);
        list.SetPipelineState(&pso);
        list.SetDescriptorHeaps(&[Some(self.srv_heap.clone())]);
        list.SetGraphicsRoot32BitConstants(0, 4, consts.as_ptr() as *const c_void, 0);
        list.SetGraphicsRootDescriptorTable(1, self.srv_heap.GetGPUDescriptorHandleForHeapStart());
        list.OMSetRenderTargets(1, Some(&rtv_h), false, None);
        list.ClearRenderTargetView(rtv_h, &[0.0, 0.0, 0.0, 1.0], None);
        list.RSSetViewports(&[D3D12_VIEWPORT { TopLeftX: x as f32, TopLeftY: y as f32, Width: w as f32, Height: h as f32, MinDepth: 0.0, MaxDepth: 1.0 }]);
        list.RSSetScissorRects(&[RECT { left: x as i32, top: y as i32, right: (x + w) as i32, bottom: (y + h) as i32 }]);
        list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        list.DrawInstanced(3, 1, 0, 0);
        true
    }
}
