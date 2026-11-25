// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The xinput1_4.dll functions the GameInput shim calls (XInput.h). Both return a Win32 error code.</summary>
internal static unsafe class XInput
{
    // XInput.h XINPUT_GAMEPAD button bits.
    public const ushort DPadUp = 0x0001;
    public const ushort DPadDown = 0x0002;
    public const ushort DPadLeft = 0x0004;
    public const ushort DPadRight = 0x0008;
    public const ushort Start = 0x0010;
    public const ushort Back = 0x0020;
    public const ushort LeftThumb = 0x0040;
    public const ushort RightThumb = 0x0080;
    public const ushort LeftShoulder = 0x0100;
    public const ushort RightShoulder = 0x0200;
    public const ushort A = 0x1000;
    public const ushort B = 0x2000;
    public const ushort X = 0x4000;
    public const ushort Y = 0x8000;

    private const string Library = "xinput1_4.dll";

    [DllImport(Library)]
    public static extern uint XInputGetState(uint userIndex, XInputState* state);

    [DllImport(Library)]
    public static extern uint XInputSetState(uint userIndex, XInputVibration* vibration);
}
