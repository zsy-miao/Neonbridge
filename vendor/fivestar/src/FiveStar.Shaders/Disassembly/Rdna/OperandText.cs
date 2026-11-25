// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// Spells registers and constants the way llvm-objdump does. Operand codes are the 9-bit source encoding of the AMD
/// "RDNA 2 Instruction Set Architecture" reference guide (SSRC/SRC0 table): 0-105 SGPRs, 106-127 special registers,
/// 128-208 inline integers, 240-248 inline floats, 255 a literal dword, 256-511 VGPRs.
/// </summary>
internal static class OperandText
{
    public const int VccLo = 106;
    public const int Null = 125;
    public const int ExecLo = 126;
    public const int Literal = 255;
    public const int FirstVgpr = 256;
    public const int Sdwa = 249;
    public const int Dpp16 = 250;
    public const int Dpp8 = 233;
    public const int Dpp8FetchInactive = 234;

    /// <summary>Stands for an operand llvm-objdump cannot decode (a range past the last register); the dword is then .long.</summary>
    public const string Undecodable = "\u0002";

    /// <summary>Marks an instruction llvm-objdump lists with an alignment warning; such instructions are left out.</summary>
    public const string Unaligned = "\u0001";

    private const int LastSgpr = 105;
    private const int LastVgpr = 255;
    private const int VccHi = 107;
    private const int FirstTtmp = 108;
    private const int LastTtmp = 123;
    private const int M0 = 124;
    private const int ExecHi = 127;
    private const int SharedBase = 235;
    private const int PopsExitingWave = 239;
    private const int Vccz = 251;
    private const int Scc = 253;
    private const int IntegerZero = 128;
    private const int LastPositive = 192;
    private const int LastNegative = 208;
    private const int FirstFloat = 240;
    private const int LastFloat = 248;
    private const int PairDwords = 2;
    private const int QuadDwords = 4;
    private const int LowBits = 32;
    private const int InlineMinimum = -16;
    private const int InlineMaximum = 64;
    private const ulong LowDwordMask = 0xFFFF_FFFF;
    private const uint HalfMask = 0xFFFF;

    /// <summary>The inline float constants 240-248 in order; the last is 1/(2*pi).</summary>
    private static readonly string[] FloatNames = ["0.5", "-0.5", "1.0", "-1.0", "2.0", "-2.0", "4.0", "-4.0", "0.15915494"];

    private const string InverseTwoPiDouble = "0.15915494309189532";

    /// <summary>Bit patterns of the inline floats, by width, in the order of <see cref="FloatNames"/>.</summary>
    private static readonly uint[] Float32Bits =
        [0x3F000000, 0xBF000000, 0x3F800000, 0xBF800000, 0x40000000, 0xC0000000, 0x40800000, 0xC0800000, 0x3E22F983];

    private static readonly ulong[] Float64Bits =
    [
        0x3FE0000000000000, 0xBFE0000000000000, 0x3FF0000000000000, 0xBFF0000000000000, 0x4000000000000000,
        0xC000000000000000, 0x4010000000000000, 0xC010000000000000, 0x3FC45F306DC9C882,
    ];

    private static readonly uint[] Float16Bits = [0x3800, 0xB800, 0x3C00, 0xBC00, 0x4000, 0xC000, 0x4400, 0xC400, 0x3118];

    private static readonly Dictionary<int, string> Named = new()
    {
        [VccHi] = "vcc_hi",
        [M0] = "m0",
        [Null] = "null",
        [ExecHi] = "exec_hi",
        [235] = "src_shared_base",
        [236] = "src_shared_limit",
        [237] = "src_private_base",
        [238] = "src_private_limit",
        [239] = "src_pops_exiting_wave_id",
        [251] = "src_vccz",
        [252] = "src_execz",
        [253] = "src_scc",
        [254] = "src_lds_direct",
    };

    /// <summary>Dwords an operand of this type occupies in registers.</summary>
    public static int Dwords(OperandType type, bool wave64)
    {
        if (type == OperandType.Quad)
        {
            return QuadDwords;
        }

        if (type is OperandType.WideInteger or OperandType.WideReal || (type == OperandType.LaneMask && wave64))
        {
            return PairDwords;
        }

        return 1;
    }

    /// <summary>A VGPR or VGPR range; a range past v255 is <see cref="Undecodable"/>.</summary>
    public static string Vgpr(int index, int dwords)
    {
        if (index + dwords - 1 > LastVgpr)
        {
            return Undecodable;
        }

        if (dwords == 1)
        {
            return "v" + index.ToString(CultureInfo.InvariantCulture);
        }

        return string.Create(CultureInfo.InvariantCulture, $"v[{index}:{index + dwords - 1}]");
    }

    /// <summary>A scalar register range by its 7-bit code, or null when the code names no register of that width.</summary>
    public static string? Scalar(int code, int dwords)
    {
        if (code <= LastSgpr)
        {
            return Aligned("s", code, dwords, LastSgpr);
        }

        if (code is >= FirstTtmp and <= LastTtmp)
        {
            return Aligned("ttmp", code - FirstTtmp, dwords, LastTtmp - FirstTtmp);
        }

        if (dwords == 1)
        {
            return Single(code);
        }

        if (dwords == PairDwords)
        {
            return Pair(code);
        }

        return null;
    }

    /// <summary>A 9-bit source: register, inline constant or (code 255) the literal; null for codes with no meaning.</summary>
    public static string? Source(int code, OperandType type, bool wave64, uint literal)
    {
        if (code >= FirstVgpr)
        {
            return Vgpr(code - FirstVgpr, Dwords(type, wave64));
        }

        if (code is >= IntegerZero and <= LastNegative)
        {
            return InlineInteger(code).ToString(CultureInfo.InvariantCulture);
        }

        if (code is >= FirstFloat and <= LastFloat)
        {
            return InlineFloat(code, type);
        }

        if (code == Literal)
        {
            return Constant(literal, type);
        }

        return Scalar(code, Dwords(type, wave64));
    }

    /// <summary>A constant as llvm-objdump prints an immediate of this type: small integers in decimal, inline floats by value, else hex.</summary>
    public static string Constant(uint value, OperandType type)
    {
        switch (type)
        {
            case OperandType.WideInteger:
                return Constant64(value, value);
            case OperandType.WideReal:
                return Constant64((ulong)value << LowBits, value);
            case OperandType.HalfInteger:
            case OperandType.HalfReal:
                return Constant16(value);
            case OperandType.PackedInteger:
                return Constant32(value);
            case OperandType.PackedReal:
                return ConstantPacked(value);
            default:
                return Constant32(value);
        }
    }

    /// <summary>A register outside its operand's register class, flagged the way llvm-objdump flags it.</summary>
    public static string Invalid(string register, string registerClass)
    {
        return $"{register}/*Invalid register, operand has '{registerClass}' register class*/";
    }

    /// <summary>Signed hex: <c>-0x10</c> for negative values.</summary>
    public static string SignedHex(long value)
    {
        if (value < 0)
        {
            return "-" + Hex((ulong)-value);
        }

        return Hex((ulong)value);
    }

    /// <summary>Lower-case hex with a 0x prefix and no padding.</summary>
    public static string Hex(ulong value)
    {
        return "0x" + value.ToString("x", CultureInfo.InvariantCulture);
    }

    private static int InlineInteger(int code)
    {
        if (code <= LastPositive)
        {
            return code - IntegerZero;
        }

        return LastPositive - code;
    }

    private static string InlineFloat(int code, OperandType type)
    {
        if (code == LastFloat && type is OperandType.WideReal or OperandType.WideInteger)
        {
            return InverseTwoPiDouble;
        }

        return FloatNames[code - FirstFloat];
    }

    private static string Constant32(uint value)
    {
        var signed = (int)value;

        if (signed is >= InlineMinimum and <= InlineMaximum)
        {
            return signed.ToString(CultureInfo.InvariantCulture);
        }

        var index = Array.IndexOf(Float32Bits, value);

        if (index >= 0)
        {
            return FloatNames[index];
        }

        return Hex(value);
    }

    private static string Constant64(ulong value, uint printed)
    {
        var signed = (long)value;

        if (signed is >= InlineMinimum and <= InlineMaximum)
        {
            return signed.ToString(CultureInfo.InvariantCulture);
        }

        var index = Array.IndexOf(Float64Bits, value);

        if (index >= 0)
        {
            return FloatNames[index];
        }

        return Hex(printed & LowDwordMask);
    }

    /// <summary>Packed f16: a small integer, an f16 inline value in the low half with a clear high half, else hex.</summary>
    private static string ConstantPacked(uint value)
    {
        var signed = (int)value;

        if (signed is >= InlineMinimum and <= InlineMaximum)
        {
            return signed.ToString(CultureInfo.InvariantCulture);
        }

        var index = Array.IndexOf(Float16Bits, value);

        if (index >= 0)
        {
            return FloatNames[index];
        }

        return Hex(value);
    }

    private static string Constant16(uint value)
    {
        var signed = (short)value;

        if (signed is >= InlineMinimum and <= InlineMaximum)
        {
            return signed.ToString(CultureInfo.InvariantCulture);
        }

        var index = Array.IndexOf(Float16Bits, value & HalfMask);

        if (index >= 0)
        {
            return FloatNames[index];
        }

        return Hex(value & HalfMask);
    }

    private static int Align(int index, int dwords)
    {
        if (dwords >= QuadDwords)
        {
            return index & ~(QuadDwords - 1);
        }

        if (dwords == PairDwords)
        {
            return index & ~1;
        }

        return index;
    }

    /// <summary>
    /// A register range rounded down to its alignment, as llvm-objdump decodes it; an unaligned one also carries
    /// <see cref="Unaligned"/>, since llvm-objdump then adds a warning that its listing parser does not accept.
    /// </summary>
    private static string Aligned(string prefix, int index, int dwords, int last)
    {
        var first = Align(index, dwords);

        if (first + dwords - 1 > last)
        {
            return Undecodable;
        }

        var text = Range(prefix, first, dwords);

        if (first != index)
        {
            text += Unaligned;
        }

        return text;
    }

    private static string Range(string prefix, int first, int dwords)
    {
        if (dwords == 1)
        {
            return prefix + first.ToString(CultureInfo.InvariantCulture);
        }

        return string.Create(CultureInfo.InvariantCulture, $"{prefix}[{first}:{first + dwords - 1}]");
    }

    private static string? Single(int code)
    {
        if (code == VccLo)
        {
            return "vcc_lo";
        }

        if (code == ExecLo)
        {
            return "exec_lo";
        }

        return Named.GetValueOrDefault(code);
    }

    private static string? Pair(int code)
    {
        switch (code)
        {
            case VccLo:
                return "vcc";
            case ExecLo:
                return "exec";
            case Null:
                return "null";
            case >= SharedBase and <= PopsExitingWave:
            case >= Vccz and <= Scc:
                return Named[code];
            default:
                return null;
        }
    }
}
