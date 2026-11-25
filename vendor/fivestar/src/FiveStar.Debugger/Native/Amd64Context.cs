// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Debugger.Native;

/// <summary>CONTEXT (winnt.h), AMD64 layout: 0x4D0 bytes, 16-byte aligned. Only the fields the debugger reads are named.</summary>
[StructLayout(LayoutKind.Explicit, Size = 0x4D0)]
internal unsafe struct Amd64Context
{
    /// <summary>CONTEXT_ALL: CONTEXT_AMD64 with control, integer, segment, floating-point and debug registers.</summary>
    public const uint All = 0x10001F;

    /// <summary>DECLSPEC_ALIGN(16) on CONTEXT.</summary>
    public const int Alignment = 16;

    /// <summary>Rax through R15, in CONTEXT order.</summary>
    public const int GeneralRegisterCount = 16;

    [FieldOffset(0x30)] public uint ContextFlags;
    [FieldOffset(0x44)] public uint EFlags;

    /// <summary>Rax, Rcx, Rdx, Rbx, Rsp, Rbp, Rsi, Rdi, R8 to R15.</summary>
    [FieldOffset(0x78)] public fixed ulong GeneralRegisters[GeneralRegisterCount];
    [FieldOffset(0x98)] public ulong Rsp;
    [FieldOffset(0xA0)] public ulong Rbp;
    [FieldOffset(0xF8)] public ulong Rip;
}
