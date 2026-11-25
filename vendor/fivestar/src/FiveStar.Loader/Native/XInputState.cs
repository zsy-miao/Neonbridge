// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>XINPUT_STATE with its XINPUT_GAMEPAD inline (XInput.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct XInputState
{
    public uint PacketNumber;
    public ushort Buttons;
    public byte LeftTrigger;
    public byte RightTrigger;
    public short ThumbLeftX;
    public short ThumbLeftY;
    public short ThumbRightX;
    public short ThumbRightY;
}
