// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// Parses operand text as llvm-objdump prints it: registers (<c>v3</c>, <c>s[4:7]</c>, <c>-|v1|</c>), aliases (<c>vcc</c>,
/// <c>exec_lo</c>, <c>m0</c>), inline float constants, integer literals and the <c>neg()</c>/<c>abs()</c> literal modifiers.
/// </summary>
public static partial class OperandParser
{
    private const string NegatedAbsolutePrefix = "-|";
    private const string AbsoluteBar = "|";

    // src_pops_exiting_wave_id: the POPS ordering wait spins until older overlapping waves exit. On PC the ordering
    // comes from rasterizer-ordered views, so the operand reads as "no overlapping wave" and the wait passes at once.
    private const string PopsExitingWaveId = "src_pops_exiting_wave_id";
    private const uint NoOverlappingWave = 0x7fffffff;

    // Inline float constants of the gfx10 operand encoding (RDNA ISA, "Inline constants") as IEEE-754 bit patterns.
    private static readonly IReadOnlyDictionary<string, uint> FloatConstants = new Dictionary<string, uint>
    {
        ["0.5"] = 0x3F000000,
        ["-0.5"] = 0xBF000000,
        ["1.0"] = 0x3F800000,
        ["-1.0"] = 0xBF800000,
        ["2.0"] = 0x40000000,
        ["-2.0"] = 0xC0000000,
        ["4.0"] = 0x40800000,
        ["-4.0"] = 0xC0800000,
        ["0.15915494"] = 0x3E22F983,
    };

    // The same constants as an f16 operation reads them (in the low half of the operand).
    private static readonly IReadOnlyDictionary<uint, uint> HalfConstants = new Dictionary<uint, uint>
    {
        [0x3F000000] = 0x3800,
        [0xBF000000] = 0xB800,
        [0x3F800000] = 0x3C00,
        [0xBF800000] = 0xBC00,
        [0x40000000] = 0x4000,
        [0xC0000000] = 0xC000,
        [0x40800000] = 0x4400,
        [0xC0800000] = 0xC400,
        [0x3E22F983] = 0x3118,
    };

    /// <summary>
    /// The bits an f16 operation reads for an inline float constant (1.0 is 0x3C00, not the low half of 0x3F800000),
    /// or null for any other operand.
    /// </summary>
    public static uint? HalfConstant(Operand operand)
    {
        if (!operand.IsImmediate || !operand.Text.Contains('.', StringComparison.Ordinal) || !HalfConstants.TryGetValue(operand.Value, out var bits))
        {
            return null;
        }

        return bits;
    }

    /// <summary>Parses one operand; throws <see cref="UnsupportedProgramException"/> for anything else.</summary>
    public static Operand Parse(string text)
    {
        if (text is "null" or "off")
        {
            return Operand.Immediate(0, text);
        }

        var alias = SpecialRegisters.FromAlias(text);

        if (alias is not null)
        {
            var (index, pair) = alias.Value;
            var high = pair ? index + 1 : index;

            return new Operand { Kind = OperandKind.Scalar, Text = text, Low = index, High = high };
        }

        var register = RegisterPattern().Match(text);

        if (register.Success)
        {
            return FromRegister(text, register);
        }

        return ParseModifiedOrConstant(text);
    }

    /// <summary>A plain register operand (<c>v3</c>, <c>s[4:7]</c>, with optional modifiers), or null for anything else.</summary>
    public static Operand? TryParseRegister(string text)
    {
        var register = RegisterPattern().Match(text);

        if (!register.Success)
        {
            return null;
        }

        return FromRegister(text, register);
    }

    /// <summary>The SGPR with this number as an operand.</summary>
    public static Operand Sgpr(int index)
    {
        return Parse("s" + index.ToString(CultureInfo.InvariantCulture));
    }

    private static Operand ParseModifiedOrConstant(string text)
    {
        if (text.StartsWith(NegatedAbsolutePrefix, StringComparison.Ordinal) && text.EndsWith(AbsoluteBar, StringComparison.Ordinal))
        {
            var inner = text[NegatedAbsolutePrefix.Length..^AbsoluteBar.Length];

            return Inner(text, inner, negate: true, absolute: true);
        }

        var modifier = LiteralModifierPattern().Match(text);

        if (modifier.Success)
        {
            var negate = modifier.Groups[1].Value == "neg";

            return Inner(text, modifier.Groups["inner"].Value, negate, !negate);
        }

        if (FloatConstants.TryGetValue(text, out var bits))
        {
            return Operand.Immediate(bits, text);
        }

        if (text == PopsExitingWaveId)
        {
            return Operand.Immediate(NoOverlappingWave, text);
        }

        if (IntegerPattern().IsMatch(text))
        {
            return Operand.Immediate(ParseInteger(text), text);
        }

        throw new UnsupportedProgramException($"operand {text}");
    }

    private static Operand FromRegister(string text, Match match)
    {
        var kind = match.Groups["kind"].Value == "s" ? OperandKind.Scalar : OperandKind.Vector;
        var single = match.Groups["single"];
        int low;
        int high;

        if (single.Success)
        {
            low = int.Parse(single.Value, CultureInfo.InvariantCulture);
            high = low;
        }
        else
        {
            low = int.Parse(match.Groups["low"].Value, CultureInfo.InvariantCulture);
            high = int.Parse(match.Groups["high"].Value, CultureInfo.InvariantCulture);
        }

        return new Operand
        {
            Kind = kind,
            Text = text,
            Low = low,
            High = high,
            Negate = match.Groups["neg"].Success,
            Absolute = match.Groups["abs"].Success,
        };
    }

    /// <summary>The inner operand's kind, registers and value with the outer operand's modifiers.</summary>
    private static Operand Inner(string text, string inner, bool negate, bool absolute)
    {
        var parsed = Parse(inner);

        return parsed with { Text = text, Negate = negate, Absolute = absolute };
    }

    private static uint ParseInteger(string text)
    {
        const string HexPrefix = "0x";

        var negative = text.StartsWith('-');
        var digits = negative ? text[1..] : text;
        long magnitude;

        if (digits.StartsWith(HexPrefix, StringComparison.Ordinal))
        {
            magnitude = long.Parse(digits[HexPrefix.Length..], NumberStyles.HexNumber, CultureInfo.InvariantCulture);
        }
        else
        {
            magnitude = long.Parse(digits, CultureInfo.InvariantCulture);
        }

        var value = negative ? -magnitude : magnitude;

        return unchecked((uint)value);
    }

    [GeneratedRegex(@"^(?<neg>-)?(?<abs>\|)?(?<kind>[sv])(?:(?<single>\d+)|\[(?<low>\d+):(?<high>\d+)\])(\|)?$")]
    private static partial Regex RegisterPattern();

    [GeneratedRegex(@"^(neg|abs)\((?<inner>.+)\)$")]
    private static partial Regex LiteralModifierPattern();

    [GeneratedRegex(@"^-?(0x[0-9a-fA-F]+|\d+)$")]
    private static partial Regex IntegerPattern();
}
