// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>LOAD_DLL_DEBUG_INFO (minwinbase.h). The debugger closes the file handle.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct LoadDllDebugInfo
{
    public nint File;
    public ulong BaseOfDll;
    public uint DebugInfoFileOffset;
    public uint DebugInfoSize;
    public ulong ImageName;
    public ushort Unicode;
}
