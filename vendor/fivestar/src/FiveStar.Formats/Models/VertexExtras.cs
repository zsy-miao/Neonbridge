// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Models;

/// <summary>The streams of a geometry beyond position, normal and first texture coordinate; each null when absent.</summary>
/// <param name="Tangents">x, y, z, w per vertex in model space; w is the bitangent sign.</param>
/// <param name="TexCoords1">Slot 4, u, v pairs.</param>
/// <param name="TexCoords2">Slot 14, u, v pairs.</param>
/// <param name="TexCoords3">Slot 15, u, v pairs.</param>
/// <param name="Skin">Joint indices and weights.</param>
public sealed record VertexExtras(float[]? Tangents, float[]? TexCoords1, float[]? TexCoords2, float[]? TexCoords3, VertexSkin? Skin)
{
    public static VertexExtras None { get; } = new(null, null, null, null, null);
}
