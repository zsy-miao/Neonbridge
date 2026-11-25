// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>XINPUT_VIBRATION (XInput.h): the low-frequency (left) and high-frequency (right) motor speeds.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XInputVibration
{
    public ushort LeftMotor;
    public ushort RightMotor;
}
