// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Shaders.Tests.Warp;

namespace FiveStar.Shaders.Tests.Cases;

/// <summary>
/// A pixel-shader case on an 8x8 target. Results are v20.. exported to mrt0.. (as uint4, bit-exact); a raw program
/// holds <c>{EXPORT}</c> where the final export goes and its own s_endpgm. <see cref="Expect"/> returns the expected
/// v20.. of a pixel, or null when the pixel must be discarded.
/// </summary>
public sealed record PixelCase(string Name, string Program, int Outputs, Func<PixelContext, Expected[]?> Expect)
{
    public const int Size = 8;

    public Bindings Bindings { get; init; } = new();

    public bool Raw { get; init; }

    /// <summary>No output signature: float SV_Target into R32G32B32A32_FLOAT.</summary>
    public bool FloatTargets { get; init; }

    /// <summary>Drawn with the half-coverage triangle (x + y &lt;= 6) instead of the fullscreen one.</summary>
    public bool HalfCoverage { get; init; }

    /// <summary>A case whose expected values are the same for every pixel.</summary>
    public static PixelCase Uniform(string name, string program, Expected[] expected, params uint[] user)
    {
        return new PixelCase(name, program, expected.Length, _ => expected) { Bindings = new Bindings { UserSgprs = user } };
    }

    public int Targets => (Outputs + 3) / 4;
}
