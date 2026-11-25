// Common command-line and receipt handling for the two independent frame guests.
#pragma once
#include <cstdio>
#include <cwchar>
#include <filesystem>
#include <fstream>
#include <stdexcept>
#include <string>

namespace tuxbox_test {
inline bool valid_pci_id(const wchar_t* value) {
    const std::wstring id(value);
    constexpr auto hex = L"0123456789abcdefABCDEF";
    return id.size() == 9 && id[4] == L':' && id.find_first_not_of(hex) == 4 &&
           id.find_first_not_of(hex, 5) == id.npos;
}

template<class Run>
int frame_main(int argc, wchar_t** argv, const char* name, bool allow_present, Run run, const wchar_t* mode_flag = L"--present", const char* mode_usage = " [--present]") {
    const wchar_t* adapter = nullptr;
    const wchar_t* report = nullptr;
    bool present = false;
    for (int i = 1; i < argc; ++i) {
        if (!std::wcscmp(argv[i], L"--adapter") && !adapter && i + 1 < argc && valid_pci_id(argv[i + 1])) {
            adapter = argv[++i];
        } else if (!std::wcscmp(argv[i], L"--report") && !report && i + 1 < argc && *argv[i + 1]) {
            report = argv[++i];
        } else if (allow_present && !std::wcscmp(argv[i], mode_flag) && !present) {
            present = true;
        } else {
            std::fprintf(stderr, "usage: %s.exe [--adapter VVVV:DDDD]%s [--report FILE]\n",
                         name, allow_present ? mode_usage : "");
            return 2;
        }
    }
    std::ofstream file;
    try {
        if (report) {
            file.open(std::filesystem::path(report), std::ios::binary | std::ios::trunc);
            if (!file) throw std::runtime_error("cannot create report");
        }
        const auto receipt = run(adapter, present);
        if (report) {
            file << receipt << '\n';
            file.close();
            if (!file) throw std::runtime_error("cannot write report");
        }
        std::puts(receipt.c_str());
        return 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "%s failed: %s\n", name, error.what());
        if (file.is_open()) file << name << " failed: " << error.what() << '\n';
        return 1;
    }
}
}
