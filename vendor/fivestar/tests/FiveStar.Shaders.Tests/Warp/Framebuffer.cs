// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// Render targets of a draw: <see cref="Count"/> R32G32B32A32 targets (UINT, or FLOAT) of Width x Height, cleared to
/// <see cref="ClearUint"/> / <see cref="ClearFloatBits"/> so unwritten (discarded) pixels are recognisable.
/// </summary>
public sealed record Framebuffer(int Count, int Width, int Height, int ViewportWidth, int ViewportHeight, bool FloatTargets)
{
    /// <summary>UINT clear value; ClearRenderTargetView converts from float, so it stays below 2^24.</summary>
    public const uint ClearUint = 0x00BADBAD;

    public const uint ClearFloatBits = 0x7FBADBAD;

    /// <summary>Targets whose viewport is the target size (geometry given in pixels of the target).</summary>
    public static Framebuffer Exact(int count, int width, int height)
    {
        return new Framebuffer(count, width, height, width, height, FloatTargets: false);
    }

    /// <summary>
    /// Targets for the built-in triangle: the viewport is twice the target, so the triangle covers the target without
    /// clipping (WARP splits a clipped triangle, which creates helper lanes along the split edge).
    /// </summary>
    public static Framebuffer Fullscreen(int count, int width, int height, bool isFloat)
    {
        return new Framebuffer(count, width, height, 2 * width, 2 * height, isFloat);
    }
}
