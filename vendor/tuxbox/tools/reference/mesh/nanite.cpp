// Exercise retail HWRasterizeMS and HWRasterizePS with a small packed cluster.
#include "mesh_fixture.h"
#include "cache_controls.h"
#include "frame_cli.h"
#include <filesystem>
#include <fstream>
#include <iomanip>
#include <vector>

namespace {
using namespace mesh_fixture;
constexpr UINT width = 19, height = 13, pitch = 256;
constexpr D3D12_RECT region{3, 2, 16, 10};
constexpr UINT dataSize = pitch * height;
constexpr UINT readbackStride = (dataSize + 511) & ~511u;
constexpr UINT cases = 7;
using Data = std::array<BYTE, dataSize>;

std::vector<BYTE> cooked(const char* path) {
    std::ifstream file(path, std::ios::binary);
    if (!file) throw std::runtime_error("missing retail shader");
    return {std::istreambuf_iterator<char>(file), std::istreambuf_iterator<char>()};
}

template <size_t N> ComPtr<ID3D12Resource> gpu_input(ID3D12Device* device, const std::array<UINT, N>& data) {
    auto input = buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(data), D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(input.Get(), data.data(), sizeof(data));
    return input;
}

void srv(const Context& ctx, UINT index, ID3D12Resource* resource, UINT bytes, UINT stride) {
    D3D12_SHADER_RESOURCE_VIEW_DESC view{};
    view.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
    view.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
    view.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
    view.Buffer.NumElements = bytes / (stride ? stride : 4);
    view.Buffer.StructureByteStride = stride;
    view.Buffer.Flags = stride ? D3D12_BUFFER_SRV_FLAG_NONE : D3D12_BUFFER_SRV_FLAG_RAW;
    ctx.device->CreateShaderResourceView(resource, &view, ctx.cpu(index));
}

void pack(std::array<UINT, 256>& page, UINT bit, UINT value, UINT bits) {
    for (UINT i = 0; i < bits; ++i)
        page[(bit + i) / 32] |= ((value >> i) & 1u) << ((bit + i) % 32);
}

std::array<UINT, 256> geometry(UINT pass) {
    constexpr UINT vertexOffset = 128, triangleOffset = 512, baseBits = 8;
    std::array<UINT, 256> page{};
    const bool late = pass == 1;
    const UINT verts = pass == 2 ? 0 : late ? 195 : 3;
    const UINT triangles = pass == 2 ? 0 : late ? 97 : 1;
    page[0] = pass >= 5 ? 2 : 1; // second cluster in this page is empty
    page[4] = verts | (vertexOffset << 9);
    page[5] = triangles | (triangleOffset << 8);
    const UINT secondColumn = 4 + 4 * page[0];
    page[secondColumn] = page[secondColumn + 1] = UINT(-4); // quantization origins
    page[secondColumn + 3] = baseBits | ((pass == 4 ? 11u : 8u) << 4) | (4 << 9) | (4 << 14);
    // An enclosing triangle with quantized positions (-4,-4), (4,-4), (0,4).
    // Crossing a 64-lane boundary must preserve global vertex and primitive indices.
    const UINT start = late ? 192 : 0;
    pack(page, vertexOffset * 8 + (start + 0) * 8, 0x00, 8);
    pack(page, vertexOffset * 8 + (start + 1) * 8, 0x08, 8);
    pack(page, vertexOffset * 8 + (start + 2) * 8, 0x84, 8);
    for (UINT p = 0; p < triangles; ++p) {
        const bool visible = pass != 3 && p + 1 == triangles;
        UINT tri = (visible ? start : 255u) | (1u << baseBits) | (2u << (baseBits + 5));
        pack(page, triangleOffset * 8 + p * (baseBits + 10), tri, baseBits + 10);
    }
    return page;
}

std::string pipeline_survey(const Context& ctx) {
    D3D12_DESCRIPTOR_RANGE ranges[]{{D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 7, 0, 0, 1}};
    D3D12_ROOT_PARAMETER param{};
    param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    param.DescriptorTable = {2, ranges};
    auto root = ctx.root({1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE});
    std::ifstream entries("pipelines.txt");
    std::ofstream results("pipeline-results.txt");
    if (!entries || !results) throw std::runtime_error("cannot open mesh pipeline manifest or results");
    UINT entry, created = 0, attempted = 0;
    while (entries >> entry) {
        const auto source = cooked((std::to_string(entry) + ".dxbc").c_str());
        auto s = stream(root.Get(), {source.data(), source.size()}, {});
        s.topology.payload = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT;
        ComPtr<ID3D12PipelineState> pso;
        const HRESULT hr = ctx.create_pipeline(s, pso);
        results << std::dec << entry << " " << std::hex << std::setw(8) << std::setfill('0')
                << static_cast<UINT>(hr) << std::endl;
        created += hr == S_OK && pso;
        ++attempted;
    }
    if (!entries.eof() || !attempted || attempted != created || !results)
        throw std::runtime_error("could not create all " + std::to_string(attempted) + " mesh pipelines");
    char text[120];
    std::snprintf(text, sizeof(text), "nanite-mesh-pipelines PASS pci=%04x:%04x created=%u", ctx.adapter.VendorId,
                  ctx.adapter.DeviceId, created);
    return text;
}

std::string run(const wchar_t* adapter, bool cacheCheck, bool alternate, bool survey) {
    const auto temp = std::filesystem::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("set shader cache location");
    Context ctx(adapter, 6 * cases);
    if (survey) return pipeline_survey(ctx);
    const auto format = alternate ? DXGI_FORMAT_R32_UINT : DXGI_FORMAT_R32G32_UINT;
    const UINT texelBytes = alternate ? 4u : 8u;
    auto* device = ctx.device.Get();
    auto* list = ctx.list.Get();
    D3D12_DESCRIPTOR_RANGE ranges[]{{D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 4, 0, 0, 1},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 5}};
    D3D12_ROOT_PARAMETER param{};
    param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    param.DescriptorTable = {3, ranges};
    D3D12_ROOT_SIGNATURE_DESC layout{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    auto root = ctx.root(layout);
    const auto ms = cooked(alternate ? "2886.dxbc" : "3194.dxbc"),
               ps = cooked(alternate ? "3130.dxbc" : "2853.dxbc");
    auto s = stream(root.Get(), {ms.data(), ms.size()}, {ps.data(), ps.size()});
    s.topology.payload = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT;
    if (cacheCheck) return cache_controls(ctx, s, cooked("portable-mesh.cso"), cooked("as-envelope.dxbc"));
    ComPtr<ID3D12PipelineState> pipeline;
    check(ctx.create_pipeline(s, pipeline), "retail Xbox mesh pipeline");

    std::array<UINT, 64> constants{};
    constants[0x50 / 4] = 1; // instance data stride
    constants[0x60 / 4] = 1; // one entry in the visible cluster list
    std::array<UINT, 12> instance{};
    instance[4] = 0x80008000;
    instance[5] = 0x4000;
    instance[6] = 0x80018001;
    instance[7] = 0x008e8001;
    const std::array<UINT, 4> visibleCluster{0, 0, 0, 1u << 23}; // cluster zero, then cluster one
    std::array<UINT, 200> view{};
    // Identity XY projection, constant depth .75 and W=1.
    view[0xc0 / 4] = view[0xc0 / 4 + 5] = view[0xc0 / 4 + 15] = 0x3f800000;
    view[0xc0 / 4 + 14] = 0x3f400000;
    view[0x290 / 4] = 0x41980000;     // 19.0, shadow view width
    view[0x290 / 4 + 1] = 0x41500000; // 13.0, shadow view height
    auto b0 = gpu_input(device, constants), t0 = gpu_input(device, instance);
    constants[0x60 / 4] = 2;
    auto twoEntries = gpu_input(device, constants);
    auto t2 = gpu_input(device, visibleCluster), t3 = gpu_input(device, view);

    Data seed{};
    for (UINT y = 0; y < height; ++y)
        for (UINT x = 0; x < width; ++x) {
            const UINT depth = (x + y) % 3 == 0 ? 0x40112233u : 0x31122334u;
            const UINT64 value = alternate ? depth : (UINT64(depth) << 32) | (0x3010 + y * width + x);
            std::memcpy(seed.data() + y * pitch + x * texelBytes, &value, texelBytes);
        }
    auto source = buffer(device, D3D12_HEAP_TYPE_UPLOAD, dataSize, D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(source.Get(), seed.data(), sizeof(seed));
    auto result = buffer(device, D3D12_HEAP_TYPE_READBACK, cases * readbackStride, D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_HEAP_PROPERTIES props{};
    props.Type = D3D12_HEAP_TYPE_DEFAULT;
    props.CreationNodeMask = props.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC td{};
    td.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    td.Width = width;
    td.Height = height;
    td.DepthOrArraySize = td.MipLevels = td.SampleDesc.Count = 1;
    td.Format = format;
    td.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> target;
    check(device->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &td, D3D12_RESOURCE_STATE_COPY_DEST,
                                          nullptr, IID_PPV_ARGS(&target)),
          "Nanite visibility texture");
    std::array<ComPtr<ID3D12Resource>, cases> pages;
    auto copy = method<Copy>(list, 83);
    D3D12_TEXTURE_COPY_LOCATION texture{}, linear{};
    texture.pResource = target.Get();
    texture.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    linear.PlacedFootprint.Footprint = {format, width, height, 1, pitch};
    ctx.bind(root.Get(), width, height);
    list->RSSetScissorRects(1, &region);
    list->SetPipelineState(pipeline.Get());
    for (UINT pass = 0; pass < cases; ++pass) {
        const UINT first = 6 * pass;
        auto page = geometry(pass);
        pages[pass] = gpu_input(device, page);
        D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{(pass >= 5 ? twoEntries : b0)->GetGPUVirtualAddress(),
                                            sizeof(constants)};
        device->CreateConstantBufferView(&cbv, ctx.cpu(first));
        srv(ctx, first + 1, t0.Get(), sizeof(instance), 16);
        srv(ctx, first + 2, pages[pass].Get(), sizeof(page), 0);
        srv(ctx, first + 3, t2.Get(), sizeof(visibleCluster), 0);
        srv(ctx, first + 4, t3.Get(), sizeof(view), sizeof(view));
        device->CreateUnorderedAccessView(target.Get(), nullptr, nullptr, ctx.cpu(first + 5));
        if (pass) transition(list, target.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        linear.pResource = source.Get();
        linear.PlacedFootprint.Offset = 0;
        copy(list, &texture, 0, 0, 0, &linear, nullptr, 0);
        transition(list, target.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        ctx.table(0, first);
        ctx.dispatch(pass == 6 ? 2 : 1, 1, 1);
        transition(list, target.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        linear.pResource = result.Get();
        linear.PlacedFootprint.Offset = pass * readbackStride;
        copy(list, &linear, 0, 0, 0, &texture, nullptr, 0);
    }
    ctx.submit();
    D3D12_RANGE read{0, cases * readbackStride}, empty{};
    void* ptr = nullptr;
    check(result->Map(0, &read, &ptr), "map visibility results");
    UINT errors = 0, changed[cases]{};
    for (UINT p = 0; p < cases; ++p) {
        const auto bytes = static_cast<const BYTE*>(ptr) + p * readbackStride;
        const UINT64 fragment = alternate ? 0x3f400000u : 0x3f40000000000000ull | 128u | (p == 1 ? 96u : 0u);
        for (LONG y = 0; y < LONG(height); ++y)
            for (LONG x = 0; x < LONG(width); ++x) {
                UINT index = y * pitch + x * texelBytes;
                UINT64 before = 0, after = 0;
                std::memcpy(&before, seed.data() + index, texelBytes);
                std::memcpy(&after, bytes + index, texelBytes);
                const bool inside = x >= region.left && x < region.right && y >= region.top && y < region.bottom;
                // Quantized (-4,-4),(4,-4),(0,4) scaled by 1/8 in pass 4.
                // Integer inequalities evaluate the triangle at D3D pixel centers.
                const LONG nx = 2 * x + 1 - width, ny = height - 2 * y - 1;
                const bool insideTriangle = 2 * ny > -LONG(height) && 2 * ny < LONG(height) &&
                                            4 * std::abs(nx) * LONG(height) < (LONG(height) - 2 * ny) * LONG(width);
                const bool draws = p < 2 || (p == 4 && insideTriangle) || p == 6;
                UINT64 expected = (draws && inside) ? (std::max)(fragment, before) : before;
                changed[p] += after != before;
                if (after != expected && errors++ < 4)
                    std::fprintf(stderr, "pass %u (%ld,%ld) got %016llx expected %016llx\n", p, x, y, after,
                                 expected);
            }
    }
    result->Unmap(0, &empty);
    if (errors) throw std::runtime_error(std::to_string(errors) + " visibility values differ");
    char text[200];
    std::snprintf(
        text, sizeof(text),
        "nanite-mesh PASS pci=%04x:%04x ms=%u ps=%u format=%s "
        "texels=1729/1729 primitive0=%u primitive96=%u empty=%u invalid=%u clipped=%u group0=%u groups01=%u",
        ctx.adapter.VendorId, ctx.adapter.DeviceId, alternate ? 2886u : 3194u, alternate ? 3130u : 2853u,
        alternate ? "R32_UINT" : "R32G32_UINT", changed[0], changed[1], changed[2], changed[3], changed[4],
        changed[5], changed[6]);
    return text;
}
}
int wmain(int argc, wchar_t** argv) {
    bool cacheCheck = argc > 1 && std::wcscmp(argv[1], L"--check-cache") == 0;
    const bool alternate = argc > 1 && std::wcscmp(argv[1], L"--alternate") == 0;
    const bool survey = argc > 1 && std::wcscmp(argv[1], L"--pipelines") == 0;
    const int flags = cacheCheck || alternate || survey;
    return tuxbox_test::frame_main(argc - flags, argv + flags, "nanite-mesh", false,
                                   [cacheCheck, alternate, survey](const wchar_t* adapter, bool) {
                                       return run(adapter, cacheCheck, alternate, survey);
                                   });
}
