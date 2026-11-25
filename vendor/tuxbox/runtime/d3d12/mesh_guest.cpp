// Check the Xbox mesh stream, direct dispatch, and the GPU-supplied indirect record.
#include "mesh_fixture.h"
#include "frame_cli.h"
#include "mesh_ms.h"
#include "mesh_ms_payload.h"
#include "mesh_as.h"
#include "mesh_ps.h"
#include <algorithm>

namespace {
using namespace mesh_fixture;
constexpr UINT pixels = 64, cases = 4, guard = 4, block = pixels + 2 * guard;
constexpr UINT sentinel = 0x98765432;
using Words = std::array<UINT, block * cases>;

void check_feature(ID3D12Device* device) {
    struct Options {
        D3D12_FEATURE_DATA_D3D12_OPTIONS7 caps;
        UINT guard;
    } opt{};
    opt.guard = sentinel;
    if (device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &opt, sizeof(opt.caps) - 1) != E_INVALIDARG)
        throw std::runtime_error("short mesh options accepted");
    check(device->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS7, &opt, sizeof(opt)), "Xbox mesh options");
    if (opt.guard != sentinel) throw std::runtime_error("mesh options overwrote guard");
    if (opt.caps.MeshShaderTier != D3D12_MESH_SHADER_TIER_1)
        throw std::runtime_error("selected adapter lacks mesh shader tier 1");
}

std::string run(const wchar_t* adapter, bool) {
    Context ctx(adapter, cases);
    auto* device = ctx.device.Get();
    auto* list = ctx.list.Get();
    check_feature(device);
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER param{};
    param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    param.DescriptorTable = {1, &range};
    D3D12_ROOT_SIGNATURE_DESC desc{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    auto root = ctx.root(desc);
    auto s = stream(root.Get(), {mesh_ms, sizeof(mesh_ms)}, {mesh_ps, sizeof(mesh_ps)});
    s.topology.payload = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT; // Frosty's 680-byte stream value
    ComPtr<ID3D12PipelineState> plain, amplified, invalid;
    auto reject = [&](const char* message) {
        if (SUCCEEDED(ctx.create_pipeline(s, invalid)) || invalid) throw std::runtime_error(message);
    };
    s.stencil[16] = 1;
    reject("inconsistent Xbox stencil masks accepted");
    s.stencil[16] = 0;
    std::swap(s.ms.payload, s.ps.payload);
    reject("pixel shader accepted as mesh");
    std::swap(s.ms.payload, s.ps.payload);
    s.as.payload = s.ms.payload;
    reject("mesh shader accepted as amplification");
    s.as.payload = {};
    check(ctx.create_pipeline(s, plain), "Xbox MS/PS mesh pipeline");
    s.as.payload = {mesh_as, sizeof(mesh_as)};
    s.ms.payload = {mesh_ms_payload, sizeof(mesh_ms_payload)};
    check(ctx.create_pipeline(s, amplified), "Xbox AS/MS/PS mesh pipeline");

    Words expected{};
    expected.fill(sentinel);
    for (UINT i = 0; i < cases; ++i)
        std::fill_n(expected.begin() + i * block + guard, pixels, 0);
    auto seed = buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(Words), D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(seed.Get(), expected.data(), sizeof(Words));
    auto output = buffer(device, D3D12_HEAP_TYPE_DEFAULT, sizeof(Words), D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device, D3D12_HEAP_TYPE_READBACK, sizeof(Words), D3D12_RESOURCE_STATE_COPY_DEST);
    list->CopyBufferRegion(output.Get(), 0, seed.Get(), 0, sizeof(Words));
    transition(list, output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    for (UINT i = 0; i < cases; ++i) {
        D3D12_UNORDERED_ACCESS_VIEW_DESC view{};
        view.Format = DXGI_FORMAT_R32_TYPELESS;
        view.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        view.Buffer.FirstElement = i * block + guard;
        view.Buffer.NumElements = pixels;
        view.Buffer.Flags = D3D12_BUFFER_UAV_FLAG_RAW;
        device->CreateUnorderedAccessView(output.Get(), nullptr, &view, ctx.cpu(i));
    }
    D3D12_INDIRECT_ARGUMENT_DESC action{};
    action.Type = static_cast<D3D12_INDIRECT_ARGUMENT_TYPE>(107);
    D3D12_COMMAND_SIGNATURE_DESC signatureDesc{16, 1, &action, 1};
    ComPtr<ID3D12CommandSignature> signature;
    check(device->CreateCommandSignature(&signatureDesc, nullptr, IID_PPV_ARGS(&signature)), "mesh signature");
    // Three 16-byte records following a prefix. The count limits the first call to two records.
    const UINT args[]{0, 0, 0, 0, 2, 1, 1, 0, 1, 2, 1, 0, 1, 1, 1, 0, 2, 0};
    auto arguments = buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(args), D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(arguments.Get(), args, sizeof(args));

    ctx.bind(root.Get(), 8, 8);
    list->SetPipelineState(plain.Get());
    ctx.table(0, 0);
    ctx.dispatch(2, 1, 3);
    ctx.dispatch(2, 0, 3);
    list->SetPipelineState(amplified.Get());
    ctx.table(0, 1);
    ctx.dispatch(2, 3, 1);
    list->SetPipelineState(plain.Get());
    for (UINT i = 0; i < 2; ++i) {
        ctx.table(0, 2 + i);
        list->ExecuteIndirect(signature.Get(), 3, arguments.Get(), 16, arguments.Get(), 64 + 4 * i);
    }
    transition(list, output.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, sizeof(Words));
    ctx.submit();

    // MS adds 1+2*x+5*y+11*z per group. Each AS launches two child groups
    // whose base contributions are 1 and 6, plus its parent payload for each.
    const UINT sums[]{78, 6 * 7 + 2 * (6 * 100 + 3 * 13 + 6 * 19), (1 + 3) + (1 + 6), 0};
    for (UINT i = 0; i < cases; ++i)
        std::fill_n(expected.begin() + i * block + guard, pixels, sums[i]);
    D3D12_RANGE read{0, sizeof(Words)}, empty{};
    void* mapped = nullptr;
    check(readback->Map(0, &read, &mapped), "map mesh pixels");
    Words observed;
    std::memcpy(observed.data(), mapped, sizeof(Words));
    readback->Unmap(0, &empty);
    for (size_t i = 0; i < expected.size(); ++i)
        if (expected[i] != observed[i])
            throw std::runtime_error("word " + std::to_string(i) + " expected " + std::to_string(expected[i]) +
                                     " observed " + std::to_string(observed[i]));
    check(ctx.allocator->Reset(), "reset mesh allocator");
    check(list->Reset(ctx.allocator.Get(), plain.Get()), "reset mesh list");
    ctx.dispatch(65536, 1, 1);
    if (list->Close() != E_INVALIDARG) throw std::runtime_error("oversized dispatch accepted");

    char receipt[200];
    std::snprintf(receipt, sizeof(receipt),
                  "d3d12-mesh PASS pci=%04x:%04x tier=1 stream=680 direct=6 "
                  "amplified=12 indirect=2 zero=0 pixels=256/256 guards=32 rejected=5",
                  ctx.adapter.VendorId, ctx.adapter.DeviceId);
    return receipt;
}
}
int wmain(int argc, wchar_t** argv) { return tuxbox_test::frame_main(argc, argv, "d3d12-mesh", false, run); }
