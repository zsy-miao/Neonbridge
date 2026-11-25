// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>Where physical gamepads come from (XInput on the desktop).</summary>
internal interface IGamepadSource
{
    /// <summary>The number of gamepad slots; each becomes one IGameInputDevice.</summary>
    int SlotCount { get; }

    /// <summary>The state of one slot at <paramref name="now"/> (microseconds); false when nothing is connected there.</summary>
    bool TryRead(int slot, long now, out GamepadState state);

    /// <summary>Sets the two rumble motors of one slot, 0..1 each.</summary>
    void SetRumble(int slot, float lowFrequency, float highFrequency);
}
