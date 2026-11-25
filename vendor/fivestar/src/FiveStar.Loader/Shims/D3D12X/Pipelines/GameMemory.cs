// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.D3D12X.Pipelines;

/// <summary>Copies what a pipeline description points at out of game memory.</summary>
internal static unsafe class GameMemory
{
    // Bytecode beyond this is not a shader (the largest DXBC container of the reference dump is a few MB).
    private const long MaxRange = 1L << 30;

    /// <summary>The bytes of a {pointer, size} range; empty for a null pointer or zero size.</summary>
    public static byte[] Copy(nint address, long size)
    {
        if (address == 0 || size <= 0)
        {
            return [];
        }

        if (size > MaxRange)
        {
            throw new InvalidDataException($"range of 0x{size:x} bytes at 0x{address:x} is not plausible");
        }

        return new ReadOnlySpan<byte>((void*)address, (int)size).ToArray();
    }

    /// <summary>The first <paramref name="limit"/> bytes of a range (all of it when shorter).</summary>
    public static byte[] CopyPrefix(nint address, long size, int limit)
    {
        return Copy(address, Math.Min(size, limit));
    }

    /// <summary>A NUL-terminated ANSI string, empty for a null pointer.</summary>
    public static string Ansi(nint address)
    {
        if (address == 0)
        {
            return string.Empty;
        }

        return Marshal.PtrToStringAnsi(address) ?? string.Empty;
    }

    /// <summary>The {pointer, u64 size} pair at <paramref name="field"/>.</summary>
    public static (nint Address, long Size) Range(byte* desc, int field)
    {
        return (*(nint*)(desc + field), *(long*)(desc + field + PipelineLayout.PointerSize));
    }
}
