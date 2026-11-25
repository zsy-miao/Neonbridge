// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// STARTUPINFOW (processthreadsapi.h), x64 layout: 0x68 bytes. Only cb is set; the rest stays zero, so the program
/// inherits the console and the standard handles.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = 0x68)]
internal struct StartupInfo
{
    [FieldOffset(0x0)] public uint Size;
}
