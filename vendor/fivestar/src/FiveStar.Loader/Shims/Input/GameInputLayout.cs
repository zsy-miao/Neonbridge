// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// Every GameInput fact the shim depends on: module names, interface ids, vtable slots, structure offsets and flag
/// values. Taken from GameInput.h of the PC GDK 251005 (the v0 API: IGameInput 11BE2A7E-...). The console header and
/// later API versions may differ; this is the one file to change for them. The state structures
/// (<see cref="GamepadState"/>, <see cref="MouseState"/>, <see cref="KeyState"/>) take their offsets from here.
/// </summary>
internal static class GameInputLayout
{
    public const string Module = "gameinput.dll";

    /// <summary>API-set names the module may also be imported as (not confirmed), for symbols starting with "GameInput".</summary>
    public const string AliasPattern = @"^(api|ext)-ms-win-.*gameinput.*\.dll$";

    public const string AliasSymbolPrefix = "GameInput";

    public const string CreateFunction = "GameInputCreate";

    /// <summary>
    /// The console GameInput.dll export GameInputCreate is imported by ordinal 1. Read from the executable's only call:
    /// one zeroed IGameInput** argument, an HRESULT tested with <c>js</c>, then the result used as a COM object.
    /// </summary>
    public const string CreateOrdinal = "#1";

    // GameInputKind.
    public const uint KindUnknown = 0x0;
    public const uint KindControllerAxis = 0x2;
    public const uint KindControllerButton = 0x4;
    public const uint KindKeyboard = 0x10;
    public const uint KindMouse = 0x20;
    public const uint KindGamepad = 0x40000;

    // GameInputEnumerationKind.
    public const uint NoEnumeration = 0;

    // GameInputDeviceStatus.
    public const uint StatusNone = 0x0;
    public const uint StatusConnected = 0x1;
    public const uint StatusInputEnabled = 0x2;
    public const uint StatusOutputEnabled = 0x4;

    // GameInputDeviceFamily.
    public const int FamilyVirtual = -1;
    public const int FamilyXboxOne = 1;
    public const int FamilyHid = 3;

    // GameInputRumbleMotors.
    public const uint RumbleLowFrequency = 0x1;
    public const uint RumbleHighFrequency = 0x2;

    // GameInputMouseButtons.
    public const uint MouseLeft = 0x1;
    public const uint MouseRight = 0x2;
    public const uint MouseMiddle = 0x4;
    public const uint MouseButton4 = 0x8;
    public const uint MouseButton5 = 0x10;

    // GameInputGamepadButtons.
    public const uint GamepadMenu = 0x1;
    public const uint GamepadView = 0x2;
    public const uint GamepadA = 0x4;
    public const uint GamepadB = 0x8;
    public const uint GamepadX = 0x10;
    public const uint GamepadY = 0x20;
    public const uint GamepadDPadUp = 0x40;
    public const uint GamepadDPadDown = 0x80;
    public const uint GamepadDPadLeft = 0x100;
    public const uint GamepadDPadRight = 0x200;
    public const uint GamepadLeftShoulder = 0x400;
    public const uint GamepadRightShoulder = 0x800;
    public const uint GamepadLeftThumbstick = 0x1000;
    public const uint GamepadRightThumbstick = 0x2000;

    // GameInputKeyboardKind and GameInputLabel.
    public const int KeyboardUnknown = -1;
    public const int LabelUnknown = -1;

    // GameInputBatteryStatus.
    public const int BatteryUnknown = -1;

    // GAMEINPUT_E_* HRESULTs.
    public const int DeviceNotFound = unchecked((int)0x838A0002);
    public const int ReadingNotFound = unchecked((int)0x838A0003);

    // IGameInput vtable.
    public const int InputGetCurrentTimestamp = 3;
    public const int InputGetCurrentReading = 4;
    public const int InputGetNextReading = 5;
    public const int InputGetPreviousReading = 6;
    public const int InputRegisterDeviceCallback = 9;
    public const int InputStopCallback = 12;
    public const int InputUnregisterCallback = 13;
    public const int InputFindDeviceFromId = 16;

    // IGameInputReading vtable.
    public const int ReadingGetInputKind = 3;
    public const int ReadingGetSequenceNumber = 4;
    public const int ReadingGetTimestamp = 5;
    public const int ReadingGetDevice = 6;
    public const int ReadingGetRawReport = 7;
    public const int ReadingGetKeyCount = 14;
    public const int ReadingGetKeyState = 15;
    public const int ReadingGetMouseState = 16;
    public const int ReadingGetGamepadState = 22;

    // IGameInputDevice vtable.
    public const int DeviceGetDeviceInfo = 3;
    public const int DeviceGetDeviceStatus = 4;
    public const int DeviceGetBatteryState = 5;
    public const int DeviceSetRumbleState = 10;

    // IUnknown vtable.
    public const int QueryInterfaceSlot = 0;
    public const int AddRefSlot = 1;
    public const int ReleaseSlot = 2;

    // GameInputGamepadState: buttons, then six floats.
    public const int GamepadButtonsOffset = 0;
    public const int GamepadLeftTriggerOffset = 4;
    public const int GamepadRightTriggerOffset = 8;
    public const int GamepadLeftXOffset = 12;
    public const int GamepadLeftYOffset = 16;
    public const int GamepadRightXOffset = 20;
    public const int GamepadRightYOffset = 24;
    public const int GamepadStateSize = 28;

    // GameInputMouseState: buttons, then four int64 at 8-byte alignment.
    public const int MouseButtonsOffset = 0;
    public const int MousePositionXOffset = 8;
    public const int MousePositionYOffset = 16;
    public const int MouseWheelXOffset = 24;
    public const int MouseWheelYOffset = 32;
    public const int MouseStateSize = 40;

    // GameInputKeyState: scanCode, codePoint, virtualKey, isDeadKey.
    public const int KeyScanCodeOffset = 0;
    public const int KeyCodePointOffset = 4;
    public const int KeyVirtualKeyOffset = 8;
    public const int KeyDeadKeyOffset = 9;
    public const int KeyStateSize = 12;

    // GameInputRumbleParams: four floats.
    public const int RumbleLowFrequencyOffset = 0;
    public const int RumbleHighFrequencyOffset = 4;

    // GameInputBatteryState: four floats and the status.
    public const int BatteryStatusOffset = 16;
    public const int BatteryStateSize = 20;

    // GameInputString: sizeInBytes, codePointCount, data.
    public const int StringSizeOffset = 0;
    public const int StringCodePointsOffset = 4;
    public const int StringDataOffset = 8;
    public const int StringSize = 16;

    // GameInputDeviceInfo.
    public const int InfoSizeOffset = 0;
    public const int InfoVendorIdOffset = 4;
    public const int InfoProductIdOffset = 6;
    public const int InfoUsagePageOffset = 12;
    public const int InfoUsageIdOffset = 14;
    public const int InfoDeviceIdOffset = 32;
    public const int InfoDeviceRootIdOffset = 64;
    public const int DeviceIdSize = 32;
    public const int InfoFamilyOffset = 96;
    public const int InfoSupportedInputOffset = 104;
    public const int InfoRumbleMotorsOffset = 108;
    public const int InfoKeyboardInfoOffset = 208;
    public const int InfoMouseInfoOffset = 216;
    public const int InfoGamepadInfoOffset = 256;
    public const int InfoDisplayNameOffset = 296;
    public const int DeviceInfoSize = 320;

    // GameInputKeyboardInfo: kind, layout, keyCount, functionKeyCount, maxSimultaneousKeys, two platform ids, nativeLanguage.
    public const int KeyboardKindOffset = 0;
    public const int KeyboardKeyCountOffset = 8;
    public const int KeyboardMaxKeysOffset = 16;
    public const int KeyboardInfoSize = 40;

    // GameInputMouseInfo: supportedButtons, sampleRate, sensorDpi, hasWheelX, hasWheelY.
    public const int MouseSupportedButtonsOffset = 0;
    public const int MouseInfoSize = 16;

    /// <summary>GameInputGamepadInfo: 14 GameInputLabel values (int32).</summary>
    public const int GamepadInfoLabels = 14;

    public const int GamepadInfoSize = GamepadInfoLabels * sizeof(int);

    /// <summary>
    /// The GameInputGamepadInfo labels in field order (menu, view, A, B, X, Y, d-pad up/down/left/right, left
    /// shoulder, right shoulder, left stick, right stick) as GameInputLabel Xbox values.
    /// </summary>
    public static readonly IReadOnlyList<int> XboxGamepadLabels = [4, 5, 7, 8, 9, 10, 11, 12, 13, 14, 15, 18, 17, 20];

    public static readonly Guid UnknownIid = new("00000000-0000-0000-c000-000000000046");
    public static readonly Guid InputIid = new("11be2a7e-4254-445a-9c09-ffc40f006918");
    public static readonly Guid ReadingIid = new("2156947a-e1fa-4de0-a30b-d812931dbd8d");
    public static readonly Guid DeviceIid = new("31dd86fb-4c1b-408a-868f-439b3cd47125");

    /// <summary>The methods of IGameInput, IGameInputReading and IGameInputDevice by slot, for the not-implemented log.</summary>
    public static readonly IReadOnlyList<string> InputMethods =
    [
        "QueryInterface", "AddRef", "Release", "GetCurrentTimestamp", "GetCurrentReading", "GetNextReading",
        "GetPreviousReading", "GetTemporalReading", "RegisterReadingCallback", "RegisterDeviceCallback",
        "RegisterGuideButtonCallback", "RegisterKeyboardLayoutCallback", "StopCallback", "UnregisterCallback",
        "CreateDispatcher", "CreateAggregateDevice", "FindDeviceFromId", "FindDeviceFromObject",
        "FindDeviceFromPlatformHandle", "FindDeviceFromPlatformString", "EnableOemDeviceSupport", "SetFocusPolicy",
    ];

    public static readonly IReadOnlyList<string> ReadingMethods =
    [
        "QueryInterface", "AddRef", "Release", "GetInputKind", "GetSequenceNumber", "GetTimestamp", "GetDevice",
        "GetRawReport", "GetControllerAxisCount", "GetControllerAxisState", "GetControllerButtonCount",
        "GetControllerButtonState", "GetControllerSwitchCount", "GetControllerSwitchState", "GetKeyCount",
        "GetKeyState", "GetMouseState", "GetTouchCount", "GetTouchState", "GetMotionState", "GetArcadeStickState",
        "GetFlightStickState", "GetGamepadState", "GetRacingWheelState", "GetUiNavigationState",
    ];

    public static readonly IReadOnlyList<string> DeviceMethods =
    [
        "QueryInterface", "AddRef", "Release", "GetDeviceInfo", "GetDeviceStatus", "GetBatteryState",
        "CreateForceFeedbackEffect", "IsForceFeedbackMotorPoweredOn", "SetForceFeedbackMotorGain",
        "SetHapticMotorState", "SetRumbleState", "SetInputSynchronizationState", "SendInputSynchronizationHint",
        "PowerOff", "CreateRawDeviceReport", "GetRawDeviceFeature", "SetRawDeviceFeature", "SendRawDeviceOutput",
        "SendRawDeviceOutputWithResponse", "ExecuteRawDeviceIoControl", "AcquireExclusiveRawDeviceAccess",
        "ReleaseExclusiveRawDeviceAccess",
    ];

    /// <summary>Methods that return an HRESULT; a not-implemented stub returns E_NOTIMPL for these and 0 for the rest.</summary>
    public static readonly FrozenSet<string> HResultMethods = new[]
    {
        "QueryInterface", "GetCurrentReading", "GetNextReading", "GetPreviousReading", "GetTemporalReading",
        "RegisterReadingCallback", "RegisterDeviceCallback", "RegisterGuideButtonCallback",
        "RegisterKeyboardLayoutCallback", "CreateDispatcher", "CreateAggregateDevice", "FindDeviceFromId",
        "FindDeviceFromObject", "FindDeviceFromPlatformHandle", "FindDeviceFromPlatformString",
        "EnableOemDeviceSupport", "CreateForceFeedbackEffect", "SetHapticMotorState", "CreateRawDeviceReport",
        "GetRawDeviceFeature", "SetRawDeviceFeature", "SendRawDeviceOutput", "SendRawDeviceOutputWithResponse",
        "ExecuteRawDeviceIoControl",
    }.ToFrozenSet(StringComparer.Ordinal);
}
