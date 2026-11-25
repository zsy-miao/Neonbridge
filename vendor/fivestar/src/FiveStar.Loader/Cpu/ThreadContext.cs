// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// The fields of an AMD64 CONTEXT (winnt.h) the emulation reads and writes: the integer registers in x86 register
/// number order (rax, rcx, rdx, rbx, rsp, rbp, rsi, rdi, r8-r15), RIP and the XMM registers of its FltSave area.
/// </summary>
internal static unsafe class ThreadContext
{
    /// <summary>CONTEXT.ContextFlags, and CONTEXT_FLOATING_POINT (CONTEXT_AMD64 | 0x8): FltSave holds the XMM registers.</summary>
    private const int ContextFlagsOffset = 0x30;
    private const uint ContextFloatingPoint = 0x00100008;

    /// <summary>CONTEXT.Rax; the other integer registers follow at 8-byte steps.</summary>
    private const int RaxOffset = 0x78;

    private const int RipOffset = 0xF8;

    /// <summary>CONTEXT.Xmm0: FltSave (0x100) plus XMM_SAVE_AREA32.XmmRegisters (0xA0); 16 bytes per register.</summary>
    private const int Xmm0Offset = 0x1A0;
    private const int XmmSize = 16;

    /// <summary>Whether the CONTEXT carries the XMM registers, so the SSE4a emulation can read and write them.</summary>
    [HotPath]
    public static bool HasXmm(byte* context)
    {
        return (*(uint*)(context + ContextFlagsOffset) & ContextFloatingPoint) == ContextFloatingPoint;
    }

    [HotPath]
    public static ulong* Register(byte* context, int number)
    {
        return (ulong*)(context + RaxOffset + (number * sizeof(ulong)));
    }

    [HotPath]
    public static nint Rip(byte* context)
    {
        return *(nint*)(context + RipOffset);
    }

    [HotPath]
    public static void SetRip(byte* context, nint rip)
    {
        *(nint*)(context + RipOffset) = rip;
    }

    /// <summary>XMM register <paramref name="number"/> as its low and high quadwords.</summary>
    [HotPath]
    public static ulong* Xmm(byte* context, int number)
    {
        return (ulong*)(context + Xmm0Offset + (number * XmmSize));
    }
}
