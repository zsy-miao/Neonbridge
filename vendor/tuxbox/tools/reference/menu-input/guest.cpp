#include <GameInput.h>
#include <cstdio>
#include <stdexcept>
#include <windows.h>
#include <Xinput.h>
#include <wrl/client.h>

using Microsoft::WRL::ComPtr;

static void require(bool passed, const char *message) {
  if (!passed)
    throw std::runtime_error(message);
}

static void pump() {
  MSG message{};
  while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
    TranslateMessage(&message);
    DispatchMessageW(&message);
  }
}

static ComPtr<IGameInputReading> expect(IGameInput *input,
                                        const GameInputGamepadState &expected) {
  auto deadline = GetTickCount64() + 2000;
  HRESULT hr = S_OK;
  GameInputGamepadState state{};
  do {
    pump();
    ComPtr<IGameInputReading> reading;
    hr = input->GetCurrentReading(GameInputKindGamepad, nullptr, &reading);
    if (SUCCEEDED(hr) &&
        reading->GetGamepadState(&state) && state.buttons == expected.buttons &&
        state.leftTrigger == expected.leftTrigger &&
        state.rightTrigger == expected.rightTrigger &&
        state.leftThumbstickX == expected.leftThumbstickX &&
        state.leftThumbstickY == expected.leftThumbstickY &&
        state.rightThumbstickX == expected.rightThumbstickX &&
        state.rightThumbstickY == expected.rightThumbstickY)
      return reading;
    Sleep(8);
  } while (GetTickCount64() < deadline);
  char details[300];
  std::snprintf(details, sizeof(details),
                "GetCurrentReading hr=%08lx buttons=%x/%x triggers=%.2f,%.2f/%.2f,%.2f sticks=%.2f,%.2f,%.2f,%.2f/%.2f,%.2f,%.2f,%.2f (observed/expected)",
                static_cast<unsigned long>(hr), state.buttons, expected.buttons,
                state.leftTrigger, state.rightTrigger, expected.leftTrigger, expected.rightTrigger,
                state.leftThumbstickX, state.leftThumbstickY, state.rightThumbstickX, state.rightThumbstickY,
                expected.leftThumbstickX, expected.leftThumbstickY, expected.rightThumbstickX, expected.rightThumbstickY);
  throw std::runtime_error(details);
}

// The Linux harness delivers these requests through XTest on the private display.
static char last_input[80] = "initial";

static void desktop_input(const char *action, const char *argument) {
  std::snprintf(last_input, sizeof(last_input), "%s %s", action, argument);
  FILE *file{};
  require(!fopen_s(&file, "input.next", "wb"), "open host request");
  std::fprintf(file, "%s %s\n", action, argument);
  require(!fclose(file) && MoveFileW(L"input.next", L"input.cmd"),
          "publish host request");
  const auto deadline = GetTickCount64() + 2000;
  while (GetFileAttributesW(L"input.done") == INVALID_FILE_ATTRIBUTES) {
    require(GetTickCount64() < deadline, "host input timed out");
    pump();
    Sleep(2);
  }
  require(DeleteFileW(L"input.done"), "clear host acknowledgment");
}

static void key(WORD vk, bool pressed) {
  char letter[2] = {static_cast<char>(vk - 'A' + 'a'), 0};
  const char *name = letter;
  switch (vk) {
  case VK_UP: name = "Up"; break;
  case VK_DOWN: name = "Down"; break;
  case VK_LEFT: name = "Left"; break;
  case VK_RIGHT: name = "Right"; break;
  case VK_RETURN: name = "Return"; break;
  case VK_ESCAPE: name = "Escape"; break;
  }
  desktop_input(pressed ? "keydown" : "keyup", name);
}

static void expect_mouse_direction(IGameInput *input, int direction) {
  const auto deadline = GetTickCount64() + 2000;
  do {
    desktop_input("mousemove_relative --", direction > 0 ? "35 -35" : "-35 35");
    pump();
    ComPtr<IGameInputReading> reading;
    GameInputGamepadState state{};
    if (SUCCEEDED(input->GetCurrentReading(GameInputKindGamepad, nullptr, &reading)) &&
        reading->GetGamepadState(&state) && state.leftTrigger == 1 &&
        direction * state.rightThumbstickX > 0 && direction * state.rightThumbstickY > 0)
      return;
  } while (GetTickCount64() < deadline);
  throw std::runtime_error("Missing signed mouse-look axes");
}

int wmain(int argc, wchar_t **argv) {
  FILE *receipt{};
  if (argc != 3 || _wfopen_s(&receipt, argv[2], L"w"))
    return 2;
  try {
    WNDCLASSW cls{};
    cls.lpfnWndProc = DefWindowProcW;
    cls.hInstance = GetModuleHandleW(nullptr);
    cls.lpszClassName = L"TuxBoxGamepadTest";
    require(RegisterClassW(&cls) != 0, "RegisterClass");
    HWND window = CreateWindowW(
        cls.lpszClassName, L"TuxBox keyboard controller test",
        WS_OVERLAPPEDWINDOW | WS_VISIBLE, CW_USEDEFAULT, CW_USEDEFAULT, 320,
        180, nullptr, nullptr, cls.hInstance, nullptr);
    require(window != nullptr, "CreateWindow");
    ShowWindow(window, SW_SHOW);
    const auto deadline = GetTickCount64() + 5000;
    do {
      pump();
      SetForegroundWindow(window);
      Sleep(16);
    } while (GetForegroundWindow() != window && GetTickCount64() < deadline);
    if (GetForegroundWindow() != window) {
      HWND foreground = GetForegroundWindow();
      DWORD owner = 0;
      GetWindowThreadProcessId(foreground, &owner);
      std::fprintf(
          receipt,
          "foreground=%p owner=%lu window=%p pid=%lu active=%p focus=%p\n",
          foreground, owner, window, GetCurrentProcessId(), GetActiveWindow(),
          GetFocus());
      char cls[128]{}, text[256]{};
      GetClassNameA(foreground, cls, sizeof(cls));
      GetWindowTextA(foreground, text, sizeof(text));
      std::fprintf(receipt, "class=%s text=%s\n", cls, text);
      throw std::runtime_error("focus");
    }
    ComPtr<IGameInput> input;
    require(SUCCEEDED(GameInputCreate(&input)), "GameInputCreate");
    XINPUT_STATE hardware{};
    for (DWORD slot = 0; slot < XUSER_MAX_COUNT; ++slot)
      require(XInputGetState(slot, &hardware) != ERROR_SUCCESS, "disconnect physical controllers for the desktop-only test");
    if (wcscmp(argv[1], L"off") == 0) {
      ComPtr<IGameInputReading> reading;
      require(FAILED(input->GetCurrentReading(GameInputKindGamepad, nullptr, &reading)) && !reading,
              "unexpected keyboard gamepad");
      std::fprintf(receipt, "keyboard_pad_absent=1\n");
    } else {
      GameInputGamepadState expected{};
      auto previous = expect(input.Get(), expected);
      ComPtr<IGameInputDevice> device;
      previous->GetDevice(&device);
      require(device && device->GetDeviceInfo()->supportedInput & GameInputKindGamepad, "keyboard gamepad identity");
      require(!device->GetDeviceInfo()->supportedRumbleMotors,
              "synthetic rumble");
      const struct {
        WORD key;
        GameInputGamepadButtons button;
      } cases[] = {{'Z', GameInputGamepadA},
                   {VK_RETURN, GameInputGamepadA},
                   {VK_ESCAPE, GameInputGamepadB},
                   {'X', GameInputGamepadB},
                   {'M', GameInputGamepadMenu},
                   {'I', GameInputGamepadDPadUp},
                   {'J', GameInputGamepadDPadLeft},
                   {'K', GameInputGamepadDPadDown},
                   {'L', GameInputGamepadDPadRight},
                   {'C', GameInputGamepadX},
                   {'V', GameInputGamepadY},
                   {'N', GameInputGamepadView},
                   {'Q', GameInputGamepadLeftShoulder},
                   {'E', GameInputGamepadRightShoulder}};
      unsigned transitions = 0;
      auto edge = [&](WORD vk, bool down) {
        key(vk, down);
        auto now = expect(input.Get(), expected);
        Sleep(40);
        require(expect(input.Get(), expected).Get() == now.Get(),
                "duplicate held reading");
        ComPtr<IGameInputReading> next;
        require(SUCCEEDED(input->GetNextReading(previous.Get(),
                                                GameInputKindGamepad,
                                                device.Get(), &next)) &&
                    next.Get() == now.Get(),
                "missing history edge");
        previous = now;
        ++transitions;
      };
      for (const auto &item : cases) {
        expected.buttons = item.button;
        edge(item.key, true);
        expected.buttons = GameInputGamepadNone;
        edge(item.key, false);
      }
      // Each pair is pressed together, then released separately. The two
      // opposite keys produce neutral, releasing just one restores the other.
      const struct {
        WORD negative, positive;
        float GameInputGamepadState::*axis;
      } axes[] = {{'A', 'D', &GameInputGamepadState::leftThumbstickX},
                  {'S', 'W', &GameInputGamepadState::leftThumbstickY},
                  {VK_LEFT, VK_RIGHT, &GameInputGamepadState::rightThumbstickX},
                  {VK_DOWN, VK_UP, &GameInputGamepadState::rightThumbstickY}};
      for (const auto &pair : axes) {
        expected.*pair.axis = -1.0f;
        edge(pair.negative, true);
        expected.*pair.axis = 0.0f;
        edge(pair.positive, true);
        expected.*pair.axis = 1.0f;
        edge(pair.negative, false);
        expected.*pair.axis = 0.0f;
        edge(pair.positive, false);
      }
      expected.leftTrigger = 1.0f;
      edge('R', true);
      expected.rightTrigger = 1.0f;
      edge('F', true);
      expected.leftTrigger = 0.0f;
      edge('R', false);
      expected.rightTrigger = 0.0f;
      edge('F', false);
      desktop_input("mousedown", "1");
      expected.buttons = GameInputGamepadA;
      expect(input.Get(), expected);
      desktop_input("mouseup", "1");
      expected.buttons = GameInputGamepadNone;
      expect(input.Get(), expected);

      POINT home{45, 55}, actual{};
      require(ClientToScreen(window, &home) && SetCursorPos(home.x, home.y), "cursor home");
      desktop_input("mousedown", "3");
      expected.leftTrigger = 1;
      expect(input.Get(), expected);
      desktop_input("mousedown", "1");
      expected.rightTrigger = 1;
      expect(input.Get(), expected);
      desktop_input("mouseup", "1");
      expected.rightTrigger = 0;
      expect(input.Get(), expected);
      expect_mouse_direction(input.Get(), 1);
      expect(input.Get(), expected);
      expect_mouse_direction(input.Get(), -1);
      expect(input.Get(), expected);
      desktop_input("mouseup", "3");
      expected.leftTrigger = 0;
      expect(input.Get(), expected);
      require(GetCursorPos(&actual) && actual.x == home.x && actual.y == home.y,
              "restore cursor after mouse look");
      std::fprintf(receipt,
                   "keyboard_device_id=%u\nbuttons=12\nkeyboard_aliases=2\nbipolar_axes=4\n"
                   "triggers=2\nordered_edges=%u\nmouse_confirm=1\nmouse_aim_fire=1\n"
                   "mouse_look_signed_axes=2\nmouse_idle_neutral=1\ncursor_restored=1\n",
                   device->GetDeviceInfo()->deviceId.value[0], transitions);
    }
    DestroyWindow(window);
    fclose(receipt);
    return 0;
  } catch (const std::exception &error) {
    wchar_t title[160]{};
    GetWindowTextW(GetForegroundWindow(), title, 160);
    std::fprintf(receipt, "error=%s after %s foreground=%ls pressed=%x\n", error.what(), last_input, title,
                 static_cast<unsigned>(GetAsyncKeyState('Z')));
    fclose(receipt);
    return 1;
  }
}
