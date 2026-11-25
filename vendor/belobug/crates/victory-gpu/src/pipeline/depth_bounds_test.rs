//! Synthetic shaders, depth target and readback for a pipeline stream and
//! draws that go through the title entry points.

use super::*;
use crate::object::{S_OK, slot};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D::{Fxc::D3DCompile, ID3DBlob};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIAdapter1, IDXGIFactory4,
};
use windows::core::{GUID, Interface, PCSTR, s};

#[repr(C, align(8))]
struct Depth1(u32, D3D12_DEPTH_STENCIL_DESC1);

#[test]
fn depth_bounds_flag_changes_the_stream_state() {
    let mut g = host::GraphicsState::default();
    let mut depth = Depth1(21, D3D12_DEPTH_STENCIL_DESC1::default());
    let read = |g: &mut host::GraphicsState, d: &Depth1| {
        // SAFETY: a whole kind-21 subobject.
        unsafe { g.read(21, std::ptr::from_ref(d).cast()) };
    };
    read(&mut g, &depth);
    let disabled = g.describe();
    depth.1.DepthBoundsTestEnable = true.into();
    read(&mut g, &depth);
    assert!(g.describe() != disabled, "enabled bounds must have a different pipeline key");
    depth.1.DepthBoundsTestEnable = false.into();
    read(&mut g, &depth);
    assert_eq!(g.describe(), disabled);
}

fn fullscreen_vertex_shader() -> Arc<Shader> {
    let source = b"float4 main(uint id : SV_VertexID) : SV_Position {
        return float4(id == 1 ? 3 : -1, id == 2 ? -3 : 1, 0.25, 1);
    }";
    let mut code: Option<ID3DBlob> = None;
    // SAFETY: the arguments are live and code is the output pointer.
    unsafe {
        D3DCompile(
            source.as_ptr().cast(), source.len(), PCSTR::null(), None, None,
            s!("main"), s!("vs_5_1"), 0, 0, &mut code, None,
        ).unwrap();
    }
    let code = code.unwrap();
    // SAFETY: the blob owns these bytes.
    let bytes = unsafe {
        std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
    };
    host_shader("vertex", victory_shader::HwStage::Vs, bytes)
}

fn host_shader(stage: &'static str, hw_stage: victory_shader::HwStage, bytes: &[u8]) -> Arc<Shader> {
    let shader = shader(stage, bytes);
    shader.translated.get_or_init(|| Some(Translated {
        resource_heap: false,
        writes_memory: true,
        host: victory_shader::emit::HostShader {
            stage: hw_stage,
            dxil: bytes.to_vec(),
            user_data: Vec::new(),
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: Vec::new(),
            root_srvs: Vec::new(),
            root_uavs: Vec::new(),
            stride_table: false,
        },
        used: Vec::new(),
    }));
    shader
}

#[test]
fn depth_bounds_control_a_title_draw() {
    let Some(host) = crate::host::host() else {
        eprintln!("skipped depth bounds render: no host D3D12 device");
        return;
    };
    render_with_depth_bounds(host, fullscreen_vertex_shader(), None);
}

#[test]
fn depth_bounds_control_a_title_mesh_dispatch() {
    let Some(host) = crate::host::host().filter(|h| h.mesh_shaders()) else {
        eprintln!("skipped depth bounds mesh render: no host mesh shaders");
        return;
    };
    let Some(dxc) = std::env::var_os("VICTORY_PC_DXC") else {
        eprintln!("skipped depth bounds mesh render: VICTORY_PC_DXC is unset");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("triangle.hlsl");
    // WARP removed the device on a depth-only mesh dispatch without a
    // pixel shader, even with depth bounds disabled.
    std::fs::write(&source, r#"
        struct Vertex { float4 position : SV_Position; };
        [numthreads(1, 1, 1)] [outputtopology("triangle")]
        void main(out vertices Vertex verts[3], out indices uint3 triangles[1]) {
            SetMeshOutputCounts(3, 1);
            verts[0].position = float4(-1,  1, 0.25, 1);
            verts[1].position = float4( 3,  1, 0.25, 1);
            verts[2].position = float4(-1, -3, 0.25, 1);
            triangles[0] = uint3(0, 1, 2);
        }
        void ps() {}
    "#).unwrap();
    let compile = |profile, entry| {
        let code = dir.path().join(profile);
        let compiled = std::process::Command::new(&dxc)
            .args(["-T", profile, "-E", entry, "-Fo"])
            .arg(&code)
            .arg(&source)
            .output()
            .unwrap();
        assert!(compiled.status.success(), "{}", String::from_utf8_lossy(&compiled.stderr));
        std::fs::read(&code).unwrap()
    };
    let ms = host_shader("mesh", victory_shader::HwStage::Ms, &compile("ms_6_6", "main"));
    let ps = host_shader("pixel", victory_shader::HwStage::Ps, &compile("ps_6_6", "ps"));
    render_with_depth_bounds(host, ms, Some(ps));
}

fn render_with_depth_bounds(host: &crate::host::Host, shader: Arc<Shader>, ps: Option<Arc<Shader>>) {
    let device = &host.device;
    // SAFETY: plain adapter queries on a live device.
    let adapter = unsafe {
        let factory: IDXGIFactory4 = CreateDXGIFactory2(DXGI_CREATE_FACTORY_FLAGS(0)).unwrap();
        let adapter: IDXGIAdapter1 = factory.EnumAdapterByLuid(device.GetAdapterLuid()).unwrap();
        adapter.GetDesc1().unwrap()
    };
    let name = String::from_utf16_lossy(&adapter.Description);
    let supported = host.depth_bounds_supported;
    let mesh = shader.stage == "mesh";
    eprintln!("{} depth bounds on {}: {supported}", shader.stage, name.trim_end_matches('\0'));

    #[repr(C)]
    struct Stream {
        shader_kind: u64,
        shader: Bytecode,
        ps_kind: u64,
        ps: Bytecode,
        depth: Depth1,
        topology: [u32; 2],
        dsv: [u32; 2],
    }
    let stream = Stream {
        shader_kind: if mesh { 24 } else { 1 },
        shader: Bytecode { code: shader.container.as_ptr(), size: shader.container.len() },
        ps_kind: 2,
        ps: ps.as_ref().map_or(
            Bytecode { code: std::ptr::null(), size: 0 },
            |ps| Bytecode { code: ps.container.as_ptr(), size: ps.container.len() },
        ),
        depth: Depth1(21, D3D12_DEPTH_STENCIL_DESC1 {
            DepthEnable: true.into(),
            DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
            DepthFunc: D3D12_COMPARISON_FUNC_ALWAYS,
            DepthBoundsTestEnable: true.into(),
            ..Default::default()
        }),
        topology: [14, D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE.0 as u32],
        dsv: [16, DXGI_FORMAT_D32_FLOAT.0 as u32],
    };
    let desc = StreamDesc { size: size_of_val(&stream), stream: (&raw const stream).cast() };
    let null = std::ptr::null();
    let mut pso = null;
    assert_eq!(create_stream(null, &desc, &PIPELINE_IIDS[0], &mut pso), S_OK);
    assert_eq!(host_pipeline(pso).expect("host refused the depth bounds pipeline").mesh, mesh);

    let mut allocator = null;
    let mut list = null;
    let iid = |g: GUID| guid(g.data1, g.data2, g.data3, g.data4);
    assert_eq!(
        crate::list::create_allocator(null, 0, &iid(ID3D12CommandAllocator::IID), &mut allocator),
        S_OK
    );
    assert_eq!(
        crate::list::create_list(null, 0, 0, allocator, null, &iid(ID3D12GraphicsCommandList::IID), &mut list),
        S_OK
    );
    let commands = crate::list::host_list(list).unwrap();
    let l = &commands.list;
    let newer = crate::list::has_newer_table(list);
    let bounds = slot::<extern "system" fn(This, f32, f32)>(list, if newer { 0x230 } else { 0x220 });
    let scissors = slot::<extern "system" fn(This, u32, *const RECT)>(list, 0xb0);
    let draw = slot::<extern "system" fn(This, u32, u32, u32, u32)>(list, 0x60);
    let draw_pixel = |x| {
        scissors(list, 1, &RECT { left: x, top: 0, right: x + 1, bottom: 1 });
        if mesh {
            let dispatch = slot::<extern "system" fn(This, u32, u32, u32)>(
                list, if newer { 0x408 } else { 0x3f8 },
            );
            dispatch(list, 1, 1, 1);
        } else {
            draw(list, 3, 1, 0, 0);
        }
    };
    let depth_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: 4,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_D32_FLOAT,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
        ..Default::default()
    };
    let buffer_desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: 256,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: depth_desc.SampleDesc,
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        ..Default::default()
    };
    let resource = |desc: &D3D12_RESOURCE_DESC, kind, state| {
        let heap = D3D12_HEAP_PROPERTIES { Type: kind, ..Default::default() };
        let mut result: Option<ID3D12Resource> = None;
        // SAFETY: valid resource descriptions and output pointer.
        unsafe {
            device.CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, desc, state, None, &mut result)
                .unwrap();
        }
        result.unwrap()
    };
    let depth = resource(&depth_desc, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_DEPTH_WRITE);
    let readback = resource(&buffer_desc, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    let dsv = host.dsv(&depth, None);
    let bind = |l: &ID3D12GraphicsCommandList| {
        // SAFETY: an open list and the live depth buffer's descriptor.
        unsafe { l.OMSetRenderTargets(0, None, false, Some(&dsv)) };
        slot::<extern "system" fn(This, This)>(list, 0xc8)(list, pso);
        if !mesh {
            slot::<extern "system" fn(This, u32)>(list, 0xa0)(list, 4); // triangle list
        }
        slot::<extern "system" fn(This, u32, *const D3D12_VIEWPORT)>(list, 0xa8)(
            list, 1, &D3D12_VIEWPORT { Width: 4.0, Height: 1.0, MaxDepth: 1.0, ..Default::default() },
        );
    };
    let submit = |commands: &crate::host::HostList| {
        assert_eq!(slot::<extern "system" fn(This) -> HRESULT>(list, 0x48)(list), S_OK);
        host.execute(&[commands]);
    };
    // SAFETY: the list is open; the depth buffer is in DEPTH_WRITE.
    unsafe { l.ClearDepthStencilView(dsv, D3D12_CLEAR_FLAG_DEPTH, 0.5, 0, None) };
    bind(l);
    draw_pixel(0); // Default bounds on a fresh list.
    bounds(list, 0.0, 0.25);
    draw_pixel(1); // Excludes the stored depth, includes the triangle depth.
    bounds(list, 0.5, 0.75);
    draw_pixel(2); // Includes the stored depth, excludes the triangle depth.
    bounds(list, 0.0, 0.25);
    submit(&commands);

    assert_eq!(slot::<extern "system" fn(This, This, This) -> HRESULT>(list, 0x50)(list, allocator, null), S_OK);
    // Resetting a list whose host list is still referenced (here by
    // `commands`) records on into a fresh host list.
    let commands = crate::list::host_list(list).unwrap();
    let l = &commands.list;
    bind(l);
    draw_pixel(3); // Reset restores the default bounds.
    let mut from = D3D12_TEXTURE_COPY_LOCATION {
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        ..Default::default()
    };
    let mut to = D3D12_TEXTURE_COPY_LOCATION {
        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                Offset: 0,
                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                    Format: depth_desc.Format,
                    Width: depth_desc.Width as u32,
                    Height: 1,
                    Depth: 1,
                    RowPitch: 256,
                },
            },
        },
        ..Default::default()
    };
    // SAFETY: the locations borrow live resources, and the copy covers the whole subresource.
    unsafe {
        from.pResource = std::mem::transmute_copy(&depth);
        to.pResource = std::mem::transmute_copy(&readback);
        l.ResourceBarrier(&[crate::host::transition(&depth, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_COPY_SOURCE)]);
        l.CopyTextureRegion(&to, 0, 0, 0, &from, None);
    }
    submit(&commands);
    let mut mapped = std::ptr::null_mut();
    // SAFETY: the GPU copy finished and the readback buffer holds at least four floats.
    let actual = unsafe {
        readback.Map(0, Some(&D3D12_RANGE { Begin: 0, End: 16 }), Some(&mut mapped)).unwrap();
        let values = mapped.cast::<[f32; 4]>().read();
        readback.Unmap(0, Some(&D3D12_RANGE::default()));
        values
    };
    eprintln!("depth readback (fresh, excluded, included, reset): {actual:?}");
    assert_eq!(actual, [0.25, if supported { 0.5 } else { 0.25 }, 0.25, 0.25]);

    for object in [list, allocator, pso] {
        assert_eq!(slot::<extern "system" fn(This) -> u32>(object, 0x10)(object), 0);
    }
}
