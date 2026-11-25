//! A present's second plane, drawn over the first in our window.
//!
//! The console's display composes the planes of a present. The title in
//! its Series S mode presents two: its game image at 1920x1080 and its
//! interface at 3840x2160, whose empty texels are zero with alpha zero. A
//! window showing the first plane alone is black wherever the game plane
//! is, the menus and loading screens included.
//!
//! The pass here draws the second plane over the back buffer once the first
//! has been copied there: one triangle, the plane sampled across the whole
//! buffer (stretched to it), blended by its alpha.
//!
//! shortcut: the alpha is taken as premultiplied (what an interface drawn
//! onto a cleared target with the usual blending holds); planes after the
//! second, and the placement and scaling a plane's parameters can ask for,
//! are not read.

use std::sync::Mutex;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT, DXGI_SAMPLE_DESC};

const SOURCE: &str = r#"
Texture2D<float4> plane : register(t0);
SamplerState bilinear : register(s0);
struct Corner { float4 position : SV_Position; float2 at : TEXCOORD0; };
Corner corner(uint i : SV_VertexID) {
    Corner c;
    c.at = float2(float(i & 1) * 2.0, float(i >> 1) * 2.0);
    c.position = float4(c.at.x * 2.0 - 1.0, 1.0 - c.at.y * 2.0, 0.0, 1.0);
    return c;
}
float4 over(Corner c) : SV_Target { return plane.Sample(bilinear, c.at); }
"#;

/// Views kept until the list that uses them has run: a present can be sent
/// ahead of the card, so a view is not reused for this many presents.
///
/// shortcut: a fixed count, not derived from how many presents are in
/// flight. Presents sent ahead each hold a back buffer and the swap chain
/// has two, so no more than a few lists are ever waiting; 16 is well past
/// that.
const RING: u32 = 16;

struct Pass {
    format: i32,
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    /// The same draw for an opaque picture at an opacity: the list's blend factor.
    faded: ID3D12PipelineState,
    targets: ID3D12DescriptorHeap,
    views: ID3D12DescriptorHeap,
    target_step: usize,
    view_step: u32,
    next: u32,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Pass {}

static PASS: Mutex<Option<Pass>> = Mutex::new(None);

/// A bilinear, clamping sampler at `s0` for the stages `visibility` names.
fn bilinear(visibility: D3D12_SHADER_VISIBILITY) -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_LINEAR_MIP_POINT,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        MaxLOD: f32::MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: visibility,
        ..Default::default()
    }
}

/// A root signature of `parameters` and one bilinear sampler, serialized by
/// the runtime and not taken from a shader: the system compiler under Wine
/// compiles a `[RootSignature]` attribute without writing the signature
/// into the shader, and `CreateRootSignature` then fails on it (#188).
fn root_signature(
    device: &ID3D12Device,
    parameters: &[D3D12_ROOT_PARAMETER],
    sampler: D3D12_SHADER_VISIBILITY,
    flags: D3D12_ROOT_SIGNATURE_FLAGS,
) -> windows::core::Result<ID3D12RootSignature> {
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    let samplers = [bilinear(sampler)];
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: samplers.as_ptr(),
        Flags: flags,
    };
    let mut blob: Option<ID3DBlob> = None;
    // SAFETY: the description points at live slices; the blob is the
    // runtime's own, read while it lives.
    unsafe {
        D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)?;
        let blob = blob.ok_or_else(windows::core::Error::empty)?;
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes)
    }
}

/// One descriptor range of one view at register 0, at `offset` in its table.
fn range(kind: D3D12_DESCRIPTOR_RANGE_TYPE, offset: u32) -> D3D12_DESCRIPTOR_RANGE {
    D3D12_DESCRIPTOR_RANGE {
        RangeType: kind,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: offset,
    }
}

/// A root parameter that is a table of `ranges`, for the stages `visibility`
/// names.
fn table(ranges: &[D3D12_DESCRIPTOR_RANGE], visibility: D3D12_SHADER_VISIBILITY) -> D3D12_ROOT_PARAMETER {
    D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: ranges.len() as u32,
                pDescriptorRanges: ranges.as_ptr(),
            },
        },
        ShaderVisibility: visibility,
    }
}

fn make(device: &ID3D12Device, format: DXGI_FORMAT) -> windows::core::Result<Pass> {
    use windows::core::s;
    let vs = crate::host::compile(SOURCE, s!("corner"), s!("vs_5_1"))?;
    let ps = crate::host::compile(SOURCE, s!("over"), s!("ps_5_1"))?;
    let code = |bytes: &[u8]| D3D12_SHADER_BYTECODE {
        pShaderBytecode: bytes.as_ptr().cast(),
        BytecodeLength: bytes.len(),
    };
    let plane = [range(D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 0)];
    let root = root_signature(
        device,
        &[table(&plane, D3D12_SHADER_VISIBILITY_PIXEL)],
        D3D12_SHADER_VISIBILITY_PIXEL,
        D3D12_ROOT_SIGNATURE_FLAG_NONE,
    )?;
    // SAFETY: the description points at live data and never releases its
    // pointer.
    unsafe {
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            VS: code(&vs[..]),
            PS: code(&ps[..]),
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
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
        desc.BlendState.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
            BlendEnable: true.into(),
            SrcBlend: D3D12_BLEND_ONE,
            DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOp: D3D12_BLEND_OP_ADD,
            SrcBlendAlpha: D3D12_BLEND_ONE,
            DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
            BlendOpAlpha: D3D12_BLEND_OP_ADD,
            RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
            ..Default::default()
        };
        let pipeline: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc)?;
        desc.BlendState.RenderTarget[0].SrcBlend = D3D12_BLEND_BLEND_FACTOR;
        desc.BlendState.RenderTarget[0].DestBlend = D3D12_BLEND_INV_BLEND_FACTOR;
        desc.BlendState.RenderTarget[0].SrcBlendAlpha = D3D12_BLEND_BLEND_FACTOR;
        desc.BlendState.RenderTarget[0].DestBlendAlpha = D3D12_BLEND_INV_BLEND_FACTOR;
        let faded: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc)?;
        let targets: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: RING,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })?;
        let views: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: RING,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
        Ok(Pass {
            format: format.0,
            root,
            pipeline,
            faded,
            target_step: device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)
                as usize,
            view_step: device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            targets,
            views,
            next: 0,
        })
    }
}

/// Records into `list` the draw of `plane` (readable by pixel shaders, read
/// as `plane_format`) over `back` (in the render-target state, `width` by
/// `height`, of `format`). False if the pass could not be made: nothing was
/// recorded then.
///
/// The pass sets the list's descriptor heap, root signature, pipeline,
/// target, viewport, scissor and topology and puts none of them back: it is
/// for a list of ours that records nothing of the title's.
pub(crate) fn record(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    back: &ID3D12Resource,
    format: DXGI_FORMAT,
    size: (u32, u32),
    plane: &ID3D12Resource,
    plane_format: DXGI_FORMAT,
) -> bool {
    draw(device, list, back, format, size, plane, plane_format, None)
}

/// [`record`] for an opaque `picture` shown at `alpha` (0 gone .. 1 covering): the boot splash.
#[allow(clippy::too_many_arguments)]
pub(crate) fn record_faded(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    back: &ID3D12Resource,
    format: DXGI_FORMAT,
    size: (u32, u32),
    picture: &ID3D12Resource,
    picture_format: DXGI_FORMAT,
    alpha: f32,
) -> bool {
    draw(device, list, back, format, size, picture, picture_format, Some(alpha))
}

#[allow(clippy::too_many_arguments)]
fn draw(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    back: &ID3D12Resource,
    format: DXGI_FORMAT,
    (width, height): (u32, u32),
    plane: &ID3D12Resource,
    plane_format: DXGI_FORMAT,
    alpha: Option<f32>,
) -> bool {
    let mut pass = PASS.lock().unwrap_or_else(|e| e.into_inner());
    if pass.as_ref().is_none_or(|p| p.format != format.0) {
        *pass = match make(device, format) {
            Ok(p) => Some(p),
            Err(e) => {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| {
                    tracing::warn!(error = %e, "no pass to draw a present's second plane: it is not shown");
                });
                return false;
            }
        };
        tracing::info!(format = format.0, "a present's second plane is drawn over the first");
    }
    let Some(p) = pass.as_mut() else { return false };
    let slot = p.next % RING;
    p.next = p.next.wrapping_add(1);
    let view = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: plane_format,
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                ..Default::default()
            },
        },
    };
    let target_desc = D3D12_RENDER_TARGET_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_RTV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    // SAFETY: an open list; slots of our heaps; live resources and objects
    // (the pass's own live as long as the process or until remade, and a
    // list holds what it was recorded with).
    unsafe {
        let target = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: p.targets.GetCPUDescriptorHandleForHeapStart().ptr + slot as usize * p.target_step,
        };
        let cpu = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: p.views.GetCPUDescriptorHandleForHeapStart().ptr
                + (slot * p.view_step) as usize,
        };
        let gpu = D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: p.views.GetGPUDescriptorHandleForHeapStart().ptr
                + u64::from(slot * p.view_step),
        };
        device.CreateRenderTargetView(back, Some(&target_desc), target);
        device.CreateShaderResourceView(plane, Some(&view), cpu);
        list.SetDescriptorHeaps(&[Some(p.views.clone())]);
        list.SetGraphicsRootSignature(&p.root);
        match alpha {
            Some(a) => {
                list.SetPipelineState(&p.faded);
                list.OMSetBlendFactor(Some(&[a; 4]));
            }
            None => list.SetPipelineState(&p.pipeline),
        }
        list.SetGraphicsRootDescriptorTable(0, gpu);
        list.OMSetRenderTargets(1, Some(&target), false, None);
        list.RSSetViewports(&[D3D12_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: width as f32,
            Height: height as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        }]);
        list.RSSetScissorRects(&[RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        }]);
        list.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY(4));
        list.DrawInstanced(3, 1, 0, 0);
    }
    true
}

// ---------------------------------------------------------------------------
// The upscaler's output, by a plain stretch
// ---------------------------------------------------------------------------

/// The title in its Series S mode renders below its output size and has the
/// console's upscaler (its `ffxDispatch`) write the output. That upscaler's
/// passes are compute pipelines in a container we do not translate, so the
/// output stayed black. This pass stands in for it: each output texel is the
/// bilinear sample of the rendered part of the colour input.
///
/// shortcut: no temporal accumulation, sharpening or use of the depth and
/// motion inputs: an image, not the upscaler's quality.
const STRETCH: &str = r#"
cbuffer Sizes : register(b0) { float2 scale; float2 texel; };
Texture2D<float4> source : register(t0);
RWTexture2D<float4> target : register(u0);
SamplerState bilinear : register(s0);
[numthreads(8, 8, 1)]
void stretch(uint3 id : SV_DispatchThreadID) {
    // Groups are 8 by 8: the last ones reach past a size that is not a
    // multiple of 8.
    uint width, height;
    target.GetDimensions(width, height);
    if (id.x >= width || id.y >= height) return;
    float2 at = (float2(id.xy) + 0.5) * texel * scale;
    target[id.xy] = source.SampleLevel(bilinear, at, 0);
}
"#;

struct Stretch {
    root: ID3D12RootSignature,
    pipeline: ID3D12PipelineState,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: u32,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Stretch {}

static STRETCHER: Mutex<Option<Stretch>> = Mutex::new(None);

/// Views of a stretch are kept this many stretches (two views each).
///
/// shortcut: a fixed count. The title dispatches the upscaler once a frame
/// and a list runs within the frames the title keeps in flight; 64 frames
/// is far more than it keeps.
const STRETCH_RING: u32 = 64;

fn make_stretch(device: &ID3D12Device) -> windows::core::Result<Stretch> {
    use windows::core::s;
    let cs = crate::host::compile(STRETCH, s!("stretch"), s!("cs_5_1"))?;
    let views = [
        range(D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 0),
        range(D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1),
    ];
    let sizes = D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Constants: D3D12_ROOT_CONSTANTS {
                ShaderRegister: 0,
                RegisterSpace: 0,
                Num32BitValues: 4,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    let root = root_signature(
        device,
        &[sizes, table(&views, D3D12_SHADER_VISIBILITY_ALL)],
        D3D12_SHADER_VISIBILITY_ALL,
        D3D12_ROOT_SIGNATURE_FLAG_NONE,
    )?;
    // SAFETY: the description points at live data and never releases its
    // pointer.
    unsafe {
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: cs.as_ptr().cast(),
                BytecodeLength: cs.len(),
            },
            ..Default::default()
        };
        let pipeline: ID3D12PipelineState = device.CreateComputePipelineState(&desc)?;
        let views: ID3D12DescriptorHeap =
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: STRETCH_RING * 2,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?;
        Ok(Stretch {
            root,
            pipeline,
            step: device
                .GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV),
            views,
            next: 0,
        })
    }
}

/// A typeless colour format as the typed one shaders read and write.
fn typed(format: DXGI_FORMAT) -> DXGI_FORMAT {
    DXGI_FORMAT(match format.0 {
        1 => 2,
        9 => 10,
        23 => 24,
        27 => 28,
        f => f,
    })
}

/// Records into `list` the stretch of the top-left `render` texels of
/// `source` (readable by compute shaders) over the whole of `target` (in
/// the unordered-access state). False if nothing was recorded.
///
/// The pass sets the list's descriptor heap, compute root signature and
/// pipeline and puts none of them back: the caller replays the title's.
pub(crate) fn record_stretch(
    device: &ID3D12Device,
    list: &ID3D12GraphicsCommandList,
    target: &ID3D12Resource,
    source: &ID3D12Resource,
    render: [u32; 2],
) -> bool {
    // SAFETY: live resources.
    let (to, from) = unsafe { (target.GetDesc(), source.GetDesc()) };
    if to.Flags.0 & D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS.0 == 0
        || to.Width == 0
        || from.Width == 0
    {
        return false;
    }
    let mut made = STRETCHER.lock().unwrap_or_else(|e| e.into_inner());
    if made.is_none() {
        // Tried again at the next dispatch if it fails, as the pass over a
        // present is: a failure need not last.
        *made = match make_stretch(device) {
            Ok(p) => {
                tracing::info!("the upscaler's output is made by a plain stretch of its colour input");
                Some(p)
            }
            Err(e) => {
                static TOLD: std::sync::Once = std::sync::Once::new();
                TOLD.call_once(|| {
                    tracing::warn!(error = %e, "no pass to stretch the upscaler's input: its output stays as it is");
                });
                None
            }
        };
    }
    let Some(p) = made.as_mut() else { return false };
    let slot = (p.next % STRETCH_RING) * 2;
    p.next = p.next.wrapping_add(1);
    let (width, height) = (to.Width as u32, to.Height);
    let rendered = [
        if render[0] == 0 { from.Width as u32 } else { render[0].min(from.Width as u32) },
        if render[1] == 0 { from.Height } else { render[1].min(from.Height) },
    ];
    let sizes = [
        rendered[0] as f32 / from.Width as f32,
        rendered[1] as f32 / from.Height as f32,
        1.0 / width as f32,
        1.0 / height as f32,
    ];
    let read = D3D12_SHADER_RESOURCE_VIEW_DESC {
        Format: typed(from.Format),
        ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
        Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
        Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_SRV {
                MostDetailedMip: 0,
                MipLevels: 1,
                ..Default::default()
            },
        },
    };
    let written = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: typed(to.Format),
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        ..Default::default()
    };
    // SAFETY: an open list; slots of our heap; live resources and objects
    // (the pass's own live as long as the process).
    unsafe {
        let cpu = |i: u32| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: p.views.GetCPUDescriptorHandleForHeapStart().ptr + ((slot + i) * p.step) as usize,
        };
        let gpu = D3D12_GPU_DESCRIPTOR_HANDLE {
            ptr: p.views.GetGPUDescriptorHandleForHeapStart().ptr + u64::from(slot * p.step),
        };
        device.CreateShaderResourceView(source, Some(&read), cpu(0));
        device.CreateUnorderedAccessView(target, None, Some(&written), cpu(1));
        list.SetDescriptorHeaps(&[Some(p.views.clone())]);
        list.SetComputeRootSignature(&p.root);
        list.SetPipelineState(&p.pipeline);
        list.SetComputeRoot32BitConstants(0, 4, sizes.as_ptr().cast(), 0);
        list.SetComputeRootDescriptorTable(1, gpu);
        list.Dispatch(width.div_ceil(8), height.div_ceil(8), 1);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Typeless colour formats are read and written as their typed ones;
    /// typed formats, and formats with no typeless form here, stay.
    #[test]
    fn typeless_formats_get_their_typed_one() {
        for (typeless, typed_one) in [(1, 2), (9, 10), (23, 24), (27, 28)] {
            assert_eq!(typed(DXGI_FORMAT(typeless)).0, typed_one);
            assert_eq!(typed(DXGI_FORMAT(typed_one)).0, typed_one);
        }
        // R11G11B10_FLOAT, R8G8B8A8_UNORM_SRGB and the FP16 stand-in's own.
        for format in [26, 29, 10, 0] {
            assert_eq!(typed(DXGI_FORMAT(format)).0, format);
        }
    }

    /// The passes' root signatures are serialized by the runtime: neither
    /// source asks the compiler for one (Wine's writes none).
    #[test]
    fn the_sources_carry_no_root_signature() {
        for source in [SOURCE, STRETCH] {
            assert!(!source.contains("RootSignature"));
            assert!(!source.contains("#define RS"));
        }
    }

    /// The stretch's source guards its writes: groups of 8 reach past sizes
    /// that are not multiples of 8.
    #[test]
    fn the_stretch_writes_inside_its_target_only() {
        assert!(STRETCH.contains("numthreads(8, 8, 1)"));
        assert!(STRETCH.contains("if (id.x >= width || id.y >= height) return;"));
    }
}
