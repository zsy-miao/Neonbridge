//! A frame's worth of one-record indirect draws on the host GPU, clamped one
//! at a time and in windows, recorded as `main` does (the scratch buffer
//! back to the common state after every execution, the clamp charging the
//! budget record by record) and as this tree does (readable until the next
//! clamp, one charge per group). Ignored by default; run it with
//! `cargo test -p victory-gpu --release --lib indirect::bench -- --ignored --nocapture`.
//! It needs no title files: the draws are tiny triangles of ours.

use super::*;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_R8G8B8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory2, DXGI_CREATE_FACTORY_FLAGS, IDXGIFactory4,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, WaitForSingleObject};
use windows::core::{Interface, PCSTR};

/// Draws a frame (the title's gameplay frames hold about 3,200 indirect
/// executions, #107), records a window holds, and how far apart the
/// records are (the title keeps them 32 bytes apart).
const DRAWS: u32 = 3200;
const WINDOW: u32 = 32;
const STEP: u32 = 32;
const RUNS: usize = 9;

const SHADERS: &str = r#"
#define RS "RootFlags(0)"
[RootSignature(RS)]
float4 vs(uint v : SV_VertexID, uint i : SV_InstanceID) : SV_Position {
float2 p = float2((v & 1) ? 0.05 : 0.0, (v & 2) ? 0.05 : 0.0) - 0.5;
return float4(p, 0.5, 1);
}
float4 ps() : SV_Target { return float4(1, 0.5, 0.25, 1); }
"#;

fn compile(entry: &str, target: &str) -> Vec<u8> {
    let entry = std::ffi::CString::new(entry).unwrap();
    let target = std::ffi::CString::new(target).unwrap();
    let mut code: Option<ID3DBlob> = None;
    // SAFETY: the source and names outlive the call.
    unsafe {
        D3DCompile(
            SHADERS.as_ptr().cast(),
            SHADERS.len(),
            s!("bench"),
            None,
            None,
            PCSTR(entry.as_ptr().cast()),
            PCSTR(target.as_ptr().cast()),
            0,
            0,
            &mut code,
            None,
        )
        .unwrap();
        let code = code.unwrap();
        std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize())
            .to_vec()
    }
}

/// The clamp as it was before it charged the budget once per group
/// (record by record, every thread racing for the budget word), kept
/// here to measure against.
const OLD_SOURCE: &str = r#"
#define RS "RootConstants(num32BitConstants=6, b0), SRV(t0), SRV(t1), UAV(u0), UAV(u1)"
cbuffer Record : register(b0) {
uint kind;
uint stride;
uint records;
uint args_offset;
// Bit 0: a count buffer bounds the records. Bit 1: the records are a
// window clamped ahead of their draws: one that asks for more than can
// run is left empty and costs nothing.
uint has_count;
uint count_offset;
};
ByteAddressBuffer src_args : register(t0);
ByteAddressBuffer count_buf : register(t1);
RWByteAddressBuffer dst_args : register(u0);
// The list's work so far (vertex instances, threads): past the budget,
// records run empty.
RWByteAddressBuffer budget : register(u1);
static const uint BUDGET = 1u << 27;
// Groups of one dispatch; more run empty.
static const uint GROUPS = 1u << 20;
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID) {
uint eff_count = records;
bool ahead = (has_count & 2u) != 0;
if ((has_count & 1u) != 0) {
    eff_count = min(eff_count, count_buf.Load(count_offset));
}
if (id.x >= eff_count) return;
uint src_at = args_offset + id.x * stride;
uint dst_at = id.x * stride;
uint cost;
uint2 draw_counts = uint2(0, 0);
uint3 groups = uint3(0, 0, 0);
if (kind <= 1) {
    // Draws: vertex or index count, then instances.
    uint2 asked = src_args.Load2(src_at);
    if (ahead && (asked.x > (1u << 24) || asked.y > (1u << 24))) {
        asked = uint2(0, 0);
    }
    uint count = min(asked.x, 1u << 24);
    uint instances = min(asked.y, (1u << 26) / max(count, 1u));
    cost = count * instances;
    draw_counts = uint2(count, instances);
} else if (kind == 2) {
    uint3 g = min(src_args.Load3(src_at), uint3(65535u, 65535u, 65535u));
    // x * y fits 32 bits; the whole count may not.
    if (g.z != 0 && g.x * g.y > GROUPS / g.z) {
        g = uint3(0, 0, 0);
    }
    cost = g.x * g.y * g.z * 64u;
    groups = g;
} else {
    // Mesh dispatches: the console's are one-dimensional.
    uint3 g = src_args.Load3(src_at);
    // Ahead of its draw, a record over the most a dispatch runs is a
    // slot the title has not filled (the title leaves all
    // ones there): empty, and not counted below either.
    if (ahead && g.x > 65535u) {
        g = uint3(0, 0, 0);
    }
    // Diagnostics, in the budget buffer's next words: the largest
    // group count a mesh record asked for, and the groups asked in all.
    uint was;
    budget.InterlockedMax(4, g.x, was);
    budget.InterlockedAdd(8, min(g.x, 1u << 24), was);
    g = uint3(min(g.x, 65535u), min(g.y, 1u), min(g.z, 1u));
    cost = g.x * g.y * g.z * 128u;
    groups = g;
}
bool over = false;
if (cost > 0) {
    uint before;
    budget.InterlockedOr(0, 0, before);
    [allow_uav_condition]
    while (before < BUDGET) {
        uint next = (BUDGET - before <= cost) ? BUDGET : (before + cost);
        uint prev;
        budget.InterlockedCompareExchange(0, before, next, prev);
        if (prev == before) break;
        before = prev;
    }
    over = before >= BUDGET || BUDGET - before < cost;
}
if (kind <= 1) {
    if (over) {
        draw_counts = uint2(0, 0);
    }
    dst_args.Store2(dst_at, draw_counts);
    for (uint off = 8; off < stride; off += 4) {
        dst_args.Store(dst_at + off, src_args.Load(src_at + off));
    }
} else {
    if (over) {
        groups = uint3(0, 0, 0);
    }
    dst_args.Store3(dst_at, groups);
    for (uint off = 12; off < stride; off += 4) {
        dst_args.Store(dst_at + off, src_args.Load(src_at + off));
    }
}
}
"#;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Barriers {
    /// Back to the common state after every execution (as before).
    Old,
    /// Readable as arguments until the next clamp, common at the end.
    New,
}

#[test]
#[ignore = "benchmark; see the module notes"]
fn bench_indirect_draw_barriers() {
    let Some(host) = super::host() else {
        eprintln!("no host GPU");
        return;
    };
    let device = &host.device;
    let clamps = [
        make_from(device, OLD_SOURCE).expect("the old clamp"),
        make_for(device).expect("the clamp"),
    ];
    // SAFETY: plain D3D12 object creation with valid descriptions; the
    // objects outlive every list recorded and run below.
    unsafe {
        let adapter = {
            let luid = device.GetAdapterLuid();
            CreateDXGIFactory2::<IDXGIFactory4>(DXGI_CREATE_FACTORY_FLAGS(0))
                .and_then(|f| {
                    f.EnumAdapterByLuid::<windows::Win32::Graphics::Dxgi::IDXGIAdapter1>(luid)
                })
                .and_then(|a| a.GetDesc1())
                .map(|d| {
                    String::from_utf16_lossy(&d.Description)
                        .trim_end_matches('\0')
                        .to_owned()
                })
                .unwrap_or_else(|_| "an unnamed adapter".to_owned())
        };
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap();
        let allocator: ID3D12CommandAllocator = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .unwrap();
        let list: ID3D12GraphicsCommandList = device
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
            .unwrap();
        list.Close().unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event = CreateEventW(None, false, false, None).unwrap();
        let frequency = queue.GetTimestampFrequency().unwrap() as f64;

        // The pipeline: tiny triangles into a small target.
        let (vs, ps) = (compile("vs", "vs_5_1"), compile("ps", "ps_5_1"));
        let root: ID3D12RootSignature = device.CreateRootSignature(0, &vs).unwrap();
        let mut desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            VS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: vs.as_ptr().cast(),
                BytecodeLength: vs.len(),
            },
            PS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: ps.as_ptr().cast(),
                BytecodeLength: ps.len(),
            },
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
        desc.BlendState.RenderTarget[0].RenderTargetWriteMask =
            D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
        desc.RTVFormats[0] = DXGI_FORMAT_R8G8B8A8_UNORM;
        let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc).unwrap();
        let target_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: 256,
            Height: 256,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
            ..Default::default()
        };
        let mut target: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &target_desc,
                D3D12_RESOURCE_STATE_RENDER_TARGET,
                None,
                &mut target,
            )
            .unwrap();
        let target = target.unwrap();
        let rtvs: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: 1,
                ..Default::default()
            })
            .unwrap();
        let rtv = rtvs.GetCPUDescriptorHandleForHeapStart();
        device.CreateRenderTargetView(&target, None, rtv);

        // The title's records: three vertices, one instance, 32 bytes apart.
        let (src, src_ptr) = host.shared_buffer(u64::from(DRAWS * STEP)).unwrap();
        for i in 0..DRAWS as usize {
            let words = [3u32, 1, 0, 0];
            std::ptr::copy_nonoverlapping(
                words.as_ptr().cast::<u8>(),
                src_ptr.add(i * STEP as usize),
                16,
            );
        }
        let (budget, budget_ptr) = host.shared_buffer(256).unwrap();
        let budget_gpu = budget.GetGPUVirtualAddress();
        // The same words in the card's own memory (zeroed when made; the
        // runs below charge far less than the budget).
        let card_budget = host.uav_buffer(256).unwrap();
        let card_budget_gpu = card_budget.GetGPUVirtualAddress();
        let scratch = host
            .uav_buffer(u64::from(WINDOW * STEP).max(64 << 10))
            .unwrap();
        let signature = {
            let arg = D3D12_INDIRECT_ARGUMENT_DESC {
                Type: D3D12_INDIRECT_ARGUMENT_TYPE_DRAW,
                ..Default::default()
            };
            let desc = D3D12_COMMAND_SIGNATURE_DESC {
                ByteStride: 16,
                NumArgumentDescs: 1,
                pArgumentDescs: &arg,
                NodeMask: 0,
            };
            let mut sig: Option<ID3D12CommandSignature> = None;
            device
                .CreateCommandSignature(&desc, None::<&ID3D12RootSignature>, &mut sig)
                .unwrap();
            sig.unwrap()
        };
        let stamps: ID3D12QueryHeap = {
            let mut heap: Option<ID3D12QueryHeap> = None;
            device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC {
                        Type: D3D12_QUERY_HEAP_TYPE_TIMESTAMP,
                        Count: 2,
                        NodeMask: 0,
                    },
                    &mut heap,
                )
                .unwrap();
            heap.unwrap()
        };
        let mut back: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_READBACK,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                    Width: 16,
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
                None,
                &mut back,
            )
            .unwrap();
        let back = back.unwrap();
        let mut submitted = 0u64;

        // One frame's lists: GPU ms (timestamps around the list), CPU ms
        // recording it, ms from submission to the end of its run.
        let mut frame = |barriers: Barriers, clamp: &Clamp, window: u32, card: bool| -> [f64; 3] {
            allocator.Reset().unwrap();
            list.Reset(&allocator, None).unwrap();
            let recording = std::time::Instant::now();
            list.EndQuery(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 0);
            list.OMSetRenderTargets(1, Some(&rtv), false, None);
            list.RSSetViewports(&[D3D12_VIEWPORT {
                Width: 256.0,
                Height: 256.0,
                MaxDepth: 1.0,
                ..Default::default()
            }]);
            list.RSSetScissorRects(&[RECT {
                left: 0,
                top: 0,
                right: 256,
                bottom: 256,
            }]);
            list.IASetPrimitiveTopology(
                windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
            );
            list.SetGraphicsRootSignature(&root);
            let mut state = D3D12_RESOURCE_STATE_COMMON;
            for first in (0..DRAWS).step_by(window as usize) {
                let records = window.min(DRAWS - first);
                assert!(record_clamp_as(
                    clamp,
                    &list,
                    (&scratch, state),
                    if card { card_budget_gpu } else { budget_gpu },
                    (&src, u64::from(first * STEP)),
                    None,
                    (0, STEP, records),
                    window > 1,
                ));
                state = D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT;
                list.SetPipelineState(&pso);
                for k in 0..records {
                    list.ExecuteIndirect(&signature, 1, &scratch, u64::from(k * STEP), None, 0);
                    if barriers == Barriers::Old {
                        list.ResourceBarrier(&[transition(
                            &scratch,
                            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                            D3D12_RESOURCE_STATE_COMMON,
                        )]);
                        state = D3D12_RESOURCE_STATE_COMMON;
                    }
                }
            }
            if state != D3D12_RESOURCE_STATE_COMMON {
                list.ResourceBarrier(&[transition(&scratch, state, D3D12_RESOURCE_STATE_COMMON)]);
            }
            list.EndQuery(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 1);
            list.ResolveQueryData(&stamps, D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, &back, 0);
            list.Close().unwrap();
            let recorded = recording.elapsed().as_secs_f64() * 1e3;
            let running = std::time::Instant::now();
            queue.ExecuteCommandLists(&[Some(list.cast::<ID3D12CommandList>().unwrap())]);
            submitted += 1;
            queue.Signal(&fence, submitted).unwrap();
            fence.SetEventOnCompletion(submitted, event).unwrap();
            WaitForSingleObject(event, INFINITE);
            let ran = running.elapsed().as_secs_f64() * 1e3;
            let mut data = std::ptr::null_mut();
            back.Map(0, None, Some(&mut data)).unwrap();
            let t = std::slice::from_raw_parts(data.cast::<u64>(), 2);
            let gpu = t[1].saturating_sub(t[0]) as f64 / frequency * 1e3;
            back.Unmap(0, None);
            (budget_ptr as *mut u32).write_volatile(0);
            [gpu, recorded, ran]
        };

        // What `main` records, then this branch; the windows a step at a
        // time (barriers, then the clamp's one charge per group).
        let (old, new) = (&clamps[0], &clamps[1]);
        // The last two keep the budget in the card's memory instead of
        // memory the CPU maps (where the list's budget is now).
        let cases: [(&str, Barriers, &Clamp, u32, bool); 7] = [
            ("one at a time, main", Barriers::Old, old, 1, false),
            ("one at a time, this", Barriers::New, new, 1, false),
            ("windows of 32, main", Barriers::Old, old, WINDOW, false),
            ("windows, new barriers", Barriers::New, old, WINDOW, false),
            ("windows of 32, this", Barriers::New, new, WINDOW, false),
            ("one at a time, card", Barriers::New, new, 1, true),
            ("windows of 32, card", Barriers::New, new, WINDOW, true),
        ];
        // Warm up, then the cases in turn, so drift falls on all alike.
        for &(_, barriers, clamp, window, card) in &cases {
            frame(barriers, clamp, window, card);
        }
        let mut times: Vec<Vec<[f64; 3]>> = vec![Vec::new(); cases.len()];
        for _ in 0..RUNS {
            for (i, &(_, barriers, clamp, window, card)) in cases.iter().enumerate() {
                times[i].push(frame(barriers, clamp, window, card));
            }
        }
        let median = |v: &[[f64; 3]], k: usize| {
            let mut x: Vec<f64> = v.iter().map(|t| t[k]).collect();
            x.sort_by(f64::total_cmp);
            x[x.len() / 2]
        };
        eprintln!(
            "indirect draw barriers on {adapter}: {DRAWS} one-record draws a frame, \
             medians of {RUNS} frames"
        );
        for (i, (name, ..)) in cases.iter().enumerate() {
            let gpu = median(&times[i], 0);
            eprintln!(
                "  {name:<22} GPU {gpu:7.2} ms ({:5.2} us a draw), recording {:6.2} ms, \
                 submit to done {:7.2} ms",
                gpu * 1e3 / f64::from(DRAWS),
                median(&times[i], 1),
                median(&times[i], 2),
            );
        }
        for (from, to) in [(0, 1), (2, 3), (3, 4), (2, 4), (1, 5), (4, 6)] {
            let (a, b) = (median(&times[from], 0), median(&times[to], 0));
            eprintln!(
                "  {} -> {}: GPU time {:+.1} %",
                cases[from].0,
                cases[to].0,
                (b / a - 1.0) * 100.0
            );
        }
    }
}
