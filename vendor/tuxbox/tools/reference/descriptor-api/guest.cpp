#include "../compute_buffers.h"
#include "work.h"
using namespace compute_buffers;

static ComPtr<ID3D12DescriptorHeap> descriptors(ID3D12Device* device, UINT count, D3D12_DESCRIPTOR_HEAP_FLAGS flags) {
    D3D12_DESCRIPTOR_HEAP_DESC layout{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, count, flags, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&layout, IID_PPV_ARGS(&heap)), "create heap");
    const auto actual = heap->GetDesc();
    if (actual.Type != layout.Type || actual.NumDescriptors != count || actual.Flags != flags)
        throw std::runtime_error("unexpected heap shape");
    return heap;
}

int main() {
    FILE* result = nullptr;
    if (fopen_s(&result, "descriptor-api.txt", "w")) return 1;
    try {
        auto dll = LoadLibraryExW(L"d3d12_xs.dll", nullptr, LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        using Create = HRESULT(WINAPI*)(IUnknown*, const void*, REFIID, void**);
        auto create = dll ? reinterpret_cast<Create>(GetProcAddress(dll, "D3D12XboxCreateDevice")) : nullptr;
        if (!create) throw std::runtime_error("missing device factory");
        ComPtr<ID3D12Device> guest;
        check(create(nullptr, frame_abi::profile.data(), IID_PPV_ARGS(&guest)), "guest device");
        const BYTE api = reinterpret_cast<volatile const BYTE*>(guest.Get())[0x18];
        std::fprintf(result, "api_copy_byte=%u\n", unsigned(api));
        if (api != 1) throw std::runtime_error("device did not select the API copy branch");

        D3D12_DESCRIPTOR_RANGE ranges[] = {{D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
                                           {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 1}};
        D3D12_ROOT_PARAMETER table{};
        table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        table.DescriptorTable = {2, ranges};
        D3D12_ROOT_SIGNATURE_DESC layout{1, &table, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> sig;
        check(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &sig, nullptr), "serialize root");
        ComPtr<ID3D12RootSignature> root;
        check(guest->CreateRootSignature(0, sig->GetBufferPointer(), sig->GetBufferSize(), IID_PPV_ARGS(&root)), "guest root");
        ComPtr<ID3D12Device> owner;
        check(root->GetDevice(IID_PPV_ARGS(&owner)), "owning desktop device");
        D3D12_COMPUTE_PIPELINE_STATE_DESC ps{};
        ps.pRootSignature = root.Get();
        ps.CS = {work_dxil, sizeof(work_dxil)};
        ComPtr<ID3D12PipelineState> pipeline;
        check(owner->CreateComputePipelineState(&ps, IID_PPV_ARGS(&pipeline)), "compute pipeline");

        auto source = descriptors(guest.Get(), 1024, D3D12_DESCRIPTOR_HEAP_FLAG_NONE);
        auto dest = descriptors(guest.Get(), 500000, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE);
        using Handle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
        const auto srcBase = frame_abi::method<Handle>(source.Get(), 9)(source.Get());
        const auto dstBase = frame_abi::method<Handle>(dest.Get(), 9)(dest.Get());
        const auto gpuBase = frame_abi::method<Handle>(dest.Get(), 10)(dest.Get());
        const auto type = D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV;
        const UINT step = guest->GetDescriptorHandleIncrementSize(type);
        std::fprintf(result, "step=%u\nsrc_low5=%llu\ndst_low5=%llu\n", step, srcBase & 31, dstBase & 31);
        constexpr UINT count = 37, first = 3, words = 43, guard = 0x66778899, seed = 0xacbca987;
        auto cb = buffer(owner.Get(), 256, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto init = buffer(owner.Get(), 4 * words, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto values = buffer(owner.Get(), 4 * words, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                             D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto readback = buffer(owner.Get(), 4 * words, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        std::array<UINT, words> initial;
        initial.fill(guard);
        upload(cb.Get(), &seed, sizeof(seed));
        upload(init.Get(), initial.data(), sizeof(initial));
        D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb->GetGPUVirtualAddress(), 256};
        const D3D12_CPU_DESCRIPTOR_HANDLE cbFrom{srcBase + 37 * step}, cbTo{dstBase + 17 * step};
        guest->CreateConstantBufferView(&cbv, cbFrom);
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
        uav.Format = DXGI_FORMAT_R32_UINT;
        uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        uav.Buffer.FirstElement = first;
        uav.Buffer.NumElements = count;
        const D3D12_CPU_DESCRIPTOR_HANDLE uavFrom{srcBase + 38 * step}, uavTo{dstBase + 18 * step};
        guest->CreateUnorderedAccessView(values.Get(), nullptr, &uav, uavFrom);
        guest->CopyDescriptorsSimple(1, cbTo, cbFrom, type);
        UINT one = 1;
        guest->CopyDescriptors(1, &uavTo, &one, 1, &uavFrom, nullptr, type);

        D3D12_COMMAND_QUEUE_DESC q{};
        ComPtr<ID3D12CommandQueue> queue;
        ComPtr<ID3D12CommandAllocator> alloc;
        ComPtr<ID3D12GraphicsCommandList> list;
        check(owner->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "queue");
        check(owner->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "allocator");
        check(owner->CreateCommandList(0, q.Type, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "commands");
        list->CopyBufferRegion(values.Get(), 0, init.Get(), 0, words * 4);
        transition(list.Get(), values.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        ID3D12DescriptorHeap* heaps[] = {dest.Get()};
        list->SetDescriptorHeaps(1, heaps);
        list->SetComputeRootSignature(root.Get());
        list->SetComputeRootDescriptorTable(0, {gpuBase + 17 * step});
        list->Dispatch(count, 1, 1);
        transition(list.Get(), values.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(), 0, values.Get(), 0, words * 4);
        submit_and_wait(owner.Get(), queue.Get(), list.Get());
        D3D12_RANGE read{0, words * 4};
        void* data;
        check(readback->Map(0, &read, &data), "readback");
        UINT matches = 0;
        for (UINT i = 0; i < words; ++i) {
            const UINT expected = i >= first && i < first + count ? seed ^ ((i - first) * 2654435761u) : guard;
            matches += static_cast<const UINT*>(data)[i] == expected;
        }
        readback->Unmap(0, nullptr);
        std::fprintf(result, "matching_words=%u\n", matches);
        if (matches != words) throw std::runtime_error("GPU output differs");
    } catch (const std::exception& ex) {
        std::fprintf(result, "error=%s\n", ex.what());
        fclose(result);
        return 1;
    }
    fclose(result);
    return 0;
}
