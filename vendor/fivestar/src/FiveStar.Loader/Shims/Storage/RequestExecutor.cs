// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Formats.Compression;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// Runs the requests of one queue on its worker thread: two overlapped read slots, so the next request reads while the
/// current one inflates, a reusable zlib inflater, BCPack pages decoded into a reused block buffer, and delivery to system memory or the destination sink. A request
/// whose sector-aligned source does not fit the staging size is read in pieces when it is uncompressed, and fails
/// otherwise.
/// </summary>
internal sealed unsafe class RequestExecutor : IDisposable
{
    private const int SlotCount = 2;
    private const int MinimumStagingBytes = 0x100000;
    private const int MaxLoggedFailures = 16;

    private readonly ReadSlot[] _slots = new ReadSlot[SlotCount];
    private readonly ZlibInflater _inflater = new();
    private readonly IStorageDestinationSink _sink;
    private readonly BcPackPages _bcPack;
    private readonly LoaderLog _log;
    private readonly int _stagingBytes;
    private byte[] _pageBlocks = [];
    private int _loggedFailures;
    private bool _loggedCompression;

    public RequestExecutor(uint stagingBytes, IStorageDestinationSink sink, BcPackPages bcPack, LoaderLog log)
    {
        _stagingBytes = (int)Math.Clamp(stagingBytes, MinimumStagingBytes, int.MaxValue);
        _sink = sink;
        _bcPack = bcPack;
        _log = log;

        for (int i = 0; i < SlotCount; i++)
        {
            _slots[i] = new ReadSlot();
        }
    }

    /// <summary>Starts reading a file request when a slot is free and it was not started yet; memory sources need no read.</summary>
    [HotPath]
    public void Start(long sequence, in StorageRequest request)
    {
        if (request.SourceIsMemory || Find(sequence) is not null)
        {
            return;
        }

        var slot = FreeSlot();
        nint handle = FileHandle(request);

        if (slot is null || handle == 0)
        {
            return;
        }

        slot.TryBegin(sequence, handle, request.SourceOffset, request.SourceSize, _stagingBytes);
    }

    /// <summary>Finishes one request: waits for its read, inflates it and writes the destination. Returns its HRESULT.</summary>
    [HotPath]
    public int Complete(long sequence, in StorageRequest request)
    {
        int result = Run(sequence, request);

        if (HResult.Failed(result))
        {
            LogFailure(request, result);
        }

        return result;
    }

    public void Dispose()
    {
        foreach (var slot in _slots)
        {
            slot.Dispose();
        }
    }

    [HotPath]
    private static nint FileHandle(in StorageRequest request)
    {
        if (request.Source == 0)
        {
            return 0;
        }

        return StorageObjects.Owner<StorageFile>(request.Source).Handle;
    }

    [HotPath]
    private int Run(long sequence, in StorageRequest request)
    {
        if (request.SourceIsMemory)
        {
            return Deliver(request, new ReadOnlySpan<byte>((void*)request.Source, (int)request.SourceSize));
        }

        nint handle = FileHandle(request);

        if (handle == 0)
        {
            return HResult.InvalidArgument;
        }

        Start(sequence, request);
        var slot = Find(sequence);

        if (slot is null)
        {
            return ReadInPieces(request, handle);
        }

        int result = slot.Wait(out var data);

        if (!HResult.Failed(result))
        {
            result = Deliver(request, data);
        }

        slot.Release();

        return result;
    }

    /// <summary>An uncompressed request larger than the staging size, read through one slot a staging buffer at a time.</summary>
    private int ReadInPieces(in StorageRequest request, nint handle)
    {
        if (request.Compression != StorageCompression.None || !request.DestinationIsMemory || request.Destination == 0 || request.DestinationSize < request.SourceSize)
        {
            return HResult.InvalidArgument;
        }

        var slot = FreeSlot()!;
        uint piece = (uint)(_stagingBytes - (ReadSlot.Alignment * SlotCount));
        var destination = (byte*)request.Destination;

        for (uint done = 0; done < request.SourceSize; done += piece)
        {
            uint size = Math.Min(piece, request.SourceSize - done);
            slot.TryBegin(0, handle, request.SourceOffset + done, size, _stagingBytes);
            int result = slot.Wait(out var data);
            slot.Release();

            if (HResult.Failed(result))
            {
                return result;
            }

            data.CopyTo(new Span<byte>(destination + done, (int)size));
        }

        return HResult.Ok;
    }

    [HotPath]
    private int Deliver(in StorageRequest request, ReadOnlySpan<byte> source)
    {
        int result = Decompress(request, source, out var data);

        if (HResult.Failed(result))
        {
            return result;
        }

        if (!request.DestinationIsMemory)
        {
            return _sink.Write(request, data);
        }

        if (request.Destination == 0 || data.Length > request.DestinationSize)
        {
            return HResult.InvalidArgument;
        }

        data.CopyTo(new Span<byte>((void*)request.Destination, (int)request.DestinationSize));

        return HResult.Ok;
    }

    /// <summary>The request's final bytes: the source as stored, inflated, or decoded from BCPack to BC blocks.</summary>
    [HotPath]
    private int Decompress(in StorageRequest request, ReadOnlySpan<byte> source, out ReadOnlySpan<byte> data)
    {
        data = source;

        if (request.Compression == StorageCompression.None)
        {
            return HResult.Ok;
        }

        if (request.Compression == StorageCompression.Zlib)
        {
            var inflated = _inflater.Inflate(source, DeflateWrapper.Zlib, (int)request.DestinationSize);
            data = _inflater.Output;

            if (inflated.Status != InflateStatus.Complete || data.Length != request.DestinationSize)
            {
                return HResult.InvalidData;
            }

            return HResult.Ok;
        }

        int codec = StorageCompression.BcPackCodec(request.Compression);

        if (codec == 0)
        {
            return UnknownCompression(request);
        }

        if (request.DestinationSize > (uint)_stagingBytes)
        {
            return HResult.InvalidArgument;
        }

        var blocks = PageBlocks((int)request.DestinationSize);
        data = blocks;

        return _bcPack.Decode(codec, source, blocks);
    }

    /// <summary>The reused buffer for a page's BC blocks, grown to the largest page so far.</summary>
    private Span<byte> PageBlocks(int size)
    {
        if (_pageBlocks.Length < size)
        {
            _pageBlocks = new byte[size];
        }

        return _pageBlocks.AsSpan(0, size);
    }

    [HotPath]
    private ReadSlot? Find(long sequence)
    {
        foreach (var slot in _slots)
        {
            if (slot.Busy && slot.Sequence == sequence)
            {
                return slot;
            }
        }

        return null;
    }

    [HotPath]
    private ReadSlot? FreeSlot()
    {
        foreach (var slot in _slots)
        {
            if (!slot.Busy)
            {
                return slot;
            }
        }

        return null;
    }

    /// <summary>A format neither zlib nor BCPack: logged once, failed with E_NOTIMPL.</summary>
    private int UnknownCompression(in StorageRequest request)
    {
        if (!_loggedCompression)
        {
            _loggedCompression = true;
            _log.Warn($"  [xcrdapi] DirectStorage compression format {request.Compression} is not implemented (only none, zlib and BCPack)");
        }

        return HResult.NotImplemented;
    }

    private void LogFailure(in StorageRequest request, int result)
    {
        if (_loggedFailures >= MaxLoggedFailures)
        {
            return;
        }

        _loggedFailures++;
        string source = "memory";

        if (!request.SourceIsMemory && request.Source != 0)
        {
            source = StorageObjects.Owner<StorageFile>(request.Source).Path;
        }

        _log.Warn($"  [xcrdapi] DirectStorage request failed {HResult.Format(result)}: {source} at 0x{request.SourceOffset:x}, 0x{request.SourceSize:x} bytes, compression {request.Compression}, 0x{request.DestinationSize:x} out");
    }
}
