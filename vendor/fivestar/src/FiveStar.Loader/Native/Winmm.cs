// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Native;

/// <summary>The multimedia timer calls the loader makes.</summary>
internal static class Winmm
{
    private const string Library = "winmm";

    /// <summary>MMRESULT timeBeginPeriod(UINT ms): 0 on success.</summary>
    [DllImport(Library)]
    public static extern uint timeBeginPeriod(uint milliseconds);

    /// <summary>MMRESULT timeEndPeriod(UINT ms): ends a matching timeBeginPeriod.</summary>
    [DllImport(Library)]
    public static extern uint timeEndPeriod(uint milliseconds);
}
