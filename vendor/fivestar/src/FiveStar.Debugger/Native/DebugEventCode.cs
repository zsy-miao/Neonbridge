// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>DEBUG_EVENT.dwDebugEventCode (minwinbase.h).</summary>
internal enum DebugEventCode : uint
{
    Exception = 1,
    CreateThread = 2,
    CreateProcess = 3,
    ExitThread = 4,
    ExitProcess = 5,
    LoadDll = 6,
    UnloadDll = 7,
    OutputDebugString = 8,
    Rip = 9,
}
