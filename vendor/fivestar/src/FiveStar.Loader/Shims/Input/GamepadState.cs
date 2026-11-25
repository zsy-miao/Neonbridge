// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Input;

/// <summary>GameInputGamepadState as the game reads it: button bits, triggers 0..1, sticks -1..1 with +Y up.</summary>
[StructLayout(LayoutKind.Explicit, Size = GameInputLayout.GamepadStateSize)]
internal struct GamepadState : IEquatable<GamepadState>
{
    [FieldOffset(GameInputLayout.GamepadButtonsOffset)]
    public uint Buttons;

    [FieldOffset(GameInputLayout.GamepadLeftTriggerOffset)]
    public float LeftTrigger;

    [FieldOffset(GameInputLayout.GamepadRightTriggerOffset)]
    public float RightTrigger;

    [FieldOffset(GameInputLayout.GamepadLeftXOffset)]
    public float LeftX;

    [FieldOffset(GameInputLayout.GamepadLeftYOffset)]
    public float LeftY;

    [FieldOffset(GameInputLayout.GamepadRightXOffset)]
    public float RightX;

    [FieldOffset(GameInputLayout.GamepadRightYOffset)]
    public float RightY;

    public readonly bool Equals(GamepadState other)
    {
        return Buttons == other.Buttons
            && LeftTrigger == other.LeftTrigger
            && RightTrigger == other.RightTrigger
            && LeftX == other.LeftX
            && LeftY == other.LeftY
            && RightX == other.RightX
            && RightY == other.RightY;
    }

    public override readonly bool Equals(object? obj)
    {
        return obj is GamepadState other && Equals(other);
    }

    public override readonly int GetHashCode()
    {
        return HashCode.Combine(Buttons, LeftTrigger, RightTrigger, LeftX, LeftY, RightX, RightY);
    }
}
