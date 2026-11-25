// Dispatch the original grid shader over sparse cells and overlapping reservations.
#include "../compute_buffers.h"
#include "lightgrid_blobs.h"
#include <vector>
using namespace compute_buffers;

struct Scenario {
    const char* name;
    UINT lights, captures, per_cell;
    bool first_small;
    bool multi = false;
};

static void run(FILE* out, const Scenario& scenario) {
    ComPtr<ID3D12Device> dev;
    check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&dev)), "create device");
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "create DXGI");
    check(factory->EnumAdapterByLuid(dev->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "enumerate adapter");
    check(adapter->GetDesc1(&gpu), "adapter");
    std::fprintf(out, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    D3D12_FEATURE_DATA_D3D12_OPTIONS1 wave{};
    check(dev->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS1, &wave, sizeof(wave)), "query wave size");
    std::fprintf(out, "wave_min=%u\nwave_max=%u\n", wave.WaveLaneCountMin, wave.WaveLaneCountMax);
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, cooked_shader, sizeof(cooked_shader), IID_PPV_ARGS(&root)),
          "create original root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.CS = {desktop_shader, sizeof(desktop_shader)};
    ComPtr<ID3D12PipelineState> pso;
    check(dev->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pso)), "create light grid PSO");
    const auto upload_heap = [&](UINT size) {
        return buffer(dev.Get(), size, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    };
    std::vector<UINT> b0(1024), b1(16384), b2(64);
    std::array<float, 4> sphere{0, 0, 0, 1000000}, direction{};
    constexpr UINT one = 0x3f800000, first = 5, sentinel = 0xdeadbeef;
    const UINT cells = scenario.multi ? 48u : 24u;
    // Finite screen/depth transforms, one local sphere and one capture volume.
    for (UINT i = 0; i < 4; ++i) {
        b0[4 * (36 + i) + i] = one;
        b0[4 * (12 + i) + i] = one;
    }
    b0[4 * 31 + 2] = one;
    b0[4 * 31 + 3] = one;
    b0[4 * 30 + 2] = one;
    b0[4 * 65 + 2] = one;
    b0[4 * 65 + 3] = one;
    b0[4 * 134 + 2] = one;
    b0[4 * 134 + 3] = one;
    for (UINT i = 0; i < scenario.captures; ++i)
        std::memcpy(&b1[4 * i + 3], &sphere[3], 4);
    b2[0] = scenario.lights;
    b2[1] = scenario.captures;
    b2[3] = cells;
    b2[4] = scenario.multi ? 6u : 3u;
    b2[5] = 2;
    b2[6] = 4;
    b2[7] = scenario.per_cell;
    b2[12] = one;
    b2[14] = one;
    auto params0 = upload_heap(4096), params1 = upload_heap(65536), params2 = upload_heap(256);
    upload(params0.Get(), b0.data(), 4096);
    upload(params1.Get(), b1.data(), 65536);
    upload(params2.Get(), b2.data(), 256);
    std::array<float, 4> small_volume{4.0f, 0.0f, 6.0f, 0.25f};
    std::array<std::array<float, 4>, 2> volumes{scenario.first_small ? small_volume : sphere, small_volume};
    std::array<std::array<float, 4>, 2> cones{direction, direction};
    auto lights = upload_heap(sizeof(volumes)), directions = upload_heap(sizeof(cones));
    upload(lights.Get(), volumes.data(), sizeof(volumes));
    upload(directions.Get(), cones.data(), sizeof(cones));
    constexpr UINT words = 1024, bytes = words * 4, heads = 16, links = 192;
    std::vector<UINT> initial(words, sentinel);
    initial[0] = first;
    for (UINT i = 0; i < cells * 2; ++i)
        initial[heads + i] = 0xcc000000u | i;
    auto seed = upload_heap(bytes);
    upload(seed.Get(), initial.data(), bytes);
    auto dst = buffer(dev.Get(), bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto read = buffer(dev.Get(), bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                  0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "create views");
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    auto cpu = [&](UINT i) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        return {heap->GetCPUDescriptorHandleForHeapStart().ptr + i * step};
    };
    auto handle = [&](UINT i) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        return {heap->GetGPUDescriptorHandleForHeapStart().ptr + i * step};
    };
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.Format = DXGI_FORMAT_R32G32B32A32_FLOAT;
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Buffer.NumElements = 2;
    dev->CreateShaderResourceView(lights.Get(), &srv, cpu(0));
    dev->CreateShaderResourceView(directions.Get(), &srv, cpu(1));
    const D3D12_CONSTANT_BUFFER_VIEW_DESC cbvs[] = {{params0->GetGPUVirtualAddress(), 4096},
                                                    {params1->GetGPUVirtualAddress(), 65536},
                                                    {params2->GetGPUVirtualAddress(), 256}};
    for (UINT i = 0; i < 3; ++i)
        dev->CreateConstantBufferView(&cbvs[i], cpu(64 + i));
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.Format = DXGI_FORMAT_R32_UINT;
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    const UINT starts[] = {0, heads, links}, counts[] = {1, 2 * cells, words - links};
    for (UINT i = 0; i < 3; ++i) {
        uav.Buffer.FirstElement = starts[i];
        uav.Buffer.NumElements = counts[i];
        dev->CreateUnorderedAccessView(dst.Get(), nullptr, &uav, cpu(80 + i));
    }
    D3D12_COMMAND_QUEUE_DESC q{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "create queue");
    check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "create allocator");
    check(dev->CreateCommandList(0, q.Type, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)), "create list");
    list->CopyBufferRegion(dst.Get(), 0, seed.Get(), 0, bytes);
    transition(list.Get(), dst.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    list->SetDescriptorHeaps(1, heap.GetAddressOf());
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, handle(0));
    list->SetComputeRootDescriptorTable(1, handle(64));
    list->SetComputeRootDescriptorTable(3, handle(80));
    list->Dispatch(2, 2, 2);
    transition(list.Get(), dst.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(read.Get(), 0, dst.Get(), 0, bytes);
    submit_and_wait(dev.Get(), queue.Get(), list.Get());
    void* mapped{};
    D3D12_RANGE full{0, bytes}, empty{};
    check(read->Map(0, &full, &mapped), "map readback");
    auto values = static_cast<const UINT*>(mapped);
    auto expected = initial;
    UINT allocated = first;
    for (UINT volume = 0; volume < scenario.lights + scenario.captures; ++volume) {
        bool capture = volume >= scenario.lights;
        std::array<UINT, 2> previous{};
        UINT index = capture ? volume - scenario.lights : volume;
        for (UINT cell = 0; cell < cells; ++cell) {
            // The small local light overlaps only cell (2,0,2), logical wave lane 34.
            if (!capture && (index != 0 || scenario.first_small) && cell != 14)
                continue;
            const UINT head_index = (capture ? cells : 0) + cell;
            const UINT link = scenario.multi ? values[heads + head_index] : allocated;
            ++allocated;
            if (scenario.multi && (link < first || link >= first + 2 * cells || expected[links + 2 * link] != sentinel))
                throw std::runtime_error("shared allocator returned duplicate or out of bounds node");
            if (scenario.multi) {
                // Different workgroups may reserve in either order, but each
                // reservation assigns consecutive nodes in Xbox lane order.
                UINT& last = previous[(cell % 6u) / 4u];
                if (last && link != last + 1)
                    throw std::runtime_error("light grid reordered lanes within a workgroup");
                last = link;
            }
            if (link >= 2 * cells * scenario.per_cell)
                continue;
            UINT head = heads + head_index;
            expected[links + 2 * link] = index;
            expected[links + 2 * link + 1] = expected[head];
            expected[head] = link;
        }
    }
    expected[0] = allocated;
    UINT matched = 0;
    for (UINT i = 0; i < words; ++i) {
        matched += expected[i] == values[i];
        if (expected[i] != values[i] && i < 25)
            std::fprintf(out, "mismatch_%s_%u=%08x/%08x\n", scenario.name, i, values[i], expected[i]);
    }
    std::fprintf(out, "%s_counter=%u\n%s_matched_words=%u\n", scenario.name, values[0],
                 scenario.name, matched);
    read->Unmap(0, &empty);
    if (matched != words)
        throw std::runtime_error("light grid mismatch");
}
int main() {
    FILE* out{};
    if (fopen_s(&out, "readback.txt", "wb"))
        return 2;
    int status = 0;
    try {
        run(out, {"two_productive_groups", 1, 1, 4, false, true});
        run(out, {"broad", 1, 1, 4, false});
        run(out, {"small", 1, 1, 4, true});
        run(out, {"chained", 2, 2, 4, false});
        run(out, {"limited", 2, 2, 1, false});
        run(out, {"captures", 0, 2, 4, false});
        run(out, {"lights", 2, 0, 4, false});
        run(out, {"empty", 0, 0, 4, false});
    } catch (const std::exception& e) {
        std::fprintf(out, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(out) ? 2 : status;
}
