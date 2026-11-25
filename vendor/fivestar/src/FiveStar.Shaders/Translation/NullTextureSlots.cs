// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Translation;

/// <summary>
/// Null texture descriptors of each dimension at fixed heap slots, after the loader's reserved buffer slots. A T# whose
/// heap index is not a game view (0 from a zero or unwritten record, another reserved slot, or past the heap) is read
/// through the null descriptor of its own dimension (rdna_prelude tex_slot), not as a texture over the raw window buffer
/// at slot 0: a type mismatch, undefined in D3D12 (XCL reports a zero T# indexing slot 0 hung NVIDIA; facts only).
/// The game's views start at <see cref="End"/>.
/// </summary>
public static class NullTextureSlots
{
    /// <summary>The first null texture slot (the loader's reserved buffer slots end here).</summary>
    public const uint First = 10;

    /// <summary>The first slot after them: the first desktop index of the game's views (rdna_prelude FIRST_GAME_VIEW).</summary>
    public const uint End = First + 11;

    /// <summary>The read-only dimensions, in slot order from <see cref="First"/>.</summary>
    public static readonly string[] Views = ["Texture1D", "Texture1DArray", "Texture2D", "Texture2DArray", "Texture3D", "TextureCube"];

    /// <summary>The writable dimensions, in slot order after <see cref="Views"/>.</summary>
    public static readonly string[] Uavs = ["RWTexture1D", "RWTexture1DArray", "RWTexture2D", "RWTexture2DArray", "RWTexture3D"];

    /// <summary>The null slot of an HLSL texture type (Texture2D, RWTexture3D, ...).</summary>
    public static uint For(string type)
    {
        int view = Array.IndexOf(Views, type);

        if (view >= 0)
        {
            return First + (uint)view;
        }

        int uav = Array.IndexOf(Uavs, type);

        if (uav < 0)
        {
            throw new ArgumentException($"no null slot for {type}", nameof(type));
        }

        return First + (uint)Views.Length + (uint)uav;
    }

    /// <summary>The HLSL heap index of a texture of <paramref name="type"/> whose T# holds <paramref name="index"/>.</summary>
    public static string Slot(string type, string index)
    {
        return $"tex_slot({index}, {For(type)}u)";
    }
}
