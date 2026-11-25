// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Input;

/// <summary>
/// Keyboard and mouse from GetAsyncKeyState and GetCursorPos, which work on any thread without a window. They read
/// the whole desktop whether or not the game has focus; mouse motion stops at the screen edges; the wheel is not
/// available this way.
/// </summary>
internal sealed unsafe class DesktopInput : IDesktopSource
{
    private const int PointFields = 2;

    private readonly uint[] _scanCodes = new uint[InputLimits.VirtualKeyCount];
    private CursorPosition _last;

    public DesktopInput()
    {
        for (int key = 1; key < InputLimits.VirtualKeyCount; key++)
        {
            _scanCodes[key] = User32.MapVirtualKeyW((uint)key, User32.MapVirtualKeyToScanCode);
        }
    }

    [HotPath]
    public void ReadKeys(PressedKeys keys)
    {
        for (int key = 1; key < InputLimits.VirtualKeyCount; key++)
        {
            keys.Set(key, (User32.GetAsyncKeyState(key) & User32.KeyDownBit) != 0);
        }
    }

    [HotPath]
    public CursorPosition ReadCursor()
    {
        int* point = stackalloc int[PointFields];

        // GetCursorPos fails while another desktop (UAC, lock screen) is active: the cursor then stays where it was.
        if (User32.GetCursorPos(point) != 0)
        {
            _last = new CursorPosition(point[0], point[1]);
        }

        return _last;
    }

    [HotPath]
    public uint ScanCode(int virtualKey)
    {
        return _scanCodes[virtualKey];
    }
}
