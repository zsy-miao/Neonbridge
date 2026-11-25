// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// The process's private bytes and the machine's free physical memory and commit, for the thread sampler: a boot uses
/// most of a 64 GB machine (docs/loader.md), so a stall or a device removal is read against these.
/// </summary>
internal static unsafe class MemorySnapshot
{
    private const double BytesPerGigabyte = 1L << 30;

    public static string Summary()
    {
        ProcessMemoryCountersEx counters = default;
        counters.Size = (uint)sizeof(ProcessMemoryCountersEx);
        MemoryStatusEx status = default;
        status.Length = (uint)sizeof(MemoryStatusEx);

        if (Kernel32.K32GetProcessMemoryInfo(Kernel32.GetCurrentProcess(), &counters, counters.Size) == 0 || KernelBase.GlobalMemoryStatusEx(&status) == 0)
        {
            return "memory unknown";
        }

        double committed = (status.TotalPageFile - status.AvailablePageFile) / BytesPerGigabyte;

        return string.Create(
            CultureInfo.InvariantCulture,
            $"memory: {counters.PrivateUsage / BytesPerGigabyte:F1} GB private, {status.AvailablePhysical / BytesPerGigabyte:F1} GB physical free, commit {committed:F1} of {status.TotalPageFile / BytesPerGigabyte:F1} GB");
    }
}
