// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// CREATE_PROCESS_DEBUG_INFO (minwinbase.h). The debugger closes the file handle; the system owns the process and
/// thread handles.
/// </summary>
[StructLayout(LayoutKind.Sequential)]
internal struct CreateProcessDebugInfo
{
    public nint File;
    public nint Process;
    public nint Thread;
    public ulong BaseOfImage;
    public uint DebugInfoFileOffset;
    public uint DebugInfoSize;
    public ulong ThreadLocalBase;
    public ulong StartAddress;
    public ulong ImageName;
    public ushort Unicode;
}
