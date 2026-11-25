// Dispatch translated compute shaders into a view that starts partway through a guarded buffer.
#include "context.h"

using namespace shader_control;

static void dispatch(Context &ctx, FILE *receipt, const std::string &name, UINT stride, const std::string &view, UINT groups) {
    ID3D12Device *device = ctx.device.Get();
    constexpr UINT words = 384;
    constexpr UINT first_byte = 384;
    constexpr UINT view_bytes = 768;
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER table{};
    table.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
    table.DescriptorTable = {1, &range};
    D3D12_ROOT_SIGNATURE_DESC description{1, &table, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&description, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create root");
    const auto code = read_shader(name);
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.CS = {code.data(), code.size()};
    ComPtr<ID3D12PipelineState> pipeline;
    check(device->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pipeline)), "compute pipeline");

    auto seed = buffer(device, words * 4, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto out = buffer(device, words * 4, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device, words * 4, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    std::array<UINT, words> initial{};
    for (UINT i = 0; i < words; ++i)
        initial[i] = 0x76540000u + i;
    // Optional per-case seed data is copied into the default heap on the GPU.
    // Existing controls without a seed retain the original guarded pattern.
    std::ifstream input(name + ".seed", std::ios::binary);
    if (input) {
        input.read(reinterpret_cast<char *>(initial.data()), sizeof(initial));
        if (input.gcount() != sizeof(initial) || input.peek() != std::char_traits<char>::eof())
            throw std::runtime_error("invalid compute seed " + name);
    }
    upload(seed.Get(), initial.data(), sizeof(initial));
    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1,
                                  D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    ComPtr<ID3D12DescriptorHeap> heap;
    check(device->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&heap)), "create UAV heap");
    const UINT element_size = stride ? stride : sizeof(UINT);
    if (first_byte % element_size || view_bytes % element_size)
        throw std::runtime_error("unaligned test view");
    D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
    uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
    if (view == "structured" && stride) uav.Format = DXGI_FORMAT_UNKNOWN;
    else if (view == "uint" && !stride) uav.Format = DXGI_FORMAT_R32_UINT;
    else if (view == "float" && !stride) uav.Format = DXGI_FORMAT_R32_FLOAT;
    else if (view == "raw" && !stride) uav.Format = DXGI_FORMAT_R32_TYPELESS;
    else throw std::runtime_error("invalid UAV format");
    uav.Buffer = {first_byte / element_size, view_bytes / element_size, stride, 0,
                  view == "raw" ? D3D12_BUFFER_UAV_FLAG_RAW : D3D12_BUFFER_UAV_FLAG_NONE};
    device->CreateUnorderedAccessView(out.Get(), nullptr, &uav, heap->GetCPUDescriptorHandleForHeapStart());
    D3D12_COMMAND_QUEUE_DESC qd{};
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    check(device->CreateCommandAllocator(qd.Type, IID_PPV_ARGS(&allocator)), "allocator");
    check(device->CreateCommandList(0, qd.Type, allocator.Get(), pipeline.Get(), IID_PPV_ARGS(&list)), "list");
    list->CopyBufferRegion(out.Get(), 0, seed.Get(), 0, sizeof(initial));
    transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    ID3D12DescriptorHeap *heaps[] = {heap.Get()};
    list->SetDescriptorHeaps(1, heaps);
    list->SetComputeRootSignature(root.Get());
    list->SetComputeRootDescriptorTable(0, heap->GetGPUDescriptorHandleForHeapStart());
    list->Dispatch(groups, 1, 1);
    transition(list.Get(), out.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, out.Get(), 0, sizeof(initial));
    submit_and_wait(device, ctx.queue.Get(), list.Get());
    write_buffer(readback.Get(), name, sizeof(initial));
    std::fprintf(receipt, "%s_words=%u\n", name.c_str(), words);
}

int main() {
    FILE *receipt{};
    if (fopen_s(&receipt, "result.txt", "wb")) return 2;
    int status = 0;
    try {
        Context ctx(receipt);
        std::ifstream cases("cases.txt");
        std::string name, view;
        UINT stride, groups;
        while (cases >> name >> stride >> view >> groups) dispatch(ctx, receipt, name, stride, view, groups);
        if (!cases.eof()) throw std::runtime_error("invalid compute cases");
    } catch (const std::exception &error) {
        std::fprintf(receipt, "error=%s\n", error.what());
        status = 1;
    }
    return std::fclose(receipt) ? 2 : status;
}
