// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>CREATE_THREAD_DEBUG_INFO (minwinbase.h). The system owns the thread handle.</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct CreateThreadDebugInfo
{
    public nint Thread;
    public ulong ThreadLocalBase;
    public ulong StartAddress;
}
