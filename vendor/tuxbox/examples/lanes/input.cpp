// Lanes only sees GameInput readings. Window and host-device APIs belong to the provider.
#define NOMINMAX
#include "input.h"
#include <windows.h>
#include <GameInput.h>
#include <wrl/client.h>
#include <algorithm>
#include <array>
#include <climits>
#include <cstdio>
#include <stdexcept>
#include <string>

using Microsoft::WRL::ComPtr;
namespace {
enum Button : unsigned { left = 1, right = 2, hold = 4, restart = 8, quit = 16 };
struct Edges {
    unsigned held{};
    bool known{};
    unsigned update(unsigned current) {
        const unsigned pressed = known ? current & ~held : 0;
        known = true;
        held = current;
        return pressed;
    }
};
unsigned pad_edges(Edges& state, const GameInputGamepadState& pad) {
    unsigned buttons = 0;
    const float release = 10000.f / 32768.f, press = 16000.f / 32768.f;
    if ((pad.buttons & GameInputGamepadDPadLeft) || pad.leftThumbstickX < -(state.held & left ? release : press))
        buttons |= left;
    if ((pad.buttons & GameInputGamepadDPadRight) || pad.leftThumbstickX > (state.held & right ? release : press))
        buttons |= right;
    if (pad.buttons & GameInputGamepadA) buttons |= hold;
    if (pad.buttons & GameInputGamepadMenu) buttons |= restart;
    auto edges = state.update(buttons);
    if ((buttons & left) && (buttons & right)) edges &= ~(left | right);
    return edges;
}
void check(HRESULT hr, const char* operation) {
    if (SUCCEEDED(hr)) return;
    char result[16]; std::snprintf(result, sizeof(result), "0x%08lx", static_cast<unsigned long>(hr));
    throw std::runtime_error(std::string(operation) + " failed: " + result);
}
ComPtr<IGameInputReading> current(IGameInput* input, GameInputKind kind) {
    ComPtr<IGameInputReading> reading;
    const HRESULT hr = input->GetCurrentReading(kind, nullptr, &reading);
    if (hr == GAMEINPUT_E_READING_NOT_FOUND || hr == GAMEINPUT_E_DEVICE_DISCONNECTED) return {};
    check(hr, "GetCurrentReading");
    if (!reading || !(reading->GetInputKind() & kind)) throw std::runtime_error("invalid GameInput reading kind");
    return reading;
}
}

struct LanesInput::State {
    ComPtr<IGameInput> input;
    Edges keyboard;
    GameInputMouseState mouse{};
    bool have_mouse{};
    struct Port { ComPtr<IGameInputDevice> device; Edges controls; };
    std::array<Port, 4> pads{};
    ComPtr<IGameInputReading> last_pad;

    unsigned keys() {
        const auto reading = current(input.Get(), GameInputKindKeyboard);
        if (!reading) { keyboard = {}; return 0; }
        std::array<GameInputKeyState,256> keys{};
        const auto n = reading->GetKeyState(static_cast<UINT32>(keys.size()), keys.data());
        unsigned active = 0;
        for (unsigned i = 0; i < n; ++i) {
            switch (keys[i].virtualKey) {
            case VK_LEFT: active |= left; break;
            case VK_RIGHT: active |= right; break;
            case VK_SPACE: active |= hold; break;
            case VK_ESCAPE: active |= quit; break;
            case 'R': active |= restart; break;
            }
        }
        return keyboard.update(active);
    }

    void pointer(InputFrame& frame) {
        auto reading = current(input.Get(), GameInputKindMouse);
        GameInputMouseState now{};
        if (!reading || !reading->GetMouseState(&now)) { have_mouse = false; return; }
        if (have_mouse) {
            frame.mouse_dx = static_cast<int>(std::clamp<int64_t>(now.positionX - mouse.positionX, INT_MIN, INT_MAX));
            frame.mouse_dy = static_cast<int>(std::clamp<int64_t>(now.positionY - mouse.positionY, INT_MIN, INT_MAX));
            frame.click = (now.buttons & GameInputMouseLeftButton) && !(mouse.buttons & GameInputMouseLeftButton);
        }
        mouse = now; have_mouse = true;
    }

    unsigned pad_reading(IGameInputReading* reading) {
        GameInputGamepadState pad{};
        ComPtr<IGameInputDevice> device;
        reading->GetDevice(&device);
        if (!device || !(device->GetDeviceStatus() & GameInputDeviceInputEnabled) || !reading->GetGamepadState(&pad)) return 0;
        for (auto& port : pads)
            if (port.device.Get() == device.Get()) return pad_edges(port.controls, pad);
        for (auto& port : pads) if (!port.device) {
            port.device = device;
            return pad_edges(port.controls, pad);
        }
        return 0;
    }

    unsigned controllers() {
        auto latest = current(input.Get(), GameInputKindGamepad);
        for (auto& p : pads) if (p.device && !(p.device->GetDeviceStatus() & GameInputDeviceInputEnabled)) p = {};
        if (!latest) { last_pad.Reset(); return 0; }
        if (!last_pad) {
            // Seed all already connected ports. Holding a button on connection is neutral.
            auto reading = latest;
            for (;;) {
                ComPtr<IGameInputDevice> device;
                reading->GetDevice(&device);
                const auto known = std::any_of(pads.begin(), pads.end(), [&](const auto& p) { return p.device.Get() == device.Get(); });
                if (!known) pad_reading(reading.Get());
                ComPtr<IGameInputReading> previous;
                const HRESULT hr = input->GetPreviousReading(reading.Get(), GameInputKindGamepad, nullptr, &previous);
                if (hr == GAMEINPUT_E_READING_NOT_FOUND || hr == GAMEINPUT_E_REFERENCE_READING_TOO_OLD) break;
                check(hr, "GetPreviousReading");
                reading = std::move(previous);
            }
            last_pad = latest;
            return 0;
        }
        unsigned pressed = 0;
        while (last_pad.Get() != latest.Get()) {
            ComPtr<IGameInputReading> next;
            const HRESULT hr = input->GetNextReading(last_pad.Get(), GameInputKindGamepad, nullptr, &next);
            if (hr == GAMEINPUT_E_REFERENCE_READING_TOO_OLD || hr == GAMEINPUT_E_READING_NOT_FOUND) {
                pads = {}; last_pad.Reset();
                break;
            }
            check(hr, "GetNextReading");
            pressed |= pad_reading(next.Get());
            last_pad = std::move(next);
        }
        return pressed;
    }
};

LanesInput::LanesInput() : state(std::make_unique<State>()) {
    check(GameInputCreate(&state->input), "GameInputCreate");
    if (!state->input) throw std::runtime_error("empty GameInput interface");
    state->input->GetCurrentTimestamp();
}
LanesInput::~LanesInput() = default;

InputFrame LanesInput::poll() {
    InputFrame frame;
    const unsigned buttons = state->keys() | state->controllers();
    frame.left = buttons & left; frame.right = buttons & right;
    frame.hold = buttons & hold; frame.restart = buttons & restart;
    frame.quit = buttons & quit; state->pointer(frame);
    return frame;
}

void lanes_input_replay_check() {
    ComPtr<IGameInput> first, second;
    check(GameInputCreate(&first), "first replay GameInputCreate");
    check(GameInputCreate(&second), "second replay GameInputCreate");
    if (!first || second.Get() != first.Get())
        throw std::runtime_error("GameInput singleton identity differs");
    Edges port;
    GameInputGamepadState pad{};
    auto expect = [&](unsigned edges) {
        if (pad_edges(port, pad) != edges) throw std::runtime_error("gamepad edge or hysteresis regression");
    };
    pad.buttons = GameInputGamepadA; expect(0);
    pad.buttons = GameInputGamepadDPadRight; expect(right);
    pad.buttons |= GameInputGamepadA; expect(hold);
    pad.buttons = GameInputGamepadDPadRight; expect(0);
    pad.buttons = GameInputGamepadNone;
    pad.leftThumbstickX = -20000.f / 32768; expect(left);
    pad.leftThumbstickX = -14000.f / 32768; expect(0);
    pad.leftThumbstickX = -20000.f / 32768; expect(0);
}
