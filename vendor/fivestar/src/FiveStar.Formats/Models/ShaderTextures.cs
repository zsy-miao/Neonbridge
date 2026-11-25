// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Pso;

namespace FiveStar.Formats.Models;

/// <summary>One shader of a drawable's shader group: its 64-bit name hash and its texture parameters.</summary>
public sealed record ShaderTextures(ulong Name, IReadOnlyList<TextureSlot> Slots)
{
    /// <summary>The base colour parameter.</summary>
    public static readonly uint DiffuseParameter = Joaat.Hash("DiffuseTex");

    /// <summary>The texture bound to <c>DiffuseTex</c>, if any.</summary>
    public ulong? Diffuse
    {
        get
        {
            foreach (var slot in Slots)
            {
                if (slot.Parameter == DiffuseParameter)
                {
                    return slot.Texture;
                }
            }

            return null;
        }
    }
}
