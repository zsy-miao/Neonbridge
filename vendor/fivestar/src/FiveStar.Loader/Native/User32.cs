// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The user32 functions the GameInput and accessibility shims and the host call; none needs a window or a message queue.</summary>
internal static unsafe class User32
{
    /// <summary>The bit of a GetAsyncKeyState result that says the key is down.</summary>
    public const short KeyDownBit = unchecked((short)0x8000);

    /// <summary>MapVirtualKeyW MAPVK_VK_TO_VSC.</summary>
    public const uint MapVirtualKeyToScanCode = 0;

    private const string Library = "user32";

    [DllImport(Library)]
    public static extern short GetAsyncKeyState(int virtualKey);

    /// <summary>GetCursorPos: <paramref name="point"/> is a POINT (two int32).</summary>
    [DllImport(Library)]
    public static extern int GetCursorPos(int* point);

    [DllImport(Library)]
    public static extern uint MapVirtualKeyW(uint code, uint mapType);

    /// <summary>SetProcessDpiAwarenessContext: non-zero on success; fails once the awareness is set.</summary>
    [DllImport(Library, SetLastError = true)]
    public static extern int SetProcessDpiAwarenessContext(nint context);

    /// <summary>SystemParametersInfoW with a pointer parameter, such as SPI_GETHIGHCONTRAST and a HIGHCONTRASTW.</summary>
    [DllImport(Library)]
    public static extern int SystemParametersInfoW(uint action, uint parameter, void* value, uint flags);
}
