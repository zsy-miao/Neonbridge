// Draw Frosty's cached Slate vertex and pixel shaders with controlled geometry and resource inputs.
#include "frame_abi.h"
#include "draw_blobs.h"
#include <cstring>
#ifdef TUXBOX_COOKED_DRAW
#include "frame_stream.h"
#include <filesystem>
#endif
using frame_abi::check;
using frame_abi::ComPtr;

constexpr LONG extent = 16;
constexpr UINT pitch = 256;
constexpr DXGI_FORMAT format = DXGI_FORMAT_R8G8B8A8_UNORM;

// The Xbox profile moves these commands while retaining their desktop arguments.
template<class... Args>
static auto command(ID3D12GraphicsCommandList* list, [[maybe_unused]] size_t slot,
                    [[maybe_unused]] void (WINAPI ID3D12GraphicsCommandList::*member)(Args...)) {
#ifdef TUXBOX_COOKED_DRAW
    using Call = void (WINAPI*)(ID3D12GraphicsCommandList*, Args...);
    return [list, call = frame_abi::method<Call>(list, slot)](Args... args) { call(list, args...); };
#else
    return [list, member](Args... args) { (list->*member)(args...); };
#endif
}

static D3D12_CPU_DESCRIPTOR_HANDLE cpu_start(ID3D12DescriptorHeap* heap) {
#ifdef TUXBOX_COOKED_DRAW
    return {frame_abi::method<frame_abi::CpuHandle>(heap, 9)(heap)};
#else
    return heap->GetCPUDescriptorHandleForHeapStart();
#endif
}

static D3D12_GPU_DESCRIPTOR_HANDLE gpu_start(ID3D12DescriptorHeap* heap) {
#ifdef TUXBOX_COOKED_DRAW
    using GpuHandle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
    return {frame_abi::method<GpuHandle>(heap, 10)(heap)};
#else
    return heap->GetGPUDescriptorHandleForHeapStart();
#endif
}

static ComPtr<ID3D12Resource> resource(ID3D12Device* dev, D3D12_HEAP_TYPE type, UINT width, UINT height,
                                       D3D12_RESOURCE_STATES state) {
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = type;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    const bool texture = type == D3D12_HEAP_TYPE_DEFAULT;
    desc.Dimension = texture ? D3D12_RESOURCE_DIMENSION_TEXTURE2D : D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = texture ? format : DXGI_FORMAT_UNKNOWN;
    desc.Layout = texture ? D3D12_TEXTURE_LAYOUT_UNKNOWN : D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = texture ? D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET : D3D12_RESOURCE_FLAG_NONE;
    ComPtr<ID3D12Resource> result;
    check(dev->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc, state, nullptr, IID_PPV_ARGS(&result)),
          "create resource");
    return result;
}

static ComPtr<ID3D12DescriptorHeap> heap(ID3D12Device* dev, D3D12_DESCRIPTOR_HEAP_TYPE type, UINT count) {
    D3D12_DESCRIPTOR_HEAP_DESC desc{type, count,
                                    type == D3D12_DESCRIPTOR_HEAP_TYPE_RTV ? D3D12_DESCRIPTOR_HEAP_FLAG_NONE
                                                                           : D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                                    0};
    ComPtr<ID3D12DescriptorHeap> result;
    check(dev->CreateDescriptorHeap(&desc, IID_PPV_ARGS(&result)), "create descriptor heap");
    return result;
}

static void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* res, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER b{};
    b.Transition = {res, 0, D3D12_RESOURCE_STATE_RENDER_TARGET, after};
    list->ResourceBarrier(1, &b);
}

static void draw(ID3D12Device* dev, ID3D12RootSignature* root, D3D12_SHADER_BYTECODE pixel, UINT entry, FILE* output) {
    const D3D12_INPUT_ELEMENT_DESC layout[] = {
        {"ATTRIBUTE", 0, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 0, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 1, DXGI_FORMAT_R32G32_FLOAT, 0, 16, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 2, DXGI_FORMAT_R32G32_FLOAT, 0, 24, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 3, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 32, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 4, DXGI_FORMAT_R32G32B32A32_FLOAT, 0, 48, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 5, DXGI_FORMAT_R32G32_UINT, 0, 64, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
    };
    D3D12_GRAPHICS_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root;
    desc.VS = {vertex_shader, sizeof(vertex_shader)};
    desc.PS = pixel;
    desc.InputLayout = {layout, _countof(layout)};
    desc.SampleMask = UINT_MAX;
    desc.SampleDesc.Count = 1;
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.RasterizerState.DepthClipEnable = TRUE;
    desc.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
    desc.NumRenderTargets = 1;
    desc.RTVFormats[0] = format;
    ComPtr<ID3D12PipelineState> pipeline;
#ifdef TUXBOX_COOKED_DRAW
    frame_abi::RasterPipelineStream parts;
    parts.root.payload = root;
    parts.layout.payload = desc.InputLayout;
    parts.vs.payload = desc.VS;
    parts.ps.payload = desc.PS;
    parts.blend.payload = desc.BlendState;
    parts.raster.payload = desc.RasterizerState;
    parts.formats.payload.NumRenderTargets = 1;
    parts.formats.payload.RTFormats[0] = format;
    parts.dsv.payload = DXGI_FORMAT_UNKNOWN;
    D3D12_PIPELINE_STATE_STREAM_DESC stream{sizeof(parts), &parts};
    using CreateStream = HRESULT (WINAPI*)(ID3D12Device2*, const D3D12_PIPELINE_STATE_STREAM_DESC*, REFIID, void**);
    ComPtr<ID3D12Device2> device2;
    check(dev->QueryInterface(IID_PPV_ARGS(&device2)), "query ID3D12Device2");
    const auto pipeline_result = frame_abi::method<CreateStream>(device2.Get(), 86)(device2.Get(), &stream, IID_PPV_ARGS(&pipeline));
    std::fprintf(output, "entry_%u_pipeline=0x%08x\n", entry, static_cast<unsigned>(pipeline_result));
    check(pipeline_result, "slot 86");
#else
    check(dev->CreateGraphicsPipelineState(&desc, IID_PPV_ARGS(&pipeline)), "create real graphics PSO");
#endif

    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    D3D12_COMMAND_QUEUE_DESC q{};
    check(dev->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "create queue");
    check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "create allocator");
#ifdef TUXBOX_COOKED_DRAW
    const UINT params[]{static_cast<UINT>(q.Type), 0, 1, 0};
    check(frame_abi::method<frame_abi::CreateList>(dev, 73)(dev, params, alloc.Get(), pipeline.Get(),
          IID_PPV_ARGS(&list)), "create guest list");
#else
    check(dev->CreateCommandList(0, q.Type, alloc.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "create list");
#endif
    auto srv = heap(dev, D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 96);
    auto sampler = heap(dev, D3D12_DESCRIPTOR_HEAP_TYPE_SAMPLER, 16);
    auto rtv = heap(dev, D3D12_DESCRIPTOR_HEAP_TYPE_RTV, 2);
    auto input = resource(dev, D3D12_HEAP_TYPE_DEFAULT, 1, 1, D3D12_RESOURCE_STATE_RENDER_TARGET);
    auto target = resource(dev, D3D12_HEAP_TYPE_DEFAULT, extent, extent, D3D12_RESOURCE_STATE_RENDER_TARGET);
    auto upload = resource(dev, D3D12_HEAP_TYPE_UPLOAD, 768, 1, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto readback = resource(dev, D3D12_HEAP_TYPE_READBACK, pitch * extent, 1, D3D12_RESOURCE_STATE_COPY_DEST);
    const auto cpu = cpu_start(srv.Get());
    const auto gpu = gpu_start(srv.Get());
    const UINT stride = dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    dev->CreateShaderResourceView(input.Get(), nullptr, cpu);
    D3D12_SAMPLER_DESC s{};
    s.Filter = D3D12_FILTER_MIN_MAG_MIP_POINT;
    s.AddressU = s.AddressV = s.AddressW = D3D12_TEXTURE_ADDRESS_MODE_CLAMP;
    s.ComparisonFunc = D3D12_COMPARISON_FUNC_ALWAYS;
    s.MaxAnisotropy = 1;
    s.MaxLOD = D3D12_FLOAT32_MAX;
    dev->CreateSampler(&s, cpu_start(sampler.Get()));
    const auto in_rtv = cpu_start(rtv.Get());
    const D3D12_CPU_DESCRIPTOR_HANDLE out_rtv{in_rtv.ptr +
                                              dev->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV)};
    dev->CreateRenderTargetView(input.Get(), nullptr, in_rtv);
    dev->CreateRenderTargetView(target.Get(), nullptr, out_rtv);

    void* data{};
    D3D12_RANGE none{};
    check(upload->Map(0, &none, &data), "map input");
    std::memset(data, 0, 768);
    auto* floats = static_cast<float*>(data);
    const float corners[3][2] = {{-1, -1}, {-1, 3}, {3, -1}};
    for (UINT i = 0; i < 3; ++i) {
        auto* vertex = floats + 18 * i;
        vertex[6] = corners[i][0];
        vertex[7] = corners[i][1];
        for (UINT c = 8; c != 16; ++c)
            vertex[c] = 1.f;
    }
    // Pixel b0 suppresses gamma conversion. Vertex b0 is an identity transform.
    floats[64] = floats[65] = floats[67] = 1.f;
    for (UINT i = 0; i < 4; ++i)
        floats[128 + 5 * i] = 1.f;
    upload->Unmap(0, nullptr);
    const auto addr = upload->GetGPUVirtualAddress();
    D3D12_CONSTANT_BUFFER_VIEW_DESC cb{addr + 256, 256};
    dev->CreateConstantBufferView(&cb, {cpu.ptr + stride * 64});
    cb.BufferLocation += 256;
    dev->CreateConstantBufferView(&cb, {cpu.ptr + stride * 80});
    const float red[4] = {1, 0, 0, 1}, blue[4] = {0, 0, 1, 1};
    command(list.Get(), 52, &ID3D12GraphicsCommandList::ClearRenderTargetView)(in_rtv, red, 0, nullptr);
    transition(list.Get(), input.Get(), D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE);
    command(list.Get(), 52, &ID3D12GraphicsCommandList::ClearRenderTargetView)(out_rtv, blue, 0, nullptr);
    ID3D12DescriptorHeap* heaps[] = {srv.Get(), sampler.Get()};
    command(list.Get(), 32, &ID3D12GraphicsCommandList::SetDescriptorHeaps)(2, heaps);
    command(list.Get(), 34, &ID3D12GraphicsCommandList::SetGraphicsRootSignature)(root);
    const auto set_table = command(list.Get(), 36, &ID3D12GraphicsCommandList::SetGraphicsRootDescriptorTable);
    set_table(0, gpu);
    set_table(1, {gpu.ptr + 64 * stride});
    set_table(2, gpu_start(sampler.Get()));
    set_table(4, {gpu.ptr + 80 * stride});
    command(list.Get(), 50, &ID3D12GraphicsCommandList::OMSetRenderTargets)(1, &out_rtv, FALSE, nullptr);
    D3D12_VIEWPORT viewport{0, 0, extent, extent, 0, 1};
    const D3D12_RECT scissor{3, 2, 12, 11};
    list->RSSetViewports(1, &viewport);
    list->RSSetScissorRects(1, &scissor);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    D3D12_VERTEX_BUFFER_VIEW vb{addr, 3 * 72, 72};
    command(list.Get(), 48, &ID3D12GraphicsCommandList::IASetVertexBuffers)(0, 1, &vb);
    list->DrawInstanced(3, 1, 0, 0);
    transition(list.Get(), target.Get(), D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION from{}, to{};
    from.pResource = target.Get();
    to.pResource = readback.Get();
    to.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    to.PlacedFootprint.Footprint = {format, extent, extent, 1, pitch};
#ifdef TUXBOX_COOKED_DRAW
    frame_abi::method<frame_abi::Copy>(list.Get(), 83)(list.Get(), &to, 0, 0, 0, &from, nullptr, 0);
#else
    list->CopyTextureRegion(&to, 0, 0, 0, &from, nullptr);
#endif
    check(list->Close(), "close list");
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    ComPtr<ID3D12Fence> fence;
    check(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    check(queue->Signal(fence.Get(), 1), "signal fence");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event)
        throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD waited = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "schedule event");
    if (waited != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence failed");
    D3D12_RANGE read{0, pitch * extent};
    check(readback->Map(0, &read, &data), "map readback");
    UINT guards = 0, interior = 0;
    auto* pixels = static_cast<const UINT*>(data);
    for (LONG y = 0; y < extent; ++y) {
        for (LONG x = 0; x < extent; ++x) {
            const UINT actual = pixels[y * pitch / 4 + x];
            if (x >= scissor.left && x < scissor.right && y >= scissor.top && y < scissor.bottom)
                interior += actual == 0xff0000ff;
            else
                guards += actual == 0xffff0000;
        }
    }
    std::fprintf(output, "entry_%u_drawn_red=%u\nentry_%u_guard_blue=%u\nentry_%u_center=%08x\n", entry, interior,
                 entry, guards, entry, pixels[8 * pitch / 4 + 8]);
    readback->Unmap(0, &none);
    if (interior != 81 || guards != 175)
        throw std::runtime_error("unexpected rasterized pixel values");
}

int main() {
    FILE* out{};
    if (fopen_s(&out, "readback.txt", "wb"))
        return 2;
    int status = 0;
    try {
        DXGI_ADAPTER_DESC1 gpu{};
#ifdef TUXBOX_COOKED_DRAW
        const auto temp = std::filesystem::absolute("temp");
        if (!SetEnvironmentVariableW(L"TMP", temp.c_str()) || !SetEnvironmentVariableW(L"TEMP", temp.c_str()))
            throw std::runtime_error("set guest cache directory");
        constexpr std::array<UINT, 18> release{0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1};
        auto dev = frame_abi::create_device(nullptr, gpu, release.data());
#else
        ComPtr<ID3D12Device> dev;
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&dev)), "D3D12CreateDevice");
        ComPtr<IDXGIFactory4> factory;
        ComPtr<IDXGIAdapter1> adapter;
        check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
        check(factory->EnumAdapterByLuid(dev->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "EnumAdapterByLuid");
        check(adapter->GetDesc1(&gpu), "GetDesc1");
#endif
        std::fprintf(out, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
        ComPtr<ID3D12RootSignature> root;
        check(dev->CreateRootSignature(0, cooked_shader, sizeof(cooked_shader), IID_PPV_ARGS(&root)),
              "create cooked root");
        draw(dev.Get(), root.Get(), {pixel_a, sizeof(pixel_a)}, 785, out);
        draw(dev.Get(), root.Get(), {pixel_b, sizeof(pixel_b)}, 792, out);
    } catch (const std::exception& e) {
        std::fprintf(out, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(out) ? 2 : status;
}
