#include "selftest.h"
#include "game.h"
#include "gpu.h"
#include "screen.h"
#include "../../runtime/d3d12/compute_fixture.h"
#include <cstdint>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iterator>
#include <vector>
#include "course_shader.h"
#include "xbox_shader.h"
#include "reclaim_shader.h"
#include <algorithm>
#include <memory>

namespace {
using compute_fixture::buffer;
using frame_abi::check;
using frame_abi::ComPtr;
using frame_abi::method;

// The Scarlett release record with ID3D12Device11 selects the queue and
// command-list tables a Series X|S title built with a recent GDK uses.
constexpr std::array<UINT, 18> scarlett_profile{
    0x20004, 0, UINT_MAX, 0, 0, 0, UINT_MAX, UINT_MAX, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0};

// A queue held on a label is released by the provider after five seconds, so a
// scenario that needs that release must already count as failed.
constexpr DWORD deadline_ms = 2000;

// Xbox command-list and queue slots (GDK gen9/d3d12_xs.h).
constexpr size_t slot_copy_memory = 62, slot_write32 = 71, slot_write64 = 72, slot_wait64 = 74,
                 slot_dispatch_flagged = 80, slot_copy_texture = 83, slot_build = 119,
                 slot_emit_sizes = 120, slot_sync_top = 131, slot_sync_bottom = 132;
constexpr size_t slot_compute_root = 33, device_create_compute = 62, device_create_stream = 86;
constexpr size_t slot_bind_heaps = 32, slot_compute_table = 35, queue_wait = 17;
constexpr size_t slot_execute_indirect = 61, slot_root_constants = 39, slot_root_uav = 45;
constexpr size_t queue_signal = 16, queue_execute_frame = 35, device_place = 44, device_create_list = 73;

using CopyMemory_ = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, UINT64, UINT64, UINT);
using Write32 = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, UINT, UINT, UINT);
using Write64 = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, UINT64, UINT, UINT);
using Wait64 = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT64, D3D12_COMPARISON_FUNC, UINT64, UINT);
using DispatchFlagged = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, UINT, UINT);
using Build = void (WINAPI*)(ID3D12GraphicsCommandList*, const void*, UINT, const void*);
using Sync = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, UINT64, UINT64);
using SignalQueue = HRESULT (WINAPI*)(ID3D12CommandQueue*, ID3D12Fence*, UINT64);
using ExecuteFrame = void (WINAPI*)(ID3D12CommandQueue*, UINT, ID3D12CommandList* const*, UINT64, UINT);
using CreateComputePipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_COMPUTE_PIPELINE_STATE_DESC*,
                                                UINT, const void*, REFIID, void**);
using CreateStreamPipeline = HRESULT (WINAPI*)(ID3D12Device*, const D3D12_PIPELINE_STATE_STREAM_DESC*,
                                               REFIID, void**);
using SetRoot = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12RootSignature*);
using ExecuteIndirect_ = void (WINAPI*)(ID3D12GraphicsCommandList*, ID3D12CommandSignature*, UINT,
                                         ID3D12Resource*, UINT64, ID3D12Resource*, UINT64);
using SetConstants = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, UINT, const void*, UINT);
using SetUav = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_VIRTUAL_ADDRESS);
using BindHeaps = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, ID3D12DescriptorHeap* const*);
using SetTable = void (WINAPI*)(ID3D12GraphicsCommandList*, UINT, D3D12_GPU_DESCRIPTOR_HANDLE);
using WaitQueue = HRESULT (WINAPI*)(ID3D12CommandQueue*, ID3D12Fence*, UINT64);
using GpuHandle = UINT64 (WINAPI*)(ID3D12DescriptorHeap*);
using EmitSizes = void (WINAPI*)(ID3D12GraphicsCommandList*,
                                 const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*, UINT,
                                 const D3D12_GPU_VIRTUAL_ADDRESS*);
using Place = HRESULT (WINAPI*)(ID3D12Device*, UINT64, const D3D12_RESOURCE_DESC*, D3D12_RESOURCE_STATES,
                                const D3D12_CLEAR_VALUE*, REFIID, void**);

// Committed title memory. A Series title addresses the GPU with these pointers.
struct Memory {
    BYTE* base;
    explicit Memory(size_t bytes)
        : base(static_cast<BYTE*>(VirtualAlloc(nullptr, bytes, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE))) {
        if (!base) throw std::runtime_error("allocate title memory");
    }
    ~Memory() { VirtualFree(base, 0, MEM_RELEASE); }
    Memory(const Memory&) = delete;
    Memory& operator=(const Memory&) = delete;
    UINT64 address(size_t offset = 0) const { return reinterpret_cast<UINT64>(base + offset); }
    UINT64 read64(size_t offset) const { return *reinterpret_cast<volatile const UINT64*>(base + offset); }
};

struct Queue {
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Fence> fence;
    UINT64 submitted = 0;

    Queue(ID3D12Device* device, D3D12_COMMAND_LIST_TYPE type) {
        D3D12_COMMAND_QUEUE_DESC desc{};
        desc.Type = type;
        check(device->CreateCommandQueue(&desc, IID_PPV_ARGS(&queue)), "create queue");
        check(device->CreateCommandAllocator(type, IID_PPV_ARGS(&allocator)), "create allocator");
        const UINT record[] = {UINT(type), 0, 1, 0};
        check(method<frame_abi::CreateList>(device, device_create_list)(
                  device, record, allocator.Get(), nullptr, IID_PPV_ARGS(&list)), "create list");
        check(list->Reset(allocator.Get(), nullptr), "open list");
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "create fence");
    }
    void write32(UINT64 address, UINT value, UINT flush = 0) {
        method<Write32>(list.Get(), slot_write32)(list.Get(), address, value, flush, 0);
    }
    void write64(UINT64 address, UINT64 value, UINT flush = 0) {
        method<Write64>(list.Get(), slot_write64)(list.Get(), address, value, flush, 0);
    }
    void wait64(UINT64 address, UINT64 value) {
        method<Wait64>(list.Get(), slot_wait64)(list.Get(), address, D3D12_COMPARISON_FUNC_GREATER_EQUAL, value, 0);
    }
    void close() { check(list->Close(), "close list"); }
    // Reuse the allocator and list once everything submitted has finished.
    void reopen() {
        check(allocator->Reset(), "reset allocator");
        check(list->Reset(allocator.Get(), nullptr), "reopen list");
    }
    void signal() {
        check(method<SignalQueue>(queue.Get(), queue_signal)(queue.Get(), fence.Get(), ++submitted), "signal queue");
    }
    void submit() {
        close();
        ID3D12CommandList* batch[]{list.Get()};
        queue->ExecuteCommandLists(1, batch);
        signal();
    }
    // Wait until everything submitted has finished, or fail.
    void finish(const char* what) {
        HANDLE event = CreateEventW(nullptr, FALSE, FALSE, nullptr);
        if (!event) throw std::runtime_error("create completion event");
        const HRESULT armed = fence->SetEventOnCompletion(submitted, event);
        const DWORD waited = SUCCEEDED(armed) ? WaitForSingleObject(event, deadline_ms) : WAIT_FAILED;
        CloseHandle(event);
        check(armed, "arm completion event");
        if (waited != WAIT_OBJECT_0)
            throw std::runtime_error(std::string(what) + " did not finish within 2 seconds");
    }
};

void expect_value(const Memory& memory, size_t offset, UINT64 value, const char* what) {
    const UINT64 found = memory.read64(offset);
    if (found != value) {
        char message[160];
        std::snprintf(message, sizeof(message), "%s: expected %llu, found %llu", what,
                      static_cast<unsigned long long>(value), static_cast<unsigned long long>(found));
        throw std::runtime_error(message);
    }
}

// Both bottom-of-pipe writes store ordinary values, with or without a flush.
void labels(ID3D12Device* device) {
    Memory memory(65536);
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    direct.write32(memory.address(0), 0x12345678);
    direct.write32(memory.address(8), 7, 1);
    direct.write64(memory.address(16), 0x1122334455667788ull);
    direct.write64(memory.address(24), 42, 0x100);
    direct.submit();
    direct.finish("label writes");
    expect_value(memory, 0, 0x12345678, "32-bit write");
    expect_value(memory, 8, 7, "32-bit write with a flush");
    expect_value(memory, 16, 0x1122334455667788ull, "64-bit write");
    expect_value(memory, 24, 42, "64-bit write with a flush");
}

// A compute queue waits for a label the direct queue writes later, and the
// direct queue then waits for the compute queue's answer.
void cross_queue(ID3D12Device* device) {
    Memory memory(65536);
    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    compute.wait64(memory.address(0), 5);
    compute.write64(memory.address(8), 6);
    compute.submit();
    direct.write64(memory.address(0), 5);
    direct.submit();
    direct.finish("direct queue");
    compute.finish("compute queue waiting on a direct queue label");
    expect_value(memory, 8, 6, "compute answer");
}

// The compute queue submits through ExecuteCommandListsX as well.
void compute_frame(ID3D12Device* device) {
    Memory memory(65536);
    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    compute.write64(memory.address(0), 99);
    compute.close();
    ID3D12CommandList* batch[]{compute.list.Get()};
    method<ExecuteFrame>(compute.queue.Get(), queue_execute_frame)(compute.queue.Get(), 1, batch, 1, 3);
    compute.signal();
    compute.finish("compute frame submission");
    expect_value(memory, 0, 99, "compute frame write");
}

// A command that cannot be recorded is left out on its own. The list still
// closes and its fence write, which another queue may need, still happens.
void skipped_command(ID3D12Device* device) {
    Memory memory(65536);
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    auto sync_top = method<Sync>(direct.list.Get(), slot_sync_top);
    auto sync_bottom = method<Sync>(direct.list.Get(), slot_sync_bottom);
    sync_top(direct.list.Get(), 0xc280, 0x20000, 0, 0xffffffffff00ull);
    // The BVH builder passes no description for a build it cannot express.
    method<Build>(direct.list.Get(), slot_build)(direct.list.Get(), nullptr, 0, nullptr);
    sync_bottom(direct.list.Get(), 0xc280, 0x20000, 0, 0xffffffffff00ull);
    direct.write64(memory.address(0), 11);
    direct.submit();
    direct.finish("list with a skipped command");
    expect_value(memory, 0, 11, "write after a skipped command");
}

// CopyMemoryToMemoryX copies from the source to the destination.
void copy_memory(ID3D12Device* device) {
    constexpr size_t bytes = 4096, source = 0, destination = 65536;
    Memory memory(2 * 65536);
    for (size_t i = 0; i < bytes; ++i) {
        memory.base[source + i] = static_cast<BYTE>(i * 31 + 7);
        memory.base[destination + i] = 0xee;
    }
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    method<CopyMemory_>(direct.list.Get(), slot_copy_memory)(
        direct.list.Get(), memory.address(destination), memory.address(source), bytes, 0);
    direct.submit();
    direct.finish("memory copy");
    if (std::memcmp(memory.base + source, memory.base + destination, bytes))
        throw std::runtime_error("memory copy did not reach the destination");
    if (memory.base[source + 1] != 38)
        throw std::runtime_error("memory copy overwrote its source");
}

// The compute queue waits for a label, then reads a guest texture the direct
// queue writes before setting that label. A provider that makes the writer wait
// for the earlier submitted reader deadlocks the two queues.
void shared_texture(ID3D12Device* device) {
    constexpr UINT width = 256, height = 256, pitch = 256;
    constexpr UINT64 bytes = UINT64(pitch) * height;
    Memory memory(4 * 65536);
    Memory label(65536);
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8_UNORM;
    desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x105);
    ComPtr<ID3D12Resource> texture;
    check(method<Place>(device, device_place)(device, memory.address(), &desc, D3D12_RESOURCE_STATE_COMMON,
                                              nullptr, IID_PPV_ARGS(&texture)), "place guest texture");
    auto upload = buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto readback = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
    std::vector<BYTE> expected(bytes);
    for (size_t i = 0; i < bytes; ++i) expected[i] = static_cast<BYTE>(i * 13 + i / pitch);
    compute_fixture::upload(upload.Get(), expected.data(), bytes);

    D3D12_TEXTURE_COPY_LOCATION image{}, linear{};
    image.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    image.pResource = texture.Get();
    linear.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    linear.PlacedFootprint.Footprint = {DXGI_FORMAT_R8_UNORM, width, height, 1, pitch};

    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    compute.wait64(label.address(), 1);
    linear.pResource = readback.Get();
    method<frame_abi::Copy>(compute.list.Get(), slot_copy_texture)(
        compute.list.Get(), &linear, 0, 0, 0, &image, nullptr, 0);
    compute.submit();
    linear.pResource = upload.Get();
    method<frame_abi::Copy>(direct.list.Get(), slot_copy_texture)(
        direct.list.Get(), &image, 0, 0, 0, &linear, nullptr, 0);
    direct.write64(label.address(), 1);
    direct.submit();
    direct.finish("direct queue writing a texture a waiting queue reads");
    compute.finish("compute queue reading the texture");

    D3D12_RANGE range{0, static_cast<SIZE_T>(bytes)}, empty{};
    void* mapped{};
    check(readback->Map(0, &range, &mapped), "map readback");
    const bool matched = std::memcmp(mapped, expected.data(), bytes) == 0;
    readback->Unmap(0, &empty);
    if (!matched) throw std::runtime_error("the reader did not see the writer's texels");
}

// A console shader reaches the GPU through the translation cache. The provider
// substitutes an empty pipeline when no translation exists, so `verify` also
// checks that it reports loading the translated one.
void xbox_shader_scenario(ID3D12Device* device) {
    ComPtr<ID3D12RootSignature> root;
    D3D12_ROOT_SIGNATURE_DESC signature{};
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&signature, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
          "serialize empty root signature");
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create empty root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {xbox_shader, sizeof(xbox_shader)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<CreateComputePipeline>(device, device_create_compute)(device, &shader, 0, nullptr,
                                                                       IID_PPV_ARGS(&pipeline)),
          "create pipeline from a console shader");
    Memory memory(65536);
    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    method<SetRoot>(compute.list.Get(), slot_compute_root)(compute.list.Get(), root.Get());
    compute.list->SetPipelineState(pipeline.Get());
    compute.list->Dispatch(1, 1, 1);
    compute.write64(memory.address(0), 3);
    compute.submit();
    compute.finish("dispatch of the console shader");
    expect_value(memory, 0, 3, "write after the dispatch");
}

// ExecuteIndirect on its Series X|S slot runs the course shader from an
// argument buffer, and matches the same course dispatched directly.
void indirect_dispatch(ID3D12Device* device) {
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[0].Constants = {1, 2, 2};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {2, 1};
    D3D12_ROOT_SIGNATURE_DESC signature{};
    signature.NumParameters = 2;
    signature.pParameters = params;
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&signature, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
          "serialize course root signature");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create course root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {course_dxil, sizeof(course_dxil)};
    ComPtr<ID3D12PipelineState> pipeline;
    check(method<CreateComputePipeline>(device, device_create_compute)(device, &shader, 0, nullptr,
                                                                       IID_PPV_ARGS(&pipeline)),
          "create course pipeline");

    D3D12_INDIRECT_ARGUMENT_DESC argument{};
    argument.Type = D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH;
    D3D12_COMMAND_SIGNATURE_DESC indirect{};
    indirect.ByteStride = sizeof(D3D12_DISPATCH_ARGUMENTS);
    indirect.NumArgumentDescs = 1;
    indirect.pArgumentDescs = &argument;
    ComPtr<ID3D12CommandSignature> command;
    check(device->CreateCommandSignature(&indirect, nullptr, IID_PPV_ARGS(&command)), "create command signature");
    const D3D12_DISPATCH_ARGUMENTS groups{(course_length + 7) / 8, 1, 1};
    auto arguments = buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(groups), D3D12_RESOURCE_STATE_GENERIC_READ);
    compute_fixture::upload(arguments.Get(), &groups, sizeof(groups));

    constexpr UINT64 bytes = sizeof(Course);
    ComPtr<ID3D12Resource> output[2], readback[2];
    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    auto* list = compute.list.Get();
    const UINT constants[] = {0x5194c0d1u, course_length};
    method<SetRoot>(list, slot_compute_root)(list, root.Get());
    list->SetPipelineState(pipeline.Get());
    method<SetConstants>(list, slot_root_constants)(list, 0, 2, constants, 0);
    for (int pass = 0; pass < 2; ++pass) {
        output[pass] = buffer(device, D3D12_HEAP_TYPE_DEFAULT, bytes, D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                              D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
        readback[pass] = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
        method<SetUav>(list, slot_root_uav)(list, 1, output[pass]->GetGPUVirtualAddress());
        if (pass == 0)
            list->Dispatch(groups.ThreadGroupCountX, 1, 1);
        else
            method<ExecuteIndirect_>(list, slot_execute_indirect)(list, command.Get(), 1, arguments.Get(), 0,
                                                                  nullptr, 0);
        compute_fixture::transition(list, output[pass].Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                    D3D12_RESOURCE_STATE_COPY_SOURCE);
        list->CopyBufferRegion(readback[pass].Get(), 0, output[pass].Get(), 0, bytes);
    }
    compute.submit();
    compute.finish("direct and indirect course dispatches");

    Course rows[2]{};
    for (int pass = 0; pass < 2; ++pass) {
        D3D12_RANGE range{0, bytes}, empty{};
        void* mapped{};
        check(readback[pass]->Map(0, &range, &mapped), "map course readback");
        std::memcpy(rows[pass].data(), mapped, bytes);
        readback[pass]->Unmap(0, &empty);
    }
    if (rows[0].front() != 0x0001) throw std::runtime_error("direct course dispatch wrote an unexpected course");
    if (rows[0] != rows[1]) throw std::runtime_error("indirect dispatch differs from the direct dispatch");
}

// CopyMemoryToMemoryX reads a guest texture back into title memory, as linear
// rows at the copyable footprint's pitch.
void texture_readback(ID3D12Device* device) {
    constexpr UINT width = 256, height = 256, pitch = 256;
    constexpr UINT64 bytes = UINT64(pitch) * height;
    Memory image(4 * 65536);
    Memory result(2 * 65536);
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = width;
    desc.Height = height;
    desc.DepthOrArraySize = 1;
    desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Format = DXGI_FORMAT_R8_UNORM;
    desc.Layout = static_cast<D3D12_TEXTURE_LAYOUT>(0x105);
    ComPtr<ID3D12Resource> texture;
    check(method<Place>(device, device_place)(device, image.address(), &desc, D3D12_RESOURCE_STATE_COMMON,
                                              nullptr, IID_PPV_ARGS(&texture)), "place guest texture");
    auto upload = buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    std::vector<BYTE> expected(bytes);
    for (size_t i = 0; i < bytes; ++i) expected[i] = static_cast<BYTE>(i * 7 + i / pitch * 3);
    compute_fixture::upload(upload.Get(), expected.data(), bytes);

    D3D12_TEXTURE_COPY_LOCATION target{}, source{};
    target.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
    target.pResource = texture.Get();
    source.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
    source.pResource = upload.Get();
    source.PlacedFootprint.Footprint = {DXGI_FORMAT_R8_UNORM, width, height, 1, pitch};
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    method<frame_abi::Copy>(direct.list.Get(), slot_copy_texture)(direct.list.Get(), &target, 0, 0, 0, &source,
                                                                  nullptr, 0);
    // The write promoted the texture to a copy destination. It must leave that
    // state before it can be read.
    compute_fixture::transition(direct.list.Get(), texture.Get(), D3D12_RESOURCE_STATE_COPY_DEST,
                                D3D12_RESOURCE_STATE_COPY_SOURCE);
    method<CopyMemory_>(direct.list.Get(), slot_copy_memory)(direct.list.Get(), result.address(),
                                                             image.address(), bytes, 0);
    direct.submit();
    direct.finish("texture readback");
    if (std::memcmp(result.base, expected.data(), bytes))
        throw std::runtime_error("texture readback differs from the uploaded texels");
}

// `count` timed calls, in microseconds, sorted.
template<class Timed>
std::vector<double> timed_us(unsigned count, Timed timed) {
    LARGE_INTEGER frequency{}, before{}, after{};
    QueryPerformanceFrequency(&frequency);
    std::vector<double> readings(count);
    for (auto& reading : readings) {
        QueryPerformanceCounter(&before);
        timed();
        QueryPerformanceCounter(&after);
        reading = double(after.QuadPart - before.QuadPart) * 1e6 / double(frequency.QuadPart);
    }
    std::sort(readings.begin(), readings.end());
    return readings;
}

template<class Timed>
double median_us(unsigned count, Timed timed) {
    return timed_us(count, timed)[count / 2];
}

// The translation of the console shader that preparation left in the cache, or
// nothing when it has not been prepared.
std::vector<char> prepared_translation() {
    wchar_t temporary[MAX_PATH + 1]{};
    if (!GetTempPathW(MAX_PATH, temporary)) return {};
    const auto file = std::filesystem::path(temporary) / L"tuxbox_shader_cache" / L"by_source" /
                      (std::wstring(xbox_shader_digest) + L".cso");
    std::ifstream input(file, std::ios::binary);
    return {std::istreambuf_iterator<char>(input), std::istreambuf_iterator<char>()};
}

struct Readings {
    double submit_us, roundtrip_us, copy_us;
    // Zero when the console shader's translation is not prepared.
    double shader_us, dxil_us;
};

Readings measure(ID3D12Device* device) {
    Readings result{};
    Memory memory(4 * 65536);
    {
        // One list holding a fence write: record, submit and wait.
        Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
        UINT64 value = 0;
        result.submit_us = median_us(300, [&] {
            if (value) direct.reopen();
            direct.write64(memory.address(0), ++value);
            direct.submit();
            direct.finish("benchmark submission");
        });
    }
    {
        // A label handed from the direct queue to a waiting compute queue.
        Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
        Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
        UINT64 value = 0;
        result.roundtrip_us = median_us(200, [&] {
            if (value) {
                compute.reopen();
                direct.reopen();
            }
            ++value;
            compute.wait64(memory.address(8), value);
            compute.write64(memory.address(16), value);
            compute.submit();
            direct.write64(memory.address(8), value);
            direct.submit();
            direct.finish("benchmark label writer");
            compute.finish("benchmark label waiter");
        });
    }
    {
        Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
        bool used = false;
        result.copy_us = median_us(200, [&] {
            if (used) direct.reopen();
            used = true;
            method<CopyMemory_>(direct.list.Get(), slot_copy_memory)(
                direct.list.Get(), memory.address(2 * 65536), memory.address(65536), 65536, 0);
            direct.submit();
            direct.finish("benchmark memory copy");
        });
    }
    const auto translation = prepared_translation();
    if (!translation.empty()) {
        // The same pipeline twice over: from the console shader, which the
        // provider looks up in the translation cache, and from the translation
        // itself. The difference is what the lookup costs.
        D3D12_ROOT_SIGNATURE_DESC signature{};
        ComPtr<ID3DBlob> serialized;
        check(D3D12SerializeRootSignature(&signature, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
              "serialize empty root signature");
        ComPtr<ID3D12RootSignature> root;
        check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                          IID_PPV_ARGS(&root)), "create empty root signature");
        auto create = [&](const void* bytes, size_t size) {
            D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
            shader.pRootSignature = root.Get();
            shader.CS = {bytes, size};
            ComPtr<ID3D12PipelineState> pipeline;
            check(method<CreateComputePipeline>(device, device_create_compute)(device, &shader, 0, nullptr,
                                                                               IID_PPV_ARGS(&pipeline)),
                  "benchmark pipeline");
        };
        result.shader_us = median_us(100, [&] { create(xbox_shader, sizeof(xbox_shader)); });
        result.dxil_us = median_us(100, [&] { create(translation.data(), translation.size()); });
    }
    return result;
}

// TuxBox's XboxBvhBuilder, loaded by name as a title's builder calls reach it.
struct Builder {
    using Construct = void* (WINAPI*)(void*, ID3D12Device*, bool);
    using Destroy = void (WINAPI*)(void*);
    using Build = void (WINAPI*)(void*, ID3D12GraphicsCommandList*,
                                 const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC*, UINT,
                                 const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC*);
    // D3D12XBOX_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_BATCHED_DESC2.
    struct Entry {
        D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC desc;
        UINT64 report_count;
        const D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC* reports;
        UINT64 stream_bytes;
        const void* stream;
    };
    using Batch = void (WINAPI*)(void*, ID3D12GraphicsCommandList*, UINT, const Entry*);
    using CopyStructure = void (WINAPI*)(void*, ID3D12GraphicsCommandList*, D3D12_GPU_VIRTUAL_ADDRESS,
                                         D3D12_GPU_VIRTUAL_ADDRESS,
                                         D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE);
    using Prebuild = void (WINAPI*)(void*, const D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS*,
                                    const UINT64*,
                                    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO*);

    HMODULE library;
    alignas(8) BYTE object[24]{};
    explicit Builder(ID3D12Device* device)
        : library(LoadLibraryExW(L"XboxBvhBuilder.dll", nullptr, 0)) {
        if (!library) throw std::runtime_error("load XboxBvhBuilder.dll");
        entry<Construct>(1)(object, device, false);
    }
    ~Builder() {
        entry<Destroy>(2)(object);
        FreeLibrary(library);
    }
    Builder(const Builder&) = delete;
    Builder& operator=(const Builder&) = delete;
    template<class Function> Function entry(WORD ordinal) const {
        auto address = GetProcAddress(library, MAKEINTRESOURCEA(ordinal));
        if (!address) throw std::runtime_error("missing XboxBvhBuilder export");
        return reinterpret_cast<Function>(address);
    }
};

void unordered_barrier(ID3D12GraphicsCommandList* list) {
    D3D12_RESOURCE_BARRIER barrier{};
    barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_UAV;
    list->ResourceBarrier(1, &barrier);
}

// A bottom-level structure built singly with a size report, copied, and built
// again through the batched call, all by way of the console's builder library.
void ray_tracing_builder(ID3D12Device* device) {
    Builder builder(device);
    const float triangle[] = {0.f, 0.f, 0.f, 1.f, 0.f, 0.f, 0.f, 1.f, 0.f};
    auto vertices = buffer(device, D3D12_HEAP_TYPE_UPLOAD, sizeof(triangle), D3D12_RESOURCE_STATE_GENERIC_READ);
    compute_fixture::upload(vertices.Get(), triangle, sizeof(triangle));
    D3D12_RAYTRACING_GEOMETRY_DESC geometry{};
    geometry.Type = D3D12_RAYTRACING_GEOMETRY_TYPE_TRIANGLES;
    geometry.Flags = D3D12_RAYTRACING_GEOMETRY_FLAG_OPAQUE;
    geometry.Triangles.VertexFormat = DXGI_FORMAT_R32G32B32_FLOAT;
    geometry.Triangles.VertexCount = 3;
    geometry.Triangles.VertexBuffer = {vertices->GetGPUVirtualAddress(), 3 * sizeof(float)};
    D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_INPUTS inputs{};
    inputs.Type = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_TYPE_BOTTOM_LEVEL;
    inputs.Flags = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_BUILD_FLAG_ALLOW_COMPACTION;
    inputs.NumDescs = 1;
    inputs.pGeometryDescs = &geometry;
    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_PREBUILD_INFO size{};
    builder.entry<Builder::Prebuild>(7)(builder.object, &inputs, nullptr, &size);
    if (!size.ResultDataMaxSizeInBytes || !size.ScratchDataSizeInBytes)
        throw std::runtime_error("no prebuild sizes; ray tracing is unavailable");

    auto gpu_buffer = [&](UINT64 bytes, D3D12_RESOURCE_STATES state) {
        return buffer(device, D3D12_HEAP_TYPE_DEFAULT, bytes, state,
                      D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    };
    ComPtr<ID3D12Resource> structure[3];
    for (auto& item : structure)
        item = gpu_buffer(size.ResultDataMaxSizeInBytes,
                          D3D12_RESOURCE_STATE_RAYTRACING_ACCELERATION_STRUCTURE);
    auto scratch = gpu_buffer(size.ScratchDataSizeInBytes, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    auto reports = gpu_buffer(32, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    auto readback = buffer(device, D3D12_HEAP_TYPE_READBACK, 32, D3D12_RESOURCE_STATE_COPY_DEST);

    D3D12_BUILD_RAYTRACING_ACCELERATION_STRUCTURE_DESC build{};
    build.Inputs = inputs;
    build.DestAccelerationStructureData = structure[0]->GetGPUVirtualAddress();
    build.ScratchAccelerationStructureData = scratch->GetGPUVirtualAddress();
    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC report[2]{};
    for (int i = 0; i < 2; ++i) {
        report[i].DestBuffer = reports->GetGPUVirtualAddress() + 8 * i;
        report[i].InfoType = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_COMPACTED_SIZE;
    }
    Memory label(65536);
    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    auto* list = direct.list.Get();
    builder.entry<Builder::Build>(3)(builder.object, list, &build, 1, &report[0]);
    unordered_barrier(list);
    builder.entry<Builder::CopyStructure>(5)(builder.object, list, structure[1]->GetGPUVirtualAddress(),
                                             structure[0]->GetGPUVirtualAddress(),
                                             D3D12_RAYTRACING_ACCELERATION_STRUCTURE_COPY_MODE_CLONE);
    unordered_barrier(list);
    // The size of the built structure and of its copy, which a missing copy
    // leaves empty.
    D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_DESC current{};
    current.DestBuffer = reports->GetGPUVirtualAddress() + 16;
    current.InfoType = D3D12_RAYTRACING_ACCELERATION_STRUCTURE_POSTBUILD_INFO_CURRENT_SIZE;
    const D3D12_GPU_VIRTUAL_ADDRESS measured[] = {structure[0]->GetGPUVirtualAddress(),
                                                  structure[1]->GetGPUVirtualAddress()};
    method<EmitSizes>(list, slot_emit_sizes)(list, &current, 2, measured);
    unordered_barrier(list);
    Builder::Entry entry{};
    entry.desc = build;
    entry.desc.DestAccelerationStructureData = structure[2]->GetGPUVirtualAddress();
    entry.report_count = 1;
    entry.reports = &report[1];
    builder.entry<Builder::Batch>(4)(builder.object, list, 1, &entry);
    unordered_barrier(list);
    compute_fixture::transition(list, reports.Get(), D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                                D3D12_RESOURCE_STATE_COPY_SOURCE);
    list->CopyBufferRegion(readback.Get(), 0, reports.Get(), 0, 32);
    direct.write64(label.address(), 21);
    direct.submit();
    direct.finish("acceleration structure builds and copy");
    expect_value(label, 0, 21, "write after the builds");

    D3D12_RANGE range{0, 32}, empty{};
    UINT64* mapped{};
    check(readback->Map(0, &range, reinterpret_cast<void**>(&mapped)), "map size reports");
    const UINT64 single = mapped[0], batched = mapped[1], original = mapped[2], copied = mapped[3];
    readback->Unmap(0, &empty);
    if (!single || single > size.ResultDataMaxSizeInBytes)
        throw std::runtime_error("the single build reported no compacted size");
    if (batched != single) throw std::runtime_error("the batched build was not recorded like the single one");
    if (!original || copied != original) throw std::runtime_error("the acceleration structure was not copied");
}

// Slot 80 (dispatch_flagged) with USE_THREAD_DIMENSIONS (0x20) counts threads
// rather than groups, converting via the bound PSO's [numthreads(8, 1, 1)].
void thread_dimensions(ID3D12Device* device) {
    D3D12_ROOT_PARAMETER params[2]{};
    params[0].ParameterType = D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS;
    params[0].Constants = {1, 2, 2};
    params[1].ParameterType = D3D12_ROOT_PARAMETER_TYPE_UAV;
    params[1].Descriptor = {2, 1};
    D3D12_ROOT_SIGNATURE_DESC sig{};
    sig.NumParameters = 2;
    sig.pParameters = params;
    ComPtr<ID3DBlob> serialized;
    check(D3D12SerializeRootSignature(&sig, D3D_ROOT_SIGNATURE_VERSION_1, &serialized, nullptr),
          "serialize thread-dimensions root signature");
    ComPtr<ID3D12RootSignature> root;
    check(device->CreateRootSignature(0, serialized->GetBufferPointer(), serialized->GetBufferSize(),
                                      IID_PPV_ARGS(&root)), "create thread-dimensions root signature");
    D3D12_COMPUTE_PIPELINE_STATE_DESC shader{};
    shader.pRootSignature = root.Get();
    shader.CS = {course_dxil, sizeof(course_dxil)};
    ComPtr<ID3D12PipelineState> pipeline, stream_pipeline;
    check(method<CreateComputePipeline>(device, device_create_compute)(device, &shader, 0, nullptr,
                                                                       IID_PPV_ARGS(&pipeline)),
          "create thread-dimensions compute pipeline");
    struct alignas(void*) ComputeStream {
        int root_tag = 0;
        UINT pad0 = 0;
        ID3D12RootSignature* root = nullptr;
        int cs_tag = 6;
        UINT pad1 = 0;
        D3D12_SHADER_BYTECODE cs{};
    } stream{};
    static_assert(sizeof(ComputeStream) == 40);
    stream.root = root.Get();
    stream.cs = shader.CS;
    D3D12_PIPELINE_STATE_STREAM_DESC stream_desc{sizeof(stream), &stream};
    check(method<CreateStreamPipeline>(device, device_create_stream)(device, &stream_desc,
                                                                     IID_PPV_ARGS(&stream_pipeline)),
          "create thread-dimensions stream pipeline");

    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    auto dispatch_flagged = method<DispatchFlagged>(compute.list.Get(), slot_dispatch_flagged);

    // Fail closed when flags & 0x20 is set without a bound compute pipeline.
    dispatch_flagged(compute.list.Get(), 8, 1, 1, 0x20);
    if (SUCCEEDED(compute.list->Close()))
        throw std::runtime_error("slot 80 thread-dimension dispatch accepted without a pipeline");
    check(compute.list->Reset(compute.allocator.Get(), pipeline.Get()), "reset after missing pipeline");
    compute.list->ClearState(nullptr);
    dispatch_flagged(compute.list.Get(), 8, 1, 1, 0x8020);
    if (SUCCEEDED(compute.list->Close()))
        throw std::runtime_error("slot 80 thread-dimension dispatch accepted after ClearState(nullptr)");

    constexpr UINT words = 64, guard = 0xdeadbeefu;
    constexpr UINT64 bytes = words * sizeof(UINT);
    std::array<UINT, words> seed_words;
    seed_words.fill(guard);
    auto seed = buffer(device, D3D12_HEAP_TYPE_UPLOAD, bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto output = buffer(device, D3D12_HEAP_TYPE_DEFAULT, bytes, D3D12_RESOURCE_STATE_COPY_DEST,
                         D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback = buffer(device, D3D12_HEAP_TYPE_READBACK, bytes, D3D12_RESOURCE_STATE_COPY_DEST);
    compute_fixture::upload(seed.Get(), seed_words.data(), bytes);

    struct Pass {
        UINT x;
        UINT flags;
        UINT row_count;
        UINT expected_written;
        UINT mode; // 0 = Reset(pipeline), 1 = SetPipelineState(stream_pipeline), 2 = ClearState(pipeline)
    };
    constexpr Pass passes[] = {
        {16, 0x20, words, 16, 0},
        {8, 0x8020, words, 8, 1},
        {70000, 0x20, 24, 24, 2},
        {13, 0x20, 13, 13, 0},
        {0, 0x20, words, 0, 0},
        {2, 0x0, words, 16, 0},
        {1, 0x8000, words, 8, 1},
    };
    const UINT seed_value = 7;
    auto expected_word = [&](UINT row) {
        const UINT beat = (row + seed_value) & 3u;
        const UINT coin = beat == 3u ? 1u : beat;
        UINT mixed = (row + seed_value) * 747796405u + 2891336453u;
        mixed ^= mixed >> 16;
        const UINT barrier = (coin + 1u + (mixed & 1u)) % 3u;
        return coin | (barrier << 8);
    };
    for (size_t i = 0; i < std::size(passes); ++i) {
        const auto& pass = passes[i];
        check(compute.allocator->Reset(), "reset thread-dimensions allocator");
        check(compute.list->Reset(compute.allocator.Get(), pass.mode == 0 ? pipeline.Get() : nullptr),
              "reset thread-dimensions list");
        if (i != 0)
            compute_fixture::transition(compute.list.Get(), output.Get(),
                                        D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_COPY_DEST);
        compute.list->CopyBufferRegion(output.Get(), 0, seed.Get(), 0, bytes);
        compute_fixture::transition(compute.list.Get(), output.Get(),
                                    D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        if (pass.mode == 1)
            compute.list->SetPipelineState(stream_pipeline.Get());
        else if (pass.mode == 2)
            compute.list->ClearState(pipeline.Get());
        auto* raw = compute.list.Get();
        const UINT constants[] = {seed_value, pass.row_count};
        method<SetRoot>(raw, slot_compute_root)(raw, root.Get());
        method<SetConstants>(raw, slot_root_constants)(raw, 0, 2, constants, 0);
        method<SetUav>(raw, slot_root_uav)(raw, 1, output->GetGPUVirtualAddress());
        dispatch_flagged(raw, pass.x, 1, 1, pass.flags);
        compute_fixture::transition(compute.list.Get(), output.Get(),
                                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        compute.list->CopyBufferRegion(readback.Get(), 0, output.Get(), 0, bytes);
        compute.submit();
        compute.finish("slot 80 dispatch");

        std::array<UINT, words> actual{};
        D3D12_RANGE range{0, static_cast<SIZE_T>(bytes)}, empty{};
        void* mapped{};
        check(readback->Map(0, &range, &mapped), "map thread-dimensions readback");
        std::memcpy(actual.data(), mapped, bytes);
        readback->Unmap(0, &empty);
        for (UINT row = 0; row < words; ++row) {
            const UINT expected = row < pass.expected_written ? expected_word(row) : guard;
            if (actual[row] != expected) {
                char msg[160];
                std::snprintf(msg, sizeof(msg),
                              "pass %zu (x=%u flags=%#x) row %u: expected %#x, found %#x",
                              i, pass.x, pass.flags, row, expected, actual[row]);
                throw std::runtime_error(msg);
            }
        }
    }
}

// Exercise shader-visible descriptor slot reclamation on an oversized paged heap:
// (1) sustained >2x turnover with mixed table lengths, (2) overlapping in-flight
// Reset() survival, (3) ClearState pin retention, (4) re-binding an evicted slot
// with fresh view data, and (5) fail-closed E_OUTOFMEMORY without corruption.
void descriptor_reclaim(ID3D12Device* device) {
    constexpr UINT block_16k = 16384;
    constexpr UINT block_8k = 8192;
    constexpr UINT num_16k_blocks = 128;
    constexpr UINT heap_descriptors = num_16k_blocks * block_16k; // 2,097,152 > 1,000,000 native limit
    constexpr UINT max_dispatches = 1024;
    constexpr UINT sentinel = 0xdeadbeefu;

    D3D12_DESCRIPTOR_HEAP_DESC heap_desc{
        D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV, heap_descriptors, D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE, 1};
    ComPtr<ID3D12DescriptorHeap> view_heap;
    check(device->CreateDescriptorHeap(&heap_desc, IID_PPV_ARGS(&view_heap)), "create oversized view heap");
    const UINT stride = device->GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV);
    if (!stride) throw std::runtime_error("zero view descriptor stride");
    const SIZE_T cpu_base = method<frame_abi::CpuHandle>(view_heap.Get(), 9)(view_heap.Get());
    const UINT64 gpu_base = method<GpuHandle>(view_heap.Get(), 10)(view_heap.Get());
    auto cpu_at = [&](UINT index) -> D3D12_CPU_DESCRIPTOR_HANDLE {
        return {cpu_base + SIZE_T(index) * stride};
    };
    auto gpu_at = [&](UINT index) -> D3D12_GPU_DESCRIPTOR_HANDLE {
        return {gpu_base + UINT64(index) * stride};
    };

    auto make_pipeline = [&](UINT table_len, ComPtr<ID3D12RootSignature>& rs, ComPtr<ID3D12PipelineState>& pso) {
        D3D12_DESCRIPTOR_RANGE ranges[3]{};
        ranges[0] = {D3D12_DESCRIPTOR_RANGE_TYPE_CBV, 1, 0, 0, 0};
        ranges[1] = {D3D12_DESCRIPTOR_RANGE_TYPE_SRV, 1, 0, 0, 1};
        ranges[2] = {D3D12_DESCRIPTOR_RANGE_TYPE_UAV, 1, 0, 0, table_len - 1};
        D3D12_ROOT_PARAMETER param{};
        param.ParameterType = D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE;
        param.DescriptorTable = {3, ranges};
        D3D12_ROOT_SIGNATURE_DESC sig{1, &param, 0, nullptr, D3D12_ROOT_SIGNATURE_FLAG_NONE};
        ComPtr<ID3DBlob> blob;
        check(D3D12SerializeRootSignature(&sig, D3D_ROOT_SIGNATURE_VERSION_1, &blob, nullptr),
              "serialize reclaim root signature");
        check(device->CreateRootSignature(0, blob->GetBufferPointer(), blob->GetBufferSize(), IID_PPV_ARGS(&rs)),
              "create reclaim root signature");
        D3D12_COMPUTE_PIPELINE_STATE_DESC pso_desc{};
        pso_desc.pRootSignature = rs.Get();
        pso_desc.CS = {reclaim_dxil, sizeof(reclaim_dxil)};
        check(method<CreateComputePipeline>(device, device_create_compute)(
                  device, &pso_desc, 0, nullptr, IID_PPV_ARGS(&pso)),
              "create reclaim compute pipeline");
    };
    ComPtr<ID3D12RootSignature> rs16, rs8;
    ComPtr<ID3D12PipelineState> pso16, pso8;
    make_pipeline(block_16k, rs16, pso16);
    make_pipeline(block_8k, rs8, pso8);

    auto cb_key = [](UINT k) { return 0x1000u + k * 7u + 3u; };
    auto cb_salt = [](UINT k) { return 0x50000u + k * 13u + 5u; };
    auto src_word = [](UINT k) { return 0x200u + k * 19u + 11u; };
    auto expected_word = [&](UINT k) { return src_word(k) * 131u + cb_key(k) * 17u + cb_salt(k); };

    std::vector<UINT> cb_bytes(size_t(max_dispatches) * 64, 0);
    std::vector<UINT> src_words(max_dispatches, 0);
    std::vector<UINT> seed_words(max_dispatches, sentinel);
    std::vector<UINT> expected_out(max_dispatches, sentinel);
    for (UINT k = 0; k < max_dispatches; ++k) {
        cb_bytes[size_t(k) * 64 + 0] = cb_key(k);
        cb_bytes[size_t(k) * 64 + 1] = cb_salt(k);
        src_words[k] = src_word(k);
    }
    const UINT64 out_bytes = UINT64(max_dispatches) * sizeof(UINT);
    auto cb_pool = buffer(device, D3D12_HEAP_TYPE_UPLOAD, UINT64(max_dispatches) * 256,
                          D3D12_RESOURCE_STATE_GENERIC_READ);
    auto src_pool = buffer(device, D3D12_HEAP_TYPE_UPLOAD, out_bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto seed_buf = buffer(device, D3D12_HEAP_TYPE_UPLOAD, out_bytes, D3D12_RESOURCE_STATE_GENERIC_READ);
    auto dst_buf = buffer(device, D3D12_HEAP_TYPE_DEFAULT, out_bytes, D3D12_RESOURCE_STATE_COPY_DEST,
                          D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS);
    auto readback_buf = buffer(device, D3D12_HEAP_TYPE_READBACK, out_bytes, D3D12_RESOURCE_STATE_COPY_DEST);
    compute_fixture::upload(cb_pool.Get(), cb_bytes.data(), cb_bytes.size() * sizeof(UINT));
    compute_fixture::upload(src_pool.Get(), src_words.data(), size_t(out_bytes));
    compute_fixture::upload(seed_buf.Get(), seed_words.data(), size_t(out_bytes));

    const UINT64 cb_va = cb_pool->GetGPUVirtualAddress();
    auto author_table = [&](UINT slot_base, UINT table_len, UINT data_idx, UINT out_idx) {
        D3D12_CONSTANT_BUFFER_VIEW_DESC cbv{cb_va + UINT64(data_idx) * 256, 256};
        device->CreateConstantBufferView(&cbv, cpu_at(slot_base + 0));
        D3D12_SHADER_RESOURCE_VIEW_DESC srv{};
        srv.ViewDimension = D3D12_SRV_DIMENSION_BUFFER;
        srv.Shader4ComponentMapping = D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING;
        srv.Buffer.FirstElement = data_idx;
        srv.Buffer.NumElements = 1;
        srv.Buffer.StructureByteStride = sizeof(UINT);
        device->CreateShaderResourceView(src_pool.Get(), &srv, cpu_at(slot_base + 1));
        D3D12_UNORDERED_ACCESS_VIEW_DESC uav{};
        uav.ViewDimension = D3D12_UAV_DIMENSION_BUFFER;
        uav.Buffer.FirstElement = out_idx;
        uav.Buffer.NumElements = 1;
        uav.Buffer.StructureByteStride = sizeof(UINT);
        device->CreateUnorderedAccessView(dst_buf.Get(), nullptr, &uav, cpu_at(slot_base + table_len - 1));
        expected_out[out_idx] = expected_word(data_idx);
    };
    auto bind_and_dispatch = [&](ID3D12GraphicsCommandList* list, UINT slot_base, UINT table_len, bool set_pso = true) {
        ID3D12DescriptorHeap* heaps[]{view_heap.Get()};
        method<BindHeaps>(list, slot_bind_heaps)(list, 1, heaps);
        ID3D12RootSignature* rs = table_len == block_16k ? rs16.Get() : rs8.Get();
        ID3D12PipelineState* pso = table_len == block_16k ? pso16.Get() : pso8.Get();
        method<SetRoot>(list, slot_compute_root)(list, rs);
        if (set_pso) list->SetPipelineState(pso);
        method<SetTable>(list, slot_compute_table)(list, 0, gpu_at(slot_base));
        list->Dispatch(1, 1, 1);
    };

    Queue direct(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
    Queue compute(device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
    direct.list->CopyBufferRegion(dst_buf.Get(), 0, seed_buf.Get(), 0, out_bytes);
    compute_fixture::transition(direct.list.Get(), dst_buf.Get(),
                                D3D12_RESOURCE_STATE_COPY_DEST, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
    direct.submit();
    direct.finish("seed reclaim output buffer");

    auto verify_outputs = [&](UINT check_count, const char* phase) {
        direct.reopen();
        compute_fixture::transition(direct.list.Get(), dst_buf.Get(),
                                    D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_COPY_SOURCE);
        direct.list->CopyBufferRegion(readback_buf.Get(), 0, dst_buf.Get(), 0, out_bytes);
        compute_fixture::transition(direct.list.Get(), dst_buf.Get(),
                                    D3D12_RESOURCE_STATE_COPY_SOURCE, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        direct.submit();
        direct.finish(phase);
        D3D12_RANGE range{0, SIZE_T(out_bytes)}, empty{};
        const UINT* mapped = nullptr;
        check(readback_buf->Map(0, &range, reinterpret_cast<void**>(const_cast<UINT**>(&mapped))),
              "map reclaim readback");
        for (UINT i = 0; i < check_count; ++i) {
            if (mapped[i] != expected_out[i]) {
                const UINT got = mapped[i];
                readback_buf->Unmap(0, &empty);
                char msg[192];
                std::snprintf(msg, sizeof(msg), "%s mismatch at output %u: expected 0x%08x, found 0x%08x",
                              phase, i, expected_out[i], got);
                throw std::runtime_error(msg);
            }
        }
        readback_buf->Unmap(0, &empty);
    };

    // 1. Sustained turnover (> 2x shader-visible limit) mixing 8K and 16K spans.
    //    Emit 32 8K tables (blocks 0..16) interleaved with 16K blocks 16..67,
    //    then 61 16K blocks 67..128 so all of 0..67 are evicted by the end.
    struct TableJob { UINT slot_base; UINT len; UINT idx; };
    std::vector<TableJob> jobs;
    jobs.reserve(144);
    UINT next_idx = 0;
    for (UINT i = 0; i < 32; ++i) {
        jobs.push_back({i * block_8k, block_8k, next_idx++});
        jobs.push_back({(16 + i) * block_16k, block_16k, next_idx++});
    }
    for (UINT b = 48; b < 128; ++b) {
        jobs.push_back({b * block_16k, block_16k, next_idx++});
    }
    for (const auto& job : jobs) author_table(job.slot_base, job.len, job.idx, job.idx);
    auto release_leases = [](Queue& q) {
        q.reopen();
        q.close();
    };
    for (size_t offset = 0; offset < jobs.size(); offset += 8) {
        if (offset > 0) compute.reopen();
        const size_t batch_end = std::min(offset + 8, jobs.size());
        for (size_t i = offset; i < batch_end; ++i)
            bind_and_dispatch(compute.list.Get(), jobs[i].slot_base, jobs[i].len);
        compute.submit();
        compute.finish("sustained descriptor turnover batch");
    }
    release_leases(compute);
    verify_outputs(next_idx, "sustained turnover");

    // 2. Overlapping / double-buffered in-flight safety across CPU Reset():
    //    L1 (20 tables) is queued behind gate_fence, L0 (30 tables) runs & finishes
    //    on direct (so L1's last_used is OLDER than L0's), L2 (15 tables) queues
    //    behind gate_fence, L1->Reset() drops L1's recording leases while L1 is
    //    still in-flight, and L3 (25 tables) forces eviction of L0 without touching L1/L2.
    const UINT sub2_base = next_idx;
    for (UINT b = 0; b < 90; ++b) {
        author_table(b * block_16k, block_16k, next_idx, next_idx);
        ++next_idx;
    }
    auto make_list = [&](D3D12_COMMAND_LIST_TYPE type, ComPtr<ID3D12CommandAllocator>& alloc,
                         ComPtr<ID3D12GraphicsCommandList>& list) {
        check(device->CreateCommandAllocator(type, IID_PPV_ARGS(&alloc)), "create extra allocator");
        const UINT list_desc[] = {static_cast<UINT>(type), 0, 1, 0};
        check(method<frame_abi::CreateList>(device, device_create_list)(
                  device, list_desc, alloc.Get(), nullptr, IID_PPV_ARGS(&list)),
              "create extra list");
    };
    {
        ComPtr<ID3D12Fence> gate_fence;
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&gate_fence)), "create gate fence");
        check(method<WaitQueue>(compute.queue.Get(), queue_wait)(compute.queue.Get(), gate_fence.Get(), 1),
              "stall compute queue on gate fence");

        ComPtr<ID3D12CommandAllocator> alloc_l1, alloc_l2, alloc_l3;
        ComPtr<ID3D12GraphicsCommandList> list_l1, list_l2;
        make_list(D3D12_COMMAND_LIST_TYPE_COMPUTE, alloc_l1, list_l1);
        make_list(D3D12_COMMAND_LIST_TYPE_COMPUTE, alloc_l2, list_l2);
        check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_COMPUTE, IID_PPV_ARGS(&alloc_l3)),
              "create spare L3 allocator");

        check(list_l1->Reset(alloc_l1.Get(), nullptr), "open L1");
        for (UINT b = 0; b < 20; ++b) bind_and_dispatch(list_l1.Get(), b * block_16k, block_16k);
        check(list_l1->Close(), "close L1");
        {
            ID3D12CommandList* batch[]{list_l1.Get()};
            compute.queue->ExecuteCommandLists(1, batch);
        }

        direct.reopen();
        for (UINT b = 20; b < 50; ++b) bind_and_dispatch(direct.list.Get(), b * block_16k, block_16k);
        direct.submit();
        direct.finish("subtest 2 L0");
        release_leases(direct);

        check(list_l2->Reset(alloc_l2.Get(), nullptr), "open L2");
        for (UINT b = 50; b < 65; ++b) bind_and_dispatch(list_l2.Get(), b * block_16k, block_16k);
        check(list_l2->Close(), "close L2");
        {
            ID3D12CommandList* batch[]{list_l2.Get()};
            compute.queue->ExecuteCommandLists(1, batch);
        }

        // Reset L1 on the CPU while L1 is still in-flight behind gate_fence!
        check(list_l1->Reset(alloc_l3.Get(), nullptr), "reset L1 while in-flight");
        for (UINT b = 65; b < 90; ++b) bind_and_dispatch(list_l1.Get(), b * block_16k, block_16k);
        check(list_l1->Close(), "close L3");
        {
            ID3D12CommandList* batch[]{list_l1.Get()};
            compute.queue->ExecuteCommandLists(1, batch);
        }
        check(gate_fence->Signal(1), "release gate fence");
        compute.signal();
        compute.finish("overlapping L1/L2/L3 execution");
    }
    verify_outputs(sub2_base + 90, "overlapping in-flight reset");

    // 3. ClearState pin retention:
    //    Fill 0..61 on direct, open CL and bind+dispatch table A (block 61),
    //    call ClearState, refresh 1..61 on direct (making A oldest), then bind
    //    60 new tables 62..122 (ending with table B at block 121) inside CL.
    for (UINT b = 0; b < 122; ++b) {
        author_table(b * block_16k, block_16k, next_idx, next_idx);
        ++next_idx;
    }
    direct.reopen();
    for (UINT b = 0; b < 61; ++b) bind_and_dispatch(direct.list.Get(), b * block_16k, block_16k);
    direct.submit();
    direct.finish("subtest 3 fill");
    release_leases(direct);

    compute.reopen();
    bind_and_dispatch(compute.list.Get(), 61u * block_16k, block_16k); // table A (block 61)
    compute.list->ClearState(pso16.Get());

    direct.reopen();
    for (UINT b = 1; b < 61; ++b) bind_and_dispatch(direct.list.Get(), b * block_16k, block_16k);
    direct.submit();
    direct.finish("subtest 3 refresh 1..61");
    release_leases(direct);

    // Bind 60 new tables 62..122 after ClearState; keep pso16 from ClearState(pso16).
    for (UINT b = 62; b < 122; ++b)
        bind_and_dispatch(compute.list.Get(), b * block_16k, block_16k, false);
    compute.submit();
    compute.finish("subtest 3 ClearState CL");
    verify_outputs(next_idx, "ClearState pin retention");

    // 4. Re-bind after eviction with fresh view data at logical block 0 (evicted in step 3).
    const UINT fresh_out = next_idx++;
    author_table(0, block_16k, fresh_out, fresh_out);
    compute.reopen();
    bind_and_dispatch(compute.list.Get(), 0, block_16k);
    compute.submit();
    compute.finish("re-bind evicted slot 0");
    release_leases(compute);
    verify_outputs(next_idx, "re-bind evicted slot");

    // 5. Fail-closed OOM control:
    //    Pin block 0 in open list L_pinned, then pin 62 more unique tables (total 63 * 16K
    //    = 1,032,192 > 1,000,000 limit) in open list L_oom and confirm Close() returns
    //    E_OUTOFMEMORY without corrupting L_pinned or preventing post-Reset recovery.
    const UINT pinned_out = next_idx++;
    const UINT recovery_out = next_idx++;
    author_table(0, block_16k, pinned_out, pinned_out);
    author_table(1 * block_16k, block_16k, recovery_out, recovery_out);

    ComPtr<ID3D12CommandAllocator> alloc_pin, alloc_oom;
    ComPtr<ID3D12GraphicsCommandList> list_pin, list_oom;
    make_list(D3D12_COMMAND_LIST_TYPE_COMPUTE, alloc_pin, list_pin);
    make_list(D3D12_COMMAND_LIST_TYPE_COMPUTE, alloc_oom, list_oom);

    check(list_pin->Reset(alloc_pin.Get(), nullptr), "open pinned control list");
    bind_and_dispatch(list_pin.Get(), 0, block_16k);

    check(list_oom->Reset(alloc_oom.Get(), nullptr), "open OOM control list");
    for (UINT b = 1; b <= 62; ++b) bind_and_dispatch(list_oom.Get(), b * block_16k, block_16k);
    const HRESULT oom_hr = list_oom->Close();
    if (oom_hr != E_OUTOFMEMORY) {
        char msg[128];
        std::snprintf(msg, sizeof(msg), "expected E_OUTOFMEMORY on unsubmitted pin overflow, got 0x%08lx",
                      static_cast<unsigned long>(oom_hr));
        throw std::runtime_error(msg);
    }

    check(list_pin->Close(), "close pinned control list after OOM");
    {
        ID3D12CommandList* batch[]{list_pin.Get()};
        compute.queue->ExecuteCommandLists(1, batch);
    }
    check(list_oom->Reset(alloc_oom.Get(), nullptr), "reset failed OOM list");
    bind_and_dispatch(list_oom.Get(), 1 * block_16k, block_16k);
    check(list_oom->Close(), "close recovery list");
    {
        ID3D12CommandList* batch[]{list_oom.Get()};
        compute.queue->ExecuteCommandLists(1, batch);
    }
    compute.signal();
    compute.finish("pinned control and post-OOM recovery");
    verify_outputs(next_idx, "fail-closed OOM control");
}

struct Scenario {
    const wchar_t* name;
    void (*run)(ID3D12Device*);
};
constexpr Scenario scenarios[] = {
    {L"labels", labels},
    {L"cross-queue", cross_queue},
    {L"compute-frame", compute_frame},
    {L"skipped-command", skipped_command},
    {L"copy-memory", copy_memory},
    {L"shared-texture", shared_texture},
    {L"indirect-dispatch", indirect_dispatch},
    {L"texture-readback", texture_readback},
    {L"ray-tracing-builder", ray_tracing_builder},
    {L"thread-dimensions", thread_dimensions},
    {L"descriptor-reclaim", descriptor_reclaim},
    {L"xbox-shader", xbox_shader_scenario},
};

// A fresh Scarlett device on the requested adapter. The factory is reached
// through the Xbox device, so the game imports no desktop graphics library.
ComPtr<ID3D12Device> create_scarlett_device(const wchar_t* requested, char (&pci)[16]) {
    auto create = [](IUnknown* adapter, ComPtr<ID3D12Device>& out) {
        return D3D12XboxCreateDevice(adapter, scarlett_profile.data(), __uuidof(ID3D12Device11),
                                     reinterpret_cast<void**>(out.ReleaseAndGetAddressOf()));
    };
    auto matches = [&](IDXGIAdapter1* adapter) {
        DXGI_ADAPTER_DESC1 desc{};
        check(adapter->GetDesc1(&desc), "adapter description");
        std::snprintf(pci, sizeof(pci), "%04x:%04x", desc.VendorId, desc.DeviceId);
        wchar_t found[16];
        std::swprintf(found, 16, L"%04x:%04x", desc.VendorId, desc.DeviceId);
        return !(desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE) && (!requested || !_wcsicmp(requested, found));
    };
    ComPtr<ID3D12Device> device;
    check(create(nullptr, device), "Scarlett device");
    ComPtr<IDXGIDevice1> dxgi;
    check(device.As(&dxgi), "Xbox device DXGI interface");
    ComPtr<IDXGIAdapter> adapter;
    check(dxgi->GetAdapter(&adapter), "Xbox device adapter");
    ComPtr<IDXGIAdapter1> adapter1;
    check(adapter.As(&adapter1), "Xbox device adapter interface");
    if (matches(adapter1.Get())) return device;
    ComPtr<IDXGIFactory1> factory;
    check(adapter->GetParent(IID_PPV_ARGS(&factory)), "Xbox adapter parent");
    for (UINT n = 0;; ++n) {
        ComPtr<IDXGIAdapter1> candidate;
        const HRESULT hr = factory->EnumAdapters1(n, &candidate);
        if (hr == DXGI_ERROR_NOT_FOUND) throw std::runtime_error("no matching Xbox D3D12 hardware adapter");
        check(hr, "adapter enumeration");
        if (!matches(candidate.Get())) continue;
        check(create(candidate.Get(), device), "Scarlett device on selected adapter");
        return device;
    }
}

std::string narrow(const wchar_t* text) {
    std::string result;
    while (*text) result += static_cast<char>(*text++);
    return result;
}
}

const wchar_t* const selftest_scenarios[] = {L"labels",            L"cross-queue",      L"compute-frame",
                                             L"skipped-command",   L"copy-memory",      L"shared-texture",
                                             L"indirect-dispatch", L"texture-readback", L"ray-tracing-builder",
                                             L"thread-dimensions", L"descriptor-reclaim",
                                             L"xbox-shader"};
const unsigned selftest_scenario_count = static_cast<unsigned>(std::size(selftest_scenarios));
static_assert(std::size(selftest_scenarios) == std::size(scenarios));

std::string run_selftest(const wchar_t* requested, const wchar_t* adapter) {
    const bool all = !std::wcscmp(requested, L"all");
    unsigned passed = 0;
    char pci[16]{};
    for (const auto& scenario : scenarios) {
        if (!all && std::wcscmp(requested, scenario.name)) continue;
        const auto name = narrow(scenario.name);
        try {
            auto device = create_scarlett_device(adapter, pci);
            scenario.run(device.Get());
        } catch (const std::exception& error) {
            throw std::runtime_error(name + ": " + error.what());
        }
        ++passed;
        // Progress, so a crash or a stuck later scenario can be located.
        std::fprintf(stderr, "lanes-selftest %s ok\n", name.c_str());
    }
    if (!passed) throw std::runtime_error("unknown scenario " + narrow(requested));
    return "lanes-selftest PASS pci=" + std::string(pci) + " scenario=" + narrow(requested) +
           " passed=" + std::to_string(passed);
}

std::string run_benchmark(const wchar_t* adapter) {
    char pci[16]{};
    Readings readings{};
    {
        auto device = create_scarlett_device(adapter, pci);
        readings = measure(device.Get());
    }
    // The game's own frame: paint the board, upload it through the Xbox command
    // list and read it back, without a window.
    std::string selected;
    auto device = create_lanes_device(adapter, selected);
    auto game = std::make_unique<Game>();
    generate_course(device.Get(), game->rows, 0x5194c0d1u);
    Screen screen(device.Get());
    const auto frames = timed_us(300, [&] {
        game->move(0);
        if (game->finish != Finish::playing) game->restart();
        screen.verify(*game);
    });
    char line[320];
    int used = std::snprintf(line, sizeof(line),
                             "lanes-benchmark PASS pci=%s submit_us=%.1f roundtrip_us=%.1f copy64k_us=%.1f "
                             "frame_us=%.1f frame_p99_us=%.1f",
                             pci, readings.submit_us, readings.roundtrip_us, readings.copy_us,
                             frames[frames.size() / 2], frames[frames.size() * 99 / 100]);
    if (readings.shader_us > 0)
        std::snprintf(line + used, sizeof(line) - used, " shader_pipeline_us=%.1f dxil_pipeline_us=%.1f",
                      readings.shader_us, readings.dxil_us);
    return line;
}
