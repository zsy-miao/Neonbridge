// Round-trip portable root signatures using the names imported by Xbox guests.
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <d3d12.h>
#include <wrl/client.h>
#include "frame_cli.h"

using Microsoft::WRL::ComPtr;

namespace {
void check(HRESULT hr, const char* op) {
    if (FAILED(hr)) {
        char text[128];
        std::snprintf(text, sizeof(text), "%s failed with 0x%08lx", op, static_cast<unsigned long>(hr));
        throw std::runtime_error(text);
    }
}

ComPtr<ID3DBlob> serialize(const D3D12_VERSIONED_ROOT_SIGNATURE_DESC& desc) {
    ComPtr<ID3DBlob> blob, error;
    check(D3D12SerializeVersionedRootSignature(&desc, &blob, &error), "serialize");
    if (!blob || !blob->GetBufferSize()) throw std::runtime_error("empty signature");
    return blob;
}

std::string run(const wchar_t*, bool) {
    D3D12_ROOT_PARAMETER root{};
    root.ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    root.Constants = {3, 2, 5};
    root.ShaderVisibility = D3D12_SHADER_VISIBILITY_VERTEX;
    D3D12_VERSIONED_ROOT_SIGNATURE_DESC desc{};
    desc.Version = D3D_ROOT_SIGNATURE_VERSION_1_0;
    desc.Desc_1_0 = {1, &root, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    auto blob = serialize(desc);
    ComPtr<ID3D12RootSignatureDeserializer> legacy;
    check(D3D12CreateRootSignatureDeserializer(blob->GetBufferPointer(), blob->GetBufferSize(),
                                               IID_PPV_ARGS(&legacy)), "read v1.0");
    blob.Reset();
    const auto* plain = legacy->GetRootSignatureDesc();
    if (!plain || plain->NumParameters != 1 || !plain->pParameters ||
        plain->pParameters->ParameterType != root.ParameterType ||
        plain->pParameters->ShaderVisibility != root.ShaderVisibility ||
        plain->pParameters->Constants.ShaderRegister != 3 ||
        plain->pParameters->Constants.RegisterSpace != 2 ||
        plain->pParameters->Constants.Num32BitValues != 5)
        throw std::runtime_error("v1.0 root constants differ");

    D3D12_DESCRIPTOR_RANGE1 range{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 7, 5, 3,
                                  D3D12_DESCRIPTOR_RANGE_FLAG_DATA_STATIC, 1};
    D3D12_ROOT_PARAMETER1 table{};
    table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    table.DescriptorTable = {1, &range};
    table.ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
    desc.Version = D3D_ROOT_SIGNATURE_VERSION_1_1;
    desc.Desc_1_1 = {1, &table, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    blob = serialize(desc);
    ComPtr<ID3D12VersionedRootSignatureDeserializer> versioned;
    check(D3D12CreateVersionedRootSignatureDeserializer(blob->GetBufferPointer(), blob->GetBufferSize(),
                                                        IID_PPV_ARGS(&versioned)), "read v1.1");
    blob.Reset();
    const auto* direct = versioned->GetUnconvertedRootSignatureDesc();
    if (!direct || direct->Version != D3D_ROOT_SIGNATURE_VERSION_1_1 ||
        direct->Desc_1_1.NumParameters != 1 || !direct->Desc_1_1.pParameters)
        throw std::runtime_error("missing v1.1 signature");
    const auto& p = direct->Desc_1_1.pParameters[0];
    if (p.ParameterType != table.ParameterType || p.ShaderVisibility != table.ShaderVisibility ||
        p.DescriptorTable.NumDescriptorRanges != 1 || !p.DescriptorTable.pDescriptorRanges)
        throw std::runtime_error("v1.1 descriptor table differs");
    const auto& actual = p.DescriptorTable.pDescriptorRanges[0];
    if (actual.RangeType != range.RangeType || actual.NumDescriptors != 7 || actual.BaseShaderRegister != 5 ||
        actual.RegisterSpace != 3 || actual.OffsetInDescriptorsFromTableStart != 1 || actual.Flags != range.Flags)
        throw std::runtime_error("v1.1 descriptor range differs");

    unsigned char truncated[]{'D', 'X', 'B', 'C'};
    legacy.Reset();
    versioned.Reset();
    if (SUCCEEDED(D3D12CreateRootSignatureDeserializer(truncated, sizeof(truncated), IID_PPV_ARGS(&legacy))) ||
        legacy || SUCCEEDED(D3D12CreateVersionedRootSignatureDeserializer(
                      truncated, sizeof(truncated), IID_PPV_ARGS(&versioned))) || versioned)
        throw std::runtime_error("accepted truncated bytecode");

    return "d3d12-signatures PASS versions=1.0,1.1 ranges=1 invalid=2";
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-signatures", false, run);
}
