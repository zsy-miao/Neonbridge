#include <dxc/dxcapi.h>

#include <fstream>
#include <iostream>
#include <iterator>
#include <stdexcept>
#include <string>

static void check(HRESULT status, const char *operation) {
    if (FAILED(status)) throw std::runtime_error(operation);
}

static void compile(IDxcCompiler3 *compiler, const std::string &source_path,
                    const std::string &output_path) {
    std::ifstream input(source_path, std::ios::binary);
    if (!input) throw std::runtime_error("open HLSL input");
    const std::string source{std::istreambuf_iterator<char>{input}, {}};
    const std::wstring wide_name(source_path.begin(), source_path.end());
    LPCWSTR arguments[] = {wide_name.c_str(), L"-T", L"cs_6_0", L"-E", L"main"};
    const DxcBuffer buffer{source.data(), source.size(), DXC_CP_UTF8};
    CComPtr<IDxcResult> result;
    check(compiler->Compile(&buffer, arguments, 5, nullptr, IID_PPV_ARGS(&result)), "compile call");
    HRESULT status{};
    check(result->GetStatus(&status), "read compile status");
    check(status, "compile failed");
    CComPtr<IDxcBlob> bytecode;
    check(result->GetOutput(DXC_OUT_OBJECT, IID_PPV_ARGS(&bytecode), nullptr), "get DXIL");
    std::ofstream output(output_path, std::ios::binary);
    output.write(static_cast<const char *>(bytecode->GetBufferPointer()), bytecode->GetBufferSize());
    output.close();
    if (!output) throw std::runtime_error("write DXIL");
}

int main() {
    try {
        CComPtr<IDxcCompiler3> compiler;
        check(DxcCreateInstance(CLSID_DxcCompiler, IID_PPV_ARGS(&compiler)), "create compiler");
        std::string source, output;
        while (std::getline(std::cin, source)) {
            if (!std::getline(std::cin, output)) throw std::runtime_error("missing output path");
            compile(compiler, source, output);
            std::cout << "ok\n" << std::flush;
        }
        if (!std::cin.eof()) throw std::runtime_error("read compiler request");
    } catch (const std::exception &error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
