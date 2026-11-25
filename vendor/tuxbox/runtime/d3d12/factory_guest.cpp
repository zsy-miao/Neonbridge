// The two factories in Frosty's DLL return devices with the Xbox queue slots.
#include "frame_abi.h"
#include "frame_cli.h"
#include <cstring>

extern "C" __declspec(dllimport) HRESULT WINAPI D3D12XboxSetProcessDebugFlags(UINT flags);
extern "C" __declspec(dllimport) UINT WINAPI D3D12XboxGetProcessDebugFlags();

namespace {
using namespace frame_abi;

void check_flags() {
    if (D3D12XboxGetProcessDebugFlags() != 0) throw std::runtime_error("initial flags");
    for (UINT input : {0xffffffffu, 1u, 0x7c0ffffeu, 0u}) {
        if (D3D12XboxSetProcessDebugFlags(input) != S_OK ||
            D3D12XboxGetProcessDebugFlags() != (input & 0x83f00001u))
            throw std::runtime_error("process debug flag mask");
    }
}

void check_queue(ID3D12Device* device) {
    D3D12_COMMAND_QUEUE_DESC desc{};
    desc.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    ComPtr<ID3D12CommandQueue> queue;
    check(method<CreateQueue>(device, 48)(device, &desc, IID_PPV_ARGS(&queue)), "Xbox compute queue");
    ComPtr<ID3D12Device> parent;
    check(queue->GetDevice(IID_PPV_ARGS(&parent)), "queue parent");
    if (parent.Get() != device) throw std::runtime_error("foreign queue parent");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    check(queue->Signal(fence.Get(), 3), "signal fence");
    HANDLE done = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!done) throw std::runtime_error("create fence event");
    const HRESULT armed = fence->SetEventOnCompletion(3, done);
    const DWORD waited = SUCCEEDED(armed) ? WaitForSingleObject(done, 10000) : WAIT_FAILED;
    CloseHandle(done);
    if (waited != WAIT_OBJECT_0 || fence->GetCompletedValue() != 3)
        throw std::runtime_error("fence did not complete");
}

void check_private_texture(ID3D12Device* device) {
    using Create = HRESULT (WINAPI *)(ID3D12Device*, const D3D12_HEAP_PROPERTIES*, D3D12_HEAP_FLAGS,
        const void*, D3D12_RESOURCE_STATES, const D3D12_CLEAR_VALUE*, REFIID, void**);
    const auto create = method<Create>(device, 76);
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = 32;
    desc.MipLevels = 1;
    desc.DepthOrArraySize = 1;
    desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x101);
    desc.Flags = static_cast<D3D12_RESOURCE_FLAGS>(0x00c00000 | D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    alignas(8) std::array<UINT, 16> record{};
    static_assert(offsetof(D3D12_RESOURCE_DESC, Flags) == 48);
    std::memcpy(record.data(), &desc, 52);
    record[13] = 3;
    ComPtr<ID3D12Resource> texture;
    check(create(device, &heap, D3D12_HEAP_FLAG_NONE, record.data(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                 nullptr, IID_PPV_ARGS(&texture)), "Xbox private texture");
    const auto actual = texture->GetDesc();
    if (actual.Layout != desc.Layout || actual.Flags != desc.Flags || actual.Format != desc.Format ||
        actual.Width != desc.Width || actual.Height != desc.Height)
        throw std::runtime_error("Xbox private texture description");
    record[14] = 1;
    void* unsupported = device;
    if (create(device, &heap, D3D12_HEAP_FLAG_NONE, record.data(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
               nullptr, __uuidof(ID3D12Resource), &unsupported) != E_NOTIMPL || unsupported)
        throw std::runtime_error("unsupported Xbox texture extension accepted");
}

void check_dxgi(ComPtr<ID3D12Device>& device, IDXGIAdapter1* selected) {
    const LUID luid = device->GetAdapterLuid();
    ID3D12Device* original = device.Get();
    ComPtr<IUnknown> expected;
    check(device.As(&expected), "D3D device identity");
    ComPtr<IDXGIDevice1> dxgi;
    check(device.As(&dxgi), "query IDXGIDevice1");
    if (dxgi->GetAdapter(nullptr) != E_INVALIDARG)
        throw std::runtime_error("missing adapter output accepted");
    ComPtr<IUnknown> identity;
    check(dxgi.As(&identity), "DXGI device identity");
    if (identity.Get() != expected.Get()) throw std::runtime_error("split DXGI identity");
    expected.Reset();
    identity.Reset();
    device.Reset();  // Only the DXGI interface now holds a reference to the guest.
    check(dxgi.As(&device), "recover guest D3D device");
    if (device.Get() != original) throw std::runtime_error("DXGI returned another D3D device");
    ComPtr<IDXGIAdapter> adapter;
    check(dxgi->GetAdapter(&adapter), "get DXGI device adapter");
    dxgi.Reset();
    DXGI_ADAPTER_DESC desc{};
    check(adapter->GetDesc(&desc), "DXGI adapter description");
    if (desc.AdapterLuid.HighPart != luid.HighPart || desc.AdapterLuid.LowPart != luid.LowPart)
        throw std::runtime_error("DXGI and D3D adapter LUIDs differ");
    if (selected) {
        DXGI_ADAPTER_DESC1 supplied{};
        check(selected->GetDesc1(&supplied), "selected adapter description");
        if (supplied.VendorId != desc.VendorId || supplied.DeviceId != desc.DeviceId ||
            supplied.AdapterLuid.HighPart != luid.HighPart || supplied.AdapterLuid.LowPart != luid.LowPart)
            throw std::runtime_error("DXGI returned a different adapter from the selection");
    }
    ComPtr<IDXGIFactory2> factory;
    check(adapter->GetParent(IID_PPV_ARGS(&factory)), "adapter parent factory");
    if (!factory) throw std::runtime_error("empty parent factory");
}

std::string run(const wchar_t* requested, bool) {
    check_flags();
    ComPtr<IDXGIFactory1> factory;
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "DXGI factory");
    ComPtr<IDXGIAdapter1> adapter;
    if (requested) {
        for (UINT i = 0; ; ++i) {
            check(factory->EnumAdapters1(i, adapter.ReleaseAndGetAddressOf()), "find requested adapter");
            DXGI_ADAPTER_DESC1 desc{};
            check(adapter->GetDesc1(&desc), "adapter desc");
            wchar_t pci[16];
            std::swprintf(pci, 16, L"%04x:%04x", desc.VendorId, desc.DeviceId);
            if (!_wcsicmp(pci, requested)) break;
        }
    }
    // The retail title passes a null adapter. The default run uses that path.
    std::array<UINT, 18> params{
        0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};
    ComPtr<ID3D12Device> direct, standard;
    check(D3D12XboxCreateDevice(adapter.Get(), params.data(), IID_PPV_ARGS(&direct)), "0x20004 factory");
    using Deserialize = HRESULT (WINAPI *)(ID3D12Device*, const void*, REFIID, void**);
    std::array<UINT64, 9> serialized{};
    void* pipeline = direct.Get();
    if (method<Deserialize>(direct.Get(), 67)(direct.Get(), serialized.data(),
            __uuidof(ID3D12PipelineState), &pipeline) != E_NOTIMPL || pipeline)
        throw std::runtime_error("serialized Xbox pipeline fallback");
    check_private_texture(direct.Get());
    check_dxgi(direct, adapter.Get());
    check_queue(direct.Get());
    check(D3D12CreateDevice(adapter.Get(), D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&standard)), "standard factory");
    check_dxgi(standard, adapter.Get());
    check_queue(standard.Get());
    const LUID actual = direct->GetAdapterLuid();
    const LUID other = standard->GetAdapterLuid();
    if (actual.LowPart != other.LowPart || actual.HighPart != other.HighPart)
        throw std::runtime_error("factory adapter mismatch");
    ComPtr<IDXGIFactory4> factory4;
    check(factory.As(&factory4), "DXGI factory4");
    ComPtr<IDXGIAdapter1> used;
    check(factory4->EnumAdapterByLuid(actual, IID_PPV_ARGS(&used)), "lookup actual adapter");
    DXGI_ADAPTER_DESC1 desc{};
    check(used->GetDesc1(&desc), "actual adapter");
    if (desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) throw std::runtime_error("software device");

    params[15] = 1;  // AutoHDR has a distinct record.
    void* out = direct.Get();
    if (D3D12XboxCreateDevice(adapter.Get(), params.data(), __uuidof(ID3D12Device), &out) != E_NOTIMPL || out)
        throw std::runtime_error("unsupported AutoHDR record accepted");
    char receipt[128];
    std::snprintf(receipt, sizeof(receipt),
                  "xbox-factory PASS pci=%04x:%04x adapter=%s profile=0x20004 queues=2 fences=2 flag-masks=4 dxgi=2",
                  desc.VendorId, desc.DeviceId, requested ? "selected" : "default");
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-factory", false, run);
}
