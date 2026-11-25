// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.MemoryMappedFiles;

namespace FiveStar.Formats.Common;

/// <summary>A read-only file mapped into memory as a whole; spans stay valid until the source is disposed.</summary>
public sealed unsafe class MappedFileSource : IByteSource
{
    private readonly MemoryMappedFile? _map;
    private readonly MemoryMappedViewAccessor? _view;
    private readonly byte* _base;
    private bool _disposed;

    public MappedFileSource(string path)
    {
        Length = new FileInfo(path).Length;

        if (Length == 0)
        {
            return;
        }

        _map = MemoryMappedFile.CreateFromFile(path, FileMode.Open, null, 0, MemoryMappedFileAccess.Read);
        _view = _map.CreateViewAccessor(0, 0, MemoryMappedFileAccess.Read);
        byte* pointer = null;
        _view.SafeMemoryMappedViewHandle.AcquirePointer(ref pointer);
        _base = pointer + _view.PointerOffset;
    }

    public long Length { get; }

    public ReadOnlySpan<byte> Read(long offset, int maxLength)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        if (offset >= Length || offset < 0)
        {
            return ReadOnlySpan<byte>.Empty;
        }

        var length = (int)Math.Min(maxLength, Length - offset);

        return new ReadOnlySpan<byte>(_base + offset, length);
    }

    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;

        if (_view is not null)
        {
            _view.SafeMemoryMappedViewHandle.ReleasePointer();
            _view.Dispose();
        }

        _map?.Dispose();
    }
}
