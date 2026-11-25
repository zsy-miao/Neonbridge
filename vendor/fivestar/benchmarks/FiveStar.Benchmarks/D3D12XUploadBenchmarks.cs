// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Reflection;
using System.Runtime.InteropServices;
using BenchmarkDotNet.Attributes;
using FiveStar.Loader.Native;
using FiveStar.Loader.Shims.D3D12X.Binding;

namespace FiveStar.Benchmarks;

/// <summary>
/// A draw's user-data upload (UserDataRing.Upload) into write-combined memory, as the upload heap maps it: the full
/// 128 dwords against the 32 a typical root signature uses. BenchmarkDotNet derives a runner from the class, so it cannot
/// be sealed.
/// </summary>
[MemoryDiagnoser]
public unsafe class D3D12XUploadBenchmarks : IDisposable
{
    /// <summary>PAGE_WRITECOMBINE | PAGE_READWRITE (winnt.h), as an upload heap is mapped.</summary>
    private const uint WriteCombine = 0x404;
    private const uint TypicalDwords = 32;
    private const uint VectorBytes = 32;

    private UserDataRing? _ring;
    private nint _memory;
    private uint* _data;

    [GlobalSetup]
    public void Setup()
    {
        ulong bytes = (ulong)UserDataRing.Blocks * UserDataRing.BlockBytes;
        _memory = Kernel32.VirtualAlloc(0, (nuint)bytes, Win32Constants.MemReserveCommit, WriteCombine);
        var constructor = typeof(UserDataRing).GetConstructors(BindingFlags.NonPublic | BindingFlags.Instance)[0];
        _ring = (UserDataRing)constructor.Invoke([Pointer.Box((void*)_memory, typeof(byte*)), 0UL]);
        _data = (uint*)NativeMemory.AllocZeroed(CommandListBindings.UserDataDwords, sizeof(uint));
    }

    [Benchmark(Baseline = true)]
    public ulong UploadAll()
    {
        return _ring!.Upload(_data, CommandListBindings.UserDataDwords);
    }

    [Benchmark]
    public ulong UploadUsed()
    {
        return _ring!.Upload(_data, TypicalDwords);
    }

    /// <summary>The same copy to one place: what the copy itself costs without walking the ring.</summary>
    [Benchmark]
    public void CopyInPlace()
    {
        Buffer.MemoryCopy(_data, (void*)_memory, UserDataRing.BlockBytes, TypicalDwords * sizeof(uint));
    }

    /// <summary>The copy as whole 32-byte vector stores.</summary>
    [Benchmark]
    public void CopyVectors()
    {
        byte* to = (byte*)_memory;
        byte* from = (byte*)_data;

        for (uint i = 0; i < TypicalDwords * sizeof(uint); i += VectorBytes)
        {
            System.Runtime.Intrinsics.Vector256.Store(System.Runtime.Intrinsics.Vector256.Load(from + i), to + i);
        }
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _ = Kernel32.VirtualFree(_memory, 0, Win32Constants.MemRelease);
        NativeMemory.Free(_data);
        GC.SuppressFinalize(this);
    }
}
