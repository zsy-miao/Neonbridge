// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>A byte source over an array in memory.</summary>
public sealed class MemoryByteSource : IByteSource
{
    private readonly byte[] _data;

    public MemoryByteSource(byte[] data)
    {
        _data = data;
    }

    public long Length => _data.Length;

    public ReadOnlySpan<byte> Read(long offset, int maxLength)
    {
        if (offset >= _data.Length)
        {
            return ReadOnlySpan<byte>.Empty;
        }

        var length = (int)Math.Min(maxLength, _data.Length - offset);

        return _data.AsSpan((int)offset, length);
    }

    public void Dispose()
    {
    }
}
