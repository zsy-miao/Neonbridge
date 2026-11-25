// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Reflection;
using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Benchmarks;

/// <summary>
/// The per-call work of the binding slots that runs on every draw: a descriptor table bound (SetRootDescriptorTable
/// mirrors its records to the window; most are unchanged since the last bind) and root constants packed into user
/// SGPRs. BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class D3D12XBindingBenchmarks : IDisposable
{
    private const uint Records = 4096;
    private const uint Increment = 32;
    private const uint DesktopCount = 8192;
    private const uint TableExtent = 1024;
    private const uint SmallExtent = 16;
    private const int Parameters = 2;
    private const byte Constants = 8;

    private byte* _title;
    private byte* _window;
    private ShaderVisibleHeap? _heap;
    private CommandListBindings* _bindings;
    private RootLayout* _layout;

    [GlobalSetup]
    public void Setup()
    {
        _title = (byte*)NativeMemory.AllocZeroed(Records * Increment);

        for (uint i = 0; i < Records; i++)
        {
            ((uint*)(_title + (i * Increment)))[0] = i + 1;
        }

        var window = new DescriptorWindow(new LoaderLog(TextWriter.Null, null, false));
        _window = (byte*)NativeMemory.AllocZeroed(DescriptorWindow.ViewBase + (Records * DescriptorWindow.ViewRecordBytes));
        typeof(DescriptorWindow).GetProperty(nameof(DescriptorWindow.Data), BindingFlags.Instance | BindingFlags.Public)!.SetValue(window, Pointer.Box(_window, typeof(byte*)));
        _heap = new ShaderVisibleHeap((nint)_title, Records, Increment, 0, DescriptorWindow.ViewRecords(DesktopCount), null, window, null);
        _heap.Table(0, Records);

        _bindings = (CommandListBindings*)NativeMemory.AllocZeroed((nuint)sizeof(CommandListBindings));
        _layout = (RootLayout*)NativeMemory.AllocZeroed((nuint)sizeof(RootLayout));
        _layout->Count = Parameters;
        _layout->Kinds[0] = (byte)RootParameterKind.Constants;
        _layout->Sizes[0] = Constants;

        _layout->Kinds[1] = (byte)RootParameterKind.Table;
        _layout->Offsets[1] = Constants;
        _layout->Sizes[1] = 1;
        _bindings->ComputeLayout = _layout;
    }

    /// <summary>A large table bound again with its records unchanged (the shadow check of every record).</summary>
    [Benchmark]
    public uint BindLargeTable()
    {
        return _heap!.Table(0, TableExtent);
    }

    /// <summary>A small table, the usual per-draw case.</summary>
    [Benchmark]
    public uint BindSmallTable()
    {
        return _heap!.Table(SmallExtent, SmallExtent);
    }

    /// <summary>Eight root constants written one by one (SetComputeRoot32BitConstants).</summary>
    [Benchmark]
    public void WriteRootConstants()
    {
        for (uint i = 0; i < Constants; i++)
        {
            UserData.Write(_bindings, true, 0, i, i);
        }
    }

    [GlobalCleanup]
    public void Dispose()
    {
        NativeMemory.Free(_title);
        NativeMemory.Free(_window);
        NativeMemory.Free(_bindings);
        NativeMemory.Free(_layout);
        GC.SuppressFinalize(this);
    }
}
