// Queue-ordered raw Xbox BLAS body retention control. No runtime integration.
#include "../independent-shaders/context.h"
#include "retained_dxil.h"
#include "directory_dxil.h"
using namespace shader_control;

static void wait(ID3D12Fence* fence, UINT64 value) {
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(value, event);
    DWORD result = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "set completion event");
    if (result != WAIT_OBJECT_0 || fence->GetCompletedValue() < value)
        throw std::runtime_error("wait for copy");
}

int main() {
    FILE* receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    int result = 0;
    try {
        Context ctx(receipt);
        std::ifstream input("source.bin", std::ios::binary);
        std::vector<char> bytes{std::istreambuf_iterator<char>{input}, {}};
        if (bytes.size() < 65 || bytes.size() > 16 * 1024 * 1024)
            throw std::runtime_error("invalid serialized body");
        const UINT body = static_cast<UINT>(bytes.size() - 64);
        constexpr UINT offset = 256;
        auto a = buffer(ctx.device.Get(), static_cast<UINT>(bytes.size()) + offset, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto b = buffer(ctx.device.Get(), static_cast<UINT>(bytes.size()) + offset, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto old = buffer(ctx.device.Get(), body, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        auto newer = buffer(ctx.device.Get(), body, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        auto clone = buffer(ctx.device.Get(), body, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        auto readback = buffer(ctx.device.Get(), body * 3 + 192, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        auto observed = buffer(ctx.device.Get(), 192, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        D3D12_ROOT_PARAMETER params[4]{};
        for (UINT n = 0; n < 3; ++n) {
            params[n].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
            params[n].Descriptor.ShaderRegister = n;
        }
        params[3].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        D3D12_ROOT_SIGNATURE_DESC desc{4, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "root serialization");
        ComPtr<ID3D12RootSignature> root;
        check(ctx.device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(), IID_PPV_ARGS(&root)), "root signature");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
        pd.pRootSignature = root.Get();
        pd.CS = {retained_dxil, sizeof(retained_dxil)};
        ComPtr<ID3D12PipelineState> pipeline;
        check(ctx.device->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pipeline)), "pipeline");
        ComPtr<ID3D12CommandQueue> second;
        D3D12_COMMAND_QUEUE_DESC q{};
        check(ctx.device->CreateCommandQueue(&q, IID_PPV_ARGS(&second)), "second queue");
        ComPtr<ID3D12CommandAllocator> allocator_a, allocator_b;
        ComPtr<ID3D12GraphicsCommandList> list_a, list_b;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator_a)), "allocator a");
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator_b)), "allocator b");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator_a.Get(), nullptr, IID_PPV_ARGS(&list_a)), "list a");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator_b.Get(), nullptr, IID_PPV_ARGS(&list_b)), "list b");
        list_a->CopyBufferRegion(old.Get(), 0, a.Get(), offset + 64, body);
        transition(list_a.Get(), old.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list_b->CopyBufferRegion(newer.Get(), 0, b.Get(), offset + 64, body);
        list_b->CopyBufferRegion(clone.Get(), 0, old.Get(), 0, body);
        transition(list_b.Get(), newer.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        transition(list_b.Get(), clone.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list_b->CopyBufferRegion(readback.Get(), 0, old.Get(), 0, body);
        list_b->CopyBufferRegion(readback.Get(), body, newer.Get(), 0, body);
        list_b->CopyBufferRegion(readback.Get(), body * 2, clone.Get(), 0, body);
        transition(list_b.Get(), old.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(list_b.Get(), newer.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(list_b.Get(), clone.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        list_b->SetComputeRootSignature(root.Get());
        list_b->SetPipelineState(pipeline.Get());
        list_b->SetComputeRootShaderResourceView(0, old->GetGPUVirtualAddress());
        list_b->SetComputeRootShaderResourceView(1, newer->GetGPUVirtualAddress());
        list_b->SetComputeRootShaderResourceView(2, clone->GetGPUVirtualAddress());
        list_b->SetComputeRootUnorderedAccessView(3, observed->GetGPUVirtualAddress());
        list_b->Dispatch(1, 1, 1);
        transition(list_b.Get(), observed.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list_b->CopyBufferRegion(readback.Get(), body * 3, observed.Get(), 0, 192);
        check(list_a->Close(), "close a");
        check(list_b->Close(), "close b");
        // Populate both inputs after close. Only generation B differs, at the
        // last payload byte, so its distinct body is visible in the readback.
        std::vector<char> data(offset + bytes.size());
        std::copy(bytes.begin(), bytes.end(), data.begin() + offset);
        upload(a.Get(), data.data(), data.size());
        data.back() ^= 0x5a;
        upload(b.Get(), data.data(), data.size());
        ComPtr<ID3D12Fence> ready, complete;
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&ready)), "ready fence");
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&complete)), "completion fence");
        ID3D12CommandList* first[] = {list_a.Get()};
        ID3D12CommandList* next[] = {list_b.Get()};
        // Queue B waits on A's copy; CPU does not wait between submissions.
        check(second->Wait(ready.Get(), 1), "queue wait");
        second->ExecuteCommandLists(1, next);
        check(second->Signal(complete.Get(), 1), "signal complete");
        // The dependency has not been signaled, so this resource set must
        // still be retained while the second queue is pending.
        if (complete->GetCompletedValue() != 0) throw std::runtime_error("queue wait did not block");
        ctx.queue->ExecuteCommandLists(1, first);
        check(ctx.queue->Signal(ready.Get(), 1), "signal ready");
        wait(complete.Get(), 1);
        write_buffer(readback.Get(), "retained", body * 3 + 192);
        // The previous executions are complete before replay. Restore the
        // recorded initial states, rewrite both uploads, and execute again.
        ComPtr<ID3D12CommandAllocator> reset_allocator;
        ComPtr<ID3D12GraphicsCommandList> reset_list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&reset_allocator)), "reset allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, reset_allocator.Get(), nullptr, IID_PPV_ARGS(&reset_list)), "reset list");
        transition(reset_list.Get(), old.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        transition(reset_list.Get(), newer.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        transition(reset_list.Get(), clone.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        transition(reset_list.Get(), observed.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        submit_and_wait(ctx.device.Get(), ctx.queue.Get(), reset_list.Get());
        upload(a.Get(), data.data(), data.size());
        data.back() ^= 0x5a;
        upload(b.Get(), data.data(), data.size());
        check(second->Wait(ready.Get(), 2), "replay queue wait");
        second->ExecuteCommandLists(1, next);
        ctx.queue->ExecuteCommandLists(1, first);
        check(ctx.queue->Signal(ready.Get(), 2), "replay signal ready");
        check(second->Signal(complete.Get(), 2), "replay signal complete");
        wait(complete.Get(), 2);
        write_buffer(readback.Get(), "replayed", body * 3 + 192);
        // Two lists target the same retained bytes. Record the list that will
        // execute second first, then submit in the opposite order.
        auto reverse_target = buffer(ctx.device.Get(), body, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        auto reverse_readback = buffer(ctx.device.Get(), body, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        ComPtr<ID3D12CommandAllocator> reverse_allocator_a, reverse_allocator_b;
        ComPtr<ID3D12GraphicsCommandList> reverse_a, reverse_b;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&reverse_allocator_a)), "reverse allocator a");
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&reverse_allocator_b)), "reverse allocator b");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, reverse_allocator_a.Get(), nullptr, IID_PPV_ARGS(&reverse_a)), "reverse list a");
        transition(reverse_a.Get(), reverse_target.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        reverse_a->CopyBufferRegion(reverse_target.Get(), 0, a.Get(), offset + 64, body);
        transition(reverse_a.Get(), reverse_target.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        reverse_a->CopyBufferRegion(reverse_readback.Get(), 0, reverse_target.Get(), 0, body);
        check(reverse_a->Close(), "close reverse a");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, reverse_allocator_b.Get(), nullptr, IID_PPV_ARGS(&reverse_b)), "reverse list b");
        reverse_b->CopyBufferRegion(reverse_target.Get(), 0, b.Get(), offset + 64, body);
        transition(reverse_b.Get(), reverse_target.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        check(reverse_b->Close(), "close reverse b");
        ID3D12CommandList* reverse_first[] = {reverse_b.Get()};
        ID3D12CommandList* reverse_next[] = {reverse_a.Get()};
        ctx.queue->ExecuteCommandLists(1, reverse_first);
        ctx.queue->ExecuteCommandLists(1, reverse_next);
        check(ctx.queue->Signal(complete.Get(), 3), "reverse signal");
        wait(complete.Get(), 3);
        write_buffer(reverse_readback.Get(), "reversed", body);
        // Bounded directory: record A then B, submit B then A. Duplicate,
        // malformed, absent and out-of-range keys must never resolve silently.
        const UINT64 entry_a[][2] = {
            {0x12345600ull, old->GetGPUVirtualAddress()},
            {0x22345600ull, newer->GetGPUVirtualAddress()},
            {0x42345600ull, old->GetGPUVirtualAddress()},
            {0x42345600ull, newer->GetGPUVirtualAddress()},
            {0x52345600ull, 0},
        };
        UINT64 entry_b[5][2]{};
        std::memcpy(entry_b, entry_a, sizeof(entry_a));
        entry_b[0][1] = newer->GetGPUVirtualAddress();
        UINT64 entry_bad[5][2]{};
        std::memcpy(entry_bad, entry_a, sizeof(entry_a));
        entry_bad[4][1] = 1; // nonzero but not a valid aligned backing address
        const UINT64 query_values[] = {0x12345600ull, 0x22345600ull, 0x32345600ull,
            0x42345600ull, 0, 0x12345601ull, 0x10000000000ull, 0x52345600ull};
        auto entry_a_upload = buffer(ctx.device.Get(), sizeof(entry_a), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto entry_b_upload = buffer(ctx.device.Get(), sizeof(entry_b), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto entry_bad_upload = buffer(ctx.device.Get(), sizeof(entry_bad), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto query_upload = buffer(ctx.device.Get(), sizeof(query_values), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto directory = buffer(ctx.device.Get(), sizeof(entry_a), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
        auto directory_result = buffer(ctx.device.Get(), 896, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto directory_readback = buffer(ctx.device.Get(), 896, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        auto directory_seed = buffer(ctx.device.Get(), 896, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        UINT seed_words[224]{};
        for (UINT i = 0; i < 224; ++i) seed_words[i] = 0x76540000u + i;
        upload(directory_seed.Get(), seed_words, sizeof(seed_words));
        upload(entry_a_upload.Get(), entry_a, sizeof(entry_a));
        upload(entry_b_upload.Get(), entry_b, sizeof(entry_b));
        upload(entry_bad_upload.Get(), entry_bad, sizeof(entry_bad));
        upload(query_upload.Get(), query_values, sizeof(query_values));
        D3D12_DESCRIPTOR_HEAP_DESC dir_heap_desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 9, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
        ComPtr<ID3D12DescriptorHeap> dir_heap;
        check(ctx.device->CreateDescriptorHeap(&dir_heap_desc, IID_PPV_ARGS(&dir_heap)), "directory descriptor heap");
        const UINT dir_stride = ctx.device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        auto cpu = dir_heap->GetCPUDescriptorHandleForHeapStart();
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
        srv.Format = DXGI_FORMAT_R32_TYPELESS;
        srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        srv.Buffer.NumElements = sizeof(entry_a) / 4;
        ctx.device->CreateShaderResourceView(directory.Get(), &srv, cpu);
        cpu.ptr += dir_stride;
        srv.Buffer.NumElements = sizeof(query_values) / 4;
        ctx.device->CreateShaderResourceView(query_upload.Get(), &srv, cpu);
        for (UINT n = 0; n < 7; ++n) {
            cpu.ptr += dir_stride;
            D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
            uav.Format = DXGI_FORMAT_R32_TYPELESS;
            uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
            uav.Buffer.FirstElement = n * 32;
            uav.Buffer.NumElements = n == 6 ? 16 : 32;
            uav.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
            ctx.device->CreateUnorderedAccessView(directory_result.Get(), nullptr, &uav, cpu);
        }
        D3D12_DESCRIPTOR_RANGE ranges[2]{};
        ranges[0] = {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 2, 0, 0, 0};
        ranges[1] = {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
        D3D12_ROOT_PARAMETER dir_params[3]{};
        for (UINT n = 0; n < 2; ++n) {
            dir_params[n].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
            dir_params[n].DescriptorTable = {1, &ranges[n]};
        }
        dir_params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
        dir_params[2].Constants.Num32BitValues = 2;
        D3D12_ROOT_SIGNATURE_DESC dir_desc{3, dir_params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> dir_blob;
        check(D3D12SerializeRootSignature(&dir_desc, D3D_ROOT_SIGNATURE_VERSION_1, &dir_blob, nullptr), "directory root serialization");
        ComPtr<ID3D12RootSignature> dir_root;
        check(ctx.device->CreateRootSignature(0, dir_blob->GetBufferPointer(), dir_blob->GetBufferSize(), IID_PPV_ARGS(&dir_root)), "directory root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC dir_pd{};
        dir_pd.pRootSignature = dir_root.Get();
        dir_pd.CS = {directory_dxil, sizeof(directory_dxil)};
        ComPtr<ID3D12PipelineState> dir_pipeline;
        check(ctx.device->CreateComputePipelineState(&dir_pd, IID_PPV_ARGS(&dir_pipeline)), "directory pipeline");
        ComPtr<ID3D12CommandAllocator> dir_allocator_a, dir_allocator_b;
        ComPtr<ID3D12GraphicsCommandList> dir_a, dir_b;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&dir_allocator_a)), "directory allocator a");
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&dir_allocator_b)), "directory allocator b");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, dir_allocator_a.Get(), nullptr, IID_PPV_ARGS(&dir_a)), "directory list a");
        transition(dir_a.Get(), directory.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        dir_a->CopyBufferRegion(directory.Get(), 0, entry_a_upload.Get(), 0, sizeof(entry_a));
        transition(dir_a.Get(), directory.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        dir_a->CopyBufferRegion(directory_result.Get(), 0, directory_seed.Get(), 0, 896);
        transition(dir_a.Get(), directory_result.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        dir_a->SetComputeRootSignature(dir_root.Get());
        dir_a->SetPipelineState(dir_pipeline.Get());
        ID3D12DescriptorHeap* dir_heaps[] = {dir_heap.Get()};
        dir_a->SetDescriptorHeaps(1, dir_heaps);
        auto gpu = dir_heap->GetGPUDescriptorHandleForHeapStart();
        dir_a->SetComputeRootDescriptorTable(0, gpu);
        const UINT cases[][2] = {{5, 8}, {6, 8}, {0, 8}, {5, 9}, {5, 0}};
        for (UINT n = 0; n < 5; ++n) {
            auto view = gpu; view.ptr += UINT64(dir_stride) * (2 + n);
            dir_a->SetComputeRootDescriptorTable(1, view);
            dir_a->SetComputeRoot32BitConstants(2, 2, cases[n], 0);
            dir_a->Dispatch(1, 1, 1);
            D3D12_RESOURCE_BARRIER uav{};
            uav.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
            uav.UAV.pResource = directory_result.Get();
            dir_a->ResourceBarrier(1, &uav);
        }
        transition(dir_a.Get(), directory.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        dir_a->CopyBufferRegion(directory.Get(), 0, entry_bad_upload.Get(), 0, sizeof(entry_bad));
        transition(dir_a.Get(), directory.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        auto last_view = gpu; last_view.ptr += UINT64(dir_stride) * 7;
        dir_a->SetComputeRootDescriptorTable(1, last_view);
        dir_a->SetComputeRoot32BitConstants(2, 2, cases[0], 0);
        dir_a->Dispatch(1, 1, 1);
        D3D12_RESOURCE_BARRIER final_uav{};
        final_uav.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
        final_uav.UAV.pResource = directory_result.Get();
        dir_a->ResourceBarrier(1, &final_uav);
        auto short_view = gpu; short_view.ptr += UINT64(dir_stride) * 8;
        dir_a->SetComputeRootDescriptorTable(1, short_view);
        dir_a->Dispatch(1, 1, 1);
        transition(dir_a.Get(), directory_result.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        dir_a->CopyBufferRegion(directory_readback.Get(), 0, directory_result.Get(), 0, 896);
        check(dir_a->Close(), "close directory a");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, dir_allocator_b.Get(), nullptr, IID_PPV_ARGS(&dir_b)), "directory list b");
        dir_b->CopyBufferRegion(directory.Get(), 0, entry_b_upload.Get(), 0, sizeof(entry_b));
        transition(dir_b.Get(), directory.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        check(dir_b->Close(), "close directory b");
        ID3D12CommandList* dir_first[] = {dir_b.Get()};
        ID3D12CommandList* dir_next[] = {dir_a.Get()};
        // Queue A's consumer before queue B's producer; the future fence
        // signal orders the shared directory writes across the two queues.
        check(second->Wait(ready.Get(), 3), "directory queue wait");
        second->ExecuteCommandLists(1, dir_next);
        check(second->Signal(complete.Get(), 4), "directory signal");
        if (complete->GetCompletedValue() != 3) throw std::runtime_error("directory fence did not block");
        ctx.queue->ExecuteCommandLists(1, dir_first);
        check(ctx.queue->Signal(ready.Get(), 3), "directory producer signal");
        wait(complete.Get(), 4);
        auto check_directory = [&](UINT64 published) {
            D3D12_RANGE dir_range{0, 896}, empty{};
            void* dir_data{};
            check(directory_readback->Map(0, &dir_range, &dir_data), "directory readback");
            struct DirectoryResult { UINT64 backing; UINT status, reserved; };
            const auto* actual = static_cast<const DirectoryResult*>(dir_data);
            const UINT status_normal[] = {1, 1, 0, 2, 2, 2, 2, 2};
            const UINT status_empty[] = {0, 0, 0, 0, 2, 2, 2, 0};
            bool directory_matches = true;
            for (UINT batch : {0u, 1u, 2u, 3u, 5u}) for (UINT i = 0; i < 8; ++i) {
                const auto& r = actual[batch * 8 + i];
                const bool normal = batch == 0 || batch == 5;
                const UINT status = normal ? status_normal[i] : batch == 2 ? status_empty[i] : 3;
                const UINT64 backing = normal && i == 0 ? (batch == 0 ? published : entry_a[0][1]) : normal && i == 1 ? entry_a[1][1] : 0;
                if (r.status != status || r.backing != backing || r.reserved != 0) directory_matches = false;
            }
            for (UINT i = 0; i < 4; ++i) {
                const auto& r = actual[48 + i];
                if (r.status != 3 || r.backing != 0 || r.reserved != 0) directory_matches = false;
            }
            const auto* guard = static_cast<const UINT*>(dir_data);
            for (UINT i = 128; i < 160; ++i) if (guard[i] != seed_words[i]) directory_matches = false;
            for (UINT i = 208; i < 224; ++i) if (guard[i] != seed_words[i]) directory_matches = false;
            directory_readback->Unmap(0, &empty);
            if (!directory_matches) throw std::runtime_error("GPU directory publication or validation mismatch");
        };
        check_directory(entry_a[0][1]);
        // Restore initial states and replay the same closed lists with changed
        // upload contents. Reuse is legal only after the previous fence above.
        ComPtr<ID3D12CommandAllocator> dir_reset_allocator;
        ComPtr<ID3D12GraphicsCommandList> dir_reset;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&dir_reset_allocator)), "directory reset allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, dir_reset_allocator.Get(), nullptr, IID_PPV_ARGS(&dir_reset)), "directory reset list");
        transition(dir_reset.Get(), directory.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        transition(dir_reset.Get(), directory_result.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        submit_and_wait(ctx.device.Get(), ctx.queue.Get(), dir_reset.Get());
        upload(entry_a_upload.Get(), entry_b, sizeof(entry_b));
        check(second->Wait(ready.Get(), 4), "directory replay wait");
        second->ExecuteCommandLists(1, dir_next);
        check(second->Signal(complete.Get(), 5), "directory replay signal");
        if (complete->GetCompletedValue() != 4) throw std::runtime_error("directory replay fence did not block");
        ctx.queue->ExecuteCommandLists(1, dir_first);
        check(ctx.queue->Signal(ready.Get(), 4), "directory replay producer signal");
        wait(complete.Get(), 5);
        check_directory(entry_b[0][1]);
        std::fprintf(receipt, "body_bytes=%u\nsource_offset=%u\nsource_after_close=1\ncross_queue_wait=1\nblocked_fence_verified=1\ncompleted_list_replay=1\nreverse_submission=1\ngpu_directory_publication=1\ndirectory_entries=5\ndirectory_queries=44\ndirectory_short_output_guard=1\ndirectory_unaligned_backing=1\ndirectory_zero_query_guard=1\ndirectory_cross_queue=1\ndirectory_replay=1\n", body, offset + 64);
    } catch (const std::exception& e) {
        std::fprintf(receipt, "error=%s\n", e.what()); result = 1;
    }
    return std::fclose(receipt) ? 2 : result;
}
