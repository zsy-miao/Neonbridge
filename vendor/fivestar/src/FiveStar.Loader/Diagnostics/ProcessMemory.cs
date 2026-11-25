// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>The process's memory for the sample summary: its private commit and the parts the loader can name.</summary>
internal static unsafe class ProcessMemory
{
    private const double BytesPerGigabyte = 1 << 30;

    /// <summary>
    /// Private commit, the managed heap, the game's XMem allocations through the loader and the physical pages it
    /// holds (pagefile-backed sections, so commit but not private bytes).
    /// </summary>
    public static string Summary(long xmemBytes, long physicalBytes)
    {
        ProcessMemoryCountersEx counters = default;
        counters.Size = (uint)sizeof(ProcessMemoryCountersEx);
        ulong privateBytes = 0;

        if (Kernel32.K32GetProcessMemoryInfo(Kernel32.GetCurrentProcess(), &counters, counters.Size) != 0)
        {
            privateBytes = counters.PrivateUsage;
        }

        return $"memory: private {Gigabytes((long)privateBytes)} ({Gigabytes(WriteCombinedMemory.CachedBytes)} of it write-combined: the driver's mapping of video memory, not RAM), managed heap {Gigabytes(GC.GetTotalMemory(false))}, XMem allocations {Gigabytes(xmemBytes)}, physical pages {Gigabytes(physicalBytes)}";
    }

    /// <summary>
    /// The machine's RAM in use (total less available) and how much of it is outside this process's RAM-backed private
    /// bytes, its physical pages and its helpers (<paramref name="helperBytes"/>: the compile host and a pipeline host),
    /// with the GPU's system-memory use as DXGI reports it (<paramref name="gpuSystem"/>, null when unknown): boot w32 was
    /// ended at 3.9 GB free while those came to 31 GB of 63, everything else rising from 22 to 26 GB.
    /// </summary>
    public static string Machine(long physicalBytes, long helperBytes, ulong? gpuSystem)
    {
        MemoryStatusEx status = default;
        status.Length = (uint)sizeof(MemoryStatusEx);

        if (KernelBase.GlobalMemoryStatusEx(&status) == 0)
        {
            return "machine: unknown";
        }

        ProcessMemoryCountersEx counters = default;
        counters.Size = (uint)sizeof(ProcessMemoryCountersEx);
        long privateBytes = 0;

        if (Kernel32.K32GetProcessMemoryInfo(Kernel32.GetCurrentProcess(), &counters, counters.Size) != 0)
        {
            privateBytes = (long)counters.PrivateUsage;
        }

        long used = (long)(status.TotalPhysical - status.AvailablePhysical);
        long ours = Math.Max(0, privateBytes - WriteCombinedMemory.CachedBytes) + physicalBytes + helperBytes;
        string gpu = string.Empty;

        if (gpuSystem is ulong system)
        {
            gpu = $", GPU system memory {Gigabytes((long)system)}";
        }

        return $"machine: {Gigabytes(used)} of {Gigabytes((long)status.TotalPhysical)} in use, {Gigabytes(ours)} of it this process (RAM-backed), its pages and helpers ({Gigabytes(helperBytes)}), {Gigabytes(used - ours)} everything else{gpu}";
    }

    private static string Gigabytes(long bytes)
    {
        return $"{bytes / BytesPerGigabyte:F1} GB";
    }
}
