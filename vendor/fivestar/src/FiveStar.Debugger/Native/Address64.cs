// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>ADDRESS64 (dbghelp.h): 0x10 bytes.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct Address64
{
    /// <summary>ADDRESS_MODE AddrModeFlat.</summary>
    public const uint FlatMode = 3;

    public ulong Offset;
    public ushort Segment;
    public uint Mode;
}
