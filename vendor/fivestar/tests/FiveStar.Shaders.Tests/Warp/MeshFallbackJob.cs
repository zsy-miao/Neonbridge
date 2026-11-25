// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A mesh dispatch through the mesh-to-compute fallback (docs/shader-translation.md, "Mesh fallback contract"): the
/// compute shader fills the vertex records (u0), indices (u1) and DrawIndexed arguments + vertex counter (u2) in
/// register space 9, then ExecuteIndirect draws them with the vertex-pulling VS (t0) and the pixel shader.
/// <see cref="StaleIndices"/> pre-fill the index buffer (data an earlier frame left there).
/// </summary>
public sealed record MeshFallbackJob(
    byte[] ComputeShader,
    byte[] VertexShader,
    byte[] PixelShader,
    Framebuffer Framebuffer,
    Bindings Bindings,
    int Groups,
    int VertexBytes,
    int IndexBytes)
{
    public IReadOnlyList<uint> StaleIndices { get; init; } = [];

    /// <summary>Bind u0..u2 / t0 as root descriptors instead of a descriptor table.</summary>
    public bool RootDescriptors { get; init; }
}
