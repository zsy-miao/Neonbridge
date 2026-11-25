// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

namespace FiveStar.Formats.Common;

/// <summary>Random read access to a byte range that may be far larger than one span (an archive of many gigabytes).</summary>
public interface IByteSource : IDisposable
{
    /// <summary>Gets the total length in bytes.</summary>
    long Length { get; }

    /// <summary>The bytes at <paramref name="offset"/>, cut short at the end of the source (empty past it).</summary>
    ReadOnlySpan<byte> Read(long offset, int maxLength);
}
