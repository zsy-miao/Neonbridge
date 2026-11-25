// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Audio;

/// <summary>One stream of an AWC container: its 29-bit name hash and its chunks.</summary>
/// <param name="Id">The low 29 bits of the stream's name hash.</param>
/// <param name="Chunks">The stream's chunks, in table order.</param>
public sealed record AwcStreamEntry(uint Id, IReadOnlyList<AwcChunk> Chunks)
{
    /// <summary>The first chunk with the tag, or null.</summary>
    public AwcChunk? Find(AwcChunkTag tag)
    {
        foreach (var chunk in Chunks)
        {
            if (chunk.Tag == (byte)tag)
            {
                return chunk;
            }
        }

        return null;
    }
}
