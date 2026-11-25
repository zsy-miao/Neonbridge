// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The descs of the null texture descriptors (<see cref="FiveStar.Shaders.Translation.NullTextureSlots"/>): a
/// D3D12_SHADER_RESOURCE_VIEW_DESC or D3D12_UNORDERED_ACCESS_VIEW_DESC of the dimension of an HLSL texture type, one
/// mip and one array slice, R32G32B32A32_FLOAT, written into the 40 bytes the desktop calls take.
/// </summary>
internal static unsafe class NullTextureViews
{
    /// <summary>DXGI_FORMAT_R32G32B32A32_FLOAT.</summary>
    private const int Format = 2;

    /// <summary>D3D12_SRV_DIMENSION and D3D12_UAV_DIMENSION values of the texture kinds (the same numbers for both).</summary>
    private const int Texture1D = 2;
    private const int Texture1DArray = 3;
    private const int Texture2D = 4;
    private const int Texture2DArray = 5;
    private const int Texture3D = 8;
    private const int TextureCube = 9;

    /// <summary>Dwords of an SRV desc: the component mapping at 2, the dimension's fields from 4 (MostDetailedMip, MipLevels, then FirstArraySlice, ArraySize).</summary>
    private const int MappingDword = 2;
    private const int SrvMipLevels = 5;
    private const int SrvArraySize = 7;

    /// <summary>Dwords of a UAV desc: the dimension's fields from 2 (MipSlice, then FirstArraySlice / FirstWSlice, ArraySize / WSize).</summary>
    private const int UavArraySize = 4;

    private const int DimensionDword = 1;

    /// <summary>The null SRV desc of <paramref name="type"/> (Texture1D to TextureCube).</summary>
    public static BufferViewDescription ShaderResource(string type)
    {
        BufferViewDescription view = default;
        int* dwords = (int*)&view;
        dwords[0] = Format;
        dwords[DimensionDword] = Dimension(type);
        dwords[MappingDword] = BufferViewDescription.DefaultMapping;
        dwords[SrvMipLevels] = 1;

        if (type.EndsWith("Array", StringComparison.Ordinal))
        {
            dwords[SrvArraySize] = 1;
        }

        return view;
    }

    /// <summary>The null UAV desc of <paramref name="type"/> (RWTexture1D to RWTexture3D).</summary>
    public static BufferViewDescription UnorderedAccess(string type)
    {
        BufferViewDescription view = default;
        int* dwords = (int*)&view;
        dwords[0] = Format;
        dwords[DimensionDword] = Dimension(type["RW".Length..]);

        // One array slice, or for a 3D view one W slice.
        if (type.EndsWith("Array", StringComparison.Ordinal) || type.EndsWith("3D", StringComparison.Ordinal))
        {
            dwords[UavArraySize] = 1;
        }

        return view;
    }

    private static int Dimension(string type)
    {
        return type switch
        {
            "Texture1D" => Texture1D,
            "Texture1DArray" => Texture1DArray,
            "Texture2D" => Texture2D,
            "Texture2DArray" => Texture2DArray,
            "Texture3D" => Texture3D,
            "TextureCube" => TextureCube,
            _ => throw new ArgumentException($"no null view for {type}", nameof(type)),
        };
    }
}
