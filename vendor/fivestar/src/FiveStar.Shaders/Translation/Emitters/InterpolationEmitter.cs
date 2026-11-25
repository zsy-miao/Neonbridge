// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Pixel-shader attribute interpolation. A v_interp_p1/p2 pair becomes one read of the interpolated attribute (attrN);
/// v_interp_mov p0 a flat read, p10/p20 a per-vertex read (GetAttributeAtVertex). Packed f16 channels are read from
/// unpacked low/high copies the vertex stage exports.
/// </summary>
internal sealed partial class InterpolationEmitter
{
    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public InterpolationEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    public void Translate(string op, string operands)
    {
        var match = InterpolationPattern().Match(operands.Replace("_e32", string.Empty, StringComparison.Ordinal));

        if (!match.Success)
        {
            throw new UnsupportedProgramException($"interp {operands}");
        }

        var destination = OperandParser.Parse(match.Groups["dst"].Value);
        var attribute = int.Parse(match.Groups["attr"].Value, CultureInfo.InvariantCulture);
        var channel = match.Groups["chan"].Value;
        var high = match.Groups["high"].Success;
        var operation = op.Replace("_e32", string.Empty, StringComparison.Ordinal).Replace("_e64", string.Empty, StringComparison.Ordinal);

        if (operation.StartsWith("v_interp_p1", StringComparison.Ordinal))
        {
            // The first half of a p1/p2 pair has no visible result of its own (folded into p2).
            _state.Touch(destination.At(0));

            return;
        }

        switch (operation)
        {
            case "v_interp_mov_f32":
                Move(destination, attribute, channel, match.Groups["src"].Value);
                break;
            case "v_interp_p2_f32":
                _state.AddAttributeMode(attribute, "linear");
                _operands.Write(destination, $"asuint(attr{attribute}.{channel})");
                break;
            case "v_interp_p2_f16":
            case "v_interp_p2_legacy_f16":
                PackedHalf(destination, attribute, channel, high);
                break;
            default:
                throw new UnsupportedProgramException(operation);
        }
    }

    private void Move(Operand destination, int attribute, string channel, string source)
    {
        if (source == "p0")
        {
            // Vertex 0 is the D3D provoking vertex of nointerpolation attributes.
            _state.AddAttributeMode(attribute, "flat");
            _operands.Write(destination, $"asuint(attr{attribute}.{channel})");

            return;
        }

        var vertex = source switch
        {
            "p10" => "1",
            "p20" => "2",
            _ => throw new UnsupportedProgramException($"v_interp_mov {source}"),
        };
        _state.AddAttributeMode(attribute, "vtx" + vertex);
        _operands.Write(destination, $"asuint(attr{attribute}v{vertex}.{channel})");
    }

    private void PackedHalf(Operand destination, int attribute, string channel, bool high)
    {
        // A packed-f16 attribute channel: the selected half is interpolated on its own.
        var half = high ? "hi" : "lo";
        _state.AddAttributeMode(attribute, "f16" + half);
        _operands.WriteHalf(destination, $"f32tof16(attr{half}{attribute}.{channel})", high: false);
    }

    [GeneratedRegex(@"^(?<dst>v\d+),\s*(?<src>\S+),\s*attr(?<attr>\d+)\.(?<chan>[xyzw])(?<high>\s+high)?")]
    private static partial Regex InterpolationPattern();
}
