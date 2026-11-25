// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>EXCEPTION_POINTERS (winnt.h).</summary>
[StructLayout(LayoutKind.Sequential)]
internal unsafe struct ExceptionPointers
{
    public ExceptionRecord* Record;
    public Amd64Context* Context;
}
