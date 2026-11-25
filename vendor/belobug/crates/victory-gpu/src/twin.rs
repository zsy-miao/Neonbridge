//! Depth textures that shaders also write as unordered-access images.
//!
//! The console lets a depth texture be both: Grand Theft Auto VI's material
//! classifier (`CS_GenerateMaterialMasks`, `CS_MaterialTilesInitial`) writes
//! each pixel's material number (n / 65535) into the depth plane of a
//! 16-bit depth + stencil texture and flag bits into its stencil plane; the
//! tiled lighting then draws each material's tiles at depth n / 65535 with
//! that texture bound for depth, so only the pixels of that material are
//! lit. The host refuses unordered access on depth textures, so those
//! writes were lost and only material 0 was ever lit (whole tiles without
//! it stayed unlit squares).
//!
//! Such a texture gets a twin per plane the host can write: depth as
//! `R16_UNORM` (the console's 16-bit depth), stencil as `R8_UINT`. The
//! title's unordered-access views are made on the twins, and right after a
//! dispatch that wrote one, a small draw copies it into the depth texture
//! (depth through `SV_Depth`, stencil through `SV_StencilRef`).
//! Everything else (draws, shader reads, clears) keeps using the depth
//! texture itself.
//!
//! shortcut: the copy goes one way. Depth or stencil written by draws or
//! clears is not copied back to the twins, so a dispatch writing only part
//! of a twin after that puts back older values elsewhere. The material
//! classifier writes every pixel first, so it does not; only the console's
//! 16-bit depth + stencil formats (118-120) get twins for now, not the
//! 16-bit shadow maps draws write.
//! shortcut: mip 0 and slice 0 only.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::core::{PCSTR, s};

use crate::host::{Host, HostList};
use crate::object::This;

macro_rules! once {
    ($($arg:tt)*) => {{
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| tracing::warn!($($arg)*));
    }};
}

/// A depth texture's planes as textures shaders can write.
pub(crate) struct Twin {
    pub(crate) depth: ID3D12Resource,
    pub(crate) stencil: Option<ID3D12Resource>,
    depth_format: DXGI_FORMAT,
    /// The depth texture's typed depth format.
    dsv_format: DXGI_FORMAT,
    width: u32,
    height: u32,
}

/// The planes of a depth texture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Plane {
    Depth,
    Stencil,
}

/// Console formats of 16-bit depth with 8-bit stencil (typed, depth view,
/// stencil view).
const D16_S8: std::ops::RangeInclusive<u32> = 118..=120;

/// Whether a title texture of `format` and `flags` gets twins.
/// `VICTORY_DEPTH_TWINS=0` turns them off (only the first material is lit).
pub(crate) fn wanted(format: u32, flags: u32) -> bool {
    const DEPTH: u32 = 2;
    const UNORDERED: u32 = 4;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| !std::env::var("VICTORY_DEPTH_TWINS").is_ok_and(|v| v == "0"))
        && flags & (DEPTH | UNORDERED) == DEPTH | UNORDERED
        && D16_S8.contains(&format)
}

/// Makes the twins of the host depth texture `texture`.
pub(crate) fn make(texture: &ID3D12Resource) -> Option<Twin> {
    let host = crate::host::host()?;
    // SAFETY: a live resource.
    let d = unsafe { texture.GetDesc() };
    let own = d.Format.0 as u32;
    let typed = crate::resource::DEPTH_FORMATS
        .iter()
        .find(|&&(typed, typeless)| typed == own || typeless == own)
        .map(|&(typed, _)| typed)?;
    // D24S8 and D32S8 have stencil (the console's 16-bit depth with stencil
    // becomes D24S8 on the host).
    let has_stencil = matches!(typed, 20 | 45);
    let plane = |format: DXGI_FORMAT| -> Option<ID3D12Resource> {
        let desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: d.Width,
            Height: d.Height,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Layout: D3D12_TEXTURE_LAYOUT_UNKNOWN,
            Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
            ..Default::default()
        };
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let mut made: Option<ID3D12Resource> = None;
        // SAFETY: valid descriptions; the out pointer is ours.
        let result = unsafe {
            host.device.CreateCommittedResource(
                &heap,
                D3D12_HEAP_FLAG_NONE,
                &desc,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                None,
                &mut made,
            )
        };
        if let Err(e) = result {
            tracing::warn!(error = %e, ?format, "depth twin not made");
        }
        made
    };
    let depth_format = DXGI_FORMAT_R16_UNORM;
    let twin = Twin {
        depth: plane(depth_format)?,
        stencil: if has_stencil {
            plane(DXGI_FORMAT_R8_UINT)
        } else {
            None
        },
        depth_format,
        dsv_format: DXGI_FORMAT(typed as i32),
        width: d.Width as u32,
        height: d.Height,
    };
    tracing::info!(
        width = twin.width,
        height = twin.height,
        dsv = typed,
        stencil = twin.stencil.is_some(),
        "depth texture written as an image: twins made"
    );
    Some(twin)
}

/// The plane a title unordered-access view `words` (host layout, title
/// format) names.
pub(crate) fn plane_of(words: Option<&[u32; 10]>) -> Plane {
    let Some(w) = words else {
        return Plane::Depth;
    };
    const TEXTURE2D: u32 = 4;
    let format = crate::resource::pc_format(w[0]);
    // Stencil views: X24_TYPELESS_G8_UINT, X32_TYPELESS_G8X24_UINT,
    // R8_UINT, or plane 1 of a two-dimensional view.
    if matches!(format, 47 | 22 | 62) || (w[1] == TEXTURE2D && w[3] == 1) {
        Plane::Stencil
    } else {
        Plane::Depth
    }
}

/// Makes the title's unordered-access view `words` of a depth texture with
/// twins at `at`, on the twin of its plane; that twin.
pub(crate) fn view(twin: &Twin, words: Option<&[u32; 10]>, at: D3D12_CPU_DESCRIPTOR_HANDLE) -> Option<ID3D12Resource> {
    let plane = plane_of(words);
    let (texture, format) = match plane {
        Plane::Depth => (&twin.depth, twin.depth_format),
        Plane::Stencil => (twin.stencil.as_ref()?, DXGI_FORMAT_R8_UINT),
    };
    if let Some(w) = words
        && (w[1] != 4 || w[2] != 0)
    {
        once!(
            dimension = w[1],
            mip = w[2],
            "a depth texture written as an image other than mip 0 of a 2D view: taken as mip 0"
        );
    }
    let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_UAV {
                MipSlice: 0,
                PlaneSlice: 0,
            },
        },
    };
    let host = crate::host::host()?;
    // SAFETY: a live texture that allows unordered access; a slot of the
    // caller's heap.
    unsafe {
        host.device
            .CreateUnorderedAccessView(texture, None::<&ID3D12Resource>, Some(&desc), at)
    };
    Some(texture.clone())
}

thread_local! {
    /// Twins the dispatch being recorded writes: title resource, plane.
    static WRITTEN: RefCell<Vec<(usize, Plane)>> = const { RefCell::new(Vec::new()) };
}

/// A dispatch is about to bind its views.
pub(crate) fn begin() {
    WRITTEN.with(|w| {
        let mut w = w.borrow_mut();
        if !w.is_empty() {
            once!("a draw wrote a depth texture as an image: not copied to it");
            w.clear();
        }
    });
}

/// The dispatch being bound writes the plane of `resource` that `words`
/// names (an unordered-access view of a texture with twins).
pub(crate) fn note(resource: This, words: Option<&[u32; 10]>) {
    let entry = (resource as usize, plane_of(words));
    WRITTEN.with(|w| {
        let mut w = w.borrow_mut();
        if !w.contains(&entry) {
            w.push(entry);
        }
    });
}

/// After a dispatch: copies the twins it wrote into their depth textures.
/// True if anything was recorded (the list's pipeline, root signatures,
/// heaps, targets, viewports and scissors then need setting again).
pub(crate) fn resolve(list: &HostList) -> bool {
    let written = WRITTEN.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let mut recorded = false;
    let mut done: Vec<usize> = Vec::new();
    for &(resource, _) in &written {
        if done.contains(&resource) {
            continue;
        }
        done.push(resource);
        let depth = written.contains(&(resource, Plane::Depth));
        let stencil = written.contains(&(resource, Plane::Stencil));
        recorded |= copy(list, resource as This, depth, stencil);
    }
    recorded
}

const SOURCE: &str = r#"
#define RS "DescriptorTable(SRV(t0, numDescriptors=2), visibility=SHADER_VISIBILITY_PIXEL)"
Texture2D<float> depth_ : register(t0);
Texture2D<uint> stencil_ : register(t1);
[RootSignature(RS)]
float4 vs(uint id : SV_VertexID) : SV_Position {
    float2 uv = float2((id << 1) & 2, id & 2);
    return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1);
}
struct Both { float depth : SV_Depth; uint stencil : SV_StencilRef; };
[RootSignature(RS)]
Both both(float4 at : SV_Position) {
    Both o;
    o.depth = depth_.Load(int3(at.xy, 0));
    o.stencil = stencil_.Load(int3(at.xy, 0));
    return o;
}
[RootSignature(RS)]
float depth_only(float4 at : SV_Position) : SV_Depth {
    return depth_.Load(int3(at.xy, 0));
}
[RootSignature(RS)]
uint stencil_only(float4 at : SV_Position) : SV_StencilRef {
    return stencil_.Load(int3(at.xy, 0));
}
"#;

/// Descriptor pairs, used in turn: more than the copies of every list in
/// flight.
const SLOTS: u32 = 4096;

/// Depth format, depth written, stencil written.
type PipelineKey = (i32, bool, bool);

struct Copier {
    root: ID3D12RootSignature,
    vs: ID3DBlob,
    both: ID3DBlob,
    depth_only: ID3DBlob,
    stencil_only: ID3DBlob,
    /// Whether the host takes stencil values from pixel shaders.
    stencil_ref: bool,
    /// By depth format, depth written, stencil written.
    pipelines: std::sync::Mutex<Vec<(PipelineKey, ID3D12PipelineState)>>,
    views: ID3D12DescriptorHeap,
    step: u32,
    next: AtomicU32,
}
// SAFETY: D3D12 objects are free-threaded; blobs are only read.
unsafe impl Send for Copier {}
// SAFETY: as above.
unsafe impl Sync for Copier {}

fn compile(entry: PCSTR, target: PCSTR) -> windows::core::Result<ID3DBlob> {
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    // SAFETY: the source and names live through the call; the out pointers
    // are ours.
    let result = unsafe {
        D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            s!("twin"),
            None,
            None,
            entry,
            target,
            0,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    if let (Err(e), Some(errors)) = (&result, &errors) {
        // SAFETY: the blob's own bytes.
        let text = unsafe {
            std::slice::from_raw_parts(
                errors.GetBufferPointer().cast::<u8>(),
                errors.GetBufferSize(),
            )
        };
        tracing::warn!(error = %e, text = %String::from_utf8_lossy(text), "depth twin copy not compiled");
    }
    result?;
    code.ok_or_else(windows::core::Error::empty)
}

fn bytes(b: &ID3DBlob) -> &[u8] {
    // SAFETY: the blob's own bytes, alive as long as it.
    unsafe { std::slice::from_raw_parts(b.GetBufferPointer().cast::<u8>(), b.GetBufferSize()) }
}

fn make_copier(host: &Host) -> windows::core::Result<Copier> {
    let vs = compile(s!("vs"), s!("vs_5_1"))?;
    let both = compile(s!("both"), s!("ps_5_1"))?;
    let depth_only = compile(s!("depth_only"), s!("ps_5_1"))?;
    let stencil_only = compile(s!("stencil_only"), s!("ps_5_1"))?;
    let device = &host.device;
    let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
    // SAFETY: the structure the feature names, ours.
    let stencil_ref = unsafe {
        device.CheckFeatureSupport(
            D3D12_FEATURE_D3D12_OPTIONS,
            std::ptr::from_mut(&mut options).cast(),
            std::mem::size_of_val(&options) as u32,
        )
    }
    .is_ok()
        && options.PSSpecifiedStencilRefSupported.as_bool();
    if !stencil_ref {
        tracing::warn!("the host takes no stencil from pixel shaders: depth twins copy depth only");
    }
    // SAFETY: a root signature the compiler embedded.
    let root: ID3D12RootSignature = unsafe { device.CreateRootSignature(0, bytes(&vs))? };
    // SAFETY: a plain description.
    let views = unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
            NumDescriptors: 2 * SLOTS,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
            NodeMask: 0,
        })?
    };
    Ok(Copier {
        root,
        vs,
        both,
        depth_only,
        stencil_only,
        stencil_ref,
        pipelines: std::sync::Mutex::new(Vec::new()),
        views,
        // SAFETY: a plain query.
        step: unsafe { device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV) },
        next: AtomicU32::new(0),
    })
}

impl Copier {
    fn pipeline(&self, host: &Host, format: DXGI_FORMAT, depth: bool, stencil: bool) -> Option<ID3D12PipelineState> {
        let key = (format.0, depth, stencil);
        let mut made = self.pipelines.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, p)) = made.iter().find(|(k, _)| *k == key) {
            return Some(p.clone());
        }
        let ps = match (depth, stencil) {
            (true, true) => &self.both,
            (true, false) => &self.depth_only,
            (false, true) => &self.stencil_only,
            (false, false) => return None,
        };
        let code = |b: &ID3DBlob| D3D12_SHADER_BYTECODE {
            // SAFETY: the blob lives as long as the copier.
            pShaderBytecode: unsafe { b.GetBufferPointer() },
            // SAFETY: as above.
            BytecodeLength: unsafe { b.GetBufferSize() },
        };
        let face = D3D12_DEPTH_STENCILOP_DESC {
            StencilFailOp: D3D12_STENCIL_OP_REPLACE,
            StencilDepthFailOp: D3D12_STENCIL_OP_REPLACE,
            StencilPassOp: D3D12_STENCIL_OP_REPLACE,
            StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
        };
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            // SAFETY: borrowed for the call; the copier holds the root.
            pRootSignature: unsafe { std::mem::transmute_copy(&self.root) },
            VS: code(&self.vs),
            PS: code(ps),
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: false.into(),
                ..Default::default()
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                // Depth tests also gate depth writes: always pass.
                DepthEnable: true.into(),
                DepthWriteMask: if depth {
                    D3D12_DEPTH_WRITE_MASK_ALL
                } else {
                    D3D12_DEPTH_WRITE_MASK_ZERO
                },
                DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
                StencilEnable: stencil.into(),
                StencilReadMask: 0xff,
                StencilWriteMask: 0xff,
                FrontFace: face,
                BackFace: face,
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 0,
            DSVFormat: format,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        // SAFETY: a complete description with live shaders and root.
        match unsafe { host.device.CreateGraphicsPipelineState::<ID3D12PipelineState>(&desc) } {
            Ok(p) => {
                made.push((key, p.clone()));
                Some(p)
            }
            Err(e) => {
                once!(error = %e, ?format, depth, stencil, "depth twin copy pipeline refused");
                None
            }
        }
    }
}

fn copier(host: &Host) -> Option<&'static Copier> {
    static COPIER: std::sync::OnceLock<Option<Copier>> = std::sync::OnceLock::new();
    COPIER
        .get_or_init(|| {
            make_copier(host)
                .inspect_err(|e| tracing::warn!(error = %e, "depth twins cannot be copied"))
                .ok()
        })
        .as_ref()
}

/// Records the copy of `resource`'s twins (the planes asked) into it.
fn copy(list: &HostList, resource: This, depth: bool, stencil: bool) -> bool {
    let (Some(host), Some(texture)) = (crate::host::host(), crate::resource::host_resource(resource)) else {
        return false;
    };
    let Some(twin) = crate::resource::depth_twin(resource) else {
        return false;
    };
    let Some(c) = copier(host) else {
        return false;
    };
    let stencil = stencil && c.stencil_ref && twin.stencil.is_some();
    let Some(pso) = c.pipeline(host, twin.dsv_format, depth, stencil) else {
        return false;
    };
    let l = &list.list;
    const DEPTH_WRITE: u32 = 0x10;
    crate::list::ensure_state(list, resource, DEPTH_WRITE);
    let planes: Vec<&ID3D12Resource> = std::iter::once(&twin.depth).chain(twin.stencil.as_ref()).collect();
    let barriers = |before, after| -> Vec<D3D12_RESOURCE_BARRIER> {
        planes
            .iter()
            .map(|t| crate::host::transition(t, before, after))
            .collect()
    };
    let first = 2 * (c.next.fetch_add(1, Ordering::Relaxed) % SLOTS);
    // SAFETY: an open list; live textures; the views are made in slots no
    // list in flight reads.
    unsafe {
        let cpu = c.views.GetCPUDescriptorHandleForHeapStart();
        let gpu = c.views.GetGPUDescriptorHandleForHeapStart();
        for (i, (plane, format)) in [
            (Some(&twin.depth), twin.depth_format),
            (twin.stencil.as_ref(), DXGI_FORMAT_R8_UINT),
        ]
        .into_iter()
        .enumerate()
        {
            let at = D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: cpu.ptr + (first as usize + i) * c.step as usize,
            };
            let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: format,
                ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Texture2D: D3D12_TEX2D_SRV {
                        MipLevels: 1,
                        ..Default::default()
                    },
                },
            };
            host.device.CreateShaderResourceView(plane, Some(&desc), at);
        }
        l.ResourceBarrier(&barriers(
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
        ));
        let dsv_desc = D3D12_DEPTH_STENCIL_VIEW_DESC {
            Format: twin.dsv_format,
            ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2D,
            Flags: D3D12_DSV_FLAG_NONE,
            Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_DSV { MipSlice: 0 },
            },
        };
        let dsv = host.dsv(texture, Some(&dsv_desc));
        l.OMSetRenderTargets(0, None, false, Some(&dsv));
        l.SetDescriptorHeaps(&[Some(c.views.clone())]);
        l.SetGraphicsRootSignature(&c.root);
        l.SetPipelineState(&pso);
        l.SetGraphicsRootDescriptorTable(
            0,
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: gpu.ptr + u64::from(first) * u64::from(c.step),
            },
        );
        l.IASetPrimitiveTopology(windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        l.RSSetViewports(&[D3D12_VIEWPORT {
            Width: twin.width as f32,
            Height: twin.height as f32,
            MaxDepth: 1.0,
            ..Default::default()
        }]);
        l.RSSetScissorRects(&[windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: twin.width as i32,
            bottom: twin.height as i32,
        }]);
        l.DrawInstanced(3, 1, 0, 0);
        l.ResourceBarrier(&barriers(
            D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
        ));
    }
    list.keep(&c.views);
    list.keep(&pso);
    list.keep(&twin.depth);
    if let Some(s) = &twin.stencil {
        list.keep(s);
    }
    true
}
