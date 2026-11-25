// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X.Binding;

/// <summary>
/// <c>--typed-image-views</c>: a texture view's component class in dword 3 of its descriptor record, which the
/// translated programs read to load, gather and store an integer view through a <c>uint4</c> or <c>int4</c> declaration
/// (FiveStar.Shaders TranslationOptions.TypedImageViews). A <c>float4</c> declaration of an integer view leaves its bits
/// undefined, and the lighting classifies pixels by the main depth's stencil plane (X32_TYPELESS_G8X24_UINT).
/// </summary>
internal static unsafe class ImageViewClasses
{
    /// <summary>The classes: float (unorm, snorm, float, sRGB, and no class written), uint, sint.</summary>
    public const uint Float = 0;
    public const uint Uint = 1;
    public const uint Sint = 2;

    /// <summary>The *_UINT and *_SINT members of DXGI_FORMAT, the stencil planes and the console's X16_TYPELESS_G8_UINT (120).</summary>
    private static readonly HashSet<int> UintFormats = [3, 7, 12, 17, 22, 25, 30, 36, 42, 47, 50, 57, 62, 120];
    private static readonly HashSet<int> SintFormats = [4, 8, 14, 18, 32, 38, 43, 52, 59, 64];

    /// <summary>Whether texture view records carry their class (off by default).</summary>
    public static bool Enabled { get; set; }

    /// <summary>The class of a view in <paramref name="format"/>.</summary>
    public static uint Of(int format)
    {
        if (UintFormats.Contains(format))
        {
            return Uint;
        }

        if (SintFormats.Contains(format))
        {
            return Sint;
        }

        return Float;
    }

    /// <summary>
    /// <paramref name="record"/> with the class of the texture view <paramref name="description"/> describes (an SRV or UAV
    /// desc: the format at +0, the dimension at +4) when this is on; buffer views and views without a desc as they are.
    /// </summary>
    [HotPath]
    public static BufferRecord With(BufferRecord record, void* description)
    {
        if (!Enabled || description == null || record.IsBuffer || ((int*)description)[1] == BufferViewDescription.BufferDimension)
        {
            return record;
        }

        return record with { ImageClass = Of(*(int*)description) };
    }
}
