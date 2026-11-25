// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Shims.Xg;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// The DXGI format a desktop SRV takes on a texture made for console memory (dxgiformat.h). A typeless texture takes
/// the view's format; a texture with a full format keeps its own (the desktop casts no further without castable
/// formats); a depth format has no SRV form, so the view is null.
/// </summary>
internal static class ViewFormats
{
    /// <summary>The *_TYPELESS members of DXGI_FORMAT, ascending.</summary>
    private static readonly int[] Typeless = [1, 5, 9, 15, 19, 23, 27, 33, 39, 44, 48, 53, 60, 70, 73, 76, 79, 82, 90, 92, 94, 97];

    /// <summary>D32_FLOAT_S8X24_UINT, D32_FLOAT, D24_UNORM_S8_UINT, D16_UNORM.</summary>
    private static readonly int[] Depth = [20, 40, 45, 55];

    public const int Unknown = 0;

    /// <summary>The format for a view of <paramref name="viewFormat"/> on a texture of <paramref name="textureFormat"/>; <see cref="Unknown"/> for none.</summary>
    [HotPath]
    public static int ForView(int textureFormat, int viewFormat)
    {
        if (Array.BinarySearch(Depth, textureFormat) >= 0)
        {
            return Unknown;
        }

        if (viewFormat == Unknown || Array.BinarySearch(Typeless, textureFormat) < 0)
        {
            return textureFormat;
        }

        return viewFormat;
    }

    /// <summary>
    /// What a view in <paramref name="viewFormat"/> loses when it is created in <paramref name="created"/> on a texture
    /// of <paramref name="textureFormat"/> (<see cref="ForView"/>): nothing, the cast to another member of the texture's
    /// family (sRGB decode, integer or signed reading), or the reading of its bytes in a format of another family.
    /// </summary>
    public static ViewFormatLoss Loss(int textureFormat, int viewFormat, int created)
    {
        if (viewFormat == Unknown || created == Unknown || created == viewFormat)
        {
            return ViewFormatLoss.None;
        }

        if (FormatFamilies.Members(textureFormat).Contains(viewFormat))
        {
            return ViewFormatLoss.Cast;
        }

        return ViewFormatLoss.Reinterpret;
    }

    /// <summary>
    /// The format to create a read-only texture in for a view: the view's own when it has the same element size as
    /// the resource's (sRGB and typeless twins), else the resource's.
    /// </summary>
    public static int ForTexture(int resourceFormat, int viewFormat)
    {
        if (viewFormat == Unknown || viewFormat == resourceFormat)
        {
            return resourceFormat;
        }

        var resource = XgFormats.Find((uint)resourceFormat);
        var view = XgFormats.Find((uint)viewFormat);

        if (resource.BytesPerElement != view.BytesPerElement || resource.BlockDimension != view.BlockDimension || resource.StencilPlane)
        {
            return resourceFormat;
        }

        return viewFormat;
    }
}
