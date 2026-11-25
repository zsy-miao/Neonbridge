// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// What a run left behind: each render target as raw 32-bit words (4 per pixel, row-major), every writable buffer and
/// texture by heap slot, the pipeline statistics and, for the mesh fallback, the DrawIndexed arguments + vertex counter.
/// </summary>
public sealed record WarpResult(
    IReadOnlyList<uint[]> Targets,
    int Width,
    IReadOnlyDictionary<int, uint[]> Buffers,
    PipelineStatistics? Statistics,
    uint[]? IndirectArguments)
{
    public const int WordsPerPixel = 4;

    /// <summary>Every writable texture by heap slot, texels as in <see cref="TextureResource.Words"/>.</summary>
    public IReadOnlyDictionary<int, uint[]> Textures { get; init; } = new Dictionary<int, uint[]>();

    /// <summary>The four words of one pixel of a render target.</summary>
    public uint[] Pixel(int target, int x, int y)
    {
        var start = ((y * Width) + x) * WordsPerPixel;

        return Targets[target].AsSpan(start, WordsPerPixel).ToArray();
    }
}
