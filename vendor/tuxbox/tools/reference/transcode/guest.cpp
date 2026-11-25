// Exercise TranscodePageToGPU's segment copies and shared-word triangle updates.
#include "../compute_buffers.h"
#include "transcode_blobs.h"
#include <algorithm>
#include <vector>
using namespace compute_buffers;

struct Page {
    UINT source_byte, destination_byte, segments;
};
constexpr Page pages[] = {{256, 128, 161}, {4096, 4096, 17}};

static void run(FILE* receipt) {
    ComPtr<ID3D12Device> dev;
    check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&dev)), "D3D12CreateDevice");
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    check(factory->EnumAdapterByLuid(dev->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "EnumAdapterByLuid");
    check(adapter->GetDesc1(&gpu), "GetDesc1");
    std::fprintf(receipt, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, cooked_shader, sizeof(cooked_shader), IID_PPV_ARGS(&root)), "root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {desktop_shader, sizeof(desktop_shader)};
    ComPtr<ID3D12PipelineState> pso;
    check(dev->CreateComputePipelineState(&desc, IID_PPV_ARGS(&pso)), "create transcode PSO");

    constexpr UINT bytes = 8192;
    auto up = [&](UINT size) {
        return buffer(dev.Get(), size, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    };
    auto page_table = up(256), dependencies = up(256), cb = up(256), source = up(bytes), seed = up(bytes);
    auto result = buffer(dev.Get(), bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(dev.Get(), bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    std::vector<UINT> inputs(bytes / 4), expected(bytes / 4, 0xdeadbeef);
    std::array<UINT, 64> table{}, constants{}, unused{};
    constants[0] = 1; // StartInstallIndex: select table entries 1 and 2 via GroupID.y.
    upload(seed.Get(), expected.data(), bytes);
    UINT copied = 0;
    for (UINT i = 0; i < _countof(pages); ++i) {
        auto page = pages[i];
        table[4 * (i + 1)] = page.source_byte;
        table[4 * (i + 1) + 1] = page.destination_byte;
        inputs[page.source_byte / 4 + 1] = 0; // no clusters; isolate prefix-copy branch before cluster reconstruction.
        inputs[page.source_byte / 4 + 2] = page.segments;
        for (UINT k = 0; k < 4 * page.segments; ++k) {
            UINT value = (i + 1) * 0x9e3779b9u + k * 0x7f4a7c15u;
            inputs[page.source_byte / 4 + 8 + k] = value;
            expected[page.destination_byte / 4 + k] = value;
            ++copied;
        }
    }
    // One cluster with a single new strip triangle (indices 0, 1, 2), no vertices or references.
    // Native code 0x0564..0x065c packs the base index, then the two five-bit deltas into the UAV.
    constexpr UINT third_source = 6144, third_dest = 5632, triangles_byte = 256;
    table[12] = third_source;
    table[13] = third_dest;
    inputs[(third_source + 4) / 4] = 1;                          // cluster count
    inputs[(third_source + 24) / 4] = 256;                       // strip mask offset
    inputs[(third_source + 84) / 4] = (triangles_byte << 8) | 1; // packed destination and triangle count
    inputs[(third_source + 256) / 4] = 1;                        // lane-zero strip start; no back references
    const UINT atomic_word = (third_dest + triangles_byte) / 4;
    expected[atomic_word] = (0xdeadbeef & ~0x3ffu) | (2u << 5) | 1u;
    // Four independent strip starts share two destination words. Triangle 2 straddles their boundary.
    constexpr UINT fourth_source = 6800, fourth_dest = 6260;
    table[16] = fourth_source;
    table[17] = fourth_dest;
    inputs[(fourth_source + 4) / 4] = 1;
    inputs[(fourth_source + 24) / 4] = 256;
    inputs[(fourth_source + 84) / 4] = (triangles_byte << 8) | 4;
    inputs[(fourth_source + 108) / 4] = 4; // 4 base-index bits plus 5 bits per delta.
    inputs[(fourth_source + 256) / 4] = 15;
    const UINT concurrent_word = (fourth_dest + triangles_byte) / 4;
    UINT64 packed = 0;
    for (UINT i = 0; i < 4; ++i)
        packed |= UINT64(3 * i | 1u << 4 | 2u << 9) << (14 * i);
    expected[concurrent_word] = UINT(packed);
    expected[concurrent_word + 1] = (0xdeadbeefu & 0xff000000u) | UINT(packed >> 32);
    upload(page_table.Get(), table.data(), sizeof(table));
    upload(dependencies.Get(), unused.data(), sizeof(unused));
    upload(cb.Get(), constants.data(), sizeof(constants));
    upload(source.Get(), inputs.data(), bytes);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                  0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "descriptor heap");
    const UINT step = dev->GetDescriptorHandleIncrementSize(hd.Type);
    auto cpu = [&](UINT i) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        return {heap->GetCPUDescriptorHandleForHeapStart().ptr + i * step};
    };
    auto gpu_handle = [&](UINT i) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        return {heap->GetGPUDescriptorHandleForHeapStart().ptr + i * step};
    };
    D3D12_SHADER_RESOURCE_VIEW_DESC view{};
    view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    view.Buffer.StructureByteStride = 16;
    view.Buffer.NumElements = 16;
    dev->CreateShaderResourceView(page_table.Get(), &view, cpu(0));
    view.Buffer.StructureByteStride = 4;
    view.Buffer.NumElements = 64;
    dev->CreateShaderResourceView(dependencies.Get(), &view, cpu(1));
    view.Format = DXGI_FORMAT_R32_TYPELESS;
    view.Buffer.StructureByteStride = 0;
    view.Buffer.NumElements = bytes / 4;
    view.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
    dev->CreateShaderResourceView(source.Get(), &view, cpu(2));
    D3D12_CONSTANT_BUFFER_VIEW_DESC cv{cb->GetGPUVirtualAddress(), 256};
    dev->CreateConstantBufferView(&cv, cpu(64));
    D3D12_UNORDERED_ACCESS_VIEW_DESC uv{};
    uv.Format = DXGI_FORMAT_R32_TYPELESS;
    uv.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uv.Buffer.NumElements = bytes / 4;
    uv.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
    dev->CreateUnorderedAccessView(result.Get(), nullptr, &uv, cpu(80));

    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC q{};
    check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "create queue");
    check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "create allocator");
    check(dev->CreateCommandList(0, q.Type, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)), "create command list");
    list->CopyBufferRegion(result.Get(), 0, seed.Get(), 0, bytes);
    transition(list.Get(), result.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    list->SetDescriptorHeaps(1, heap.GetAddressOf());
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, gpu_handle(0));
    list->SetComputeRootDescriptorTable(1, gpu_handle(64));
    list->SetComputeRootDescriptorTable(3, gpu_handle(80));
    list->Dispatch(2, 4, 1);
    transition(list.Get(), result.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, result.Get(), 0, bytes);
    submit_and_wait(dev.Get(), queue.Get(), list.Get());
    void* data{};
    D3D12_RANGE read{0, bytes}, empty{};
    check(readback->Map(0, &read, &data), "map readback");
    const auto* words = static_cast<const UINT*>(data);
    UINT matched = 0;
    for (UINT i = 0; i < expected.size(); ++i)
        matched += words[i] == expected[i];
    std::fprintf(receipt, "atomic_word_expected=%08x\natomic_word_actual=%08x\n", expected[atomic_word],
                 words[atomic_word]);
    std::fprintf(receipt, "packed_low=%08x\npacked_high=%08x\n", words[concurrent_word], words[concurrent_word + 1]);
    readback->Unmap(0, &empty);
    std::fprintf(receipt, "copied_words=%u\nguard_words=%zu\nmatched_words=%u\n", copied, expected.size() - copied - 3,
                 matched);
    if (matched != expected.size())
        throw std::runtime_error("transcode page copy readback mismatch");
}

int main() {
    FILE* out{};
    if (fopen_s(&out, "readback.txt", "wb"))
        return 2;
    int status = 0;
    try {
        run(out);
    } catch (const std::exception& e) {
        std::fprintf(out, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(out) ? 2 : status;
}
