// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader;

namespace FiveStar.Benchmarks;

/// <summary>
/// What a GDK call costs the game: the thunk path xgameruntime.lib compiles every X* call to (QueryApiImpl, a vtable
/// slot through the drop-this thunk, Release), an XAsync round trip on a manual queue (XAsyncRun, the work and the
/// completion dispatched, XAsyncGetStatus), and a callback handed to a thread-pool queue and waited for.
/// BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class GdkCallBenchmarks : IDisposable
{
    private const int DeviceTypeSlot = 3;
    private const int ReleaseSlot = 2;
    private const uint ManualMode = 0;
    private const uint ThreadPoolMode = 1;
    private const uint WorkPort = 0;
    private const uint CompletionPort = 1;
    private const uint NoWait = 0;

    /// <summary>sizeof(XAsyncBlock) on x64; the queue is at 0 and the completion routine at 16.</summary>
    private const int AsyncBlockSize = 0x38;

    private const int CallbackOffset = 16;

    private static readonly Guid Clsid = new("5d6c0d1a-0000-4000-8000-000000000001");
    private static readonly Guid Iid = new("5d6c0d1a-0000-4000-8000-000000000002");
    private static readonly SemaphoreSlim Ran = new(0);

    private LoaderRuntime? _runtime;
    private delegate* unmanaged<Guid*, Guid*, nint*, int> _query;
    private delegate* unmanaged<nint, uint, uint, byte> _dispatch;
    private delegate* unmanaged<nint, nint, int> _run;
    private delegate* unmanaged<nint, byte, int> _status;
    private delegate* unmanaged<nint, uint, nint, nint, int> _submit;
    private nint _manual;
    private nint _pool;
    private nint _block;
    private nint _work;
    private nint _signal;

    [GlobalSetup]
    public void Setup()
    {
        _runtime = BenchmarkRuntime.Create();
        _runtime.Gdk.Runtime.RegisterAbi(Clsid, Iid, DeviceTypeSlot, "XSystemGetDeviceType");
        _query = (delegate* unmanaged<Guid*, Guid*, nint*, int>)Shim("QueryApiImpl");
        _dispatch = (delegate* unmanaged<nint, uint, uint, byte>)Shim("XTaskQueueDispatch");
        _run = (delegate* unmanaged<nint, nint, int>)Shim("XAsyncRun");
        _status = (delegate* unmanaged<nint, byte, int>)Shim("XAsyncGetStatus");

        _submit = (delegate* unmanaged<nint, uint, nint, nint, int>)Shim("XTaskQueueSubmitCallback");
        _manual = CreateQueue(ManualMode);
        _pool = CreateQueue(ThreadPoolMode);
        _block = (nint)NativeMemory.AllocZeroed(AsyncBlockSize);
        *(nint*)_block = _manual;

        delegate* unmanaged<nint, void> completed = &Completed;
        delegate* unmanaged<nint, int> work = &Work;
        delegate* unmanaged<nint, byte, void> signal = &Signal;
        *(nint*)(_block + CallbackOffset) = (nint)completed;
        _work = (nint)work;
        _signal = (nint)signal;
    }

    /// <summary>One GDK call as the game makes it: QueryApiImpl, the method slot, Release.</summary>
    [Benchmark]
    public uint ThunkCall()
    {
        Guid clsid = Clsid;
        Guid iid = Iid;
        nint instance = 0;
        _query(&clsid, &iid, &instance);
        nint* vtable = *(nint**)instance;
        uint value = ((delegate* unmanaged<nint, uint>)vtable[DeviceTypeSlot])(instance);

        ((delegate* unmanaged<nint, uint>)vtable[ReleaseSlot])(instance);

        return value;
    }

    [Benchmark]
    public int XAsyncRoundTrip()
    {
        _run(_block, _work);
        _dispatch(_manual, WorkPort, NoWait);
        _dispatch(_manual, CompletionPort, NoWait);

        return _status(_block, 0);
    }

    /// <summary>Submit to a thread-pool queue and wait until a pool thread ran the callback.</summary>
    [Benchmark]
    public bool ThreadPoolDispatch()
    {
        _submit(_pool, WorkPort, 0, _signal);

        return Ran.Wait(Timeout.Infinite);
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _runtime?.Dispose();
        GC.SuppressFinalize(this);
    }

    [UnmanagedCallersOnly]
    private static int Work(nint block)
    {
        return 0;
    }

    [UnmanagedCallersOnly]
    private static void Completed(nint block)
    {
    }

    [UnmanagedCallersOnly]
    private static void Signal(nint context, byte canceled)
    {
        Ran.Release();
    }

    private nint CreateQueue(uint mode)
    {
        nint queue = 0;
        ((delegate* unmanaged<uint, uint, nint*, int>)Shim("XTaskQueueCreate"))(mode, mode, &queue);

        return queue;
    }

    private nint Shim(string name)
    {
        return BenchmarkRuntime.Gdk(_runtime!, name);
    }
}
