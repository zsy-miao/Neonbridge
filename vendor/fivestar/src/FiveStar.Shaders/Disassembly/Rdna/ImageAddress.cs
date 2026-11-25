// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Shaders.Disassembly.Rdna;

/// <summary>
/// The number of address VGPRs of an image instruction, from the address layout of the AMD "RDNA 2 Instruction Set
/// Architecture" reference guide (image address VGPRs): offset, bias and compare dwords, then derivatives, then the
/// coordinates and LOD/clamp/mip. With A16 the coordinate group (and, lacking G16, the derivatives) pack two per VGPR.
/// </summary>
internal static class ImageAddress
{
    private const int RayAddresses = 11;
    private const int Ray64Addresses = 12;
    private const int RayA16Saving = 3;
    private const int PairDwords = 2;

    /// <summary>Coordinates per dimension (1D, 2D, 3D, cube, 1D array, 2D array, 2D MSAA, 2D MSAA array).</summary>
    private static readonly int[] Coordinates = [1, 2, 3, 3, 2, 3, 3, 4];

    /// <summary>Derivatives per dimension, in the same order.</summary>
    private static readonly int[] Gradients = [2, 4, 6, 4, 2, 4, 4, 4];

    public static int Count(string name, int dim, bool a16)
    {
        if (name.StartsWith("image_bvh", StringComparison.Ordinal))
        {
            return Ray(name, a16);
        }

        if (name == "image_get_resinfo")
        {
            return 1;
        }

        var tokens = name.Split('_').Skip(PairDwords).ToHashSet(StringComparer.Ordinal);
        var extra = 0;
        var lod = 0;

        foreach (var token in new[] { "o", "b", "c" })
        {
            if (tokens.Contains(token))
            {
                extra++;
            }
        }

        if (tokens.Contains("l") || tokens.Contains("cl") || tokens.Contains("mip"))
        {
            lod = 1;
        }

        var group = Coordinates[dim] + lod;
        var gradients = 0;

        if (tokens.Contains("d") || tokens.Contains("cd"))
        {
            gradients = Gradients[dim];
        }

        if (a16 || tokens.Contains("g16"))
        {
            gradients = Packed(gradients);
        }

        if (a16)
        {
            group = (group + 1) / PairDwords;
        }

        return extra + gradients + group;
    }

    /// <summary>Derivatives packed two per VGPR, each coordinate's pair in its own dword pair.</summary>
    private static int Packed(int gradients)
    {
        if (gradients == 0)
        {
            return 0;
        }

        return ((gradients / PairDwords) + 1) & ~1;
    }

    private static int Ray(string name, bool a16)
    {
        var count = RayAddresses;

        if (name.Contains("bvh64", StringComparison.Ordinal))
        {
            count = Ray64Addresses;
        }

        if (a16)
        {
            count -= RayA16Saving;
        }

        return count;
    }
}
