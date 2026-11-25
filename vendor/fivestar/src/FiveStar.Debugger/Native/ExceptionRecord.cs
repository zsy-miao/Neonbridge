// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>EXCEPTION_RECORD (winnt.h), x64 layout: 0x98 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ExceptionRecord
{
    /// <summary>EXCEPTION_MAXIMUM_PARAMETERS (winnt.h).</summary>
    public const int MaximumParameters = 15;

    public uint Code;
    public uint Flags;
    public ulong InnerRecord;
    public ulong Address;
    public uint NumberParameters;
    public fixed ulong Information[MaximumParameters];
}
