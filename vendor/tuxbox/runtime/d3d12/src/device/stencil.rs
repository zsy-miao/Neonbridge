//! Stencil-reference export through an extra R8 render target. The depth test,
//! coverage and discard still happen in the title's original draw.
use super::Device;
use std::sync::{Arc, Mutex, OnceLock};
use windows::Win32::Foundation::{E_INVALIDARG, E_NOTIMPL};
use windows::Win32::Graphics::{Direct3D12::*, Dxgi::Common::*};
use windows::core::Result;

pub(crate) struct Surface {
    pub(crate) depth: ID3D12Resource,
    mirror: OnceLock<Mirror>,
    creation: Mutex<()>,
}

pub(crate) struct Mirror {
    pub(crate) color: ID3D12Resource,
    pub(crate) transfer: ID3D12Resource,
    pub(crate) layout: D3D12_PLACED_SUBRESOURCE_FOOTPRINT,
    heap: ID3D12DescriptorHeap,
}

pub(super) fn supported_view(
    res: &ID3D12Resource,
    view: Option<&D3D12_DEPTH_STENCIL_VIEW_DESC>,
) -> bool {
    let d = unsafe { res.GetDesc() };
    let fmt = view.map_or(d.Format, |v| v.Format);
    matches!(
        fmt,
        DXGI_FORMAT_D32_FLOAT_S8X24_UINT | DXGI_FORMAT_D24_UNORM_S8_UINT
    ) && view.is_none_or(|v| {
        v.Flags == D3D12_DSV_FLAG_NONE && v.ViewDimension == D3D12_DSV_DIMENSION_TEXTURE2D
    }) && d.Dimension == D3D12_RESOURCE_DIMENSION_TEXTURE2D
        && d.MipLevels == 1
        && d.DepthOrArraySize == 1
        && d.SampleDesc.Count == 1
}

impl Surface {
    pub(super) fn new(depth: &ID3D12Resource) -> Self {
        Self {
            depth: depth.clone(),
            mirror: OnceLock::new(),
            creation: Mutex::new(()),
        }
    }

    pub(crate) fn mirror(&self, device: &ID3D12Device) -> Result<&Mirror> {
        if let Some(value) = self.mirror.get() {
            return Ok(value);
        }
        let _lock = self.creation.lock().unwrap_or_else(|e| e.into_inner());
        if self.mirror.get().is_none() {
            let _ = self.mirror.set(Mirror::new(device, &self.depth)?);
        }
        Ok(self
            .mirror
            .get()
            .expect("mirror initialized under creation lock"))
    }
}

pub(crate) fn cpu_heap(
    device: &ID3D12Device,
    kind: D3D12_DESCRIPTOR_HEAP_TYPE,
    count: u32,
) -> Result<ID3D12DescriptorHeap> {
    unsafe {
        device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
            Type: kind,
            NumDescriptors: count,
            NodeMask: 1,
            Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
        })
    }
}

impl Mirror {
    pub(crate) fn rtv(&self) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        unsafe { self.heap.GetCPUDescriptorHandleForHeapStart() }
    }

    fn new(device: &ID3D12Device, depth: &ID3D12Resource) -> Result<Self> {
        let desc = unsafe { depth.GetDesc() };
        let mut layout = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
        let (mut size, mut rows, mut row_bytes) = (0, 0, 0);
        unsafe {
            device.GetCopyableFootprints(
                &desc,
                1,
                1,
                0,
                Some(&mut layout),
                Some(&mut rows),
                Some(&mut row_bytes),
                Some(&mut size),
            );
        }
        if size == 0
            || size == u64::MAX
            || rows != desc.Height
            || row_bytes != desc.Width
            || !matches!(
                layout.Footprint.Format,
                DXGI_FORMAT_R8_UINT | DXGI_FORMAT_R8_TYPELESS
            )
        {
            return Err(E_NOTIMPL.into());
        }
        let heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            CreationNodeMask: 1,
            VisibleNodeMask: 1,
            ..Default::default()
        };
        let allocate = |desc: &D3D12_RESOURCE_DESC, state| {
            let mut out = None;
            unsafe {
                device.CreateCommittedResource(
                    &heap,
                    D3D12_HEAP_FLAG_NONE,
                    desc,
                    state,
                    None,
                    &mut out,
                )?;
            }
            out.ok_or_else(|| windows::core::Error::from(E_INVALIDARG))
        };
        let color = allocate(
            &D3D12_RESOURCE_DESC {
                Format: DXGI_FORMAT_R8_UINT,
                Alignment: 0,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                ..desc
            },
            D3D12_RESOURCE_STATE_RENDER_TARGET,
        )?;
        let transfer = allocate(
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
                ..Default::default()
            },
            D3D12_RESOURCE_STATE_COPY_DEST,
        )?;
        let heap = cpu_heap(device, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1)?;
        unsafe {
            device.CreateRenderTargetView(&color, None, heap.GetCPUDescriptorHandleForHeapStart());
        }
        Ok(Self {
            color,
            transfer,
            layout,
            heap,
        })
    }
}

/// The parent is the guest device retained by the command list.
pub(crate) unsafe fn binding<'a>(
    parent: *mut core::ffi::c_void,
    dsv: usize,
) -> Option<(&'a ID3D12Device, Arc<Surface>)> {
    let device = unsafe { &*parent.cast::<Device>() };
    if !needs_emulation(device) {
        return None;
    }
    Some((&device.native, device.state.depth_views.get(dsv)?))
}

pub(super) fn needs_emulation(device: &Device) -> bool {
    *device.state.emulate_stencil.get_or_init(|| {
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS::default();
        unsafe {
            device.native.CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS,
                (&raw mut options).cast(),
                size_of_val(&options) as u32,
            )
        }
        .is_ok()
            && !options.PSSpecifiedStencilRefSupported.as_bool()
    })
}

/// The mirror can implement a full-byte REPLACE for passing fragments. Other
/// stencil operations depend on the previous byte or on fragments failing tests.
pub(super) fn redirect(
    device: &Device,
    ps: &[u8],
    samples: u32,
    depth: &mut D3D12_DEPTH_STENCIL_DESC1,
    formats: &mut D3D12_RT_FORMAT_ARRAY,
    blend: &mut D3D12_BLEND_DESC,
) -> Result<Option<(u32, Vec<u8>)>> {
    use std::sync::{LazyLock, Once};
    static TRACE: LazyLock<bool> =
        LazyLock::new(|| std::env::var("TUXBOX_TRACE_STENCIL").as_deref() == Ok("1"));
    let emulated = needs_emulation(device);
    if (!emulated && !*TRACE)
        || !depth.StencilEnable.as_bool()
        || !tuxbox_dxbc::exports_stencil_reference(ps)
    {
        return Ok(None);
    }
    let slot = formats.NumRenderTargets;
    if *TRACE {
        static FIRST: Once = Once::new();
        let face = |f: &D3D12_DEPTH_STENCILOP_DESC| {
            format!(
                "{}/{}/{}/{}",
                f.StencilFunc.0, f.StencilPassOp.0, f.StencilFailOp.0, f.StencilDepthFailOp.0
            )
        };
        crate::diagnostics::log_once(&FIRST, || {
            format!(
                "stencil output rt={slot} samples={samples} mask={} depth={} front={} back={}",
                depth.StencilWriteMask,
                depth.DepthFunc.0,
                face(&depth.FrontFace),
                face(&depth.BackFace)
            )
        });
    }
    let replace = |face: &D3D12_DEPTH_STENCILOP_DESC| {
        face.StencilFunc == D3D12_COMPARISON_FUNC_ALWAYS
            && face.StencilPassOp == D3D12_STENCIL_OP_REPLACE
            && face.StencilFailOp == D3D12_STENCIL_OP_KEEP
            && face.StencilDepthFailOp == D3D12_STENCIL_OP_KEEP
    };
    if !emulated
        || slot >= 8
        || samples != 1
        || depth.StencilWriteMask != 255
        || !replace(&depth.FrontFace)
        || !replace(&depth.BackFace)
    {
        return Ok(None);
    }
    let code = super::shader_cache::stencil_target(ps, slot).ok_or(E_NOTIMPL)?;
    // Leave the native DSV enabled with no stencil writes. The integer
    // target receives each surviving fragment under the original tests.
    depth.StencilWriteMask = 0;
    if !blend.IndependentBlendEnable.as_bool() {
        for n in 1..slot as usize {
            blend.RenderTarget[n] = blend.RenderTarget[0];
        }
        blend.IndependentBlendEnable = true.into();
    }
    blend.RenderTarget[slot as usize] = D3D12_RENDER_TARGET_BLEND_DESC {
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_ZERO,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_ZERO,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_RED.0 as u8,
        ..Default::default()
    };
    formats.RTFormats[slot as usize] = DXGI_FORMAT_R8_UINT;
    formats.NumRenderTargets += 1;
    Ok(Some((slot, code)))
}
