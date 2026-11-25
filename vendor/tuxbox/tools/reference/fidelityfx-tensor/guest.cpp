// Run one tensor pass with the root signature shipped in the upscaler DLL.
#include "../independent-shaders/context.h"
#include <algorithm>
#include <array>
using namespace shader_control;

static void dispatch(Context &ctx) {
    auto *device = ctx.device.Get();
    auto original = read_shader("original"), shader = read_shader("translated");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, original.data(), original.size(), IID_PPV_ARGS(&root)), "packaged root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root.Get();
    desc.CS = {shader.data(), shader.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateComputePipelineState(&desc, IID_PPV_ARGS(&pipeline)), "tensor pipeline");
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pipeline.Get(),
                                    IID_PPV_ARGS(&list)), "command list");

    constexpr UINT tensorBytes = 32 << 20, weightBytes = 256 << 10;
    std::vector<UINT> tensor(tensorBytes / 4), weights(weightBytes / 4);
    for (UINT i = 0; i < tensor.size(); ++i) tensor[i] = (i % 7 * 0x01020304u) ^ 0xf105fc02u;
    // Ready flags for upstream tiles, including several different 32-lane waves.
    std::fill_n(tensor.begin() + 21029376 / 4, 4096, 1u);
    for (UINT i = 0; i < weights.size(); ++i) weights[i] = i % 5 * 3;
    std::array<UINT, 64> dimensions{};
    for (UINT i = 0; i < dimensions.size(); i += 2) {
        dimensions[i] = 128;
        dimensions[i + 1] = 4;
    }
    auto makeUpload = [&](const void *data, UINT bytes) {
        auto resource = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(resource.Get(), data, bytes);
        return resource;
    };
    auto cb = makeUpload(dimensions.data(), sizeof(dimensions));
    auto weightBuffer = makeUpload(weights.data(), weightBytes);
    auto seed = makeUpload(tensor.data(), tensorBytes);
    auto output = buffer(device, tensorBytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto copy = buffer(device, tensorBytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    list->CopyBufferRegion(output.Get(), 0, seed.Get(), 0, tensorBytes);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);

    constexpr UINT uavCount = 13, srvCount = 21;
    D3D12_DESCRIPTOR_HEAP_DESC heapDesc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, uavCount + srvCount,
                                       D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&heapDesc, IID_PPV_ARGS(&heap)), "tensor descriptor heap");
    auto cpu = heap->GetCPUDescriptorHandleForHeapStart();
    auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
    const UINT stride = device->GetDescriptorHandleIncrementSize(heapDesc.Type);
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Format = DXGI_FORMAT_R32_TYPELESS;
    uav.Buffer.NumElements = tensorBytes / 4;
    uav.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
    device->CreateUnorderedAccessView(output.Get(), nullptr, &uav, {cpu.ptr + 11 * stride});
    D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
    srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    srv.Format = DXGI_FORMAT_R32_TYPELESS;
    srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    srv.Buffer.NumElements = weightBytes / 4;
    srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
    device->CreateShaderResourceView(weightBuffer.Get(), &srv, {cpu.ptr + (uavCount + 18) * stride});
    list->SetDescriptorHeaps(1, heap.GetAddressOf());
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, gpu);
    list->SetComputeRootDescriptorTable(1, {gpu.ptr + uavCount * stride});
    for (UINT i = 2; i < 6; ++i) list->SetComputeRootConstantBufferView(i, cb->GetGPUVirtualAddress());
    list->Dispatch(1, 1, 1);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(copy.Get(), 0, output.Get(), 0, tensorBytes);
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(copy.Get(), "tensor", tensorBytes);
}

static int run_case() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "w")) return 2;
    try {
        Context ctx(receipt);
        dispatch(ctx);
    } catch (const std::exception &error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        fclose(receipt);
        return 1;
    }
    return fclose(receipt) ? 1 : 0;
}

int main(int argc, char **argv) {
    if (argc == 1) return run_case();
    for (int i = 1; i < argc; ++i) {
        if (!SetCurrentDirectoryA(argv[i])) return 2;
        int result = run_case();
        if (result) return result;
        if (!SetCurrentDirectoryA("..")) return 2;
    }
    return 0;
}
