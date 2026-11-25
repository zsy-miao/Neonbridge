// Copyright (c) 2026 Five Star Recomp contributors.
// SPDX-License-Identifier: GPL-3.0-or-later

using System.Globalization;
using System.Text;
using FiveStar.Loader.Diagnostics;

namespace FiveStar.Loader.Shims.D3D12X.DebugLayer;

/// <summary>
/// <c>--watch-pool-pages</c>: finds the game's small-block pool pages (<see cref="PoolPageHeader"/>) in its coherent
/// graphics memory (XMemVirtualAlloc with PAGE_GRAPHICS_COHERENT, the 1.1 GiB heap where the GPU also keeps frame cells
/// and the transient pool) and checks their headers every <see cref="PollMilliseconds"/> ms. A header the CPU cannot
/// have produced (<see cref="PoolPageHeader.CpuCanBecome"/>, read twice to rule out a race with an allocation) was
/// written by something else, most likely the GPU: the page, its old and new header, its first bytes and every
/// queue's executions retired since the last check (<see cref="QueueProgress.DescribeSince"/>; with
/// <c>--d3d12-breadcrumbs</c> also their last dispatches) are logged, the first <see cref="DetailedReports"/> in full.
/// A page whose counter did not move forward was given back by the allocator and reused for other data: it leaves the
/// watch. The pools themselves are watched too (<see cref="PoolObjectWatch"/>).
/// Every read goes through ReadProcessMemory (an allocation the game frees between scans reads as zeros).
/// The heaps are scanned for pages every <see cref="ScanMilliseconds"/> ms on the same thread; a page found is only
/// reported on once the allocator was seen at work on it (its counter moved), so GPU data that happens to read as a
/// header is not taken for one.
/// </summary>
internal static unsafe class PoolPageWatch
{
    private const int PollMilliseconds = 100;
    private const int ScanMilliseconds = 2000;
    private const int RecheckMilliseconds = 1;
    private const int MaxPages = 16384;
    private const int DetailedReports = 16;
    private const int DumpBytes = 64;
    private const int PreviousOffset = 16;

    /// <summary>Qwords read per header: the free list, the links and the word holding the partial flag.</summary>
    private const int HeaderWords = 5;

    /// <summary>winnt.h (console) PAGE_GRAPHICS_COHERENT.</summary>
    public const uint GraphicsCoherent = 0x20000;

    private static readonly Dictionary<nint, (PoolPageHeader Header, bool Confirmed)> Pages = [];
    private static LoaderLog? _log;
    private static ulong[] _since = [];
    private static long _checks;
    private static long _reports;
    private static int _watched;
    private static int _confirmed;

    /// <summary>Pages watched, header checks made and writes found, for the exit summary.</summary>
    public static string Summary => $"[pool pages] {Volatile.Read(ref _watched)} pages watched ({Volatile.Read(ref _confirmed)} seen in use), {Interlocked.Read(ref _checks)} checks, {Interlocked.Read(ref _reports)} headers written by something other than the CPU's allocator; {PoolObjectWatch.Summary}";

    /// <summary>Whether the watch runs.</summary>
    public static bool Started { get; private set; }

    public static void Start(LoaderLog log)
    {
        _log = log;
        Started = true;
        var thread = new Thread(Watch) { IsBackground = true, Name = "fivestar pool pages" };
        thread.Start();
        log.Info($"[pool pages] watching the small-block pool pages in coherent graphics memory every {PollMilliseconds} ms");
    }

    private static void Watch()
    {
        long scanned = 0;

        while (true)
        {
            long now = Environment.TickCount64;

            if (now - scanned >= ScanMilliseconds)
            {
                Scan();
                scanned = now;
            }

            Check();
            Thread.Sleep(PollMilliseconds);
        }
    }

    /// <summary>Finds the pool pages of every coherent heap: each 16 KiB page whose header reads as one.</summary>
    private static void Scan()
    {
        foreach (var (start, bytes) in ShimContext.Runtime.XMemPhysical.Graphics.Matching(GraphicsCoherent))
        {
            if (!Committed(start, bytes))
            {
                continue;
            }

            for (nint page = start; page + PoolPageHeader.PageBytes <= start + (nint)bytes && Pages.Count < MaxPages; page += PoolPageHeader.PageBytes)
            {
                var header = Read(page);

                if (header.Plausible())
                {
                    Pages.TryAdd(page, (header, false));
                }
            }
        }
    }

    /// <summary>Compares every watched header with the last one seen.</summary>
    private static void Check()
    {
        Interlocked.Increment(ref _checks);

        foreach (var page in Pages.Keys.ToArray())
        {
            var (before, confirmed) = Pages[page];
            var now = Read(page);

            if (before.CpuCanBecome(now))
            {
                // A page is a pool page once the allocator was seen at work on it (its counter moved).
                Pages[page] = (now, confirmed || now.Counter != before.Counter);

                continue;
            }

            // A race with an allocation or a free on another thread reads a torn page: read again.
            Thread.Sleep(RecheckMilliseconds);
            var again = Read(page);

            if (again != now || !confirmed)
            {
                Pages[page] = (again, confirmed);

                continue;
            }

            // A page the allocator gave back is used for other data: watched again only when a scan finds it pooled.
            if (!before.StillPooled(now))
            {
                Pages.Remove(page);

                continue;
            }

            Report(page, before, now);
            Pages[page] = (now, confirmed);
        }

        PoolObjectWatch.Check(Pages, _since, _log);
        _since = QueueProgress.Completed();
        Volatile.Write(ref _watched, Pages.Count);
        Volatile.Write(ref _confirmed, Pages.Values.Count(page => page.Confirmed));
    }

    private static void Report(nint page, PoolPageHeader before, PoolPageHeader now)
    {
        long count = Interlocked.Increment(ref _reports);

        if (count > DetailedReports && (count & (count - 1)) != 0)
        {
            return;
        }

        var text = new StringBuilder();
        text.Append(CultureInfo.InvariantCulture, $"[pool pages] #{count}: page 0x{page:x} header {Describe(before)} became {Describe(now)}, not by the CPU's allocator; first bytes:");

        for (int i = 0; i < DumpBytes; i += sizeof(ulong))
        {
            ulong value = 0;
            Copy(page + i, &value, sizeof(ulong));
            text.Append(CultureInfo.InvariantCulture, $" {value:x16}");
        }

        if (count <= DetailedReports)
        {
            text.Append("\n      executions retired since the last check:");
            QueueProgress.DescribeSince(text, _since);
        }

        _log?.Warn(text.ToString());
    }

    private static string Describe(PoolPageHeader header)
    {
        return string.Create(CultureInfo.InvariantCulture, $"(free 0x{header.FirstFree:x} next 0x{header.FirstNext:x}, count {header.Count}, counter {header.Counter}, links 0x{header.Next:x} 0x{header.Previous:x}, partial {header.Partial})");
    }

    /// <summary>The header at <paramref name="page"/> and the next-offset of its first free block.</summary>
    /// <remarks>
    /// Read through the kernel (ReadProcessMemory): the game may free a coherent allocation between two scans, and a
    /// plain load there faulted this thread and ended boot 17. Memory that cannot be read reads as zeros.
    /// </remarks>
    private static PoolPageHeader Read(nint page)
    {
        ulong* header = stackalloc ulong[HeaderWords];
        Copy(page, header, HeaderWords * sizeof(ulong));
        ulong free = header[0];
        uint first = (uint)free;
        uint next = 0;

        if (first != 0 && first <= PoolPageHeader.PageBytes - sizeof(uint))
        {
            Copy(page + (nint)first, &next, sizeof(uint));
        }

        byte partial = ((byte*)header)[PoolPageHeader.PartialOffset];

        return new PoolPageHeader(free, header[1], header[PreviousOffset / sizeof(ulong)], partial, next);
    }

    private static void Copy(nint from, void* to, int bytes)
    {
        nuint done = 0;

        if (Native.Kernel32.ReadProcessMemory(Native.Kernel32.GetCurrentProcess(), from, to, (nuint)bytes, &done) == 0 || done != (nuint)bytes)
        {
            new Span<byte>(to, bytes).Clear();
        }
    }

    /// <summary>Whether the whole range is still committed memory (one VirtualQuery per region, at each scan).</summary>
    private static bool Committed(nint start, ulong bytes)
    {
        nint at = start;

        while (at < start + (nint)bytes)
        {
            Native.MemoryBasicInformation info;

            if (Native.Kernel32.VirtualQueryEx(Native.Kernel32.GetCurrentProcess(), at, &info, (nuint)sizeof(Native.MemoryBasicInformation)) == 0 || info.State != Native.Win32Constants.MemCommit)
            {
                return false;
            }

            at = info.BaseAddress + (nint)info.RegionSize;
        }

        return true;
    }
}
