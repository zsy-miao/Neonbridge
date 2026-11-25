// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The committed private write-combined memory (protection 0x404) and how much of it is in 32 MiB regions. On the RTX
/// 4090 it is mostly the driver's CPU mapping of video memory: 400 default-heap textures of 1 MB added 400 MB here, to
/// private bytes and to GPU local memory alike, and nothing to the working set (a cold boot's grew from 5 to 12.6 GB
/// after the story load, step for step with GPU local memory). It is charged to commit, not backed by RAM. One walk of the address space (VirtualQueryEx, 0.4 to 1 s at 250,000
/// regions) per sample summary; <see cref="CachedBytes"/> is refreshed by its own timer for the compile ceiling.
/// </summary>
internal static unsafe class WriteCombinedMemory
{
    // MEMORY_BASIC_INFORMATION (winnt.h): MEM_COMMIT, MEM_PRIVATE, PAGE_READWRITE | PAGE_WRITECOMBINE.
    private const uint CommitState = 0x1000;
    private const uint PrivateType = 0x20000;
    private const uint WriteCombinedReadWrite = 0x404;
    private const long PoolRegionBytes = 32L << 20;
    private const long UserSpaceEnd = 0x7FFF_FFFF_0000;
    private const double BytesPerGigabyte = 1 << 30;

    private static readonly TimeSpan RefreshInterval = TimeSpan.FromSeconds(15);
    private static readonly Lazy<Timer> Refresher = new(() => new Timer(_ => Refresh(), null, TimeSpan.Zero, RefreshInterval));
    private static long _cached;

    /// <summary>The write-combined bytes at the last walk, at most 15 s old; the first read starts the walks.</summary>
    public static long CachedBytes
    {
        get
        {
            _ = Refresher.Value;

            return Interlocked.Read(ref _cached);
        }
    }

    /// <summary>"write-combined: G GB in N regions, M of 32 MiB".</summary>
    public static string Summary()
    {
        var (bytes, regions, pools) = Walk();
        Interlocked.Exchange(ref _cached, bytes);

        return $"write-combined: {bytes / BytesPerGigabyte:F1} GB in {regions} regions, {pools} of 32 MiB";
    }

    private static void Refresh()
    {
        Interlocked.Exchange(ref _cached, Walk().Bytes);
    }

    private static (long Bytes, int Regions, int Pools) Walk()
    {
        nint process = Kernel32.GetCurrentProcess();
        nint address = 0;
        long bytes = 0;
        int regions = 0;
        int pools = 0;
        MemoryBasicInformation info;

        while (address < UserSpaceEnd && Kernel32.VirtualQueryEx(process, address, &info, (nuint)sizeof(MemoryBasicInformation)) != 0)
        {
            if (info.State == CommitState && info.Type == PrivateType && info.Protect == WriteCombinedReadWrite)
            {
                bytes += (long)info.RegionSize;
                regions++;

                if ((long)info.RegionSize == PoolRegionBytes)
                {
                    pools++;
                }
            }

            address = info.BaseAddress + (nint)info.RegionSize;
        }

        return (bytes, regions, pools);
    }
}
