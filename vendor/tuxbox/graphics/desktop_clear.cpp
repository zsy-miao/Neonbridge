// Desktop D3D12 hardware baseline. One full clear and one rectangular clear give
// every readback row an expected pattern, including rows around the rectangle.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <d3d12.h>
#include <dxgi1_4.h>
#include <wrl/client.h>
#include <array>
#include <cstdio>
#include <cwchar>
#include <filesystem>
#include <fstream>
#include <cstring>
#include <stdexcept>
#include <string>

using Microsoft::WRL::ComPtr;

namespace {
constexpr UINT width = 37, height = 13;
constexpr DXGI_FORMAT format = DXGI_FORMAT_R8G8B8A8_UNORM;
constexpr std::array<BYTE, 4> background{17, 91, 203, 255}, foreground{244, 51, 115, 69};
constexpr D3D12_RECT inset{5, 2, 31, 10};

void check(HRESULT hr, const char* operation) {
    if (FAILED(hr)) {
        char code[16];
        std::snprintf(code, sizeof(code), "0x%08lx", static_cast<unsigned long>(hr));
        throw std::runtime_error(std::string(operation) + " failed with " + code);
    }
}

bool is_hex_quad(const wchar_t* text) {
    for (unsigned i = 0; i < 4; ++i) {
        const wchar_t c = text[i];
        if (!((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')))
            return false;
    }
    return true;
}

std::string run(const wchar_t* adapter_id) {
    ComPtr<IDXGIFactory1> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    ComPtr<ID3D12Device> device;
    DXGI_ADAPTER_DESC1 adapter{};
    for (UINT index = 0; ; ++index) {
        ComPtr<IDXGIAdapter1> candidate;
        HRESULT hr = factory->EnumAdapters1(index, &candidate);
        if (hr == DXGI_ERROR_NOT_FOUND)
            throw std::runtime_error("no matching hardware adapter provides D3D12 feature level 11_0");
        check(hr, "EnumAdapters1");
        check(candidate->GetDesc1(&adapter), "GetDesc1");
        if (adapter.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) continue;
        wchar_t id[32];
        std::swprintf(id, 32, L"%04x:%04x", adapter.VendorId, adapter.DeviceId);
        if (adapter_id && _wcsicmp(id, adapter_id)) continue;
        hr = D3D12CreateDevice(candidate.Get(), D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&device));
        if (SUCCEEDED(hr)) break;
        if (adapter_id) check(hr, "D3D12CreateDevice");
    }

    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    queue_desc.Type = D3D12_COMMAND_LIST_TYPE_DIRECT;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "CreateCommandQueue");
    check(device->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "CreateCommandAllocator");
    check(device->CreateCommandList(0, queue_desc.Type, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "CreateCommandList");

    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC texture_desc{};
    texture_desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    texture_desc.Width = width;
    texture_desc.Height = height;
    texture_desc.DepthOrArraySize = texture_desc.MipLevels = 1;
    texture_desc.Format = format;
    texture_desc.SampleDesc.Count = 1;
    texture_desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> texture;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &texture_desc,
          D3D12_RESOURCE_STATE_RENDER_TARGET, nullptr, IID_PPV_ARGS(&texture)), "CreateCommittedResource(texture)");

    D3D12_PLACED_SUBRESOURCE_FOOTPRINT layout{};
    UINT rows = 0;
    UINT64 row_bytes = 0, buffer_bytes = 0;
    device->GetCopyableFootprints(&texture_desc, 0, 1, 0, &layout, &rows, &row_bytes, &buffer_bytes);
    const auto pitch = layout.Footprint.RowPitch;
    if (rows != height || row_bytes != width * 4 || pitch < row_bytes || pitch % D3D12_TEXTURE_DATA_PITCH_ALIGNMENT ||
        layout.Offset != 0 || buffer_bytes < UINT64(height - 1) * pitch + row_bytes)
        throw std::runtime_error("unexpected copyable footprint");
    heap.Type = D3D12_HEAP_TYPE_READBACK;
    D3D12_RESOURCE_DESC readback_desc{};
    readback_desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    readback_desc.Width = buffer_bytes;
    readback_desc.Height = readback_desc.DepthOrArraySize = readback_desc.MipLevels = 1;
    readback_desc.SampleDesc.Count = 1;
    readback_desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> readback;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &readback_desc,
          D3D12_RESOURCE_STATE_COPY_DEST, nullptr, IID_PPV_ARGS(&readback)), "CreateCommittedResource(readback)");

    D3D12_DESCRIPTOR_HEAP_DESC rtv_desc{};
    rtv_desc.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
    rtv_desc.NumDescriptors = 1;
    ComPtr<ID3D12DescriptorHeap> views;
    check(device->CreateDescriptorHeap(&rtv_desc, IID_PPV_ARGS(&views)), "CreateDescriptorHeap");
    auto rtv = views->GetCPUDescriptorHandleForHeapStart();
    device->CreateRenderTargetView(texture.Get(), nullptr, rtv);
    auto clear = [&](const std::array<BYTE, 4>& color, const D3D12_RECT* rect) {
        float rgba[4];
        for (size_t i = 0; i < color.size(); ++i) rgba[i] = color[i] / 255.f;
        list->ClearRenderTargetView(rtv, rgba, rect ? 1 : 0, rect);
    };
    clear(background, nullptr);
    clear(foreground, &inset);
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    barrier.Transition = {texture.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                          D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE};
    list->ResourceBarrier(1, &barrier);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = texture.Get();
    src.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    dst.pResource = readback.Get();
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint = layout;
    list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
    check(list->Close(), "Close command list");

    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "CreateFence");
    ID3D12CommandList* batch[] = {list.Get()};
    queue->ExecuteCommandLists(1, batch);
    check(queue->Signal(fence.Get(), 1), "Signal");
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) throw std::runtime_error("CreateEventW failed");
    HRESULT notify = fence->SetEventOnCompletion(1, done);
    DWORD waited = SUCCEEDED(notify) ? WaitForSingleObject(done, 15000) : WAIT_FAILED;
    CloseHandle(done);
    check(notify, "SetEventOnCompletion");
    if (waited != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1) {
        check(device->GetDeviceRemovedReason(), "GetDeviceRemovedReason");
        throw std::runtime_error("copy fence did not complete within 15 seconds");
    }

    D3D12_RANGE valid{0, static_cast<SIZE_T>(buffer_bytes)}, unwritten{0, 0};
    void* mapped = nullptr;
    check(readback->Map(0, &valid, &mapped), "Map");
    for (UINT y = 0; y < height; ++y) {
        for (UINT x = 0; x < width; ++x) {
            const BYTE* actual = static_cast<const BYTE*>(mapped) + y * pitch + x * 4;
            const auto& expected = (x >= inset.left && x < inset.right && y >= inset.top && y < inset.bottom)
                                   ? foreground : background;
            if (std::memcmp(actual, expected.data(), expected.size())) {
                std::fprintf(stderr, "pixel (%u,%u) expected %u,%u,%u,%u and returned %u,%u,%u,%u\n",
                    x, y, expected[0], expected[1], expected[2], expected[3], actual[0], actual[1], actual[2], actual[3]);
                readback->Unmap(0, &unwritten);
                throw std::runtime_error("pixel mismatch");
            }
        }
    }
    readback->Unmap(0, &unwritten);
    char name[512]{};
    if (!WideCharToMultiByte(CP_UTF8, 0, adapter.Description, -1, name, sizeof(name), nullptr, nullptr))
        throw std::runtime_error("could not read adapter description");
    char summary[768];
    std::snprintf(summary, sizeof(summary),
                  "desktop-clear PASS adapter=%s pci=%04x:%04x pixels=%u/%u row_bytes=%llu row_pitch=%u",
                  name, adapter.VendorId, adapter.DeviceId, width * height, width * height,
                  static_cast<unsigned long long>(row_bytes), pitch);
    return summary;
}
}

int wmain(int argc, wchar_t** argv) {
    const wchar_t* adapter = nullptr;
    const wchar_t* report = nullptr;
    for (int i = 1; i < argc; ++i) {
        if (!std::wcscmp(argv[i], L"--adapter") && !adapter && i + 1 < argc) {
            adapter = argv[++i];
            if (std::wcslen(adapter) == 9 && adapter[4] == ':' && is_hex_quad(adapter) && is_hex_quad(adapter + 5))
                continue;
        } else if (!std::wcscmp(argv[i], L"--report") && !report && i + 1 < argc) {
            report = argv[++i];
            if (*report) continue;
        }
        std::fprintf(stderr, "usage: desktop-clear.exe [--adapter VVVV:DDDD] [--report FILE]\n");
        return 2;
    }
    try {
        std::ofstream output;
        if (report) {
            output.open(std::filesystem::path(report), std::ios::binary | std::ios::trunc);
            if (!output) throw std::runtime_error("cannot create report file");
        }
        const auto summary = run(adapter);
        if (report) {
            output << summary << '\n';
            output.close();
            if (!output) throw std::runtime_error("could not write report file");
        }
        std::puts(summary.c_str());
        return 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "desktop-clear failed: %s\n", error.what());
        return 1;
    }
}
