// Guest-side declarations for the observed Xbox SDK 0x20002 profile.
// Do not include runtime/provider headers here: the fixture is an ABI consumer.
#pragma once
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <wrl/client.h>
#include <array>
#include <cstdio>
#include <cwchar>
#include <stdexcept>
#include <string>

extern "C" __declspec(dllimport) HRESULT WINAPI D3D12XboxCreateDevice(
    IUnknown* adapter, const void* creation, REFIID iid, void** out);

namespace frame_abi {
using Microsoft::WRL::ComPtr;
inline constexpr std::array<UINT, 16> profile{
    0x20002, 0, 0xffffffff, 0, 0, 0, 0xffffffff, 0xffffffff, 0, 0, 0, 0, 0, 0, 0, 0};

inline void check(HRESULT hr, const char* operation) {
    if (FAILED(hr)) {
        char code[16];
        std::snprintf(code, sizeof(code), "0x%08lx", static_cast<unsigned long>(hr));
        throw std::runtime_error(std::string(operation) + " failed: " + code);
    }
}

template<class Function, class Object>
Function method(Object* object, size_t index) {
    return reinterpret_cast<Function>((*reinterpret_cast<void***>(object))[index]);
}

struct Plane { UINT64 token; UINT count; UINT padding; ID3D12Resource** resources; };
static_assert(sizeof(Plane) == 24 && offsetof(Plane, resources) == 16);
using Present = HRESULT (STDMETHODCALLTYPE*)(ID3D12CommandQueue*, UINT, const Plane*, const void*);
using CreateQueue = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMMAND_QUEUE_DESC*, REFIID, void**);
using CreateList = HRESULT (WINAPI*)(ID3D12Device*, const UINT*, ID3D12CommandAllocator*,
                                     ID3D12PipelineState*, REFIID, void**);
using CpuHandle = SIZE_T (WINAPI*)(ID3D12DescriptorHeap*);
using Clear = void (WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE,
                             const float*, UINT, const D3D12_RECT*);
using Copy = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_TEXTURE_COPY_LOCATION*,
                            UINT, UINT, UINT, const D3D12_TEXTURE_COPY_LOCATION*, const D3D12_BOX*, UINT);

inline ComPtr<ID3D12Device> create_device(const wchar_t* requested, DXGI_ADAPTER_DESC1& selected,
                                          const UINT* creation = profile.data(), REFIID iid = __uuidof(ID3D12Device)) {
    ComPtr<IDXGIFactory1> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    for (UINT i = 0; ; ++i) {
        ComPtr<IDXGIAdapter1> adapter;
        HRESULT hr = factory->EnumAdapters1(i, &adapter);
        if (hr == DXGI_ERROR_NOT_FOUND) throw std::runtime_error("no matching hardware device");
        check(hr, "EnumAdapters1");
        check(adapter->GetDesc1(&selected), "GetDesc1");
        if (selected.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) continue;
        wchar_t id[16];
        std::swprintf(id, 16, L"%04x:%04x", selected.VendorId, selected.DeviceId);
        if (requested && _wcsicmp(requested, id)) continue;
        ComPtr<ID3D12Device> device;
        hr = D3D12XboxCreateDevice(adapter.Get(), creation, iid, reinterpret_cast<void**>(device.GetAddressOf()));
        if (SUCCEEDED(hr) && device) return device;
        if (requested) check(FAILED(hr) ? hr : E_FAIL, "D3D12XboxCreateDevice");
    }
}
}
