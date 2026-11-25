// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>RIP_INFO (minwinbase.h): the system error that ended debugging of the process.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct RipInfo
{
    public uint Error;
    public uint Type;
}
