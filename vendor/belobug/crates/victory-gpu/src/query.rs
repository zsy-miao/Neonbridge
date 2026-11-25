//! Query heaps. A heap of a type the host has (occlusion, timestamps,
//! pipeline statistics) gets a host query heap of its size on first use,
//! which lists record their queries into (`list.rs`); other heaps only
//! remember their description, and resolving them writes stand-in values.
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use crate::object::{self, E_INVALIDARG, HRESULT, Iid, Object, QUERY_HEAP, Table, This, guid};
use crate::resource::PAGEABLE_IIDS;

const HEAP_IIDS: [Iid; 4] = [
    guid(
        0x0d9658ae,
        0xed45,
        0x469e,
        [0xa6, 0x1d, 0x97, 0x0e, 0xc5, 0x83, 0xca, 0xb4],
    ),
    PAGEABLE_IIDS[0],
    PAGEABLE_IIDS[1],
    PAGEABLE_IIDS[2],
];

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub(crate) struct Desc {
    kind: u32,
    count: u32,
    nodes: u32,
}

#[repr(C)]
struct Heap {
    object: Object,
    desc: Desc,
    #[cfg(windows)]
    host: std::sync::OnceLock<Option<HostHeap>>,
}

/// Heap types (`D3D12_QUERY_HEAP_TYPE`) and the query types they hold.
const OCCLUSION_HEAP: u32 = 0;
const TIMESTAMP_HEAP: u32 = 1;
const PIPELINE_STATISTICS_HEAP: u32 = 2;
pub(crate) const OCCLUSION: u32 = 0;
pub(crate) const BINARY_OCCLUSION: u32 = 1;
pub(crate) const TIMESTAMP: u32 = 2;
pub(crate) const PIPELINE_STATISTICS: u32 = 3;

/// The heap type a query type needs, for those the host runs.
fn heap_type(kind: u32) -> Option<u32> {
    match kind {
        OCCLUSION | BINARY_OCCLUSION => Some(OCCLUSION_HEAP),
        TIMESTAMP => Some(TIMESTAMP_HEAP),
        PIPELINE_STATISTICS => Some(PIPELINE_STATISTICS_HEAP),
        _ => None,
    }
}

/// Heaps with more queries than this get no host heap (their resolves keep
/// the stand-in values): titles use thousands, not millions.
const MOST_QUERIES: u32 = 1 << 18;

/// The host query heap behind a title heap.
#[cfg(windows)]
pub(crate) struct HostHeap {
    pub(crate) heap: windows::Win32::Graphics::Direct3D12::ID3D12QueryHeap,
    /// Per query: the type its last begin was ended as in a host list, plus
    /// one (0: not ended since it began, or never run), so a resolve reads
    /// only results the host GPU wrote.
    ended: Vec<std::sync::atomic::AtomicU8>,
}

#[cfg(windows)]
impl HostHeap {
    /// A query of the heap began in a list: no result until it ends.
    pub(crate) fn began(&self, index: u32) {
        self.ended[index as usize].store(0, std::sync::atomic::Ordering::Relaxed);
    }

    /// The query ended as `kind` in a list: the host writes its result.
    pub(crate) fn ended(&self, index: u32, kind: u32) {
        self.ended[index as usize].store(kind as u8 + 1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Whether the query's last run on the host was of type `kind`.
    pub(crate) fn measured(&self, index: u32, kind: u32) -> bool {
        self.ended[index as usize].load(std::sync::atomic::Ordering::Relaxed) == kind as u8 + 1
    }
}

/// The host heap behind title heap `this` for queries of type `kind` at
/// `[first, first + count)`, made on first use: none for types the host
/// does not run, a heap of another type, indices past the heap's end, or
/// when the host refuses the heap.
#[cfg(windows)]
pub(crate) fn host<'a>(this: This, kind: u32, first: u32, count: u32) -> Option<&'a HostHeap> {
    if this.is_null() {
        return None;
    }
    // SAFETY: the title passes one of our heaps, alive while it uses it.
    let heap = unsafe { &*this.cast::<Heap>() };
    let desc = heap.desc;
    if heap_type(kind) != Some(desc.kind) || first.checked_add(count)? > desc.count {
        return None;
    }
    heap.host
        .get_or_init(|| {
            use windows::Win32::Graphics::Direct3D12::*;
            if desc.count > MOST_QUERIES {
                tracing::warn!(?desc, "query heap too big for the host: stand-in results");
                return None;
            }
            let mut made: Option<ID3D12QueryHeap> = None;
            let host_desc = D3D12_QUERY_HEAP_DESC {
                Type: D3D12_QUERY_HEAP_TYPE(desc.kind as i32),
                Count: desc.count,
                NodeMask: 0,
            };
            // SAFETY: plain object creation.
            if let Err(e) = unsafe { crate::host::host()?.device.CreateQueryHeap(&host_desc, &mut made) } {
                tracing::warn!(error = %e, ?desc, "no host query heap: stand-in results");
                return None;
            }
            Some(HostHeap {
                heap: made?,
                ended: (0..desc.count).map(|_| Default::default()).collect(),
            })
        })
        .as_ref()
}

/// Device method.
pub(crate) extern "system" fn create(
    _: This,
    desc: *const Desc,
    iid: *const Iid,
    out: *mut This,
) -> HRESULT {
    // SAFETY: the title passes a description (null is tolerated).
    let Some(&desc) = (unsafe { desc.as_ref() }) else {
        return E_INVALIDARG;
    };
    tracing::debug!(?desc, "query heap");
    let heap = Box::new(Heap {
        object: Object::new(&FUNCTIONS),
        desc,
        #[cfg(windows)]
        host: std::sync::OnceLock::new(),
    });
    object::hand_out(QUERY_HEAP, heap, iid, &HEAP_IIDS, out)
}

extern "system" fn query(this: This, iid: *const Iid, out: *mut This) -> HRESULT {
    object::answer(
        QUERY_HEAP,
        iid,
        object::is(iid, &HEAP_IIDS).then_some(this),
        out,
    )
}
extern "system" fn release(this: This) -> u32 {
    object::release_boxed::<Heap>(this)
}

/// IUnknown 3, object 4, device child 1.
static FUNCTIONS: Table<24> = {
    let mut f = object::d3d12_object(object::missing_table::<QUERY_HEAP, 24>());
    f[0] = query as *const ();
    f[2] = release as *const ();
    f[7] = object::parent_device as *const (); // 0x38
    Table(f)
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{S_OK, slot};

    const BEGIN: usize = 0xe0;
    const END: usize = 0xe8;
    const RESOLVE: usize = 0xf0;
    type Mark = extern "system" fn(This, This, u32, u32);
    type Resolve = extern "system" fn(This, This, u32, u32, u32, This, u64);

    fn release(o: This) -> u32 {
        slot::<extern "system" fn(This) -> u32>(o, 0x10)(o)
    }

    fn heap(kind: u32, count: u32) -> This {
        let mut heap = std::ptr::null();
        let desc = Desc { kind, count, nodes: 0 };
        assert_eq!(create(std::ptr::null(), &desc, &HEAP_IIDS[0], &mut heap), S_OK);
        heap
    }

    /// A title buffer of `width` bytes in title memory, filled with 0xee.
    fn buffer(width: u64) -> (This, *mut u8) {
        // The title's heap properties and resource description.
        #[repr(C)]
        struct Properties([u32; 5]);
        #[repr(C)]
        struct ResourceDesc {
            dimension: u32,
            alignment: u64,
            width: u64,
            height: u32,
            depth_or_array_size: u16,
            mip_levels: u16,
            format: u32,
            sample_count: u32,
            sample_quality: u32,
            layout: u32,
            flags: u32,
        }
        let desc = ResourceDesc {
            dimension: 1,
            alignment: 0,
            width,
            height: 1,
            depth_or_array_size: 1,
            mip_levels: 1,
            format: 0,
            sample_count: 1,
            sample_quality: 0,
            layout: 1,
            flags: 0,
        };
        let iid = guid(
            0x696442be,
            0xa72e,
            0x4059,
            [0xbc, 0x79, 0x5b, 0x5c, 0x98, 0x04, 0x0f, 0xad],
        );
        let properties = Properties([1, 0, 0, 0, 0]);
        let null = std::ptr::null();
        let mut out = null;
        let hr = crate::resource::create_committed(
            null,
            (&raw const properties).cast(),
            0,
            (&raw const desc).cast(),
            0,
            null.cast(),
            &iid,
            &mut out,
        );
        assert_eq!(hr, S_OK);
        let (data, size) = crate::resource::buffer_bytes(out).unwrap();
        assert_eq!(size, width);
        // SAFETY: the buffer holds `width` bytes.
        unsafe { data.write_bytes(0xee, width as usize) };
        (out, data)
    }

    /// A title queue, allocator and list.
    fn list() -> (This, This, This) {
        let null = std::ptr::null();
        let queue_iid = guid(
            0x0ec870a6,
            0x5d7e,
            0x4c22,
            [0x8c, 0xfc, 0x5b, 0xaa, 0xe0, 0x76, 0x16, 0xed],
        );
        let allocator_iid = guid(
            0x6102dee4,
            0xaf59,
            0x4b09,
            [0xb9, 0x99, 0xb4, 0x4d, 0x73, 0xf0, 0x9b, 0x24],
        );
        let list_iid = guid(
            0x5b160d0f,
            0xac1b,
            0x4185,
            [0x8b, 0xa8, 0xb3, 0xae, 0x42, 0xa5, 0xa4, 0x55],
        );
        let (mut queue, mut allocator, mut list) = (null, null, null);
        let queue_desc = [0u32; 4];
        assert_eq!(
            crate::queue::create(null, queue_desc.as_ptr().cast(), &queue_iid, &mut queue),
            S_OK
        );
        assert_eq!(
            crate::list::create_allocator(null, 0, &allocator_iid, &mut allocator),
            S_OK
        );
        assert_eq!(
            crate::list::create_list(null, 0, 0, allocator, null, &list_iid, &mut list),
            S_OK
        );
        (queue, allocator, list)
    }

    /// Closes the list and submits it (run and waited for with a host GPU).
    fn run(queue: This, list: This) {
        assert_eq!(slot::<extern "system" fn(This) -> HRESULT>(list, 0x48)(list), S_OK);
        slot::<extern "system" fn(This, u32, *const This)>(queue, 0x50)(queue, 1, &list);
    }

    fn read(data: *mut u8, index: usize) -> u64 {
        // SAFETY: the tests' buffers hold the words they read.
        unsafe { data.cast::<u64>().add(index).read_unaligned() }
    }

    /// The smallest scaffolding for a host draw the title's list records
    /// around (title pipelines need translated console shaders): a
    /// full-screen triangle into a 64x64 target, recorded straight into
    /// the title list's host list.
    #[cfg(windows)]
    struct Triangle {
        root: windows::Win32::Graphics::Direct3D12::ID3D12RootSignature,
        pso: windows::Win32::Graphics::Direct3D12::ID3D12PipelineState,
        target: windows::Win32::Graphics::Direct3D12::ID3D12Resource,
    }

    #[cfg(windows)]
    impl Triangle {
        const SIZE: u32 = 64;

        fn new() -> Triangle {
            use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
            use windows::Win32::Graphics::Direct3D::*;
            use windows::Win32::Graphics::Direct3D12::*;
            use windows::Win32::Graphics::Dxgi::Common::*;
            use windows::core::s;
            let device = &crate::host::host().unwrap().device;
            let compile = |source: &str, target: windows::core::PCSTR| {
                let mut code: Option<ID3DBlob> = None;
                // SAFETY: the source and names live through the call.
                unsafe {
                    D3DCompile(source.as_ptr().cast(), source.len(), None, None, None, s!("main"), target, 0, 0, &mut code, None)
                        .unwrap();
                }
                code.unwrap()
            };
            let vs = compile(
                concat!(
                    "float4 main(uint id : SV_VertexID) : SV_Position {",
                    " float2 uv = float2((id << 1) & 2, id & 2);",
                    " return float4(uv * float2(2, -2) + float2(-1, 1), 0, 1); }",
                ),
                s!("vs_5_0"),
            );
            let ps = compile("float4 main() : SV_Target { return 1; }", s!("ps_5_0"));
            // SAFETY: the blobs' own bytes.
            let bytes = |b: &ID3DBlob| unsafe {
                D3D12_SHADER_BYTECODE { pShaderBytecode: b.GetBufferPointer(), BytecodeLength: b.GetBufferSize() }
            };
            let mut blob: Option<ID3DBlob> = None;
            // SAFETY: plain serialization and creation; the descriptions
            // point at live data.
            unsafe {
                D3D12SerializeRootSignature(&D3D12_ROOT_SIGNATURE_DESC::default(), D3D_ROOT_SIGNATURE_VERSION_1, &mut blob, None)
                    .unwrap();
                let blob = blob.unwrap();
                let root: ID3D12RootSignature = device
                    .CreateRootSignature(
                        0,
                        std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize()),
                    )
                    .unwrap();
                let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    VS: bytes(&vs),
                    PS: bytes(&ps),
                    SampleMask: u32::MAX,
                    RasterizerState: D3D12_RASTERIZER_DESC {
                        FillMode: D3D12_FILL_MODE_SOLID,
                        CullMode: D3D12_CULL_MODE_NONE,
                        DepthClipEnable: true.into(),
                        ..Default::default()
                    },
                    PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
                    NumRenderTargets: 1,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    ..Default::default()
                };
                desc.BlendState.RenderTarget[0].RenderTargetWriteMask = 0xf;
                desc.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
                let pso = device.CreateGraphicsPipelineState(&desc).unwrap();
                let mut target: Option<ID3D12Resource> = None;
                device
                    .CreateCommittedResource(
                        &D3D12_HEAP_PROPERTIES { Type: D3D12_HEAP_TYPE_DEFAULT, ..Default::default() },
                        D3D12_HEAP_FLAG_NONE,
                        &D3D12_RESOURCE_DESC {
                            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                            Width: u64::from(Self::SIZE),
                            Height: Self::SIZE,
                            DepthOrArraySize: 1,
                            MipLevels: 1,
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                            ..Default::default()
                        },
                        D3D12_RESOURCE_STATE_RENDER_TARGET,
                        None,
                        &mut target,
                    )
                    .unwrap();
                Triangle { root, pso, target: target.unwrap() }
            }
        }

        /// Draws the triangle; `scissor` false cuts it to nothing.
        fn draw(&self, list: This, scissor: bool) {
            use windows::Win32::Foundation::RECT;
            use windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST;
            use windows::Win32::Graphics::Direct3D12::D3D12_VIEWPORT;
            let host = crate::host::host().unwrap();
            let list = crate::list::host_list(list).unwrap();
            let rtv = host.rtv(&self.target, None);
            let size = Self::SIZE as i32;
            // Cut away: a rectangle beside the target (the driver here does not
            // always cut everything for an empty one).
            let rect = if scissor { RECT { left: 0, top: 0, right: size, bottom: size } } else { RECT { left: size, top: size, right: size + 1, bottom: size + 1 } };
            // SAFETY: an open list; our live objects, alive until it ran.
            unsafe {
                let l = &list.list;
                l.OMSetRenderTargets(1, Some(&rtv), false, None);
                l.RSSetViewports(&[D3D12_VIEWPORT { Width: size as f32, Height: size as f32, MaxDepth: 1.0, ..Default::default() }]);
                l.RSSetScissorRects(&[rect]);
                l.SetGraphicsRootSignature(&self.root);
                l.SetPipelineState(&self.pso);
                l.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                l.DrawInstanced(3, 1, 0, 0);
            }
        }
    }

    /// Occlusion, binary occlusion and pipeline statistics queries around
    /// host draws: what the host GPU counted reaches the title's buffer and
    /// the buffer's host copy, at the offsets resolved to.
    #[cfg(windows)]
    #[test]
    fn host_queries_count_what_draws_did() {
        if crate::host::host().is_none() {
            return; // no host GPU here
        }
        let triangle = Triangle::new();
        let (queue, allocator, list) = list();
        let (occlusion, statistics) = (heap(OCCLUSION_HEAP, 8), heap(PIPELINE_STATISTICS_HEAP, 2));
        let (buffer, data) = buffer(512);
        let (begin, end) = (slot::<Mark>(list, BEGIN), slot::<Mark>(list, END));
        let resolve = slot::<Resolve>(list, RESOLVE);
        // Queries 0, 1 see a full-screen triangle; 2, 3 one cut away.
        for (index, kind, scissor) in [
            (0, OCCLUSION, true),
            (1, BINARY_OCCLUSION, true),
            (2, OCCLUSION, false),
            (3, BINARY_OCCLUSION, false),
        ] {
            begin(list, occlusion, kind, index);
            begin(list, occlusion, kind, index); // running already: dropped
            triangle.draw(list, scissor);
            end(list, occlusion, kind, index);
        }
        begin(list, statistics, PIPELINE_STATISTICS, 1);
        triangle.draw(list, true);
        end(list, statistics, PIPELINE_STATISTICS, 1);
        // Left running: the host list ends it at close.
        begin(list, occlusion, OCCLUSION, 7);
        // Results at 8-byte steps from 16, statistics at 128.
        for (index, kind) in [(0, OCCLUSION), (1, BINARY_OCCLUSION), (2, OCCLUSION), (3, BINARY_OCCLUSION)] {
            resolve(list, occlusion, kind, index, 1, buffer, 16 + 8 * u64::from(index));
        }
        resolve(list, statistics, PIPELINE_STATISTICS, 1, 1, buffer, 128);
        run(queue, list);

        let pixels = u64::from(Triangle::SIZE * Triangle::SIZE);
        assert_eq!(read(data, 2), pixels, "samples passed");
        assert_eq!(read(data, 3), 1, "visible");
        assert_eq!(read(data, 4), 0, "no samples passed");
        assert_eq!(read(data, 5), 0, "not visible");
        // `D3D12_QUERY_DATA_PIPELINE_STATISTICS`: input vertices and
        // primitives, vertex shader runs, ..., pixel shader runs.
        let stats: Vec<u64> = (16..27).map(|i| read(data, i)).collect();
        assert_eq!((stats[0], stats[1]), (3, 1), "{stats:?}");
        assert!(stats[2] >= 3 && stats[7] >= pixels, "{stats:?}");
        // Around the results the title's bytes stay.
        for word in [0, 1, 6, 15, 27, 63] {
            assert_eq!(read(data, word), 0xeeee_eeee_eeee_eeee, "word {word}");
        }
        // The host copy holds the results too, for later GPU commands (it is
        // up to date only where something reads it).
        let shadow = crate::shadow::find(data as u64, 512).unwrap();
        for (from, to) in [(16, 48), (128, 216)] {
            // SAFETY: inside the buffer's 512 bytes.
            let title = unsafe { std::slice::from_raw_parts(data.add(from), to - from) };
            assert_eq!(shadow.host_bytes(data as u64 + from as u64, to - from), title);
        }
        for object in [buffer, statistics, occlusion, list, allocator, queue] {
            release(object);
        }
    }

    /// A title draw the host does not run (here: no pipeline bound) inside
    /// an occlusion query leaves it uncounted, so the query keeps its
    /// stand-in result instead of hiding what the draw would have shown.
    /// Statistics keep what the host counted.
    #[cfg(windows)]
    #[test]
    fn occlusion_queries_around_draws_not_run_stay_visible() {
        if crate::host::host().is_none() {
            return; // no host GPU here
        }
        let triangle = Triangle::new();
        let (queue, allocator, list) = list();
        let (occlusion, statistics) = (heap(OCCLUSION_HEAP, 4), heap(PIPELINE_STATISTICS_HEAP, 1));
        let (buffer, data) = buffer(256);
        let (begin, end) = (slot::<Mark>(list, BEGIN), slot::<Mark>(list, END));
        let resolve = slot::<Resolve>(list, RESOLVE);
        // `DrawInstanced` with nothing bound: recorded, not run.
        let title_draw = || slot::<extern "system" fn(This, u32, u32, u32, u32)>(list, 0x60)(list, 3, 1, 0, 0);
        // 0, 1: a cut-away host draw and a title draw not run. 2: the
        // cut-away draw alone, begun after the title draw.
        for (index, kind, missed) in [(0, OCCLUSION, true), (1, BINARY_OCCLUSION, true), (2, OCCLUSION, false)] {
            begin(list, occlusion, kind, index);
            triangle.draw(list, false);
            if missed {
                title_draw();
            }
            end(list, occlusion, kind, index);
        }
        begin(list, statistics, PIPELINE_STATISTICS, 0);
        triangle.draw(list, true);
        title_draw();
        end(list, statistics, PIPELINE_STATISTICS, 0);
        resolve(list, occlusion, OCCLUSION, 0, 1, buffer, 0);
        resolve(list, occlusion, BINARY_OCCLUSION, 1, 1, buffer, 8);
        resolve(list, occlusion, OCCLUSION, 2, 1, buffer, 16);
        resolve(list, statistics, PIPELINE_STATISTICS, 0, 1, buffer, 64);
        run(queue, list);

        assert_eq!(read(data, 0), 1 << 20, "stand-in samples");
        assert_eq!(read(data, 1), 1, "stand-in visible");
        assert_eq!(read(data, 2), 0, "measured: nothing passed");
        assert_eq!((read(data, 8), read(data, 9)), (3, 1), "measured statistics");
        for object in [buffer, statistics, occlusion, list, allocator, queue] {
            release(object);
        }
    }

    /// Host timestamps come out on the title's GPU clock, in order, within
    /// the submission.
    #[cfg(windows)]
    #[test]
    fn host_timestamps_are_on_the_title_clock() {
        if crate::host::host().is_none() {
            return; // no host GPU here
        }
        let triangle = Triangle::new();
        let (queue, allocator, list) = list();
        let stamps = heap(TIMESTAMP_HEAP, 4);
        let (buffer, data) = buffer(64);
        let (begin, end) = (slot::<Mark>(list, BEGIN), slot::<Mark>(list, END));
        begin(list, stamps, TIMESTAMP, 0); // timestamps only end: dropped
        end(list, stamps, TIMESTAMP, 0);
        for _ in 0..64 {
            triangle.draw(list, true);
        }
        end(list, stamps, TIMESTAMP, 1);
        slot::<Resolve>(list, RESOLVE)(list, stamps, TIMESTAMP, 0, 2, buffer, 8);
        let before = crate::queue::gpu_clock();
        run(queue, list);
        let after = crate::queue::gpu_clock();
        let (first, second) = (read(data, 1), read(data, 2));
        // The calibration is good to microseconds: 1 ms of slack.
        let slack = crate::queue::TIMESTAMP_FREQUENCY / 1000;
        assert!(before.saturating_sub(slack) <= first, "{before} {first}");
        assert!(first <= second && second <= after, "{first} {second} {after}");
        assert_eq!(read(data, 0), 0xeeee_eeee_eeee_eeee);
        for object in [buffer, stamps, list, allocator, queue] {
            release(object);
        }
    }

    /// Queries that never ran on the host, of a heap of another type, past
    /// the heap's end or of types the host lacks keep the stand-in values;
    /// resolves past the buffer write nothing. The same with or without a
    /// host GPU.
    #[test]
    fn queries_not_run_keep_their_stand_in_values() {
        let (queue, allocator, list) = list();
        let occlusion = heap(OCCLUSION_HEAP, 4);
        let (buffer, data) = buffer(256);
        let resolve = slot::<Resolve>(list, RESOLVE);
        // An end without its begin runs nothing.
        slot::<Mark>(list, END)(list, occlusion, OCCLUSION, 3);
        resolve(list, occlusion, OCCLUSION, 2, 2, buffer, 0);
        resolve(list, occlusion, BINARY_OCCLUSION, 0, 1, buffer, 16);
        resolve(list, occlusion, PIPELINE_STATISTICS, 0, 1, buffer, 32); // heap of another type
        resolve(list, occlusion, OCCLUSION, 3, 2, buffer, 120); // past the heap's end
        resolve(list, occlusion, 9, 0, 1, buffer, 136); // a type the host lacks: 16 bytes
        // Past the buffer's end, and an offset that would wrap.
        resolve(list, occlusion, OCCLUSION, 0, 2, buffer, 248);
        resolve(list, occlusion, OCCLUSION, 0, 2, buffer, u64::MAX - 8);
        run(queue, list);
        assert_eq!((read(data, 0), read(data, 1)), (1 << 20, 1 << 20));
        assert_eq!(read(data, 2), 1);
        assert!((4..15).all(|i| read(data, i) == 0));
        assert_eq!((read(data, 15), read(data, 16)), (1 << 20, 1 << 20));
        assert_eq!((read(data, 17), read(data, 18)), (0, 0));
        for word in [3, 19, 30, 31] {
            assert_eq!(read(data, word), 0xeeee_eeee_eeee_eeee, "word {word}");
        }
        for object in [buffer, occlusion, list, allocator, queue] {
            release(object);
        }
    }
}
