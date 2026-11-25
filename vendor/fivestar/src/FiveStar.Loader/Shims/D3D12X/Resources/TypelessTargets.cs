// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// <c>--typeless-targets</c>: a GPU-written color texture in R8G8B8A8_UNORM, B8G8R8A8_UNORM, B8G8R8X8_UNORM or R8_UNORM
/// (the G-buffer and lighting inputs) is made in its family's TYPELESS format, so a view in another member of the family
/// (the lighting's UNORM_SRGB read of an albedo target, a UINT read) takes the format the game asked for; with the full
/// format the desktop keeps the texture's own and the sRGB decode or integer reading is lost (<see cref="ViewFormats.Loss"/>).
/// A view the game makes without a desc gets the UNORM member, the format it created the texture in, as the desktop
/// takes no view of a typeless texture without one.
/// </summary>
internal static unsafe class TypelessTargets
{
    /// <summary>DXGI formats (dxgiformat.h): each UNORM member and its family's TYPELESS format.</summary>
    private static readonly (int Unorm, int Typeless)[] Families = [(28, 27), (87, 90), (88, 92), (61, 60)];

    /// <summary>D3D12_RESOURCE_DIMENSION_TEXTURE1D, TEXTURE2D and TEXTURE3D (d3d12.h).</summary>
    private const int Texture1D = 2;
    private const int Texture2D = 3;
    private const int Texture3D = 4;

    /// <summary>The RTV, UAV and SRV dimensions (d3d12.h): TEXTURE1D 2, 1D ARRAY 3, 2D 4, 2D ARRAY 5, 2DMS 6, 2DMS ARRAY 7, 3D 8.</summary>
    private const int View1D = 2;
    private const int View2D = 4;
    private const int View2DMs = 6;
    private const int View3D = 8;
    private const int ArrayStep = 1;

    /// <summary>D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING; -1 for all mips or all W slices.</summary>
    private const int DefaultMapping = 0x1688;
    private const int All = -1;

    /// <summary>Dword of an SRV desc's union (D3D12_SHADER_RESOURCE_VIEW_DESC: it holds a UINT64, so it starts at +0x10).</summary>
    private const int ShaderViewUnion = 4;

    /// <summary>Dword of an RTV or UAV desc's union and of an SRV desc's mapping.</summary>
    private const int TargetViewUnion = 2;
    private const int MappingDword = 2;

    /// <summary>Union dwords: FirstArraySlice of an SRV and of an RTV or UAV (ArraySize follows), and an RTV or UAV's WSize.</summary>
    private const int ShaderFirstSlice = 2;
    private const int TargetFirstSlice = 1;
    private const int TargetWSize = 2;

    /// <summary>ID3D12Resource::GetDesc (desktop order: object, then the return buffer).</summary>
    private const int GetDescSlot = 10;

    /// <summary>Whether GPU-written UNORM color textures are made typeless (off by default).</summary>
    public static bool Enabled { get; set; }

    /// <summary>The format a GPU-written color texture of <paramref name="format"/> is made in.</summary>
    public static int Resource(int format)
    {
        foreach (var (unorm, typeless) in Families)
        {
            if (Enabled && format == unorm)
            {
                return typeless;
            }
        }

        return format;
    }

    /// <summary>The UNORM member of a typeless family this class makes; 0 for any other format.</summary>
    public static int Member(int format)
    {
        foreach (var (unorm, typeless) in Families)
        {
            if (format == typeless)
            {
                return unorm;
            }
        }

        return ViewFormats.Unknown;
    }

    /// <summary>
    /// For a view without a desc of <paramref name="resource"/> (a desktop resource): the whole-resource desc in its UNORM
    /// member, written to <paramref name="copy"/>, when this class made it typeless; else null.
    /// </summary>
    public static void* WholeView(nint resource, byte* copy, int size, bool shaderView, bool unordered)
    {
        if (!Enabled || resource == 0)
        {
            return null;
        }

        ResourceDescription desc;
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)ComObject.Slot(resource, GetDescSlot))(resource, &desc);
        int member = Member(desc.Format);

        if (member == ViewFormats.Unknown || (unordered && desc.SampleCount > 1))
        {
            return null;
        }

        new Span<byte>(copy, size).Clear();
        int* dwords = (int*)copy;
        dwords[0] = member;
        dwords[1] = Dimension(desc);
        Fill(dwords, dwords[1], desc.DepthOrArraySize, shaderView);

        return copy;
    }

    /// <summary>The view dimension of a whole-resource view of <paramref name="desc"/>.</summary>
    internal static int Dimension(in ResourceDescription desc)
    {
        bool array = desc.Dimension != Texture3D && desc.DepthOrArraySize > 1;
        int dimension = View3D;

        if (desc.Dimension == Texture1D)
        {
            dimension = View1D;
        }
        else if (desc.Dimension == Texture2D && desc.SampleCount > 1)
        {
            dimension = View2DMs;
        }
        else if (desc.Dimension == Texture2D)
        {
            dimension = View2D;
        }

        if (array)
        {
            dimension += ArrayStep;
        }

        return dimension;
    }

    /// <summary>The union after the dimension (dword 2 of an RTV or UAV desc; dword 4 of an SRV desc, 8-byte aligned past its mapping).</summary>
    private static void Fill(int* dwords, int dimension, int arraySize, bool shaderView)
    {
        int* union = dwords + TargetViewUnion;

        if (shaderView)
        {
            dwords[MappingDword] = DefaultMapping;
            union = dwords + ShaderViewUnion;
        }

        bool multisample = dimension == View2DMs || dimension == View2DMs + ArrayStep;
        bool array = dimension != View3D && ((dimension - View1D) & ArrayStep) != 0;

        // SRV: MostDetailedMip, MipLevels, then FirstArraySlice and ArraySize; RTV and UAV: MipSlice, then those two.
        int arrayAt = shaderView ? ShaderFirstSlice : TargetFirstSlice;

        if (shaderView && !multisample)
        {
            union[1] = All;
        }

        if (multisample)
        {
            arrayAt = 0;
        }

        if (array)
        {
            union[arrayAt + 1] = arraySize;
        }
        else if (dimension == View3D && !shaderView)
        {
            union[TargetWSize] = All;
        }
    }
}
