// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.D3D12X;

/// <summary>
/// The DXGI cast families: a *_TYPELESS format and the formats a view of it may take (dxgiformat.h order). A resource
/// may carry ALLOW_UNORDERED_ACCESS when its format or another member of its family has typed UAV support; the
/// runtime checks exactly that (GetResourceAllocationInfo and resource creation refuse the rest).
/// </summary>
internal static class FormatFamilies
{
    /// <summary>Each family: the typeless format first, then its members. B8G8R8A8/X8 and the XR_BIAS format sit apart.</summary>
    private static readonly int[][] Families =
    [
        [1, 2, 3, 4], [5, 6, 7, 8], [9, 10, 11, 12, 13, 14], [15, 16, 17, 18], [19, 20, 21, 22],
        [23, 24, 25, 89], [27, 28, 29, 30, 31, 32], [33, 34, 35, 36, 37, 38], [39, 40, 41, 42, 43], [44, 45, 46, 47],
        [48, 49, 50, 51, 52], [53, 54, 55, 56, 57, 58, 59], [60, 61, 62, 63, 64], [70, 71, 72], [73, 74, 75], [76, 77, 78],
        [79, 80, 81], [82, 83, 84], [90, 87, 91], [92, 88, 93], [94, 95, 96], [97, 98, 99],
    ];

    /// <summary>The formats sharing <paramref name="format"/>'s family; the format alone when it has none.</summary>
    public static IReadOnlyList<int> Members(int format)
    {
        foreach (var family in Families)
        {
            if (Array.IndexOf(family, format) >= 0)
            {
                return family;
            }
        }

        return [format];
    }
}
