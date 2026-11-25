// Render a scissored triangle at a known depth. Read back every declared color target and depth.
#include "context.h"
#include <algorithm>

using namespace shader_control;
static constexpr UINT side = 16, pitch = 256, plane_size = side * pitch;

static ComPtr<ID3D12Resource> surface(ID3D12Device *device, DXGI_FORMAT format, bool depth) {
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = desc.Height = side;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = format;
    desc.Flags = depth ? D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL : D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    D3D12_HEAP_PROPERTIES hp{};
    hp.Type = D3D12_HEAP_TYPE_DEFAULT;
    hp.CreationNodeMask = hp.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> resource;
    check(device->CreateCommittedResource(&hp, D3D12_HEAP_FLAG_NONE, &desc,
          depth ? D3D12_RESOURCE_STATE_DEPTH_WRITE : D3D12_RESOURCE_STATE_RENDER_TARGET,
          nullptr, IID_PPV_ARGS(&resource)), "create surface");
    return resource;
}

static void copy_surface(ID3D12GraphicsCommandList *list, ID3D12Resource *surface, DXGI_FORMAT format,
                         D3D12_RESOURCE_STATES state, ID3D12Resource *dest, UINT plane) {
    transition(list, surface, state, D3D12_RESOURCE_STATE_COPY_SOURCE);
    D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
    src.pResource = surface;
    dst.pResource = dest;
    dst.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    dst.PlacedFootprint = {UINT64(plane) * plane_size, {format, side, side, 1, pitch}};
    list->CopyTextureRegion(&dst, 0, 0, 0, &src, nullptr);
}

static void draw(Context &ctx, FILE *receipt, const std::string &name, const std::string &vertex,
                 const std::vector<DXGI_FORMAT> &formats) {
    auto *device = ctx.device.Get();
    const auto ps = read_shader(name), vs = read_shader(vertex);
    D3D12_ROOT_SIGNATURE_DESC rd{};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&rd, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create root");
    D3D12_GRAPHICS_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.VS = {vs.data(), vs.size()};
    pd.PS = {ps.data(), ps.size()};
    pd.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE;
    pd.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    pd.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    pd.RasterizerState.DepthClipEnable = TRUE;
    pd.DepthStencilState.DepthEnable = TRUE;
    pd.DepthStencilState.DepthWriteMask = D3D12_DEPTH_WRITE_MASK_ALL;
    pd.DepthStencilState.DepthFunc = D3D12_COMPARISON_FUNC_LESS;
    pd.DSVFormat = DXGI_FORMAT_D32_FLOAT;
    pd.SampleDesc.Count = 1;
    pd.SampleMask = UINT_MAX;
    pd.NumRenderTargets = static_cast<UINT>(formats.size());
    pd.BlendState.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL;
    std::copy(formats.begin(), formats.end(), pd.RTVFormats);
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateGraphicsPipelineState(&pd, IID_PPV_ARGS(&pipeline)), "graphics pipeline");
    auto depth = surface(device, DXGI_FORMAT_D32_FLOAT, true);
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    ComPtr<ID3D12DescriptorHeap> dsv, rtv;
    check(device->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&dsv)), "create DSV heap");
    auto ds = dsv->GetCPUDescriptorHandleForHeapStart();
    device->CreateDepthStencilView(depth.Get(), nullptr, ds);
    std::vector<ComPtr<ID3D12Resource>> targets;
    std::vector<D3D12_CPU_DESCRIPTOR_HANDLE> handles;
    if (!formats.empty()) {
        hd.NumDescriptors = pd.NumRenderTargets;
        hd.Type = D3D12_DESCRIPTOR_HEAP_TYPE_RTV;
        check(device->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&rtv)), "create RTV heap");
        auto handle = rtv->GetCPUDescriptorHandleForHeapStart();
        for (auto format : formats) {
            targets.push_back(surface(device, format, false));
            device->CreateRenderTargetView(targets.back().Get(), nullptr, handle);
            handles.push_back(handle);
            handle.ptr += device->GetDescriptorHandleIncrementSize(hd.Type);
        }
    }
    const UINT bytes = (pd.NumRenderTargets + 1) * plane_size;
    auto read = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    // Initialize row padding as well, so each complete readback is reproducible.
    auto zeros = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    std::vector<BYTE> padding(bytes, 0);
    upload(zeros.Get(), padding.data(), bytes);
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pipeline.Get(),
                                    IID_PPV_ARGS(&list)), "list");
    list->CopyBufferRegion(read.Get(), 0, zeros.Get(), 0, bytes);
    const FLOAT guard[] = {0.25f, -0.5f, 0.75f, 1.5f};
    for (auto handle : handles) list->ClearRenderTargetView(handle, guard, 0, nullptr);
    list->ClearDepthStencilView(ds, D3D12_CLEAR_FLAG_DEPTH, 0.875f, 0, 0, nullptr);
    list->OMSetRenderTargets(pd.NumRenderTargets, handles.data(), FALSE, &ds);
    list->SetGraphicsRootSignature(root.Get());
    D3D12_VIEWPORT viewport{0, 0, side, side, 0, 1};
    D3D12_RECT scissor{3, 2, 12, 11};
    list->RSSetViewports(1, &viewport);
    list->RSSetScissorRects(1, &scissor);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
    list->DrawInstanced(3, 1, 0, 0);
    for (UINT i = 0; i < pd.NumRenderTargets; ++i)
        copy_surface(list.Get(), targets[i].Get(), formats[i], D3D12_RESOURCE_STATE_RENDER_TARGET, read.Get(), i);
    copy_surface(list.Get(), depth.Get(), DXGI_FORMAT_R32_TYPELESS, D3D12_RESOURCE_STATE_DEPTH_WRITE,
                 read.Get(), pd.NumRenderTargets);
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(read.Get(), name, bytes);
    std::fprintf(receipt, "%s_planes=%u\n", name.c_str(), pd.NumRenderTargets + 1);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    int status = 0;
    try {
        Context ctx(receipt);
        std::ifstream cases("cases.txt");
        std::string name, vertex;
        UINT count;
        while (cases >> name >> vertex >> count) {
            if (count > D3D12_SIMULTANEOUS_RENDER_TARGET_COUNT) throw std::runtime_error("too many targets");
            std::vector<DXGI_FORMAT> formats;
            for (UINT i = 0; i < count; ++i) {
                UINT n;
                if (!(cases >> n)) throw std::runtime_error("missing target format");
                switch (n) {
                    case 1: formats.push_back(DXGI_FORMAT_R32_FLOAT); break;
                    case 2: formats.push_back(DXGI_FORMAT_R32G32_FLOAT); break;
                    case 4: formats.push_back(DXGI_FORMAT_R32G32B32A32_FLOAT); break;
                    default: throw std::runtime_error("bad target format");
                }
            }
            try {
                draw(ctx, receipt, name, vertex, formats);
            } catch (const std::exception &error) {
                std::fprintf(receipt, "%s_error=%s\n", name.c_str(), error.what());
                status = 1;
            }
        }
        if (!cases.eof()) throw std::runtime_error("invalid pixel cases");
    } catch (const std::exception &error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        status = 1;
    }
    return std::fclose(receipt) ? 2 : status;
}
