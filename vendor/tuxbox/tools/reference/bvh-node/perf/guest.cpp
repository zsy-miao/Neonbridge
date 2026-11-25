// Measure the float32 reference on a synthetic workload and compare every
// output word with a separately generated native-instruction result.
#include "context.h"
#include <cstdio>
#include <vector>
using namespace shader_control;

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    try {
        Context ctx(receipt);
        auto *device = ctx.device.Get();
        D3D12_ROOT_PARAMETER params[2]{};
        params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        params[0].Descriptor = {0, 0};
        params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        params[1].Descriptor = {0, 0};
        D3D12_ROOT_SIGNATURE_DESC rs{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob;
        check(D3D12SerializeRootSignature(&rs, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
        ComPtr<ID3D12RootSignature> root;
        check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "root");
        const auto code = read_shader("f32");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
        pd.pRootSignature = root.Get();
        pd.CS = {code.data(), code.size()};
        ComPtr<ID3D12PipelineState> pipeline;
        check(device->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pipeline)), "pipeline");
        constexpr UINT groups = 1024, threads = groups * 64, repeats = 4;
        auto input = buffer(device, 256, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        float vertices[12] = {0,0,0,1.127f,0,0,0,1.133f,0,1,1,0};
        uint32_t seed[64]{};
        memcpy(seed, vertices, sizeof(vertices));
        seed[15] = 9;
        float ray[6] = {.2f,.3f,-1.f,.1f,.2f,1.f};
        memcpy(seed + 32, ray, sizeof(ray));
        upload(input.Get(), seed, sizeof(seed));
        auto output = buffer(device, UINT64(threads)*16, D3D12_HEAP_TYPE_DEFAULT,
                             D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto output_copy = buffer(device, UINT64(threads)*16, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        auto time_copy = buffer(device, 2*sizeof(UINT64), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_QUERY_HEAP_DESC qh{D3D12_QUERY_HEAP_TYPE_TIMESTAMP, 2, 0};
        ComPtr<ID3D12QueryHeap> query;
        check(device->CreateQueryHeap(&qh, IID_PPV_ARGS(&query)), "query heap");
        ComPtr<ID3D12CommandAllocator> alloc;
        ComPtr<ID3D12GraphicsCommandList> list;
        check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&alloc)), "allocator");
        check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, alloc.Get(), nullptr, IID_PPV_ARGS(&list)), "list");
        list->SetComputeRootSignature(root.Get());
        list->SetComputeRootShaderResourceView(0, input->GetGPUVirtualAddress());
        list->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
        list->SetPipelineState(pipeline.Get());
        auto dispatch = [&] {
            list->Dispatch(groups, 1, 1);
            D3D12_RESOURCE_BARRIER barrier{};
            barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
            barrier.UAV.pResource = output.Get();
            list->ResourceBarrier(1, &barrier);
        };
        dispatch(); // warm-up
        list->EndQuery(query.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 0);
        for (unsigned i = 0; i < repeats; i++) dispatch();
        list->EndQuery(query.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 1);
        transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(output_copy.Get(), 0, output.Get(), 0, UINT64(threads)*16);
        list->ResolveQueryData(query.Get(), D3D12_QUERY_TYPE_TIMESTAMP, 0, 2, time_copy.Get(), 0);
        UINT64 frequency;
        check(ctx.queue->GetTimestampFrequency(&frequency), "timestamp frequency");
        submit_and_wait(device, ctx.queue.Get(), list.Get());
        void *times{};
        D3D12_RANGE time_range{0, 2*sizeof(UINT64)}, none{};
        check(time_copy->Map(0, &time_range, &times), "map timestamps");
        auto *t = static_cast<UINT64 *>(times);
        if (!frequency || t[1] <= t[0]) throw std::runtime_error("invalid timestamps");
        double ms = 1000.0 * double(t[1]-t[0]) / double(frequency) / repeats;
        time_copy->Unmap(0, &none);
        std::vector<UINT> native(threads * 4);
        std::ifstream expected("native.bin", std::ios::binary);
        expected.read(reinterpret_cast<char *>(native.data()), native.size()*4);
        if (expected.gcount() != native.size()*4 || expected.peek() != std::char_traits<char>::eof())
            throw std::runtime_error("invalid native result");
        void *mapped{};
        D3D12_RANGE output_range{0, SIZE_T(threads)*16};
        check(output_copy->Map(0, &output_range, &mapped), "map output");
        unsigned mismatches = 0;
        for (unsigned i=0; i<threads*4; i++)
            if (native[i] != static_cast<UINT *>(mapped)[i]) mismatches++;
        output_copy->Unmap(0, &none);
        std::fprintf(receipt, "threads=%u\nrepeats=%u\ngpu_ms=%.6f\nmismatched_words=%u\n",
                     threads, repeats, ms, mismatches);
        if (mismatches) throw std::runtime_error("output differs from native instruction");
        return fclose(receipt) ? 2 : 0;
    } catch (const std::exception &error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        fclose(receipt);
        return 1;
    }
}
