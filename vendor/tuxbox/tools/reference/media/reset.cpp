#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <d3d11.h>
#include <d3d11on12.h>
#include <d3d10.h>
#include <mfobjects.h>
#include <wrl/client.h>
#include <cstdio>
using Microsoft::WRL::ComPtr;

static HRESULT report(const char* op, HRESULT hr) {
    char s[120];
    std::snprintf(s, sizeof(s), "FrostyMF %s: 0x%08lx", op, (unsigned long)hr);
    OutputDebugStringA(s);
    return hr;
}

extern "C" HRESULT WINAPI MFResetDXGIDeviceManagerX(IMFDXGIDeviceManager* manager, IUnknown* device,
                                                    UINT token) {
    if (!manager || !device) return E_POINTER;
    ComPtr<ID3D11Device> video;
    if (SUCCEEDED(device->QueryInterface(IID_PPV_ARGS(&video))))
        return report("reset D3D11", manager->ResetDevice(video.Get(), token));
    ComPtr<ID3D12Device> media12, owner;
    HRESULT hr = device->QueryInterface(IID_PPV_ARGS(&media12));
    if (FAILED(hr)) return report("query D3D12", hr);
    // D3D11On12 consumes desktop interfaces. Resolve the per-adapter D3D12
    // device by the guest's LUID, so decoding and the game share allocations.
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    hr = CreateDXGIFactory1(IID_PPV_ARGS(&factory));
    if (FAILED(hr)) return report("get DXGI factory", hr);
    hr = factory->EnumAdapterByLuid(media12->GetAdapterLuid(), IID_PPV_ARGS(&adapter));
    if (FAILED(hr)) return report("get game adapter", hr);
    hr = D3D12CreateDevice(adapter.Get(), D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&owner));
    if (FAILED(hr)) return report("get desktop D3D12 device", hr);
    D3D12_COMMAND_QUEUE_DESC desc{};
    ComPtr<ID3D12CommandQueue> queue;
    hr = owner->CreateCommandQueue(&desc, IID_PPV_ARGS(&queue));
    if (FAILED(hr)) return report("create queue", hr);
    IUnknown* queues[] = {queue.Get()};
    hr =
        D3D11On12CreateDevice(owner.Get(), D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
                              nullptr, 0, queues, 1, 0, &video, nullptr, nullptr);
    if (FAILED(hr)) return report("D3D11On12CreateDevice", hr);
    ComPtr<ID3D10Multithread> multithread;
    hr = video.As(&multithread);
    if (FAILED(hr)) return report("D3D11 multithread interface", hr);
    multithread->SetMultithreadProtected(TRUE);
    // The media manager can outlive the title's reference to its D3D12 device.
    // Keep the queue (and its owning device) tied to the exact D3D11 device.
    const GUID queue_lifetime{0x57e39624, 0x6e1c, 0x4821, {0x98, 0x32, 0x7b, 0xba, 0x50, 0xf5, 0x4c, 0x1e}};
    hr = video->SetPrivateDataInterface(queue_lifetime, queue.Get());
    if (FAILED(hr)) return report("retain media queue", hr);
    return report("reset D3D11On12", manager->ResetDevice(video.Get(), token));
}
