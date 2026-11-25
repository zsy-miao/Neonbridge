// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.DXGI;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// A texture at a <c>ResourceDescriptorHeap</c> slot, texels given as raw 32-bit words, layer after layer and in a layer
/// level after level. <see cref="Layers"/> is the array size of an array or cube (6) and the depth of a 3D texture;
/// <see cref="Levels"/> (2D textures and arrays) its mip levels. A writable texture is bound as a UAV and read back after
/// the run.
/// </summary>
public sealed record TextureResource(int HeapIndex, Format Format, int Width, int Height, uint[] Words)
{
    public TextureKind Kind { get; init; } = TextureKind.Texture2D;

    public int Layers { get; init; } = 1;

    public int Levels { get; init; } = 1;

    public bool Writable { get; init; }

    /// <summary>Bytes per texel: 4 for the one-channel 32-bit formats, 8 for the two-channel ones (64-bit atomics), else 16 (RGBA32).</summary>
    public int TexelBytes
    {
        get
        {
            if (Format is Format.R32_UInt or Format.R32_SInt or Format.R32_Float)
            {
                return 4;
            }

            if (Format is Format.R32G32_UInt or Format.R32G32_SInt)
            {
                return 8;
            }

            return 16;
        }
    }
}
