// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Packed 16-bit pairs (VOP3P v_pk_*): each half computed separately. op_sel picks the source half of the low result,
/// op_sel_hi that of the high result; neg_lo/neg_hi negate per half.
/// </summary>
internal sealed partial class PackedMathEmitter
{
    private const int HalfBits = 16;
    private const int ThreeSources = 3;

    private readonly OperandAccess _operands;

    public PackedMathEmitter(OperandAccess operands)
    {
        _operands = operands;
    }

    /// <summary>Translates the instruction when it is a packed operation; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var floats = PackedHalfPattern().Match(op);

        if (floats.Success)
        {
            PackedHalf(op, floats.Groups[1].Value, a, modifiers);

            return true;
        }

        var integers = PackedIntegerPattern().Match(op);

        if (!integers.Success)
        {
            return false;
        }

        PackedInteger(integers.Groups[1].Value, a);

        return true;
    }

    private void PackedHalf(string op, string kind, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var count = kind switch
        {
            "fma" or "mad" => ThreeSources,
            "add" or "mul" or "max" or "min" or "sub" => PairDwords,
            _ => 0,
        };

        if (count == 0)
        {
            throw new UnsupportedProgramException(op);
        }

        var selectHigh = ArgumentList.Bits(modifiers.Value("op_sel_hi"));

        if (selectHigh.Count == 0)
        {
            selectHigh = [1, 1, 1];
        }

        selectHigh.AddRange([1, 1, 1]);
        var lanes = new PackedLanes(
            HalfPrecisionEmitter.Padded(modifiers.Value("op_sel"), 0),
            selectHigh,
            HalfPrecisionEmitter.Padded(modifiers.Value("neg_lo"), 0),
            HalfPrecisionEmitter.Padded(modifiers.Value("neg_hi"), 0));
        var low = Combine(kind, count, a, lanes, high: false);
        var high = Combine(kind, count, a, lanes, high: true);

        if (modifiers.Has("clamp"))
        {
            (low, high) = ($"saturate({low})", $"saturate({high})");
        }

        _operands.Write(a[Destination], $"f32tof16({low}) | (f32tof16({high}) << 16)");
    }

    private string Combine(string kind, int count, IReadOnlyList<Operand> a, PackedLanes lanes, bool high)
    {
        var s = new List<string>();

        for (var source = 0; source < count; source++)
        {
            s.Add(Source(a[source + 1], source, lanes, high));
        }

        while (s.Count < ThreeSources)
        {
            s.Add("0.0");
        }

        return kind switch
        {
            "add" => $"{s[0]} + {s[1]}",
            "sub" => $"{s[0]} - {s[1]}",
            "mul" => $"{s[0]} * {s[1]}",
            "max" or "min" => $"{kind}({s[0]}, {s[1]})",
            _ => $"mad({s[0]}, {s[1]}, {s[^1]})",
        };
    }

    private string Source(Operand operand, int source, PackedLanes lanes, bool high)
    {
        var selectList = high ? lanes.SelectHigh : lanes.Select;
        var negateList = high ? lanes.NegateHigh : lanes.NegateLow;
        var value = _operands.Read(operand);
        var half = selectList[source] == 1 ? $"f16tof32({value} >> 16)" : $"f16tof32({value})";

        if (negateList[source] != 0)
        {
            half = $"(-{half})";
        }

        return half;
    }

    private void PackedInteger(string kind, IReadOnlyList<Operand> a)
    {
        var x = _operands.Read(a[Source0]);
        var y = _operands.Read(a[Source1]);
        var low = IntegerHalf(kind, x, y, 0);
        var high = IntegerHalf(kind, x, y, 1);
        _operands.Write(a[Destination], $"({low} & 0xffffu) | (({high} & 0xffffu) << 16)");
    }

    private static string IntegerHalf(string kind, string x, string y, int half)
    {
        var p = $"(({x} >> {HalfBits * half}) & 0xffffu)";
        var q = $"(({y} >> {HalfBits * half}) & 0xffffu)";

        return kind switch
        {
            "add" => $"({p} + {q})",
            "sub" => $"({p} - {q})",
            "mul_lo" => $"({p} * {q})",
            "max" or "min" => $"{kind}({p}, {q})",
            "lshlrev" => $"({q} << ({p} & 15))",
            "lshrrev" => $"({q} >> ({p} & 15))",
            _ => $"asuint((asint({q} << 16) >> 16) >> ({p} & 15))",
        };
    }

    [GeneratedRegex(@"^v_pk_(\w+)_f16$")]
    private static partial Regex PackedHalfPattern();

    [GeneratedRegex(@"^v_pk_(add|sub|mul_lo|max|min|lshlrev|lshrrev|ashrrev)_(u16|i16|b16)$")]
    private static partial Regex PackedIntegerPattern();
}
