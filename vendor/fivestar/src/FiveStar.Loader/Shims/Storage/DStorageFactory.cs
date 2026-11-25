// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Codegen;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Loading;
using FiveStar.Loader.Mods;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// The DirectStorage factory XCRDAPI.dll hands out: one native object for any interface id. It opens package files
/// through the file overlay, creates status arrays and CPU queues (<see cref="StorageQueue"/>), and takes the staging
/// size the game sets at start-up. The console interfaces are not public; the slots come from the executable's calls
/// (<see cref="DStorageLayout"/>).
/// </summary>
internal sealed unsafe class DStorageFactory : IDisposable
{
    private const string OverlayFunction = "DStorageOpenFile";

    private readonly object _gate = new();
    private readonly List<StorageQueue> _queues = [];
    private readonly UnresolvedStubs _stubs;
    private readonly CodeHeap _heap;
    private readonly FileOverlay _overlay;
    private readonly IStorageDestinationSink _sink;
    private readonly LoaderLog _log;
    private StorageVtables? _vtables;
    private nint _factory;
    private uint _stagingBytes = DStorageLayout.DefaultStagingBytes;

    public DStorageFactory(UnresolvedStubs stubs, CodeHeap heap, FileOverlay overlay, IStorageDestinationSink sink, LoaderLog log)
    {
        _stubs = stubs;
        _heap = heap;
        _overlay = overlay;
        _sink = sink;
        _log = log;
        BcPack = new BcPackPages(log);
    }

    /// <summary>The BCPack page decoder every queue shares, with its placeholder switch and totals.</summary>
    public BcPackPages BcPack { get; }

    /// <summary>DStorageGetFactory: the one factory object for any interface id.</summary>
    public int GetFactory(Guid* iid, nint* result)
    {
        if (result == null)
        {
            return HResult.Pointer;
        }

        lock (_gate)
        {
            if (_vtables is null)
            {
                _vtables = new StorageVtables(_stubs, _heap);
                _factory = StorageObjects.Create(_vtables.Factory, this, 0);
            }

            *result = _factory;
        }

        string requested = iid == null ? "null" : iid->ToString();
        _log.Info($"  [xcrdapi] DStorageGetFactory({requested}) -> CPU DirectStorage factory");

        return HResult.Ok;
    }

    /// <summary>OpenFile: the path through the mod overlay, opened for overlapped unbuffered reads.</summary>
    public int OpenFile(char* path, nint* result)
    {
        if (path == null || result == null)
        {
            return HResult.InvalidArgument;
        }

        *result = 0;
        string target = _overlay.Redirect(path, OverlayFunction) ?? new string(path);
        int opened = StorageFile.Open(target, out var file);
        _log.Detail($"  [xcrdapi] IDStorageFactoryX::OpenFile(\"{target}\") -> {HResult.Format(opened)}");

        if (file is null)
        {
            return opened;
        }

        *result = StorageObjects.Create(_vtables!.File, file, 0);

        return HResult.Ok;
    }

    /// <summary>CreateStatusArray: <paramref name="capacity"/> entries, all complete with S_OK until a status is enqueued.</summary>
    public int CreateStatusArray(uint capacity, nint* result)
    {
        if (result == null || capacity == 0 || capacity > int.MaxValue / sizeof(StatusEntry))
        {
            return HResult.InvalidArgument;
        }

        nint array = StorageObjects.Create(_vtables!.StatusArray, null, (int)capacity * sizeof(StatusEntry));
        ((StorageObjectHeader*)array)->Count = (int)capacity;
        var entries = StorageObjects.Entries(array);

        for (int i = 0; i < (int)capacity; i++)
        {
            entries[i].Complete = 1;
        }

        *result = array;

        return HResult.Ok;
    }

    /// <summary>The staging size of queues created afterwards; any size is accepted (small ones are raised to a minimum).</summary>
    public int SetStagingBufferSize(uint bytes)
    {
        Volatile.Write(ref _stagingBytes, bytes);
        _log.Detail($"  [xcrdapi] IDStorageFactoryX::SetStagingBufferSize(0x{bytes:x}) -> S_OK");

        return HResult.Ok;
    }

    /// <summary>CreateQueue: a CPU queue with its own worker thread.</summary>
    public int CreateQueue(byte* desc, nint* result)
    {
        if (desc == null || result == null)
        {
            return HResult.InvalidArgument;
        }

        var settings = QueueSettings.Read(desc);
        var executor = new RequestExecutor(Volatile.Read(ref _stagingBytes), _sink, BcPack, _log);
        var queue = new StorageQueue(settings, executor, _log);
        queue.Start();

        lock (_gate)
        {
            _queues.Add(queue);
        }

        *result = StorageObjects.Create(_vtables!.Queue, queue, 0);
        _log.Detail($"  [xcrdapi] IDStorageFactoryX::CreateQueue(\"{settings.Name}\", memory source {settings.MemorySource}, capacity {settings.Capacity}, priority {settings.Priority}) -> S_OK");

        return HResult.Ok;
    }

    /// <summary>One line of totals over every queue, for the thread sampler.</summary>
    public string Summary()
    {
        StorageQueue[] queues;

        lock (_gate)
        {
            queues = [.. _queues];
        }

        long enqueued = queues.Sum(q => q.Enqueued);
        long submitted = queues.Sum(q => q.Submitted);
        long run = queues.Sum(q => q.RequestsRun);
        long failed = queues.Sum(q => q.RequestsFailed);
        long bytes = queues.Sum(q => q.BytesDelivered);

        return $"{queues.Length} queues: {enqueued} entries enqueued, {submitted} submitted, {run} requests run ({failed} failed), 0x{bytes:x} bytes delivered; {BcPack.Summary()}";
    }

    /// <summary>Stops every queue; the native objects the game still holds are left to the process exit.</summary>
    public void Dispose()
    {
        StorageQueue[] queues;

        lock (_gate)
        {
            queues = [.. _queues];
            _queues.Clear();
        }

        foreach (var queue in queues)
        {
            queue.Dispose();
        }

        _vtables?.Dispose();
    }
}
