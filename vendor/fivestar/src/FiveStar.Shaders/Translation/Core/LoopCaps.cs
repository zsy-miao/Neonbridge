// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;

namespace FiveStar.Shaders.Translation.Core;

/// <summary>
/// Loop iterations one invocation may run, per stage (<see cref="LoopGuard"/>): compute, the geometry stages (vertex,
/// mesh, geometry) and pixel. <paramref name="StepsPerIteration"/>, when not 0, also bounds the block dispatch form
/// (which has no structured loops) at that many blocks per allowed iteration. Part of the translation options, so of
/// the program cache's identity. The group rounds of a mesh program (its barriers in the two-phase form, its steps in
/// the superstep form) are bounded at <see cref="RoundsPerIteration"/> per allowed iteration (<see cref="Rounds"/>):
/// a console loop around an s_barrier runs one round per iteration and no structured loop counts it.
/// </summary>
public sealed record LoopCaps(long Compute, long Geometry, long Pixel, long StepsPerIteration)
{
    /// <summary>
    /// Group rounds per allowed iteration (4,096 for mesh). A superstep round of TransformTerrainNormalTint e74ae427 cost
    /// 40 to 60 us on the RTX 4090 for DispatchMesh(94, 1, 1) (each thread runs to its next stop, then the group's
    /// barrier), so 16 per iteration (65,536) let a loop around a wave operation run for 3 to 4 s, past the 2 s GPU
    /// timeout: the DEVICE_HUNG of the mesh-shader ExecuteIndirects in boots w31 to w47. 4,096 rounds take about 0.25 s.
    /// </summary>
    public const long RoundsPerIteration = 1;

    /// <summary>
    /// Blocks a superstep thread may run per allowed iteration (<see cref="SuperstepSteps"/>, 8,192 for mesh). A light
    /// block cost about 0.6 us per thread on the RTX 4090, but MSNC_VS_LinearDepthSkin's layer loop (a sample and some
    /// thirty dependent loads a pass) ran its 2^16 blocks (16 per iteration) in 0.5 to 1.5 s per DispatchMesh in boot w49,
    /// so frames passed the game's 30 s hang watchdog; 8,192 take about 0.1 to 0.2 s. A program runs 100 to 250 blocks.
    /// </summary>
    public const long SuperstepBlocksPerIteration = 2;

    /// <summary>Blocks per allowed iteration in the dispatch form, both caps.</summary>
    private const long VictorySteps = 256;
    private const int Parts = 3;
    private const long DefaultCompute = 1 << 16;
    private const long DefaultGraphics = 1 << 12;
    private const long VictoryCompute = 4096;
    private const long VictoryGeometry = 256;
    private const long VictoryPixel = 128;

    /// <summary>
    /// The translator's own caps: 2^16 compute iterations, 2^12 elsewhere, 256 dispatch blocks per iteration (2^24 for
    /// compute, 2^20 elsewhere; 2^24 everywhere before, which let a 4-group mesh dispatch run for seconds).
    /// </summary>
    public static LoopCaps Default { get; } = new(DefaultCompute, DefaultGraphics, DefaultGraphics, VictorySteps);

    /// <summary>victory-gta6's caps (emit/lower.rs, count_iteration): compute 4,096, vertex and mesh 256, pixel 128.</summary>
    public static LoopCaps Victory { get; } = new(VictoryCompute, VictoryGeometry, VictoryPixel, VictorySteps);

    /// <summary>"victory", "default" or "compute,geometry,pixel"; null for anything else.</summary>
    public static LoopCaps? Parse(string text)
    {
        if (text.Equals("victory", StringComparison.OrdinalIgnoreCase))
        {
            return Victory;
        }

        if (text.Equals("default", StringComparison.OrdinalIgnoreCase))
        {
            return Default;
        }

        var parts = text.Split(',');

        if (parts.Length != Parts || !parts.All(part => long.TryParse(part, NumberStyles.Integer, CultureInfo.InvariantCulture, out var value) && value > 0))
        {
            return null;
        }

        var values = parts.Select(part => long.Parse(part, CultureInfo.InvariantCulture)).ToArray();
        var (compute, geometry, pixel) = (values[0], values[1], values[^1]);

        return new LoopCaps(compute, geometry, pixel, VictorySteps);
    }

    /// <summary>The iterations of <paramref name="stage"/>.</summary>
    public long Iterations(ShaderStage stage)
    {
        return stage switch
        {
            ShaderStage.Compute => Compute,
            ShaderStage.Pixel => Pixel,
            _ => Geometry,
        };
    }

    /// <summary>The group rounds a mesh program of <paramref name="stage"/> may run (barriers or superstep rounds).</summary>
    public long Rounds(ShaderStage stage)
    {
        return Math.Min(LoopGuard.DispatchSteps, Iterations(stage) * RoundsPerIteration);
    }

    /// <summary>
    /// The blocks a thread of a mesh program's superstep form may run, at <see cref="SuperstepBlocksPerIteration"/> per
    /// allowed iteration (2^16 for mesh). The dispatch form's 256 per iteration let a one-block console loop fed a garbage count
    /// run 2^20 times: MSNC_VS_LinearDepthSkin's loop over a constant-buffer count (dword 0x11c), each pass a sample and
    /// some thirty dependent loads, the likely cause of the device removals of boots w39 and w40.
    /// </summary>
    public long SuperstepSteps(ShaderStage stage)
    {
        return Math.Min(DispatchSteps(stage), Iterations(stage) * SuperstepBlocksPerIteration);
    }

    /// <summary>The dispatch form's blocks for <paramref name="stage"/>.</summary>
    public long DispatchSteps(ShaderStage stage)
    {
        if (StepsPerIteration == 0)
        {
            return LoopGuard.DispatchSteps;
        }

        return Math.Min(LoopGuard.DispatchSteps, Iterations(stage) * StepsPerIteration);
    }
}
