// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Numerics;
using System.Runtime.CompilerServices;

namespace FiveStar.Loader.Shims.D3D12X.Resources;

/// <summary>
/// A 64-bit hash of a texture's bytes, to skip an upload when the memory did not change since the last one. Not
/// cryptographic: a multiply-rotate fold over 8-byte words (the 64-bit FNV prime), never 0.
/// </summary>
internal static unsafe class ContentHash
{
    private const ulong Seed = 0xCBF29CE484222325;
    private const ulong Prime = 0x100000001B3;
    private const int Rotation = 29;

    [HotPath]
    public static ulong Of(byte* data, ulong size)
    {
        ulong hash = Seed ^ size;
        ulong words = size / sizeof(ulong);
        var word = (ulong*)data;

        for (ulong index = 0; index < words; index++)
        {
            hash = BitOperations.RotateLeft((hash ^ Unsafe.ReadUnaligned<ulong>(word + index)) * Prime, Rotation);
        }

        for (ulong index = words * sizeof(ulong); index < size; index++)
        {
            hash = (hash ^ data[index]) * Prime;
        }

        return hash | 1;
    }
}
