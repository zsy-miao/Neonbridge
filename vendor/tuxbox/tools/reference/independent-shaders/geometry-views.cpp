// Capture the viewport dimensions exported by Frosty's Nanite vertex shader.
#include "context.h"
#include <array>

using namespace shader_control;

static void check_views(Context &ctx) {
    auto *device = ctx.device.Get();
    D3D12_DESCRIPTOR_RANGE ranges[] = {{D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0},
                                       {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 5, 0, 0, 1}};
    D3D12_ROOT_PARAMETER table{};
    table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    table.DescriptorTable = {2, ranges};
    D3D12_ROOT_SIGNATURE_DESC layout{1, &table, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_ALLOW_STREAM_OUTPUT};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&layout, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)), "root");

    auto code = read_shader("2927");
    const UINT output_stride = 16;
    D3D12_SO_DECLARATION_ENTRY dimensions{0, "TEXCOORD", 2, 0, 4, 0};
    D3D12_GRAPHICS_PIPELINE_STATE_DESC desc{};
    desc.VS = {code.data(), code.size()};
    desc.pRootSignature = root.Get();
    desc.StreamOutput = {&dimensions, 1, &output_stride, 1, D3D12_SO_NO_RASTERIZED_STREAM};
    desc.PrimitiveTopologyType = D3D12_PRIMITIVE_TOPOLOGY_TYPE_POINT;
    desc.RasterizerState.FillMode = D3D12_FILL_MODE_SOLID;
    desc.RasterizerState.CullMode = D3D12_CULL_MODE_NONE;
    desc.SampleMask = UINT_MAX;
    desc.SampleDesc.Count = 1;
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateGraphicsPipelineState(&desc, IID_PPV_ARGS(&pipeline)), "vertex pipeline");

    D3D12_DESCRIPTOR_HEAP_DESC heap_desc{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 6,
                                         D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&heap_desc, IID_PPV_ARGS(&heap)), "heap");
    auto handle = heap->GetCPUDescriptorHandleForHeapStart();
    const auto step = device->GetDescriptorHandleIncrementSize(heap_desc.Type);
    std::array<ComPtr<ID3D12Resource>, 6> inputs;
    for (UINT i = 0; i < inputs.size(); ++i) {
        std::array<UINT, 1024> words{};
        if (i == 0) words[24] = 3; // Visible cluster count in b0.
        if (i == 4) {
            // t3 has twelve-byte cluster records in reverse instance order. Bits 14:3 select the view.
            words[0] = 2 << 3;
            words[3] = 1 << 3;
        }
        if (i == 5) {
            words.fill(0xbad);
            for (UINT view = 0; view < 3; ++view) {
                words[view * 200 + 162] = 257 + view;
                words[view * 200 + 163] = 513 + view;
            }
        }
        inputs[i] = buffer(device, sizeof(words), D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
        upload(inputs[i].Get(), words.data(), sizeof(words));
        if (i == 0) {
            D3D12_CONSTANT_BUFFER_VIEW_DESC cb{inputs[i]->GetGPUVirtualAddress(), sizeof(words)};
            device->CreateConstantBufferView(&cb, handle);
        } else {
            constexpr UINT strides[] = {16, 16, 0, 0, 800};
            const UINT stride = strides[i - 1];
            D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
            srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
            srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
            srv.Format = stride ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R32_TYPELESS;
            srv.Buffer.StructureByteStride = stride;
            srv.Buffer.NumElements = sizeof(words) / (stride ? stride : 4);
            srv.Buffer.Flags = stride ? D3D12_BUFFER_SRV_FLAG_NONE : D3D12_BUFFER_SRV_FLAG_RAW;
            device->CreateShaderResourceView(inputs[i].Get(), &srv, handle);
        }
        handle.ptr += step;
    }

    constexpr UINT bytes = 80, counter = 64;
    auto output = buffer(device, bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    auto readback = buffer(device, bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get(), pipeline.Get(),
                                    IID_PPV_ARGS(&list)), "list");
    list->CopyBufferRegion(output.Get(), 0, inputs[1].Get(), 0, bytes);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_STREAM_OUT);
    D3D12_STREAM_OUTPUT_BUFFER_VIEW target{output->GetGPUVirtualAddress(), counter, output->GetGPUVirtualAddress() + counter};
    ID3D12DescriptorHeap *heaps[] = {heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetGraphicsRootSignature(root.Get());
    list->SetGraphicsRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    list->IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_POINTLIST);
    list->SOSetTargets(0, 1, &target);
    list->DrawInstanced(1, 3, 0, 0);
    transition(list.Get(), output.Get(), D3D12_RESOURCE_STATE_STREAM_OUT, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, bytes);
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(readback.Get(), "views", bytes);
}

int main() {
    FILE *report{};
    if (fopen_s(&report, "result.txt", "wb")) return 2;
    int status = 0;
    try {
        Context ctx(report);
        check_views(ctx);
    } catch (const std::exception &error) {
        std::fprintf(report, "error=%s\n", error.what());
        status = 1;
    }
    return std::fclose(report) ? 2 : status;
}
