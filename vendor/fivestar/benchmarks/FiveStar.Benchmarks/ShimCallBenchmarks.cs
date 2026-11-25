// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader;
using FiveStar.Loader.Codegen;

namespace FiveStar.Benchmarks;

/// <summary>
/// The cost of the calls mapped code makes into the loader, through the same native function pointers its IAT holds:
/// a trivial shim against a trivial native function, an XMemAlloc/XMemFree pair, and an XTaskQueue submit plus dispatch
/// of a no-op callback on a manual queue. BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class ShimCallBenchmarks : IDisposable
{
    private const nuint AllocationSize = 64;
    private const ulong DefaultAttributes = 0;
    private const uint ManualMode = 0;
    private const uint WorkPort = 0;
    private const uint NoWait = 0;

    private LoaderRuntime? _runtime;
    private delegate* unmanaged<uint> _nativeBaseline;
    private delegate* unmanaged<uint> _deviceType;
    private delegate* unmanaged<nuint, ulong, nint> _alloc;
    private delegate* unmanaged<nint, ulong, void> _free;
    private delegate* unmanaged<nint, uint, nint, nint, int> _submit;
    private delegate* unmanaged<nint, uint, uint, byte> _dispatch;
    private nint _queue;
    private nint _noop;

    [GlobalSetup]
    public void Setup()
    {
        _runtime = BenchmarkRuntime.Create();

        _nativeBaseline = (delegate* unmanaged<uint>)WriteBaseline(_runtime.CodeHeap);
        _deviceType = (delegate* unmanaged<uint>)Shim("XSystemGetDeviceType");
        _alloc = (delegate* unmanaged<nuint, ulong, nint>)_runtime.Shims.TryResolve("xmem.dll", "XMemAlloc", 0)!.Address;
        _free = (delegate* unmanaged<nint, ulong, void>)_runtime.Shims.TryResolve("xmem.dll", "XMemFree", 0)!.Address;
        _submit = (delegate* unmanaged<nint, uint, nint, nint, int>)Shim("XTaskQueueSubmitCallback");
        _dispatch = (delegate* unmanaged<nint, uint, uint, byte>)Shim("XTaskQueueDispatch");

        nint queue = 0;
        ((delegate* unmanaged<uint, uint, nint*, int>)Shim("XTaskQueueCreate"))(ManualMode, ManualMode, &queue);
        _queue = queue;
        delegate* unmanaged<nint, byte, void> noop = &Noop;
        _noop = (nint)noop;
    }

    [Benchmark(Baseline = true)]
    public uint NativeToNative()
    {
        return _nativeBaseline();
    }

    [Benchmark]
    public uint NativeToShim()
    {
        return _deviceType();
    }

    [Benchmark]
    public nint XMemAllocFree()
    {
        nint block = _alloc(AllocationSize, DefaultAttributes);
        _free(block, DefaultAttributes);

        return block;
    }

    [Benchmark]
    public byte TaskQueueSubmitDispatch()
    {
        _submit(_queue, WorkPort, 0, _noop);

        return _dispatch(_queue, WorkPort, NoWait);
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _runtime?.Dispose();
        GC.SuppressFinalize(this);
    }

    [UnmanagedCallersOnly]
    private static void Noop(nint context, byte canceled)
    {
    }

    private static nint WriteBaseline(CodeHeap heap)
    {
        var code = new X64Emitter();
        code.MovEax(BenchmarkRuntime.DeviceType);
        code.Ret();
        nint function = heap.Write(code.ToArray());
        heap.Seal();

        return function;
    }

    private nint Shim(string name)
    {
        return BenchmarkRuntime.Gdk(_runtime!, name);
    }
}
