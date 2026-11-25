// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// Requests into the game's streaming heap: option bit 0x200000 (set at 0x144ffd6bc) makes the destination a list
/// of physical frame numbers, one per 64 KiB page (the game's page table, read at 0x144ffd8c0), with the byte offset
/// into the first page in the word after it. The data goes into those pages (<see cref="XMemPhysicalMemory.WriteFrames"/>),
/// so every mapping of them sees it. Other destinations go to <paramref name="other"/>.
/// </summary>
internal sealed unsafe class PhysicalPageSink(XMemPhysicalMemory memory, IStorageDestinationSink other) : IStorageDestinationSink
{
    /// <summary>The destination-type bit the game sets for its streaming heap.</summary>
    public const ulong PageListFlag = 0x200000;

    /// <summary>DSTORAGE_REQUEST offset of the byte offset into the first page (a 16-bit field).</summary>
    private const int FirstPageOffset = 0x10;

    private const ulong FirstPageOffsetMask = 0xFFFF;

    public int Write(in StorageRequest request, ReadOnlySpan<byte> data)
    {
        ulong options = request.Options & ~(DStorageLayout.CompressionMask | DStorageLayout.SourceMemoryFlag);

        if (options != PageListFlag)
        {
            return other.Write(request, data);
        }

        RequestBlock block = request.Raw;
        ReadOnlySpan<ulong> words = block;
        ulong offset = words[FirstPageOffset / sizeof(ulong)] & FirstPageOffsetMask;

        if (request.Destination == 0 || data.Length > request.DestinationSize || !memory.WriteFrames((ulong*)request.Destination, offset, data))
        {
            return HResult.InvalidArgument;
        }

        return HResult.Ok;
    }
}
