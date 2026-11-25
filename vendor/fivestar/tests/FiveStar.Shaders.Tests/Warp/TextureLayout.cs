// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Vortice.Direct3D12;

namespace FiveStar.Shaders.Tests.Warp;

/// <summary>
/// How a <see cref="TextureResource"/> becomes a D3D12 texture: its resource and view descriptions and the staging
/// layout of its subresources in a buffer (256-byte row pitch, 512-byte aligned subresources of level 0's size).
/// </summary>
public sealed class TextureLayout
{
    private const int RowAlignment = 256;
    private const int PlacementAlignment = 512;

    private readonly TextureResource _texture;

    public TextureLayout(TextureResource texture)
    {
        _texture = texture;
        RowPitch = Align(texture.Width * texture.TexelBytes, RowAlignment);
        Depth = 1;
        Subresources = texture.Layers * texture.Levels;

        if (texture.Kind == TextureKind.Texture3D)
        {
            Depth = texture.Layers;
            Subresources = 1;
        }

        SubresourceBytes = Align(RowPitch * texture.Height * Depth, PlacementAlignment);
    }

    public int RowPitch { get; }

    public int Depth { get; }

    public int Subresources { get; }

    public int SubresourceBytes { get; }

    public int StagedBytes => SubresourceBytes * Subresources;

    public ResourceDescription Resource()
    {
        var flags = ResourceFlags.None;

        if (_texture.Writable)
        {
            flags = ResourceFlags.AllowUnorderedAccess;
        }

        var (width, height, layers) = ((uint)_texture.Width, (uint)_texture.Height, (ushort)_texture.Layers);

        switch (_texture.Kind)
        {
            case TextureKind.Texture1D:
            case TextureKind.Texture1DArray:
                return ResourceDescription.Texture1D(_texture.Format, width, layers, 1, flags);
            case TextureKind.Texture3D:
                return ResourceDescription.Texture3D(_texture.Format, width, height, layers, 1, flags);
            default:
                return ResourceDescription.Texture2D(_texture.Format, width, height, layers, (ushort)_texture.Levels, 1, 0, flags);
        }
    }

    public PlacedSubresourceFootPrint Footprint(int subresource)
    {
        var level = subresource % _texture.Levels;
        var footprint = new SubresourceFootPrint(_texture.Format, (uint)Extent(_texture.Width, level), (uint)Extent(_texture.Height, level), (uint)Depth, (uint)RowPitch);

        return new PlacedSubresourceFootPrint { Offset = (ulong)(subresource * SubresourceBytes), Footprint = footprint };
    }

    /// <summary>The texels in staging layout.</summary>
    public uint[] Stage()
    {
        var staged = new uint[StagedBytes / 4];
        Copy(_texture.Words, staged, toStaging: true);

        return staged;
    }

    /// <summary>The texels of a staging copy, packed as in <see cref="TextureResource.Words"/>.</summary>
    public uint[] Unstage(uint[] staged)
    {
        var words = new uint[_texture.Words.Length];
        Copy(staged, words, toStaging: false);

        return words;
    }

    public ShaderResourceViewDescription ShaderResourceView()
    {
        var description = new ShaderResourceViewDescription { Format = _texture.Format, Shader4ComponentMapping = ShaderComponentMapping.Default };
        var layers = (uint)_texture.Layers;

        switch (_texture.Kind)
        {
            case TextureKind.Texture1D:
                description.ViewDimension = ShaderResourceViewDimension.Texture1D;
                description.Texture1D = new Texture1DShaderResourceView { MipLevels = 1 };
                break;
            case TextureKind.Texture3D:
                description.ViewDimension = ShaderResourceViewDimension.Texture3D;
                description.Texture3D = new Texture3DShaderResourceView { MipLevels = 1 };
                break;
            case TextureKind.Texture1DArray:
                description.ViewDimension = ShaderResourceViewDimension.Texture1DArray;
                description.Texture1DArray = new Texture1DArrayShaderResourceView { MipLevels = 1, ArraySize = layers };
                break;
            case TextureKind.Texture2DArray:
                description.ViewDimension = ShaderResourceViewDimension.Texture2DArray;
                description.Texture2DArray = new Texture2DArrayShaderResourceView { MipLevels = (uint)_texture.Levels, ArraySize = layers };
                break;
            case TextureKind.TextureCube:
                description.ViewDimension = ShaderResourceViewDimension.TextureCube;
                description.TextureCube = new TextureCubeShaderResourceView { MipLevels = 1 };
                break;
            default:
                description.ViewDimension = ShaderResourceViewDimension.Texture2D;
                description.Texture2D = new Texture2DShaderResourceView { MipLevels = (uint)_texture.Levels };
                break;
        }

        return description;
    }

    public UnorderedAccessViewDescription UnorderedAccessView()
    {
        var description = new UnorderedAccessViewDescription { Format = _texture.Format };
        var layers = (uint)_texture.Layers;

        switch (_texture.Kind)
        {
            case TextureKind.Texture1D:
                description.ViewDimension = UnorderedAccessViewDimension.Texture1D;
                break;
            case TextureKind.Texture3D:
                description.ViewDimension = UnorderedAccessViewDimension.Texture3D;
                description.Texture3D = new Texture3DUnorderedAccessView { WSize = layers };
                break;
            case TextureKind.Texture1DArray:
                description.ViewDimension = UnorderedAccessViewDimension.Texture1DArray;
                description.Texture1DArray = new Texture1DArrayUnorderedAccessView { ArraySize = layers };
                break;
            case TextureKind.Texture2DArray:
            case TextureKind.TextureCube:
                description.ViewDimension = UnorderedAccessViewDimension.Texture2DArray;
                description.Texture2DArray = new Texture2DArrayUnorderedAccessView { ArraySize = layers };
                break;
            default:
                description.ViewDimension = UnorderedAccessViewDimension.Texture2D;
                break;
        }

        return description;
    }

    private void Copy(uint[] source, uint[] destination, bool toStaging)
    {
        var packed = 0;

        for (var subresource = 0; subresource < Subresources; subresource++)
        {
            var level = subresource % _texture.Levels;
            var rowWords = Extent(_texture.Width, level) * _texture.TexelBytes / 4;

            for (var row = 0; row < Extent(_texture.Height, level) * Depth; row++, packed += rowWords)
            {
                var staged = ((subresource * SubresourceBytes) + (row * RowPitch)) / 4;

                if (toStaging)
                {
                    Array.Copy(source, packed, destination, staged, rowWords);
                }
                else
                {
                    Array.Copy(source, staged, destination, packed, rowWords);
                }
            }
        }
    }

    private static int Extent(int size, int level)
    {
        return Math.Max(size >> level, 1);
    }

    private static int Align(int value, int alignment)
    {
        return (value + alignment - 1) / alignment * alignment;
    }
}
