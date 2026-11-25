// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Translation.Core;

namespace FiveStar.Shaders.Translation.Emitters;

/// <summary>
/// One image sample/load/gather being translated: the base operation (without _o and _cl), the texture type, the
/// address values (after offsets were taken off and a16 pairs unpacked) and the names of the HLSL objects.
/// </summary>
internal sealed record ImageRequest
{
    public required string Operation { get; init; }

    public required string Dimension { get; init; }

    public required string TextureType { get; init; }

    public required int Components { get; init; }

    public required bool IsCube { get; init; }

    public required List<string> Coordinates { get; init; }

    /// <summary>The texel offset vector of an _o variant, or null.</summary>
    public string? Offsets { get; init; }

    public required bool HasBias { get; init; }

    public required bool HasCompare { get; init; }

    public required bool HasClamp { get; init; }

    /// <summary>Number of derivative values (sample_d): 2 per spatial dimension, 4 for cube face space.</summary>
    public required int Derivatives { get; init; }

    public required string Texture { get; init; }

    /// <summary>The SamplerState name, or null for compare and load operations.</summary>
    public string? Sampler { get; init; }

    /// <summary>The S# operand, or null when the instruction has none.</summary>
    public Operand? SamplerOperand { get; init; }

    /// <summary>The temporary number shared by this instruction's HLSL objects.</summary>
    public required int Number { get; init; }

    /// <summary>Spatial address components (without the array slice).</summary>
    public int SpatialComponents
    {
        get
        {
            if (ImageDimensions.IsArray(Dimension))
            {
                return Components - 1;
            }

            return Components;
        }
    }

    /// <summary>Float coordinates; on a cube (s, t, face) from the v_cube* math becomes a direction.</summary>
    public string FloatVector(IReadOnlyList<string> values)
    {
        const int CubeAddress = 3;

        if (IsCube && values.Count == CubeAddress)
        {
            return $"cube_dir(asfloat({values[0]}), asfloat({values[1]}), asfloat({values[^1]}))";
        }

        if (values.Count > 1)
        {
            return $"float{values.Count}({string.Join(", ", values.Select(value => $"asfloat({value})"))})";
        }

        return $"asfloat({values[0]})";
    }
}
