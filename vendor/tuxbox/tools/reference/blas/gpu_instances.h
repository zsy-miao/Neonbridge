// A one-leaf cooked BLAS with two intersectable triangles. Its 192-byte
// destination forces both tested adapters to allocate larger native storage.
static std::array<UINT, 64> packed_quad() {
    std::array<UINT, 64> words{};
    for (auto [offset, value] : std::initializer_list<std::pair<UINT, UINT>>{
        {32,256}, {40,192}, {64,0x80000004}, {68,2}, {72,0xff1}, {76,1},
        {92,0x3f800000}, {96,0x3f800000}, {112,20}, {116,64}, {120,64}, {124,0x7f7f7f},
        {140,0x3f800000}, {156,0x3f800000}, {164,0x3f800000}, {168,0x3f800000},
        {176,128}, {180,1}, {184,0}, {188,0x0902}, {192,8}, {196,~0u}, {200,~0u}, {204,~0u}
    }) words[offset / 4] = value;
    return words;
}

struct CompactedSize {
    ComPtr<ID3D12Resource> output, readback;

    void record(ID3D12Device* dev, ID3D12GraphicsCommandList* list, UINT64 address) {
        output = buffer(dev, sizeof(UINT64), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        readback = buffer(dev, sizeof(UINT64), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        using Postbuild = void(WINAPI*)(IUnknown*, const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*,
                                        UINT, const UINT64*);
        const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC request{
            output->GetGPUVirtualAddress(), D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE};
        method<Postbuild>(list, 118)(list, &request, 1, &address);
        D3D12_RESOURCE_BARRIER barrier{};
        barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
        list->ResourceBarrier(1, &barrier);
        list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, sizeof(UINT64));
    }

    UINT64 read() const {
        void* data{};
        D3D12_RANGE range{0, sizeof(UINT64)}, empty{};
        check(readback->Map(0, &range, &data), "read native compacted size");
        UINT64 bytes{};
        std::memcpy(&bytes, data, sizeof(bytes));
        readback->Unmap(0, &empty);
        return bytes;
    }
};

struct CookedWrite {
    ComPtr<ID3D12Resource> source;
    ComPtr<ID3D12GraphicsCommandList> list;
    CompactedSize compacted;

    CookedWrite(ID3D12Device* device, ID3D12CommandAllocator* allocator, UINT64 address,
                const std::vector<UINT>& words) {
        source = upload(device, words.data(), words.size() * sizeof(UINT));
        list = guest_list(device, allocator);
        using Copy = void(WINAPI*)(IUnknown*, UINT64, UINT64, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
        method<Copy>(list.Get(), 119)(list.Get(), address, source->GetGPUVirtualAddress(),
                                      D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_DESERIALIZE);
        D3D12_RESOURCE_BARRIER ready{};
        ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
        list->ResourceBarrier(1, &ready);
        compacted.record(device, list.Get(), address);
        check(list->Close(), "close cooked BLAS write");
    }
};

#ifndef TUXBOX_PROMOTED_BLAS_PROBE
static UINT64 publish_quad(ID3D12Device* dev, ID3D12CommandQueue* queue, ID3D12CommandAllocator* allocator,
                           ID3D12Resource* destination, const std::vector<UINT>& words) {
    CookedWrite quad(dev, allocator, destination->GetGPUVirtualAddress(), words);
    submit_closed(dev, queue, quad.list.Get());
    return quad.compacted.read();
}

#endif

#if defined(TUXBOX_REUSED_BLAS_PROBE) || defined(TUXBOX_PROMOTED_BLAS_PROBE)
// Seven leaves plus two inner boxes and their parent form a 768-byte stream.
static std::vector<UINT> expanded_quads() {
    const auto quad = packed_quad();
    std::vector<UINT> words(768 / 4);
    std::copy_n(quad.begin(), 32, words.begin());
    words[8] = 768;
    words[10] = 704;
    words[17] = 10;
    words[19] = 7;
    const float max_x = 25.f;
    std::memcpy(&words[23], &max_x, sizeof(max_x));
    words[28] = (10u << 3) | 4u;
    for (UINT leaf = 0; leaf < 7; ++leaf) {
        auto* node = words.data() + 32 + 16 * leaf;
        std::copy_n(quad.begin() + 32, 16, node);
        for (UINT v = 0; v < 4; ++v) {
            float x;
            std::memcpy(&x, node + 3 * v, 4);
            x += 4.f * leaf;
            std::memcpy(node + 3 * v, &x, 4);
        }
        node[13] = 2 * leaf;
        node[14] = 2 * leaf + 1;
    }
    for (UINT box = 0; box < 3; ++box) {
        auto* node = words.data() + 16 * (9 + box);
        std::fill_n(node, 4, ~0u);
        if (box < 2) {
            for (UINT leaf = 4 * box; leaf < (box == 0 ? 4u : 7u); ++leaf)
                node[leaf % 4] = (1 + leaf) << 3;
        } else {
            node[0] = (8u << 3) | 4u;
            node[1] = (9u << 3) | 4u;
        }
    }
    return words;
}
#endif

#ifdef TUXBOX_REUSED_BLAS_PROBE
static void check_blas_reuse(ID3D12Device* dev, ID3D12CommandQueue* queue, ID3D12CommandAllocator* allocator,
                             ID3D12Resource* original, FILE* receipt) {
    // Slot 118 resolves the private native address while recording. Closing
    // before the rewrite and executing afterwards tests that exact old address.
    auto query = guest_list(dev, allocator);
    CompactedSize prior;
    prior.record(dev, query.Get(), original->GetGPUVirtualAddress());
    check(query->Close(), "close original BLAS size query");
    const auto larger = expanded_quads();
    UINT64 large_bytes = publish_quad(dev, queue, allocator, original, larger);
    submit_closed(dev, queue, query.Get());
    const UINT64 saved_size = prior.read();
    if (saved_size != large_bytes)
        throw std::runtime_error("old native BLAS address no longer holds the replacement");
    std::fprintf(receipt, "prior_address_compacted_bytes=%llu\n", saved_size);
    const auto initial = packed_quad();
    auto short_target =
        buffer(dev, 192, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    auto full_target =
        buffer(dev, 704, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    const auto copy = [&](ID3D12Resource* dst, ID3D12Resource* src,
                          D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE mode, bool fits) {
        auto list = guest_list(dev, allocator);
        using Copy = void(WINAPI*)(IUnknown*, UINT64, UINT64, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
        method<Copy>(list.Get(), 119)(list.Get(), dst->GetGPUVirtualAddress(), src->GetGPUVirtualAddress(), mode);
        if (!fits) {
            if (list->Close() != E_INVALIDARG)
                throw std::runtime_error("oversized BLAS copy accepted");
        } else {
            D3D12_RESOURCE_BARRIER ready{};
            ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
            list->ResourceBarrier(1, &ready);
            submit(dev, queue, list.Get());
        }
    };
    copy(short_target.Get(), original, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE, false);
    copy(full_target.Get(), original, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE, true);
    auto small_bytes = publish_quad(dev, queue, allocator, original, {initial.begin(), initial.end()});
    copy(short_target.Get(), original, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT, true);
    copy(original, full_target.Get(), D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT, true);
    if (large_bytes <= small_bytes)
        throw std::runtime_error("fixture did not increase native BLAS size");
    std::fprintf(receipt, "blas_reuse_host_compacted=%llu,%llu\nblas_reuse_guest_bytes=192,704,192,704\n", small_bytes,
                 large_bytes);
}
#endif

#include "instance_bindings_dxil.h"

class InstanceBindings {
    ComPtr<ID3D12RootSignature> root;
    ComPtr<ID3D12PipelineState> pso;
    ComPtr<ID3D12DescriptorHeap> heap;
    ComPtr<ID3D12Resource> input, output, readback;
public:
    void bind(ID3D12Device* guest, ID3D12GraphicsCommandList* list) {
        D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 0, 0, 0};
        D3D12_ROOT_PARAMETER params[4]{};
        params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        params[0].DescriptorTable = {1, &range};
        params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        params[1].Descriptor.ShaderRegister = 1;
        params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[3].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
        params[3].Constants.Num32BitValues = 4;
        D3D12_ROOT_SIGNATURE_DESC desc{4, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize binding root");
        check(guest->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "binding root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pipeline{};
        pipeline.pRootSignature = root.Get();
        pipeline.CS = {instance_bindings_dxil, sizeof(instance_bindings_dxil)};
        using Compute = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*, REFIID, void**);
        check(method<Compute>(guest,62)(guest, &pipeline, 0, nullptr, IID_PPV_ARGS(&pso)), "binding PSO");
        D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
        check(guest->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "binding descriptor heap");
        const UINT data[] = {17, 23};
        input = upload(guest, data, sizeof(data));
        output = buffer(guest, 16, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        readback = buffer(guest, 16, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
        srv.Format = DXGI_FORMAT_R32_TYPELESS;
        srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.Buffer.NumElements = 2;
        srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        const auto cpu = method<SIZE_T(WINAPI*)(IUnknown*)>(heap.Get(),9)(heap.Get());
        const auto gpu = method<UINT64(WINAPI*)(IUnknown*)>(heap.Get(),10)(heap.Get());
        guest->CreateShaderResourceView(input.Get(), &srv, {cpu});
        auto h = heap.Get();
        method<void(WINAPI*)(IUnknown*,UINT,ID3D12DescriptorHeap* const*)>(list, 32)(list, 1, &h);
        method<void(WINAPI*)(IUnknown*,ID3D12RootSignature*)>(list, 33)(list, root.Get());
        method<void(WINAPI*)(IUnknown*,UINT,D3D12_GPU_DESCRIPTOR_HANDLE)>(list,35)(list,0,{gpu});
        method<void(WINAPI*)(IUnknown*,UINT,UINT64)>(list,43)(list,1,input->GetGPUVirtualAddress());
        method<void(WINAPI*)(IUnknown*,UINT,UINT64)>(list,45)(list,2,output->GetGPUVirtualAddress());
        const UINT constants[] = {37, 0, 0, 0, 53};
        using Constants = void(WINAPI*)(IUnknown*,UINT,UINT,const UINT*,UINT);
        method<Constants>(list,39)(list,3,4,constants,0);
        method<Constants>(list,39)(list,3,1,constants + 4,2);
        list->SetPipelineState(pso.Get());
    }

    void dispatch(ID3D12GraphicsCommandList* list) {
        list->Dispatch(1,1,1);
        D3D12_RESOURCE_BARRIER barrier{};
        barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        barrier.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
        list->ResourceBarrier(1, &barrier);
        list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, 16);
    }

    void check_result() {
        D3D12_RANGE range{0,16}, none{};
        void* p{};
        check(readback->Map(0, &range, &p), "read guest binding checksum");
        std::array<UINT,4> words{};
        std::memcpy(words.data(),p,16);
        readback->Unmap(0,&none);
        if (words != std::array<UINT,4>{17,23,37,53}) throw std::runtime_error("guest bindings changed across scene build");
    }
};
