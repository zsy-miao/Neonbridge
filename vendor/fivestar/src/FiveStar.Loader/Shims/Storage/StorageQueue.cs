// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.Storage;

/// <summary>
/// One DirectStorage queue. Enqueued entries wait until Submit; a worker thread then runs them in order: requests
/// through the <see cref="RequestExecutor"/> (reading the next request while the current one inflates), status entries
/// with the first failure since the previous status, fences signalled. Enqueue and Submit allocate nothing.
/// </summary>
internal sealed unsafe class StorageQueue : IDisposable
{
    private const int StatusComplete = 1;
    private const int StatusPending = 0;

    private readonly ConcurrentQueue<QueueItem> _items = new();
    private readonly SemaphoreSlim _wake = new(0);
    private readonly RequestExecutor _executor;
    private readonly LoaderLog _log;
    private Thread? _worker;
    private long _sequence;
    private long _enqueued;
    private long _submitted;
    private long _taken;
    private long _requestsRun;
    private long _requestsFailed;
    private long _bytesDelivered;
    private int _closed;
    private int _batchResult;
    private QueueItem _lookahead;
    private bool _hasLookahead;

    public StorageQueue(QueueSettings settings, RequestExecutor executor, LoaderLog log)
    {
        Settings = settings;
        _executor = executor;
        _log = log;
    }

    public QueueSettings Settings { get; }

    /// <summary>Entries enqueued so far (requests, status entries and signals).</summary>
    public long Enqueued => Volatile.Read(ref _enqueued);

    /// <summary>Entries handed to the worker by Submit so far.</summary>
    public long Submitted => Volatile.Read(ref _submitted);

    /// <summary>Requests the worker ran, and how many of them failed.</summary>
    public long RequestsRun => Volatile.Read(ref _requestsRun);

    public long RequestsFailed => Volatile.Read(ref _requestsFailed);

    /// <summary>Destination bytes of the requests that succeeded.</summary>
    public long BytesDelivered => Volatile.Read(ref _bytesDelivered);

    /// <summary>Starts the worker thread.</summary>
    public void Start()
    {
        _worker = new Thread(Run)
        {
            IsBackground = true,
            Name = "DirectStorage " + Settings.Name,
        };

        _worker.Start();
    }

    /// <summary>EnqueueRequest: copies the request; holds a reference on its file until it ran.</summary>
    [HotPath]
    public void EnqueueRequest(byte* request)
    {
        var copy = StorageRequest.Read(request);

        if (!copy.SourceIsMemory && copy.Source != 0)
        {
            StorageObjects.AddRef(copy.Source);
        }

        Add(new QueueItem(QueueItemKind.Request, NextSequence(), copy, 0, 0));
    }

    /// <summary>EnqueueStatus: the entry reads incomplete from now until the worker reaches it.</summary>
    [HotPath]
    public void EnqueueStatus(nint statusArray, uint index)
    {
        if (statusArray == 0 || index >= (uint)((StorageObjectHeader*)statusArray)->Count)
        {
            return;
        }

        Volatile.Write(ref StorageObjects.Entries(statusArray)[index].Complete, StatusPending);
        StorageObjects.AddRef(statusArray);
        Add(new QueueItem(QueueItemKind.Status, NextSequence(), default, statusArray, index));
    }

    /// <summary>EnqueueSignal: signals <paramref name="fence"/> to <paramref name="value"/> once the entries before it ran.</summary>
    [HotPath]
    public void EnqueueSignal(nint fence, ulong value)
    {
        if (fence == 0)
        {
            return;
        }

        ((delegate* unmanaged<nint, uint>)ComObject.Slot(fence, DStorageLayout.AddRefSlot))(fence);
        Add(new QueueItem(QueueItemKind.Signal, NextSequence(), default, fence, value));
    }

    /// <summary>Submit: hands every entry enqueued so far to the worker.</summary>
    [HotPath]
    public void Submit()
    {
        if (Volatile.Read(ref _closed) != 0)
        {
            return;
        }

        long enqueued = Volatile.Read(ref _enqueued);
        long submitted = Volatile.Read(ref _submitted);

        while (submitted < enqueued)
        {
            long seen = Interlocked.CompareExchange(ref _submitted, enqueued, submitted);

            if (seen == submitted)
            {
                break;
            }

            submitted = seen;
        }

        _wake.Release();
    }

    /// <summary>Close and the last Release: stops the worker; entries not run yet complete as aborted.</summary>
    public void Dispose()
    {
        if (Interlocked.Exchange(ref _closed, 1) != 0)
        {
            return;
        }

        _wake.Release();
        _worker?.Join();
        Abort();
        _executor.Dispose();

        // The semaphore stays undisposed: a Submit racing with Close may still release it.
        _log.Detail($"  [xcrdapi] DirectStorage queue \"{Settings.Name}\" closed");
    }

    [HotPath]
    private long NextSequence()
    {
        return Interlocked.Increment(ref _sequence);
    }

    /// <summary>Queues an entry; after Close it is aborted at once, so its references are not kept.</summary>
    [HotPath]
    private void Add(in QueueItem item)
    {
        if (Volatile.Read(ref _closed) != 0)
        {
            AbortItem(item);

            return;
        }

        _items.Enqueue(item);
        Interlocked.Increment(ref _enqueued);
    }

    private void Run()
    {
        while (true)
        {
            _wake.Wait();

            if (Volatile.Read(ref _closed) != 0)
            {
                return;
            }

            while (TryTake(out var item))
            {
                Execute(item);
            }
        }
    }

    /// <summary>The next submitted entry: the look-ahead when there is one, else the queue's head.</summary>
    [HotPath]
    private bool TryTake(out QueueItem item)
    {
        if (_hasLookahead)
        {
            item = _lookahead;
            _hasLookahead = false;

            return true;
        }

        if (_taken >= Volatile.Read(ref _submitted) || !_items.TryDequeue(out item))
        {
            item = default;

            return false;
        }

        _taken++;

        return true;
    }

    /// <summary>Takes the entry after the current one when it was submitted, so its read can start early.</summary>
    [HotPath]
    private void PeekNext()
    {
        if (_hasLookahead || _taken >= Volatile.Read(ref _submitted) || !_items.TryDequeue(out _lookahead))
        {
            return;
        }

        _taken++;
        _hasLookahead = true;
    }

    [HotPath]
    private void Execute(in QueueItem item)
    {
        switch (item.Kind)
        {
            case QueueItemKind.Request:
                RunRequest(item);
                break;
            case QueueItemKind.Status:
                CompleteStatus(item, Interlocked.Exchange(ref _batchResult, HResult.Ok));
                break;
            default:
                SignalFence(item);
                break;
        }
    }

    [HotPath]
    private void RunRequest(in QueueItem item)
    {
        _executor.Start(item.Sequence, item.Request);
        PeekNext();

        if (_hasLookahead && _lookahead.Kind == QueueItemKind.Request)
        {
            _executor.Start(_lookahead.Sequence, _lookahead.Request);
        }

        int result = _executor.Complete(item.Sequence, item.Request);
        Interlocked.Increment(ref _requestsRun);

        if (HResult.Failed(result))
        {
            Interlocked.CompareExchange(ref _batchResult, result, HResult.Ok);
            Interlocked.Increment(ref _requestsFailed);
        }
        else
        {
            Interlocked.Add(ref _bytesDelivered, item.Request.DestinationSize);
        }

        ReleaseSource(item.Request);
    }

    [HotPath]
    private static void ReleaseSource(in StorageRequest request)
    {
        if (request.SourceIsMemory || request.Source == 0)
        {
            return;
        }

        StorageFileMethods.ReleaseFile(request.Source);
    }

    [HotPath]
    private static void CompleteStatus(in QueueItem item, int result)
    {
        ref var entry = ref StorageObjects.Entries(item.Target)[item.Value];
        entry.Result = result;
        Volatile.Write(ref entry.Complete, StatusComplete);
        StatusArrayMethods.ReleaseArray(item.Target);
    }

    [HotPath]
    private static void SignalFence(in QueueItem item)
    {
        ((delegate* unmanaged<nint, ulong, int>)ComObject.Slot(item.Target, DStorageLayout.FenceSignalSlot))(item.Target, item.Value);
        ComObject.Release(item.Target);
    }

    /// <summary>After the worker stopped: requests and fences are dropped, status entries complete with E_ABORT.</summary>
    private void Abort()
    {
        if (_hasLookahead)
        {
            _hasLookahead = false;
            AbortItem(_lookahead);
        }

        while (_items.TryDequeue(out var item))
        {
            AbortItem(item);
        }
    }

    private static void AbortItem(in QueueItem item)
    {
        switch (item.Kind)
        {
            case QueueItemKind.Request:
                ReleaseSource(item.Request);
                break;
            case QueueItemKind.Status:
                CompleteStatus(item, HResult.Abort);
                break;
            default:
                ComObject.Release(item.Target);
                break;
        }
    }
}
