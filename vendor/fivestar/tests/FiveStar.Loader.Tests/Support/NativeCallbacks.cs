// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Collections.Concurrent;
using System.Runtime.InteropServices;

namespace FiveStar.Loader.Tests.Support;

/// <summary>
/// Native callbacks handed to the GDK shims, recording what they were called with. [UnmanagedCallersOnly] methods
/// cannot carry an instance, so the recordings live here; <see cref="Reset"/> clears them at the start of each test.
/// </summary>
internal static unsafe class NativeCallbacks
{
    private static int _running;
    private static int _maxRunning;

    public static ConcurrentQueue<(nint Context, bool Canceled, int Thread)> Calls { get; } = new();

    public static ConcurrentQueue<nint> Terminated { get; } = new();

    public static ConcurrentQueue<nint> Completed { get; } = new();

    public static ConcurrentQueue<uint> ProviderOperations { get; } = new();

    public static ConcurrentQueue<string> Items { get; } = new();

    public static int MaxConcurrency => Volatile.Read(ref _maxRunning);

    /// <summary>XAsyncComplete and XAsyncSchedule, for <see cref="TestProvider"/>.</summary>
    public static delegate* unmanaged<nint, int, nuint, void> Complete { get; set; }

    public static delegate* unmanaged<nint, uint, int> Schedule { get; set; }

    /// <summary>XAsyncGetStatus, for <see cref="AsyncCompletedWaiting"/>.</summary>
    public static delegate* unmanaged<nint, byte, int> Status { get; set; }

    public static void Reset()
    {
        Calls.Clear();
        Terminated.Clear();
        Completed.Clear();
        ProviderOperations.Clear();
        Items.Clear();
        Volatile.Write(ref _maxRunning, 0);
    }

    public static List<(nint Context, bool Canceled, int Thread)> Drain()
    {
        var calls = new List<(nint, bool, int)>();

        while (Calls.TryDequeue(out var call))
        {
            calls.Add(call);
        }

        return calls;
    }

    /// <summary>XTaskQueueCallback.</summary>
    [UnmanagedCallersOnly]
    public static void Record(nint context, byte canceled)
    {
        Calls.Enqueue((context, canceled != 0, Environment.CurrentManagedThreadId));
    }

    /// <summary>XTaskQueueCallback that takes a moment and tracks how many run at once.</summary>
    [UnmanagedCallersOnly]
    public static void RecordSlowly(nint context, byte canceled)
    {
        int running = Interlocked.Increment(ref _running);
        int max;

        do
        {
            max = Volatile.Read(ref _maxRunning);
        }
        while (max < running && Interlocked.CompareExchange(ref _maxRunning, running, max) != max);

        Thread.Sleep(2);
        Interlocked.Decrement(ref _running);
        Calls.Enqueue((context, canceled != 0, Environment.CurrentManagedThreadId));
    }

    [UnmanagedCallersOnly]
    public static void QueueTerminated(nint context)
    {
        Terminated.Enqueue(context);
    }

    [UnmanagedCallersOnly]
    public static void AsyncCompleted(nint block)
    {
        Completed.Enqueue(block);
    }

    /// <summary>A completion routine that waits on its own block, as title code does before freeing it.</summary>
    [UnmanagedCallersOnly]
    public static void AsyncCompletedWaiting(nint block)
    {
        Status(block, 1);
        Completed.Enqueue(block);
    }

    /// <summary>A provider written the way title code writes one: schedule in Begin (context = delay), complete with 4 bytes in DoWork.</summary>
    [UnmanagedCallersOnly]
    public static int TestProvider(uint operation, nint* data)
    {
        ProviderOperations.Enqueue(operation);
        nint block = data[0];

        switch (operation)
        {
            case 0:
                return Schedule(block, (uint)data[3]);
            case 1:
                return CompleteWithFourBytes(block);
            case 2:
                return WriteResult((uint*)data[2]);
            default:
                return 0;
        }
    }

    private static int CompleteWithFourBytes(nint block)
    {
        Complete(block, 0, 4);

        return 0;
    }

    private static int WriteResult(uint* buffer)
    {
        *buffer = 0xDEADBEEF;

        return 0;
    }

    /// <summary>XAsyncWork for XAsyncRun: records the call and returns 0x1234 as the call's result.</summary>
    [UnmanagedCallersOnly]
    public static int RunWork(nint block)
    {
        Calls.Enqueue((block, false, Environment.CurrentManagedThreadId));

        return 0x1234;
    }

    /// <summary>XGameSaveContainerInfoCallback: "name|displayName|blobCount|totalSize".</summary>
    [UnmanagedCallersOnly]
    public static byte ContainerInfo(nint info, nint context)
    {
        Items.Enqueue($"{Marshal.PtrToStringUTF8(*(nint*)info)}|{Marshal.PtrToStringUTF8(*(nint*)(info + 8))}|{*(uint*)(info + 16)}|{*(ulong*)(info + 24)}");

        return 1;
    }

    /// <summary>XGameSaveBlobInfoCallback: "name=size".</summary>
    [UnmanagedCallersOnly]
    public static byte BlobInfo(nint info, nint context)
    {
        Items.Enqueue($"{Marshal.PtrToStringUTF8(*(nint*)info)}={*(uint*)(info + 8)}");

        return 1;
    }

    /// <summary>XPackageInstallationProgressCallback: "context:monitor" in hex.</summary>
    [UnmanagedCallersOnly]
    public static void ProgressChanged(nint context, nint monitor)
    {
        Items.Enqueue($"{context:x}:{monitor:x}");
    }

    /// <summary>XPackageEnumerationCallback: the package identifier.</summary>
    [UnmanagedCallersOnly]
    public static byte Package(nint context, nint details)
    {
        Items.Enqueue(Marshal.PtrToStringUTF8(*(nint*)details) ?? string.Empty);

        return 1;
    }
}
