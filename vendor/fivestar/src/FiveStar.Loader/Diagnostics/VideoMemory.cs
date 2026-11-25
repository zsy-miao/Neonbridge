// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Diagnostics;

/// <summary>
/// This process's GPU memory per adapter (IDXGIAdapter3::QueryVideoMemoryInfo): local (dedicated) and non-local
/// (shared system memory) usage against the budget. Adapters the process uses nothing on are left out.
/// </summary>
internal static unsafe class VideoMemory
{
    // DXGI_MEMORY_SEGMENT_GROUP.
    private const int LocalGroup = 0;
    private const int NonLocalGroup = 1;

    // DXGI_QUERY_VIDEO_MEMORY_INFO: Budget, CurrentUsage, AvailableForReservation, CurrentReservation.
    private const int InfoWords = 4;
    private const int BudgetWord = 0;
    private const int UsageWord = 1;

    private const double BytesPerGigabyte = 1L << 30;

    public static string Summary()
    {
        Guid id = Dxgi.Factory1Id;
        nint factory = 0;

        if (Dxgi.CreateDXGIFactory1(&id, &factory) < 0)
        {
            return "gpu memory unknown";
        }

        try
        {
            return Adapters(factory);
        }
        finally
        {
            ComObject.Release(factory);
        }
    }

    private static string Adapters(nint factory)
    {
        var enumAdapters = (delegate* unmanaged<nint, uint, nint*, int>)ComObject.Slot(factory, Dxgi.EnumAdapters1Slot);
        var text = new StringBuilder("gpu memory:");

        for (uint index = 0; ; index++)
        {
            nint adapter = 0;

            if (enumAdapters(factory, index, &adapter) < 0)
            {
                return text.ToString();
            }

            try
            {
                Describe(adapter, index, text);
            }
            finally
            {
                ComObject.Release(adapter);
            }
        }
    }

    private static void Describe(nint adapter, uint index, StringBuilder text)
    {
        Guid id = Dxgi.Adapter3Id;
        nint adapter3 = 0;

        if (((delegate* unmanaged<nint, Guid*, nint*, int>)ComObject.Slot(adapter, ComObject.QueryInterfaceSlot))(adapter, &id, &adapter3) < 0)
        {
            return;
        }

        try
        {
            var local = stackalloc ulong[InfoWords];
            var nonLocal = stackalloc ulong[InfoWords];
            var query = (delegate* unmanaged<nint, uint, int, ulong*, int>)ComObject.Slot(adapter3, Dxgi.QueryVideoMemoryInfoSlot);

            if (query(adapter3, 0, LocalGroup, local) < 0 || query(adapter3, 0, NonLocalGroup, nonLocal) < 0 || local[UsageWord] + nonLocal[UsageWord] == 0)
            {
                return;
            }

            text.Append(CultureInfo.InvariantCulture, $" adapter {index}: local {Gigabytes(local[UsageWord])} of {Gigabytes(local[BudgetWord])} budget, non-local {Gigabytes(nonLocal[UsageWord])} of {Gigabytes(nonLocal[BudgetWord])};");
        }
        finally
        {
            ComObject.Release(adapter3);
        }
    }

    private static string Gigabytes(ulong bytes)
    {
        return string.Create(CultureInfo.InvariantCulture, $"{bytes / BytesPerGigabyte:F2} GB");
    }
}
