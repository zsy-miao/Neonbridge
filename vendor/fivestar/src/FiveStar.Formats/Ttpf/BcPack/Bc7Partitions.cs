// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Ttpf.BcPack;

/// <summary>The public BC7 (and BC6H) anchor pixels and mode properties.</summary>
internal static class Bc7Partitions
{
    /// <summary>Anchor of the second subset of the two-subset partitions.</summary>
    public static readonly byte[] Anchor2 =
    [
        15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 2, 8, 2, 2, 8, 8, 15, 2, 8, 2, 2, 8, 8, 2, 2,
        15, 15, 6, 8, 2, 8, 15, 15, 2, 8, 2, 2, 2, 15, 15, 6, 6, 2, 6, 8, 15, 15, 2, 2, 15, 15, 15, 15, 15, 2, 2, 15,
    ];

    /// <summary>Anchor of the second subset of the three-subset partitions.</summary>
    public static readonly byte[] Anchor3Second =
    [
        3, 3, 15, 15, 8, 3, 15, 15, 8, 8, 6, 6, 6, 5, 3, 3, 3, 3, 8, 15, 3, 3, 6, 10, 5, 8, 8, 6, 8, 5, 15, 15,
        8, 15, 3, 5, 6, 10, 8, 15, 15, 3, 15, 5, 15, 15, 15, 15, 3, 15, 5, 5, 5, 8, 5, 10, 5, 10, 8, 13, 15, 12, 3, 3,
    ];

    /// <summary>Anchor of the third subset of the three-subset partitions.</summary>
    public static readonly byte[] Anchor3Third =
    [
        15, 8, 8, 3, 15, 15, 3, 8, 15, 15, 15, 15, 15, 15, 15, 8, 15, 8, 15, 3, 15, 8, 15, 8, 3, 15, 6, 10, 15, 15, 10, 8,
        15, 3, 15, 10, 10, 8, 9, 10, 6, 15, 8, 15, 3, 6, 6, 8, 15, 3, 15, 15, 15, 15, 15, 15, 15, 15, 15, 15, 3, 15, 15, 8,
    ];

    /// <summary>Endpoint bits of R, G, B and A per mode, before p-bits and rotation.</summary>
    public static readonly byte[][] EndpointBits =
    [
        [4, 4, 4, 0], [6, 6, 6, 0], [5, 5, 5, 0], [7, 7, 7, 0], [5, 5, 5, 6], [7, 7, 7, 8], [7, 7, 7, 7], [5, 5, 5, 5],
    ];

    /// <summary>Index bits of the primary and secondary sets per mode.</summary>
    public static readonly byte[][] IndexBits =
    [
        [3, 0], [3, 0], [2, 0], [2, 0], [3, 2], [2, 2], [4, 0], [2, 0],
    ];

    /// <summary>Whether a mode has p-bits (modes 0, 1, 3, 6 and 7; mode 1 shares one per subset).</summary>
    public static readonly bool[] HasPBits = [true, true, false, true, false, false, true, true];
}
