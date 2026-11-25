// GPU control for the isolated Rust-owned, immutable serialized BLAS snapshot.
#include "../independent-shaders/context.h"
#include <algorithm>
#include "owned_shadow_dxil.h"
#include "verified_initialize_dxil.h"
#include "verified_compare_dxil.h"
#include "verified_commit_dxil.h"
#include "verified_finalize_dxil.h"
#include "guarded_shadow_dxil.h"
using namespace shader_control;
extern "C" int tb_shadow_create(ID3D12Device*, const unsigned char*, size_t, void**);
extern "C" int tb_shadow_update(void*, const unsigned char*, size_t);
extern "C" int tb_shadow_record_copy(void*, ID3D12GraphicsCommandList*);
extern "C" int tb_shadow_gpu_resources(const void*, ID3D12Resource**, ID3D12Resource**, UINT64*);
extern "C" int tb_shadow_retain_recording(void*);
extern "C" int tb_shadow_release_recording(void*);
extern "C" int tb_shadow_info(const void*, UINT64*, UINT64*);
extern "C" int tb_shadow_hold(void*, ID3D12Fence*, UINT64);
extern "C" int tb_shadow_try_release(void*);

static void wait(ID3D12Fence* fence, UINT64 value) {
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(value, event);
    DWORD status = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event); check(hr, "completion event");
    if (status != WAIT_OBJECT_0 || fence->GetCompletedValue() < value) throw std::runtime_error("fence wait");
}
int main() {
    FILE* receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    int result = 0;
    try {
        Context ctx(receipt);
        std::ifstream input("source.bin", std::ios::binary);
        std::vector<unsigned char> data{std::istreambuf_iterator<char>{input}, {}};
        if (data.size() < 128) throw std::runtime_error("short input");
        auto invalid = data;
        invalid[0] = 1;
        void* handle{};
        if (tb_shadow_create(ctx.device.Get(), invalid.data(), invalid.size(), &handle) != -2 || handle)
            throw std::runtime_error("invalid serialized header accepted");
        handle = reinterpret_cast<void*>(1);
        if (tb_shadow_create(ctx.device.Get(), data.data(), 127, &handle) != -1 || handle)
            throw std::runtime_error("short source accepted or stale output retained");
        invalid = data;
        UINT root_pointer{}; std::memcpy(&root_pointer, invalid.data() + 112, sizeof(root_pointer));
        const size_t node = 64 + (root_pointer >> 3) * 64;
        if ((root_pointer & 7) != 4 || node + 28 > invalid.size()) throw std::runtime_error("unexpected root");
        const unsigned short nan = 0x7e00;
        std::memcpy(invalid.data() + node + 22, &nan, sizeof(nan));
        if (tb_shadow_create(ctx.device.Get(), invalid.data(), invalid.size(), &handle) != -2 || handle)
            throw std::runtime_error("invalid node bounds accepted");
        if (tb_shadow_create(ctx.device.Get(), data.data(), data.size(), &handle) != 0 || !handle)
            throw std::runtime_error("owned shadow creation");
        UINT64 address{}, size{};
        if (tb_shadow_info(handle, &address, &size) != 0 || size != data.size()-64 || !address)
            throw std::runtime_error("shadow address or size");
        std::array<unsigned char,64> expected{};
        std::memcpy(expected.data(), data.data()+64, expected.size());
        data[64] ^= 0x5a; // must not mutate the immutable GPU snapshot
        auto output = buffer(ctx.device.Get(), 64, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto readback = buffer(ctx.device.Get(), 64, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        auto output2 = buffer(ctx.device.Get(), 64, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto readback2 = buffer(ctx.device.Get(), 64, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_COMMAND_QUEUE_DESC queue_desc{};
        ComPtr<ID3D12CommandQueue> queue2;
        check(ctx.device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue2)), "second queue");
        D3D12_ROOT_PARAMETER params[2]{};
        params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        D3D12_ROOT_SIGNATURE_DESC desc{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "root serialization");
        ComPtr<ID3D12RootSignature> root;
        check(ctx.device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pd{}; pd.pRootSignature = root.Get();
        pd.CS = {owned_shadow_dxil, sizeof(owned_shadow_dxil)};
        ComPtr<ID3D12PipelineState> pso;
        check(ctx.device->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pso)), "pipeline");
        ComPtr<ID3D12CommandAllocator> allocator; ComPtr<ID3D12GraphicsCommandList> list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pso.Get(), IID_PPV_ARGS(&list)), "list");
        if (tb_shadow_record_copy(nullptr, list.Get()) != -1 || tb_shadow_record_copy(handle, nullptr) != -1)
            throw std::runtime_error("invalid copy args accepted");
        if (tb_shadow_record_copy(handle, list.Get()) != 0) throw std::runtime_error("record shadow copy");
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released while copy list is recorded");
        list->SetComputeRootSignature(root.Get());
        list->SetComputeRootShaderResourceView(0, address);
        list->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
        list->Dispatch(1,1,1);
        transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, 64);
        check(list->Close(), "close");
        ComPtr<ID3D12CommandAllocator> allocator2; ComPtr<ID3D12GraphicsCommandList> list2;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator2)), "second allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator2.Get(), pso.Get(), IID_PPV_ARGS(&list2)), "second list");
        if (tb_shadow_retain_recording(handle) != 0) throw std::runtime_error("retain second recording");
        list2->SetComputeRootSignature(root.Get());
        list2->SetComputeRootShaderResourceView(0, address);
        list2->SetComputeRootUnorderedAccessView(1, output2->GetGPUVirtualAddress());
        list2->Dispatch(1,1,1);
        transition(list2.Get(), output2.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        list2->CopyBufferRegion(readback2.Get(), 0, output2.Get(), 0, 64);
        check(list2->Close(), "close second list");
        ComPtr<ID3D12Fence> gate, gate2, complete, extra;
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate)), "gate");
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate2)), "second gate");
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&complete)), "complete");
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&extra)), "extra");
        if (tb_shadow_hold(handle, complete.Get(), 1) || tb_shadow_hold(handle, extra.Get(), 1))
            throw std::runtime_error("register holds");
        if (tb_shadow_hold(handle, complete.Get(), 0) != -1 || tb_shadow_hold(handle, complete.Get(), ~UINT64(0)) != -1)
            throw std::runtime_error("reserved fence value accepted");
        check(ctx.queue->Wait(gate.Get(), 1), "queue wait");
        ID3D12CommandList* lists[] = {list.Get()};
        ctx.queue->ExecuteCommandLists(1, lists);
        check(ctx.queue->Signal(complete.Get(), 1), "queue signal");
        check(queue2->Wait(gate2.Get(), 1), "second queue wait");
        check(queue2->Wait(complete.Get(), 1), "wait for default heap copy");
        ID3D12CommandList* lists2[] = {list2.Get()};
        queue2->ExecuteCommandLists(1, lists2);
        check(queue2->Signal(extra.Get(), 1), "second queue signal");
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released while GPU blocked");
        if (tb_shadow_update(handle, data.data(), data.size()) != -5) throw std::runtime_error("updated upload during GPU work");
        check(gate->Signal(1), "release GPU gate");
        wait(complete.Get(), 1);
        D3D12_RANGE range{0,64}, none{}; void* mapped{};
        check(readback->Map(0, &range, &mapped), "readback");
        bool identical = std::memcmp(mapped, expected.data(), 64) == 0;
        readback->Unmap(0, &none);
        if (!identical) throw std::runtime_error("GPU snapshot differed");
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released with second fence pending");
        check(gate2->Signal(1), "release second GPU gate");
        wait(extra.Get(), 1);
        check(readback2->Map(0, &range, &mapped), "second readback");
        identical = std::memcmp(mapped, expected.data(), 64) == 0;
        readback2->Unmap(0, &none);
        if (!identical) throw std::runtime_error("second GPU snapshot differed");
        // The source visible at the next submission can change. Validate and
        // stage it only after all prior uses complete; recorded copy size stays fixed.
        data[64] ^= 0x5a; // restore original user bytes
        if (tb_shadow_update(handle, invalid.data(), invalid.size()) != -2) throw std::runtime_error("invalid update accepted");
        if (tb_shadow_update(handle, data.data(), data.size() - 1) != -1) throw std::runtime_error("changed size accepted");
        auto updated = data;
        float lower{}; std::memcpy(&lower, updated.data() + 80, sizeof(lower));
        lower -= 1.0f;
        std::memcpy(updated.data() + 80, &lower, sizeof(lower));
        if (tb_shadow_update(handle, updated.data(), updated.size()) != 0) throw std::runtime_error("valid update rejected");
        std::memcpy(expected.data(), updated.data() + 64, expected.size());
        updated[64] ^= 0x5a; // staging owns a copy independent of the caller
        // Replay the same closed copy-and-read list after both queues finish.
        // Restore the output state and register the new completion before use.
        ComPtr<ID3D12CommandAllocator> reset_allocator;
        ComPtr<ID3D12GraphicsCommandList> reset_list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&reset_allocator)), "reset allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, reset_allocator.Get(), nullptr, IID_PPV_ARGS(&reset_list)), "reset list");
        transition(reset_list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        submit_and_wait(ctx.device.Get(), ctx.queue.Get(), reset_list.Get());
        if (tb_shadow_hold(handle, complete.Get(), 2) != 0) throw std::runtime_error("replay hold");
        check(ctx.queue->Wait(gate.Get(), 2), "replay gate");
        ctx.queue->ExecuteCommandLists(1, lists);
        check(ctx.queue->Signal(complete.Get(), 2), "replay completion");
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released during copy replay");
        check(gate->Signal(2), "release replay gate");
        wait(complete.Get(), 2);
        check(readback->Map(0, &range, &mapped), "replay readback");
        identical = std::memcmp(mapped, expected.data(), 64) == 0;
        readback->Unmap(0, &none);
        if (!identical) throw std::runtime_error("replayed GPU copy differed");
        // GPU-source control: an earlier copy populates the source after Close.
        // A parallel GPU threads verify every DWORD against the validated
        // private snapshot before copying any bytes into the shadow.
        updated[64] ^= 0x5a;
        if ((size & 3) || size > 64 * 1024 * 1024) throw std::runtime_error("verification control size unsupported");
        ID3D12Resource* shadow_resource{}; ID3D12Resource* validation_resource{}; UINT64 validation_address{};
        if (tb_shadow_gpu_resources(handle, &shadow_resource, &validation_resource, &validation_address) || !shadow_resource || !validation_resource || !validation_address)
            throw std::runtime_error("verification resources");
        auto guest_upload = buffer(ctx.device.Get(), static_cast<UINT>(size + 256), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto guest_gpu = buffer(ctx.device.Get(), static_cast<UINT>(size + 128), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        auto verify_status = buffer(ctx.device.Get(), 36, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto verify_output = buffer(ctx.device.Get(), 64, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto verify_readback = buffer(ctx.device.Get(), 100, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        auto verify_body_readback = buffer(ctx.device.Get(), static_cast<UINT>(size), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        // Separate raw views deliberately include source, validation and
        // destination ranges one DWORD short for the negative controls.
        D3D12_DESCRIPTOR_HEAP_DESC verify_heap_desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 26, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
        ComPtr<ID3D12DescriptorHeap> verify_heap;
        check(ctx.device->CreateDescriptorHeap(&verify_heap_desc, IID_PPV_ARGS(&verify_heap)), "verification heap");
        const UINT verify_stride = ctx.device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
        auto verify_cpu = verify_heap->GetCPUDescriptorHandleForHeapStart();
        D3D12_SHADER_RESOURCE_VIEW_DESC verify_srv{};
        verify_srv.Format = DXGI_FORMAT_R32_TYPELESS;
        verify_srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        verify_srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        verify_srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        for (UINT pair = 0; pair < 4; ++pair) {
            verify_srv.Buffer.FirstElement = 128 / 4;
            verify_srv.Buffer.NumElements = static_cast<UINT>(size / 4) - (pair == 1 ? 1 : 0);
            ctx.device->CreateShaderResourceView(guest_gpu.Get(), &verify_srv, verify_cpu);
            verify_cpu.ptr += verify_stride;
            verify_srv.Buffer.FirstElement = 0;
            verify_srv.Buffer.NumElements = static_cast<UINT>(size / 4) - (pair == 2 ? 1 : 0);
            ctx.device->CreateShaderResourceView(validation_resource, &verify_srv, verify_cpu);
            verify_cpu.ptr += verify_stride;
        }
        for (UINT n = 0; n < 9; ++n) {
            D3D12_UNORDERED_ACCESS_VIEW_DESC view{};
            view.Format = DXGI_FORMAT_R32_TYPELESS;
            view.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
            view.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
            view.Buffer.NumElements = static_cast<UINT>(size / 4) - (n == 3 ? 1 : 0);
            ctx.device->CreateUnorderedAccessView(shadow_resource, nullptr, &view, verify_cpu);
            verify_cpu.ptr += verify_stride;
            view.Buffer.FirstElement = n;
            view.Buffer.NumElements = 1;
            ctx.device->CreateUnorderedAccessView(verify_status.Get(), nullptr, &view, verify_cpu);
            verify_cpu.ptr += verify_stride;
        }
        D3D12_DESCRIPTOR_RANGE verify_ranges[2]{};
        verify_ranges[0] = {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 2, 0, 0, 0};
        verify_ranges[1] = {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 2, 0, 0, 0};
        D3D12_ROOT_PARAMETER verify_params[3]{};
        for (UINT n = 0; n < 2; ++n) {
            verify_params[n].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
            verify_params[n].DescriptorTable = {1, &verify_ranges[n]};
        }
        verify_params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
        verify_params[2].Constants.Num32BitValues = 3;
        D3D12_ROOT_SIGNATURE_DESC verify_desc{3, verify_params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> verify_blob;
        check(D3D12SerializeRootSignature(&verify_desc, D3D_ROOT_SIGNATURE_VERSION_1, &verify_blob, nullptr), "verification root serialization");
        ComPtr<ID3D12RootSignature> verify_root;
        check(ctx.device->CreateRootSignature(0, verify_blob->GetBufferPointer(), verify_blob->GetBufferSize(), IID_PPV_ARGS(&verify_root)), "verification root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC verify_pd{}; verify_pd.pRootSignature = verify_root.Get();
        const D3D12_SHADER_BYTECODE verify_codes[] = {
            {verified_initialize_dxil, sizeof(verified_initialize_dxil)},
            {verified_compare_dxil, sizeof(verified_compare_dxil)},
            {verified_commit_dxil, sizeof(verified_commit_dxil)},
            {verified_finalize_dxil, sizeof(verified_finalize_dxil)},
        };
        ComPtr<ID3D12PipelineState> verify_pso[4];
        for (UINT n = 0; n < 4; ++n) {
            verify_pd.CS = verify_codes[n];
            check(ctx.device->CreateComputePipelineState(&verify_pd, IID_PPV_ARGS(&verify_pso[n])), "verification pipeline");
        }
        D3D12_ROOT_PARAMETER guarded_params[3]{};
        guarded_params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        guarded_params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
        guarded_params[1].Descriptor.ShaderRegister = 1;
        guarded_params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
        D3D12_ROOT_SIGNATURE_DESC guarded_desc{3, guarded_params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> guarded_blob;
        check(D3D12SerializeRootSignature(&guarded_desc, D3D_ROOT_SIGNATURE_VERSION_1, &guarded_blob, nullptr), "guarded root serialization");
        ComPtr<ID3D12RootSignature> guarded_root;
        check(ctx.device->CreateRootSignature(0, guarded_blob->GetBufferPointer(), guarded_blob->GetBufferSize(), IID_PPV_ARGS(&guarded_root)), "guarded root");
        D3D12_COMPUTE_PIPELINE_STATE_DESC guarded_pd{}; guarded_pd.pRootSignature = guarded_root.Get();
        guarded_pd.CS = {guarded_shadow_dxil, sizeof(guarded_shadow_dxil)};
        ComPtr<ID3D12PipelineState> guarded_pso;
        check(ctx.device->CreateComputePipelineState(&guarded_pd, IID_PPV_ARGS(&guarded_pso)), "guarded pipeline");
        ComPtr<ID3D12CommandAllocator> verify_allocator; ComPtr<ID3D12GraphicsCommandList> verify_list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&verify_allocator)), "verification allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, verify_allocator.Get(), nullptr, IID_PPV_ARGS(&verify_list)), "verification list");
        if (tb_shadow_retain_recording(handle) != 0) throw std::runtime_error("verification recording hold");
        transition(verify_list.Get(), guest_gpu.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        verify_list->CopyBufferRegion(guest_gpu.Get(), 128, guest_upload.Get(), 256, size);
        transition(verify_list.Get(), guest_gpu.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(verify_list.Get(), shadow_resource, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        verify_list->SetComputeRootSignature(verify_root.Get());
        ID3D12DescriptorHeap* verify_heaps[] = {verify_heap.Get()};
        verify_list->SetDescriptorHeaps(1, verify_heaps);
        auto verify_gpu = verify_heap->GetGPUDescriptorHandleForHeapStart();
        const UINT verify_sizes[] = {static_cast<UINT>(size), static_cast<UINT>(size), static_cast<UINT>(size),
            static_cast<UINT>(size), 0, static_cast<UINT>(size - 1), static_cast<UINT>(size + 4),
            static_cast<UINT>(size), static_cast<UINT>(size)};
        for (UINT n = 0; n < 9; ++n) {
            const UINT input_pair = n == 1 ? 1 : n == 2 ? 2 : 0;
            auto inputs = verify_gpu; inputs.ptr += UINT64(verify_stride) * (2 * input_pair);
            auto outputs = verify_gpu; outputs.ptr += UINT64(verify_stride) * (8 + 2 * n);
            verify_list->SetComputeRootDescriptorTable(0, inputs);
            verify_list->SetComputeRootDescriptorTable(1, outputs);
            const UINT groups = static_cast<UINT>((size + 255) / 256);
            const UINT groups_x = std::min(groups, 65535u);
            const UINT groups_y = (groups + groups_x - 1) / groups_x;
            const UINT actual_x = n == 7 ? groups_x - 1 : groups_x;
            const UINT constants[] = {verify_sizes[n], n == 8 ? 0u : actual_x, groups_y};
            verify_list->SetComputeRoot32BitConstants(2, 3, constants, 0);
            for (UINT stage = 0; stage < 4; ++stage) {
                verify_list->SetPipelineState(verify_pso[stage].Get());
                verify_list->Dispatch(stage == 1 || stage == 2 ? actual_x : 1,
                                      stage == 1 || stage == 2 ? groups_y : 1, 1);
                D3D12_RESOURCE_BARRIER uav{};
                uav.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
                verify_list->ResourceBarrier(1, &uav);
            }
        }
        transition(verify_list.Get(), shadow_resource, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(verify_list.Get(), verify_status.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        verify_list->SetComputeRootSignature(guarded_root.Get());
        verify_list->SetPipelineState(guarded_pso.Get());
        verify_list->SetComputeRootShaderResourceView(0, address);
        verify_list->SetComputeRootShaderResourceView(1, verify_status->GetGPUVirtualAddress());
        verify_list->SetComputeRootUnorderedAccessView(2, verify_output->GetGPUVirtualAddress());
        verify_list->Dispatch(1, 1, 1);
        transition(verify_list.Get(), verify_status.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_SOURCE);
        verify_list->CopyBufferRegion(verify_readback.Get(), 0, verify_status.Get(), 0, 36);
        transition(verify_list.Get(), verify_output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        verify_list->CopyBufferRegion(verify_readback.Get(), 36, verify_output.Get(), 0, 64);
        transition(verify_list.Get(), shadow_resource, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_SOURCE);
        verify_list->CopyBufferRegion(verify_body_readback.Get(), 0, shadow_resource, 0, size);
        transition(verify_list.Get(), shadow_resource, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        check(verify_list->Close(), "close verification list");
        std::vector<unsigned char> source_data(256 + size);
        std::memcpy(source_data.data() + 256, updated.data() + 64, size);
        upload(guest_upload.Get(), source_data.data(), source_data.size());
        auto inspect_verification = [&](UINT expected_status) {
            D3D12_RANGE read{0,100}, no_write{}; void* bytes{};
            check(verify_readback->Map(0, &read, &bytes), "verification readback");
            UINT statuses[9]{}; std::memcpy(statuses, bytes, sizeof(statuses));
            std::array<UINT,16> failure_marker{}; failure_marker.fill(0xdeadbeefu);
            const void* expected_output = expected_status == 1 ? static_cast<const void*>(expected.data()) : static_cast<const void*>(failure_marker.data());
            bool same = statuses[0] == expected_status && std::all_of(statuses + 1, statuses + 9, [](UINT status) { return status == 3; }) &&
                std::memcmp(static_cast<unsigned char*>(bytes) + 36, expected_output, 64) == 0;
            verify_readback->Unmap(0, &no_write);
            D3D12_RANGE full{0, static_cast<SIZE_T>(size)};
            check(verify_body_readback->Map(0, &full, &bytes), "verification body readback");
            same = same && std::memcmp(bytes, updated.data() + 64, size) == 0;
            verify_body_readback->Unmap(0, &no_write);
            if (!same) throw std::runtime_error("GPU source validation mismatch");
        };
        if (tb_shadow_hold(handle, complete.Get(), 3) != 0) throw std::runtime_error("verification hold");
        ID3D12CommandList* verification[] = {verify_list.Get()};
        ctx.queue->ExecuteCommandLists(1, verification);
        check(ctx.queue->Signal(complete.Get(), 3), "verification signal");
        wait(complete.Get(), 3);
        inspect_verification(1);
        // Reset the transient states, then mutate only the last DWORD after the
        // list was closed. Mismatch must not modify even the old shadow header.
        ComPtr<ID3D12CommandAllocator> verify_reset_allocator;
        ComPtr<ID3D12GraphicsCommandList> verify_reset;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&verify_reset_allocator)), "verification reset allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, verify_reset_allocator.Get(), nullptr, IID_PPV_ARGS(&verify_reset)), "verification reset list");
        transition(verify_reset.Get(), verify_status.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        transition(verify_reset.Get(), verify_output.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        submit_and_wait(ctx.device.Get(), ctx.queue.Get(), verify_reset.Get());
        source_data.back() ^= 0x5a;
        upload(guest_upload.Get(), source_data.data(), source_data.size());
        if (tb_shadow_hold(handle, complete.Get(), 4) != 0) throw std::runtime_error("verification replay hold");
        ctx.queue->ExecuteCommandLists(1, verification);
        check(ctx.queue->Signal(complete.Get(), 4), "verification replay signal");
        wait(complete.Get(), 4);
        inspect_verification(2);
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released while lists can replay");
        ComPtr<ID3D12Fence> final_fence;
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&final_fence)), "final fence");
        if (tb_shadow_hold(handle, final_fence.Get(), 1) != 0) throw std::runtime_error("final hold");
        list.Reset(); list2.Reset(); verify_list.Reset(); // no recorded list can be replayed now
        if (tb_shadow_release_recording(handle) != 0 || tb_shadow_release_recording(handle) != 0 ||
            tb_shadow_release_recording(handle) != 0 || tb_shadow_release_recording(handle) != -2) throw std::runtime_error("recording release mismatch");
        if (tb_shadow_try_release(handle) != 0) throw std::runtime_error("released with private fence pending");
        check(final_fence->Signal(1), "final fence signal");
        if (tb_shadow_try_release(handle) != 1) throw std::runtime_error("completed resource not retired");
        // Separate case: reset the recorded list while its GPU copy is blocked.
        // The recording reference can go away, but the pending private fence
        // must still retain both source and destination resources.
        void* pending_handle{};
        if (tb_shadow_create(ctx.device.Get(), data.data(), data.size(), &pending_handle) != 0)
            throw std::runtime_error("pending shadow creation");
        ComPtr<ID3D12CommandAllocator> pending_allocator, replacement_allocator;
        ComPtr<ID3D12GraphicsCommandList> pending_list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&pending_allocator)), "pending allocator");
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&replacement_allocator)), "replacement allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, pending_allocator.Get(), nullptr, IID_PPV_ARGS(&pending_list)), "pending list");
        if (tb_shadow_record_copy(pending_handle, pending_list.Get()) != 0) throw std::runtime_error("pending copy");
        check(pending_list->Close(), "close pending list");
        ComPtr<ID3D12Fence> pending_gate, pending_done;
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&pending_gate)), "pending gate");
        check(ctx.device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&pending_done)), "pending done");
        if (tb_shadow_hold(pending_handle, pending_done.Get(), 1) != 0) throw std::runtime_error("pending hold");
        check(ctx.queue->Wait(pending_gate.Get(), 1), "pending wait");
        ID3D12CommandList* pending_lists[] = {pending_list.Get()};
        ctx.queue->ExecuteCommandLists(1, pending_lists);
        check(ctx.queue->Signal(pending_done.Get(), 1), "pending signal");
        check(pending_list->Reset(replacement_allocator.Get(), nullptr), "reset pending list");
        check(pending_list->Close(), "close reset pending list");
        if (tb_shadow_release_recording(pending_handle) != 0) throw std::runtime_error("release pending recording");
        if (tb_shadow_try_release(pending_handle) != 0) throw std::runtime_error("released in-flight GPU copy");
        check(pending_gate->Signal(1), "unblock pending copy");
        wait(pending_done.Get(), 1);
        if (tb_shadow_try_release(pending_handle) != 1) throw std::runtime_error("pending shadow not retired");
        // Synthetic 128 KiB body: preserve the verified sphere graph, append
        // unused blocks, and update the serialized and BVH length fields.
        auto large = data;
        constexpr UINT large_body = 128 * 1024;
        large.resize(64 + large_body, 0);
        const UINT64 large_total = large.size(), large_bytes = large_body;
        const UINT large_blocks_minus_one = large_body / 64 - 1;
        std::memcpy(large.data() + 32, &large_total, 8);
        std::memcpy(large.data() + 40, &large_bytes, 8);
        std::memcpy(large.data() + 68, &large_blocks_minus_one, 4);
        void* large_handle{};
        if (tb_shadow_create(ctx.device.Get(), large.data(), large.size(), &large_handle) != 0)
            throw std::runtime_error("large validated shadow creation");
        UINT64 large_address{}, large_size{}, large_validation_address{};
        ID3D12Resource *large_shadow{}, *large_validation{};
        if (tb_shadow_info(large_handle, &large_address, &large_size) || large_size != large_body ||
            tb_shadow_gpu_resources(large_handle, &large_shadow, &large_validation, &large_validation_address))
            throw std::runtime_error("large shadow information");
        auto large_source_upload = buffer(ctx.device.Get(), large_body, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        auto large_source = buffer(ctx.device.Get(), large_body, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        auto large_status = buffer(ctx.device.Get(), 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        auto large_readback = buffer(ctx.device.Get(), large_body + 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        D3D12_DESCRIPTOR_HEAP_DESC large_hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 4, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
        ComPtr<ID3D12DescriptorHeap> large_heap;
        check(ctx.device->CreateDescriptorHeap(&large_hd, IID_PPV_ARGS(&large_heap)), "large heap");
        auto large_cpu = large_heap->GetCPUDescriptorHandleForHeapStart();
        D3D12_SHADER_RESOURCE_VIEW_DESC large_srv{};
        large_srv.Format = DXGI_FORMAT_R32_TYPELESS;
        large_srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        large_srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        large_srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        large_srv.Buffer.NumElements = large_body / 4;
        ctx.device->CreateShaderResourceView(large_source.Get(), &large_srv, large_cpu);
        large_cpu.ptr += verify_stride;
        ctx.device->CreateShaderResourceView(large_validation, &large_srv, large_cpu);
        large_cpu.ptr += verify_stride;
        D3D12_UNORDERED_ACCESS_VIEW_DESC large_uav{};
        large_uav.Format = DXGI_FORMAT_R32_TYPELESS;
        large_uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        large_uav.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
        large_uav.Buffer.NumElements = large_body / 4;
        ctx.device->CreateUnorderedAccessView(large_shadow, nullptr, &large_uav, large_cpu);
        large_cpu.ptr += verify_stride;
        large_uav.Buffer.NumElements = 1;
        ctx.device->CreateUnorderedAccessView(large_status.Get(), nullptr, &large_uav, large_cpu);
        ComPtr<ID3D12CommandAllocator> large_allocator; ComPtr<ID3D12GraphicsCommandList> large_list;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&large_allocator)), "large allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, large_allocator.Get(), nullptr, IID_PPV_ARGS(&large_list)), "large list");
        if (tb_shadow_retain_recording(large_handle) != 0) throw std::runtime_error("large recording hold");
        transition(large_list.Get(), large_source.Get(), D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        large_list->CopyBufferRegion(large_source.Get(), 0, large_source_upload.Get(), 0, large_body);
        transition(large_list.Get(), large_source.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        transition(large_list.Get(), large_shadow, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        large_list->SetComputeRootSignature(verify_root.Get());
        ID3D12DescriptorHeap* large_heaps[] = {large_heap.Get()}; large_list->SetDescriptorHeaps(1, large_heaps);
        auto large_gpu = large_heap->GetGPUDescriptorHandleForHeapStart();
        large_list->SetComputeRootDescriptorTable(0, large_gpu);
        large_gpu.ptr += UINT64(verify_stride) * 2;
        large_list->SetComputeRootDescriptorTable(1, large_gpu);
        constexpr UINT large_groups = (large_body + 255) / 256;
        constexpr UINT large_groups_x = 16, large_groups_y = large_groups / large_groups_x;
        const UINT large_constants[] = {large_body, large_groups_x, large_groups_y};
        large_list->SetComputeRoot32BitConstants(2, 3, large_constants, 0);
        for (UINT stage = 0; stage < 4; ++stage) {
            large_list->SetPipelineState(verify_pso[stage].Get());
            large_list->Dispatch(stage == 1 || stage == 2 ? large_groups_x : 1,
                                 stage == 1 || stage == 2 ? large_groups_y : 1, 1);
            D3D12_RESOURCE_BARRIER uav{}; uav.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
            large_list->ResourceBarrier(1, &uav);
        }
        transition(large_list.Get(), large_status.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        large_list->CopyBufferRegion(large_readback.Get(), 0, large_status.Get(), 0, 4);
        transition(large_list.Get(), large_shadow, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        large_list->CopyBufferRegion(large_readback.Get(), 4, large_shadow, 0, large_body);
        transition(large_list.Get(), large_shadow, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE);
        check(large_list->Close(), "close large list");
        upload(large_source_upload.Get(), large.data() + 64, large_body);
        if (tb_shadow_hold(large_handle, complete.Get(), 5)) throw std::runtime_error("large fence hold");
        ID3D12CommandList* large_lists[] = {large_list.Get()};
        ctx.queue->ExecuteCommandLists(1, large_lists);
        check(ctx.queue->Signal(complete.Get(), 5), "large signal"); wait(complete.Get(), 5);
        auto inspect_large = [&](UINT want) {
            D3D12_RANGE read{0, large_body + 4}, no_write{}; void* bytes{};
            check(large_readback->Map(0, &read, &bytes), "large readback");
            UINT status{}; std::memcpy(&status, bytes, 4);
            bool matches = std::memcmp(static_cast<unsigned char*>(bytes) + 4, large.data() + 64, large_body) == 0;
            large_readback->Unmap(0, &no_write);
            if (status != want || !matches) throw std::runtime_error("large parallel verification mismatch");
        };
        inspect_large(1);
        ComPtr<ID3D12CommandAllocator> large_reset_allocator; ComPtr<ID3D12GraphicsCommandList> large_reset;
        check(ctx.device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&large_reset_allocator)), "large reset allocator");
        check(ctx.device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, large_reset_allocator.Get(), nullptr, IID_PPV_ARGS(&large_reset)), "large reset list");
        transition(large_reset.Get(), large_status.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        submit_and_wait(ctx.device.Get(), ctx.queue.Get(), large_reset.Get());
        auto altered_large = large;
        altered_large.back() ^= 0x5a;
        upload(large_source_upload.Get(), altered_large.data() + 64, large_body);
        if (tb_shadow_hold(large_handle, complete.Get(), 6)) throw std::runtime_error("large replay hold");
        ctx.queue->ExecuteCommandLists(1, large_lists);
        check(ctx.queue->Signal(complete.Get(), 6), "large replay signal"); wait(complete.Get(), 6);
        inspect_large(2);
        large_list.Reset();
        if (tb_shadow_release_recording(large_handle) || tb_shadow_try_release(large_handle) != 1)
            throw std::runtime_error("large shadow retirement");
        std::fprintf(receipt, "large_parallel_body=131072\nlarge_last_dword_mismatch=1\nmultidimensional_dispatch=1\nunderdispatch_rejected=1\ngpu_written_source_verified=1\nlast_dword_mismatch_preserves_shadow=1\nguarded_consumer_rejects_mismatch=1\nparallel_verification=1\nbounded_views_reject_invalid_sizes=1\ninflight_copy_after_list_reset=1\nvalidated_source_update=1\ninflight_update_rejected=1\nimmutable_gpu_snapshot=1\nmalformed_rejected=1\nblocked_gpu_retained=1\ntwo_fence_retirement=1\ntwo_gpu_queues=1\ndefault_heap_gpu_copy=1\ncompleted_list_copy_replay=1\nclosed_list_retention=1\nbody_bytes=%llu\n", size);
    } catch (const std::exception& e) { std::fprintf(receipt, "error=%s\n", e.what()); result = 1; }
    return std::fclose(receipt) ? 2 : result;
}
