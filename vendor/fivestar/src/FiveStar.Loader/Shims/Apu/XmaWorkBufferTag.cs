// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Loader.Shims.Apu;

/// <summary>
/// What the CPU decoder keeps in a context's 0x200-byte work buffer: a tag naming the context and the enable it belongs
/// to. The hardware's own content is not known. When a voice chains two contexts the game copies the finished context's
/// work buffer into the next one before enabling it (0x1400781b6), so the next context finds the tag of the decoder whose
/// overlap it continues; a fresh voice zeroes the buffer (0x1400785d0) and finds none.
/// </summary>
internal readonly record struct XmaWorkBufferTag(uint Context, uint Generation)
{
    /// <summary>"XWB1" little-endian: marks a tag written by this decoder.</summary>
    private const uint Magic = 0x31425758;

    private const int MagicIndex = 0;
    private const int ContextIndex = 1;
    private const int GenerationIndex = 2;

    /// <summary>The tag in <paramref name="buffer"/>, or null when the buffer holds none.</summary>
    public static unsafe XmaWorkBufferTag? Read(uint* buffer)
    {
        if (buffer == null || buffer[MagicIndex] != Magic)
        {
            return null;
        }

        return new XmaWorkBufferTag(buffer[ContextIndex], buffer[GenerationIndex]);
    }

    public unsafe void Write(uint* buffer)
    {
        if (buffer == null)
        {
            return;
        }

        buffer[MagicIndex] = Magic;
        buffer[ContextIndex] = Context;
        buffer[GenerationIndex] = Generation;
    }
}
