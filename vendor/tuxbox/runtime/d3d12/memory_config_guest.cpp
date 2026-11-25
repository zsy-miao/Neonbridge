#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <cstdint>
#include <cstdio>
#include <cwchar>

struct Config {
    uint32_t flags, entries, preserve_bytes, padding;
    uint64_t out0, out1;
};

int wmain(int argc, wchar_t** argv) {
    if (argc != 3) return 2;
    const bool provider = std::wcscmp(argv[1], L"--provider") == 0;
    if (!provider && std::wcscmp(argv[1], L"--reference")) return 2;
    HMODULE module = LoadLibraryW(L"d3d12_xs.dll");
    if (!module) return 3;
    using Getter = uint32_t (WINAPI*)();
    using Configure = HRESULT (WINAPI*)(const Config*);
    const auto get_address = GetProcAddress(module, "GetFrameBufferPreserveMemorySizeBytes");
    const auto configure_address = GetProcAddress(module, "D3D12ConfigureVirtualMemoryX");
    if (!get_address || !configure_address || get_address != GetProcAddress(module, MAKEINTRESOURCEA(2)) ||
        configure_address != GetProcAddress(module, MAKEINTRESOURCEA(5))) return 4;
    auto get = reinterpret_cast<Getter>(get_address);
    auto configure = reinterpret_cast<Configure>(configure_address);
    const auto initial = get();
    Config bad_flags{2, 0, 123, 0, 0x123456789abcdef0ull, 0xfedcba9876543210ull};
    Config bad_count{0, 34, 123, 0, 0x123456789abcdef0ull, 0xfedcba9876543210ull};
    const auto null_hr = configure(nullptr);
    const auto flags_hr = configure(&bad_flags);
    const auto count_hr = configure(&bad_count);
    const auto after_invalid = get();
    if (initial || null_hr != E_INVALIDARG || flags_hr != E_INVALIDARG || count_hr != E_INVALIDARG || after_invalid ||
        bad_flags.out0 != 0x123456789abcdef0ull || bad_flags.out1 != 0xfedcba9876543210ull ||
        bad_count.out0 != 0x123456789abcdef0ull || bad_count.out1 != 0xfedcba9876543210ull) return 5;
    if (provider) {
        Config valid{0, 0, 123, 0, 0x123456789abcdef0ull, 0xfedcba9876543210ull};
        if (configure(&valid) != E_FAIL || get() != 123 || valid.out0 != 0 || valid.out1 != 0) return 6;
        valid.flags = 1;
        valid.entries = 33;
        valid.preserve_bytes = 456;
        if (configure(&valid) != E_FAIL || get() != 456 || valid.out0 != 0 || valid.out1 != 0) return 6;
        Config request{0, 1, 0, 0, 0, 0};
        if (configure(&request) != 0x10000000 || get() != 0 ||
            request.out0 != 0x80000000ull || request.out1 != 0x7f8000000ull) return 8;
        Config retry{1, 33, 0, 0, 0, 0};
        if (configure(&retry) != E_FAIL || retry.out0 != request.out0 || retry.out1 != request.out1) return 9;
    }
    FILE* file = nullptr;
    if (_wfopen_s(&file, argv[2], L"wb")) return 7;
    const int n = std::fprintf(file, "initial=0\nnull=E_INVALIDARG\ninvalid_flags=E_INVALIDARG\ninvalid_count=E_INVALIDARG\nafter_invalid=0\noutputs_unchanged=1\n%s", provider ? "preserve_allocation=E_FAIL\nzero_preserve_config=0x10000000\nrepeat=E_FAIL\n" : "");
    return std::fclose(file) || n < 0;
}
