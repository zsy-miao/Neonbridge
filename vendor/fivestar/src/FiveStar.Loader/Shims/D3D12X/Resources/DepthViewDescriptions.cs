// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Pipelines;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// Clear values, depth-stencil views and SRVs in the console depth formats (<see cref="DepthFormats"/>), rewritten
/// into a caller's stack copy in the desktop formats; any other desc passes unchanged. A resource made from a
/// console depth format is R32G8X24_TYPELESS, which takes no view without a desc, so a null desc on one becomes the
/// whole-resource view in the depth format (D32_FLOAT_S8X24_UINT, or R32_FLOAT_X8X24_TYPELESS for an SRV); a
/// multisampled one keeps the null desc.
/// </summary>
internal static unsafe class DepthViewDescriptions
{
    /// <summary>Bytes of D3D12_CLEAR_VALUE, D3D12_DEPTH_STENCIL_VIEW_DESC and D3D12_SHADER_RESOURCE_VIEW_DESC (d3d12.h).</summary>
    public const int ClearValueSize = 20;
    public const int DepthStencilViewSize = 24;
    public const int ShaderResourceViewSize = 40;

    /// <summary>ID3D12Resource::GetDesc (desktop order: object, then the return buffer).</summary>
    private const int GetDescSlot = 10;

    /// <summary>D3D12_DSV_DIMENSION values for 2D textures and arrays; the D3D12_SRV_DIMENSION ones are one higher.</summary>
    private const int DsvTexture2D = 3;
    private const int DsvTexture2DArray = 4;
    private const int SrvDimensionOffsetFromDsv = 1;

    /// <summary>D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING.</summary>
    private const int DefaultComponentMapping = 0x1688;

    /// <summary>D3D12_TEX2D_ARRAY_DSV.ArraySize (dword 5) and D3D12_TEX2D_ARRAY_SRV.MipLevels and .ArraySize (dwords 5 and 7).</summary>
    private const int DsvArraySizeDword = 5;
    private const int SrvMipLevelsDword = 5;
    private const int SrvArraySizeDword = 7;
    private const int ViewDimensionDword = 1;
    private const int SrvMappingDword = 2;

    /// <summary>MipLevels -1: every mip from the most detailed.</summary>
    private const int AllMips = -1;

    /// <summary>
    /// The desc to pass for a clear value on a resource created in <paramref name="resourceFormat"/>:
    /// <paramref name="clear"/>, or <paramref name="copy"/> with the desktop depth format or render-target substitute.
    /// </summary>
    [HotPath]
    public static void* ClearValue(void* clear, byte* copy, int resourceFormat)
    {
        if (clear == null)
        {
            return null;
        }

        int format = *(int*)clear;
        int desktop = DepthFormats.IsConsoleDepth(format) ? DepthFormats.D32S8 : TargetFormats.View(format, resourceFormat);

        return Patched(clear, copy, ClearValueSize, desktop);
    }

    /// <summary>The desc to pass for a depth-stencil view of <paramref name="resource"/> (a desktop resource).</summary>
    [HotPath]
    public static void* DepthStencilView(nint resource, void* description, byte* copy)
    {
        if (description != null)
        {
            return Patched(description, copy, DepthStencilViewSize, DepthFormats.Desktop(*(int*)description));
        }

        if (!IsDepthFamily(resource, out var desc))
        {
            return null;
        }

        new Span<byte>(copy, DepthStencilViewSize).Clear();
        int* dwords = (int*)copy;
        dwords[0] = DepthFormats.D32S8;
        dwords[ViewDimensionDword] = Dimension(desc);

        if (desc.DepthOrArraySize > 1)
        {
            dwords[DsvArraySizeDword] = desc.DepthOrArraySize;
        }

        return copy;
    }

    /// <summary>The desc to pass for an SRV of <paramref name="resource"/> (a desktop resource).</summary>
    [HotPath]
    public static void* ShaderResourceView(nint resource, void* description, byte* copy)
    {
        if (description != null)
        {
            return Patched(description, copy, ShaderResourceViewSize, DepthFormats.ShaderView(*(int*)description));
        }

        if (!IsDepthFamily(resource, out var desc))
        {
            return null;
        }

        new Span<byte>(copy, ShaderResourceViewSize).Clear();
        int* dwords = (int*)copy;
        dwords[0] = DepthFormats.R32X8X24;
        dwords[ViewDimensionDword] = Dimension(desc) + SrvDimensionOffsetFromDsv;
        dwords[SrvMappingDword] = DefaultComponentMapping;
        dwords[SrvMipLevelsDword] = AllMips;

        if (desc.DepthOrArraySize > 1)
        {
            dwords[SrvArraySizeDword] = desc.DepthOrArraySize;
        }

        return copy;
    }

    [HotPath]
    private static void* Patched(void* description, byte* copy, int size, int format)
    {
        if (format == *(int*)description)
        {
            return description;
        }

        Buffer.MemoryCopy(description, copy, size, size);
        *(int*)copy = format;

        return copy;
    }

    /// <summary>Whether <paramref name="resource"/> is in the typeless family console depth formats become.</summary>
    [HotPath]
    private static bool IsDepthFamily(nint resource, out ResourceDescription desc)
    {
        desc = default;

        if (resource == 0)
        {
            return false;
        }

        ResourceDescription read;
        _ = ((delegate* unmanaged<nint, ResourceDescription*, ResourceDescription*>)ComObject.Slot(resource, GetDescSlot))(resource, &read);
        desc = read;

        return read.Format == DepthFormats.R32G8X24Typeless && read.SampleCount <= 1;
    }

    /// <summary>The DSV dimension of a whole 2D texture: an array when it has more than one slice.</summary>
    [HotPath]
    private static int Dimension(ResourceDescription desc)
    {
        if (desc.DepthOrArraySize > 1)
        {
            return DsvTexture2DArray;
        }

        return DsvTexture2D;
    }
}
