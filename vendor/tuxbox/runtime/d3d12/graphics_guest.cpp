#include "frame_abi.h"
#include "frame_cli.h"
#include "frame_stream.h"
#include "compute_fixture.h"
#include "triangle_vs.h"
#include "triangle_input_vs.h"
#include "triangle_ps.h"
#include "rectangle_vs.h"
#include "rectangle_ps.h"
#include <cstring>

namespace {
using namespace frame_abi;
using CreatePipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_GRAPHICS_PIPELINE_STATE_DESC*,
                                         UINT, const void*, REFIID, void**);
using SetRoot = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using SetTargets = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, const D3D12_CPU_DESCRIPTOR_HANDLE*,
                                  BOOL, const D3D12_CPU_DESCRIPTOR_HANDLE*);
constexpr LONG extent = 16;
constexpr UINT pitch = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT;
constexpr DXGI_FORMAT format = DXGI_FORMAT_R8G8B8A8_UNORM;
constexpr D3D12_RECT scissor{3, 4, 9, 10};
constexpr std::array<UINT, 18> profile_20004{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1};

ComPtr<ID3D12PipelineState> create_pipeline(ID3D12Device* device, ID3D12RootSignature* root, bool rectangle) {
    D3D12_GRAPHICS_PIPELINE_STATE_DESC desc{};
    static_assert(sizeof(desc) == 0x290);
    desc.pRootSignature = root;
    desc.VS = {triangle_vs, sizeof(triangle_vs)};
    desc.PS = {triangle_ps, sizeof(triangle_ps)};
    desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
    desc.SampleMask = UINT_MAX;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.RasterizerState.DepthClipEnable = TRUE;
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    if (rectangle) {
        desc.VS = {rectangle_vs, sizeof(rectangle_vs)};
        desc.PS = {rectangle_ps, sizeof(rectangle_ps)};
        desc.PrimitiveTopologyType = static_cast<D3D12_PRIMITIVE_TOPOLOGY_TYPE>(0x13);
    }
    desc.NumRenderTargets = 1;
    desc.RTVFormats[0] = format;
    desc.SampleDesc.Count = 1;
    const auto create = method<CreatePipeline>(device, 60);
    void* rejected = device;
    UINT extension = 0;
    if (create(device, &desc, 1, &extension, __uuidof(ID3D12PipelineState), &rejected) != E_NOTIMPL || rejected)
        throw std::runtime_error("unhandled graphics extension accepted");
    ComPtr<ID3D12PipelineState> result;
    check(create(device, &desc, 0, nullptr, IID_PPV_ARGS(&result)), "Xbox graphics pipeline slot 60");
    return result;
}

ComPtr<ID3D12PipelineState> create_stream_pipeline(ID3D12Device* device, ID3D12RootSignature* root,
                                                   ID3DBlob* serialized, const wchar_t* requested, bool rectangle) {
    ComPtr<ID3D12Device2> device2;
    check(device->QueryInterface(IID_PPV_ARGS(&device2)), "query Xbox ID3D12Device2");
    ComPtr<IUnknown> base_identity, stream_identity;
    check(device->QueryInterface(IID_PPV_ARGS(&base_identity)), "base identity");
    check(device2.As(&stream_identity), "Device2 identity");
    if (base_identity != stream_identity) throw std::runtime_error("Device2 identity differs");

    RasterPipelineStream parts;
    parts.root.payload = root;
    const D3D12_INPUT_ELEMENT_DESC position{"POSITION", 0, DXGI_FORMAT_R32G32_FLOAT, 2, 0,
                                            D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0};
    parts.layout.payload = {&position, 1};
    parts.vs.payload = {triangle_input_vs, sizeof(triangle_input_vs)};
    parts.ps.payload = {triangle_ps, sizeof(triangle_ps)};
    auto& blend = parts.blend.payload.RenderTarget[0];
    blend.BlendEnable = TRUE;
    blend.SrcBlend = D3D12_BLEND_BLEND_FACTOR;
    blend.DestBlend = D3D12_BLEND_ZERO;
    blend.BlendOp = D3D12_BLEND_OP_ADD;
    blend.SrcBlendAlpha = D3D12_BLEND_ONE;
    blend.DestBlendAlpha = D3D12_BLEND_ZERO;
    blend.BlendOpAlpha = D3D12_BLEND_OP_ADD;
    blend.RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_RED;
    parts.raster.payload.FillMode = D3D12_FILL_MODE_SOLID;
    parts.raster.payload.CullMode = D3D12_CULL_MODE_NONE;
    parts.raster.payload.DepthClipEnable = TRUE;
    parts.formats.payload.NumRenderTargets = 1;
    parts.formats.payload.RTFormats[0] = format;
    if (rectangle) {
        parts.layout.payload = {};
        parts.vs.payload = {rectangle_vs, sizeof(rectangle_vs)};
        parts.ps.payload = {rectangle_ps, sizeof(rectangle_ps)};
        parts.topology.payload = 0x13;
        blend.BlendEnable = FALSE;
        blend.RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
    }
    parts.dsv.payload = DXGI_FORMAT_UNKNOWN;
    const std::array<BYTE, 16> console_hint{0x58, 0x53};
    parts.cached.payload = {console_hint.data(), console_hint.size()};
    D3D12_PIPELINE_STATE_STREAM_DESC stream{sizeof(parts), &parts};
    using CreateStream = HRESULT (WINAPI*)(ID3D12Device2*, const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**);
    const auto create = method<CreateStream>(device2.Get(), 86);
    auto reject = [&](const char* message) {
        void* output = device;
        if (SUCCEEDED(create(device2.Get(), &stream, __uuidof(ID3D12PipelineState), &output)) || output)
            throw std::runtime_error(message);
    };
    if (create(device2.Get(), &stream, __uuidof(ID3D12PipelineState), nullptr) != E_POINTER)
        throw std::runtime_error("missing stream output was accepted");
    --stream.SizeInBytes;
    reject("truncated stream was accepted");
    stream.SizeInBytes = sizeof(parts);
    parts.stencil[16] = 0x7f; // Xbox read mask must match the depth/stencil state.
    reject("inconsistent stencil masks were accepted");
    parts.stencil[16] = 0;
    DXGI_ADAPTER_DESC1 selected{};
    auto other = create_device(requested, selected, profile_20004.data());
    ComPtr<ID3D12RootSignature> other_root;
    check(other->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                     IID_PPV_ARGS(&other_root)), "other device root");
    parts.root.payload = other_root.Get();
    reject("foreign root was accepted");
    parts.root.payload = root;

    ComPtr<ID3D12PipelineState> result;
    check(create(device2.Get(), &stream, IID_PPV_ARGS(&result)), "Xbox graphics pipeline slot 86");
    ComPtr<ID3D12Device2> owner;
    check(result->GetDevice(IID_PPV_ARGS(&owner)), "stream pipeline owner");
    ComPtr<IUnknown> owner_identity;
    check(owner.As(&owner_identity), "stream pipeline owner identity");
    if (base_identity != owner_identity) throw std::runtime_error("stream pipeline owner differs");
    return result;
}

ComPtr<ID3D12Resource> draw_indexed(ID3D12Device* device, ID3D12GraphicsCommandList* list) {
    // Each view starts inside the upload, and both draws use a nonzero first
    // index and base vertex. Omitting any binding or offset changes the pixels.
    struct Geometry {
        float positions[7][2];
        UINT16 indices16[5];
        UINT indices32[4];
    };
    constexpr Geometry data{
        {{10, 10}, {10, 10}, {-1, -1}, {-1, 3}, {10, 10}, {3, -1}, {10, 10}},
        {0, 0, 2, 3, 5}, {0, 2, 3, 5}};
    auto input = compute_fixture::buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(data), D3D12_RESOURCE_STATE_GENERIC_READ);
    compute_fixture::upload(input.Get(), &data, sizeof(data));
    const auto address = input->GetGPUVirtualAddress();
    const UINT stride = sizeof(data.positions[0]);
    const D3D12_VERTEX_BUFFER_VIEW vertices{address + stride, sizeof(data.positions) - stride, stride};
    using SetVertices = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const D3D12_VERTEX_BUFFER_VIEW*);
    using SetIndex = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_INDEX_BUFFER_VIEW*);
    const auto set_vertices = method<SetVertices>(list, 48);
    const auto set_index = method<SetIndex>(list, 47);
    set_vertices(list, 2, 1, nullptr);
    set_vertices(list, 2, 1, &vertices);
    set_index(list, nullptr);
    const D3D12_INDEX_BUFFER_VIEW indices[]{
        {address + offsetof(Geometry, indices16) + sizeof(UINT16), 4 * sizeof(UINT16), DXGI_FORMAT_R16_UINT},
        {address + offsetof(Geometry, indices32), sizeof(data.indices32), DXGI_FORMAT_R32_UINT}};
    constexpr LONG middle = (scissor.left + scissor.right) / 2;
    const D3D12_RECT halves[]{{scissor.left, scissor.top, middle, scissor.bottom},
                              {middle, scissor.top, scissor.right, scissor.bottom}};
    for (UINT i = 0; i < 2; ++i) {
        const float blend[]{i ? 1.0f : 128.0f / 255.0f, 1, 1, 1};
        list->OMSetBlendFactor(blend);
        list->OMSetStencilRef(i);
        set_index(list, &indices[i]);
        list->RSSetScissorRects(1, &halves[i]);
        list->DrawIndexedInstanced(3, 1, 1, -1, 0);
    }
    return input;
}

std::string run(const wchar_t* requested, bool stream, bool rectangle = false) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected, stream ? profile_20004.data() : profile.data());
    D3D12_ROOT_SIGNATURE_DESC signature{};
    if (stream) signature.Flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT;
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&signature, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "graphics root");
    auto pipeline = stream ? create_stream_pipeline(device.Get(), root.Get(), serialized.Get(), requested, rectangle)
                           : create_pipeline(device.Get(), root.Get(), rectangle);

    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC target_desc{};
    target_desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    target_desc.Width = target_desc.Height = extent;
    target_desc.DepthOrArraySize = 1;
    target_desc.MipLevels = 1;
    target_desc.SampleDesc.Count = 1;
    target_desc.Format = format;
    target_desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> target;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &target_desc, D3D12_RESOURCE_STATE_RENDER_TARGET,
                                          nullptr, IID_PPV_ARGS(&target)), "triangle render target");
    auto readback = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_READBACK, extent * pitch,
                                           D3D12_RESOURCE_STATE_COPY_DEST);
    D3D12_DESCRIPTOR_HEAP_DESC views_desc{D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    ComPtr<ID3D12DescriptorHeap> views;
    check(device->CreateDescriptorHeap(&views_desc, IID_PPV_ARGS(&views)), "RTV heap");
    D3D12_CPU_DESCRIPTOR_HANDLE rtv{method<CpuHandle>(views.Get(), 9)(views.Get())};
    device->CreateRenderTargetView(target.Get(), nullptr, rtv);

    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    check(device->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "direct queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "direct allocator");
    const UINT parameters[]{D3D12_COMMAND_LIST_TYPE_DIRECT, 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device.Get(), 73)(device.Get(), parameters, allocator.Get(), nullptr,
                                              IID_PPV_ARGS(&list)), "direct list");
    const float blue[]{0, 0, 1, rectangle ? 0.0f : 1.0f};
    method<Clear>(list.Get(), 52)(list.Get(), rtv, blue, 0, nullptr);
    if (stream) {
        // Frosty's inline pipeline bind copies these eight DWORDs before slot 25.
        auto* packets = reinterpret_cast<BYTE*>(list.Get()) + 0x540;
        const auto* pipeline_packets = reinterpret_cast<const BYTE*>(pipeline.Get()) + 0x10;
        std::memcpy(packets, pipeline_packets, 32);
    }
    list->SetPipelineState(pipeline.Get());
    method<SetRoot>(list.Get(), 34)(list.Get(), root.Get());
    method<SetTargets>(list.Get(), 50)(list.Get(), 1, &rtv, FALSE, nullptr);
    D3D12_VIEWPORT viewport{0, 0, extent, extent, 0, 1};
    list->RSSetViewports(1, &viewport);
    list->RSSetScissorRects(1, &scissor);
    // Reproduce Frosty's unsigned inline packet-capacity comparison at RVA 0x57f02fa.
    const auto* object = reinterpret_cast<const BYTE*>(list.Get());
    UINT_PTR cursor, end;
    std::memcpy(&cursor, object + 0x18, sizeof(cursor));
    std::memcpy(&end, object + 0x30, sizeof(end));
    if (cursor < end) throw std::runtime_error("list advertises writable Xbox packet memory");
    ComPtr<ID3D12Resource> geometry, arguments;
    ComPtr<ID3D12CommandSignature> draw_signature;
    if (rectangle) {
        D3D12_RECT full{0, 0, extent, extent};
        list->RSSetScissorRects(1, &full);
        list->IASetPrimitiveTopology(static_cast<D3D_PRIMITIVE_TOPOLOGY>(0x11));
        if (stream) {
            const UINT16 indices[] = {77, 0, 1, 2};
            geometry = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD,
                                                sizeof(indices), D3D12_RESOURCE_STATE_GENERIC_READ);
            compute_fixture::upload(geometry.Get(), indices, sizeof(indices));
            D3D12_INDEX_BUFFER_VIEW ib{geometry->GetGPUVirtualAddress(), sizeof(indices), DXGI_FORMAT_R16_UINT};
            using SetIndex = void (WINAPI*)(ID3D12GraphicsCommandList*, const D3D12_INDEX_BUFFER_VIEW*);
            method<SetIndex>(list.Get(), 47)(list.Get(), &ib);
            const D3D12_DRAW_INDEXED_ARGUMENTS draws{3, 2, 1, 0, 0};
            D3D12_INDIRECT_ARGUMENT_DESC operation{};
            operation.Type = D3D12_INDIRECT_ARGUMENT_TYPE_DRAW_INDEXED;
            D3D12_COMMAND_SIGNATURE_DESC command_signature{sizeof(draws), 1, &operation, 1};
            check(device->CreateCommandSignature(&command_signature, nullptr, IID_PPV_ARGS(&draw_signature)),
                  "Xbox rectangle indirect signature");
            arguments = compute_fixture::buffer(device.Get(), D3D12_HEAP_TYPE_UPLOAD,
                                                 sizeof(draws), D3D12_RESOURCE_STATE_GENERIC_READ);
            compute_fixture::upload(arguments.Get(), &draws, sizeof(draws));
            list->ExecuteIndirect(draw_signature.Get(), 1, arguments.Get(), 0, nullptr, 0);
        } else {
            list->DrawInstanced(6, 1, 0, 0);
        }
    } else {
        list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
        if (stream) geometry = draw_indexed(device.Get(), list.Get());
        else list->DrawInstanced(3, 1, 0, 0);
    }
    compute_fixture::transition(list.Get(), target.Get(), D3D12_RESOURCE_STATE_RENDER_TARGET, D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION from{}, to{};
    from.pResource = target.Get();
    from.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    to.pResource = readback.Get();
    to.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    to.PlacedFootprint.Footprint = {format, extent, extent, 1, pitch};
    method<Copy>(list.Get(), 83)(list.Get(), &to, 0, 0, 0, &from, nullptr, 0);
    check(list->Close(), "close draw");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "draw fence");
    ID3D12CommandList* batch[]{list.Get()};
    queue->ExecuteCommandLists(1, batch);
    check(queue->Signal(fence.Get(), 1), "signal draw fence");
    compute_fixture::wait_for_fence(fence.Get(), 1);
    const D3D12_RANGE read{0, extent * pitch}, empty{};
    void* bytes = nullptr;
    check(readback->Map(0, &read, &bytes), "map triangle");
    char mismatch[100]{};
    for (LONG y = 0; y < extent; ++y) {
        auto row = reinterpret_cast<const UINT*>(static_cast<const BYTE*>(bytes) + pitch * y);
        for (LONG x = 0; x < extent; ++x) {
            const bool covered = x >= scissor.left && x < scissor.right && y >= scissor.top && y < scissor.bottom;
            const UINT drawn = stream ? (x < (scissor.left + scissor.right) / 2 ? 0xffff0080u : 0xffff00ffu)
                                      : 0xff0000ffu;
            UINT expected = covered ? drawn : 0xffff0000u;
            if (rectangle) {
                const bool right = x >= 9 && x < 15 && y >= 9 && y < 15;
                const bool left = x >= 1 && x < 7 && y >= 2 && y < 8;
                expected = left || right ? 0xff000000u | (right ? 0x00ff0000u : 0)
                    | (x >= (right ? 12 : 4) ? 0x000000ffu : 0)
                    | (y >= (right ? 12 : 5) ? 0x0000ff00u : 0) : 0x00ff0000u;
            }
            if (!mismatch[0] && row[x] != expected)
                std::snprintf(mismatch, sizeof(mismatch), "triangle pixel (%ld,%ld) = %08x, expected %08x",
                              x, y, row[x], expected);
        }
    }
    readback->Unmap(0, &empty);
    if (mismatch[0]) throw std::runtime_error(mismatch);
    char receipt[128];
    if (rectangle)
        std::snprintf(receipt, sizeof(receipt), "d3d12-rectangle PASS pci=%04x:%04x slot=%d rectangles=2 texels=256",
                      selected.VendorId, selected.DeviceId, stream ? 86 : 60);
    else if (stream)
        std::snprintf(receipt, sizeof(receipt), "d3d12-stream PASS pci=%04x:%04x slot=86 bytes=712 indices=16,32 half-red-blue=18 magenta=18 blue=220 rejected=4",
                      selected.VendorId, selected.DeviceId);
    else
        std::snprintf(receipt, sizeof(receipt), "d3d12-draw PASS pci=%04x:%04x red=36 blue=220",
                      selected.VendorId, selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    if (argc > 1 && (!std::wcscmp(argv[1], L"--rect") || !std::wcscmp(argv[1], L"--rect-stream"))) {
        const bool stream = !std::wcscmp(argv[1], L"--rect-stream");
        return tuxbox_test::frame_main(argc - 1, argv + 1, "d3d12-rectangle", false,
            [stream](const wchar_t* adapter, bool) { return run(adapter, stream, true); });
    }
    if (argc > 1 && std::wcscmp(argv[1], L"--stream") == 0)
        return tuxbox_test::frame_main(argc - 1, argv + 1, "d3d12-stream", false,
            [](const wchar_t* adapter, bool) { return run(adapter, true); });
    return tuxbox_test::frame_main(argc, argv, "d3d12-draw", false, [](const wchar_t* adapter, bool) { return run(adapter, false); });
}
