// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Graphics.Rendering;

namespace FiveStar.Cli.Graphics;

/// <summary>A triangle list of <see cref="UnlitScene"/>-layout vertices drawn with one texture.</summary>
public sealed record DrawBatch(TextureData Texture, byte[] Vertices)
{
    public int VertexCount => Vertices.Length / UnlitScene.VertexStride;
}
