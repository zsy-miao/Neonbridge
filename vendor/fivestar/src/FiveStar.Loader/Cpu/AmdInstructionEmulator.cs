// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.Intrinsics.X86;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Performs a decoded AMD-only instruction on a faulting thread's CONTEXT (AMD64 Architecture Programmer's Manual,
/// volume 3 for CLZERO, MONITORX and MWAITX, volume 4 for EXTRQ and INSERTQ). RIP is left to the caller.
/// </summary>
internal static unsafe class AmdInstructionEmulator
{
    /// <summary>CLZERO clears one cache line: 64 bytes on Zen 2 (CPUID Fn8000_0005 ECX[7:0], the L1 data line size).</summary>
    public const int CacheLineSize = 64;

    /// <summary>Register numbers in the CONTEXT: rax, rcx, rbx.</summary>
    private const int Rax = 0;
    private const int Rcx = 1;
    private const int Rbx = 3;

    /// <summary>MWAITX ECX bit 1: EBX holds the longest wait in TSC ticks.</summary>
    private const ulong TimerEnable = 2;

    /// <summary>A timed MWAITX up to this many ticks only pauses: the fault itself takes longer (about 1 us = 4000 ticks).</summary>
    private const uint ShortWaitTicks = 4000;

    /// <summary>The low 32 bits of a register, the address under a 67h prefix.</summary>
    private const ulong LowDword = 0xFFFF_FFFF;

    /// <summary>Whether the CONTEXT holds what the instruction needs (the XMM registers for SSE4a).</summary>
    [HotPath]
    public static bool CanRun(byte* context, in AmdInstruction instruction)
    {
        bool sse4a = instruction.Kind == AmdInstructionKind.Extrq || instruction.Kind == AmdInstructionKind.Insertq;

        return !sse4a || ThreadContext.HasXmm(context);
    }

    [HotPath]
    public static void Execute(byte* context, in AmdInstruction instruction)
    {
        switch (instruction.Kind)
        {
            case AmdInstructionKind.Extrq:
                Extrq(context, instruction);
                break;
            case AmdInstructionKind.Insertq:
                Insertq(context, instruction);
                break;
            case AmdInstructionKind.Clzero:
                ZeroLine(LineAddress(context, instruction));
                break;
            case AmdInstructionKind.MWaitX:
                MWaitX(context);
                break;
            default:
                // MONITORX arms nothing to wake on and CLWB has no persistent memory to write back to on the PC.
                break;
        }
    }

    /// <summary>The cache line CLZERO clears: rAX (EAX under 67h) rounded down to the line.</summary>
    [HotPath]
    public static nint LineAddress(byte* context, in AmdInstruction instruction)
    {
        ulong address = *ThreadContext.Register(context, Rax);

        if (instruction.AddressSize32)
        {
            address &= LowDword;
        }

        return (nint)(address & ~(ulong)(CacheLineSize - 1));
    }

    [HotPath]
    public static void ZeroLine(nint line)
    {
        new Span<byte>((void*)line, CacheLineSize).Clear();
    }

    /// <summary>
    /// EXTRQ: the destination's low quadword becomes the field, zero-extended. The manual leaves the high quadword
    /// undefined; Zen hardware clears it (checked by the self-test against the CPU), and so does this.
    /// </summary>
    private static void Extrq(byte* context, in AmdInstruction instruction)
    {
        ulong* destination = ThreadContext.Xmm(context, instruction.Destination);
        var field = instruction.Field ?? BitField.FromControl(ThreadContext.Xmm(context, instruction.Source)[0]);
        destination[0] = BitField.Extract(destination[0], field);
        destination[1] = 0;
    }

    /// <summary>INSERTQ: the register form's control is the source's high quadword; the high quadword is cleared as by EXTRQ.</summary>
    private static void Insertq(byte* context, in AmdInstruction instruction)
    {
        ulong* destination = ThreadContext.Xmm(context, instruction.Destination);
        ulong* source = ThreadContext.Xmm(context, instruction.Source);
        var field = instruction.Field ?? BitField.FromControl(source[1]);
        destination[0] = BitField.Insert(destination[0], source[0], field);
        destination[1] = 0;
    }

    /// <summary>MWAITX: nothing wakes an emulated monitor, so a short timed wait pauses and any other wait yields.</summary>
    private static void MWaitX(byte* context)
    {
        ulong hints = *ThreadContext.Register(context, Rcx);
        ulong ticks = *ThreadContext.Register(context, Rbx) & LowDword;

        if ((hints & TimerEnable) != 0 && ticks <= ShortWaitTicks)
        {
            X86Base.Pause();

            return;
        }

        _ = Kernel32.SwitchToThread();
    }
}
