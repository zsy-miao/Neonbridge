// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Input;

/// <summary>The set of virtual keys held at one poll; filled in place so polling allocates nothing.</summary>
internal sealed class PressedKeys
{
    private readonly bool[] _down = new bool[InputLimits.VirtualKeyCount];

    [HotPath]
    public bool IsDown(int? virtualKey)
    {
        if (virtualKey is null)
        {
            return false;
        }

        return _down[virtualKey.Value];
    }

    [HotPath]
    public void Set(int virtualKey, bool down)
    {
        _down[virtualKey] = down;
    }

    [HotPath]
    public void Clear()
    {
        Array.Clear(_down);
    }

    [HotPath]
    public void CopyFrom(PressedKeys other)
    {
        Array.Copy(other._down, _down, _down.Length);
    }
}
