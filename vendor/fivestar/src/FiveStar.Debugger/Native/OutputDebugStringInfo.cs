// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>OUTPUT_DEBUG_STRING_INFO (minwinbase.h): the string lives in the debuggee; the length counts its terminator.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct OutputDebugStringInfo
{
    public ulong Data;
    public ushort Unicode;
    public ushort Length;
}
