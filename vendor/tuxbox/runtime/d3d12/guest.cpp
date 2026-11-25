#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <wrl/client.h>
#include <array>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include <filesystem>
#include <fstream>
#include <stdexcept>
#include <string>
#include "affine.h"

using Microsoft::WRL::ComPtr;

namespace {
void check(HRESULT hr, const char* operation) {
    if (FAILED(hr)) {
        char code[16];
        std::snprintf(code, sizeof(code), "0x%08lx", static_cast<unsigned long>(hr));
        throw std::runtime_error(std::string(operation) + " failed with " + code);
    }
}

std::wstring module_path(HMODULE module) {
    std::wstring path(32768, L'\0');
    DWORD size = GetModuleFileNameW(module, path.data(), static_cast<DWORD>(path.size()));
    if (!size || size >= path.size()) throw std::runtime_error("could not identify loaded PE");
    path.resize(size);
    return path;
}

ComPtr<ID3DBlob> serialize_and_check() {
    auto shim = GetModuleHandleW(L"d3d12_xs.dll");
    if (!shim || std::filesystem::path(module_path(nullptr)).parent_path() !=
                 std::filesystem::path(module_path(shim)).parent_path())
        throw std::runtime_error("guest did not load the app-local d3d12_xs.dll");

    D3D12_ROOT_PARAMETER parameters[2]{};
    parameters[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    parameters[0].Constants = {5, 2, 4}; // b5/space2: four DWORDs
    parameters[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    parameters[1].Descriptor = {3, 4}; // u3/space4
    D3D12_ROOT_SIGNATURE_DESC description{};
    description.NumParameters = 2;
    description.pParameters = parameters;
    ComPtr<ID3DBlob> blob, error;
    HRESULT hr = D3D12SerializeRootSignature(&description, D3D_ROOT_SIGNATURE_VERSION_1, &blob, &error);
    if (FAILED(hr) && error) std::fwrite(error->GetBufferPointer(), 1, error->GetBufferSize(), stderr);
    check(hr, "guest D3D12SerializeRootSignature");
    if (!blob || !blob->GetBufferSize()) throw std::runtime_error("empty root signature");
    ComPtr<ID3D12RootSignatureDeserializer> reader;
    check(D3D12CreateRootSignatureDeserializer(blob->GetBufferPointer(), blob->GetBufferSize(),
                                               IID_PPV_ARGS(&reader)), "desktop root signature deserialization");
    const auto* desc = reader->GetRootSignatureDesc();
    if (!desc || desc->NumParameters != 2 || !desc->pParameters || desc->NumStaticSamplers ||
        desc->Flags != D3D12_ROOT_SIGNATURE_FLAG_NONE)
        throw std::runtime_error("root signature shape mismatch");
    const auto& cb = desc->pParameters[0];
    const auto& uav = desc->pParameters[1];
    if (cb.ParameterType != D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS ||
        cb.ShaderVisibility != D3D12_SHADER_VISIBILITY_ALL || cb.Constants.ShaderRegister != 5 ||
        cb.Constants.RegisterSpace != 2 || cb.Constants.Num32BitValues != 4 ||
        uav.ParameterType != D3D12_ROOT_PARAMETER_TYPE_UAV ||
        uav.ShaderVisibility != D3D12_SHADER_VISIBILITY_ALL || uav.Descriptor.ShaderRegister != 3 ||
        uav.Descriptor.RegisterSpace != 4)
        throw std::runtime_error("register, space or root parameter type mismatch");
    return blob;
}

constexpr UINT words = 64;
ComPtr<ID3D12Resource> buffer(ID3D12Device* device, D3D12_HEAP_TYPE heap_type,
                              D3D12_RESOURCE_STATES state, D3D12_RESOURCE_FLAGS flags) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = heap_type;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = words * sizeof(UINT);
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = flags;
    ComPtr<ID3D12Resource> result;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
                                          IID_PPV_ARGS(&result)), "CreateCommittedResource");
    return result;
}

void transition(ID3D12GraphicsCommandList* commands, ID3D12Resource* resource,
                D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER b{};
    b.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    b.Transition = {resource, D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, before, after};
    commands->ResourceBarrier(1, &b);
}

std::string run(const wchar_t* selected) {
    auto blob = serialize_and_check();
    ComPtr<IDXGIFactory1> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    ComPtr<ID3D12Device> device;
    DXGI_ADAPTER_DESC1 adapter{};
    for (UINT i = 0; ; ++i) {
        ComPtr<IDXGIAdapter1> candidate;
        HRESULT hr = factory->EnumAdapters1(i, &candidate);
        if (hr == DXGI_ERROR_NOT_FOUND) throw std::runtime_error("no matching D3D12 hardware adapter");
        check(hr, "EnumAdapters1");
        check(candidate->GetDesc1(&adapter), "GetDesc1");
        if (adapter.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) continue;
        wchar_t id[32];
        std::swprintf(id, 32, L"%04x:%04x", adapter.VendorId, adapter.DeviceId);
        if (selected && _wcsicmp(selected, id)) continue;
        hr = D3D12CreateDevice(candidate.Get(), D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device));
        if (SUCCEEDED(hr)) break;
        if (selected) check(hr, "D3D12CreateDevice");
    }

    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "CreateRootSignature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pipeline{};
    pipeline.pRootSignature = root.Get();
    pipeline.CS = {affine_dxil, sizeof(affine_dxil)};
    ComPtr<ID3D12PipelineState> pso;
    check(device->CreateComputePipelineState(&pipeline, IID_PPV_ARGS(&pso)), "CreateComputePipelineState");
    D3D12_COMMAND_QUEUE_DESC q{};
    q.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> commands;
    check(device->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "CreateCommandQueue");
    check(device->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)), "CreateCommandAllocator");
    check(device->CreateCommandList(0, q.Type, allocator.Get(), pso.Get(), IID_PPV_ARGS(&commands)), "CreateCommandList");
    auto upload = buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ, D3D12_RESOURCE_FLAG_NONE);
    auto output = buffer(device.Get(), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_FLAG_NONE);

    std::array<UINT, words> expected{};
    for (UINT i = 0; i < words; ++i) expected[i] = 0x99a50000u + i;
    D3D12_RANGE no_read{0, 0}, all{0, sizeof(expected)};
    void* mapping = nullptr;
    check(upload->Map(0, &no_read, &mapping), "upload Map");
    std::memcpy(mapping, expected.data(), sizeof(expected));
    upload->Unmap(0, &all);
    commands->CopyBufferRegion(output.Get(), 0, upload.Get(), 0, sizeof(expected));
    transition(commands.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    commands->SetComputeRootSignature(root.Get());
    commands->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
    // Rebind all four words between dispatches. Two disjoint ranges leave 16 guarded words.
    constexpr std::array<std::array<UINT, 4>, 2> constants{{{31, 13, 3, 29}, {17, 37, 38, 19}}};
    for (const auto& params : constants) {
        commands->SetComputeRoot32BitConstants(0, static_cast<UINT>(params.size()), params.data(), 0);
        commands->Dispatch((params[3] + 7) / 8, 1, 1);
        for (UINT j = 0; j < params[3]; ++j) expected[params[2] + j] = j * params[1] + params[0];
    }
    transition(commands.Get(), output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    commands->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, sizeof(expected));
    check(commands->Close(), "Close");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "CreateFence");
    ID3D12CommandList* batch[] = {commands.Get()};
    queue->ExecuteCommandLists(1, batch);
    check(queue->Signal(fence.Get(), 1), "Signal");
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) throw std::runtime_error("CreateEventW failed");
    HRESULT hr = fence->SetEventOnCompletion(1, done);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(done, 15000) : WAIT_FAILED;
    CloseHandle(done);
    check(hr, "SetEventOnCompletion");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU dispatch did not complete");
    check(readback->Map(0, &all, &mapping), "readback Map");
    bool match = !std::memcmp(mapping, expected.data(), sizeof(expected));
    readback->Unmap(0, &no_read);
    if (!match) throw std::runtime_error("GPU output or guarded range differs from CPU reference");
    char result[180];
    std::snprintf(result, sizeof(result), "d3d12-root-smoke PASS import=D3D12SerializeRootSignature "
                  "pci=%04x:%04x constant=b5/space2 uav=u3/space4 dispatches=2 words=64/64 guards=16",
                  adapter.VendorId, adapter.DeviceId);
    return result;
}
}

int wmain(int argc, wchar_t** argv) {
    const wchar_t* selected = nullptr;
    const wchar_t* report = nullptr;
    for (int i = 1; i < argc; ++i) {
        if (!std::wcscmp(argv[i], L"--adapter") && !selected && i + 1 < argc) selected = argv[++i];
        else if (!std::wcscmp(argv[i], L"--report") && !report && i + 1 < argc) report = argv[++i];
        else {
            std::fprintf(stderr, "usage: d3d12-root-smoke.exe [--adapter VVVV:DDDD] [--report FILE]\n");
            return 2;
        }
    }
    try {
        std::ofstream result;
        if (report) {
            result.open(std::filesystem::path(report), std::ios::binary | std::ios::trunc);
            if (!result) throw std::runtime_error("cannot open report");
        }
        const auto summary = run(selected);
        if (report) {
            result << summary << '\n';
            result.close();
            if (!result) throw std::runtime_error("could not write report");
        }
        std::puts(summary.c_str());
        return 0;
    } catch (const std::exception& e) {
        std::fprintf(stderr, "d3d12-root-smoke failed: %s\n", e.what());
        return 1;
    }
}
