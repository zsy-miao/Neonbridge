// Exercise the lossless D32S8 depth plane / staging buffer / R32 UAV path.
#include "frame_abi.h"
#include "modify_dxil.h"
#include <algorithm>
#include <cstring>
using frame_abi::check;
using frame_abi::ComPtr;

static ComPtr<ID3D12Resource> resource(ID3D12Device* dev, D3D12_RESOURCE_DESC desc, D3D12_HEAP_TYPE heap,
                                       D3D12_RESOURCE_STATES state, const D3D12_CLEAR_VALUE* clear = nullptr) {
    D3D12_HEAP_PROPERTIES props{};
    props.Type = heap;
    props.CreationNodeMask = props.VisibleNodeMask = 1;
    ComPtr<ID3D12Resource> result;
    check(dev->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state, clear, IID_PPV_ARGS(&result)),
          "create resource");
    return result;
}

static void transition(ID3D12GraphicsCommandList* list, ID3D12Resource* res, UINT subresource,
                       D3D12_RESOURCE_STATES before, D3D12_RESOURCE_STATES after) {
    D3D12_RESOURCE_BARRIER b{};
    b.Transition = {res, subresource, before, after};
    list->ResourceBarrier(1, &b);
}

static void copy(ID3D12GraphicsCommandList* list, ID3D12Resource* texture, UINT index, ID3D12Resource* buffer,
                 D3D12_PLACED_SUBRESOURCE_FOOTPRINT footprint, bool to_buffer, bool tuxbox = false) {
    D3D12_TEXTURE_COPY_LOCATION image{}, linear{};
    image.pResource = texture;
    image.SubresourceIndex = index;
    linear.pResource = buffer;
    linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    linear.PlacedFootprint = footprint;
    auto* dst = to_buffer ? &linear : &image;
    auto* src = to_buffer ? &image : &linear;
    if (tuxbox)
        frame_abi::method<frame_abi::Copy>(list, 83)(list, dst, 0, 0, 0, src, nullptr, 0);
    else
        list->CopyTextureRegion(dst, 0, 0, 0, src, nullptr);
}

static void run(FILE* out, UINT width, UINT height, DXGI_FORMAT depth_format, ID3D12Device* provider,
                bool placed, bool tuxbox) {
    const D3D12_RECT inset{5, 3, LONG(width) - 7, LONG(height) - 3};
    ComPtr<ID3D12Device> dev;
    check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&dev)), "D3D12CreateDevice");
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "CreateDXGIFactory1");
    check(factory->EnumAdapterByLuid(dev->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "EnumAdapterByLuid");
    check(adapter->GetDesc1(&gpu), "GetDesc1");
    std::fprintf(out, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);

    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Format = depth_format;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
    D3D12_CLEAR_VALUE clear{};
    clear.Format = DXGI_FORMAT_D32_FLOAT_S8X24_UINT;
    clear.DepthStencil = {.25f, 0x37};
    const auto initial_state = placed ? D3D12_RESOURCE_STATE_DEPTH_WRITE :
        D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE | D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE;
    if (provider)
        desc.Flags = static_cast<D3D12_RESOURCE_FLAGS>(0x01000006);
    ComPtr<ID3D12Resource> dsv;
    ComPtr<ID3D12Heap> heap;
    if (placed) {
        constexpr UINT64 offset = 0x52c0000;
        // The Xbox allocation query takes the result pointer before the device.
        using Query = void (WINAPI*)(D3D12_RESOURCE_ALLOCATION_INFO*, ID3D12Device*, UINT, UINT,
                                     const D3D12_RESOURCE_DESC*);
        D3D12_RESOURCE_ALLOCATION_INFO allocation{};
        frame_abi::method<Query>(provider, 25)(&allocation, provider, 0, 1, &desc);
        if (!allocation.Alignment || allocation.SizeInBytes == UINT64_MAX || offset % allocation.Alignment)
            throw std::runtime_error("invalid placed depth allocation");
        D3D12_HEAP_DESC memory{};
        memory.Properties.Type = D3D12_HEAP_TYPE_DEFAULT;
        memory.Properties.CreationNodeMask = memory.Properties.VisibleNodeMask = 1;
        memory.SizeInBytes = offset + allocation.SizeInBytes;
        memory.Flags = D3D12_HEAP_FLAG_ALLOW_ONLY_RT_DS_TEXTURES;
        check(provider->CreateHeap(&memory, IID_PPV_ARGS(&heap)), "create depth heap");
        check(provider->CreatePlacedResource(heap.Get(), offset, &desc, initial_state, &clear,
                                             IID_PPV_ARGS(&dsv)), "place depth resource");
        std::fprintf(out, "offset=%llu\nallocation=%llu\n", offset, allocation.SizeInBytes);
    } else {
        dsv = resource(provider ? provider : dev.Get(), desc, D3D12_HEAP_TYPE_DEFAULT, initial_state, &clear);
    }
    if (provider) {
        dev.Reset();
        check(dsv->GetDevice(IID_PPV_ARGS(&dev)), "get device from paired resource");
        desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL;
    }
    std::fprintf(out, "resource_format=%u\nwidth=%u\nheight=%u\n", depth_format, width, height);
    D3D12_PLACED_SUBRESOURCE_FOOTPRINT planes[2]{}, color{};
    UINT rows[2]{};
    UINT64 row_bytes[2]{}, bytes = 0, color_size = 0;
    dev->GetCopyableFootprints(&desc, 0, 2, 0, planes, rows, row_bytes, &bytes);
    desc.Format = DXGI_FORMAT_R32_FLOAT;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> uav;
    if (!provider) uav = resource(dev.Get(), desc, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    UINT64 color_row_bytes{};
    dev->GetCopyableFootprints(&desc, 0, 1, 0, &color, nullptr, &color_row_bytes, &color_size);
    std::fprintf(out,
                 "depth_format=%u\nstencil_format=%u\nuav_format=%u\ndepth_pitch=%u\nstencil_pitch=%u\nuav_pitch=%u\n",
                 planes[0].Footprint.Format, planes[1].Footprint.Format, color.Footprint.Format,
                 planes[0].Footprint.RowPitch, planes[1].Footprint.RowPitch, color.Footprint.RowPitch);
    if (row_bytes[0] != width * 4 || color_row_bytes != width * 4 || row_bytes[1] != width || rows[0] != height ||
        rows[1] != height)
        throw std::runtime_error("unexpected plane layout");
    // GetCopyableFootprints may choose a different pitch for DS and R32. Use the
    // same aligned pitch on both buffer interpretations so row n names the same bytes.
    UINT common_pitch = std::max(planes[0].Footprint.RowPitch, color.Footprint.RowPitch);
    auto staging_depth = planes[0];
    staging_depth.Footprint.RowPitch = color.Footprint.RowPitch = common_pitch;
    desc = {};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = UINT64(common_pitch) * height;
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    ComPtr<ID3D12Resource> staging;
    if (!provider) staging = resource(dev.Get(), desc, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST);
    desc.Width = bytes;
    auto readback = resource(dev.Get(), desc, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);

    D3D12_DESCRIPTOR_HEAP_DESC hd{D3D12_DESCRIPTOR_HEAP_TYPE_DSV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_NONE, 0};
    ComPtr<ID3D12DescriptorHeap> dsv_heap, uav_heap;
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&dsv_heap)), "create DSV heap");
    hd = {D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, 1, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 0};
    check(dev->CreateDescriptorHeap(&hd, IID_PPV_ARGS(&uav_heap)), "create UAV heap");
    auto cpu = [&](ID3D12DescriptorHeap* object) {
        return tuxbox ? D3D12_CPU_DESCRIPTOR_HANDLE{frame_abi::method<frame_abi::CpuHandle>(object, 9)(object)}
                      : object->GetCPUDescriptorHandleForHeapStart();
    };
    const auto depth_handle = cpu(dsv_heap.Get());
    D3D12_DEPTH_STENCIL_VIEW_DESC view{};
    view.Format = clear.Format;
    view.ViewDimension = D3D12_DSV_DIMENSION_TEXTURE2D;
    dev->CreateDepthStencilView(dsv.Get(), &view, depth_handle);
    if (provider) {
        D3D12_UNORDERED_ACCESS_VIEW_DESC plane{};
        plane.Format = DXGI_FORMAT_R32_FLOAT_X8X24_TYPELESS;
        plane.ViewDimension = D3D12_UAV_DIMENSION_TEXTURE2D;
        provider->CreateUnorderedAccessView(dsv.Get(), nullptr, &plane, cpu(uav_heap.Get()));
    } else {
        dev->CreateUnorderedAccessView(uav.Get(), nullptr, nullptr, uav_heap->GetCPUDescriptorHandleForHeapStart());
    }
    D3D12_DESCRIPTOR_RANGE range{D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, 0};
    D3D12_ROOT_PARAMETER param{};
    param.DescriptorTable = {1, &range};
    D3D12_ROOT_SIGNATURE_DESC sig{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&sig, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(dev->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&root)),
          "create root");
    D3D12_COMPUTE_PIPELINE_STATE_DESC pd{};
    pd.pRootSignature = root.Get();
    pd.CS = {modify_dxil, sizeof(modify_dxil)};
    ComPtr<ID3D12PipelineState> pso;
    if (tuxbox) {
        using Compute = HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                         UINT, const void*, REFIID, void**);
        check(frame_abi::method<Compute>(dev.Get(), 62)(dev.Get(), &pd, 0, nullptr, IID_PPV_ARGS(&pso)),
              "create Xbox compute PSO");
    } else {
        check(dev->CreateComputePipelineState(&pd, IID_PPV_ARGS(&pso)), "create compute PSO");
    }

    ComPtr<ID3D12CommandAllocator> alloc;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC q{};
    check((provider ? provider : dev.Get())->CreateCommandQueue(&q, IID_PPV_ARGS(&queue)), "create queue");
    check(dev->CreateCommandAllocator(q.Type, IID_PPV_ARGS(&alloc)), "create allocator");
    if (provider) {
        const UINT params[]{0, 0, 1, 0};
        check(frame_abi::method<frame_abi::CreateList>(provider, 73)(provider, params, alloc.Get(),
                pso.Get(), IID_PPV_ARGS(&list)), "create Xbox list");
    } else {
    check(dev->CreateCommandList(0, q.Type, alloc.Get(), pso.Get(), IID_PPV_ARGS(&list)), "create list");
    }
    transition(list.Get(), dsv.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES, initial_state,
               D3D12_RESOURCE_STATE_DEPTH_WRITE);
    const auto both = D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL;
    using Clear = void(WINAPI*)(ID3D12GraphicsCommandList*, D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_CLEAR_FLAGS,
                                 FLOAT, UINT8, UINT, const D3D12_RECT*);
    const auto clear_depth = frame_abi::method<Clear>(list.Get(), provider ? 51 : 47);
    if (provider) {
        using Discard = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12Resource*, const D3D12_DISCARD_REGION*);
        const D3D12_DISCARD_REGION plane_zero{0, nullptr, 0, 1};
        frame_abi::method<Discard>(list.Get(), 55)(list.Get(), dsv.Get(), &plane_zero);
    }
    clear_depth(list.Get(), depth_handle, both, .25f, 0x37, 0, nullptr);
    clear_depth(list.Get(), depth_handle, both, .75f, 0xc9, 1, &inset);
    if (provider) {
        transition(list.Get(), dsv.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                   D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    } else {
        transition(list.Get(), dsv.Get(), 0, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_COPY_SOURCE);
        copy(list.Get(), dsv.Get(), 0, staging.Get(), staging_depth, true);
        transition(list.Get(), staging.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        copy(list.Get(), uav.Get(), 0, staging.Get(), color, false);
        transition(list.Get(), uav.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    }
    using Heaps = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
    using Root = void(WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
    using Table = void(WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
    frame_abi::method<Heaps>(list.Get(), provider ? 32 : 28)(list.Get(), 1, uav_heap.GetAddressOf());
    frame_abi::method<Root>(list.Get(), provider ? 33 : 29)(list.Get(), root.Get());
    using GpuHandle = UINT64(WINAPI*)(ID3D12DescriptorHeap*);
    const auto uav_handle = tuxbox
        ? D3D12_GPU_DESCRIPTOR_HANDLE{frame_abi::method<GpuHandle>(uav_heap.Get(), 10)(uav_heap.Get())}
        : uav_heap->GetGPUDescriptorHandleForHeapStart();
    frame_abi::method<Table>(list.Get(), provider ? 35 : 31)(list.Get(), 0, uav_handle);
    list->Dispatch((width + 7) / 8, (height + 7) / 8, 1);
    if (provider) {
        transition(list.Get(), dsv.Get(), D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                   D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
    } else {
        transition(list.Get(), uav.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        transition(list.Get(), staging.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        copy(list.Get(), uav.Get(), 0, staging.Get(), color, true);
        transition(list.Get(), staging.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        transition(list.Get(), dsv.Get(), 0, D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        copy(list.Get(), dsv.Get(), 0, staging.Get(), staging_depth, false);
        transition(list.Get(), dsv.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_COPY_SOURCE);
        transition(list.Get(), dsv.Get(), 1, D3D12_RESOURCE_STATE_DEPTH_WRITE, D3D12_RESOURCE_STATE_COPY_SOURCE);
    }
    for (UINT i = 0; i != 2; ++i)
        copy(list.Get(), dsv.Get(), i, readback.Get(), planes[i], true, tuxbox);
    check(list->Close(), "close list");
    ID3D12CommandList* lists[] = {list.Get()};
    queue->ExecuteCommandLists(1, lists);
    ComPtr<ID3D12Fence> fence;
    check(dev->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    check(queue->Signal(fence.Get(), 1), "signal queue");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event)
        throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "schedule GPU event");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence failed");
    void* data{};
    D3D12_RANGE read{0, SIZE_T(bytes)}, empty{};
    check(readback->Map(0, &read, &data), "map readback");
    auto base = static_cast<const BYTE*>(data);
    UINT depth_ok = 0, stencil_ok = 0, updated = 0;
    for (UINT y = 0; y < height; ++y)
        for (UINT x = 0; x < width; ++x) {
            bool center =
                x >= UINT(inset.left) && x < UINT(inset.right) && y >= UINT(inset.top) && y < UINT(inset.bottom);
            UINT expected = center ? 0x3f400000 : 0x3e800000;
            if ((x + 3 * y) % 5 == 0) {
                expected += x + 7 * y + 1;
                ++updated;
            }
            UINT actual{};
            std::memcpy(&actual, base + planes[0].Offset + y * planes[0].Footprint.RowPitch + 4 * x, 4);
            depth_ok += actual == expected;
            stencil_ok += base[planes[1].Offset + y * planes[1].Footprint.RowPitch + x] == (center ? 0xc9 : 0x37);
        }
    readback->Unmap(0, &empty);
    std::fprintf(out, "matched_depth=%u\nmatched_stencil=%u\nexpected_modified=%u\n", depth_ok, stencil_ok, updated);
    if (depth_ok != width * height || stencil_ok != width * height)
        throw std::runtime_error("depth/stencil readback mismatch");
}

static int test(const char* name, UINT width, UINT height, DXGI_FORMAT format, ID3D12Device* provider = nullptr,
                bool placed = false, bool tuxbox = false) {
    FILE* out{};
    if (fopen_s(&out, name, "wb"))
        return 2;
    int status = 0;
    try {
        run(out, width, height, format, provider, placed, tuxbox);
    } catch (const std::exception& e) {
        std::fprintf(out, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(out) ? 2 : status;
}

int main(int argc, char** argv) {
    if (argc > 1) {
        HMODULE module = LoadLibraryExW(L"d3d12_xs.dll", nullptr,
            LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        if (!module) return 2;
        const auto factory = reinterpret_cast<decltype(&D3D12XboxCreateDevice)>(
            GetProcAddress(module, "D3D12XboxCreateDevice"));
        if (!factory) return 2;
        const std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        ComPtr<ID3D12Device> provider;
        if (FAILED(factory(nullptr, profile.data(), IID_PPV_ARGS(&provider)))) return 2;
        const bool tuxbox = argc == 3 && !std::strcmp(argv[2], "--tuxbox");
        if (!std::strcmp(argv[1], "placed"))
            return test("placed.txt", 2976, 1676, DXGI_FORMAT_R32G8X24_TYPELESS, provider.Get(), true, tuxbox);
        return test("wrapped.txt", 1024, 1024, DXGI_FORMAT_R32G8X24_TYPELESS, provider.Get(), false, tuxbox);
    }
    int typed = test("typed.txt", 73, 19, DXGI_FORMAT_D32_FLOAT_S8X24_UINT);
    int typeless = test("typeless.txt", 1024, 1024, DXGI_FORMAT_R32G8X24_TYPELESS);
    return typed || typeless;
}
