// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Debugger.Native;

/// <summary>The CreateProcess creation flags the debugger uses (WinBase.h).</summary>
[Flags]
internal enum ProcessCreationFlags : uint
{
    None = 0,

    /// <summary>DEBUG_PROCESS: debug the process and every process it creates.</summary>
    DebugProcess = 0x1,

    /// <summary>DEBUG_ONLY_THIS_PROCESS: debug the process alone.</summary>
    DebugOnlyThisProcess = 0x2,

    /// <summary>CREATE_UNICODE_ENVIRONMENT: the environment block is UTF-16.</summary>
    CreateUnicodeEnvironment = 0x400,
}
