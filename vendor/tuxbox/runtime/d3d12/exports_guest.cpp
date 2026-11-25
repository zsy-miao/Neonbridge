#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3dcommon.h>
#include <d3d12sdklayers.h>
#include <dxgi.h>
#include <array>
#include <cstdio>
#include <cstring>
#include <cwchar>
#include "function-ordinals.inc"

extern "C" __declspec(dllimport) HRESULT WINAPI D3D10CreateBlob(SIZE_T, ID3DBlob**);
extern "C" __declspec(dllimport) UINT WINAPI D3D12XboxGetProcessDebugFlags();
extern "C" __declspec(dllimport) HRESULT WINAPI D3D12XboxSetProcessDebugFlags(UINT);

// The Xbox object has two guest tables. Its first slot takes a flag mask.
struct Debug;
struct DebugMethods {
    HRESULT (WINAPI* set_flags)(Debug*, UINT);
};
struct DebugUnknown {
    HRESULT (WINAPI* query)(Debug*, REFIID, void**);
    ULONG (WINAPI* add_ref)(Debug*);
    ULONG (WINAPI* release)(Debug*);
};
struct Debug {
    const DebugMethods* methods;
    const DebugUnknown* unknown;
};

static FARPROC lookup(HMODULE module, const char* name, WORD ordinal) {
    auto address = GetProcAddress(module, name);
    if (address && address == GetProcAddress(module, MAKEINTRESOURCEA(ordinal))) return address;
    std::fprintf(stderr, "Missing or mismatched %s at ordinal %u\n", name, ordinal);
    return nullptr;
}

static bool check_debug(HMODULE module) {
    using Factory = HRESULT (WINAPI*)(REFIID, void**);
    auto factory = reinterpret_cast<Factory>(lookup(module, "D3D12GetDebugInterface", ORD_D3D12GetDebugInterface));
    if (!factory) return false;
    Debug* debug = nullptr;
    const auto iid = __uuidof(ID3D12Debug);
    if (factory(iid, reinterpret_cast<void**>(&debug)) != S_OK || !debug) return false;
    void* other = nullptr;
    if (factory(iid, &other) != S_OK || other != debug ||
        factory(__uuidof(IUnknown), &other) != E_NOINTERFACE || other != debug ||
        debug->unknown->query(debug, __uuidof(IUnknown), &other) != E_NOINTERFACE || other != debug) return false;
    other = nullptr;
    if (debug->unknown->query(debug, iid, &other) != S_OK || other != debug ||
        debug->unknown->add_ref(debug) != 1 || debug->unknown->release(debug) != 0) return false;
    const UINT saved_flags = D3D12XboxGetProcessDebugFlags();
    const bool flags_ok = debug->methods->set_flags(debug, 0x81123457) == S_OK &&
                          D3D12XboxGetProcessDebugFlags() == 0x81100001 &&
                          debug->methods->set_flags(debug, 0) == S_OK && D3D12XboxGetProcessDebugFlags() == 0;
    return D3D12XboxSetProcessDebugFlags(saved_flags) == S_OK && flags_ok;
}

static bool check_dxgix(HMODULE module) {
    struct Entry { const char* name; WORD ordinal; };
    constexpr Entry entries[]{
        {"DXGIXGetFrameStatistics", ORD_DXGIXGetFrameStatistics},
        {"DXGIXPresentArray", ORD_DXGIXPresentArray},
        {"DXGIXSetFrameNotification", ORD_DXGIXSetFrameNotification},
        {"DXGIXSetVLineNotification", ORD_DXGIXSetVLineNotification},
    };
    // Both reference DLLs return their HRESULT without reading any argument.
    using Call = HRESULT (WINAPI*)(void*, void*, void*, void*);
    for (const auto& entry : entries) {
        auto call = reinterpret_cast<Call>(lookup(module, entry.name, entry.ordinal));
        UINT canary = 0xfedcba98;
        if (!call || call(nullptr, nullptr, nullptr, nullptr) != DXGI_ERROR_UNSUPPORTED ||
            call(&canary, &canary, &canary, &canary) != DXGI_ERROR_UNSUPPORTED || canary != 0xfedcba98) return false;
    }
    return true;
}

static bool check_blob(HMODULE module) {
    if (!lookup(module, "D3D10CreateBlob", ORD_D3D10CreateBlob)) return false;
    ID3DBlob* blob = nullptr;
    const HRESULT hr = D3D10CreateBlob(257, &blob);
    if (hr != S_OK || !blob) {
        std::fprintf(stderr, "D3D10CreateBlob returned %#lx\n", hr);
        return false;
    }
    if (blob->GetBufferSize() != 257 || !blob->GetBufferPointer()) return false;
    ID3DBlob* other = nullptr;
    if (blob->QueryInterface(IID_PPV_ARGS(&other)) != S_OK || other != blob || other->Release() != 1) return false;
    auto bytes = static_cast<BYTE*>(blob->GetBufferPointer());
    std::memset(bytes, 0x49, 257);
    if (blob->AddRef() != 2 || blob->Release() != 1) return false;
    return bytes[0] == 0x49 && bytes[256] == 0x49 && blob->Release() == 0;
}

#if defined(ORD_PixGetPixieEntryPoints) && defined(ORD_D3D12UploadCustomMicrocodeX)
static bool check_extra_exports(HMODULE module) {
    using Pix = HRESULT (WINAPI*)(void*);
    using Upload = HRESULT (WINAPI*)(UINT, SIZE_T, const void*, SIZE_T, const void*, SIZE_T, const void*);
    auto pix = reinterpret_cast<Pix>(lookup(module, "PixGetPixieEntryPoints", ORD_PixGetPixieEntryPoints));
    auto upload = reinterpret_cast<Upload>(lookup(module, "D3D12UploadCustomMicrocodeX", ORD_D3D12UploadCustomMicrocodeX));
    if (!pix || !upload || pix(nullptr) != E_POINTER) return false;
    UINT header[] = {8, 0xffffffff};
    if (pix(header) != S_OK || header[0] != 8 || header[1] != 0) return false;
    // Three distinct, inert buffers for the desktop driver-unavailable control.
    const BYTE first[3]{}, second[5]{}, third[7]{};
    constexpr DWORD sentinel = 0xa14c;
    auto rejects = [&](UINT selector, SIZE_T n1, const void* p1, SIZE_T n2, const void* p2,
                       SIZE_T n3, const void* p3) {
        SetLastError(sentinel);
        return upload(selector, n1, p1, n2, p2, n3, p3) == E_INVALIDARG && GetLastError() == sentinel;
    };
    if (!rejects(0, 3, first, 5, second, 7, third) ||
        !rejects(1, 0, first, 5, second, 7, third) ||
        !rejects(1, 3, nullptr, 5, second, 7, third) ||
        !rejects(1, 3, first, 0, second, 7, third) ||
        !rejects(1, 3, first, 5, nullptr, 7, third) ||
        !rejects(1, 3, first, 5, second, 0, third) ||
        !rejects(1, 3, first, 5, second, 7, nullptr)) return false;
    for (UINT selector : {1u, 2u, 0xffffffffu, 1u}) {
        SetLastError(sentinel);
        if (upload(selector, 3, first, 5, second, 7, third) != E_FAIL ||
            GetLastError() != ERROR_INVALID_HANDLE) return false;
    }
    return true;
}
#endif

#if defined(ORD_GetFrameBufferPreserveMemorySizeBytes) && defined(ORD_D3D12ConfigureVirtualMemoryX)
static bool check_memory_config_invalid(HMODULE module) {
    using GetSize = UINT (WINAPI*)();
    using Configure = HRESULT (WINAPI*)(const UINT*);
    auto get_size = reinterpret_cast<GetSize>(lookup(module, "GetFrameBufferPreserveMemorySizeBytes",
                                                       ORD_GetFrameBufferPreserveMemorySizeBytes));
    auto configure = reinterpret_cast<Configure>(lookup(module, "D3D12ConfigureVirtualMemoryX",
                                                        ORD_D3D12ConfigureVirtualMemoryX));
    const UINT bad_flags[] = {2, 0, 123};
    const UINT bad_count[] = {0, 34, 123};
    return get_size && configure && get_size() == 0 && configure(nullptr) == E_INVALIDARG &&
           configure(bad_flags) == E_INVALIDARG && configure(bad_count) == E_INVALIDARG && get_size() == 0;
}
#endif

#ifdef ORD_D3D12ZeroMemoryX
static bool check_zero_memory(HMODULE module) {
    struct Range { void* address; SIZE_T bytes; };
    struct Workspace { void* commands; UINT command_bytes; void* scratch; UINT scratch_bytes; };
    using Clear = HRESULT (WINAPI*)(UINT, const Range*, const Workspace*, UINT);
    auto clear = reinterpret_cast<Clear>(lookup(module, "D3D12ZeroMemoryX", ORD_D3D12ZeroMemoryX));
    if (!clear || clear(0, nullptr, nullptr, 0) != S_OK || clear(1, nullptr, nullptr, 1) != E_INVALIDARG)
        return false;
    alignas(16) std::array<BYTE, 128> data;
    data.fill(0xa5);
    auto expected = data;
    static BYTE commands[65536], scratch[256];
    Workspace work{commands, sizeof(commands), scratch, sizeof(scratch)};
    Range ranges[] = {{data.data() + 16, 32}, {data.data() + 96, 16},
                      {data.data() + 32, 32}, {nullptr, 0}};
    Range bad[] = {ranges[0], {data.data() + 97, 16}};
    if (clear(2, bad, &work, 1) != E_INVALIDARG || data != expected) return false;
    bad[1] = {data.data() + 96, 17};
    if (clear(2, bad, &work, 1) != E_INVALIDARG || data != expected) return false;
    const Workspace invalid[] = {{nullptr, 65536, scratch, 256}, {commands, 65535, scratch, 256},
                                 {commands, 65536, nullptr, 256}, {commands, 65536, scratch, 255}};
    for (const auto& item : invalid)
        if (clear(4, ranges, &item, 1) != E_INVALIDARG || data != expected) return false;
    if (clear(4, ranges, &work, 1) != S_OK) return false;
    std::memset(expected.data() + 16, 0, 48);
    std::memset(expected.data() + 96, 0, 16);
    if (data != expected) return false;
    data.fill(0x4d);
    expected = data;
    std::memset(expected.data() + 16, 0, 32);
    return clear(1, ranges, nullptr, 0x81) == S_OK && data == expected;
}
#endif

int wmain(int argc, wchar_t** argv) {
#if defined(ORD_PixGetPixieEntryPoints) && defined(ORD_D3D12UploadCustomMicrocodeX)
    if (argc == 2 && !std::wcscmp(argv[1], L"--extra-only")) {
        const HMODULE module = GetModuleHandleW(L"d3d12_xs.dll");
        if (!module || !check_extra_exports(module)) return 1;
        std::puts("Additional export checks passed");
        return 0;
    }
#endif
    int arg = 1;
    // Pin the packaged driver during a control run, as a live device would.
    if (argc > 2 && !std::wcscmp(argv[arg], L"--reference-umd")) {
        if (!LoadLibraryW(argv[arg + 1])) {
            std::fprintf(stderr, "Could not load reference driver (%lu)\n", GetLastError());
            return 2;
        }
        arg += 2;
    }
    if (argc != arg && (argc != arg + 2 || std::wcscmp(argv[arg], L"--report"))) return 2;
    const HMODULE module = GetModuleHandleW(L"d3d12_xs.dll");
    if (!module || !check_debug(module)) { std::fputs("Debug interface failed\n", stderr); return 1; }
    if (!check_dxgix(module)) { std::fputs("Legacy DXGIX calls failed\n", stderr); return 1; }
    if (!check_blob(module)) { std::fputs("Blob checks failed\n", stderr); return 1; }
#if defined(ORD_PixGetPixieEntryPoints) && defined(ORD_D3D12UploadCustomMicrocodeX)
    if (!check_extra_exports(module)) { std::fputs("Additional export checks failed\n", stderr); return 1; }
#endif
#if defined(ORD_GetFrameBufferPreserveMemorySizeBytes) && defined(ORD_D3D12ConfigureVirtualMemoryX)
    if (!check_memory_config_invalid(module)) { std::fputs("Memory configuration checks failed\n", stderr); return 1; }
#endif
#ifdef ORD_D3D12ZeroMemoryX
    if (!check_zero_memory(module)) { std::fputs("CPU memory clear failed\n", stderr); return 1; }
    const char* summary = "d3d12-exports PASS debug=1 blob=1 dxgix=4 cpu_clear=1\n";
#else
    const char* summary = "d3d12-exports PASS debug=1 blob=1 dxgix=4\n";
#endif
    FILE* out = stdout;
    if (argc != arg && _wfopen_s(&out, argv[arg + 1], L"wb")) return 2;
    const int result = std::fputs(summary, out);
    return (out != stdout && std::fclose(out)) || result < 0;
}
