// Use the SDK's complete vtables to retain the exact v0 signature of every slot.
#define CINTERFACE
#define NOMINMAX
#include <windows.h>
#include <GameInput.h>
#include <array>
#include <type_traits>

extern "C" {
HRESULT gi_query(const void*, REFIID, void**);
ULONG gi_add(const void*);
ULONG gi_release(const void*);
uint64_t gi_time();
HRESULT gi_read(IGameInput*, GameInputKind, IGameInputDevice*, IGameInputReading*,
                int direction, IGameInputReading**);
HRESULT gi_register_device(IGameInput*, IGameInputDevice*, GameInputKind, GameInputDeviceStatus,
                           GameInputEnumerationKind, void*, GameInputDeviceCallback, GameInputCallbackToken*);
void gi_stop(IGameInput*, GameInputCallbackToken);
bool gi_unregister(IGameInput*, GameInputCallbackToken, uint64_t);
GameInputKind gi_kind(IGameInputReading*);
uint64_t gi_sequence(IGameInputReading*, GameInputKind);
uint64_t gi_timestamp(IGameInputReading*);
void gi_device(IGameInputReading*, IGameInputDevice**);
UINT32 gi_slot(IGameInputDevice*);
GameInputDeviceStatus gi_status(IGameInputDevice*);
UINT32 gi_key_count(IGameInputReading*);
UINT32 gi_keys(IGameInputReading*, UINT32, GameInputKeyState*);
bool gi_mouse(IGameInputReading*, GameInputMouseState*);
bool gi_pad(IGameInputReading*, GameInputGamepadState*);
}

static_assert(sizeof(GameInputMouseState) == 40 && sizeof(GameInputKeyState) == 12 &&
              sizeof(GameInputGamepadState) == 28);

namespace {
template<class T> HRESULT query(T* self, REFIID iid, void** out) { return gi_query(self, iid, out); }
template<class T> ULONG add(T* self) { return gi_add(self); }
template<class T> ULONG release(T* self) { return gi_release(self); }

// An unsupported SDK operation returns the correct type and clears interface outputs.
template<class T> void clear_interface(T) {}
template<class T> void clear_interface(T** out) { if (out) *out = nullptr; }
template<class Signature> struct Unavailable;
template<class R, class... Args>
struct Unavailable<R (STDMETHODCALLTYPE *)(Args...)> {
    static R STDMETHODCALLTYPE call(Args... args) {
        (clear_interface(args), ...);
        if constexpr (std::is_same_v<R, HRESULT>) return E_NOTIMPL;
        else if constexpr (!std::is_void_v<R>) return R{};
    }
};
#define UNAVAILABLE(table, method) table.method = Unavailable<decltype(table.method)>::call

GameInputDeviceInfo const* info(IGameInputDevice* device) {
    static const GameInputKeyboardInfo keyboard = [] {
        GameInputKeyboardInfo k{}; k.kind = GameInputAnsiKeyboard; k.maxSimultaneousKeys = 256;
        return k;
    }();
    static const GameInputMouseInfo mouse = [] {
        GameInputMouseInfo m{}; m.supportedButtons = GameInputMouseLeftButton | GameInputMouseRightButton |
            GameInputMouseMiddleButton | GameInputMouseButton4 | GameInputMouseButton5;
        return m;
    }();
    static const GameInputGamepadInfo pad{};
    static const auto devices = [] {
        std::array<GameInputDeviceInfo, 7> result{};
        for (unsigned i = 0; i < result.size(); ++i) {
            auto& d = result[i]; d.infoSize = sizeof(d);
            d.deviceId.value[0] = static_cast<uint8_t>(i + 1);
            d.deviceRootId = d.deviceId;
            d.deviceFamily = i < 2 ? GameInputFamilyHid : GameInputFamilyXbox360;
            d.supportedInput = i == 0 ? GameInputKindKeyboard : i == 1 ? GameInputKindMouse : GameInputKindGamepad;
            if (i == 0) d.keyboardInfo = &keyboard;
            if (i == 1) d.mouseInfo = &mouse;
            if (i >= 2) d.gamepadInfo = &pad;
        }
        return result;
    }();
    return &devices[gi_slot(device)];
}

constexpr IGameInputVtbl input_table() {
    IGameInputVtbl t{};
    t.QueryInterface = query; t.AddRef = add; t.Release = release;
    t.GetCurrentTimestamp = [](IGameInput*) { return gi_time(); };
    t.GetCurrentReading = [](IGameInput* s, GameInputKind k, IGameInputDevice* d, IGameInputReading** r) {
        return gi_read(s, k, d, nullptr, 0, r);
    };
    t.GetNextReading = [](IGameInput* s, IGameInputReading* ref, GameInputKind k, IGameInputDevice* d, IGameInputReading** r) {
        return gi_read(s, k, d, ref, 1, r);
    };
    t.GetPreviousReading = [](IGameInput* s, IGameInputReading* ref, GameInputKind k, IGameInputDevice* d, IGameInputReading** r) {
        return gi_read(s, k, d, ref, -1, r);
    };
    UNAVAILABLE(t, GetTemporalReading);
    UNAVAILABLE(t, RegisterReadingCallback);
    t.RegisterDeviceCallback = gi_register_device;
    UNAVAILABLE(t, RegisterSystemButtonCallback);
    UNAVAILABLE(t, RegisterKeyboardLayoutCallback);
    t.StopCallback = gi_stop;
    t.UnregisterCallback = gi_unregister;
    UNAVAILABLE(t, CreateDispatcher);
    UNAVAILABLE(t, CreateAggregateDevice);
    UNAVAILABLE(t, FindDeviceFromId);
    UNAVAILABLE(t, FindDeviceFromObject);
    UNAVAILABLE(t, FindDeviceFromPlatformHandle);
    UNAVAILABLE(t, FindDeviceFromPlatformString);
    UNAVAILABLE(t, EnableOemDeviceSupport);
    UNAVAILABLE(t, SetFocusPolicy);
    return t;
}
constexpr IGameInputReadingVtbl reading_table() {
    IGameInputReadingVtbl t{};
    t.QueryInterface = query; t.AddRef = add; t.Release = release;
    t.GetInputKind = gi_kind; t.GetSequenceNumber = gi_sequence; t.GetTimestamp = gi_timestamp;
    t.GetDevice = gi_device; t.GetKeyCount = gi_key_count; t.GetKeyState = gi_keys;
    t.GetMouseState = gi_mouse; t.GetGamepadState = gi_pad;
    UNAVAILABLE(t, GetRawReport);
    UNAVAILABLE(t, GetControllerAxisCount);
    UNAVAILABLE(t, GetControllerAxisState);
    UNAVAILABLE(t, GetControllerButtonCount);
    UNAVAILABLE(t, GetControllerButtonState);
    UNAVAILABLE(t, GetControllerSwitchCount);
    UNAVAILABLE(t, GetControllerSwitchState);
    UNAVAILABLE(t, GetTouchCount);
    UNAVAILABLE(t, GetTouchState);
    UNAVAILABLE(t, GetMotionState);
    UNAVAILABLE(t, GetArcadeStickState);
    UNAVAILABLE(t, GetFlightStickState);
    UNAVAILABLE(t, GetRacingWheelState);
    UNAVAILABLE(t, GetUiNavigationState);
    return t;
}
constexpr IGameInputDeviceVtbl device_table() {
    IGameInputDeviceVtbl t{};
    t.QueryInterface = query; t.AddRef = add; t.Release = release;
    t.GetDeviceInfo = info; t.GetDeviceStatus = gi_status;
    t.GetBatteryState = [](IGameInputDevice*, GameInputBatteryState* state) { if (state) *state = {}; };
    UNAVAILABLE(t, CreateForceFeedbackEffect);
    UNAVAILABLE(t, IsForceFeedbackMotorPoweredOn);
    UNAVAILABLE(t, SetForceFeedbackMotorGain);
    UNAVAILABLE(t, SetHapticMotorState);
    UNAVAILABLE(t, SetRumbleState);
    UNAVAILABLE(t, SetInputSynchronizationState);
    UNAVAILABLE(t, SendInputSynchronizationHint);
    UNAVAILABLE(t, PowerOff);
    UNAVAILABLE(t, CreateRawDeviceReport);
    UNAVAILABLE(t, GetRawDeviceFeature);
    UNAVAILABLE(t, SetRawDeviceFeature);
    UNAVAILABLE(t, SendRawDeviceOutput);
    UNAVAILABLE(t, SendRawDeviceOutputWithResponse);
    UNAVAILABLE(t, ExecuteRawDeviceIoControl);
    UNAVAILABLE(t, AcquireExclusiveRawDeviceAccess);
    UNAVAILABLE(t, ReleaseExclusiveRawDeviceAccess);
    return t;
}
}
extern "C" {
extern const IGameInputVtbl gi_input_table = input_table();
extern const IGameInputDeviceVtbl gi_device_table = device_table();
extern const IGameInputReadingVtbl gi_reading_table = reading_table();
}
