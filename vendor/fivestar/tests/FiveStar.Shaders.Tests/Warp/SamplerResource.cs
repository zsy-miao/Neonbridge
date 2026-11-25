// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A clamp-addressed sampler at a <c>SamplerDescriptorHeap</c> slot, point or linear filtered; a comparison sampler
/// (less-or-equal) for <c>SamplerComparisonState</c>; or exactly <see cref="Description"/> when set.
/// </summary>
public sealed record SamplerResource(int HeapIndex, bool Linear)
{
    public bool Comparison { get; init; }

    public Vortice.Direct3D12.SamplerDescription? Description { get; init; }
}
