// Exercise both Scarlett XDXR factories through the named and ordinal entry points.
#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <objidl.h>
#include <dxcapi.h>
#include <d3d12.h>
#include <wrl/client.h>
#include <cstdio>
#include <cwchar>
#include <stdexcept>
#include <string>

using Microsoft::WRL::ComPtr;
constexpr GUID compiler_id{0x16bb3c85,0x006d,0x4b72,{0x9e,0x10,0x63,0xcd,0x97,0xdf,0x57,0xf0}};
extern "C" __declspec(dllimport) HRESULT WINAPI CreateXdxrCompiler(REFCLSID, REFIID, void**);
extern "C" __declspec(dllimport) HRESULT WINAPI CreateXdxrCompiler2(IMalloc*, REFCLSID, REFIID, void**);

struct Bytecode { const void* data; UINT size; };
struct Export { const wchar_t* source; const wchar_t* name; UINT flags; };
struct Library { Bytecode code; UINT count; Export* exports; };
using Rename = HRESULT (STDMETHODCALLTYPE*)(IUnknown*, const Bytecode*, UINT, Export*, UINT, IDxcOperationResult**);
using RenameGrouped = HRESULT (STDMETHODCALLTYPE*)(IUnknown*, const Library*, UINT, Export*, UINT, IDxcOperationResult**);

static void require(bool value, const char* message) {
    if (!value) throw std::runtime_error(message);
}

static void verify_library(IDxcCompiler3* dxc, IDxcOperationResult* result) {
    HRESULT status;
    require(result && result->GetStatus(&status) == S_OK && status == S_OK, "library link failed");
    ComPtr<IDxcBlob> blob;
    require(result->GetResult(&blob) == S_OK && blob, "missing linked shader");
    DxcBuffer buffer{blob->GetBufferPointer(), blob->GetBufferSize(), 0};
    ComPtr<IDxcResult> disassembly;
    require(dxc->Disassemble(&buffer, IID_PPV_ARGS(&disassembly)) == S_OK, "output is not DXIL");
    ComPtr<IDxcBlobUtf8> listing;
    require(disassembly->GetOutput(DXC_OUT_DISASSEMBLY, IID_PPV_ARGS(&listing), nullptr) == S_OK && listing,
            "missing DXIL disassembly");
    std::string assembly(listing->GetStringPointer(), listing->GetStringLength());
    require(assembly.find("AliasedMiss") != std::string::npos && assembly.find("OtherMiss") == std::string::npos &&
            assembly.find("PrimaryMiss") == std::string::npos && assembly.find(", 42") != std::string::npos,
            "wrong exports or shader body after link");
}

static void run() {
    HMODULE dll = GetModuleHandleW(L"dxcompiler_xs.dll");
    require(dll && GetProcAddress(dll, MAKEINTRESOURCEA(1)) == GetProcAddress(dll, "CreateXdxrCompiler2") &&
            GetProcAddress(dll, MAKEINTRESOURCEA(2)) == GetProcAddress(dll, "CreateXdxrCompiler"), "XDXR ordinals differ");
    ComPtr<IUnknown> normal, custom;
    require(CreateXdxrCompiler(compiler_id, compiler_id, reinterpret_cast<void**>(normal.GetAddressOf())) == S_OK && normal,
            "CreateXdxrCompiler failed");
    ComPtr<IMalloc> allocator;
    require(CoGetMalloc(1, &allocator) == S_OK, "no COM allocator");
    require(CreateXdxrCompiler2(allocator.Get(), compiler_id, compiler_id, reinterpret_cast<void**>(custom.GetAddressOf())) == S_OK && custom,
            "CreateXdxrCompiler2 failed");

    ComPtr<IDxcCompiler3> dxc;
    require(DxcCreateInstance(CLSID_DxcCompiler, IID_PPV_ARGS(&dxc)) == S_OK, "DXC creation failed");
    constexpr char shader[] = "struct Payload {uint v;};"
        "[shader(\"miss\")] void PrimaryMiss(inout Payload p) {p.v += 42;}"
        "[shader(\"miss\")] void OtherMiss(inout Payload p) {p.v += 67;}";
    DxcBuffer source{shader, sizeof(shader)-1, DXC_CP_UTF8};
    LPCWSTR args[] = {L"-T", L"lib_6_3", L"-Vd"};
    ComPtr<IDxcResult> build;
    require(dxc->Compile(&source, args, ARRAYSIZE(args), nullptr, IID_PPV_ARGS(&build)) == S_OK, "input compile failed");
    ComPtr<IDxcBlob> blob;
    require(build->GetResult(&blob) == S_OK && blob, "missing input ray library");
    Bytecode code{blob->GetBufferPointer(), static_cast<UINT>(blob->GetBufferSize())};
    Export renamed{L"\x0001?PrimaryMiss@@YAXUPayload@@@Z", L"AliasedMiss", 0x400};
    ComPtr<IDxcOperationResult> linked;
    auto rename = reinterpret_cast<Rename>((*reinterpret_cast<void***>(normal.Get()))[6]);
    require(rename(normal.Get(), &code, 1, &renamed, 1, &linked) == S_OK && renamed.flags == 0x400,
            "flat rename failed");
    verify_library(dxc.Get(), linked.Get());

    auto grouped = reinterpret_cast<RenameGrouped>((*reinterpret_cast<void***>(custom.Get()))[11]);
    Library library{code, 1, &renamed};
    Export selected{nullptr, L"AliasedMiss", 0x800};
    linked.Reset();
    require(grouped(custom.Get(), &library, 1, &selected, 1, &linked) == S_OK &&
            renamed.flags == 0x400 && selected.flags == 0x800, "grouped rename failed");
    verify_library(dxc.Get(), linked.Get());

    // The Series UMD supplies one shader table association per export. This
    // library only touches its ray payload and has no shader record resources.
    struct Binding { LPCWSTR name; const D3D12_VERSIONED_ROOT_SIGNATURE_DESC* local; };
    struct Bindings {
        const Binding* entries; UINT count; UINT reserved;
        const D3D12_VERSIONED_ROOT_SIGNATURE_DESC* global;
        UINT sampler_bytes, view_bytes, identifier_bytes, reserved2;
    };
    using Patch = HRESULT (STDMETHODCALLTYPE*)(IUnknown*, const Bytecode*, const Bindings*, IDxcOperationResult**);
    D3D12_VERSIONED_ROOT_SIGNATURE_DESC global{};
    global.Version = D3D_ROOT_SIGNATURE_VERSION_1_0;
    Binding entry{L"AliasedMiss", nullptr};
    Bindings tables{&entry, 1, 0, &global, 32, 32, 32, 0};
    ComPtr<IDxcBlob> aliased;
    require(linked->GetResult(&aliased) == S_OK && aliased, "missing renamed library");
    Bytecode to_patch{aliased->GetBufferPointer(), static_cast<UINT>(aliased->GetBufferSize())};
    ComPtr<IDxcOperationResult> patched;
    auto patch = reinterpret_cast<Patch>((*reinterpret_cast<void***>(normal.Get()))[12]);
    require(patch(normal.Get(), &to_patch, &tables, &patched) == S_OK, "shader table patch failed");
    verify_library(dxc.Get(), patched.Get());

    struct SingleBinding {
        LPCWSTR name;
        const D3D12_VERSIONED_ROOT_SIGNATURE_DESC* global;
        const D3D12_VERSIONED_ROOT_SIGNATURE_DESC* local;
        UINT sampler_bytes, view_bytes, identifier_bytes;
    };
    using PatchSingle = HRESULT (STDMETHODCALLTYPE*)(IUnknown*, LPCWSTR, const Bytecode*, const SingleBinding*, IDxcOperationResult**);
    SingleBinding single{entry.name, &global, nullptr, 32, 32, 32};
    auto patch_single = reinterpret_cast<PatchSingle>((*reinterpret_cast<void***>(custom.Get()))[7]);
    patched.Reset();
    require(patch_single(custom.Get(), entry.name, &to_patch, &single, &patched) == S_OK, "single shader table patch failed");
    verify_library(dxc.Get(), patched.Get());
}

int wmain(int argc, wchar_t** argv) {
    FILE* out = stdout;
    if (argc != 1 && (argc != 3 || std::wcscmp(argv[1], L"--report") || _wfopen_s(&out, argv[2], L"wb"))) return 2;
    try { run(); }
    catch (const std::exception& error) {
        std::fprintf(stderr, "xdxr-link failed: %s\n", error.what());
        if (out != stdout) std::fclose(out);
        return 1;
    }
    const int written = std::fputs("xdxr-link PASS factories=2 flat=1 grouped=1 retained-shader-body=1 shader-tables=single,multiple\n", out);
    return (out != stdout && std::fclose(out)) || written < 0;
}
