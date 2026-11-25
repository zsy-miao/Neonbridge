// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>A draw through VS + PS, or VS + GS + PS, with pipeline statistics.</summary>
public sealed record GraphicsJob(byte[] VertexShader, byte[] PixelShader, Framebuffer Framebuffer, Bindings Bindings)
{
    public byte[]? GeometryShader { get; init; }

    public IReadOnlyList<VertexElement> InputLayout { get; init; } = [];

    public IReadOnlyList<VertexBufferData> VertexBuffers { get; init; } = [];

    /// <summary>Point list instead of triangle list.</summary>
    public bool Points { get; init; }

    public int VertexCount { get; init; } = 3;

    public int InstanceCount { get; init; } = 1;
}
