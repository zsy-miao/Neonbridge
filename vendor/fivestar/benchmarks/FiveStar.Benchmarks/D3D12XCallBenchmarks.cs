// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Shims.D3D12X;

namespace FiveStar.Benchmarks;

/// <summary>
/// The hottest D3D12.X path: a call the game makes on a wrapper object that ends in a slot of the desktop object. The
/// generated tail-call thunk (ForwardingThunk, with and without unwrapping a wrapped argument in rdx) against the same
/// forward written as a managed [UnmanagedCallersOnly] handler, and the desktop slot called directly as the baseline.
/// BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class D3D12XCallBenchmarks : IDisposable
{
    private const int DesktopSlot = 3;
    private const int DesktopSlots = DesktopSlot + 1;
    private const int ThunkSlot = 0;
    private const int UnwrapSlot = 1;
    private const int ManagedSlot = 2;
    private const int GuardedSlot = 3;
    private const int WrapperSlots = GuardedSlot + 1;

    /// <summary>An object is its vtable pointer plus, for a wrapper, the desktop object at D3D12XLayout.InnerOffset.</summary>
    private static readonly nuint ObjectBytes = (nuint)(D3D12XLayout.InnerOffset + sizeof(nint));

    private LoaderRuntime? _runtime;
    private nint* _desktopVtable;
    private nint* _wrapperVtable;
    private nint _desktop;
    private nint _wrapper;
    private delegate* unmanaged<nint, nint, uint> _direct;
    private delegate* unmanaged<nint, nint, uint> _thunk;
    private delegate* unmanaged<nint, nint, uint> _unwrap;
    private delegate* unmanaged<nint, nint, uint> _managed;
    private delegate* unmanaged<nint, nint, uint> _guarded;

    [GlobalSetup]
    public void Setup()
    {
        _runtime = BenchmarkRuntime.Create();
        var heap = _runtime.CodeHeap;
        _desktopVtable = (nint*)NativeMemory.AllocZeroed(DesktopSlots, (nuint)sizeof(nint));
        _wrapperVtable = (nint*)NativeMemory.AllocZeroed(WrapperSlots, (nuint)sizeof(nint));
        _desktopVtable[DesktopSlot] = WriteDesktopMethod(heap);

        _desktop = (nint)NativeMemory.AllocZeroed(ObjectBytes);
        *(nint**)_desktop = _desktopVtable;
        _wrapper = (nint)NativeMemory.AllocZeroed(ObjectBytes);
        *(nint**)_wrapper = _wrapperVtable;
        *(nint*)(_wrapper + D3D12XLayout.InnerOffset) = _desktop;

        int vtableBytes = WrapperSlots * sizeof(nint);
        _wrapperVtable[ThunkSlot] = ForwardingThunk.Emit(heap, new ForwardedSlot(ThunkSlot, DesktopSlot, false, ArgumentRegisters.None), (nint)_wrapperVtable, vtableBytes, 0);
        _wrapperVtable[UnwrapSlot] = ForwardingThunk.Emit(heap, new ForwardedSlot(UnwrapSlot, DesktopSlot, false, ArgumentRegisters.Rdx), (nint)_wrapperVtable, vtableBytes, 0);
        delegate* unmanaged<nint, nint, uint> managed = &ManagedForward;
        _wrapperVtable[ManagedSlot] = (nint)managed;

        delegate* unmanaged<nint, nint, uint> guarded = &GuardedForward;
        _wrapperVtable[GuardedSlot] = (nint)guarded;
        heap.Seal();

        _direct = (delegate* unmanaged<nint, nint, uint>)_desktopVtable[DesktopSlot];
        _thunk = (delegate* unmanaged<nint, nint, uint>)_wrapperVtable[ThunkSlot];
        _unwrap = (delegate* unmanaged<nint, nint, uint>)_wrapperVtable[UnwrapSlot];
        _managed = (delegate* unmanaged<nint, nint, uint>)_wrapperVtable[ManagedSlot];
        _guarded = (delegate* unmanaged<nint, nint, uint>)_wrapperVtable[GuardedSlot];
    }

    [Benchmark(Baseline = true)]
    public uint DesktopDirect()
    {
        return _direct(_desktop, 0);
    }

    [Benchmark]
    public uint ForwardingThunkCall()
    {
        return _thunk(_wrapper, 0);
    }

    /// <summary>The thunk with a wrapped object in rdx, which it replaces by the desktop object.</summary>
    [Benchmark]
    public uint ForwardingThunkUnwrap()
    {
        return _unwrap(_wrapper, _wrapper);
    }

    [Benchmark]
    public uint ManagedHandler()
    {
        return _managed(_wrapper, 0);
    }

    /// <summary>The same handler inside the try statement every product entry point has (UnmanagedCallersGuardRule).</summary>
    [Benchmark]
    public uint GuardedHandler()
    {
        return _guarded(_wrapper, 0);
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _runtime?.Dispose();
        NativeMemory.Free(_desktopVtable);
        NativeMemory.Free(_wrapperVtable);
        NativeMemory.Free((void*)_desktop);
        NativeMemory.Free((void*)_wrapper);
        GC.SuppressFinalize(this);
    }

    /// <summary>What a shim does instead of a thunk: read the desktop object, call its slot.</summary>
    [UnmanagedCallersOnly]
    private static uint ManagedForward(nint self, nint argument)
    {
        nint inner = *(nint*)(self + D3D12XLayout.InnerOffset);
        nint* vtable = *(nint**)inner;

        return ((delegate* unmanaged<nint, nint, uint>)vtable[DesktopSlot])(inner, argument);
    }

    [UnmanagedCallersOnly]
    private static uint GuardedForward(nint self, nint argument)
    {
        try
        {
            nint inner = *(nint*)(self + D3D12XLayout.InnerOffset);
            nint* vtable = *(nint**)inner;

            return ((delegate* unmanaged<nint, nint, uint>)vtable[DesktopSlot])(inner, argument);
        }
        catch (Exception)
        {
            return 0;
        }
    }

    private static nint WriteDesktopMethod(CodeHeap heap)
    {
        var code = new X64Emitter();
        code.MovEax(BenchmarkRuntime.DeviceType);
        code.Ret();

        return heap.Write(code.ToArray());
    }
}
