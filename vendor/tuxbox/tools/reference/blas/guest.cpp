#include "frame_abi.h"
#include "query_dxil.h"
#include <algorithm>
#include <cmath>
#include <cstring>
#include <fstream>
#include <iterator>
#include <optional>
#include <vector>

using frame_abi::check;
using frame_abi::ComPtr;
using frame_abi::method;
using Inputs = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS;
using Sizes = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO;
using Build = D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC;

static ComPtr<ID3D12Resource> buffer(ID3D12Device* dev, UINT64 bytes, D3D12_HEAP_TYPE heap,
                                     D3D12_RESOURCE_STATES state, bool uav = false) {
    D3D12_HEAP_PROPERTIES props{};
    props.Type = heap;
    props.CreationNodeMask = props.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = bytes;
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    if (uav) desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    ComPtr<ID3D12Resource> res;
    check(dev->CreateCommittedResource(&props, D3D12_HEAP_FLAG_NONE, &desc, state,
                                        nullptr, IID_PPV_ARGS(&res)), "create buffer");
    return res;
}

static void write_upload(ID3D12Resource* res, const void* data, size_t bytes) {
    D3D12_RANGE empty{}, written{0, bytes};
    void* dest{};
    check(res->Map(0, &empty, &dest), "map upload");
    std::memcpy(dest, data, bytes);
    res->Unmap(0, &written);
}

static ComPtr<ID3D12Resource> upload(ID3D12Device* dev, const void* data, size_t bytes) {
    auto res = buffer(dev, bytes, D3D12_HEAP_TYPE_UPLOAD, D3D12_RESOURCE_STATE_GENERIC_READ);
    write_upload(res.Get(), data, bytes);
    return res;
}

static ComPtr<ID3D12RootSignature> root_signature(ID3D12Device* device) {
    D3D12_ROOT_PARAMETER params[3]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[2].ParameterType = D3D12_ROOT_PARAMETER_TYPE_SRV;
    params[2].Descriptor.ShaderRegister = 1;
    D3D12_ROOT_SIGNATURE_DESC desc{3, params, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
    ComPtr<ID3DBlob> blob;
    check(D3D12SerializeRootSignature(&desc, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr), "serialize root");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(),
                                       IID_PPV_ARGS(&root)), "create root");
    return root;
}

static ComPtr<ID3D12Device> provider_device(bool xbox) {
    ComPtr<ID3D12Device> device;
    if (xbox) {
        const std::array<UINT, 18> profile{0x20004, 0, ~0u, 0, 0, 0, ~0u, ~0u, 0, 1};
        HMODULE module = LoadLibraryExW(L"d3d12_xs.dll", nullptr,
                                        LOAD_LIBRARY_SEARCH_APPLICATION_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32);
        using Factory = HRESULT(WINAPI*)(IUnknown*, const void*, REFIID, void**);
        auto factory = module ? reinterpret_cast<Factory>(GetProcAddress(module, "D3D12XboxCreateDevice")) : nullptr;
        if (!factory) throw std::runtime_error("missing Xbox device factory");
        check(factory(nullptr, profile.data(), IID_PPV_ARGS(&device)), "create Xbox device");
    } else {
        check(D3D12CreateDevice(nullptr, D3D_FEATURE_LEVEL_12_0, IID_PPV_ARGS(&device)), "create desktop device");
    }
    return device;
}

static void submit_closed(ID3D12Device* owner, ID3D12CommandQueue* queue, ID3D12GraphicsCommandList* list) {
    ID3D12CommandList* lists[] = {list};
    ComPtr<ID3D12Fence> fence;
    check(owner->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    queue->ExecuteCommandLists(1, lists);
    check(queue->Signal(fence.Get(), 1), "signal fence");
    HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    if (!event) throw std::runtime_error("create event");
    HRESULT hr = fence->SetEventOnCompletion(1, event);
    DWORD wait = SUCCEEDED(hr) ? WaitForSingleObject(event, 15000) : WAIT_FAILED;
    CloseHandle(event);
    check(hr, "fence event");
    if (wait != WAIT_OBJECT_0 || fence->GetCompletedValue() != 1)
        throw std::runtime_error("GPU fence did not complete");
}

static void submit(ID3D12Device* owner, ID3D12CommandQueue* queue, ID3D12GraphicsCommandList* list) {
    check(list->Close(), "close list");
    submit_closed(owner, queue, list);
}

static std::vector<char> read_file(const char* path) {
    std::ifstream in(path, std::ios::binary);
    if (!in) throw std::runtime_error(std::string("cannot open ") + path);
    return {std::istreambuf_iterator<char>{in}, std::istreambuf_iterator<char>{}};
}

static void same_adapter(ID3D12Device* owner, ID3D12Device* guest) {
    LUID guest_id{}, host_id = owner->GetAdapterLuid();
    using Adapter = LUID* (WINAPI*)(ID3D12Device*, LUID*);
    method<Adapter>(guest, 43)(guest, &guest_id);
    if (std::memcmp(&guest_id, &host_id, sizeof(host_id)))
        throw std::runtime_error("Xbox and desktop tests require the same adapter");
}

static void copy_structures(ID3D12Device* owner, UINT64 bytes,
                            std::array<ComPtr<ID3D12Resource>, 2>& structures) {
    const auto guest = provider_device(true);
    same_adapter(owner, guest.Get());
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    check(guest->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "Xbox copy queue");
    check(guest->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "Xbox copy allocator");
    const UINT params[]{0, 0, 1, 0};
    auto create = method<frame_abi::CreateList>(guest.Get(), 73);
    using Copy = void(WINAPI*)(ID3D12GraphicsCommandList4*, UINT64, UINT64,
                              D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
    ComPtr<ID3D12GraphicsCommandList> list;
    check(create(guest.Get(), params, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "Xbox copy list");
    ComPtr<ID3D12GraphicsCommandList4> list4;
    check(list.As(&list4), "Xbox List4 query");
    if (static_cast<void*>(list.Get()) != static_cast<void*>(list4.Get()))
        throw std::runtime_error("Xbox List4 identity");
    auto copy = method<Copy>(list4.Get(), 119);
    std::array<ComPtr<ID3D12Resource>, 2> destinations;
    for (unsigned i = 0; i < destinations.size(); ++i) {
        destinations[i] = buffer(owner, bytes, D3D12_HEAP_TYPE_DEFAULT,
            D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
        auto mode = i == 0 ? D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE :
                             D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_COMPACT;
        copy(list4.Get(), destinations[i]->GetGPUVirtualAddress(), structures[i]->GetGPUVirtualAddress(), mode);
    }
    D3D12_RESOURCE_BARRIER ready{};
    ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    list->ResourceBarrier(1, &ready);
    submit(guest.Get(), queue.Get(), list.Get());
    auto expect_error = [&](UINT64 dst, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE mode, HRESULT expected) {
        ComPtr<ID3D12GraphicsCommandList> invalid;
        check(create(guest.Get(), params, allocator.Get(), nullptr, IID_PPV_ARGS(&invalid)), "Xbox negative list");
        ComPtr<ID3D12GraphicsCommandList4> interface4;
        check(invalid.As(&interface4), "Xbox negative List4 query");
        copy(interface4.Get(), dst, structures[0]->GetGPUVirtualAddress(), mode);
        if (invalid->Close() != expected) throw std::runtime_error("incorrect Xbox AS copy rejection");
    };
    expect_error(destinations[0]->GetGPUVirtualAddress()+1,
                 D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE, E_INVALIDARG);
    expect_error(destinations[0]->GetGPUVirtualAddress(),
                 D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_VISUALIZATION_DECODE_FOR_TOOLS, E_NOTIMPL);
    structures.swap(destinations);
}

struct Entry {
    Build build;
    std::array<UINT64, 4> reserved;
};
static_assert(sizeof(Inputs) == 24 && sizeof(Sizes) == 24 && sizeof(Entry) == 80);

static ComPtr<ID3D12GraphicsCommandList> guest_list(ID3D12Device* guest, ID3D12CommandAllocator* allocator) {
    const UINT params[]{0, 0, 1, 0};
    ComPtr<ID3D12GraphicsCommandList> list;
    check(method<frame_abi::CreateList>(guest, 73)(guest, params, allocator, nullptr, IID_PPV_ARGS(&list)),
          "create Xbox build list");
    return list;
}

#ifdef TUXBOX_RAY_LIBRARY_PROBE
#include "ray_library_probe.h"
#endif
#ifdef TUXBOX_MATERIAL_HIT_PROBE
#include "material_probe.h"
#endif
#ifdef TUXBOX_GPU_INSTANCE_PROBE
#include "gpu_instances.h"
#endif
#ifdef TUXBOX_LIGHTING_PROBE
#include "lighting_probe.h"
#endif
#if defined(TUXBOX_INLINE434_REPLAY) || defined(TUXBOX_INLINE426_REPLAY)
#define TUXBOX_REFLECTION_PROBE
#endif
#if defined(TUXBOX_INLINE58_REPLAY) || defined(TUXBOX_INLINE114_REPLAY)
#define TUXBOX_SCREEN_GATHER_PROBE
#endif
#if defined(TUXBOX_INLINE318_REPLAY)
#define TUXBOX_INLINE_RADIANCE 318
#elif defined(TUXBOX_INLINE313_REPLAY) || defined(TUXBOX_INLINE306_REPLAY)
#define TUXBOX_INLINE_RADIANCE 306
#endif
#if defined(TUXBOX_REFLECTION_PROBE) || defined(TUXBOX_INLINE_RADIANCE) || defined(TUXBOX_INLINE219_REPLAY) || defined(TUXBOX_SCREEN_OCCLUSION_PROBE) || defined(TUXBOX_SCREEN_GATHER_PROBE) || defined(TUXBOX_CARD_TILE_PROBE)
#include "observed_ray_pipeline.h"
#endif
#ifdef TUXBOX_INLINE219_REPLAY
#include "inline219_replay.h"
#include "card_tile_probe.h"
#endif
#ifdef TUXBOX_CARD_TILE_PROBE
#include "card_candidate_dxil.h"
#include "zero_aabb_dxil.h"
#include "sidecar_bounds_dxil.h"
#include "sidecar_map_dxil.h"
#include "inline219_dxil.h"
#include "card_tile_probe.h"
#include "zero_aabb_probe.h"
#include "sphere_sidecar_probe.h"
#endif
#ifdef TUXBOX_SCREEN_GATHER_PROBE
#include "screen_gather_probe.h"
#endif
#if defined(TUXBOX_RADIANCE_PROBE) || defined(TUXBOX_INLINE_RADIANCE)
#include "radiance_probe.h"
#endif
#ifdef TUXBOX_SCREEN_OCCLUSION_PROBE
#include "occlusion_probe.h"
#endif
#ifdef TUXBOX_LUMEN_VOLUME_PROBE
#include "lumen_volume_probe.h"
#endif
#ifdef TUXBOX_REFLECTION_PROBE
#include "reflection_probe.h"
#endif

static void guest_batch(ID3D12GraphicsCommandList* list, UINT count, const Entry* entries,
                        [[maybe_unused]] UINT64 output = 0) {
#ifdef TUXBOX_SINGLE_BUILD_PROBE
    ComPtr<ID3D12GraphicsCommandList4> list4;
    check(list->QueryInterface(IID_PPV_ARGS(&list4)), "Xbox List4 query");
    using Single = void(WINAPI*)(ID3D12GraphicsCommandList4*, const Build*, UINT, const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*);
    for (UINT i = 0; i < count; ++i) {
        D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC sizes[] = {
            {output + i * sizeof(UINT64), D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_CURRENT_SIZE},
            {output + (count + i) * sizeof(UINT64), D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE},
        };
        method<Single>(list4.Get(), 117)(list4.Get(), &entries[i].build, output ? 2 : 0, output ? sizes : nullptr);
    }
#else
    const IID iid{0xc3827890, 0x4024, 0x474c, {0x98, 0xcf, 0x64, 0x5b, 0x51, 0xdc, 0x48, 0x74}};
    ComPtr<IUnknown> list6;
    check(list->QueryInterface(iid, reinterpret_cast<void**>(list6.GetAddressOf())), "Xbox List6 query");
    if (static_cast<void*>(list) != static_cast<void*>(list6.Get()))
        throw std::runtime_error("List6 did not preserve Xbox command list identity");
    using BatchBuild = void(WINAPI*)(IUnknown*, UINT, const Entry*);
    method<BatchBuild>(list6.Get(), 131)(list6.Get(), count, entries);
#endif
}

#ifdef TUXBOX_PROMOTED_BLAS_PROBE
#include "blas_promotion.h"
#endif

#ifdef TUXBOX_AMBIGUOUS_AS_PROBE
static void probe_ambiguous_as(ID3D12Device* guest, ID3D12CommandAllocator* allocator,
                               const Entry& valid, UINT64 ordinary, FILE* receipt) {
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_BUFFER;
    desc.Width = 131072;
    desc.Height = desc.DepthOrArraySize = desc.MipLevels = desc.SampleDesc.Count = 1;
    desc.Layout = D3D12_TEXTURE_LAYOUT_ROW_MAJOR;
    desc.Flags = D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS;
    D3D12_HEAP_DESC hd{};
    hd.SizeInBytes = 131072;
    hd.Properties.Type = D3D12_HEAP_TYPE_DEFAULT;
    hd.Properties.CreationNodeMask = hd.Properties.VisibleNodeMask = 1;
    hd.Flags = D3D12_HEAP_FLAG_ALLOW_ONLY_BUFFERS;
    ComPtr<ID3D12Heap> heap;
    check(guest->CreateHeap(&hd, IID_PPV_ARGS(&heap)), "ambiguous AS heap");
    ComPtr<ID3D12Resource> a, b;
    check(guest->CreatePlacedResource(heap.Get(), 0, &desc, D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
                                     nullptr, IID_PPV_ARGS(&a)), "first aliased AS buffer");
    check(guest->CreatePlacedResource(heap.Get(), 0, &desc, D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE,
                                     nullptr, IID_PPV_ARGS(&b)), "second aliased AS buffer");
    const UINT64 va = a->GetGPUVirtualAddress();
    if (!va || va != b->GetGPUVirtualAddress()) {
        std::fprintf(receipt, "ambiguous_as_equal_va=0\n");
        return;
    }
    auto buildList = guest_list(guest, allocator);
    auto ambiguous = valid;
    ambiguous.build.DestAccelerationStructureData = va;
    guest_batch(buildList.Get(), 1, &ambiguous);
    if (buildList->Close() != E_NOTIMPL) throw std::runtime_error("ambiguous AS build was accepted");
    auto updateList = guest_list(guest, allocator);
    auto update = valid;
    update.build.Inputs.Flags |= D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PERFORM_UPDATE;
    update.build.SourceAccelerationStructureData = va;
    guest_batch(updateList.Get(), 1, &update);
    if (updateList->Close() != E_NOTIMPL) throw std::runtime_error("ambiguous AS update source was accepted");
    using Copy = void(WINAPI*)(IUnknown*, UINT64, UINT64, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
    auto reject_copy = [&](UINT64 destination, UINT64 source) {
        auto list = guest_list(guest, allocator);
        method<Copy>(list.Get(), 119)(list.Get(), destination, source, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE);
        if (list->Close() != E_NOTIMPL) throw std::runtime_error("ambiguous AS copy was accepted");
    };
    reject_copy(ordinary, va);
    reject_copy(va, ordinary);
    b.Reset();
    auto unambiguous = guest_list(guest, allocator);
    guest_batch(unambiguous.Get(), 1, &ambiguous);
    check(unambiguous->Close(), "unique placed AS build after alias release");
    std::fprintf(receipt, "ambiguous_as_equal_va=1\nambiguous_as_rejections=4\nunique_as_after_release=1\n");
}
#endif


static void run(bool xbox, bool guest_build, bool guest_copy, bool deserialize, FILE* receipt, bool sphere) {
    const auto desktop = provider_device(false);
    const auto provider = xbox ? provider_device(true) : desktop;
    const auto root = root_signature(desktop.Get());
    ComPtr<ID3D12Device5> owner;
    check(desktop.As(&owner), "desktop DXR device");
    if (xbox) same_adapter(owner.Get(), provider.Get());
    ComPtr<IDXGIFactory4> factory;
    ComPtr<IDXGIAdapter1> adapter;
    DXGI_ADAPTER_DESC1 gpu{};
    check(CreateDXGIFactory1(IID_PPV_ARGS(&factory)), "DXGI factory");
    check(factory->EnumAdapterByLuid(owner->GetAdapterLuid(), IID_PPV_ARGS(&adapter)), "DXGI adapter");
    check(adapter->GetDesc1(&gpu), "GPU description");
    std::fprintf(receipt, "adapter=%04x:%04x\n", gpu.VendorId, gpu.DeviceId);
    D3D12_FEATURE_DATA_D3D12_OPTIONS5 options{};
    check(owner->CheckFeatureSupport(D3D12_FEATURE_D3D12_OPTIONS5, &options, sizeof(options)), "options5");
    std::fprintf(receipt, "raytracing_tier=%u\n", static_cast<unsigned>(options.RaytracingTier));
    if (options.RaytracingTier < D3D12_RAYTRACING_TIER_1_1) throw std::runtime_error("requires DXR 1.1");

    const float vertices[][3] = {{-1.5f,-0.5f,0},{-0.5f,-0.5f,0},{-1,0.5f,0},
                                 {0.5f,-0.5f,1},{1.5f,-0.5f,1},{1,0.5f,1}};
    constexpr UINT triangle_bytes = 9 * sizeof(float);
    auto packed = sphere ? read_file("sphere.xyz") : std::vector<char>{};
    if (sphere && packed.size() != 528 * triangle_bytes)
        throw std::runtime_error("unexpected sphere geometry size");
    auto mesh = sphere ? upload(owner.Get(), packed.data(), packed.size())
                       : upload(owner.Get(), vertices, sizeof(vertices));
    static_assert(sizeof(D3D12_RAYTRACING_GEOMETRY_DESC) == 56);
    std::array<D3D12_RAYTRACING_GEOMETRY_DESC, 2> geometries{};
    const UINT geometry_count = sphere ? 1u : 2u;
    for (UINT i = 0; i < geometry_count; ++i) {
        auto& g = geometries[i];
        g.Type = D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES;
        g.Flags = sphere ? D3D12_RAYTRACING_GEOMETRY_FLAG_NONE : D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE;
        g.Triangles.VertexFormat = DXGI_FORMAT_R32G32B32_FLOAT;
        g.Triangles.VertexCount = sphere ? static_cast<UINT>(packed.size() / sizeof(vertices[0])) : 3;
        g.Triangles.VertexBuffer.StartAddress = mesh->GetGPUVirtualAddress() + i * 3 * sizeof(vertices[0]);
        g.Triangles.VertexBuffer.StrideInBytes = sizeof(vertices[0]);
    }
    Inputs bottom{};
    bottom.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL;
    bottom.NumDescs = geometry_count;
    bottom.pGeometryDescs = geometries.data();
    bottom.Flags = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_PREFER_FAST_TRACE;
    if (guest_copy || guest_build || sphere) bottom.Flags |= D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION;
    Sizes native{};
    owner->GetRaytracingAccelerationStructurePrebuildInfo(&bottom, &native);
    if (!native.ResultDataMaxSizeInBytes || !native.ScratchDataSizeInBytes) throw std::runtime_error("empty prebuild");
    Sizes sizes = native;
    if (xbox) {
        struct { UINT64 before; Sizes sizes; UINT64 after; } guarded{1, {}, 2};
        using Prebuild = void(WINAPI*)(ID3D12Device*, const Inputs*, Sizes*);
        method<Prebuild>(provider.Get(), 102)(provider.Get(), &bottom, &guarded.sizes);
        if (guarded.before != 1 || guarded.after != 2 || std::memcmp(&native, &guarded.sizes, sizeof(native)))
            throw std::runtime_error("Xbox prebuild output differs from desktop or modified guard");
        sizes = guarded.sizes;
    }
    std::fprintf(receipt, "blas_result_bytes=%llu\nblas_scratch_bytes=%llu\n",
                 sizes.ResultDataMaxSizeInBytes, sizes.ScratchDataSizeInBytes);
    std::fflush(receipt);
    std::array<ComPtr<ID3D12Resource>, 2> structures, scratch;
    ComPtr<ID3D12Resource> inline_sizes;
    std::array<Entry, 2> batch{};
    for (size_t i = 0; i < batch.size(); ++i) {
        structures[i] = buffer(owner.Get(), sizes.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                               D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
        scratch[i] = buffer(owner.Get(), sizes.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                            D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        batch[i].build.Inputs = bottom;
        batch[i].build.DestAccelerationStructureData = structures[i]->GetGPUVirtualAddress();
        batch[i].build.ScratchAccelerationStructureData = scratch[i]->GetGPUVirtualAddress();
    }
    D3D12_COMMAND_QUEUE_DESC queue_desc{};
    ComPtr<ID3D12CommandQueue> queue;
    check(owner->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&queue)), "create verification queue");
    ComPtr<ID3D12CommandAllocator> allocator;
    check(owner->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&allocator)), "create verification allocator");
    ComPtr<ID3D12CommandQueue> guest_queue;
    ComPtr<ID3D12CommandAllocator> guest_allocator;
    D3D12_RESOURCE_BARRIER ready{};
    ready.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    if (guest_build) {
        check(provider->CreateCommandQueue(&queue_desc, IID_PPV_ARGS(&guest_queue)), "Xbox build queue");
        check(provider->CreateCommandAllocator(queue_desc.Type, IID_PPV_ARGS(&guest_allocator)), "Xbox build allocator");
#ifdef TUXBOX_AMBIGUOUS_AS_PROBE
        probe_ambiguous_as(provider.Get(), guest_allocator.Get(), batch[0], structures[0]->GetGPUVirtualAddress(), receipt);
#endif
        auto list = guest_list(provider.Get(), guest_allocator.Get());
        guest_batch(list.Get(), 0, nullptr);
        ComPtr<ID3D12Resource> cooked;
        std::vector<char> bytes;
        if (deserialize) {
            bytes = read_file("sphere.serialized");
            if (bytes.size() != 31296) throw std::runtime_error("unexpected serialized sphere size");
            // Swap one primitive from each of two leaves. Ray hits must use the
            // original IDs that the guest restores between Close and Execute.
            auto recorded = bytes;
            UINT leaf_offset{};
            std::memcpy(&leaf_offset, bytes.data() + 64 + 52, sizeof(leaf_offset));
            const size_t first_id = 64ull + leaf_offset + 52;
            if (first_id + 64 + sizeof(UINT) > bytes.size())
                throw std::runtime_error("bad sphere leaf table");
            auto id = recorded.data() + first_id;
            std::swap_ranges(id, id + sizeof(UINT), id + 64);
            cooked = upload(provider.Get(), recorded.data(), recorded.size());
            using Copy = void(WINAPI*)(IUnknown*, UINT64, UINT64, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
            for (auto& structure : structures) {
                structure = buffer(provider.Get(), 31232, D3D12_HEAP_TYPE_DEFAULT,
                    D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
                method<Copy>(list.Get(), 119)(list.Get(), structure->GetGPUVirtualAddress(),
                    cooked->GetGPUVirtualAddress(), D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_DESERIALIZE);
            }
        } else {
#ifdef TUXBOX_SINGLE_BUILD_PROBE
            inline_sizes = buffer(owner.Get(), 4 * sizeof(UINT64), D3D12_HEAP_TYPE_DEFAULT,
                                  D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
            guest_batch(list.Get(), static_cast<UINT>(batch.size()), batch.data(), inline_sizes->GetGPUVirtualAddress());
#else
            guest_batch(list.Get(), static_cast<UINT>(batch.size()), batch.data());
#endif
        }
        list->ResourceBarrier(1, &ready);
        check(list->Close(), "close guest build list");
        if (deserialize) {
            write_upload(cooked.Get(), bytes.data(), bytes.size());
            std::fprintf(receipt, "serialized_upload=updated_after_close\n");
        }
        submit_closed(provider.Get(), guest_queue.Get(), list.Get());
        auto invalid = guest_list(provider.Get(), guest_allocator.Get());
        auto bad_batch = batch;
        bad_batch[1].build.DestAccelerationStructureData |= 1;
        guest_batch(invalid.Get(), static_cast<UINT>(bad_batch.size()), bad_batch.data());
        if (invalid->Close() != E_INVALIDARG) throw std::runtime_error("unaligned batch accepted");
        auto extended = guest_list(provider.Get(), guest_allocator.Get());
#ifdef TUXBOX_SINGLE_BUILD_PROBE
        ComPtr<ID3D12GraphicsCommandList4> single;
        check(extended.As(&single), "Xbox negative List4 query");
        using Single = void(WINAPI*)(ID3D12GraphicsCommandList4*, const Build*, UINT, const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*);
        method<Single>(single.Get(), 117)(single.Get(), &batch[1].build, 1, nullptr);
        if (extended->Close() != E_INVALIDARG) throw std::runtime_error("null postbuild array accepted");
#else
        bad_batch[1] = batch[1];
        bad_batch[1].reserved[0] = 1;
        guest_batch(extended.Get(), static_cast<UINT>(bad_batch.size()), bad_batch.data());
        if (extended->Close() != E_NOTIMPL) throw std::runtime_error("unknown batch extension accepted");
#endif
    } else {
        ComPtr<ID3D12GraphicsCommandList4> list;
        check(owner->CreateCommandList(0, queue_desc.Type, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "create list");
        for (const auto& entry : batch)
            list->BuildRaytracingAccelerationStructure(&entry.build, 0, nullptr);
        list->ResourceBarrier(1, &ready);
        submit(owner.Get(), queue.Get(), list.Get());
    }

    auto original_blas = structures;
    if (guest_copy) {
        copy_structures(owner.Get(), sizes.ResultDataMaxSizeInBytes, structures);
        std::fprintf(receipt, "copy_modes=clone,compact\ninvalid_copies=2\n");
    }
#ifdef TUXBOX_SHADOW_TLAS_PROBE
    std::array<D3D12_RAYTRACING_INSTANCE_DESC, 3> instances{};
#else
    std::array<D3D12_RAYTRACING_INSTANCE_DESC, 2> instances{};
#endif
    for (UINT i = 0; i < instances.size(); ++i) {
        auto& instance = instances[i];
        for (UINT axis = 0; axis < 3; ++axis) instance.Transform[axis][axis] = 1;
        instance.Transform[0][3] = static_cast<float>(i) * (sphere ? 512.0f : 4.0f);
#if defined(TUXBOX_INLINE434_REPLAY) || defined(TUXBOX_INLINE219_REPLAY)
        instance.InstanceMask = i == 0 ? 0x20 : 0x01;
#elif defined(TUXBOX_INLINE313_REPLAY) || defined(TUXBOX_INLINE58_REPLAY)
        instance.InstanceMask = i == 0 ? 0x01 : 0x20;
#else
        instance.InstanceMask = 0xff;
#endif
        instance.InstanceID = 7 + i;
        instance.AccelerationStructure = structures[i % structures.size()]->GetGPUVirtualAddress();
    }
#ifdef TUXBOX_SHADOW_TLAS_PROBE
    // The second instance combines rotation, nonuniform scaling and shear.
    // The third remains present in the TLAS with an empty visibility mask.
    instances[1].Transform[0][0] = 0;
    instances[1].Transform[0][1] = -2;
    instances[1].Transform[0][2] = .5f;
    instances[1].Transform[1][0] = .5f;
    instances[1].Transform[1][1] = 0;
    instances[1].Transform[1][2] = .25f;
    instances[1].Transform[2][2] = 1.5f;
    instances[2].InstanceMask = 0;
#endif
    ComPtr<ID3D12Resource> instance_buffer;
    UINT instance_count = static_cast<UINT>(instances.size());
#ifdef TUXBOX_GPU_INSTANCE_PROBE
#ifdef TUXBOX_SHADOW_TLAS_PROBE
    auto gpu_instances = instances;
#else
    std::array<D3D12_RAYTRACING_INSTANCE_DESC, 3> gpu_instances{instances[0], instances[1], instances[0]};
#endif
    ComPtr<ID3D12Resource> tiny;
    if (guest_build && !sphere) {
#ifdef TUXBOX_PROMOTED_BLAS_PROBE
        constexpr UINT pool_bytes = 2048;
#elif defined(TUXBOX_REUSED_BLAS_PROBE)
        constexpr UINT pool_bytes = 704;
#else
        constexpr UINT pool_bytes = 192;
#endif
        tiny = buffer(provider.Get(), pool_bytes, D3D12_HEAP_TYPE_DEFAULT,
                      D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
        gpu_instances[2].AccelerationStructure = tiny->GetGPUVirtualAddress();
        gpu_instances[2].InstanceID = 11;
        gpu_instances[2].Transform[0][3] = 10.0f;
        ++instance_count;
    }
    instance_buffer = upload(guest_build ? provider.Get() : desktop.Get(), gpu_instances.data(), sizeof(gpu_instances));
#else
    instance_buffer = upload(guest_build ? provider.Get() : desktop.Get(), instances.data(), sizeof(instances));
#endif
    Inputs top{};
    top.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_TOP_LEVEL;
    top.NumDescs = instance_count;
    top.InstanceDescs = instance_buffer->GetGPUVirtualAddress();
    Sizes top_sizes{};
    owner->GetRaytracingAccelerationStructurePrebuildInfo(&top, &top_sizes);
    auto scene = buffer(owner.Get(), top_sizes.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                        D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
    auto scene_scratch = buffer(owner.Get(), top_sizes.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                                D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    D3D12_COMPUTE_PIPELINE_STATE_DESC pso_desc{};
    pso_desc.pRootSignature = root.Get();
    pso_desc.CS = {query_dxil, sizeof(query_dxil)};
    ComPtr<ID3D12PipelineState> pso;
    check(owner->CreateComputePipelineState(&pso_desc, IID_PPV_ARGS(&pso)), "create rayquery pipeline");
    check(allocator->Reset(), "reset completed allocator");
    ComPtr<ID3D12GraphicsCommandList4> verify;
    check(owner->CreateCommandList(0, queue_desc.Type, allocator.Get(), pso.Get(), IID_PPV_ARGS(&verify)),
          "create verification list");
    ComPtr<ID3D12Resource> size_readback, size_output;
    if (sphere || guest_build) {
        constexpr UINT bytes = 4 * sizeof(UINT64);
        size_output = buffer(owner.Get(), bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        size_readback = buffer(owner.Get(), inline_sizes ? 2 * bytes : bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
        const UINT64 sources[] = {original_blas[0]->GetGPUVirtualAddress(), original_blas[1]->GetGPUVirtualAddress()};
        const auto dst = size_output->GetGPUVirtualAddress();
        ComPtr<ID3D12GraphicsCommandList> query;
        if (guest_build) query = guest_list(provider.Get(), guest_allocator.Get());
        using SizeQuery = void(WINAPI*)(IUnknown*, const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*,
                                        UINT, const UINT64*);
        auto sizes_for = [&](auto type, UINT64 address) {
            D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC info{address, type};
            if (query) method<SizeQuery>(query.Get(), 118)(query.Get(), &info, ARRAYSIZE(sources), sources);
            else verify->EmitRaytracingAccelerationStructurePostbuildInfo(&info, ARRAYSIZE(sources), sources);
        };
        sizes_for(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_CURRENT_SIZE, dst);
        sizes_for(D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE, dst + 2 * sizeof(UINT64));
        if (query) {
            submit(provider.Get(), guest_queue.Get(), query.Get());
            auto invalid = guest_list(provider.Get(), guest_allocator.Get());
            const UINT64 unaligned[] = {sources[0], sources[1] + 4};
            D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC info{
                dst, D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE};
            method<SizeQuery>(invalid.Get(), 118)(invalid.Get(), &info, ARRAYSIZE(unaligned), unaligned);
            if (invalid->Close() != E_INVALIDARG) throw std::runtime_error("unaligned postbuild source accepted");
        }
        D3D12_RESOURCE_BARRIER to_copy{};
        to_copy.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        to_copy.Transition = {size_output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
        verify->ResourceBarrier(1, &to_copy);
        verify->CopyBufferRegion(size_readback.Get(), 0, size_output.Get(), 0, bytes);
        if (inline_sizes) {
            to_copy.Transition.pResource = inline_sizes.Get();
            verify->ResourceBarrier(1, &to_copy);
            verify->CopyBufferRegion(size_readback.Get(), bytes, inline_sizes.Get(), 0, bytes);
        }
    }
    Build tlas{};
    tlas.Inputs = top;
    tlas.DestAccelerationStructureData = scene->GetGPUVirtualAddress();
    tlas.ScratchAccelerationStructureData = scene_scratch->GetGPUVirtualAddress();
    if (guest_build) {
        auto list = guest_list(provider.Get(), guest_allocator.Get());
#ifdef TUXBOX_GPU_INSTANCE_PROBE
        InstanceBindings bindings;
        bindings.bind(provider.Get(), list.Get());
        auto initial = gpu_instances;
        for (auto& i : initial) i.InstanceMask = 0;
        write_upload(instance_buffer.Get(), initial.data(), sizeof(initial));
        constexpr UINT offset = 32;
        auto gpu_records = buffer(provider.Get(), offset + sizeof(gpu_instances), D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_COPY_DEST, true);
        list->CopyBufferRegion(gpu_records.Get(), offset, instance_buffer.Get(), 0, sizeof(gpu_instances));
        D3D12_RESOURCE_BARRIER readable{};
        readable.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        readable.Transition = {gpu_records.Get(), 0, D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE};
        list->ResourceBarrier(1, &readable);
        tlas.Inputs.InstanceDescs = gpu_records->GetGPUVirtualAddress() + offset;
#endif
        Entry entry{tlas, {}};
        guest_batch(list.Get(), 1, &entry);
        list->ResourceBarrier(1, &ready);
#ifdef TUXBOX_GPU_INSTANCE_PROBE
        bindings.dispatch(list.Get());
        check(list->Close(), "close scene list");
#ifndef TUXBOX_PROMOTED_BLAS_PROBE
        if (tiny) {
            const auto quad = packed_quad();
            const auto native_size = publish_quad(provider.Get(), guest_queue.Get(), guest_allocator.Get(), tiny.Get(), {quad.begin(), quad.end()});
            if (native_size <= 192) throw std::runtime_error("quad fits guest destination on this adapter, missing relocation coverage");
            std::fprintf(receipt, "quad_guest_bytes=192\nquad_native_bytes=%llu\n", native_size);
        }
#endif
        auto short_list = guest_list(provider.Get(), guest_allocator.Get());
        auto outside = entry;
        outside.build.Inputs.InstanceDescs += 2 * sizeof(D3D12_RAYTRACING_INSTANCE_DESC);
        guest_batch(short_list.Get(), 1, &outside);
        if (short_list->Close() != E_INVALIDARG) throw std::runtime_error("partially unmapped GPU instance array accepted");
        write_upload(instance_buffer.Get(), gpu_instances.data(), sizeof(gpu_instances));
#ifdef TUXBOX_PROMOTED_BLAS_PROBE
        check_blas_promotion(provider.Get(), guest_queue.Get(), guest_allocator.Get(), tiny.Get(), entry, gpu_records.Get(),
                             offset, instance_buffer.Get(), sizeof(gpu_instances), receipt);
#endif
        submit_closed(provider.Get(), guest_queue.Get(), list.Get());
        bindings.check_result();
        submit_closed(provider.Get(), guest_queue.Get(), list.Get());
        bindings.check_result();
        UINT executions = 2;
#ifdef TUXBOX_REUSED_BLAS_PROBE
        if (tiny) {
            check_blas_reuse(provider.Get(), guest_queue.Get(), guest_allocator.Get(), tiny.Get(), receipt);
            // Rebuild the TLAS after all modifications to the referenced BLAS.
            submit_closed(provider.Get(), guest_queue.Get(), list.Get());
            ++executions;
        }
#endif
        std::fprintf(receipt, "restored_compute_bindings=17,23,37,53\nclosed_list_executions=%u\n", executions);
        std::fprintf(receipt, "instance_memory=default_heap_gpu_copy_after_close\n");
#else
        submit(provider.Get(), guest_queue.Get(), list.Get());
#endif
        std::fprintf(receipt, "guest_tlas_instances=%u\ninvalid_batches=2\n", instance_count);
    } else {
        verify->BuildRaytracingAccelerationStructure(&tlas, 0, nullptr);
    }
    ready.UAV.pResource = scene.Get();
    verify->ResourceBarrier(1, &ready);
    struct Probe { float origin[3], minimum, direction[3], maximum; UINT flags; };
    static_assert(sizeof(Probe) == 36);
    const Probe original[] = {
        {{-1, 0, -1}, 0, {0, 0, 1}, 10, 0}, {{1, 0, -1}, 0, {0, 0, 1}, 10, 0},
        {{3, 0, -1}, 0, {0, 0, 1}, 10, 0}, {{5, 0, -1}, 0, {0, 0, 1}, 10, 0},
        {{7, 0, -1}, 0, {0, 0, 1}, 10, 0},
#ifdef TUXBOX_GPU_INSTANCE_PROBE
        {{10.25f, .25f, -1}, 0, {0, 0, 1}, 10, 0}, {{10.75f, .75f, -1}, 0, {0, 0, 1}, 10, 0},
        {{11.5f, .5f, -1}, 0, {0, 0, 1}, 10, 0},
#endif
    };
    const auto probes = sphere ? read_file("sphere-rays.bin") : std::vector<char>{};
    if (sphere && (probes.empty() || probes.size() % sizeof(original[0])
                   || probes.size() / sizeof(original[0]) > D3D12_CS_DISPATCH_MAX_THREAD_GROUPS_PER_DIMENSION))
        throw std::runtime_error("invalid ray probe input");
    const UINT ray_count = sphere ? static_cast<UINT>(probes.size() / sizeof(original[0])) : static_cast<UINT>(std::size(original));
    auto ray_buffer = sphere ? upload(owner.Get(), probes.data(), probes.size())
                             : upload(owner.Get(), original, sizeof(original));
    const size_t bytes = 4 * sizeof(UINT) * ray_count;
    auto output = buffer(owner.Get(), bytes, D3D12_HEAP_TYPE_DEFAULT, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
    auto readback = buffer(owner.Get(), bytes, D3D12_HEAP_TYPE_READBACK, D3D12_RESOURCE_STATE_COPY_DEST);
    verify->SetComputeRootSignature(root.Get());
    verify->SetComputeRootShaderResourceView(0, scene->GetGPUVirtualAddress());
    verify->SetComputeRootUnorderedAccessView(1, output->GetGPUVirtualAddress());
    verify->SetComputeRootShaderResourceView(2, ray_buffer->GetGPUVirtualAddress());
    verify->Dispatch(ray_count, 1, 1);
    D3D12_RESOURCE_BARRIER copy{};
    copy.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
    copy.Transition = {output.Get(), 0, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE};
    verify->ResourceBarrier(1, &copy);
    verify->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, bytes);
    submit(owner.Get(), queue.Get(), verify.Get());
    D3D12_RANGE read{0, bytes}, empty{};
    void* data{};
    check(readback->Map(0, &read, &data), "map ray query results");
    std::vector<std::array<UINT, 4>> hits(ray_count);
    std::memcpy(hits.data(), data, bytes);
    readback->Unmap(0, &empty);
    if (size_readback) {
        const D3D12_RANGE range{0, (inline_sizes ? 8u : 4u) * sizeof(UINT64)};
        check(size_readback->Map(0, &range, &data), "map BLAS sizes");
        std::array<UINT64, 4> measured{};
        std::memcpy(measured.data(), data, sizeof(measured));
        const bool match = !inline_sizes || !std::memcmp(measured.data(), static_cast<UINT64*>(data) + 4, sizeof(measured));
        size_readback->Unmap(0, &empty);
        if (!match) throw std::runtime_error("inline size output differs from explicit size queries");
        if (inline_sizes) std::fprintf(receipt, "inline_postbuild_sizes_matched=4\n");
        std::fprintf(receipt, "blas_current_bytes=%llu\nblas_compacted_bytes=%llu\n", measured[0], measured[2]);
        if (!measured[0] || !measured[2] || measured[2] > sizes.ResultDataMaxSizeInBytes
                || measured[0] != measured[1] || measured[2] != measured[3])
            throw std::runtime_error("invalid postbuild size array");
        if (guest_build) std::fprintf(receipt, "guest_postbuild_source_count=2\n");
    }
    if (sphere) {
        std::ofstream file("sphere-hits.bin", std::ios::binary);
        file.write(reinterpret_cast<const char*>(hits.data()), bytes);
        file.close();
        if (!file) throw std::runtime_error("write sphere ray hits");
        std::fprintf(receipt, "rays=%u\n", ray_count);
        return;
    }
#ifdef TUXBOX_GPU_INSTANCE_PROBE
    if (guest_build) {
#if defined(TUXBOX_REUSED_BLAS_PROBE) || defined(TUXBOX_PROMOTED_BLAS_PROBE)
        constexpr UINT first = 0, second = 1;
#else
        constexpr UINT first = 1, second = 0;
#endif
        if (hits[5] != std::array<UINT,4>{11,0,first,0x3f800000} ||
            hits[6] != std::array<UINT,4>{11,0,second,0x3f800000} || hits[7][0] != ~0u)
            throw std::runtime_error("late relocated instance rays differ");
        std::fprintf(receipt, "late_relocation_rays=11/0/%u/1,11/0/%u/1,miss\n", first, second);
    }
#endif
    std::array<UINT, 5> values{};
    for (size_t i = 0; i < values.size(); ++i) {
        const auto& hit = hits[i];
        if (hit[0] == ~0u) { values[i] = 0xdead; continue; }
        float distance;
        std::memcpy(&distance, &hit[3], sizeof(float));
        if (hit[2] != 0 || !(distance >= 0 && distance <= 10)) throw std::runtime_error("unexpected triangle hit");
        values[i] = (hit[0] << 16) | (hit[1] << 8) | static_cast<UINT>(std::round(16 * distance));
    }
    std::fprintf(receipt, "ray_query_results=%x,%x,%x,%x,%x\n",
                 values[0], values[1], values[2], values[3], values[4]);
    if (values != std::array<UINT, 5>{0x70010, 0x70120, 0x80010, 0x80120, 0xdead})
        throw std::runtime_error("ray queries differ");
#ifdef TUXBOX_RAY_LIBRARY_PROBE
    for (bool indirect : {false, true})
        probe_ray_libraries(owner.Get(), queue.Get(), scene.Get(), receipt, xbox ? provider.Get() : nullptr, indirect);
#endif
#ifdef TUXBOX_MATERIAL_HIT_PROBE
    probe_materials(owner.Get(), queue.Get(), scene.Get(), instances, receipt, xbox ? provider.Get() : nullptr);
#endif
#ifdef TUXBOX_INLINE_RADIANCE
    for (auto input : {RadianceInput::empty, RadianceInput::tile, RadianceInput::geometry})
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, input, TUXBOX_INLINE_RADIANCE == 306 ? 299 : 323);
#ifdef TUXBOX_INLINE313_REPLAY
    probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::far_geometry, 299);
#endif
#endif
#ifdef TUXBOX_INLINE219_REPLAY
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,false);
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,false,8.f,8.f,1,20.f,-1.f);
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,true,0.f,2.f,2,-1.f,-1.f);
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,true,2.f,4.f,3,3.f,-1.f);
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,false,2.f,4.f,4,-1.f,-1.f);
    probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,false,0.f,2.f,5,3.f,-1.f);
#endif
#ifdef TUXBOX_CARD_TILE_PROBE
    if (!xbox) {
        probe_zero_aabb(owner.Get(), queue.Get(), receipt);
        probe_sphere_sidecar(owner.Get(), queue.Get(), receipt);
        for (UINT entry : {218u,1064u}) for (bool active : {false,true})
            probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,entry,active);
        probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,1064,true,true);
        probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,true);
        const float bounds[][4]{{.5f,2.f,-1.f,-1.f}, {1.f,2.f,-1.f,-1.f}, {1.25f,2.f,-1.f,-1.f},
                                {2.f,3.f,-1.f,-1.f}, {0.f,.5f,-1.f,-1.f}, {0.f,1.f,-1.f,-1.f},
                                {0.f,2.f,-1.f,-1.f}, {0.f,2.f,20.f,-1.f}, {1.25f,2.f,20.f,-1.f},
                                {0.f,2.f,-1.f,0.f}};
        for (UINT i = 0; i < std::size(bounds); ++i)
            probe_card_tile(owner.Get(),queue.Get(),scene.Get(),receipt,218,true,false,true,true,bounds[i][0],bounds[i][1],i+1,bounds[i][2],bounds[i][3]);
        // Align a second instance's z=1 triangle with the first instance's
        // z=0 triangle, giving the test ray candidates at distances 1 and 2.
        auto stacked_instances = instances;
        stacked_instances[1].Transform[0][3] = -2.f;
        auto stacked_upload = upload(owner.Get(), stacked_instances.data(), sizeof(stacked_instances));
        auto stacked_scene = buffer(owner.Get(), top_sizes.ResultDataMaxSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                                    D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE, true);
        auto stacked_scratch = buffer(owner.Get(), top_sizes.ScratchDataSizeInBytes, D3D12_HEAP_TYPE_DEFAULT,
                                      D3D12_RESOURCE_STATE_UNORDERED_ACCESS, true);
        ComPtr<ID3D12CommandAllocator> stacked_allocator;
        ComPtr<ID3D12GraphicsCommandList4> stacked_list;
        check(owner->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&stacked_allocator)), "stacked allocator");
        check(owner->CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, stacked_allocator.Get(), nullptr, IID_PPV_ARGS(&stacked_list)), "stacked list");
        Build stacked = tlas;
        stacked.Inputs.InstanceDescs = stacked_upload->GetGPUVirtualAddress();
        stacked.DestAccelerationStructureData = stacked_scene->GetGPUVirtualAddress();
        stacked.ScratchAccelerationStructureData = stacked_scratch->GetGPUVirtualAddress();
        stacked_list->BuildRaytracingAccelerationStructure(&stacked, 0, nullptr);
        stacked_list->ResourceBarrier(1, &ready);
        submit(owner.Get(), queue.Get(), stacked_list.Get());
        probe_card_tile(owner.Get(),queue.Get(),stacked_scene.Get(),receipt,218,true,false,true,true,1.25f,3.f,11,-1.f,-1.f);
    }
#endif
#if defined(TUXBOX_INLINE58_REPLAY) || defined(TUXBOX_INLINE114_REPLAY)
#ifdef TUXBOX_INLINE58_REPLAY
    constexpr UINT tested = 58, control = 64;
#else
    constexpr UINT tested = 114, control = 71;
#endif
    const auto shader_name = std::to_string(tested) + ".dxbc";
    InlineSceneFixture forward(shader_name.c_str(), tested == 58 ? 1 : 0xff, tested == 58 ? 0x20 : 0xff, true);
    std::optional<InlineSceneFixture> reversed;
    for (UINT scenario = 0; scenario < (tested == 114 ? 5u : 2u); ++scenario) {
        if (scenario == 3) {
            reversed.emplace(shader_name.c_str(), 0xff, 0xff, true, true);
            // Reverse exactly the same triangle as the guest on the independent native scene.
            float positions[6][3];
            std::memcpy(positions, vertices, sizeof(positions));
            std::swap(positions[1], positions[2]);
            std::swap(positions[4], positions[5]);
            write_upload(mesh.Get(), positions, sizeof(positions));
            ComPtr<ID3D12GraphicsCommandList4> rebuild;
            check(owner->CreateCommandList(0, queue_desc.Type, allocator.Get(), nullptr, IID_PPV_ARGS(&rebuild)),
                  "rebuild reversed screen fixture");
            for (auto& e : batch)
                rebuild->BuildRaytracingAccelerationStructure(&e.build, 0, nullptr);
            ready.UAV.pResource = nullptr;
            rebuild->ResourceBarrier(1, &ready);
            rebuild->BuildRaytracingAccelerationStructure(&tlas, 0, nullptr);
            rebuild->ResourceBarrier(1, &ready);
            submit(owner.Get(), queue.Get(), rebuild.Get());
        }
        auto reference = probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, control, scenario != 0,
                                             scenario >= 2, nullptr, nullptr, scenario);
        probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, tested, scenario != 0, scenario >= 2,
                            reversed ? &*reversed : &forward, &reference, scenario);
    }
#ifdef TUXBOX_INLINE58_REPLAY
    const auto original114 = read_file("114.dxbc");
    D3D12_COMPUTE_PIPELINE_STATE_DESC desc114{};
    desc114.pRootSignature = forward.root.Get();
    desc114.CS = {original114.data(), original114.size()};
    using CreateScreen =
        HRESULT(WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*, UINT, const void*, REFIID, void**);
    ComPtr<ID3D12PipelineState> producer;
    check(method<CreateScreen>(forward.dev.Get(), 62)(forward.dev.Get(), &desc114, 0, nullptr, IID_PPV_ARGS(&producer)),
          "near-field producer pipeline");
    auto consumer = forward.pipeline;
    for (UINT scenario : {5u, 6u}) {
        auto reference_produced = probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, 71, true, true,
                                                      nullptr, nullptr, scenario);
        forward.pipeline = producer;
        auto produced = probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, 114, true, true, &forward,
                                            &reference_produced, scenario);
        auto reference = probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, 64, true, true, nullptr,
                                             nullptr, scenario, &reference_produced);
        forward.pipeline = consumer;
        probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, 58, true, true, &forward, &reference,
                            scenario, &produced);
    }
#endif
#elif defined(TUXBOX_SCREEN_GATHER_PROBE)
    if (!xbox)
        for (UINT entry : {64u, 71u}) {
            probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, entry, false, false);
            probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, entry, true, false);
            probe_screen_gather(owner.Get(), queue.Get(), scene.Get(), receipt, entry, true, true);
        }
#endif
#if defined(TUXBOX_INLINE434_REPLAY) || defined(TUXBOX_INLINE426_REPLAY)
    {
#ifdef TUXBOX_INLINE434_REPLAY
        constexpr UINT compute = 434, control = 432;
#else
        constexpr UINT compute = 426, control = 421;
#endif
        const auto original = std::to_string(compute) + ".dxbc";
        InlineSceneFixture fixture(original.c_str(), compute == 434 ? 0x20 : 0xff, compute == 434 ? 0x01 : 0xff, true);
        for (UINT scenario = 0; scenario < (compute == 434 ? 3u : 2u); ++scenario) {
            bool geometry = scenario != 0, masked = scenario == 2;
            auto reference = probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, control, geometry,
                                               nullptr, nullptr, masked);
            probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, compute, geometry, &fixture, &reference,
                              masked);
        }
    }
#endif
#ifdef TUXBOX_LIGHTING_PROBE
    if (!xbox) probe_lighting(owner.Get(), queue.Get(), scene.Get(), receipt);
#endif
#ifdef TUXBOX_SCREEN_OCCLUSION_PROBE
    if (!xbox) {
#ifdef TUXBOX_RADIANCE_PROBE
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::empty);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::tile);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::geometry);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::tile, 299);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::tile, 311);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::geometry, 299);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::geometry, 311);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::tile, 264);
        probe_radiance_cache(owner.Get(), queue.Get(), scene.Get(), receipt, RadianceInput::tile, 266);
#endif
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3801);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3801, ShadowInput::scene_masked);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3824);
#ifdef TUXBOX_LUMEN_VOLUME_PROBE
        probe_lumen_volume(owner.Get(), queue.Get(), scene.Get(), receipt, 619);
        probe_lumen_volume(owner.Get(), queue.Get(), scene.Get(), receipt, 620);
#endif
#ifdef TUXBOX_REFLECTION_PROBE
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 467);
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 425);
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 456);
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 421);
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 432);
        probe_reflections(owner.Get(), queue.Get(), scene.Get(), receipt, 456, true);
#endif
#ifdef TUXBOX_VOXEL_OCCLUSION_PROBE
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3810, ShadowInput::absent_page);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3810, ShadowInput::empty_page);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3810, ShadowInput::occupied_page);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3810, ShadowInput::hair_masked);
        probe_screen_occlusion(owner.Get(), queue.Get(), scene.Get(), receipt, 3810, ShadowInput::hair_lit);
#endif
    }
#endif
}

int wmain(int argc, wchar_t** argv) {
    if ((argc != 3 && (argc != 4 || wcscmp(argv[3], L"sphere"))) || (wcscmp(argv[1], L"desktop") && wcscmp(argv[1], L"xbox") &&
                      wcscmp(argv[1], L"xbox-prebuild") && wcscmp(argv[1], L"xbox-copy") && wcscmp(argv[1], L"xbox-deserialize"))) return 2;
    FILE* receipt{};
    if (_wfopen_s(&receipt, argv[2], L"wb")) return 2;
    int status = 0;
    try {
        const bool deserialize = wcscmp(argv[1], L"xbox-deserialize") == 0;
        const bool build = !wcscmp(argv[1], L"xbox") || deserialize;
        run(build || wcscmp(argv[1], L"xbox-prebuild") == 0, build,
            wcscmp(argv[1], L"xbox-copy") == 0, deserialize, receipt, argc == 4);
    } catch (const std::exception& e) {
        std::fprintf(receipt, "error=%s\n", e.what());
        status = 1;
    }
    return fclose(receipt) ? 2 : status;
}
