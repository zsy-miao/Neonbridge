// Execute an unmodified Scarlett pixel shader through the Xbox graphics pipeline stream.
#include "frame_cli.h"
#include "mesh_fixture.h"
#include <filesystem>
#include <fstream>
#include <vector>

namespace {
using namespace mesh_fixture;
constexpr UINT width = 16, height = 8;
constexpr DXGI_FORMAT depth_format = DXGI_FORMAT_D32_FLOAT_S8X24_UINT;
using Words = std::vector<UINT>;
using SetTargets = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_CPU_DESCRIPTOR_HANDLE*, BOOL,
                                 const D3D12_CPU_DESCRIPTOR_HANDLE*);
using ClearDepth = void(WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CLEAR_FLAGS,
                                  float, UINT8, UINT, const D3D12_RECT*);

std::vector<char> load(const char* path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) throw std::runtime_error(std::string("cannot read ") + path);
    return {std::istreambuf_iterator<char>(in), {}};
}

ComPtr<ID3D12Resource> texture(const Context& gpu, DXGI_FORMAT fmt, D3D12_RESOURCE_FLAGS flags,
                              D3D12_RESOURCE_STATES state) {
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = fmt;
    desc.Flags = flags;
    ComPtr<ID3D12Resource> result;
    check(gpu.device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr,
                                              IID_PPV_ARGS(&result)), "create texture");
    return result;
}

ComPtr<ID3D12DescriptorHeap> heap(const Context& gpu, D3D12_DESCRIPTOR_HEAP_TYPE type, UINT count) {
    D3D12_DESCRIPTOR_HEAP_DESC desc{type, count, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 1};
    ComPtr<ID3D12DescriptorHeap> result;
    check(gpu.device->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&result)), "create descriptor heap");
    return result;
}

D3D12_TEXTURE_COPY_LOCATION subresource(ID3D12Resource* resource, UINT index) {
    D3D12_TEXTURE_COPY_LOCATION location{};
    location.pResource = resource;
    location.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    location.SubresourceIndex = index;
    return location;
}

D3D12_TEXTURE_COPY_LOCATION footprint(ID3D12Resource* resource, const D3D12_PLACED_SUBRESOURCE_FOOTPRINT& layout) {
    D3D12_TEXTURE_COPY_LOCATION location{};
    location.pResource = resource;
    location.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    location.PlacedFootprint = layout;
    return location;
}

std::string run(const wchar_t* adapter, bool) {
    const auto temp = std::filesystem::absolute("temp");
    if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
        throw std::runtime_error("cannot set shader cache directory");
    const bool velocity = std::filesystem::exists("velocity");
    const UINT srvs = velocity ? 8 : 6;
    Context gpu(adapter, srvs + 2);
    auto device = gpu.device.Get();
    auto list = gpu.list.Get();
    D3D12_DESCRIPTOR_RANGE ranges[]{{D3D12_DESCRIPTOR_RANGE_TYPE_SRV, srvs, 0, 0, 0},
                                    {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 2, 0, 0, 0}};
    D3D12_ROOT_PARAMETER params[2]{};
    for (UINT i = 0; i < 2; ++i) {
        params[i].ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        params[i].DescriptorTable = {1, &ranges[i]};
        params[i].ShaderVisibility = D3D12_SHADER_VISIBILITY_PIXEL;
    }
    auto root = gpu.root({2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE});
    const auto vs = load("triangle.cso"), ps = load("pixel.dxbc");
    RasterPipelineStream parts;
    parts.root.payload = root.Get();
    parts.vs.payload = {vs.data(), vs.size()};
    parts.ps.payload = {ps.data(), ps.size()};
    auto& raster = parts.raster.payload;
    raster.FillMode = D3D12_FILL_MODE_SOLID;
    raster.CullMode = D3D12_CULL_MODE_NONE;
    raster.DepthClipEnable = TRUE;
    parts.dsv.payload = depth_format;
    parts.formats.payload = {{DXGI_FORMAT_R32_UINT, DXGI_FORMAT_R16G16B16A16_FLOAT}, 2};
    auto& depth = parts.depth.payload;
    depth.DepthEnable = depth.StencilEnable = TRUE;
    depth.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ALL;
    depth.DepthFunc = D3D12_COMPARISON_FUNC_GREATER_EQUAL;
    depth.StencilReadMask = depth.StencilWriteMask = 255;
    depth.FrontFace = {D3D12_STENCIL_OP_KEEP, D3D12_STENCIL_OP_KEEP, D3D12_STENCIL_OP_REPLACE,
                       D3D12_COMPARISON_FUNC_ALWAYS};
    depth.BackFace = depth.FrontFace;
    parts.stencil[16] = parts.stencil[17] = 255;
    parts.stencil[20] = parts.stencil[21] = 1;
    D3D12_PIPELINE_STATE_STREAM_DESC stream{sizeof(parts), &parts};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<CreateStream>(gpu.device2.Get(), 86)(gpu.device2.Get(), &stream, IID_PPV_ARGS(&pipeline)),
          "Xbox stencil pipeline");

    std::vector<ComPtr<ID3D12Resource>> inputs;
    auto upload_words = [&](const Words& words) {
        auto resource = buffer(device, D3D12_HEAP_TYPE_UPLOAD, words.size() * 4, D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(resource.Get(), words.data(), words.size() * 4);
        inputs.push_back(resource);
        return resource;
    };
    for (UINT i = 0; i < srvs - 1; ++i) {
        Words words(16384);
        // Two visible clusters select primitives 0 and 1. Primitive 0 carries decal bit 3.
        if (i == 0) words[38 * 4] = 8;
        if (i == 1) words[velocity ? 4 : 1] = 1;
        if (i == (velocity ? 4u : 3u)) words[2] = 1u << 15;
        const auto input = upload_words(words);
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
        srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        UINT stride = velocity ? (i <= 2 || i == 5 ? 16 : 0) : (i == 0 ? 16 : i == 1 ? 4 : 0);
        srv.Buffer.StructureByteStride = stride;
        srv.Buffer.NumElements = UINT(words.size() * 4) / (stride ? stride : 4);
        srv.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
        if (!stride) srv.Buffer.Flags = D3D12_BUFFER_SRV_FLAG_RAW;
        device->CreateShaderResourceView(input.Get(), &srv, gpu.cpu(i));
    }
    for (UINT i = 0; i < 2; ++i) {
        Words words(2048);
        if (i == 0) {
            // The exported reference uses only the low byte even when high bits are set.
            words[12] = 0x12340021;
            words[13] = 0xffff0042;
        } else {
            words[795] = 0x3f800000;
        }
        const auto input = upload_words(words);
        D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{input->GetGPUVirtualAddress(), UINT(words.size() * 4)};
        device->CreateConstantBufferView(&cbv, gpu.cpu(srvs + i));
    }
    auto visibility = texture(gpu, DXGI_FORMAT_R32G32_UINT, D3D12_RESOURCE_FLAG_NONE, D3D12_RESOURCE_STATE_COPY_DEST);
    Words pixels(64 * height);
    for (UINT y = 0; y < height; ++y) for (UINT x = 0; x < width; ++x) {
        pixels[y * 64 + x * 2] = x < 4 ? 0 : x < 10 ? 128 : 256;
        pixels[y * 64 + x * 2 + 1] = y < 2 ? 0x3e000000 : 0x3f400000;
    }
    auto seed = upload_words(pixels);
    auto src = footprint(seed.Get(), {0, {DXGI_FORMAT_R32G32_UINT, width, height, 1, 256}});
    auto dst = subresource(visibility.Get(), 0);
    method<Copy>(list, 83)(list, &dst, 0, 0, 0, &src, nullptr, 0);
    transition(list, visibility.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
    device->CreateShaderResourceView(visibility.Get(), nullptr, gpu.cpu(srvs - 1));

    auto depth_texture = texture(gpu, depth_format, D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL, D3D12_RESOURCE_STATE_DEPTH_WRITE);
    auto depths = heap(gpu, D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 2);
    D3D12_CPU_DESCRIPTOR_HANDLE original{method<CpuHandle>(depths.Get(), 9)(depths.Get())};
    D3D12_CPU_DESCRIPTOR_HANDLE copy{original.ptr + device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_DSV)};
    device->CreateDepthStencilView(depth_texture.Get(), nullptr, original);
    device->CopyDescriptorsSimple(1, copy, original, D3D12_DESCRIPTOR_HEAP_TYPE_DSV);
    method<ClearDepth>(list, 51)(list, copy, D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL, .25f, 0x6c, 0, nullptr);
    auto colors = heap(gpu, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2);
    ComPtr<ID3D12Resource> targets[2];
    D3D12_CPU_DESCRIPTOR_HANDLE rtvs[2]{};
    for (UINT i = 0; i < 2; ++i) {
        targets[i] = texture(gpu, parts.formats.payload.RTFormats[i], D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                             D3D12_RESOURCE_STATE_RENDER_TARGET);
        rtvs[i].ptr = method<CpuHandle>(colors.Get(), 9)(colors.Get()) + i * device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV);
        device->CreateRenderTargetView(targets[i].Get(), nullptr, rtvs[i]);
    }
    // The guest may reuse its CPU descriptor immediately after OMSetRenderTargets.
    method<SetTargets>(list, 50)(list, 2, rtvs, TRUE, &copy);
    D3D12_DEPTH_STENCIL_VIEW_DESC null_dsv{};
    null_dsv.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D;
    null_dsv.Format = depth_format;
    device->CreateDepthStencilView(nullptr, &null_dsv, copy);
    list->SetPipelineState(pipeline.Get());
    list->OMSetStencilRef(0xde);
    gpu.bind(root.Get(), width, height);
    gpu.table(0, 0);
    gpu.table(1, srvs);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    for (LONG left : {LONG(0), LONG(width / 2)}) {
        const D3D12_RECT rect{left, 0, left + LONG(width / 2), LONG(height)};
        list->RSSetScissorRects(1, &rect);
        list->DrawInstanced(3, 1, 0, 0);
    }

    transition(list, depth_texture.Get(), D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_RESOURCE_DESC desc{};
    using GetDesc = D3D12_RESOURCE_DESC*(WINAPI*)(ID3D12Resource*, D3D12_RESOURCE_DESC*);
    method<GetDesc>(depth_texture.Get(), 10)(depth_texture.Get(), &desc);
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT planes[2]{};
    UINT64 bytes{};
    device->GetCopyableFootprints(&desc, 0, 2, 0, planes, nullptr, nullptr, &bytes);
    auto readback = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
    for (UINT i = 0; i < 2; ++i) {
        src = subresource(depth_texture.Get(), i);
        dst = footprint(readback.Get(), planes[i]);
        method<Copy>(list, 83)(list, &dst, 0, 0, 0, &src, nullptr, 0);
    }
    gpu.submit();
    void* mapped{};
    D3D12_RANGE span{0, size_t(bytes)}, empty{};
    check(readback->Map(0, &span, &mapped), "map depth and stencil planes");
    std::ofstream out("pixels.bin", std::ios::binary);
    const auto data = static_cast<const char*>(mapped);
    for (UINT y = 0; y < height; ++y) for (UINT x = 0; x < width; ++x) {
        out.write(data + planes[0].Offset + planes[0].Footprint.RowPitch * y + 4 * x, 4);
        out.put(data[planes[1].Offset + planes[1].Footprint.RowPitch * y + x]);
    }
    readback->Unmap(0, &empty);
    out.close();
    if (!out) throw std::runtime_error("cannot write depth/stencil readback");
    return "stencil-export readback complete";
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "stencil-export", false, run);
}
