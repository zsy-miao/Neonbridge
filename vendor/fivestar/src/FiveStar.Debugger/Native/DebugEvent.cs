// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>DEBUG_EVENT (minwinbase.h), x64 layout: the code, the ids, then the union of the event records at 0x10.</summary>
[StructLayout(LayoutKind.Explicit, Size = 0xB0)]
internal struct DebugEvent
{
    [FieldOffset(0x0)] public DebugEventCode Code;
    [FieldOffset(0x4)] public uint ProcessId;
    [FieldOffset(0x8)] public uint ThreadId;
    [FieldOffset(0x10)] public ExceptionDebugInfo Exception;
    [FieldOffset(0x10)] public CreateThreadDebugInfo CreateThread;
    [FieldOffset(0x10)] public CreateProcessDebugInfo CreateProcess;

    /// <summary>EXIT_THREAD_DEBUG_INFO and EXIT_PROCESS_DEBUG_INFO: the exit code.</summary>
    [FieldOffset(0x10)] public uint ExitCode;
    [FieldOffset(0x10)] public LoadDllDebugInfo LoadDll;

    /// <summary>UNLOAD_DLL_DEBUG_INFO.lpBaseOfDll.</summary>
    [FieldOffset(0x10)] public ulong UnloadedBase;
    [FieldOffset(0x10)] public OutputDebugStringInfo DebugString;
    [FieldOffset(0x10)] public RipInfo Rip;
}
