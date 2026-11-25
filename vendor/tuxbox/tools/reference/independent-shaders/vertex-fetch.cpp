// Capture the two declared vertex exports directly before rasterization.
#include "context.h"
#include <array>

using namespace shader_control;

static ComPtr<ID3D12Resource> vertex_buffer(ID3D12Device *device, const std::string &file, UINT bytes) {
    std::ifstream input(file, std::ios::binary);
    std::vector<char> data(bytes);
    if (!input.read(data.data(), bytes) || input.peek() != std::char_traits<char>::eof())
        throw std::runtime_error("invalid input buffer " + file);
    auto vertex = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    upload(vertex.Get(), data.data(), data.size());
    return vertex;
}

static void draw(Context &ctx, ID3D12RootSignature *root, UINT entry, bool split) {
    auto *device = ctx.device.Get();
    const auto name = std::to_string(entry);
    const std::string layout = split ? "split" : "interleaved";
    const auto vs = read_shader(name);
    const DXGI_FORMAT pos_format = entry == 2751 ? DXGI_FORMAT_R32G32_FLOAT : DXGI_FORMAT_R32G32B32A32_FLOAT;
    D3D12_INPUT_ELEMENT_DESC elements[] = {
        {"ATTRIBUTE", 1, DXGI_FORMAT_R32G32_FLOAT, 0, 4, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0},
        {"ATTRIBUTE", 0, pos_format, split ? 1u : 0u, split ? 8u : 16u, D3D12_INPUT_CLASSIFICATION_PER_VERTEX_DATA, 0}
    };
    D3D12_SO_DECLARATION_ENTRY exports[] = {
        {0, "SV_Position", 0, 0, 4, 0}, {0, "TEXCOORD", 0, 0, 2, 0}
    };
    const bool uv_output = entry == 2450;
    const UINT output_stride = uv_output ? 24 : 16;
    D3D12_GRAPHICS_PIPELINE_STATE_DESC desc{};
    desc.pRootSignature = root;
    desc.VS = {vs.data(), vs.size()};
    desc.InputLayout = entry == 1007 ? D3D12_INPUT_LAYOUT_DESC{elements + 1, 1} : D3D12_INPUT_LAYOUT_DESC{elements, 2};
    desc.StreamOutput = {exports, uv_output ? 2u : 1u, &output_stride, 1, D3D12_SO_NO_RASTERIZED_STREAM};
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.SampleMask = UINT_MAX;
    desc.SampleDesc.Count = 1;
    ComPtr<ID3D12PipelineState> state;
    check(device->CreateGraphicsPipelineState(&desc, IID_PPV_ARGS(&state)), "stream output pipeline");

    std::array<D3D12_VERTEX_BUFFER_VIEW, 2> views{};
    std::array<ComPtr<ID3D12Resource>, 2> buffers;
    const UINT slots = split ? 2 : 1;
    for (UINT slot = 0; slot < slots; ++slot) {
        const UINT stride = split ? (slot == 0 ? 24 : 32) : 48;
        const UINT bytes = 5 * stride;
        buffers[slot] = vertex_buffer(device, layout + std::to_string(slot) + ".vb", bytes);
        views[slot] = {buffers[slot]->GetGPUVirtualAddress(), bytes, stride};
    }
    std::array<BYTE, 128> initial;
    initial.fill(0xcd);
    constexpr UINT counter_offset = 112;
    std::memset(initial.data() + counter_offset, 0, sizeof(UINT64));
    constexpr UINT bytes = static_cast<UINT>(initial.size());
    auto seed = buffer(device, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto output = buffer(device, bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    auto read = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    upload(seed.Get(), initial.data(), bytes);
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), state.Get(),
                                    IID_PPV_ARGS(&list)), "list");
    list->CopyBufferRegion(output.Get(), 0, seed.Get(), 0, bytes);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_STREAM_OUT);
    const auto address = output->GetGPUVirtualAddress();
    D3D12_STREAM_OUTPUT_BUFFER_VIEW target{address + 16, 80, address + counter_offset};
    list->SetGraphicsRootSignature(root);
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_POINTLIST);
    list->IASetVertexBuffers(0, slots, views.data());
    list->SOSetTargets(0, 1, &target);
    list->DrawInstanced(3, 1, 1, 0);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_STREAM_OUT, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(read.Get(), 0, output.Get(), 0, bytes);
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(read.Get(), name + "-" + layout, bytes);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    int status = 0;
    try {
        Context ctx(receipt);
        D3D12_ROOT_SIGNATURE_DESC desc{};
        desc.Flags = D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT | D3D12_ROOT_SIGNATURE_FLAG_ALLOW_STREAM_OUTPUT;
        ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
        ComPtr<ID3D12RootSignature> root;
        check(ctx.device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                              IID_PPV_ARGS(&root)), "root");
        for (UINT entry : {1007u, 2450u, 2751u})
            for (bool split : {false, true})
                draw(ctx, root.Get(), entry, split);
    } catch (const std::exception &error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        status = 1;
    }
    return std::fclose(receipt) ? 2 : status;
}
