// Verify the visible window in screen coordinates, independently of GPU staging readbacks.
#include "frame_abi.h"
#include "frame_present.h"
#include <stdexcept>
#include <cstdio>
#include <future>

namespace tuxbox_test {
namespace {
void pump() {
    MSG message;
    while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
        TranslateMessage(&message);
        DispatchMessageW(&message);
    }
}
}

DisplayCheck::DisplayCheck(UINT width, UINT height, const wchar_t* window_mode)
    : check_size_(window_mode != nullptr) {
    if (window_mode && !wcscmp(window_mode, L"owned")) return;
    UINT client_width = width * 6, client_height = height * 6;
    if (check_size_) {
        client_width = !wcscmp(window_mode, L"fit") ? 960 : width;
        client_height = !wcscmp(window_mode, L"fit") ? 540 : height;
    }
    // The popup's client area fills the window. Keep it above other windows while sampling.
    window_ = CreateWindowExW(WS_EX_TOPMOST, L"STATIC", L"Project TuxBox GPU check",
                              WS_POPUP, 24, 24, client_width, client_height, nullptr, nullptr,
                              GetModuleHandleW(nullptr), nullptr);
    if (!window_) throw std::runtime_error("could not create presentation window");
    owns_title_ = true;
    ShowWindow(window_, SW_SHOW);
    SetActiveWindow(window_);
    UpdateWindow(window_);
    pump();
}
DisplayCheck::~DisplayCheck() { if (owns_title_) DestroyWindow(window_); }

void DisplayCheck::verify(ID3D12CommandQueue* queue, ID3D12Resource* source, UINT64 token,
                          COLORREF corner, COLORREF center) {
    struct FullPlane {
        frame_abi::Plane prefix;
        UINT options[12];
    } plane{{token, 1, 0, &source}, {}};
    struct Parameters {
        float threshold;
        UINT options[7];
    } params{token == 1 ? 10.f : 0.f, {}};
    static_assert(sizeof(FullPlane) == 72 && sizeof(Parameters) == 32);
    const auto present = frame_abi::method<frame_abi::Present>(queue, 30);
    if (token == 1) {
        params.options[5] = 1;
        if (present(queue, 1, &plane.prefix, &params) != E_NOTIMPL)
            throw std::runtime_error("unsupported presentation flag was accepted");
        params.options[5] = 0;
        // Bytes 24 and 32 of the 72-byte retail plane are the source-rectangle and
        // destination pointers; the provider dereferences them. Probe the color
        // space at byte 40 instead, a plain value it must reject when nonzero.
        plane.options[4] = 1;
        if (present(queue, 1, &plane.prefix, &params) != E_NOTIMPL)
            throw std::runtime_error("unsupported plane option was accepted");
        plane.options[4] = 0;
    }
    // Frosty's main thread owns the game window while a render thread presents.
    auto render = std::async(std::launch::async, [&] { return present(queue, 1, &plane.prefix, &params); });
    while (render.wait_for(std::chrono::milliseconds(1)) != std::future_status::ready) pump();
    HRESULT hr = render.get();
    if (hr != S_OK) {
        char message[96];
        std::snprintf(message, sizeof(message), "queue slot 30 PresentX returned %#lx", static_cast<unsigned long>(hr));
        throw std::runtime_error(message);
    }
    if (!window_) {
        EnumWindows([](HWND w, LPARAM data) -> BOOL {
            DWORD pid{};
            GetWindowThreadProcessId(w, &pid);
            if (pid != GetCurrentProcessId() || !IsWindowVisible(w) || GetWindow(w, GW_OWNER)) return TRUE;
            *reinterpret_cast<HWND*>(data) = w;
            return FALSE;
        }, reinterpret_cast<LPARAM>(&window_));
    }
    RECT client{};
    if (!GetClientRect(window_, &client)) throw std::runtime_error("could not get presentation client area");
    POINT origin{}, end{client.right, client.bottom};
    if (!ClientToScreen(window_, &origin) || !ClientToScreen(window_, &end))
        throw std::runtime_error("could not locate presentation window");
    if (check_size_) {
        MONITORINFO monitor{};
        monitor.cbSize = sizeof(monitor);
        if (!GetMonitorInfoW(MonitorFromWindow(window_, MONITOR_DEFAULTTONEAREST), &monitor))
            throw std::runtime_error("could not locate display work area");
        const auto& area = monitor.rcWork;
        sizing_ = " client=" + std::to_string(client.right) + "x" + std::to_string(client.bottom)
            + " origin=" + std::to_string(origin.x) + "," + std::to_string(origin.y)
            + " monitor=" + std::to_string(area.right - area.left) + "x" + std::to_string(area.bottom - area.top);
        if (origin.x < area.left || origin.y < area.top || end.x > area.right || end.y > area.bottom)
            throw std::runtime_error("off-screen presentation" + sizing_);
        if (std::abs(client.right * 9 - client.bottom * 16) > 16)
            throw std::runtime_error("distorted presentation" + sizing_);
    }
    const auto deadline = GetTickCount64() + 4000;
    const std::array<POINT, 5> samples{{{5, 5}, {95, 5}, {5, 95}, {95, 95}, {50, 50}}};
    std::array<COLORREF, samples.size()> pixels{};
    do {
        pump();
        HDC screen = GetDC(nullptr);
        if (!screen) throw std::runtime_error("could not acquire screen DC");
        bool matched = true;
        for (size_t i = 0; i < pixels.size(); ++i) {
            pixels[i] = GetPixel(screen, origin.x + client.right * samples[i].x / 100,
                                 origin.y + client.bottom * samples[i].y / 100);
            matched &= pixels[i] == (i == 4 ? center : corner);
        }
        ReleaseDC(nullptr, screen);
        if (matched) return;
        Sleep(16);
    } while (GetTickCount64() < deadline);
    char message[180];
    std::snprintf(message, sizeof(message), "screen colors on frame %llu: %#lx/%#lx/%#lx/%#lx/%#lx, expected %#lx border and %#lx center",
                  token, pixels[0], pixels[1], pixels[2], pixels[3], pixels[4], corner, center);
    throw std::runtime_error(message + sizing_);
}
}
