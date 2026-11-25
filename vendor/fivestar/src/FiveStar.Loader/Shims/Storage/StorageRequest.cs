// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// One DSTORAGE_REQUEST, copied at enqueue time. The fields the game fills are read at their console offsets
/// (<see cref="DStorageLayout"/>); <see cref="Raw"/> keeps the whole block for destinations decoded elsewhere.
/// </summary>
internal readonly record struct StorageRequest(RequestBlock Raw)
{
    public ulong Options => Raw[DStorageLayout.RequestOptions / sizeof(ulong)];

    /// <summary>The compression format (0 none, 1 zlib).</summary>
    public byte Compression => (byte)(Options & DStorageLayout.CompressionMask);

    public bool SourceIsMemory => (Options & DStorageLayout.SourceMemoryFlag) != 0;

    /// <summary>True when no option bit beyond the compression and the source type is set: the game's only destination, system memory.</summary>
    public bool DestinationIsMemory => (Options & ~(DStorageLayout.CompressionMask | DStorageLayout.SourceMemoryFlag)) == 0;

    public nint Destination => (nint)Raw[DStorageLayout.RequestDestination / sizeof(ulong)];

    public uint DestinationSize => (uint)Raw[DStorageLayout.RequestDestinationSize / sizeof(ulong)];

    /// <summary>The IDStorageFile, or the source pointer for a memory source.</summary>
    public nint Source => (nint)Raw[DStorageLayout.RequestSource / sizeof(ulong)];

    public ulong SourceOffset => Raw[DStorageLayout.RequestSourceOffset / sizeof(ulong)];

    public uint SourceSize => (uint)Raw[DStorageLayout.RequestSourceSize / sizeof(ulong)];

    /// <summary>Copies the request at <paramref name="request"/>.</summary>
    [HotPath]
    public static unsafe StorageRequest Read(byte* request)
    {
        RequestBlock block = default;
        Span<ulong> words = block;
        new ReadOnlySpan<byte>(request, DStorageLayout.RequestSize).CopyTo(MemoryMarshal.AsBytes(words));

        return new StorageRequest(block);
    }
}
