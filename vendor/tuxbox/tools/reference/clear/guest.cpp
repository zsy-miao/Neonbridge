// Execute the actual cached Frosty ClearCS DXIL using its cooked descriptor tables.
#include "../compute_buffers.h"
#ifdef TUXBOX_CACHE_CONTROL
#include "frame_abi.h"
#include <chrono>
#include <filesystem>
#include <fstream>
#else
#include "clear_blobs.h"
#endif
#include <algorithm>
#include <cstring>
#include <vector>

using namespace compute_buffers;

static void run(ID3D12Device* device, ID3D12RootSignature* root, ID3D12PipelineState* pso, UINT components,
                bool integer_view, FILE* receipt) {
    constexpr UINT count = 140, start = 5, end = 102;
    const std::array<UINT, 4> color = integer_view
                                          ? std::array<UINT, 4>{0x01234567, 0x00000001, 0xdeadbeef, 0xffffffff}
                                          : std::array<UINT, 4>{0x3e800000, 0xbf000000, 0x3fc00000, 0x41000000};
    const UINT bytes = count * components * sizeof(UINT);
    std::vector<UINT> expected(count * components, 0xdeadc0de);
    auto initial = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto params = buffer(device, 256, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto output = buffer(device, bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<UINT, 64> cb{};
    std::copy(color.begin(), color.end(), cb.begin());
    cb[4] = start;
    cb[8] = end;
    upload(params.Get(), cb.data(), sizeof(cb));
    upload(initial.Get(), expected.data(), bytes);
    D3D12_DESCRIPTOR_HEAP_DESC desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 32,
                                    D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&heap)), "CreateDescriptorHeap");
#ifdef TUXBOX_CACHE_CONTROL
    using GpuHandle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
    D3D12_CPU_DESCRIPTOR_HANDLE cpu{frame_abi::method<frame_abi::CpuHandle>(heap.Get(), 9)(heap.Get())};
    D3D12_GPU_DESCRIPTOR_HANDLE gpu{frame_abi::method<GpuHandle>(heap.Get(), 10)(heap.Get())};
#else
    auto cpu = heap->GetCPUDescriptorHandleForHeapStart();
    auto gpu = heap->GetGPUDescriptorHandleForHeapStart();
#endif
    UINT stride = device->GetDescriptorHandleIncrementSize(desc.Type);
    D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{params->GetGPUVirtualAddress(), 256};
    device->CreateConstantBufferView(&cbv, cpu);
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.Format = integer_view ? (components == 4 ? DXGI_FORMAT_R32G32B32A32_UINT : DXGI_FORMAT_R32_UINT)
                              : (components == 4 ? DXGI_FORMAT_R32G32B32A32_FLOAT : DXGI_FORMAT_R32_FLOAT);
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    uav.Buffer.NumElements = count;
    device->CreateUnorderedAccessView(output.Get(), nullptr, &uav, {cpu.ptr + 16 * stride});
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "CreateCommandQueue");
    check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "CreateCommandAllocator");
#ifdef TUXBOX_CACHE_CONTROL
    const UINT list_desc[]{D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    check(frame_abi::method<frame_abi::CreateList>(device, 73)(device, list_desc, allocator.Get(), pso,
          IID_PPV_ARGS(&list)), "create guest command list");
#else
    check(device->CreateCommandList(0, queue_desc.Type, allocator.Get(), pso, IID_PPV_ARGS(&list)), "CreateCommandList");
#endif
    list->CopyBufferRegion(output.Get(), 0, initial.Get(), 0, bytes);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    ID3D12DescriptorHeap* heaps[] = {heap.Get()};
#ifdef TUXBOX_CACHE_CONTROL
    using Bind = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    frame_abi::method<Bind>(list.Get(), 32)(list.Get(), 1, heaps);
    frame_abi::method<Root>(list.Get(), 33)(list.Get(), root);
    auto table = frame_abi::method<Table>(list.Get(), 35);
    table(list.Get(), 1, gpu);
    table(list.Get(), 3, {gpu.ptr + 16 * stride});
#else
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(root);
    list->SetComputeRootDescriptorTable(1, gpu);
    list->SetComputeRootDescriptorTable(3, {gpu.ptr + 16 * stride});
#endif
    list->Dispatch(2, 1, 1);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, bytes);
    submit_and_wait(device, queue.Get(), list.Get());
    for (UINT i = start; i < end; ++i) {
        std::copy_n(color.begin(), components, expected.begin() + i * components);
    }
    void* mapped{};
    D3D12_RANGE read{0, bytes}, empty{};
    check(readback->Map(0, &read, &mapped), "Map readback");
    auto* actual = static_cast<const UINT*>(mapped);
    UINT matched = 0;
    for (UINT i = 0; i < expected.size(); ++i)
        matched += (actual[i] == expected[i]);
    readback->Unmap(0, &empty);
    std::fprintf(receipt, "%s_%u_matched_words=%u\n", integer_view ? "uint_view" : "float_view", components,
                 matched);
    if (!integer_view && matched != expected.size())
        throw std::runtime_error("GPU float output differs from CPU oracle");
    std::fflush(receipt);
}

int main() {
    FILE* receipt{};
    if (fopen_s(&receipt, "readback.txt", "wb"))
        return 2;
    int status = 0;
    try {
#ifdef TUXBOX_CACHE_CONTROL
        auto work = std::filesystem::absolute("temp");
        if (!SetEnvironmentVariableW(L"TMP", work.c_str()) || !SetEnvironmentVariableW(L"TEMP", work.c_str()))
            throw std::runtime_error("set local shader cache directory");
        std::ifstream source("source.dxbc", std::ios::binary);
        std::vector<BYTE> shader{std::istreambuf_iterator<char>(source), std::istreambuf_iterator<char>()};
        std::ifstream signature("root.dxbc", std::ios::binary);
        std::vector<BYTE> cooked_root{std::istreambuf_iterator<char>(signature), std::istreambuf_iterator<char>()};
        if (shader.empty() || cooked_root.empty()) throw std::runtime_error("missing shader or root signature");
        constexpr std::array<UINT, 18> creation{
            0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
        DXGI_ADAPTER_DESC1 gpu{};
        auto device = frame_abi::create_device(nullptr, gpu, creation.data());
#else
        ComPtr<ID3D12Device> device;
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&device)), "D3D12CreateDevice");
        ComPtr<IDXGIFactory4> factory;
        ComPtr<IDXGIAdapter1> adapter;
        DXGI_ADAPTER_DESC1 gpu{};
        check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
        check(factory->EnumAdapterByLuid(device->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "EnumAdapterByLuid");
        check(adapter->GetDesc1(&gpu), "GetDesc1");
#endif
        std::fprintf(receipt, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
        ComPtr<ID3D12RootSignature> root;
#ifdef TUXBOX_CACHE_CONTROL
        check(device->CreateRootSignature(0, cooked_root.data(), cooked_root.size(), IID_PPV_ARGS(&root)), "create cooked root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
        desc.pRootSignature = root.Get();
        desc.CS = {shader.data(), shader.size()};
        ComPtr<ID3D12PipelineState> pso;
        using Pipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                           UINT, const void*, REFIID, void**);
        const auto start = std::chrono::steady_clock::now();
        const auto hr = frame_abi::method<Pipeline>(device.Get(), 62)(device.Get(), &desc, 0, nullptr,
                                                                      IID_PPV_ARGS(&pso));
        const auto elapsed = std::chrono::duration_cast<std::chrono::microseconds>(
            std::chrono::steady_clock::now() - start);
        std::fprintf(receipt, "pipeline_us=%lld\n", static_cast<long long>(elapsed.count()));
        std::fprintf(receipt, "pipeline_hr=0x%08lx\n", static_cast<unsigned long>(hr));
        check(hr, "create cooked compute pipeline");
#else
        check(device->CreateRootSignature(0, cooked_shader, sizeof(cooked_shader), IID_PPV_ARGS(&root)),
              "CreateRootSignature");
        D3D12_COMPUTE_PIPELINE_STATE_DESC desc{};
        desc.pRootSignature = root.Get();
        desc.CS = {desktop_shader, sizeof(desktop_shader)};
        ComPtr<ID3D12PipelineState> pso;
        check(device->CreateComputePipelineState(&desc, IID_PPV_ARGS(&pso)), "CreateComputePipelineState");
#endif
        run(device.Get(), root.Get(), pso.Get(), 4, false, receipt);
        run(device.Get(), root.Get(), pso.Get(), 1, false, receipt);
        // The cached shader declares a float UAV. Measure the unsigned view separately
        // since the retail caller is RHIClearUAVUint and a driver can treat it differently.
        run(device.Get(), root.Get(), pso.Get(), 4, true, receipt);
        run(device.Get(), root.Get(), pso.Get(), 1, true, receipt);
    } catch (const std::exception& error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        status = 1;
    }
    return fclose(receipt) ? 2 : status;
}
