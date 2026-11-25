// Cross-link a Frosty miss shader with the supplied Series compiler and driver.
#include <windows.h>
#include <dxcapi.h>
#include <wrl/client.h>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <stdexcept>
#include <string>
#include <vector>

using Microsoft::WRL::ComPtr;
constexpr GUID compiler_id{0x16bb3c85,0x006d,0x4b72,{0x9e,0x10,0x63,0xcd,0x97,0xdf,0x57,0xf0}};
struct Bytecode { const void* bytes; UINT size; };
struct Info { UINT state, stack_bytes; };
struct Field { const char* declaration; size_t code; };
using Compile = HRESULT (WINAPI*)(IUnknown*, const Bytecode*, UINT, LPCWSTR*, Info*, UINT, UINT, IDxcOperationResult**);
using Link = HRESULT (WINAPI*)(IUnknown*, LPCWSTR, IDxcBlob**, UINT, LPCWSTR*, UINT, UINT, UINT, IDxcOperationResult**);

static void check(bool success, const char* stage) {
    if (!success) throw std::runtime_error(stage);
}
static std::string read_file(const char* name) {
    std::ifstream input(name, std::ios::binary);
    check(input.good(), name);
    return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}

struct Driver {
    std::string runtime = read_file("runtime.ll");
    std::vector<std::string> names;
    std::vector<Field> fields;
    Driver() {
        std::ifstream input("fields.tsv");
        for (std::string line; std::getline(input, line);) {
            size_t tab = line.rfind('\t');
            check(tab != std::string::npos, "malformed field");
            names.push_back(line.substr(0, tab));
            fields.push_back({nullptr, std::stoul(line.substr(tab + 1))});
        }
        check(fields.size() == 42, "unexpected UMD field count");
        for (size_t i = 0; i < fields.size(); ++i) fields[i].declaration = names[i].c_str();
    }
    virtual const char* source() { return runtime.c_str(); }
    virtual void describe(Field* dst, UINT* count) {
        *count = static_cast<UINT>(fields.size());
        if (dst) std::memcpy(dst, fields.data(), fields.size() * sizeof(Field));
    }
    virtual UINT attribute_words() { return 2; }
    virtual UINT payload_words() { return 5; }
    virtual UINT field_count() { return static_cast<UINT>(fields.size()); }
    virtual INT64 stop_state() { return -3; }
};

static ComPtr<IDxcBlob> output(IDxcOperationResult* operation) {
    HRESULT status;
    ComPtr<IDxcBlob> blob;
    check(operation && SUCCEEDED(operation->GetStatus(&status)) && SUCCEEDED(status), "compiler diagnostic");
    check(SUCCEEDED(operation->GetResult(&blob)) && blob, "missing output");
    return blob;
}
static ComPtr<IUnknown> factory(HMODULE dll, Driver& driver) {
    using Create = HRESULT (WINAPI*)(REFCLSID, REFIID, void**);
    auto create = reinterpret_cast<Create>(GetProcAddress(dll, "CreateXdxrCompiler"));
    ComPtr<IUnknown> object;
    check(create && SUCCEEDED(create(compiler_id, compiler_id, reinterpret_cast<void**>(object.GetAddressOf()))), "factory");
    auto table = *reinterpret_cast<void***>(object.Get());
    reinterpret_cast<void (WINAPI*)(IUnknown*, Driver*)>(table[10])(object.Get(), &driver);
    return object;
}
int wmain(int argc, wchar_t** argv) {
    try {
        check(argc == 2, "expected shader export");
        HMODULE control = LoadLibraryW(L"control-xs.dll");
        HMODULE provider = LoadLibraryW(L"dxcompiler_xs.dll");
        check(control && provider, "load compiler DLLs");
        Driver driver;
        auto reference = factory(control, driver);
        auto tuxbox = factory(provider, driver);
        auto link = reinterpret_cast<Link>((*reinterpret_cast<void***>(reference.Get()))[9]);
        auto create = reinterpret_cast<decltype(&DxcCreateInstance)>(GetProcAddress(control, "DxcCreateInstance"));
        ComPtr<IDxcCompiler> disassembler;
        check(create && SUCCEEDED(create(CLSID_DxcCompiler, IID_PPV_ARGS(&disassembler))), "disassembler");
        std::string data[] = {read_file("shader.dxil"), read_file("callbacks.dxil"), read_file("scheduler.dxil")};
        Bytecode libs[3]{};
        for (UINT i = 0; i < 3; ++i) libs[i] = {data[i].data(), static_cast<UINT>(data[i].size())};
        LPCWSTR name = argv[1];
        std::ofstream report("report.txt");
        for (auto compiler : {reference.Get(), tuxbox.Get()}) {
            auto compile = reinterpret_cast<Compile>((*reinterpret_cast<void***>(compiler))[8]);
            Info info{~0u, ~0u};
            ComPtr<IDxcOperationResult> result;
            check(SUCCEEDED(compile(compiler, libs, 3, &name, &info, 1, 8, &result)), "compile miss shader");
            auto collection = output(result.Get());
            check(info.state == 0 && info.stack_bytes == 4, "leaf state and stack");
            result.Reset();
            IDxcBlob* input = collection.Get();
            check(SUCCEEDED(link(reference.Get(), L"main", &input, 1, &name, 1, 8, 1024, &result)), "reference scheduler link");
            auto shader = output(result.Get());
            ComPtr<IDxcBlobEncoding> listing;
            check(SUCCEEDED(disassembler->Disassemble(shader.Get(), &listing)), "disassemble scheduler");
            std::string text(static_cast<const char*>(listing->GetBufferPointer()), listing->GetBufferSize());
            std::string entry(name, name + std::wcslen(name));
            auto pos = text.find("@" + entry + ".ss_0(");
            auto end = text.find("\n}", pos);
            check(pos != std::string::npos && end != std::string::npos &&
                  text.substr(pos, end-pos).find("i32 -1082130432") != std::string::npos, "missing -1.0 payload store in state");
            report << (compiler == reference.Get() ? "reference" : "tuxbox") << ' ' << info.state << ' '
                   << info.stack_bytes << ' ' << collection->GetBufferSize() << ' ' << shader->GetBufferSize() << '\n';
        }
        return report.good() ? 0 : 1;
    } catch (const std::exception& e) {
        std::fprintf(stderr, "xdxr-continuation: %s\n", e.what());
        return 1;
    }
}
