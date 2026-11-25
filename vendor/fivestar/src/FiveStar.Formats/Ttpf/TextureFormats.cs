// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Frozen;

namespace FiveStar.Formats.Ttpf;

/// <summary>The DXGI formats seen in texture descriptors (numbers from the DXGI_FORMAT enumeration).</summary>
public static class TextureFormats
{
    private static readonly TextureFormat[] Catalog =
    [
        new(2, "R32G32B32A32_FLOAT", 16, 1),
        new(10, "R16G16B16A16_FLOAT", 8, 1),
        new(11, "R16G16B16A16_UNORM", 8, 1),
        new(24, "R10G10B10A2_UNORM", 4, 1),
        new(26, "R11G11B10_FLOAT", 4, 1),
        new(28, "R8G8B8A8_UNORM", 4, 1),
        new(29, "R8G8B8A8_SRGB", 4, 1),
        new(49, "R8G8_UNORM", 2, 1),
        new(54, "R16_FLOAT", 2, 1),
        new(56, "R16_UNORM", 2, 1),
        new(61, "R8_UNORM", 1, 1),
        new(65, "A8_UNORM", 1, 1),
        new(71, "BC1_UNORM", 8, 4),
        new(72, "BC1_UNORM_SRGB", 8, 4),
        new(74, "BC2_UNORM", 16, 4),
        new(75, "BC2_SRGB", 16, 4),
        new(77, "BC3_UNORM", 16, 4),
        new(78, "BC3_SRGB", 16, 4),
        new(80, "BC4_UNORM", 8, 4),
        new(81, "BC4_SNORM", 8, 4),
        new(83, "BC5_UNORM", 16, 4),
        new(84, "BC5_SNORM", 16, 4),
        new(87, "B8G8R8A8_UNORM", 4, 1),
        new(88, "B8G8R8X8_UNORM", 4, 1),
        new(91, "B8G8R8A8_SRGB", 4, 1),
        new(95, "BC6H_UF16", 16, 4),
        new(96, "BC6H_SF16", 16, 4),
        new(98, "BC7_UNORM", 16, 4),
        new(99, "BC7_SRGB", 16, 4),
    ];

    private static readonly FrozenDictionary<int, TextureFormat> ByDxgi = Catalog.ToFrozenDictionary(format => format.Dxgi);

    /// <summary>The format with a DXGI number, or null when it is not supported.</summary>
    public static TextureFormat? Find(int dxgi)
    {
        ByDxgi.TryGetValue(dxgi, out var format);

        return format;
    }
}
