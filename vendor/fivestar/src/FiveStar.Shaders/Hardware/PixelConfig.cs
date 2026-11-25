// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Hardware;

/// <summary>
/// Pixel-shader fields of an FSSM header. <see cref="PrimMaskSgpr"/> is the SGPR after the user data that holds
/// PRIM_MASK; <see cref="PopsWaveIdSgpr"/> the next one when primitive-ordered pixel shading is on.
/// </summary>
public sealed record PixelConfig
{
    public required uint Rsrc2 { get; init; }

    public required int InterpolantCount { get; init; }

    public required uint InputEnable { get; init; }

    public required IReadOnlyList<PixelInput> Inputs { get; init; }

    public required int InputVgprCount { get; init; }

    public required int PrimMaskSgpr { get; init; }

    public required uint DbShaderControl { get; init; }

    public required bool Pops { get; init; }

    public int? PopsWaveIdSgpr { get; init; }

    public required bool ZExport { get; init; }

    public required bool KillEnable { get; init; }

    public required uint CbShaderMask { get; init; }

    public required int MrtCount { get; init; }

    /// <summary>Non-empty (a, b) word pairs of the interpolant table at 0x138.</summary>
    public required IReadOnlyList<(uint First, uint Second)> InterpolantTable { get; init; }
}
