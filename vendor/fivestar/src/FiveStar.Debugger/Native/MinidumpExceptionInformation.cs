// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// MINIDUMP_EXCEPTION_INFORMATION (minidumpapiset.h), packed to 4 like every minidump structure. With ClientPointers 0
/// the exception pointers point into the debugger's own memory.
/// </summary>
[StructLayout(LayoutKind.Sequential, Pack = 4)]
internal struct MinidumpExceptionInformation
{
    public uint ThreadId;
    public nint ExceptionPointers;
    public int ClientPointers;
}
