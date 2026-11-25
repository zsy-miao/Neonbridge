// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>EXCEPTION_DEBUG_INFO (minwinbase.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal struct ExceptionDebugInfo
{
    public ExceptionRecord Record;

    /// <summary>Nonzero the first time the debugger sees the exception, 0 when no handler took it (second chance).</summary>
    public uint FirstChance;
}
