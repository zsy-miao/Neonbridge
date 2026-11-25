// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Graphics.Pipelines;
using FiveStar.Loader.Shims.D3D12X.Binding;
using Vortice.Direct3D;
using Vortice.Direct3D12;

namespace FiveStar.Benchmarks;

/// <summary>
/// A draw's user data recorded into a desktop command list on the default adapter, the two ways the loader can pass it:
/// copied to a block of the write-combined <see cref="UserDataRing"/> with root parameter 0 pointed at it (the
/// default), or as root constants (--user-data-constants). The calls go through the vtable as the loader makes them;
/// every 256 the list is closed and reset. What the GPU then reads (a CBV in upload memory or SGPRs) is not measured.
/// BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class D3D12XUserDataBenchmarks : IDisposable
{
    private const int Flushes = 256;

    /// <summary>ID3D12GraphicsCommandList: SetGraphicsRoot32BitConstants and SetGraphicsRootConstantBufferView (d3d12.h).</summary>
    private const int SetGraphicsRoot32BitConstantsSlot = 36;
    private const int SetGraphicsRootConstantBufferViewSlot = 38;

    private ID3D12Device? _device;
    private ID3D12CommandAllocator? _allocator;
    private ID3D12GraphicsCommandList? _list;
    private ID3D12RootSignature? _bufferRoot;
    private ID3D12RootSignature? _constantsRoot;
    private UserDataRing? _ring;
    private uint* _data;
    private nint* _vtable;

    /// <summary>The used dwords: a root signature of a few parameters past s8, and one of 24 SGPRs.</summary>
    [Params(16u, 32u)]
    public uint Dwords { get; set; }

    [GlobalSetup]
    public void Setup()
    {
        D3D12.D3D12CreateDevice(null, FeatureLevel.Level_11_0, out _device).CheckError();
        _allocator = _device!.CreateCommandAllocator(CommandListType.Direct);
        _list = _device.CreateCommandList<ID3D12GraphicsCommandList>(0, CommandListType.Direct, _allocator, null);
        _bufferRoot = _device.CreateRootSignature(BindlessRootSignature.Description(userConstants: false));
        _constantsRoot = _device.CreateRootSignature(BindlessRootSignature.Description(userConstants: true));

        _ring = UserDataRing.Create(_device.NativePointer) ?? throw new InvalidOperationException("no user-data ring");
        _data = (uint*)NativeMemory.AllocZeroed(CommandListBindings.UserDataDwords, sizeof(uint));
        _vtable = *(nint**)_list.NativePointer;
    }

    /// <summary>The default: a ring block and a root CBV.</summary>
    [Benchmark(Baseline = true, OperationsPerInvoke = Flushes)]
    public void RingAndRootCbv()
    {
        nint list = _list!.NativePointer;
        var setView = (delegate* unmanaged<nint, uint, ulong, void>)_vtable[SetGraphicsRootConstantBufferViewSlot];
        _list.SetGraphicsRootSignature(_bufferRoot);

        for (uint i = 0; i < Flushes; i++)
        {
            _data[0] = i;
            setView(list, BindlessRootSignature.UserDataParameter, _ring!.Upload(_data, Dwords));
        }

        Restart();
    }

    /// <summary>--user-data-constants: the same dwords as root constants.</summary>
    [Benchmark(OperationsPerInvoke = Flushes)]
    public void RootConstants()
    {
        nint list = _list!.NativePointer;
        var setConstants = (delegate* unmanaged<nint, uint, uint, uint*, uint, void>)_vtable[SetGraphicsRoot32BitConstantsSlot];
        _list.SetGraphicsRootSignature(_constantsRoot);

        for (uint i = 0; i < Flushes; i++)
        {
            _data[0] = i;
            setConstants(list, BindlessRootSignature.UserDataParameter, Dwords, _data, 0);
        }

        Restart();
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _list?.Dispose();
        _allocator?.Dispose();
        _bufferRoot?.Dispose();
        _constantsRoot?.Dispose();
        _device?.Dispose();

        NativeMemory.Free(_data);
        GC.SuppressFinalize(this);
    }

    /// <summary>The list is never executed, so its allocator can be reset at once.</summary>
    private void Restart()
    {
        _list!.Close();
        _allocator!.Reset();
        _list.Reset(_allocator, null);
    }
}
