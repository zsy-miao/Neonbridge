// Compare the Scarlett UMD's CPU record update with the TuxBox export on a
// live host device. The reference image is used only for its import-free
// record update and is supplied locally by the caller.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d12.h>
#include <array>
#include <cstdint>
#include <cstdio>

using Driver = HRESULT(WINAPI*)(UINT, void*);
struct Update {
    void* target;
    uint32_t kind, flags, a, b, c, d, extra;
};

static unsigned notifications;
static void* expected_device;
static void WINAPI disposed(void* device, void* context) {
    if (device == expected_device && context == &notifications) ++notifications;
}

int wmain(int argc, wchar_t** argv) {
    if (argc != 3) return 1;
    auto provider = LoadLibraryW(L"d3d12_xs.dll");
    auto reference = LoadLibraryExW(argv[1], nullptr, DONT_RESOLVE_DLL_REFERENCES);
    if (!provider || !reference) return 2;
    auto call = reinterpret_cast<Driver>(GetProcAddress(provider, "InternalDriverCallX"));
    auto original = reinterpret_cast<Driver>(GetProcAddress(reference, "UMDInternalDriverCallX"));
    using Create = HRESULT(WINAPI*)(void*, UINT, REFIID, void**);
    auto create = reinterpret_cast<Create>(GetProcAddress(provider, "D3D12CreateDevice"));
    if (!call || !original || !create || call(1, nullptr) != E_NOINTERFACE) return 3;
    ID3D12Device* device = nullptr;
    if (FAILED(create(nullptr, D3D_FEATURE_LEVEL_12_0, __uuidof(ID3D12Device),
                      reinterpret_cast<void**>(&device)))) return 4;

    unsigned cases = 0;
    for (uint32_t kind = 0; kind < 5; ++kind) {
        for (uint32_t flags = 0; flags < 256; ++flags) {
            for (uint32_t offset = 0; offset < 32; ++offset) {
                alignas(64) std::array<unsigned char, 384> native{}, translated{};
                for (size_t i = 0; i < native.size(); ++i)
                    native[i] = static_cast<unsigned char>(i * 31 + flags);
                translated = native;
                Update args{native.data() + offset, kind, flags, flags + 1, flags + 2,
                            flags + 3, flags + 4, (flags + offset) & 15};
                if (original(1, &args) != S_OK) return 5;
                args.target = translated.data() + offset;
                if (call(1, &args) != S_OK || native != translated) return 6;
                ++cases;
            }
        }
    }

    expected_device = device;
    struct Hook { void* device; decltype(&disposed) callback; void* context; };
    Hook hook{device, disposed, &notifications};
    if (call(3, &hook) != S_OK) return 7;
    hook.callback = nullptr;
    if (call(3, &hook) != S_OK) return 8;
    device->Release();
    if (notifications) return 9;
    if (FAILED(create(nullptr, D3D_FEATURE_LEVEL_12_0, __uuidof(ID3D12Device),
                      reinterpret_cast<void**>(&device)))) return 9;
    expected_device = device;
    hook.device = device;
    hook.callback = disposed;
    if (call(3, &hook) != S_OK) return 9;
    device->AddRef();
    device->Release();
    if (notifications) return 10;
    device->Release();
    if (notifications != 1) return 11;
    FILE* report = nullptr;
    if (_wfopen_s(&report, argv[2], L"wb")) return 12;
    int n = std::fprintf(report, "matched_records=%u\nfinal_release_notifications=%u\n", cases, notifications);
    return std::fclose(report) || n < 0;
}
