// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using BenchmarkDotNet.Attributes;
using FiveStar.Loader.Native;

namespace FiveStar.Benchmarks;

/// <summary>
/// What one VirtualQuery costs inside a large committed allocation (the game's 1.1 GiB coherent heap) and a large
/// reservation (its 24 GiB page ranges): the address-table miss path asks once per call.
/// BenchmarkDotNet derives a runner from the class, so it cannot be sealed.
/// </summary>
public unsafe class VirtualQueryBenchmarks : IDisposable
{
    private static readonly ulong CommittedBytes = 1UL << 30;
    private static readonly ulong ReservedBytes = 24UL << 30;

    /// <summary>The address asked about: the middle of each range.</summary>
    private const ulong Middle = 2;

    private nint _committed;
    private nint _reserved;

    [GlobalSetup]
    public void Setup()
    {
        _committed = Kernel32.VirtualAlloc(0, (nuint)CommittedBytes, Win32Constants.MemReserve | Win32Constants.MemCommit, Win32Constants.PageReadWrite);
        _reserved = Kernel32.VirtualAlloc(0, (nuint)ReservedBytes, Win32Constants.MemReserve, Win32Constants.PageReadWrite);
    }

    [Benchmark]
    public nuint QueryCommitted()
    {
        MemoryBasicInformation info;

        return Kernel32.VirtualQueryEx(Kernel32.GetCurrentProcess(), _committed + (nint)(CommittedBytes / Middle), &info, (nuint)sizeof(MemoryBasicInformation));
    }

    [Benchmark]
    public nuint QueryReserved()
    {
        MemoryBasicInformation info;

        return Kernel32.VirtualQueryEx(Kernel32.GetCurrentProcess(), _reserved + (nint)(ReservedBytes / Middle), &info, (nuint)sizeof(MemoryBasicInformation));
    }

    [GlobalCleanup]
    public void Dispose()
    {
        _ = Kernel32.VirtualFree(_committed, 0, Win32Constants.MemRelease);
        _ = Kernel32.VirtualFree(_reserved, 0, Win32Constants.MemRelease);
        GC.SuppressFinalize(this);
    }
}
