// Read console data through the guest IAT and check both GetProcAddress paths.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d12.h>
#include <d3d12sdklayers.h>
#include <dxgi1_4.h>
#include <cstring>
#include <cstdio>
#include <cwchar>
#include <iterator>

// These console IDs are present in both supplied DLLs.
struct __declspec(uuid("30bc77b9-8379-4b25-95de-b8588c42275d")) ID3D12XboxDmaCommandList;
struct __declspec(uuid("aceeea63-e0a9-4a1c-bbec-71b2f485f758")) IGraphicsUnknown;

#define GUID_EXPORT(symbol, expected, ordinal) extern "C" const GUID* const __imp_##symbol;
#include "guid-exports.inc"
#undef GUID_EXPORT

int wmain(int argc, wchar_t** argv) {
    if (argc != 1 && (argc != 3 || std::wcscmp(argv[1], L"--report"))) return 2;
    struct Entry {
        const char* name;
        const GUID* imported;
        const GUID* expected;
        WORD ordinal;
    };
    const Entry entries[]{
#define GUID_EXPORT(symbol, expected, ordinal) {#symbol, __imp_##symbol, &expected, ordinal},
#include "guid-exports.inc"
#undef GUID_EXPORT
    };
    const HMODULE module = GetModuleHandleW(L"d3d12_xs.dll");
    static_assert(std::size(entries) > 0);
    for (const auto& entry : entries) {
        if (!module || !entry.imported ||
            reinterpret_cast<const void*>(GetProcAddress(module, entry.name)) != entry.imported ||
            reinterpret_cast<const void*>(GetProcAddress(module, MAKEINTRESOURCEA(entry.ordinal))) != entry.imported ||
            std::memcmp(entry.imported, entry.expected, sizeof(GUID))) {
            std::fprintf(stderr, "Mismatch for %s at ordinal %u\n", entry.name, entry.ordinal);
            return 1;
        }
    }
    FILE* out = stdout;
    if (argc == 3 && _wfopen_s(&out, argv[2], L"wb")) return 2;
    const int result = std::fprintf(out, "d3d12-guids PASS identifiers=%zu bytes=%zu\n",
                                    std::size(entries), sizeof(GUID) * std::size(entries));
    const bool closed = out == stdout || std::fclose(out) == 0;
    return result < 0 || !closed;
}
