// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>GameInputMouseState: button bits and the accumulated relative position and wheel.</summary>
[StructLayout(LayoutKind.Explicit, Size = GameInputLayout.MouseStateSize)]
internal struct MouseState
{
    [FieldOffset(GameInputLayout.MouseButtonsOffset)]
    public uint Buttons;

    [FieldOffset(GameInputLayout.MousePositionXOffset)]
    public long PositionX;

    [FieldOffset(GameInputLayout.MousePositionYOffset)]
    public long PositionY;

    [FieldOffset(GameInputLayout.MouseWheelXOffset)]
    public long WheelX;

    [FieldOffset(GameInputLayout.MouseWheelYOffset)]
    public long WheelY;
}
