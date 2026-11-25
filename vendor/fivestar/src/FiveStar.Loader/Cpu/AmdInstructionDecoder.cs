// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Cpu;

/// <summary>
/// Decodes the AMD-only encodings the handler emulates (AMD64 Architecture Programmer's Manual, volume 3, appendix A
/// opcode maps, and volume 4 for SSE4a) and nothing else: any other byte sequence is rejected, so its fault goes on to
/// the next handler. Bytes are read one at a time and only as far as the encoding matches.
/// </summary>
internal static class AmdInstructionDecoder
{
    /// <summary>The architectural maximum length of an x86 instruction.</summary>
    public const int MaxLength = 15;

    /// <summary>Legacy prefixes: operand size (66h), REPNE (F2h), address size (67h), as flag bits.</summary>
    private const byte OperandSizePrefix = 0x66;
    private const byte RepnePrefix = 0xF2;
    private const byte AddressSizePrefix = 0x67;
    private const int OperandSize = 1;
    private const int Repne = 2;
    private const int AddressSize = 4;

    /// <summary>REX is 40h-4Fh; R (bit 2) extends ModRM.reg, B (bit 0) extends ModRM.rm.</summary>
    private const byte RexFirst = 0x40;
    private const byte RexLast = 0x4F;
    private const int RexR = 0x04;
    private const int RexB = 0x01;
    private const int RexRegisterBit = 8;

    /// <summary>Two-byte opcode escape and the opcodes after it.</summary>
    private const byte Escape = 0x0F;
    private const byte Group7 = 0x01;
    private const byte Sse4aImmediate = 0x78;
    private const byte Sse4aRegister = 0x79;
    private const byte Group15 = 0xAE;

    /// <summary>0F 01 with a register ModRM byte: MONITORX FA, MWAITX FB, CLZERO FC.</summary>
    private const byte MonitorXModRm = 0xFA;
    private const byte MWaitXModRm = 0xFB;
    private const byte ClzeroModRm = 0xFC;

    /// <summary>66 0F AE /6 with a memory operand is CLWB.</summary>
    private const int ClwbReg = 6;

    /// <summary>ModRM fields.</summary>
    private const int ModShift = 6;
    private const int RegShift = 3;
    private const int FieldMask = 7;
    private const int ModRegister = 3;
    private const int ModDisp8 = 1;
    private const int ModDisp32 = 2;

    /// <summary>rm = 100b needs a SIB byte; rm = 101b with mod 00 is RIP-relative, base = 101b in a SIB with mod 00 has a disp32.</summary>
    private const int RmSib = 4;
    private const int RmDisp32 = 5;
    private const int Disp32Size = 4;

    /// <summary>The escape and opcode bytes before the ModRM byte.</summary>
    private const int EscapedOpcodeLength = 2;

    /// <summary>EXTRQ and INSERTQ immediate forms end with the field length and index bytes, after the ModRM byte.</summary>
    private const int FieldImmediates = 2;
    private const int LengthImmediate = 1;
    private const int IndexImmediate = 2;

    /// <summary>Decodes the instruction at the start of <paramref name="code"/>.</summary>
    [HotPath]
    public static bool TryDecode(ReadOnlySpan<byte> code, out AmdInstruction instruction)
    {
        instruction = default;
        int prefixes = 0;
        int rex = 0;
        int at = 0;

        while (at < code.Length && PrefixFlag(code[at]) != 0)
        {
            int flag = PrefixFlag(code[at]);

            if ((prefixes & flag) != 0)
            {
                return false;
            }

            prefixes |= flag;
            at++;
        }

        if (at < code.Length && code[at] >= RexFirst && code[at] <= RexLast)
        {
            rex = code[at];
            at++;
        }

        int modRm = at + EscapedOpcodeLength;

        if (modRm >= code.Length || code[at] != Escape)
        {
            return false;
        }

        return code[at + 1] switch
        {
            Group7 => DecodeGroup7(code[modRm], modRm + 1, prefixes, out instruction),
            Sse4aImmediate => DecodeSse4a(code, modRm, prefixes, rex, true, out instruction),
            Sse4aRegister => DecodeSse4a(code, modRm, prefixes, rex, false, out instruction),
            Group15 => DecodeClwb(code, modRm, prefixes, out instruction),
            _ => false,
        };
    }

    private static int PrefixFlag(byte value)
    {
        return value switch
        {
            OperandSizePrefix => OperandSize,
            RepnePrefix => Repne,
            AddressSizePrefix => AddressSize,
            _ => 0,
        };
    }

    private static bool DecodeGroup7(byte modRm, int length, int prefixes, out AmdInstruction instruction)
    {
        instruction = default;

        if ((prefixes & (OperandSize | Repne)) != 0)
        {
            return false;
        }

        AmdInstructionKind kind;

        switch (modRm)
        {
            case ClzeroModRm:
                kind = AmdInstructionKind.Clzero;
                break;
            case MonitorXModRm:
                kind = AmdInstructionKind.MonitorX;
                break;
            case MWaitXModRm:
                kind = AmdInstructionKind.MWaitX;
                break;
            default:
                return false;
        }

        instruction = new AmdInstruction(kind, length, 0, 0, null, (prefixes & AddressSize) != 0);

        return true;
    }

    /// <summary>EXTRQ (66h) and INSERTQ (F2h); both take register operands only.</summary>
    private static bool DecodeSse4a(ReadOnlySpan<byte> code, int at, int prefixes, int rex, bool immediate, out AmdInstruction instruction)
    {
        instruction = default;
        bool extract = prefixes == OperandSize;
        int modRm = code[at];
        int regField = (modRm >> RegShift) & FieldMask;
        int length = at + 1;

        if (immediate)
        {
            length += FieldImmediates;
        }

        // EXTRQ xmm, imm8, imm8 is 66 0F 78 /0: the reg field must be 0 and the register is in rm.
        if ((prefixes != OperandSize && prefixes != Repne) || modRm >> ModShift != ModRegister || length > code.Length || (immediate && extract && regField != 0))
        {
            return false;
        }

        int reg = Extend(regField, rex, RexR);
        int rm = Extend(modRm & FieldMask, rex, RexB);
        BitFieldSpec? field = null;

        if (immediate)
        {
            field = new BitFieldSpec(code[at + LengthImmediate], code[at + IndexImmediate]);
        }

        var kind = extract ? AmdInstructionKind.Extrq : AmdInstructionKind.Insertq;
        int destination = immediate && extract ? rm : reg;
        instruction = new AmdInstruction(kind, length, destination, rm, field, false);

        return true;
    }

    /// <summary>A ModRM register field with its REX extension bit: registers 8-15.</summary>
    private static int Extend(int field, int rex, int rexBit)
    {
        if ((rex & rexBit) == 0)
        {
            return field;
        }

        return field | RexRegisterBit;
    }

    private static bool DecodeClwb(ReadOnlySpan<byte> code, int at, int prefixes, out AmdInstruction instruction)
    {
        instruction = default;
        int modRm = code[at];
        int mod = modRm >> ModShift;

        if ((prefixes & (OperandSize | Repne)) != OperandSize || mod == ModRegister || ((modRm >> RegShift) & FieldMask) != ClwbReg)
        {
            return false;
        }

        int length = MemoryOperandEnd(code, at, mod, modRm & FieldMask);

        if (length > code.Length)
        {
            return false;
        }

        instruction = new AmdInstruction(AmdInstructionKind.Clwb, length, 0, 0, null, (prefixes & AddressSize) != 0);

        return true;
    }

    /// <summary>Where a memory ModRM operand starting at <paramref name="at"/> ends: SIB and displacement included.</summary>
    private static int MemoryOperandEnd(ReadOnlySpan<byte> code, int at, int mod, int rm)
    {
        int end = at + 1;
        bool disp32 = mod == ModDisp32 || (mod == 0 && rm == RmDisp32);

        if (rm == RmSib)
        {
            if (end >= code.Length)
            {
                return int.MaxValue;
            }

            disp32 |= mod == 0 && (code[end] & FieldMask) == RmDisp32;
            end++;
        }

        if (mod == ModDisp8)
        {
            end++;
        }

        if (disp32)
        {
            end += Disp32Size;
        }

        return end;
    }
}
