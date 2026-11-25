// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Runs all but the last pass of a cache-line loop in one fault:
/// <c>clzero</c> or <c>clwb [p]</c>, then <c>add p,64; cmp p,end; jb/jbe</c> back to it. Every pass would fault
/// otherwise. The pointer register is advanced to the last pass and RIP stays on the instruction, so the last pass
/// faults once more, is emulated alone, and the hardware runs the add, cmp and branch, which leave the flags as the
/// loop would. Intel SDM volume 2 for the encodings (ADD 83 /0 ib, CMP 39 /r and 3B /r, JB 72 cb, JBE 76 cb).
/// </summary>
internal static unsafe class LineLoop
{
    /// <summary>The longest range run in one fault; a longer one (or a corrupt pointer) is left to the single passes.</summary>
    private const ulong MaxRange = 256UL * 1024 * 1024;

    private const int Rax = 0;

    /// <summary>REX.W plus REX.R (bit 2) and REX.B (bit 0); REX.X (bit 1) is never part of these forms.</summary>
    private const byte RexW = 0x48;
    private const int RexR = 0x04;
    private const int RexB = 0x01;
    private const int RexMaskWithoutX = 0xFD;
    private const byte RexFirst = 0x40;
    private const byte RexLast = 0x4F;
    private const int ExtendedRegister = 8;

    /// <summary>ADD r/m64, imm8 (83 /0) with a register operand: ModRM C0 + register.</summary>
    private const byte AddImm8Opcode = 0x83;
    private const byte ModRegister = 0xC0;
    private const int RegShift = 3;
    private const int FieldMask = 7;

    /// <summary>CMP r/m64, r64 (39 /r) and CMP r64, r/m64 (3B /r).</summary>
    private const byte CmpRmReg = 0x39;
    private const byte CmpRegRm = 0x3B;

    /// <summary>The lengths of the three instructions after the cache-line instruction.</summary>
    private const int AddLength = 4;
    private const int CmpLength = 3;
    private const int JccLength = 2;

    /// <summary>Byte positions in the REX-prefixed add and cmp: opcode, ModRM, the add's imm8.</summary>
    private const int OpcodeByte = 1;
    private const int ModRmByte = 2;
    private const int Imm8Byte = 3;

    /// <summary>JB rel8 and JBE rel8.</summary>
    private const byte JbRel8 = 0x72;
    private const byte JbeRel8 = 0x76;

    /// <summary>CLWB's ModRM is the last byte of 66 [REX] 0F AE /6; mod 00 with rm other than 100b and 101b is <c>[reg]</c>.</summary>
    private const int ClwbShortLength = 4;
    private const int ModShift = 6;
    private const int RmSib = 4;
    private const int RmDisp32 = 5;

    /// <summary>
    /// The passes run when the faulting instruction heads such a loop; 0 when it does not or when only one pass is
    /// left. <paramref name="code"/> starts at the instruction, <paramref name="marker"/> bytes after the faulting
    /// address, which the branch must return to.
    /// </summary>
    [HotPath]
    public static long TrySkip(byte* context, ReadOnlySpan<byte> code, in AmdInstruction instruction, int marker)
    {
        int pointer = PointerRegister(code, instruction);

        if (pointer < 0)
        {
            return 0;
        }

        int at = instruction.Length;
        int end = MatchAdd(code, at, pointer);
        int limit = end < 0 ? -1 : MatchCmp(code, end, pointer);
        int branch = limit < 0 ? -1 : end + CmpLength;

        if (branch < 0 || branch + 1 >= code.Length || (sbyte)code[branch + 1] != -(marker + branch + JccLength))
        {
            return 0;
        }

        bool inclusive = code[branch] == JbeRel8;

        if (!inclusive && code[branch] != JbRel8)
        {
            return 0;
        }

        return RunPasses(context, instruction, pointer, limit, inclusive);
    }

    /// <summary>The register the instruction addresses: rax for CLZERO, the base of a plain <c>[reg]</c> CLWB; -1 otherwise.</summary>
    private static int PointerRegister(ReadOnlySpan<byte> code, in AmdInstruction instruction)
    {
        if (instruction.Kind == AmdInstructionKind.Clzero)
        {
            int address = Rax;

            if (instruction.AddressSize32)
            {
                address = -1;
            }

            return address;
        }

        if (instruction.Kind != AmdInstructionKind.Clwb || instruction.Length < ClwbShortLength || instruction.Length > ClwbShortLength + 1)
        {
            return -1;
        }

        int modRm = code[instruction.Length - 1];
        int rm = modRm & FieldMask;
        int rex = instruction.Length > ClwbShortLength ? code[1] : 0;

        if (modRm >> ModShift != 0 || rm == RmSib || rm == RmDisp32 || (instruction.Length > ClwbShortLength && (rex < RexFirst || rex > RexLast)))
        {
            return -1;
        }

        return Extend(rm, rex, RexB);
    }

    /// <summary>Matches <c>add pointer,64</c> at <paramref name="at"/>; returns the offset after it, or -1.</summary>
    private static int MatchAdd(ReadOnlySpan<byte> code, int at, int pointer)
    {
        if (at + AddLength > code.Length)
        {
            return -1;
        }

        int rex = RexW;

        if (pointer >= ExtendedRegister)
        {
            rex |= RexB;
        }

        bool match = code[at] == rex && code[at + OpcodeByte] == AddImm8Opcode && code[at + ModRmByte] == (ModRegister | (pointer & FieldMask)) && code[at + Imm8Byte] == AmdInstructionEmulator.CacheLineSize;

        if (!match)
        {
            return -1;
        }

        return at + AddLength;
    }

    /// <summary>Matches <c>cmp pointer,limit</c> at <paramref name="at"/>; returns the limit register, or -1.</summary>
    private static int MatchCmp(ReadOnlySpan<byte> code, int at, int pointer)
    {
        if (at + CmpLength > code.Length || code[at] > RexLast || (code[at] & RexMaskWithoutX) != code[at] || (code[at] & RexW) != RexW || (code[at + ModRmByte] & ModRegister) != ModRegister)
        {
            return -1;
        }

        int rex = code[at];
        int reg = Extend((code[at + ModRmByte] >> RegShift) & FieldMask, rex, RexR);
        int rm = Extend(code[at + ModRmByte] & FieldMask, rex, RexB);

        if (code[at + OpcodeByte] == CmpRmReg && rm == pointer)
        {
            return reg;
        }

        if (code[at + OpcodeByte] == CmpRegRm && reg == pointer)
        {
            return rm;
        }

        return -1;
    }

    /// <summary>A ModRM register field with its REX extension bit: registers 8-15.</summary>
    private static int Extend(int field, int rex, int rexBit)
    {
        if ((rex & rexBit) == 0)
        {
            return field;
        }

        return field | ExtendedRegister;
    }

    /// <summary>Runs every pass but the last and leaves the pointer at the last one.</summary>
    private static long RunPasses(byte* context, in AmdInstruction instruction, int pointer, int limit, bool inclusive)
    {
        ulong* register = ThreadContext.Register(context, pointer);
        ulong start = *register;
        ulong last = *ThreadContext.Register(context, limit);

        if (last < start || last - start > MaxRange)
        {
            return 0;
        }

        ulong range = last - start;
        ulong step = (ulong)AmdInstructionEmulator.CacheLineSize;
        ulong passes = inclusive ? (range / step) + 1 : (range + step - 1) / step;

        if (passes <= 1)
        {
            return 0;
        }

        ulong skipped = passes - 1;

        if (instruction.Kind == AmdInstructionKind.Clzero)
        {
            for (ulong pass = 0; pass < skipped; pass++)
            {
                AmdInstructionEmulator.ZeroLine((nint)((start + (pass * step)) & ~(step - 1)));
            }
        }

        *register = start + (skipped * step);

        return (long)skipped;
    }
}
