// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// The bindless resource layout the translator expects: user SGPRs as root constants at b0 (<c>g_user4</c>), resources
/// and samplers at heap slots (descriptor dword0 is the heap index), the GPU-memory window at heap slot 0.
/// </summary>
public sealed record Bindings
{
    public IReadOnlyList<uint> UserSgprs { get; init; } = [];

    public IReadOnlyList<BufferResource> Buffers { get; init; } = [];

    public IReadOnlyList<TextureResource> Textures { get; init; } = [];

    public IReadOnlyList<SamplerResource> Samplers { get; init; } = [];

    /// <summary>Root constants bound at b0; at most 64 DWORDs fit a root signature, minus what else it holds.</summary>
    public int RootConstantCount { get; init; } = 60;
}
