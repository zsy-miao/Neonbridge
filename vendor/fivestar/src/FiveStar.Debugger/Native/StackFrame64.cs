// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>
/// STACKFRAME64 (dbghelp.h), x64 layout: 0x108 bytes. The debugger sets the program counter, frame and stack addresses
/// of the first frame and reads the program counter and stack of each; AddrBStore, the function table entry, Params,
/// Far, Virtual, Reserved and KdHelp stay zero for dbghelp.
/// </summary>
[StructLayout(LayoutKind.Explicit, Size = 0x108)]
internal struct StackFrame64
{
    [FieldOffset(0x0)] public Address64 ProgramCounter;
    [FieldOffset(0x10)] public Address64 Return;
    [FieldOffset(0x20)] public Address64 Frame;
    [FieldOffset(0x30)] public Address64 Stack;
}
