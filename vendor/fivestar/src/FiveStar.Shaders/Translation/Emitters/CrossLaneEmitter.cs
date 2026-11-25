// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Text.RegularExpressions;
using FiveStar.Shaders.Translation.Core;
using static FiveStar.Shaders.Translation.Emitters.Operands;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// Lane-indexed data movement: v_readlane/v_writelane, v_permlane(x)16, v_mbcnt (lane-mask prefix counts) and
/// ds_swizzle. Lane numbers are AMD (program) lane numbers, read through the vw_* operations; each read records how
/// far it reaches (<see cref="TranslationState.Reach"/>).
/// </summary>
internal sealed partial class CrossLaneEmitter
{
    private const string LaneIndex = WaveOps.Lane;

    // NGG code computes thread index = wave * 64 + mbcnt(lane): mesh shaders use a virtual wave64 lane.
    private const string MeshLane = "(in_gi & 63u)";

    // GSVS records are addressed per lane (mbcnt * item size); each HLSL GS invocation owns a private LDS.
    private const string GeometryLane = "0u";

    // ds_swizzle BITMASK_PERM pattern: five characters for lane bits 4..0.
    private const int BitmaskPermBits = 5;

    // Lane groups the fixed patterns stay inside.
    private const int QuadLanes = 4;
    private const int RowLanes = 16;
    private const int CrossRowLanes = 32;

    // SWAP(n) exchanges the two n-lane halves of each 2n-lane group.
    private const int SwapGroups = 2;

    private readonly TranslationState _state;
    private readonly OperandAccess _operands;

    public CrossLaneEmitter(TranslationState state, OperandAccess operands)
    {
        _state = state;
        _operands = operands;
    }

    /// <summary>Translates the instruction when it belongs to this family; false otherwise.</summary>
    public bool TryTranslate(string op, IReadOnlyList<Operand> a)
    {
        switch (op)
        {
            case "v_readlane_b32":
                ReadLane(a);
                break;
            case "v_writelane_b32":
                WriteLane(a);
                break;
            case "v_permlane16_b32":
            case "v_permlanex16_b32":
                Permute16(op, a);
                break;
            case "v_mbcnt_lo_u32_b32":
                MaskCount(a, high: false);
                break;
            case "v_mbcnt_hi_u32_b32":
                MaskCount(a, high: true);
                break;
            default:
                return false;
        }

        return true;
    }

    /// <summary>v_readlane: the lane number comes from an inline constant or an SGPR.</summary>
    private void ReadLane(IReadOnlyList<Operand> a)
    {
        var lane = a[Source1];

        if (lane.IsImmediate)
        {
            _state.ReachLane(lane.Value);
        }
        else
        {
            _state.ReachLane(lane.Key);
        }

        _operands.Write(a[Destination], $"{WaveOps.ReadLane}({U(a, Source0)}, {U(a, Source1)})");
    }

    /// <summary>v_writelane: writes one lane regardless of EXEC.</summary>
    private void WriteLane(IReadOnlyList<Operand> a)
    {
        var key = a[Destination].At(0);
        _state.Touch(key);
        var lane = U(a, Source1);
        _state.Emit($"  if ({LaneIndex} == (({lane}) & {_state.WaveSize - 1}u)) {key.Name} = {U(a, Source0)};");
    }

    private void Permute16(string op, IReadOnlyList<Operand> a)
    {
        var values = _operands.Snap(U(a, Source0), U(a, Source1), U(a, Source2));
        var (x, selectLow, selectHigh) = (values[0], values[1], values[^1]);
        var lane = $"({LaneIndex} & 15u)";
        var nibble = $"((({lane} < 8u) ? ({selectLow} >> ({lane} * 4u)) : ({selectHigh} >> (({lane} - 8u) * 4u))) & 15u)";
        var row = $"({LaneIndex} & ~15u)";
        var reach = RowLanes;

        if (op == "v_permlanex16_b32")
        {
            row = $"(({LaneIndex} & ~31u) | (({LaneIndex} & 16u) ^ 16u))";
            reach = CrossRowLanes;
        }

        _state.Reach(reach);
        _operands.Write(a[Destination], $"{WaveOps.ReadLane}({x}, {row} | {nibble})");
    }

    /// <summary>v_mbcnt_lo/hi: count of mask bits below this lane in the low/high 32 lanes, plus src1.</summary>
    private void MaskCount(IReadOnlyList<Operand> a, bool high)
    {
        var lane = LaneIndex;

        if (_state.Stage == ShaderStage.Mesh)
        {
            lane = MeshLane;
        }
        else if (_state.Stage == ShaderStage.Geometry)
        {
            lane = GeometryLane;
        }

        var mask = U(a, Source0);
        var below = high
            ? $"({lane} < 32 ? 0u : ((1u << ({lane} - 32)) - 1u))"
            : $"({lane} < 32 ? ((1u << {lane}) - 1u) : 0xffffffffu)";
        _operands.Write(a[Destination], $"countbits({mask} & {below}) + {U(a, Source1)}");
    }

    /// <summary>ds_swizzle_b32: QUAD_PERM, SWAP, REVERSE, BROADCAST and BITMASK_PERM lane patterns.</summary>
    public void Swizzle(string operands)
    {
        var match = SwizzlePattern().Match(operands);

        if (!match.Success)
        {
            throw new UnsupportedProgramException($"ds_swizzle {operands}");
        }

        var destination = OperandParser.Parse(match.Groups["dst"].Value);
        var source = OperandParser.Parse(match.Groups["src"].Value);
        var x = _operands.Read(source);
        var mode = match.Groups["mode"].Value;
        var arguments = match.Groups["args"].Value;

        if (mode == "QUAD_PERM")
        {
            var lanes = arguments.Split(',').Select(part => int.Parse(part, System.Globalization.CultureInfo.InvariantCulture)).ToList();
            _operands.Write(destination, $"quad_perm({x}, {string.Join(", ", lanes)})");
            _state.Reach(QuadLanes);

            return;
        }

        _operands.Write(destination, $"{WaveOps.ReadLane}({x}, {SwizzleSource(mode, arguments)})");
    }

    private string SwizzleSource(string mode, string arguments)
    {
        switch (mode)
        {
            case "SWAP":
                _state.Reach(SwapGroups * Number(arguments));

                return $"({LaneIndex} ^ {Number(arguments)}u)";
            case "REVERSE":
                _state.Reach(Number(arguments));

                return $"({LaneIndex} ^ {Number(arguments) - 1}u)";
            case "BROADCAST":
                _state.Reach(Number(arguments.Split(',')[0]));

                return Broadcast(arguments);
            case "BITMASK_PERM":
                _state.Reach(CrossRowLanes);

                return BitmaskPermutation(arguments.Trim('"'));
            default:
                throw new UnsupportedProgramException($"ds_swizzle {mode}");
        }
    }

    /// <summary>BROADCAST(size, lane): every group of size lanes reads its lane number lane.</summary>
    private static string Broadcast(string arguments)
    {
        var parts = arguments.Split(',');

        return $"(({LaneIndex} & ~{Number(parts[0]) - 1}u) | {Number(parts[1])}u)";
    }

    /// <summary>BITMASK_PERM: per lane bit (bit 4 first) 0, 1, p (keep) or i (invert).</summary>
    private static string BitmaskPermutation(string pattern)
    {
        var andMask = 0;
        var orMask = 0;
        var xorMask = 0;

        for (var position = 0; position < Math.Min(pattern.Length, BitmaskPermBits); position++)
        {
            var bit = 1 << (BitmaskPermBits - 1 - position);

            switch (pattern[position])
            {
                case 'p':
                    andMask |= bit;
                    break;
                case 'i':
                    andMask |= bit;
                    xorMask |= bit;
                    break;
                case '1':
                    orMask |= bit;
                    break;
                default:
                    break;
            }
        }

        return $"(({LaneIndex} & ~31u) | ((({LaneIndex} & {andMask}u) | {orMask}u) ^ {xorMask}u))";
    }

    private static int Number(string text)
    {
        return int.Parse(text.Trim(), System.Globalization.CultureInfo.InvariantCulture);
    }

    private string U(IReadOnlyList<Operand> a, int position)
    {
        return _operands.Read(a[position]);
    }

    [GeneratedRegex(@"^(?<dst>v\d+),\s*(?<src>v\d+)\s+offset:(swizzle\((?<mode>\w+)(?:,(?<args>[^)]*))?\)|\d+)")]
    private static partial Regex SwizzlePattern();
}
