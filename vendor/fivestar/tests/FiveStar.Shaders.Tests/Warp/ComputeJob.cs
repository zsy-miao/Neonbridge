// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A compute dispatch: results come back only through writable buffers and textures. <see cref="FallbackTable"/> binds
/// the mesh fallback's u0..u2 / t0 (space 9) as a descriptor table starting at that heap slot.
/// </summary>
public sealed record ComputeJob(byte[] Shader, int GroupsX, int GroupsY, int GroupsZ, Bindings Bindings)
{
    public int? FallbackTable { get; init; }
}
