// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Diagnostics;
using FiveStar.Loader.Native;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// With <c>--watch-pool-pages</c>: the game's small-block pools themselves. A pool page's list link (+8 or +16) that is
/// not a page names its pool's list head at pool +104, so the pools are found from the watched pages. A pool's current
/// page (+144, which 0x144dac530 allocates from) must be 0 or a 16 KiB page; the story-load crash read
/// 0x800000017fffffff there (dwords 0x7FFFFFFF and 0x80000001: the start values of a min/max pass) in a pool at the
/// coherent heap's start. A pool whose current page is no page is logged once with its first bytes and every queue's
/// executions retired since the last check. Reads go through ReadProcessMemory: a pool may lie outside the heaps.
/// </summary>
internal static unsafe class PoolObjectWatch
{
    /// <summary>The pool's list head, its current page and its blocks per page.</summary>
    private const int ListHeadOffset = 104;
    private const int CurrentPageOffset = 144;
    private const int CapacityOffset = 176;
    private const int DumpBytes = 192;
    private const int MaxPools = 4096;
    private const int MaxBlocks = PoolPageHeader.PageBytes / 8;
    private const ulong UserEnd = 0x0000_7FFF_FFFF_FFFF;

    private static readonly HashSet<nint> Pools = [];
    private static readonly HashSet<nint> Reported = [];
    private static long _reports;

    /// <summary>Pools watched and bad current pages found.</summary>
    public static string Summary => string.Create(CultureInfo.InvariantCulture, $"{Pools.Count} pools watched, {Interlocked.Read(ref _reports)} with a current page that is no page");

    /// <summary>Learns pools from the watched pages' links, then checks every pool's current page.</summary>
    public static void Check(Dictionary<nint, (PoolPageHeader Header, bool Confirmed)> pages, ulong[] since, LoaderLog? log)
    {
        foreach (var (header, confirmed) in pages.Values)
        {
            if (confirmed)
            {
                Learn(header.Next);
                Learn(header.Previous);
            }
        }

        foreach (nint pool in Pools)
        {
            if (!TryRead(pool + CurrentPageOffset, out ulong current))
            {
                continue;
            }

            if (IsPage(current))
            {
                Reported.Remove(pool);

                continue;
            }

            if (Reported.Add(pool))
            {
                Report(pool, current, since, log);
            }
        }
    }

    /// <summary>Whether a pool's current page can be this value: none, or a 16 KiB page in user memory.</summary>
    public static bool IsPage(ulong current)
    {
        return current == 0 || (current <= UserEnd && current % PoolPageHeader.PageBytes == 0);
    }

    private static void Learn(ulong link)
    {
        if (link == 0 || link % PoolPageHeader.PageBytes == 0 || link < ListHeadOffset || Pools.Count >= MaxPools)
        {
            return;
        }

        nint pool = (nint)(link - ListHeadOffset);

        if (Pools.Contains(pool) || !TryRead(pool + CapacityOffset, out ulong capacity) || !TryRead(pool + CurrentPageOffset, out ulong current))
        {
            return;
        }

        uint blocks = (ushort)capacity;

        if (blocks >= 1 && blocks <= MaxBlocks && IsPage(current))
        {
            Pools.Add(pool);
        }
    }

    private static void Report(nint pool, ulong current, ulong[] since, LoaderLog? log)
    {
        long count = Interlocked.Increment(ref _reports);
        var text = new StringBuilder();
        text.Append(CultureInfo.InvariantCulture, $"[pool pages] pool 0x{pool:x}: current page 0x{current:x} is no page (#{count}); first bytes:");

        for (int i = 0; i < DumpBytes; i += sizeof(ulong))
        {
            if (TryRead(pool + i, out ulong value))
            {
                text.Append(CultureInfo.InvariantCulture, $" {value:x16}");
            }
        }

        text.Append("\n      executions retired since the last check:");
        QueueProgress.DescribeSince(text, since);
        log?.Warn(text.ToString());
    }

    private static bool TryRead(nint address, out ulong value)
    {
        ulong read = 0;
        nuint done = 0;
        bool ok = Kernel32.ReadProcessMemory(Kernel32.GetCurrentProcess(), address, &read, sizeof(ulong), &done) != 0 && done == sizeof(ulong);
        value = read;

        return ok;
    }
}
