// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>Source and output modifiers as llvm-objdump spells them.</summary>
internal static class Modifiers
{
    private const int MultiplyByTwo = 1;
    private const int MultiplyByFour = 2;
    private const int DivideByTwo = 3;
    private const int IntegerZero = 128;
    private const int LastNegative = 208;
    private const int FirstFloat = 240;
    private const int LastFloat = 248;

    /// <summary>Whether operands of this type take the float modifiers neg and abs.</summary>
    public static bool IsFloat(OperandType type)
    {
        return type is OperandType.Real or OperandType.WideReal or OperandType.HalfReal;
    }

    /// <summary>
    /// <c>-x</c>, <c>|x|</c> or <c>-|x|</c>; a negated constant without abs prints <c>neg(x)</c> since <c>-1</c> would be a
    /// different constant.
    /// </summary>
    public static string Apply(string operand, bool negate, bool absolute, bool constant)
    {
        var text = operand;

        if (absolute)
        {
            text = "|" + text + "|";
        }

        if (!negate)
        {
            return text;
        }

        if (constant && !absolute)
        {
            return "neg(" + text + ")";
        }

        return "-" + text;
    }

    /// <summary>Whether a source code is a constant rather than a register.</summary>
    public static bool IsConstant(int code)
    {
        return code is (>= IntegerZero and <= LastNegative) or (>= FirstFloat and <= LastFloat) or OperandText.Literal;
    }

    /// <summary>The output modifier: " mul:2", " mul:4", " div:2" or nothing.</summary>
    public static string Output(int omod)
    {
        switch (omod)
        {
            case MultiplyByTwo:
                return " mul:2";
            case MultiplyByFour:
                return " mul:4";
            case DivideByTwo:
                return " div:2";
            default:
                return string.Empty;
        }
    }

    /// <summary>A bracketed list of bits: <c>[1,0,0]</c>.</summary>
    public static string BitList(int bits, int count)
    {
        var values = Enumerable.Range(0, count).Select(index => ((bits >> index) & 1).ToString(CultureInfo.InvariantCulture));

        return "[" + string.Join(',', values) + "]";
    }
}
