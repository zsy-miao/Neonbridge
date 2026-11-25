// Independent forced-fallback control: malformed synthetic bytecode is
// intentionally rejected by the host. It contains no title code.
#include "frame_abi.h"
#include "frame_cli.h"
#include "compute_fixture.h"
#include "affine.h"

namespace {
using namespace frame_abi;
using compute_fixture::buffer;
using compute_fixture::upload;
using compute_fixture::transition;
using Pipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                  UINT, const void*, REFIID, void**);
using SetRoot = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using SetConstants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
using SetUav = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
std::string run(const wchar_t* requested, bool disabled) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected);
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[0].Constants = {5, 2, 4};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {3, 4};
    D3D12_ROOT_SIGNATURE_DESC desc{2, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create root");
    const UINT invalid_shader[] = {0x12345678,0x90abcdef};
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {invalid_shader, sizeof(invalid_shader)};
    ComPtr<ID3D12PipelineState> unused, substitute, good;
    auto create = method<Pipeline>(device.Get(), 62);
    if (disabled) {
        const auto hr = create(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&unused));
        if (SUCCEEDED(hr) || unused) throw std::runtime_error("invalid shader accepted with fallback disabled");
        shader.CS = {affine_dxil, sizeof(affine_dxil)};
        check(create(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&good)), "valid control after rejection");
        if (!good) throw std::runtime_error("valid pipeline missing after rejection");
        char receipt[128];
        std::snprintf(receipt,sizeof(receipt),"d3d12-null-disabled PASS pci=%04x:%04x rejected=1 valid=1",selected.VendorId,selected.DeviceId);
        return receipt;
    }
    check(create(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&unused)), "unused fallback");
    check(create(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&substitute)), "used fallback");
    shader.CS = {affine_dxil, sizeof(affine_dxil)};
    check(create(device.Get(), &shader, 0, nullptr, IID_PPV_ARGS(&good)), "valid control");
    D3D12_COMMAND_QUEUE_DESC q{};
    q.Type = D3D12_COMMAND_LIST_TYPE_COMPUTE;
    ComPtr<ID3D12CommandQueue> queue;
    check(method<CreateQueue>(device.Get(), 48)(device.Get(), &q, IID_PPV_ARGS(&queue)), "queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(device->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&allocator)), "allocator");
    const UINT list_desc[] = {static_cast<UINT>(q.Type),0,1,0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<CreateList>(device.Get(), 73)(device.Get(), list_desc, allocator.Get(), substitute.Get(),
          IID_PPV_ARGS(&list)), "list");
    std::array<UINT,64> expected{};
    expected.fill(0xcdcdcdcd);
    auto seed = buffer(device.Get(),D3D12_HEAP_TYPE_UPLOAD,sizeof(expected),D3D12_RESOURCE_STATE_GENERIC_READ);
    auto out = buffer(device.Get(),D3D12_HEAP_TYPE_DEFAULT,sizeof(expected),D3D12_RESOURCE_STATE_COPY_DEST,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto result = buffer(device.Get(),D3D12_HEAP_TYPE_READBACK,sizeof(expected),D3D12_RESOURCE_STATE_COPY_DEST);
    upload(seed.Get(),expected.data(),sizeof(expected));
    check(list->Reset(allocator.Get(),substitute.Get()),"reset null");
    list->CopyBufferRegion(out.Get(),0,seed.Get(),0,sizeof(expected));
    transition(list.Get(),out.Get(),D3D12_RESOURCE_STATE_COPY_DEST,D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    auto bind = [&](UINT first) {
        method<SetRoot>(list.Get(),33)(list.Get(),root.Get());
        method<SetUav>(list.Get(),45)(list.Get(),1,out->GetGPUVirtualAddress());
        const UINT constants[] = {first+3,11,first,8};
        method<SetConstants>(list.Get(),39)(list.Get(),0,4,constants,0);
    };
    bind(0);
    // First checkpoint at 1; then the second at 64. A valid dispatch
    // in between must not be included. No presentation is involved.
    list->Dispatch(1,1,1);
    list->SetPipelineState(good.Get());
    bind(0);
    list->Dispatch(1,1,1);
    for (UINT i=0;i<8;++i) expected[i]=3+i*11;
    list->ClearState(substitute.Get());
    bind(8);
    for (UINT i=0;i<63;++i) list->Dispatch(1,1,1);
    list->ClearState(good.Get());
    bind(16);
    list->Dispatch(1,1,1);
    for (UINT i=0;i<8;++i) expected[16+i]=19+i*11;
    transition(list.Get(),out.Get(),D3D12_RESOURCE_STATE_UNORDERED_ACCESS,D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(result.Get(),0,out.Get(),0,sizeof(expected));
    check(list->Close(),"close");
    ComPtr<ID3D12Fence> fence;
    check(device->CreateFence(0,D3D12_FENCE_FLAG_NONE,IID_PPV_ARGS(&fence)),"fence");
    ID3D12CommandList* batch[]={list.Get()};
    queue->ExecuteCommandLists(1,batch);
    check(queue->Signal(fence.Get(),1),"signal");
    compute_fixture::wait_for_fence(fence.Get(),1);
    D3D12_RANGE read{0,sizeof(expected)},empty{};
    void* mapped=nullptr;
    check(result->Map(0,&read,&mapped),"readback");
    bool equal=std::memcmp(mapped,expected.data(),sizeof(expected))==0;
    result->Unmap(0,&empty);
    if (!equal) throw std::runtime_error("fallback or valid control wrote unexpected words");
    char receipt[128];
    std::snprintf(receipt,sizeof(receipt),"d3d12-null-usage PASS pci=%04x:%04x words=64/64 recorded=64 valid=2",selected.VendorId,selected.DeviceId);
    return receipt;
}
}
int wmain(int argc,wchar_t** argv) {
    if (argc > 1 && !std::wcscmp(argv[1], L"--reject"))
        return tuxbox_test::frame_main(argc-1,argv+1,"d3d12-null-disabled",false,
            [](const wchar_t* adapter, bool) { return run(adapter,true); });
    return tuxbox_test::frame_main(argc,argv,"d3d12-null-usage",false,run);
}
