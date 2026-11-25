// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.IO.MemoryMappedFiles;

namespace FiveStar.Formats.Common;

/// <summary>
/// A read-only file mapped one window at a time, for scans that move through a file larger than memory: a span stays
/// valid only until the next read. A read outside the window maps a new one and unmaps the old, so the pages already
/// scanned leave the working set (a whole-file mapping keeps every page read: 35 GB over levels.rpf).
/// </summary>
public sealed unsafe class WindowedFileSource : IByteSource
{
    /// <summary>The window for scans: large enough for every read the archive carvers make (16 MiB).</summary>
    public const long DefaultWindowBytes = 256L << 20;

    /// <summary>Views start on this boundary (the allocation granularity of Windows).</summary>
    private const long ViewAlignment = 64 << 10;

    private readonly MemoryMappedFile? _map;
    private readonly long _windowBytes;
    private MemoryMappedViewAccessor? _view;
    private byte* _base;
    private long _viewStart;
    private long _viewLength;
    private bool _disposed;

    /// <param name="path">The file.</param>
    /// <param name="windowBytes">The bytes mapped at once (<see cref="DefaultWindowBytes"/>); a longer read maps more.</param>
    public WindowedFileSource(string path, long windowBytes)
    {
        Length = new FileInfo(path).Length;
        _windowBytes = Math.Max(windowBytes, ViewAlignment);

        if (Length > 0)
        {
            _map = MemoryMappedFile.CreateFromFile(path, FileMode.Open, null, 0, MemoryMappedFileAccess.Read);
        }
    }

    public long Length { get; }

    /// <summary>The bytes at <paramref name="offset"/>, valid until the next call.</summary>
    public ReadOnlySpan<byte> Read(long offset, int maxLength)
    {
        ObjectDisposedException.ThrowIf(_disposed, this);

        if (offset >= Length || offset < 0 || maxLength <= 0)
        {
            return ReadOnlySpan<byte>.Empty;
        }

        var length = (int)Math.Min(maxLength, Length - offset);

        if (offset < _viewStart || offset + length > _viewStart + _viewLength)
        {
            Map(offset, length);
        }

        return new ReadOnlySpan<byte>(_base + (offset - _viewStart), length);
    }

    public void Dispose()
    {
        _disposed = true;
        Unmap();
        _map?.Dispose();
    }

    private void Map(long offset, int length)
    {
        Unmap();
        var start = offset / ViewAlignment * ViewAlignment;
        var size = Math.Min(Length - start, Math.Max(_windowBytes, offset + length - start));
        _view = _map!.CreateViewAccessor(start, size, MemoryMappedFileAccess.Read);

        byte* pointer = null;
        _view.SafeMemoryMappedViewHandle.AcquirePointer(ref pointer);
        _base = pointer + _view.PointerOffset;
        _viewStart = start;
        _viewLength = size;
    }

    private void Unmap()
    {
        if (_view is null)
        {
            return;
        }

        _view.SafeMemoryMappedViewHandle.ReleasePointer();
        _view.Dispose();
        _view = null;
        _base = null;
        _viewLength = 0;
    }
}
