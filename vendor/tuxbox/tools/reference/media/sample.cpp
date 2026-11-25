#include "session.h"
#include <d3d11.h>
#include <d3d11on12.h>
#include <mfapi.h>
#include <mfidl.h>
#include <cstring>
using frame_abi::check;
using frame_abi::ComPtr;

static void sample(FILE* f, bool xbox) {
    MediaSession session;
    HMODULE proxy = GetModuleHandleW(L"mfplat.dll");
    using Reset = HRESULT(WINAPI*)(IMFDXGIDeviceManager*, IUnknown*, UINT);
    const auto reset = reinterpret_cast<Reset>(GetProcAddress(proxy, "MFResetDXGIDeviceManagerX"));
    if (!reset || reinterpret_cast<Reset>(GetProcAddress(proxy, MAKEINTRESOURCEA(117))) != reset)
        throw std::runtime_error("missing Xbox device-manager reset at ordinal 117");
    ComPtr<ID3D12Device> gpu;
    check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_11_0, IID_PPV_ARGS(&gpu)), "create D3D12");
    ComPtr<ID3D12Device> input = gpu;
    if (xbox) {
        auto library = LoadLibraryW(L"d3d12_xs.dll");
        using Factory = HRESULT (WINAPI*)(IUnknown*, const UINT*, REFIID, void**);
        const auto factory = library ? reinterpret_cast<Factory>(GetProcAddress(library, "D3D12XboxCreateDevice")) : nullptr;
        if (!factory) throw std::runtime_error("missing Xbox D3D12 device factory");
        const UINT profile[18]{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        input.Reset();
        check(factory(nullptr, profile, IID_PPV_ARGS(&input)), "create Xbox D3D12 device");
    }
    ComPtr<IMFDXGIDeviceManager> manager;
    UINT token{};
    check(MFCreateDXGIDeviceManager(&token, &manager), "create MF manager");
    auto incorrect = reset(manager.Get(), input.Get(), token ^ 1u);
    if (SUCCEEDED(incorrect)) throw std::runtime_error("wrong token accepted");
    std::fprintf(f, "wrong_token_hr=0x%08lx\n", static_cast<unsigned long>(incorrect));
    check(reset(manager.Get(), input.Get(), token), "reset through Xbox entry");
    HANDLE h{};
    check(manager->OpenDeviceHandle(&h), "open MF manager");
    ComPtr<ID3D11Device> video;
    check(manager->LockDevice(h, IID_PPV_ARGS(&video), TRUE), "lock as D3D11");
    ComPtr<ID3D11On12Device> interop;
    check(video.As(&interop), "manager D3D11On12");
    ComPtr<ID3D11DeviceContext> context;
    video->GetImmediateContext(&context);
    D3D12_HEAP_PROPERTIES props{};
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    props.VisibleNodeMask = props.CreationNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = 2;
    desc.Height = 2;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    ComPtr<ID3D12Resource> texture;
    check(gpu->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, D3D12_RESOURCE_STATE_COPY_DEST,
                                       nullptr, IID_PPV_ARGS(&texture)),
          "create D3D12 texture");
    D3D11_RESOURCE_FLAGS flags{};
    ComPtr<ID3D11Texture2D> wrapped;
    check(interop->CreateWrappedResource(texture.Get(), &flags, D3D12_RESOURCE_STATE_COPY_DEST,
                                         D3D12_RESOURCE_STATE_COPY_SOURCE, IID_PPV_ARGS(&wrapped)),
          "wrap texture");
    ComPtr<IMFSample> frame;
    check(MFCreateSample(&frame), "create sample");
    using CreateBuffer = decltype(&MFCreateDXGISurfaceBuffer);
    auto surface_buffer = reinterpret_cast<CreateBuffer>(GetProcAddress(proxy, "MFCreateDXGISurfaceBufferX"));
    if (!surface_buffer || reinterpret_cast<CreateBuffer>(GetProcAddress(proxy, MAKEINTRESOURCEA(35))) != surface_buffer)
        throw std::runtime_error("missing Xbox surface buffer at ordinal 35");
    ComPtr<IMFMediaBuffer> surface;
    check(surface_buffer(__uuidof(ID3D11Texture2D), wrapped.Get(), 0, FALSE, &surface), "create MF surface");
    check(frame->AddBuffer(surface.Get()), "attach surface");
    surface.Reset();
    check(frame->GetBufferByIndex(0, &surface), "get sample surface");
    ComPtr<IMFDXGIBuffer> dxgi;
    check(surface.As(&dxgi), "query sample DXGI buffer");
    ComPtr<ID3D11Texture2D> sample_texture;
    check(dxgi->GetResource(IID_PPV_ARGS(&sample_texture)), "get sample texture");
    ComPtr<IUnknown> original, recovered;
    check(wrapped.As(&original), "wrapped identity");
    check(sample_texture.As(&recovered), "sample identity");
    if (original.Get() != recovered.Get()) throw std::runtime_error("sample texture identity changed");
    constexpr UINT words[]{0xaf42b103, 0x612afe2c, 0xd3af026e, 0x921e051a};
    ID3D11Resource* resources[]{sample_texture.Get()};
    interop->AcquireWrappedResources(resources, 1);
    context->UpdateSubresource(sample_texture.Get(), 0, nullptr, words, 8, 16);
    D3D11_TEXTURE2D_DESC read{};
    wrapped->GetDesc(&read);
    read.BindFlags = read.MiscFlags = 0;
    read.Usage = D3D11_USAGE_STAGING;
    read.CPUAccessFlags = D3D11_CPU_ACCESS_READ;
    ComPtr<ID3D11Texture2D> copy;
    check(video->CreateTexture2D(&read, nullptr, &copy), "create staging");
    context->CopyResource(copy.Get(), sample_texture.Get());
    interop->ReleaseWrappedResources(resources, 1);
    context->Flush();
    D3D11_MAPPED_SUBRESOURCE map{};
    check(context->Map(copy.Get(), 0, D3D11_MAP_READ, 0, &map), "map copied sample");
    bool matched = !std::memcmp(map.pData, words, 8) &&
                   !std::memcmp(static_cast<const BYTE*>(map.pData) + map.RowPitch, words + 2, 8);
    context->Unmap(copy.Get(), 0);
    if (!matched) throw std::runtime_error("MF surface GPU bytes differ");
    check(manager->UnlockDevice(h, FALSE), "unlock");
    check(manager->CloseDeviceHandle(h), "close manager");
    std::fprintf(f, "verified_pixel_words=4\n");
}
int wmain(int argc, wchar_t** argv) {
    const bool xbox = argc == 2 && !wcscmp(argv[1], L"--xbox");
    if (argc != 1 && !xbox) return 2;
    FILE* f{};
    if (_wfopen_s(&f, L"sample.txt", L"wb")) return 2;
    int rc = 0;
    try {
        sample(f, xbox);
    } catch (const std::exception& e) {
        std::fprintf(f, "error=%s\n", e.what());
        rc = 1;
    }
    fclose(f);
    return rc;
}
