// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// Without <c>--raytracing</c> no acceleration structure is built (XboxBvhBuilder builds nothing), so programs made on
/// demand define FIVESTAR_BVH_MISS: the image_bvh_intersect_ray emulation (rdna_bvh.hlsli) misses every node instead of
/// reading the title memory a BVH base points at. Traversal over that junk (about 70 inline ray-tracing compute
/// programs: CS_RaytracingResolve, the ray-traced shadows, bvh_*) followed garbage child pointers until the driver
/// reset the context: the world-load DEVICE_REMOVED of boot w63, gone when those programs were skipped.
/// </summary>
internal static class BvhMiss
{
    /// <summary>The define of programs whose BVH nodes all miss.</summary>
    public const string Header = "#define FIVESTAR_BVH_MISS 1\n";

    /// <summary>Whether nodes miss (default on; off with <c>--raytracing</c>).</summary>
    public static bool Enabled { get; set; } = true;
}
