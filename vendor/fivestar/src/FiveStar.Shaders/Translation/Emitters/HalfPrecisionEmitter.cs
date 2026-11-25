// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// 16-bit float math: scalar f16 operations on one half (op_sel picks the halves; the other destination half is kept),
/// packed f16 and i16 pairs (v_pk_*), mixed-precision FMA (v_fma_mix*) and packing conversions.
/// </summary>
internal sealed partial class HalfPrecisionEmitter
{
    // op_sel bit of the destination for VOP3 f16 operations (after the three source bits).
    private const int DestinationSelect = 3;
    private const int ThreeSources = 3;

    private readonly OperandAccess _operands;
    private readonly PackedMathEmitter _packed;

    public HalfPrecisionEmitter(OperandAccess operands, PackedMathEmitter packed)
    {
        _operands = operands;
        _packed = packed;
    }

    /// <summary>Translates the instruction when it belongs to this family; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        switch (op)
        {
            case "v_pack_b32_f16":
                Pack(a, modifiers);
                break;
            case "v_cvt_pk_u8_f32":
                ConvertPackByte(a);
                break;
            case "v_fma_mix_f32":
            case "v_fma_mixlo_f16":
            case "v_fma_mixhi_f16":
            case "v_mad_mix_f32":
            case "v_mad_mixlo_f16":
            case "v_mad_mixhi_f16":
                MixedMultiplyAdd(op, a, modifiers);
                break;
            default:
                return TryHalfOrPacked(op, a, modifiers);
        }

        return true;
    }

    private bool TryHalfOrPacked(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var scalar = HalfPattern().Match(op);

        if (scalar.Success && !op.StartsWith("v_cmp", StringComparison.Ordinal) && !op.StartsWith("v_cvt", StringComparison.Ordinal)
            && !op.StartsWith("v_pk_", StringComparison.Ordinal))
        {
            Scalar(op, scalar.Groups[1].Value, a, modifiers);

            return true;
        }

        return _packed.TryTranslate(op, a, modifiers);
    }

    private void Pack(IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var select = ArgumentList.Bits(modifiers.Value("op_sel"));
        var lowHigh = select.Count > 0 && select[0] == 1;
        var highHigh = select.Count > 1 && select[1] == 1;
        var low = lowHigh ? $"({Read(a, Source0)} >> 16)" : Read(a, Source0);
        var high = highHigh ? $"({Read(a, Source1)} >> 16)" : Read(a, Source1);

        var first = a[Source0];
        var second = a[Source1];

        if (first.Negate || first.Absolute || second.Negate || second.Absolute)
        {
            low = $"f32tof16({_operands.ReadHalf(first, lowHigh)})";
            high = $"f32tof16({_operands.ReadHalf(second, highHigh)})";
        }

        _operands.Write(a[Destination], $"(({low}) & 0xffffu) | (({high}) << 16)");
    }

    private void ConvertPackByte(IReadOnlyList<Operand> a)
    {
        // v_cvt_pk_u8_f32 D, S0, S1 (byte index), S2 (previous value): one byte from a rounded, clamped float.
        var previous = Read(a, Source2);
        var shift = Read(a, Source1);
        var value = _operands.ReadFloat(a[Source0]);
        var write = $"({previous} & ~(0xffu << (({shift} & 3) * 8))) | ((uint)clamp(round({value}), 0.0, 255.0) << (({Read(a, Source1)} & 3) * 8))";
        _operands.Write(a[Destination], write);
    }

    private void MixedMultiplyAdd(string op, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var select = Padded(modifiers.Value("op_sel"), 0);
        var selectHigh = Padded(modifiers.Value("op_sel_hi"), 0);
        var sources = new string[ThreeSources];

        for (var source = 0; source < ThreeSources; source++)
        {
            var operand = a[source + 1];
            var isHalf = selectHigh[source] == 1;
            sources[source] = isHalf ? _operands.ReadHalf(operand, select[source] == 1) : _operands.ReadFloat(operand);
        }

        var value = ValuEmitter.OutputModifiers($"mad({string.Join(", ", sources)})", modifiers);

        if (op.EndsWith("_f32", StringComparison.Ordinal))
        {
            _operands.Write(a[Destination], $"asuint({value})");

            return;
        }

        _operands.WriteHalf(a[Destination], $"f32tof16({value})", op.EndsWith("hi_f16", StringComparison.Ordinal));
    }

    private void Scalar(string op, string kind, IReadOnlyList<Operand> a, InstructionModifiers modifiers)
    {
        var select = Padded(modifiers.Value("op_sel"), 0);
        var value = ScalarValue(kind, a, select) ?? throw new UnsupportedProgramException(op);
        var result = $"f32tof16({ValuEmitter.OutputModifiers(value, modifiers)})";
        _operands.WriteHalf(a[Destination], result, select[DestinationSelect] == 1);
    }

    private string? ScalarValue(string kind, IReadOnlyList<Operand> a, List<int> select)
    {
        switch (kind)
        {
            case "add":
            case "sub":
            case "subrev":
            case "mul":
            case "max":
            case "min":
            case "ldexp":
                return Binary(kind, a, select);
            case "fma":
            case "mad":
                return $"mad({Half(a, Source0, select)}, {Half(a, Source1, select)}, {Half(a, Source2, select)})";
            case "fmac":
            case "mac":
                return $"mad({_operands.ReadHalf(a[Source0], false)}, {_operands.ReadHalf(a[Source1], false)}, {_operands.ReadHalf(a[Destination], false)})";
            case "max3":
            case "min3":
            case "med3":
                return Ternary(kind, a, select);
            default:
                return Unary(kind, a, select);
        }
    }

    private string Binary(string kind, IReadOnlyList<Operand> a, List<int> select)
    {
        var x = Half(a, Source0, select);
        var y = kind == "ldexp" ? $"(float)asint({Read(a, Source1)})" : Half(a, Source1, select);

        return kind switch
        {
            "add" => $"{x} + {y}",
            "sub" => $"{x} - {y}",
            "subrev" => $"{y} - {x}",
            "mul" => $"{x} * {y}",
            "ldexp" => $"ldexp({x}, {y})",
            _ => $"{kind}({x}, {y})",
        };
    }

    private string Ternary(string kind, IReadOnlyList<Operand> a, List<int> select)
    {
        var x = Half(a, Source0, select);
        var y = Half(a, Source1, select);
        var z = Half(a, Source2, select);

        return kind switch
        {
            "max3" => $"max(max({x}, {y}), {z})",
            "min3" => $"min(min({x}, {y}), {z})",
            _ => $"med3({x}, {y}, {z})",
        };
    }

    private string? Unary(string kind, IReadOnlyList<Operand> a, List<int> select)
    {
        string? template = kind switch
        {
            "rcp" => "rcp({0})",
            "rsq" => "rsqrt({0})",
            "sqrt" => "sqrt({0})",
            "exp" => "exp2({0})",
            "log" => "log2({0})",
            "fract" => "frac({0})",
            "floor" => "floor({0})",
            "ceil" => "ceil({0})",
            "trunc" => "trunc({0})",
            "rndne" => "round({0})",
            "sin" => "sin({0} * 6.2831853)",
            "cos" => "cos({0} * 6.2831853)",
            _ => null,
        };

        if (template is null)
        {
            return null;
        }

        return string.Format(System.Globalization.CultureInfo.InvariantCulture, template, Half(a, Source0, select));
    }

    /// <summary>The f16 half of source operand <paramref name="position"/> chosen by its op_sel bit.</summary>
    private string Half(IReadOnlyList<Operand> a, int position, List<int> select)
    {
        return _operands.ReadHalf(a[position], select[position - Source0] == 1);
    }

    private string Read(IReadOnlyList<Operand> a, int position)
    {
        return _operands.Read(a[position]);
    }

    /// <summary>A modifier bit list padded with four fill values.</summary>
    public static List<int> Padded(string? bits, int fill)
    {
        const int Padding = 4;

        var list = ArgumentList.Bits(bits);
        list.AddRange(Enumerable.Repeat(fill, Padding));

        return list;
    }

    [GeneratedRegex(@"^v_(\w+)_f16$")]
    private static partial Regex HalfPattern();
}
