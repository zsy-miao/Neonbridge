// Exercise the command signatures and indirect-dispatch call used by Frosty.
#include "frame_abi.h"
#include "frame_cli.h"
#include "compute_fixture.h"
#include "affine.h"
#include "affine_accumulate.h"
#include <cstring>

namespace {
using namespace frame_abi;
constexpr std::array<UINT, 18> profile_20004{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
using Pipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                   UINT, const void*, REFIID, void**);
using SetRoot = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using SetConstants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
using SetUav = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
using ComputeBudget = void (WINAPI*)(ID3D12GraphicsCommandList*, const UINT*);
using Flush = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT64, UINT64);
using Indirect = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12CommandSignature*, UINT,
                                ID3D12Resource*, UINT64, ID3D12Resource*, UINT64);
using DriverHint = UINT64 (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, UINT64);


HRESULT signature(ID3D12Device* device, UINT kind, UINT stride, ID3D12RootSignature* root,
                  ComPtr<ID3D12CommandSignature>& result) {
    D3D12_INDIRECT_ARGUMENT_DESC arg{};
    static_assert(sizeof(arg.Type) == sizeof(kind));
    std::memcpy(&arg.Type, &kind, sizeof(kind));
    D3D12_COMMAND_SIGNATURE_DESC desc{stride, 1, &arg, 1};
    HRESULT hr = device->CreateCommandSignature(&desc, root, IID_PPV_ARGS(result.ReleaseAndGetAddressOf()));
    if (SUCCEEDED(hr)) {
        if (!result) throw std::runtime_error("missing command signature");
        ComPtr<ID3D12Device> parent;
        check(result->GetDevice(IID_PPV_ARGS(&parent)), "command signature GetDevice");
        if (parent.Get() != device) throw std::runtime_error("incorrect command signature parent");
    } else if (result) {
        throw std::runtime_error("failed creation returned a signature");
    }
    return hr;
}

using compute_fixture::buffer;
using compute_fixture::upload;
using compute_fixture::transition;

std::string run(const wchar_t* requested, bool scarlett) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected, profile_20004.data(),
                                scarlett ? __uuidof(ID3D12Device11) : __uuidof(ID3D12Device));
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[0].Constants = {5, 2, 4};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {3, 4};
    D3D12_ROOT_SIGNATURE_DESC desc{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create root");

    ComPtr<ID3D12CommandSignature> compute, other;
    check(signature(device.Get(), 0, 16, nullptr, other), "draw signature");
    check(signature(device.Get(), 1, 20, nullptr, other), "indexed draw signature");
    check(signature(device.Get(), 2, 12, nullptr, other), "standard dispatch signature");
    check(signature(device.Get(), 105, 12, nullptr, compute), "Xbox dispatch signature");
    const auto mesh = signature(device.Get(), 107, 16, nullptr, other);
    const auto ray = signature(device.Get(), 9, 104, root.Get(), other);
    if (signature(device.Get(), 9, 112, root.Get(), other) != E_INVALIDARG ||
        signature(device.Get(), 1, 16, nullptr, other) != E_INVALIDARG ||
        signature(device.Get(), 106, 12, nullptr, other) != E_NOTIMPL)
        throw std::runtime_error("unexpected result for unrecognized signature layout");
    DXGI_ADAPTER_DESC1 other_adapter{};
    auto second = create_device(requested, other_adapter, profile_20004.data());
    if (signature(second.Get(), 9, 104, root.Get(), other) != E_INVALIDARG)
        throw std::runtime_error("foreign ray root accepted");
    check(signature(second.Get(), 105, 12, nullptr, other), "second device signature");

    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {affine_dxil, sizeof(affine_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<Pipeline>(device.Get(), 62)(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&pipeline)),
          "Xbox compute pipeline");
    using ScratchInfo = HRESULT (WINAPI *)(ID3D12PipelineState*, SIZE_T, SIZE_T, void**);
    void* scratch = device.Get();
    if (method<ScratchInfo>(pipeline.Get(), 11)(pipeline.Get(), 0, 0, &scratch) != E_NOTIMPL || scratch)
        throw std::runtime_error("Xbox pipeline scratch query");
    shader.CS = {accumulate_dxil, sizeof(accumulate_dxil)};
    ComPtr<ID3D12PipelineState> accumulate;
    check(method<Pipeline>(device.Get(), 62)(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&accumulate)),
          "dependent compute pipeline");
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    queue_desc.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    ComPtr<ID3D12CommandQueue> queue;
    check(method<CreateQueue>(device.Get(), 48)(device.Get(), &queue_desc, IID_PPV_ARGS(&queue)), "Xbox compute queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "compute allocator");
    const UINT list_desc[] = {static_cast<UINT>(queue_desc.Type), 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device.Get(), 73)(device.Get(), list_desc, allocator.Get(), nullptr, IID_PPV_ARGS(&list)),
          "Xbox compute list");

    std::array<UINT, 64> expected;
    expected.fill(0xcdcdcdcd);
    const UINT records[] = {0, 0, 0, 1, 1, 1, 3, 1, 1, 1, 0};
    auto args = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, sizeof(records), D3D12_RESOURCE_STATE_GENERIC_READ);
    auto seed = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, sizeof(expected), D3D12_RESOURCE_STATE_GENERIC_READ);
    auto out = buffer(device.Get(), D3D12_HEAP_TYPE_DEFAULT, sizeof(expected), D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto result = buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, sizeof(expected), D3D12_RESOURCE_STATE_COPY_DEST);
    upload(args.Get(), records, sizeof(records));
    upload(seed.Get(), expected.data(), sizeof(expected));
    const auto indirect = method<Indirect>(list.Get(), scarlett ? 61 : 59);
    // Explicitly exercise the two ABI layouts. The Scarlett tuning hint
    // is intentionally a no-op and must not poison command recording.
    const auto hint = scarlett ? method<DriverHint>(list.Get(), 101) : nullptr;
    auto reject = [&](ID3D12CommandSignature* sig, UINT64 offset, ID3D12Resource* count, UINT64 count_offset) {
        check(list->Reset(allocator.Get(), pipeline.Get()), "reset negative list");
        indirect(list.Get(), sig, 1, args.Get(), offset, count, count_offset);
        if (list->Close() != E_INVALIDARG) throw std::runtime_error("invalid ExecuteIndirect accepted");
    };
    reject(compute.Get(), sizeof(records) - 4, nullptr, 0);
    reject(compute.Get(), 1, nullptr, 0);
    reject(compute.Get(), 0, args.Get(), sizeof(records));
    reject(other.Get(), 0, nullptr, 0);
    const auto flush = method<Flush>(list.Get(), 67);
    auto reject_flush = [&](UINT mask, UINT64 va, UINT64 size, HRESULT expected_hr) {
        check(list->Reset(allocator.Get(), pipeline.Get()), "reset invalid cache flush list");
        flush(list.Get(), mask, va, size);
        if (list->Close() != expected_hr) throw std::runtime_error("invalid cache flush result");
    };
    reject_flush(UINT_MAX, 0, 512, E_NOTIMPL);
    reject_flush(0x3000, UINT64_MAX - 255, 512, E_INVALIDARG);
    const auto budget = method<ComputeBudget>(list.Get(), 96);
    std::array<UINT, 6> budget_record{};
    budget_record.back() = 1;
    check(list->Reset(allocator.Get(), pipeline.Get()), "reset nondefault budget list");
    budget(list.Get(), budget_record.data());
    if (list->Close() != E_NOTIMPL) throw std::runtime_error("nondefault compute budget accepted");
    check(list->Reset(allocator.Get(), pipeline.Get()), "reset null budget list");
    budget(list.Get(), nullptr);
    if (list->Close() != E_INVALIDARG) throw std::runtime_error("null compute budget accepted");
    budget_record.back() = 0;
    check(list->Reset(allocator.Get(), pipeline.Get()), "reset compute list");
    list->CopyBufferRegion(out.Get(), 0, seed.Get(), 0, sizeof(expected));
    transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    method<SetRoot>(list.Get(), 33)(list.Get(), root.Get());
    method<SetUav>(list.Get(), 45)(list.Get(), 1, out->GetGPUVirtualAddress());
    auto dispatch = [&](UINT first, UINT64 offset, UINT max_count, ID3D12Resource* count, UINT64 count_offset,
                        UINT expected_words) {
        const UINT constants[] = {first + 3, 11, first, 24};
        method<SetConstants>(list.Get(), 39)(list.Get(), 0, 4, constants, 0);
        indirect(list.Get(), compute.Get(), max_count, args.Get(), offset, count, count_offset);
        for (UINT i = 0; i < expected_words; ++i) expected[first + i] = constants[0] + i * constants[1];
    };
    budget(list.Get(), budget_record.data());
    if (hint && hint(list.Get(), 2, 1) != 0)
        throw std::runtime_error("unexpected DriverHintX result");
    dispatch(0, 24, 1, nullptr, 0, 24);
    dispatch(28, 12, 2, args.Get(), 36, 8);
    dispatch(40, 24, 1, args.Get(), 40, 0);
    // Frosty uses the all-address packet and the buffer-range packet. Each
    // following dispatch consumes the values written by its predecessor.
    list->SetPipelineState(accumulate.Get());
    auto consume = [&](UINT multiplier, UINT increment) {
        const UINT constants[]{increment, multiplier, 0, 24};
        method<SetConstants>(list.Get(), 39)(list.Get(), 0, 4, constants, 0);
        list->Dispatch(3, 1, 1);
        for (UINT i = 0; i < 24; ++i) expected[i] = expected[i] * multiplier + increment;
    };
    flush(list.Get(), 0xc00000, 0, 0xffffffff00ull);
    consume(7, 23);
    flush(list.Get(), 0x3000, out->GetGPUVirtualAddress(), sizeof(expected));
    consume(3, 37);
    // Wake Up flushes a 0xa30000-byte texture allocation at guest address zero.
    flush(list.Get(), 0x3000, 0, 0xa30000);
    consume(2, 5);
    flush(list.Get(), 0x3000, out->GetGPUVirtualAddress(), 0);
    consume(3, 11);
    flush(list.Get(), 0xc00000, out->GetGPUVirtualAddress(), sizeof(expected));
    consume(5, 7);
    D3D12_RESOURCE_BARRIER order{};
    order.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    order.UAV.pResource = out.Get();
    list->ResourceBarrier(1, &order);
    consume(5, 17);
    order.UAV.pResource = nullptr;
    list->ResourceBarrier(1, &order);
    consume(2, 13);
    transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(result.Get(), 0, out.Get(), 0, sizeof(expected));
    check(list->Close(), "close indirect compute");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "compute fence");
    ID3D12CommandList* batch[] = {list.Get()};
    queue->ExecuteCommandLists(1, batch);
    using Signal = HRESULT (WINAPI*)(ID3D12CommandQueue*, ID3D12Fence*, UINT64);
    check(method<Signal>(queue.Get(), scarlett ? 16 : 14)(queue.Get(), fence.Get(), 1),
          "signal compute fence");
    compute_fixture::wait_for_fence(fence.Get(), 1);
    D3D12_RANGE read{0, sizeof(expected)}, empty{};
    void* mapped = nullptr;
    check(result->Map(0, &read, &mapped), "read indirect results");
    const bool match = std::memcmp(mapped, expected.data(), sizeof(expected)) == 0;
    result->Unmap(0, &empty);
    if (!match) throw std::runtime_error("indirect dispatch or guarded words differ from CPU reference");
    char receipt[256];
    std::snprintf(receipt, sizeof(receipt),
                  "d3d12-indirect PASS pci=%04x:%04x sdk=0x20004 device=41 list=%u words=64/64 "
                  "counts=direct,one,zero flush=global,range,wake-up(0x3000:0+0xa30000),zero-length,global-range budget=default,rejected2 mesh=0x%08lx rays=0x%08lx",
                  selected.VendorId, selected.DeviceId, scarlett ? 61u : 59u, static_cast<unsigned long>(mesh), static_cast<unsigned long>(ray));
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-indirect", true, run, L"--scarlett", " [--scarlett]");
}
