// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Bvh;

/// <summary>
/// The header of a serialized console BLAS, as XboxBvhBuilder.dll's rehydration reads it (CPU path, its disassembly):
/// only the fields needed to bound the memory the rehydration touches. See docs/raytracing.md.
/// </summary>
internal static unsafe class SerializedBvhHeader
{
    /// <summary>Bytes read before any field is trusted.</summary>
    public const int Size = 0x40;

    /// <summary>dword0 bit 24: the structure is serialized and still needs rehydration.</summary>
    private const uint SerializedFlag = 0x01000000;

    private const int TotalBlocksOffset = 0x04;

    /// <summary>The structure size is (dword1 + 1) 64-byte blocks.</summary>
    private const int BlockShift = 6;

    public static bool IsSerialized(byte* header)
    {
        return (*(uint*)header & SerializedFlag) != 0;
    }

    /// <summary>
    /// The whole structure: the rehydration reads the level table at its end from the source and writes up to it in
    /// the destination.
    /// </summary>
    public static ulong TotalSize(byte* header)
    {
        return ((ulong)*(uint*)(header + TotalBlocksOffset) + 1) << BlockShift;
    }
}
