#include <windows.h>
#include <GameInput.h>
#include <Xinput.h>
#include <wrl/client.h>
#include <atomic>
#include <cmath>
#include <cstdio>
#include <initializer_list>
#include <stdexcept>

using Microsoft::WRL::ComPtr;

static void require(bool ok, const char* message) {
    if (!ok) throw std::runtime_error(message);
}

template<class Condition>
static void wait_for(const char* description, Condition condition) {
    const auto deadline = GetTickCount64() + 14000;
    do {
        MSG message;
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        if (condition()) return;
        Sleep(8);
    } while (GetTickCount64() < deadline);
    throw std::runtime_error(description);
}

static void send_primary(bool mouse, bool held) {
    INPUT event{};
    if (mouse) {
        event.type = INPUT_MOUSE;
        event.mi.dwFlags = held ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP;
    } else {
        event.type = INPUT_KEYBOARD;
        event.ki.wVk = 'Z';
        event.ki.dwFlags = held ? 0 : KEYEVENTF_KEYUP;
    }
    require(SendInput(1, &event, sizeof(event)) == 1, "SendInput");
}

static ComPtr<IGameInputReading> current(IGameInput* input, IGameInputDevice* device,
                                         GameInputGamepadState& state) {
    ComPtr<IGameInputReading> reading;
    if (SUCCEEDED(input->GetCurrentReading(GameInputKindGamepad, device, &reading)) &&
        reading->GetGamepadState(&state)) return reading;
    return {};
}

struct Changes {
    std::atomic<unsigned> connected{}, disconnected{};
};
static void CALLBACK on_status(GameInputCallbackToken, void* ctx, IGameInputDevice*, uint64_t,
                               GameInputDeviceStatus current, GameInputDeviceStatus previous) {
    auto& changes = *static_cast<Changes*>(ctx);
    if ((current ^ previous) & GameInputDeviceConnected) {
        if (current & GameInputDeviceConnected) ++changes.connected;
        else ++changes.disconnected;
    }
}

int wmain(int argc, wchar_t** argv) {
    FILE* out{};
    if (argc != 2 || _wfopen_s(&out, argv[1], L"w")) return 2;
    setvbuf(out, nullptr, _IONBF, 0);
    try {
        auto module = LoadLibraryW(L"GameInput.dll");
        auto create = reinterpret_cast<decltype(&GameInputCreate)>(GetProcAddress(module, "GameInputCreate"));
        ComPtr<IGameInput> input;
        require(create && SUCCEEDED(create(&input)), "GameInputCreate");
        WNDCLASSW cls{};
        cls.lpfnWndProc = DefWindowProcW;
        cls.hInstance = GetModuleHandleW(nullptr);
        cls.lpszClassName = L"TuxBoxXInputProbe";
        require(RegisterClassW(&cls) != 0, "register window");
        auto window = CreateWindowW(cls.lpszClassName, L"TuxBox XInput coexistence", WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                                    10, 10, 350, 220, nullptr, nullptr, cls.hInstance, nullptr);
        require(window != nullptr, "create window");
        wait_for("window focus", [&] { return GetForegroundWindow() == window; });

        GameInputGamepadState state{};
        ComPtr<IGameInputDevice> keyboard_pad;
        auto keyboard_available = [&] {
            auto reading = current(input.Get(), nullptr, state);
            if (!reading) return false;
            ComPtr<IGameInputDevice> device;
            reading->GetDevice(&device);
            if (!keyboard_pad) keyboard_pad = device;
            return device.Get() == keyboard_pad.Get();
        };
        wait_for("keyboard before host gamepad", keyboard_available);
        Changes changes{};
        GameInputCallbackToken token{};
        require(SUCCEEDED(input->RegisterDeviceCallback(nullptr, GameInputKindGamepad, GameInputDeviceConnected,
            GameInputBlockingEnumeration, &changes, on_status, &token)) && changes.connected == 1, "initial pad callback");
        std::fputs("connect\n", out);
        DWORD slot = XUSER_MAX_COUNT;
        XINPUT_STATE raw{};
        wait_for("XInput hotplug", [&] {
            for (DWORD index = 0; index < XUSER_MAX_COUNT; ++index) {
                if (XInputGetState(index, &raw) == ERROR_SUCCESS) {
                    slot = index;
                    return true;
                }
            }
            return false;
        });
        ComPtr<IGameInputDevice> physical;
        wait_for("matching GameInput hardware port", [&] {
            auto reading = current(input.Get(), nullptr, state);
            if (!reading) return false;
            reading->GetDevice(&physical);
            return physical.Get() != keyboard_pad.Get() && (physical->GetDeviceStatus() & GameInputDeviceConnected);
        });
        wait_for("physical connection and virtual disconnection", [&] { return changes.connected == 2 && changes.disconnected == 1; });
        std::fprintf(out, "slot=%lu\nhold\n", slot);
        wait_for("Linux B, left stick and trigger in XInput", [&] {
            return XInputGetState(slot, &raw) == ERROR_SUCCESS && raw.Gamepad.wButtons == XINPUT_GAMEPAD_B &&
                   raw.Gamepad.sThumbLX > 10000 && raw.Gamepad.bRightTrigger > 100;
        });
        const float x = raw.Gamepad.sThumbLX / 32767.f;
        const float trigger = raw.Gamepad.bRightTrigger / 255.f;
        auto matches = [&](GameInputGamepadButtons b) {
            return state.buttons == b && std::abs(state.leftThumbstickX - x) < 0.0001f &&
                   std::abs(state.rightTrigger - trigger) < 0.0001f;
        };
        ComPtr<IGameInputReading> previous;
        wait_for("GameInput parity", [&] {
            previous = current(input.Get(), physical.Get(), state);
            return previous && matches(GameInputGamepadB);
        });
        auto check_primary = [&](bool mouse, bool held) {
            std::fprintf(out, "%s_%s\n", mouse ? "left_click" : "Z", held ? "pressed" : "released");
            send_primary(mouse, held);
            ComPtr<IGameInputReading> now;
            wait_for("combined desktop and hardware", [&] {
                now = current(input.Get(), physical.Get(), state);
                return now && matches(held ? GameInputGamepadA | GameInputGamepadB : GameInputGamepadB);
            });
            ComPtr<IGameInputReading> next;
            require(SUCCEEDED(input->GetNextReading(previous.Get(), GameInputKindGamepad, physical.Get(), &next)) &&
                    next.Get() == now.Get(), "ordered combined state on physical device");
            require(XInputGetState(slot, &raw) == ERROR_SUCCESS && raw.Gamepad.wButtons == XINPUT_GAMEPAD_B,
                    "independent XInput stream");
            previous = now;
        };
        for (bool mouse : {false, true}) {
            if (mouse) {
                POINT inside{40, 40};
                require(ClientToScreen(window, &inside) && SetCursorPos(inside.x, inside.y), "position mouse");
            }
            check_primary(mouse, true);
            check_primary(mouse, false);
        }
        std::fprintf(out, "raw_lx=%d\nraw_rt=%u\nrelease\n", raw.Gamepad.sThumbLX, raw.Gamepad.bRightTrigger);
        wait_for("neutral", [&] {
            return XInputGetState(slot, &raw) == ERROR_SUCCESS && raw.Gamepad.wButtons == 0 &&
                   raw.Gamepad.sThumbLX == 0 && raw.Gamepad.bRightTrigger == 0 &&
                   current(input.Get(), physical.Get(), state) && state.buttons == GameInputGamepadNone &&
                   state.leftThumbstickX == 0.f && state.rightTrigger == 0.f;
        });
        std::fputs("unplug\n", out);
        wait_for("unplug and keyboard fallback", [&] {
            return XInputGetState(slot, &raw) == ERROR_DEVICE_NOT_CONNECTED &&
                   !(physical->GetDeviceStatus() & GameInputDeviceConnected) && keyboard_available();
        });
        wait_for("physical disconnection and virtual reconnection", [&] { return changes.connected == 3 && changes.disconnected == 2; });
        require(input->UnregisterCallback(token, 1000000), "unregister device changes");
        std::fputs("callback_connected=3\ncallback_disconnected=2\npassed\n", out);
    } catch (const std::exception& error) {
        std::fprintf(out, "error=%s\n", error.what());
        fclose(out);
        return 1;
    }
    fclose(out);
    return 0;
}
