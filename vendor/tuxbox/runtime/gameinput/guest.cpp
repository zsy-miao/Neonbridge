// Run the input-injection control on a private desktop.
#define NOMINMAX
#include <windows.h>
#include <GameInput.h>
#include <wrl/client.h>
#include <array>
#include <cstdio>
#include <cwchar>
#include <stdexcept>

using Microsoft::WRL::ComPtr;
namespace {
void require(bool ok, const char* message) { if (!ok) throw std::runtime_error(message); }
template<class Test> void wait_for(Test done, const char* failure) {
    const auto limit = GetTickCount64() + 5000;
    do {
        MSG msg;
        while (PeekMessageW(&msg, nullptr, 0, 0, PM_REMOVE)) { TranslateMessage(&msg); DispatchMessageW(&msg); }
        if (done()) return;
        Sleep(8);
    } while (GetTickCount64() < limit);
    throw std::runtime_error(failure);
}
ComPtr<IGameInputReading> read(IGameInput* input, GameInputKind kind) {
    ComPtr<IGameInputReading> reading;
    require(SUCCEEDED(input->GetCurrentReading(kind, nullptr, &reading)) && reading &&
            reading->GetInputKind() == kind, "read named input kind");
    return reading;
}
void send(bool pointer, bool down) {
    INPUT event{};
    if (pointer) { event.type = INPUT_MOUSE; event.mi.dwFlags = down ? MOUSEEVENTF_LEFTDOWN : MOUSEEVENTF_LEFTUP; }
    else { event.type = INPUT_KEYBOARD; event.ki.wVk = 'R'; event.ki.dwFlags = down ? 0 : KEYEVENTF_KEYUP; }
    require(SendInput(1, &event, sizeof(event)) == 1, "inject input");
}
bool has_r(IGameInputReading* reading) {
    std::array<GameInputKeyState,256> keys{};
    auto n = reading->GetKeyState(static_cast<uint32_t>(keys.size()),keys.data());
    require(n == reading->GetKeyCount() && n <= keys.size(), "keyboard count");
    for (unsigned i = 0; i < n; ++i)
        if (keys[i].virtualKey == 'R') { require(keys[i].scanCode != 0, "R scan code"); return true; }
    return false;
}
struct DeviceEvent {
    IGameInput* input;
    IGameInputDevice* expected{};
    GameInputCallbackToken token{};
    volatile LONG count{};
    bool valid{};
};
void CALLBACK arrived(GameInputCallbackToken token, void* ctx, IGameInputDevice* device, uint64_t time,
                      GameInputDeviceStatus current, GameInputDeviceStatus previous) {
    auto& event = *static_cast<DeviceEvent*>(ctx);
    event.valid = token != 0 && time != 0 && (current & GameInputDeviceConnected) && !previous &&
        device->GetDeviceInfo()->supportedInput == GameInputKindGamepad &&
        (device->GetDeviceStatus() & GameInputDeviceConnected) && (!event.expected || event.expected == device);
    event.input->StopCallback(token);
    InterlockedIncrement(&event.count);
}
void CALLBACK enumerate_kinds(GameInputCallbackToken, void* ctx, IGameInputDevice* device, uint64_t,
                              GameInputDeviceStatus current, GameInputDeviceStatus previous) {
    if ((current & GameInputDeviceConnected) && !previous)
        *static_cast<GameInputKind*>(ctx) |= device->GetDeviceInfo()->supportedInput;
}
void run() {
    require(SetEnvironmentVariableW(L"TUXBOX_KEYBOARD_GAMEPAD", L"1") != 0, "enable desktop gamepad");
    auto module = GetModuleHandleW(L"GameInput.dll");
    require(module && GetProcAddress(module,"GameInputCreate") == GetProcAddress(module,MAKEINTRESOURCEA(1)),
            "ordinal and name lookup");
    ComPtr<IGameInput> input, second;
    require(SUCCEEDED(GameInputCreate(&input)) && input, "create manager");
    require(SUCCEEDED(GameInputCreate(&second)) && input.Get() == second.Get(), "singleton");
    second.Reset();
    const auto requested = GameInputKindKeyboard | GameInputKindMouse | GameInputKindGamepad;
    GameInputKind observed{}; GameInputCallbackToken combined{};
    require(SUCCEEDED(input->RegisterDeviceCallback(nullptr, requested, GameInputDeviceConnected,
        GameInputBlockingEnumeration, &observed, enumerate_kinds, &combined)) && observed == requested &&
        input->UnregisterCallback(combined, 1000000), "title registration returns all connected input kinds");
    DeviceEvent blocking{input.Get()};
    require(SUCCEEDED(input->RegisterDeviceCallback(nullptr, GameInputKindGamepad, GameInputDeviceConnected,
        GameInputBlockingEnumeration, &blocking, arrived, &blocking.token)) && blocking.token &&
        blocking.count == 1 && blocking.valid, "blocking gamepad enumeration before window");
    require(input->UnregisterCallback(blocking.token, 1000000) &&
            !input->UnregisterCallback(blocking.token, 0), "release callback token");
    DeviceEvent ignored{input.Get()}; GameInputCallbackToken invalid = 99;
    require(FAILED(input->RegisterDeviceCallback(nullptr, GameInputKindGamepad, GameInputDeviceConnected,
        GameInputBlockingEnumeration, &ignored, nullptr, &invalid)) && invalid == 0, "clear invalid registration");

    WNDCLASSW cls{}; cls.hInstance = GetModuleHandleW(nullptr); cls.lpfnWndProc = DefWindowProcW;
    cls.lpszClassName = L"TuxBox GameInput control";
    require(RegisterClassW(&cls) != 0, "register window");
    HWND window = CreateWindowW(cls.lpszClassName, cls.lpszClassName, WS_OVERLAPPEDWINDOW | WS_VISIBLE,
                               10, 10, 220, 180, nullptr, nullptr, cls.hInstance, nullptr);
    require(window != nullptr, "create window");
    SetForegroundWindow(window);
    wait_for([&] { return GetForegroundWindow() == window; }, "focus window");
    auto pad = read(input.Get(), GameInputKindGamepad);
    ComPtr<IGameInputDevice> pad_device;
    pad->GetDevice(&pad_device);
    DeviceEvent async{input.Get(), pad_device.Get()};
    require(SUCCEEDED(input->RegisterDeviceCallback(pad_device.Get(), GameInputKindGamepad,
        GameInputDeviceConnected, GameInputAsyncEnumeration, &async, arrived, &async.token)), "async registration");
    wait_for([&] { return InterlockedCompareExchange(&async.count, 0, 0) != 0; }, "async gamepad enumeration");
    require(input->UnregisterCallback(async.token, 1000000) && async.valid, "async callback identity");
    DeviceEvent quiet{input.Get()};
    require(SUCCEEDED(input->RegisterDeviceCallback(pad_device.Get(), GameInputKindGamepad,
        GameInputDeviceConnected, GameInputNoEnumeration, &quiet, arrived, &quiet.token)), "no enumeration");
    auto before = read(input.Get(), GameInputKindKeyboard);
    require(!has_r(before.Get()), "initial keyboard state");
    send(false,true);
    ComPtr<IGameInputReading> pressed;
    wait_for([&] { pressed = read(input.Get(), GameInputKindKeyboard); return has_r(pressed.Get()); }, "R press");
    GameInputGamepadState pad_state{};
    require(read(input.Get(), GameInputKindGamepad)->GetGamepadState(&pad_state) &&
            pad_state.leftTrigger == 1.0f, "R reaches gamepad trigger");
    send(false,false);
    wait_for([&] { return !has_r(read(input.Get(), GameInputKindKeyboard).Get()); }, "R release");
    require(!has_r(before.Get()) && has_r(pressed.Get()), "retained key snapshots");
    ComPtr<IGameInputDevice> keyboard;
    pressed->GetDevice(&keyboard);
    require(keyboard && keyboard->GetDeviceInfo()->supportedInput == GameInputKindKeyboard, "keyboard identity");
    ComPtr<IGameInputReading> next, prev;
    require(SUCCEEDED(input->GetNextReading(before.Get(), GameInputKindKeyboard, keyboard.Get(), &next)) &&
            next.Get() == pressed.Get() &&
            SUCCEEDED(input->GetPreviousReading(pressed.Get(), GameInputKindKeyboard, keyboard.Get(), &prev)) &&
            prev.Get() == before.Get(), "device-filtered reading history");
    require(next->GetSequenceNumber(GameInputKindKeyboard) > prev->GetSequenceNumber(GameInputKindKeyboard) &&
            next->GetTimestamp() >= prev->GetTimestamp(), "ordered snapshots");

    auto first = read(input.Get(), GameInputKindMouse);
    GameInputMouseState original{}, moved{};
    require(first->GetMouseState(&original) && !(original.buttons & GameInputMouseLeftButton), "initial mouse");
    POINT start{}; require(GetCursorPos(&start) && SetCursorPos(start.x+11,start.y+7), "move mouse");
    send(true,true);
    wait_for([&] {
        return read(input.Get(),GameInputKindMouse)->GetMouseState(&moved) && (moved.buttons & GameInputMouseLeftButton) &&
               moved.positionX-original.positionX == 11 && moved.positionY-original.positionY == 7;
    }, "mouse delta and click");
    require(read(input.Get(), GameInputKindGamepad)->GetGamepadState(&pad_state) &&
            (pad_state.buttons & GameInputGamepadA), "mouse confirms on gamepad");
    send(true,false);
    wait_for([&] { return read(input.Get(),GameInputKindMouse)->GetMouseState(&moved) &&
                          !(moved.buttons & GameInputMouseLeftButton); }, "mouse release");
    require(first->GetMouseState(&moved) && moved.positionX == original.positionX &&
            !(moved.buttons & GameInputMouseLeftButton), "retained mouse snapshot");
    require(input->UnregisterCallback(quiet.token, 1000000) && !quiet.count, "initial devices suppressed");
    require(read(input.Get(), GameInputKindGamepad)->GetGamepadState(&pad_state) &&
            pad_state.buttons == GameInputGamepadNone && pad_state.leftTrigger == 0.0f, "gamepad release");
    DestroyWindow(window);
}
}
int wmain(int argc, wchar_t** argv) {
    if (argc != 3 || std::wcscmp(argv[1], L"--private-desktop-report")) return 2;
    FILE* report = nullptr;
    if (_wfopen_s(&report,argv[2],L"wb")) return 2;
    try {
        run();
        std::fputs("gameinput PASS ordinal=1 singleton=1 keys=2 mouse=2 delta=11,7 history=2 retained=2 enumeration=3 desktop_pad=1\n",report);
        fclose(report); return 0;
    } catch (const std::exception& err) {
        std::fprintf(report,"gameinput failed: %s\n",err.what()); fclose(report); return 1;
    }
}
