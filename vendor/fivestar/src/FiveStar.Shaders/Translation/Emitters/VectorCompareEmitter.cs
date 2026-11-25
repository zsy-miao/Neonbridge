// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// v_cmp / v_cmpx: the result is a lane mask (a per-lane bool), false on inactive lanes. v_cmpx writes EXEC only.
/// 16-bit compares convert both halves to 32-bit first; class tests use the prelude's cmp_class.
/// </summary>
internal sealed partial class VectorCompareEmitter
{
    private const uint SignBit = 0x80000000;
    private const long TwoToThe32 = 0x100000000;

    private readonly OperandAccess _operands;

    public VectorCompareEmitter(OperandAccess operands)
    {
        _operands = operands;
    }

    /// <summary>Translates the instruction when it is a vector compare; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var wide = Compare64Pattern().Match(op);

        if (wide.Success)
        {
            Compare64(wide.Groups["x"].Value.Length > 0, wide.Groups["cond"].Value, wide.Groups["type"].Value, a);

            return true;
        }

        var compare = ComparePattern().Match(op);

        if (!compare.Success)
        {
            return false;
        }

        Compare(op, compare.Groups["x"].Value.Length > 0, compare.Groups["cond"].Value, compare.Groups["type"].Value, a, modifiers);

        return true;
    }

    private void Compare64(bool writesExec, string condition, string type, IReadOnlyList<Operand> a)
    {
        var operands = a.ToList();

        if (writesExec)
        {
            operands = [OperandParser.Parse("exec"), a[^PairDwords], a[^1]];
        }

        var symbol = condition == "ne" ? "!=" : SaluEmitter.CompareSymbol(condition);
        var cast = type == "u64" ? "uint64_t" : "int64_t";
        var left = Value64(operands[Source0], cast, type);
        var right = Value64(operands[Source1], cast, type);
        _operands.WriteMask(operands[Destination], $"active && ({left} {symbol} {right})");
    }

    private string Value64(Operand operand, string cast, string type)
    {
        if (operand.IsImmediate)
        {
            long value = operand.Value;

            if (operand.Value >= SignBit && type != "u64")
            {
                value -= TwoToThe32;
            }

            return $"({cast})({Hlsl.Number(value)})";
        }

        return $"({cast})((uint64_t){_operands.Read(operand, High)} << 32 | {_operands.Read(operand, Low)})";
    }

    private void Compare(string op, bool writesExec, string condition, string type, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var operands = a.ToList();

        if (type is "f16" or "i16" or "u16")
        {
            type = WidenHalves(type, operands, modifiers);
        }

        if (writesExec)
        {
            // gfx10 v_cmpx writes EXEC only (the e32 form has no explicit destination).
            operands = [OperandParser.Parse("exec"), operands[^PairDwords], operands[^1]];
        }

        var (destination, first, second) = (operands[Destination], operands[Source0], operands[Source1]);
        var (x, y) = ReadPair(type, first, second);

        if (condition == "class")
        {
            _operands.WriteMask(destination, $"active && cmp_class({_operands.ReadFloat(first)}, {_operands.Read(second)})");

            return;
        }

        var test = Condition(condition, type == "f32", x, y) ?? throw new UnsupportedProgramException(op);
        _operands.WriteMask(destination, $"active && ({test})");
    }

    private (string X, string Y) ReadPair(string type, Operand first, Operand second)
    {
        if (type == "f32")
        {
            var fx = _operands.ReadFloat(first);

            return (fx, _operands.ReadFloat(second));
        }

        if (type == "i32")
        {
            var ix = _operands.ReadInt(first);

            return (ix, _operands.ReadInt(second));
        }

        var ux = _operands.Read(first);

        return (ux, _operands.Read(second));
    }

    /// <summary>Replaces the two sources with 32-bit temporaries of their selected halves; returns the 32-bit type.</summary>
    private string WidenHalves(string type, List<Operand> operands, InstructionModifiers modifiers)
    {
        var select = ArgumentList.Bits(modifiers.Value("op_sel"));
        select.AddRange([0, 0]);

        for (var source = 0; source < PairDwords; source++)
        {
            var position = operands.Count - PairDwords + source;
            var operand = operands[position];
            var value = _operands.Read(operand);

            if (select[source] != 0)
            {
                value = $"({value} >> 16)";
            }

            var widened = type switch
            {
                "f16" => $"asuint(f16tof32({value}))",
                "i16" => $"asuint(asint({value} << 16) >> 16)",
                _ => $"({value} & 0xffffu)",
            };
            operands[position] = Operand.FromExpression(_operands.SnapOne(widened)).WithModifiersOf(operand);
        }

        var wide = type switch
        {
            "f16" => "f32",
            "i16" => "i32",
            _ => "u32",
        };

        return wide;
    }

    private static string? Condition(string condition, bool isFloat, string x, string y)
    {
        // lg is the ordered not-equal (false on NaN); HLSL != is unordered.
        var orderedNotEqual = isFloat ? $"({x} < {y} || {x} > {y})" : $"{x} != {y}";
        var unorderedEqual = isFloat ? $"!({x} < {y} || {x} > {y})" : $"!({x} != {y})";

        return condition switch
        {
            "lt" => $"{x} < {y}",
            "eq" => $"{x} == {y}",
            "le" => $"{x} <= {y}",
            "gt" => $"{x} > {y}",
            "lg" => orderedNotEqual,
            "ne" => $"{x} != {y}",
            "ge" => $"{x} >= {y}",
            "neq" => $"!({x} == {y})",
            "nlt" => $"!({x} < {y})",
            "nle" => $"!({x} <= {y})",
            "ngt" => $"!({x} > {y})",
            "nge" => $"!({x} >= {y})",
            "nlg" => unorderedEqual,
            "o" => $"(!isnan({x}) && !isnan({y}))",
            "u" => $"(isnan({x}) || isnan({y}))",
            "f" => "false",
            "t" => "true",
            _ => null,
        };
    }

    [GeneratedRegex(@"^v_cmp(?<x>x?)_(?<cond>eq|ne|lt|le|gt|ge|lg)_(?<type>u64|i64)$")]
    private static partial Regex Compare64Pattern();

    [GeneratedRegex(@"^v_cmp(?<x>x?)_(?<cond>\w+?)_(?<type>f32|i32|u32|f16|i16|u16)$")]
    private static partial Regex ComparePattern();
}
