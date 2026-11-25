// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation.Waves;

/// <summary>
/// What a variable or expression holds on a console lane without a pixel or vertex (see <see cref="MissingLaneAnalysis"/>):
/// unknown, a known value of an HLSL type, or a lane number found in a lane mask.
/// </summary>
internal readonly record struct MissingValue(MissingKind Kind, uint Bits, MissingType Type)
{
    public static MissingValue Unknown => new(MissingKind.Unknown, 0, MissingType.UInt);

    /// <summary>A lane number found in a lane mask (s_ff1): a lane with a pixel, as for <c>TranslationState.MaskLanes</c>.</summary>
    public static MissingValue MaskLane => new(MissingKind.MaskLane, 0, MissingType.UInt);

    public static MissingValue Zero => Of(0, MissingType.UInt);

    public bool IsKnown => Kind == MissingKind.Known;

    public static MissingValue Of(uint bits, MissingType type)
    {
        return new MissingValue(MissingKind.Known, bits, type);
    }

    public static MissingValue Bool(bool value)
    {
        return Of(Convert.ToUInt32(value), MissingType.Bool);
    }

    /// <summary>The value where two paths or the arms of an unknown condition agree, else unknown.</summary>
    public static MissingValue Merge(MissingValue first, MissingValue second)
    {
        if (first == second)
        {
            return first;
        }

        return Unknown;
    }
}
