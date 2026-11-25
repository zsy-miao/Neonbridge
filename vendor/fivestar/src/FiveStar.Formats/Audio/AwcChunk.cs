// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>One entry of an AWC chunk table: a u64 of offset (bits 0..27), size (bits 28..55) and tag (bits 56..63).</summary>
/// <param name="Tag">The tag byte; see <see cref="AwcChunkTag"/>.</param>
/// <param name="Offset">Offset from the start of the container.</param>
/// <param name="Size">Size in bytes.</param>
public readonly record struct AwcChunk(byte Tag, int Offset, int Size)
{
    private const int FieldBits = 28;
    private const int TagShift = 56;
    private const ulong FieldMask = (1UL << FieldBits) - 1;

    public long End => (long)Offset + Size;

    public static AwcChunk Decode(ulong entry) =>
        new((byte)(entry >> TagShift), (int)(entry & FieldMask), (int)((entry >> FieldBits) & FieldMask));
}
