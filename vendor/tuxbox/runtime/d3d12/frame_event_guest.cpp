// Frosty's origin poll and duplicated display-flip event, across real PresentX calls.
#include "frame_abi.h"
#include "frame_cli.h"
#include <cstring>

namespace {
using namespace frame_abi;
struct SignalList { UINT count; const HANDLE* handles; };
static_assert(offsetof(SignalList, handles) == 8);
using Interval = HRESULT (WINAPI*)(ID3D12Device*, IDXGIOutput*, UINT, UINT, UINT);
using Schedule = HRESULT (WINAPI*)(ID3D12Device*, UINT, UINT, const SignalList*, UINT);
// The packaged driver reserves 0x38 bytes per statistic. On a display record
// plane zero and the two QPC fields precede the scan percentage.
struct DisplayStat {
    UINT type, reserved;
    UINT8 plane, padding[7];
    INT64 vsync, flip;
    float percent;
    UINT8 tail[20];
};
static_assert(sizeof(DisplayStat) == 56 && offsetof(DisplayStat, percent) == 32);
using Statistics = HRESULT (WINAPI*)(ID3D12Device*, UINT64, UINT, UINT*, DisplayStat*);
using Wait = HRESULT (WINAPI*)(ID3D12Device*, UINT, UINT, const void*, UINT, UINT64*);

void require(bool ok, const char* label) { if (!ok) throw std::runtime_error(label); }

std::string run(const wchar_t* requested, bool) {
    DXGI_ADAPTER_DESC1 selected{};
    auto device = create_device(requested, selected);
    auto interval = method<Interval>(device.Get(), 78);
    auto schedule = method<Schedule>(device.Get(), 79);
    auto wait = method<Wait>(device.Get(), 80);
    auto query = method<Statistics>(device.Get(), 81);
    constexpr UINT display = 0x10000, render = 0x100;
    struct Protected { DisplayStat data[2]; UINT64 guard; } stat{};
    stat.guard = 0xf032be110794ac25ULL;
    UINT count = 1;
    ComPtr<IDXGIDevice> dxgi;
    ComPtr<IDXGIAdapter> adapter;
    ComPtr<IDXGIOutput> output;
    check(device.As(&dxgi), "frame DXGI device");
    check(dxgi->GetAdapter(&adapter), "frame adapter");
    auto found = adapter->EnumOutputs(0, &output);
    if (found != DXGI_ERROR_NOT_FOUND) check(found, "first output");

    ComPtr<ID3D12CommandQueue> queue;
    D3D12_COMMAND_QUEUE_DESC desc{};
    check(device->CreateCommandQueue(&desc, IID_PPV_ARGS(&queue)), "present queue");
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = D3D12_HEAP_TYPE_DEFAULT;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC image{};
    image.Dimension = D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    image.Width = image.Height = 64;
    image.DepthOrArraySize = 1;
    image.MipLevels = 1;
    image.SampleDesc.Count = 1;
    image.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    image.Flags = D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET;
    ComPtr<ID3D12Resource> texture;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &image, D3D12_RESOURCE_STATE_PRESENT,
                                          nullptr, IID_PPV_ARGS(&texture)), "display surface");
    ID3D12Resource* surface = texture.Get();
    auto flip = [&](UINT64 token) {
        Plane plane{token, 1, 0, &surface};
        require(method<Present>(queue.Get(), 30)(queue.Get(), 1, &plane, nullptr) == S_OK, "display frame");
    };

    UINT64 token = 0;
    require(wait(device.Get(), 0, 0, nullptr, 0, &token) == E_INVALIDARG && !token, "unscheduled origin");
    require(interval(device.Get(), output.Get(), 16667, 5, 0) == E_INVALIDARG, "invalid frame period");
    check(interval(device.Get(), output.Get(), 66667, 1, 0), "frame interval");
    require(schedule(device.Get(), 2, 0, nullptr, 0) == E_INVALIDARG, "flip before origin");
    require(schedule(device.Get(), 0, 66667, nullptr, 0) == E_INVALIDARG, "origin outside interval");
    check(schedule(device.Get(), 0, 2000, nullptr, 0), "schedule origin");
    const auto started = GetTickCount64();
    require(wait(device.Get(), 0, 0, nullptr, 0, &token) == S_FALSE && !token, "poll a future origin");
    require(wait(device.Get(), 0, 0, nullptr, 2, &token) == S_FALSE && token, "process token on timeout");
    const auto first = token;
    require(wait(device.Get(), 0, 0, nullptr, 2, &token) == E_INVALIDARG &&
            token == first, "invalid repeat process-token poll");
    require(wait(device.Get(), 0, 2000, nullptr, 0, &token) == S_OK &&
            token == first && GetTickCount64() - started >= 30, "blocking wait on pending token");
    require(wait(device.Get(), 0, 0, nullptr, 0, &token) == E_INVALIDARG, "consume origin twice");
    UINT64 next = 0;
    require(wait(device.Get(), 0, 0, nullptr, 0, &next) == S_FALSE && !next, "frame latency window");

    HANDLE observer = CreateEventW(nullptr, TRUE, FALSE, nullptr);
    require(observer != nullptr, "create flip event");
    HANDLE borrowed{};
    require(DuplicateHandle(GetCurrentProcess(), observer, GetCurrentProcess(), &borrowed,
                            0, FALSE, DUPLICATE_SAME_ACCESS) != 0, "borrowed signal event");
    SignalList signals{1, &borrowed};
    check(schedule(device.Get(), 2, 0, &signals, 0), "schedule display flip");
    UINT64 display_token = first;
    require(wait(device.Get(), 2, 0, nullptr, 0, &display_token) == S_FALSE &&
            display_token == first, "pending display token");
    CloseHandle(borrowed);
    borrowed = nullptr; // The driver must have copied the list and duplicated the handle.
    require(WaitForSingleObject(observer, 0) == WAIT_TIMEOUT, "early flip notification");
    require(query(device.Get(), first, display, &count, stat.data) == S_FALSE && count == 0,
            "statistics before first present");
    LARGE_INTEGER before{}, after{};
    require(QueryPerformanceCounter(&before), "clock before present");
    flip(first);
    require(wait(device.Get(), 2, 0, nullptr, 0, &display_token) == S_OK &&
            display_token == first, "completed display token");
    require(QueryPerformanceCounter(&after), "clock after present");
    std::memset(stat.data, 0xcc, sizeof(stat.data));
    count = 2;
    require(query(device.Get(), first, display, &count, stat.data) == S_OK && count == 1,
            "completed display statistic");
    const auto first_stat = stat.data[0];
    const DisplayStat empty{};
    require(first_stat.type == display && first_stat.plane == 0 && first_stat.vsync >= before.QuadPart &&
            first_stat.flip >= first_stat.vsync && first_stat.flip <= after.QuadPart &&
            first_stat.percent == 0 && std::memcmp(&stat.data[1], &empty, sizeof(empty)) == 0,
            "display timing and record layout");
    require(stat.guard == 0xf032be110794ac25ULL, "statistics overwrote caller's buffer");
    require(WaitForSingleObject(observer, 1000) == WAIT_OBJECT_0, "display flip notification");
    check(schedule(device.Get(), 2, 0, nullptr, 1), "detach flip event");
    ResetEvent(observer);
    require(wait(device.Get(), 0, 2000, nullptr, 0, &next) == S_OK && next > first, "next origin token");
    flip(next);
    count = 1;
    require(query(device.Get(), next, display | render, &count, stat.data) == S_OK && count == 1
            && stat.data[0].type == display && stat.data[0].flip >= first_stat.flip, "second display timing");
    count = 1;
    require(query(device.Get(), first, display, &count, stat.data) == S_OK && count == 1 &&
            std::memcmp(&first_stat, stat.data, sizeof(first_stat)) == 0, "retained first display timing");
    count = 1;
    require(query(device.Get(), next, render, &count, stat.data) == S_FALSE && count == 0, "hardware duration unavailable");
    require(query(device.Get(), next, display, &count, nullptr) == S_OK && count == 1, "display statistics count query");
    count = 1;
    require(query(device.Get(), next, 0x80000000, &count, stat.data) == E_INVALIDARG, "invalid statistic class");
    require(WaitForSingleObject(observer, 0) == WAIT_TIMEOUT, "detached flip event signaled");
    CloseHandle(observer);
    auto other = create_device(requested, selected);
    UINT64 separate = 0;
    require(wait(other.Get(), 0, 0, nullptr, 0, &separate) == E_INVALIDARG && !separate,
            "frame events leaked between devices");
    count = 1;
    require(query(other.Get(), first, display, &count, stat.data) == S_FALSE && count == 0, "statistics leaked between devices");
    char receipt[160];
    std::snprintf(receipt, sizeof(receipt),
                  "d3d12-frame-events PASS pci=%04x:%04x slots=78,79,80,81 tokens=2 timeout=S_FALSE flip=1 detached=1 stats=2",
                  selected.VendorId, selected.DeviceId);
    return receipt;
}
}

int wmain(int argc, wchar_t** argv) {
    return tuxbox_test::frame_main(argc, argv, "d3d12-frame-events", false, run);
}
