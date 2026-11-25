// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// The texture UAVs the game made, by the desktop descriptor (CPU handle) that holds each: view format, dimension and
/// the size of the viewed mip, so a stand-in fill (<see cref="RtStandIns"/>) knows how to write a table's UAV. Copies
/// between the game's heaps only copy records naming the same desktop descriptor, so the handle stays the key.
/// </summary>
internal static unsafe class UavViews
{
    /// <summary>D3D12_UAV_DIMENSION: TEXTURE2D, TEXTURE2DARRAY and TEXTURE3D.</summary>
    public const int Texture2D = 4;
    public const int Texture2DArray = 5;
    public const int Texture3D = 8;

    /// <summary>D3D12_RESOURCE_DIMENSION_TEXTURE2D and _TEXTURE3D.</summary>
    private const int ResourceTexture2D = 3;
    private const int ResourceTexture3D = 4;

    /// <summary>D3D12_UNORDERED_ACCESS_VIEW_DESC: format, dimension, then the dimension's fields (mip slice first).</summary>
    private const int MipField = 2;
    private const int FirstSliceField = 3;
    private const int SliceCountField = 4;

    private static readonly ConcurrentDictionary<nint, UavView> Views = new();

    /// <summary>Notes the texture UAV the game wrote to desktop descriptor <paramref name="handle"/> (a buffer or other view is forgotten there).</summary>
    public static void Note(nint handle, ResourceDescription resource, void* description)
    {
        var view = Of(resource, (uint*)description);

        if (view is null)
        {
            Views.TryRemove(handle, out _);

            return;
        }

        Views[handle] = view.Value;
    }

    public static bool TryFind(nint handle, out UavView view)
    {
        return Views.TryGetValue(handle, out view);
    }

    /// <summary>The view of a texture UAV desc (or the whole texture without one); null for anything else.</summary>
    public static UavView? Of(ResourceDescription resource, uint* description)
    {
        int format = resource.Format;
        int dimension = resource.Dimension switch
        {
            ResourceTexture2D when resource.DepthOrArraySize > 1 => Texture2DArray,
            ResourceTexture2D => Texture2D,
            ResourceTexture3D => Texture3D,
            _ => 0,
        };
        uint mip = 0;
        uint slices = resource.DepthOrArraySize;

        if (description != null)
        {
            format = (int)description[0] != 0 ? (int)description[0] : format;
            dimension = (int)description[1];
            mip = description[MipField];
            slices = Slices(resource, description, dimension, mip);
        }

        if (dimension is not (Texture2D or Texture2DArray or Texture3D))
        {
            return null;
        }

        var width = (uint)Math.Max(1UL, resource.Width >> (int)mip);
        var height = Math.Max(1u, resource.Height >> (int)mip);

        return new UavView(format, dimension, width, height, Math.Max(1u, slices));
    }

    /// <summary>The array slices or depth slices a view covers (-1 for "all from the first").</summary>
    private static uint Slices(ResourceDescription resource, uint* description, int dimension, uint mip)
    {
        uint first = description[FirstSliceField];
        uint count = description[SliceCountField];
        uint all = resource.DepthOrArraySize;

        if (dimension == Texture3D)
        {
            all = Math.Max(1u, all >> (int)mip);
        }
        else if (dimension != Texture2DArray)
        {
            return 1;
        }

        if ((count == uint.MaxValue || first + count > all) && all > first)
        {
            return all - first;
        }

        if (count == uint.MaxValue || first + count > all)
        {
            return 1;
        }

        return count;
    }
}
