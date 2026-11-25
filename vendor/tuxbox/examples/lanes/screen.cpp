#include "screen.h"
#include "gpu.h"
#include "game.h"
#include <algorithm>
#include <array>
#include <cstdint>
#include <cstring>
#include <string_view>

using frame_abi::check;
using frame_abi::method;
using Microsoft::WRL::ComPtr;

namespace {
constexpr int width = Screen::width, height = Screen::height;
constexpr int left = width / 18, right = width - left, top = 104, bottom = height - 88;
using Pixel = std::uint32_t;
// Bytes in upload memory are R, G, B, A.
constexpr Pixel rgb(unsigned r, unsigned g, unsigned b) {
    return 0xff000000u | b << 16 | g << 8 | r;
}
constexpr Pixel ink = rgb(21, 26, 38), paper = rgb(242, 245, 249), muted = rgb(166, 181, 199);
constexpr Pixel gold = rgb(251, 196, 74), red = rgb(250, 105, 88), teal = rgb(95, 235, 202);

// Five-bit rows for the game's uppercase pixel lettering.
using Glyph = std::array<unsigned char, 7>;
constexpr Glyph letters[] = {
    {14, 17, 17, 31, 17, 17, 17}, {30, 17, 17, 30, 17, 17, 30}, {14, 17, 16, 16, 16, 17, 14},
    {30, 17, 17, 17, 17, 17, 30}, {31, 16, 16, 30, 16, 16, 31}, {31, 16, 16, 30, 16, 16, 16},
    {14, 17, 16, 23, 17, 17, 15}, {17, 17, 17, 31, 17, 17, 17}, {14, 4, 4, 4, 4, 4, 14},
    {7, 2, 2, 2, 2, 18, 12},      {17, 18, 20, 24, 20, 18, 17}, {16, 16, 16, 16, 16, 16, 31},
    {17, 27, 21, 21, 17, 17, 17}, {17, 25, 25, 21, 19, 19, 17}, {14, 17, 17, 17, 17, 17, 14},
    {30, 17, 17, 30, 16, 16, 16}, {14, 17, 17, 17, 21, 18, 13}, {30, 17, 17, 30, 20, 18, 17},
    {15, 16, 16, 14, 1, 1, 30},   {31, 4, 4, 4, 4, 4, 4},       {17, 17, 17, 17, 17, 17, 14},
    {17, 17, 17, 17, 17, 10, 4},  {17, 17, 17, 21, 21, 21, 10}, {17, 17, 10, 4, 10, 17, 17},
    {17, 17, 10, 4, 4, 4, 4},     {31, 1, 2, 4, 8, 16, 31},
};
constexpr Glyph digits[] = {
    {14, 17, 19, 21, 25, 17, 14}, {4, 12, 4, 4, 4, 4, 14},  {14, 17, 1, 2, 4, 8, 31},
    {30, 1, 1, 14, 1, 1, 30},     {2, 6, 10, 18, 31, 2, 2}, {31, 16, 16, 30, 1, 1, 30},
    {14, 16, 16, 30, 17, 17, 14}, {31, 1, 2, 4, 8, 8, 8},   {14, 17, 17, 14, 17, 17, 14},
    {14, 17, 17, 15, 1, 1, 14},
};

Glyph glyph(char c) {
    if (c >= 'A' && c <= 'Z')
        return letters[c - 'A'];
    if (c >= '0' && c <= '9')
        return digits[c - '0'];
    switch (c) {
    case '/':
        return {1, 1, 2, 4, 8, 16, 16};
    case '-':
        return {0, 0, 0, 14, 0, 0, 0};
    default:
        return {};
    }
}

struct Canvas {
    std::array<Pixel, width * height> pixels;

    void rect(int x0, int y0, int x1, int y1, Pixel color) {
        x0 = std::max(0, x0);
        x1 = std::min(width, x1);
        if (x1 <= x0)
            return;
        for (int y = std::max(0, y0); y < std::min(height, y1); ++y)
            std::fill_n(pixels.data() + y * width + x0, x1 - x0, color);
    }
    void text(int x, int y, std::string_view value, int scale, Pixel color) {
        for (char c : value) {
            const auto shape = glyph(c);
            for (int row = 0; row < 7; ++row)
                for (int col = 0; col < 5; ++col)
                    if (shape[row] & (16 >> col))
                        rect(x + col * scale, y + row * scale, x + (col + 1) * scale, y + (row + 1) * scale, color);
            x += 6 * scale;
        }
    }
    static int text_width(std::string_view s, int scale) { return s.empty() ? 0 : (int(s.size()) * 6 - 1) * scale; }
    void centered(int y, std::string_view value, int scale, Pixel color) {
        text((width - text_width(value, scale)) / 2, y, value, scale, color);
    }
    void coin(int x, int y) {
        for (int dy = -13; dy <= 13; ++dy) {
            int radius = 13;
            while (radius * radius + dy * dy > 13 * 13)
                --radius;
            rect(x - radius, y + dy, x + radius + 1, y + dy + 1, gold);
        }
        rect(x - 2, y - 7, x + 2, y + 7, rgb(118, 76, 19));
    }
    void hazard(int x, int y) {
        for (int row = 0; row <= 28; ++row) {
            const int radius = row * 16 / 28;
            rect(x - radius, y - 16 + row, x + radius + 1, y - 15 + row, red);
        }
        rect(x - 2, y - 5, x + 2, y + 3, ink);
        rect(x - 2, y + 6, x + 2, y + 9, ink);
    }
    void player(int x) {
        for (int row = 0; row <= 27; ++row) {
            const int radius = row * 19 / 27;
            const int y = bottom - 37 + row;
            rect(x - radius, y, x + radius + 1, y + 1, teal);
            if (row > 21) {
                const int gap = (row - 21) * 19 / 6;
                rect(x - gap, y, x + gap + 1, y + 1, rgb(42, 64, 76));
            }
        }
    }
    void cursor(int x, int y) {
        // The top-left vertex is the mouse hotspot.
        for (int row = 0; row < 15; ++row) {
            const int edge = std::min(row, 12);
            rect(x, y + row, x + edge + 1, y + row + 1, ink);
            if (row > 1 && row < 13)
                rect(x + 1, y + row, x + edge, y + row + 1, paper);
        }
    }
    void paint(const Game& game, int x, int y, bool show_cursor) {
        pixels.fill(ink);
        text(left, 18, "LANES", 5, paper);
        char label[48];
        std::snprintf(label, sizeof(label), "%d / %u COINS", game.score, course_length);
        text(right - text_width(label, 2), 33, label, 2, gold);
        std::snprintf(label, sizeof(label), "TURN %u OF %u", std::min(game.turn + 1, course_length), course_length);
        text(left, 77, label, 2, muted);
        auto center = [](int lane) { return left + (2 * lane + 1) * (right - left) / 6; };
        for (int i = 0; i < 3; ++i)
            rect(left + i * (right - left) / 3 + 1, top, left + (i + 1) * (right - left) / 3 - 1, bottom,
                 i == game.lane ? rgb(42, 64, 76) : rgb(34, 42, 59));
        const int spacing = (bottom - top - 42) / 5;
        for (unsigned ahead = 0; ahead < 5 && game.turn + ahead < course_length; ++ahead) {
            auto row = game.rows[game.turn + ahead];
            int row_y = bottom - 42 - int(ahead) * spacing - spacing / 2;
            coin(center(row & 255), row_y);
            hazard(center(row >> 8), row_y);
        }
        player(center(game.lane));
        if (game.finish != Finish::playing) {
            const int mid = (top + bottom) / 2;
            rect(left, mid - 53, right, mid + 53, ink);
            centered(mid - 31, game.finish == Finish::won ? "COURSE COMPLETE" : "COLLISION", 3, paper);
            centered(mid + 11, "CLICK OR PRESS R OR START TO REPLAY", 2, muted);
        }
        centered(bottom + 10, "MOVE  ARROWS / D-PAD / STICK", 2, paper);
        centered(bottom + 32, "HOLD  SPACE / A    OR CLICK A LANE", 2, paper);
        centered(bottom + 58, "COLLECT COINS AND AVOID RED BARRIERS", 2, muted);
        if (show_cursor)
            cursor(x, y);
    }
};

constexpr UINT pitch =
    (width * sizeof(Pixel) + D3D12_TEXTURE_DATA_PITCH_ALIGNMENT - 1) & ~(D3D12_TEXTURE_DATA_PITCH_ALIGNMENT - 1);
constexpr UINT64 upload_bytes = UINT64(pitch) * height;
ComPtr<ID3D12Resource> screen_resource(ID3D12Device* device, D3D12_HEAP_TYPE type) {
    const bool buffer = type != D3D12_HEAP_TYPE_DEFAULT;
    D3D12_HEAP_PROPERTIES heap{};
    heap.Type = type;
    heap.CreationNodeMask = heap.VisibleNodeMask = 1;
    D3D12_RESOURCE_DESC desc{};
    desc.Dimension = buffer ? D3D12_RESOURCE_DIMENSION_BUFFER : D3D12_RESOURCE_DIMENSION_TEXTURE2D;
    desc.Width = buffer ? upload_bytes : width;
    desc.Height = buffer ? 1 : height;
    desc.DepthOrArraySize = desc.MipLevels = 1;
    desc.SampleDesc.Count = 1;
    desc.Format = buffer ? DXGI_FORMAT_UNKNOWN : DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.Layout = buffer ? D3D12_TEXTURE_LAYOUT_ROW_MAJOR : D3D12_TEXTURE_LAYOUT_UNKNOWN;
    auto initial = D3D12_RESOURCE_STATE_COMMON;
    if (type == D3D12_HEAP_TYPE_UPLOAD)
        initial = D3D12_RESOURCE_STATE_GENERIC_READ;
    if (type == D3D12_HEAP_TYPE_READBACK)
        initial = D3D12_RESOURCE_STATE_COPY_DEST;
    ComPtr<ID3D12Resource> resource;
    check(device->CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, initial, nullptr,
                                          IID_PPV_ARGS(&resource)),
          "screen resource");
    return resource;
}
} // namespace

int Screen::lane_at(int x, int y) {
    if (x < left || x >= right || y < top || y >= bottom)
        return -1;
    return 3 * (x - left) / (right - left);
}

struct Screen::Impl {
    Canvas canvas;
    ComPtr<ID3D12CommandQueue> queue;
    ComPtr<ID3D12CommandAllocator> allocator;
    ComPtr<ID3D12GraphicsCommandList> list;
    ComPtr<ID3D12Fence> fence;
    ComPtr<ID3D12Resource> staging, texture, readback;
    UINT64 submitted = 0, presents = 0;
    bool unfenced = false;

    explicit Impl(ID3D12Device* device) {
        queue = create_lanes_queue(device, D3D12_COMMAND_LIST_TYPE_DIRECT);
        check(device->CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT, IID_PPV_ARGS(&allocator)),
              "screen allocator");
        list = create_lanes_list(device, D3D12_COMMAND_LIST_TYPE_DIRECT, allocator.Get());
        check(device->CreateFence(0, D3D12_FENCE_FLAG_NONE, IID_PPV_ARGS(&fence)), "screen fence");
        staging = screen_resource(device, D3D12_HEAP_TYPE_UPLOAD);
        texture = screen_resource(device, D3D12_HEAP_TYPE_DEFAULT);
    }

    HRESULT wait_for_upload() {
        if (unfenced) {
            HRESULT hr = queue->Signal(fence.Get(), submitted);
            if (FAILED(hr))
                return hr;
            unfenced = false;
        }
        const auto completed = fence->GetCompletedValue();
        if (completed == UINT64_MAX)
            return DXGI_ERROR_DEVICE_REMOVED;
        if (completed >= submitted)
            return S_OK;
        return fence->SetEventOnCompletion(submitted, nullptr);
    }

    void copy_to_texture(bool capture = false) {
        check(wait_for_upload(), "screen upload completion");
        if (capture && !readback) {
            ComPtr<ID3D12Device> device;
            check(texture->GetDevice(IID_PPV_ARGS(&device)), "screen texture device");
            readback = screen_resource(device.Get(), D3D12_HEAP_TYPE_READBACK);
        }
        void* bytes = nullptr;
        const D3D12_RANGE no_reads{};
        check(staging->Map(0, &no_reads, &bytes), "screen upload map");
        for (int row = 0; row < height; ++row)
            std::memcpy(static_cast<BYTE*>(bytes) + row * pitch, canvas.pixels.data() + row * width,
                        width * sizeof(Pixel));
        staging->Unmap(0, nullptr);
        check(allocator->Reset(), "screen allocator reset");
        check(list->Reset(allocator.Get(), nullptr), "screen list reset");
        D3D12_RESOURCE_BARRIER barrier{};
        barrier.Type = D3D12_RESOURCE_BARRIER_TYPE_TRANSITION;
        barrier.Transition = {texture.Get(), 0, D3D12_RESOURCE_STATE_COMMON, D3D12_RESOURCE_STATE_COPY_DEST};
        list->ResourceBarrier(1, &barrier);
        D3D12_TEXTURE_COPY_LOCATION src{}, dst{};
        src.pResource = staging.Get();
        src.Type = D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT;
        src.PlacedFootprint.Footprint = {DXGI_FORMAT_R8G8B8A8_UNORM, width, height, 1, pitch};
        dst.pResource = texture.Get();
        dst.Type = D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX;
        const auto copy = method<frame_abi::Copy>(list.Get(), 83);
        copy(list.Get(), &dst, 0, 0, 0, &src, nullptr, 0);
        barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_COPY_DEST;
        if (capture) {
            barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COPY_SOURCE;
            list->ResourceBarrier(1, &barrier);
            auto snapshot = src;
            snapshot.pResource = readback.Get();
            copy(list.Get(), &snapshot, 0, 0, 0, &dst, nullptr, 0);
            barrier.Transition.StateBefore = D3D12_RESOURCE_STATE_COPY_SOURCE;
        }
        barrier.Transition.StateAfter = D3D12_RESOURCE_STATE_COMMON;
        list->ResourceBarrier(1, &barrier);
        check(list->Close(), "screen list close");
        ID3D12CommandList* batch[]{list.Get()};
        queue->ExecuteCommandLists(1, batch);
        ++submitted;
        unfenced = true;
        check(queue->Signal(fence.Get(), submitted), "screen upload signal");
        unfenced = false;
    }
};

Screen::Screen(ID3D12Device* device) : state_(std::make_unique<Impl>(device)) {}

Screen::~Screen() {
    // On an unretirable submission the queue can still be using these objects.
    if (FAILED(state_->wait_for_upload()) && state_->fence->GetCompletedValue() != UINT64_MAX)
        state_.release();
}

bool Screen::draw(const Game& game, int cursor_x, int cursor_y, bool cursor_visible, bool dirty) {
    auto& s = *state_;
    if (dirty || !s.submitted) {
        s.canvas.paint(game, cursor_x, cursor_y, cursor_visible);
        s.copy_to_texture();
    }
    auto* texture = s.texture.Get();
    frame_abi::Plane plane{++s.presents, 1, 0, &texture};
    const HRESULT hr = method<frame_abi::Present>(s.queue.Get(), 30)(s.queue.Get(), 1, &plane, nullptr);
    if (hr == DXGI_ERROR_ACCESS_LOST)
        return false;
    check(hr, "Xbox PresentX");
    if (hr == DXGI_STATUS_OCCLUDED)
        Sleep(50);
    return true;
}

void Screen::verify(const Game& game) {
    auto& s = *state_;
    s.canvas.paint(game, 0, 0, false);
    s.copy_to_texture(true);
    check(s.wait_for_upload(), "screen readback completion");
    const D3D12_RANGE reads{0, upload_bytes}, no_writes{};
    void* mapped = nullptr;
    check(s.readback->Map(0, &reads, &mapped), "screen readback map");
    const auto* bytes = static_cast<const BYTE*>(mapped);
    bool matches = true;
    for (int y = 0; matches && y < height; ++y)
        matches = std::memcmp(bytes + y * pitch, s.canvas.pixels.data() + y * width, width * sizeof(Pixel)) == 0;
    // Sample the readback independently of the CPU canvas. A blank board or a
    // stale player position must fail even if both copies contain the same bytes.
    const auto pixel = [bytes](int x, int y) {
        Pixel value;
        std::memcpy(&value, bytes + y * pitch + x * sizeof(Pixel), sizeof(value));
        return value;
    };
    for (int lane = 0; lane < 3; ++lane) {
        const int x = left + (2 * lane + 1) * (right - left) / 6;
        matches &= pixel(x, top + 3) == (game.lane == lane ? rgb(42, 64, 76) : rgb(34, 42, 59));
        if (game.lane == lane)
            matches &= pixel(x, bottom - 32) == teal;
    }
    s.readback->Unmap(0, &no_writes);
    if (!matches)
        throw std::runtime_error("GPU screen pixels differ from the Lanes board");
}
