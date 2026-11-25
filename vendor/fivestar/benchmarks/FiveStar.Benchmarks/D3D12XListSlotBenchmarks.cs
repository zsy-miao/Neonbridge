// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader;
using FiveStar.Loader.Codegen;
using FiveStar.Loader.Shims.D3D12X;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Benchmarks;

/// <summary>
/// The command-list slot handlers the game calls most, end to end on a wrapper whose desktop list is a stand-in (every
/// desktop slot returns at once): what our handler adds per draw, dispatch, root constant and pipeline. No device: the
/// list has no root signature, so draws upload no user data. BenchmarkDotNet derives a runner from the class, so it
/// cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class D3D12XListSlotBenchmarks : IDisposable
{
    private const int DesktopSlots = 160;
    private const int DesktopObjectBytes = 64;
    private const uint Vertices = 3;
    private const uint Constant = 7;
    private const uint TriangleList = 4;

    private LoaderRuntime? _runtime;
    private nint* _desktopVtable;
    private nint _desktop;
    private XboxObject* _list;
    private delegate* unmanaged<XboxObject*, uint, uint, uint, uint, void> _draw;
    private delegate* unmanaged<XboxObject*, uint, uint, uint, void> _dispatch;
    private delegate* unmanaged<XboxObject*, uint, uint, uint, void> _constant;
    private delegate* unmanaged<XboxObject*, nint, void> _pipeline;
    private delegate* unmanaged<XboxObject*, uint, void> _topology;

    [GlobalSetup]
    public void Setup()
    {
        _runtime = BenchmarkRuntime.Create();
        var code = new X64Emitter();
        code.Ret();
        nint ret = _runtime.CodeHeap.Write(code.ToArray());
        _runtime.CodeHeap.Seal();

        _desktopVtable = (nint*)NativeMemory.AllocZeroed(DesktopSlots, (nuint)sizeof(nint));

        for (int i = 0; i < DesktopSlots; i++)
        {
            _desktopVtable[i] = ret;
        }

        _desktop = (nint)NativeMemory.AllocZeroed(DesktopObjectBytes);
        *(nint**)_desktop = _desktopVtable;
        _list = (XboxObject*)NativeMemory.AllocZeroed((nuint)XboxInterfaces.CommandListObjectSize);
        _list->Inner = _desktop;

        _draw = &CommandListBindingMethods.DrawInstanced;
        _dispatch = &CommandListBindingMethods.Dispatch;
        _constant = &CommandListBindingMethods.SetGraphicsRoot32BitConstant;
        _pipeline = &RectangleListSlots.SetPipelineState;
        _topology = &PrimitiveTopologies.SetPrimitiveTopology;
    }

    [Benchmark]
    public void Draw()
    {
        _draw(_list, Vertices, 1, 0, 0);
    }

    [Benchmark]
    public void Dispatch()
    {
        _dispatch(_list, 1, 1, 1);
    }

    [Benchmark]
    public void RootConstant()
    {
        _constant(_list, 0, Constant, 0);
    }

    [Benchmark]
    public void SetPipeline()
    {
        _pipeline(_list, _desktop);
    }

    [Benchmark]
    public void SetTopology()
    {
        _topology(_list, TriangleList);
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _runtime?.Dispose();
        NativeMemory.Free(_desktopVtable);
        NativeMemory.Free((void*)_desktop);
        NativeMemory.Free(_list);
        GC.SuppressFinalize(this);
    }
}
