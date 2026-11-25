// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using Microsoft.Win32.SafeHandles;

namespace FiveStar.Formats.Common;

/// <summary>
/// A read-only file read on demand: each read copies the bytes into a new array. For a few small reads of a large
/// file, such as a page table, where mapping the whole file would cost more than the reads.
/// </summary>
public sealed class FileByteSource : IByteSource
{
    private readonly SafeFileHandle _file;

    public FileByteSource(string path)
    {
        _file = File.OpenHandle(path, options: FileOptions.RandomAccess);
        Length = RandomAccess.GetLength(_file);
    }

    public long Length { get; }

    public ReadOnlySpan<byte> Read(long offset, int maxLength)
    {
        if (offset >= Length || offset < 0)
        {
            return ReadOnlySpan<byte>.Empty;
        }

        var bytes = new byte[(int)Math.Min(maxLength, Length - offset)];
        var read = RandomAccess.Read(_file, bytes, offset);

        return bytes.AsSpan(0, read);
    }

    public void Dispose()
    {
        _file.Dispose();
    }
}
