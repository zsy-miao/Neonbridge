// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Input;

namespace FiveStar.Loader.Tests.Support;

/// <summary>One gamepad slot holding <see cref="Gamepad"/>, the keys of <see cref="HeldKeys"/> held and a still cursor.</summary>
internal sealed class FakeInputSource : IGamepadSource, IDesktopSource
{
    public GamepadState Gamepad { get; set; }

    public IReadOnlyList<int> HeldKeys { get; set; } = [];

    public int SlotCount => 1;

    public bool TryRead(int slot, long now, out GamepadState state)
    {
        state = Gamepad;

        return true;
    }

    public void SetRumble(int slot, float lowFrequency, float highFrequency)
    {
    }

    public void ReadKeys(PressedKeys keys)
    {
        keys.Clear();

        foreach (int key in HeldKeys)
        {
            keys.Set(key, true);
        }
    }

    public CursorPosition ReadCursor()
    {
        return default;
    }

    public uint ScanCode(int virtualKey)
    {
        return 0;
    }
}
